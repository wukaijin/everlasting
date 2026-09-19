<!-- Moved from permission-layer.md 2026-09-19 (doc-split) -->

### 5c. `request_mode_change` 写操作 IPC 链路

> 2026-07-07 落地。`request_mode_change` 是 `ask_user_question` 的
> **写操作 sibling** —— 用户二选一 allow/deny,允许路径修改
> `sessions.mode` + 走 `set_session_mode` 的全套副作用(Yolo 二次
> modal / root guard / audit)。本节是**跨层契约**,跟 §5b
> worker ask round-trip 同性质(后端 IPC 链 + 前端 store 决策
> 路由)。

#### Scope / Trigger

主 agent 在 turn N 通过 LLM `tool_use` 申请切换 `sessions.mode`,
user 通过 inline card 决策。**允许**路径必须复用 `set_session_mode`
IPC 的副作用链(DB 持久化 + 审计 + Yolo 安全守门 + 模式 prefix
切换),**不**在 tool 内部独立调 `db::update_session_mode` —— 避免
双路径漂移(详见 `tool-contract/11-request-mode-change.md §"Scenario: request_mode_change
tool" §8 Design Decisions`)。Tool 内部仅 `store.register`(oneshot 互斥)
+ `emit_mode_change_request` + `tokio::select!{cancel, oneshot}` 等
决策,**不直接落库**。

#### IPC 链(双 IPC 顺序固定)

```
LLM tool_use (request_mode_change, target_mode, reason)
    │
    ▼
chat_loop.rs 拦截 → request_mode_change::execute_blocking(...)
    │
    │ (1) schema validate
    │ (2) noop check (target == current → 立即返回)
    │ (3) record_audit("mode_change_requested", { target_mode, reason })
    │ (4) store.register(payload)        ← 共用 QuestionStore
    │ (5) sink.emit_mode_change_request  ← 新 IPC event "mode:change:request"
    │ (6) tokio::select!{ cancel | oneshot.recv() }
    │
    ▼  user 在 card 上点"允许"
frontend <RequestModeChangeCard> → resolve_mode_change IPC(allow=true)
    │
    ▼
backend commands::permissions::resolve_mode_change
    │
    │ (7) is_running_as_root + target_mode == yolo?
    │     是 → 走"拒绝"路径(audit "mode_change_denied" + 返回 Err)
    │ (8) 调 set_session_mode 内部函数
    │     → db::update_session_mode(pool, session_id, target_mode)
    │     → record_audit("mode_changed", { prev, new }) ← DB 自动产生
    │ (9) 调 store.resolve(sid, Allow) ← 让 agent loop oneshot 解除
    │ (10) record_audit("mode_change_allowed", { prev, new, target })
    │ (11) return SessionRow
    │
    ▼
backend execute_blocking 拿到 Allow:
    tool_result = {"allowed": true, "prev_mode": "...", "new_mode": "..."}
    │ (is_error: false)
    ▼
chat_loop 推 ContentBlock::ToolResult
    │
    ▼
LLM 下一轮 system prompt 已是 Edit / Plan / Yolo
```

**关键不变量**:`store.resolve` 必须在 `db::update_session_mode`
**之后**调用,否则 agent loop 先收到 Allow 但 DB 未落库,出现
不一致(详见 `tool-contract/11-request-mode-change.md §8 Design Decisions` 风险段)。

#### Yolo 二段路径(双 IPC 顺序的特殊性)

申请 `yolo` 走**三段**而非两段:

```
1. user 点 card "允许" → 不直接调 resolve_mode_change
                       → dispatch 到 useChatStore.requestSetMode(sid, "yolo")
                       → 设 pendingYoloConfirm = true
                       → 弹 Yolo 二次 modal
2a. user 在 modal 点"确认" → confirmYolo action
    → set_session_mode IPC(sid, "yolo")  [IPC A — 落库 + audit "mode_changed"]
    → set_session_mode handler 检测"card dispatch 路径"
    → 额外调 store.resolve(sid, Allow)
    → 额外 record_audit("mode_change_allowed", ...)
    → resolve_mode_change IPC(sid, toolUseId, allow=true) [IPC B — 解 oneshot]
    注:resolve_mode_change handler 检测"已在 set_session_mode 阶段 resolve"
        → 二次 resolve 是 no-op(AlreadyResolved 错误码,warn log,return Ok)
2b. user 在 modal 点"取消" → cancelYolo action
    → 走"拒绝"路径:resolve_mode_change IPC(sid, toolUseId, allow=false)
    → record_audit("mode_change_denied", { reason: "yolo_cancelled_confirm" })
    → tool_result = {"cancelled_by_user": true, ...}
```

**根守卫**(`is_running_as_root`):即使 user 在 card 上点"允许 Yolo",
二次 modal 的"确认"按钮被 `disabled` + 红字 "Cannot enable Yolo as
root"。点击无效 → 等价走 2b 路径(audit `mode_change_denied
{ reason: "yolo_root_guard" }`)。

#### 跟 §5b worker ask 的对照

| 维度 | §5b worker ask | §5c request_mode_change |
|---|---|---|
| Trigger | worker tool_use 触发 Tier 4 ask | 主 session LLM tool_use 触发 |
| IPC 通道 | `permission:ask` | `mode:change:request` |
| 决策类型 | 3 选 1 (allow_once / allow_always / deny) | 2 选 1 (allow / deny) |
| 副作用 | 放行 tool 执行 + 可选写 `session_tool_permissions` | 落库 `sessions.mode` + 写 `mode_change_*` 3 类 audit |
| Yolo 处理 | 不适用 | 二次 modal 守门 |
| Session cancel | child_token 触发,worker oneshot 解 | parent token 触发,store remove + cancelled tool_result |
| Worker 是否适用 | 适用(WorkerAskBanner) | **不适用**(`STRUCTURALLY_DISABLED`) |

#### Validation & Error Matrix

| 条件 | 结果 |
|---|---|
| user Allow(IPC 返 Ok + SessionRow) | store.resolve(Allow) → agent loop 拿 `{"allowed": true, "prev_mode", "new_mode"}` |
| user Deny(IPC 返 Ok) | store.resolve(Cancelled) + `mode_change_denied` audit + tool_result `{"cancelled_by_user": true}` |
| is_running_as_root + target=yolo | 走 Deny 路径,audit `mode_change_denied { reason: "yolo_root_guard" }` |
| session cancel | cancel arm → store remove + tool_result `{"cancelled_by_session": true}` |
| `map.contains_key(session_id)` 已有 pending | `QuestionStoreError::AlreadyPending` → `{"error": "已有 pending interaction, 等当前处理完成"}` |
| `target_mode == current_mode` (noop) | 立即 `{"noop": true, "current_mode": "..."}`,不挂 store,不发 IPC |
| 重复 invoke `resolve_mode_change`(rid 已 resolve) | Err(AlreadyResolved) + warn log,no-op(防双 resolve 误写 audit) |
| `get_pending_interaction` 不存在 session | `None` |
| `get_pending_interaction` 已有 pending ask_user_question | `Some({ kind: "question", payload: ... })` |
| `get_pending_interaction` 已有 pending mode change | `Some({ kind: "mode_change", payload: ... })` |
| `get_pending_interaction` resolve 后 | `None` |

#### Tests Required

`commands/tests_resolve_mode_change.rs`(5 个)+ `commands/tests_get_pending_interaction.rs`(4 个)+
`permissions/tests_audit.rs` round-trip(ModeChangeRequested / Allowed / Denied 3 类)+ `permissions/tests_request_mode_change.rs`(11 个单测)。详见
[tool-contract/11-request-mode-change.md §"Scenario: request_mode_change tool" §6](../tool-contract/11-request-mode-change.md)
完整测试矩阵 + PRD `.trellis/tasks/07-07-07-07-request-mode-change-tool/`。

