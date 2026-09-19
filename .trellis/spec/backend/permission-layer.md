<!-- Per-Session Mode + ⑨ 关 Permission Layer scenario. Moved from llm-contract.md 2026-06-21 (A2 + B7, 2026-06-13) -->

# Per-Session Mode + ⑨ 关 Permission Layer (A2 + B7, 2026-06-13)

> **Source**: extracted from `.trellis/spec/backend/llm-contract.md` §"Scenario: Per-Session Mode + ⑨ 关 Permission Layer" (2026-06-21 doc-trim task).
>
> **Cross-references**:
> - Main LLM contract: [llm-contract.md](./llm-contract.md)
> - ⑨ 关 tool-side contract (hard kill list + IPC + path-based): 见本文 §4(5-Tier)+ §5(permission:ask IPC)(2026-06-21 从 tool-contract.md 迁入)

## Scenario: Per-Session Mode + ⑨ 关 Permission Layer (A2 + B7, 2026-06-13)

### 1. Scope / Trigger

The ⑨ 关 permission layer is the unified decision point between
the agent loop's `provider.send()` stream and `tools::execute_tool`.
PR1 implemented the backend half (per the merged A2 + B7 task
`06-12-a2-b7-permission-and-mode`); PR3 wired the frontend
PermissionModal + `usePermissionsStore`. This section captures
the cross-layer contract that touches the LLM stream — the
per-turn system prompt prefix (⑧a first defense), the
per-turn tool list filter (⑧a second defense), the 5-tier
permission decision (⑨ 关), and the `permission:ask` IPC that
backs it. Changes here cascade through the agent loop,
Tauri commands, Pinia store, and the Vue modal.

### 2. Mode enum (per-session)

The active session carries a `Mode` enum value bound to
`sessions.mode` (TEXT column, nullable, backfilled to `chat`).
Five variants, but only four are user-facing — `Background` is
reserved in the enum for schema stability and is never exposed
in the UI (PR2 decision).

| Mode | UI | Tool execution | User confirm | Notes |
|---|---|---|---|---|
| `Edit` | ✓ | full | dangerous-tool ask | Default (2026-06-13 rename from `Chat`); matches Claude Code `default` |
| `Plan` | ✓ | read-only (write tools filtered) | — | ⑧a system-prompt + tool-list filter + runtime intercept |
| `Yolo` | ✓ | full (skip Tier 3 ask) | none | Hard kill list (Tier 2) still enforced |
| `Background` | ✗ (enum only) | n/a | n/a | Reserved; future |

`Mode` serializes lowercase (`"edit"` / `"plan"` / `"yolo"`
/ `"background"`) on the IPC wire. (2026-06-13: `"chat"`
renamed to `"edit"`, `"review"` removed; breaking wire
change.)

### 3. ⑧a Triple Defense (Mode interception)

The mode check is enforced in three layers, mirroring the
Claude Code `--permission-mode plan` design (see
`research/mode-state-machine.md §Q3 "Design B"`):

1. **Per-turn system prompt prefix** —
   `app/src-tauri/src/agent/permissions::mode_system_prefix(mode)`
   returns a per-mode instruction string that the agent loop
   prepends to the system prompt before every `provider.send()`
   call. The string tells the LLM in plain English what the
   mode allows (e.g. "you may read files but CANNOT execute
   write tools"). This is the cheapest layer; a well-instructed
   LLM never attempts a forbidden tool.
2. **Per-turn tool list filter** —
   `permissions::filter_tools_for_mode(tools, mode)` returns a
   filtered tool list. `Plan` drops `write_file`,
   `edit_file`, `shell`; `Edit` / `Yolo` keep the full set.
   The filtered list is what the LLM sees in the request body
   `tools` field — LLM doesn't even know the forbidden tools
   exist in those modes.
3. **⑧a runtime intercept** — even with layers 1 + 2, the
   LLM may still emit a forbidden `tool_use` (prompt-injection
   attack, model regression, etc.). The agent loop's Tier 4
   in `permissions::check` catches this: if
   `Mode::Plan` and the tool is in the
   write-block list, the loop returns
   `Decision::Deny { reason: "I cannot execute X in Y mode
   (read-only session)", critical: false }` and the agent loop
   appends an `is_error: true` tool_result for the LLM to
   self-correct.

> **分篇**(2026-09-19):本文保留 §1-§3(Scope / Mode enum / ⑧a 三层防御)与 §7-§8(IPC 异常路径 / Tests Required);§4-§6 已按 tool-contract 模式拆至 `permission-layer/` 子目录(一节一文件,原锚点以 stub 保留)。

### 4. ⑨ 关 5-Tier Decision Order

> **已拆出**(2026-09-19 doc-split):完整契约(含 4.1 5-tier 重排 + path-based 决策 / 4.2 Tier 1 Hooks P3 工具执行前召回 / 4.3 RULE-PERM-002 grant 入口校验 + prefix 读侧消费)见 [`permission-layer/five-tier-decision-order.md`](./permission-layer/five-tier-decision-order.md)。

### 5. ⑨ 关 ↔ `permission:ask` IPC 协议

> **已拆出**(2026-09-19 doc-split):完整契约见 [`permission-layer/permission-ask-ipc.md`](./permission-layer/permission-ask-ipc.md)。

### 5b. Worker (subagent) tool_use — interactive ask round-trip

> **已拆出**(2026-09-19 doc-split):完整契约(三端一致 Contracts / B1 死代码教训 / audit outcome transcript entry)见 [`permission-layer/worker-subagent-ask.md`](./permission-layer/worker-subagent-ask.md)。

### 5c. `request_mode_change` 写操作 IPC 链路

> **已拆出**(2026-09-19 doc-split):完整契约(双 IPC 顺序固定 / Yolo 二段路径 / 与 §5b worker ask 对照)见 [`permission-layer/request-mode-change-ipc.md`](./permission-layer/request-mode-change-ipc.md)。

### 6. Audit (`session_audit_events`) — 20 类 AuditKind(2026-07-07 +3 增)

> **已拆出**(2026-09-19 doc-split):完整契约见 [`permission-layer/audit-events.md`](./permission-layer/audit-events.md)。

### 7. ⑨ 关 IPC 异常路径

| 异常场景 | 处理 |
|---|---|
| 用户从不响应 (>120s) | 后端 `tokio::time::sleep(ASK_TIMEOUT)` 触发 → 自动 deny + `is_error: true, content: "permission timed out after 120s, treat as denied"`(提醒 LLM 是超时不是 user 主动)。前端 store 也复制 120s timer 来关 modal + 弹 toast。**2026-09-03**(task `09-03-ask-no-timeout`):全局开关 `ask_no_timeout`(app_config,enable 语义 fail-closed 缺省关,单源 `permissions::ask::ASK_NO_TIMEOUT_KEY`)开 → timeout 臂替换为 `pending()` 恒不触发,审批卡无限挂起直到用户响应 / Stop / 删会话;前端 `usePermissionsStore` 同开关下不 arm 本地 timer。轮数上限软卡(`chat_loop.rs` softcap 600s auto-stop)同受此开关管辖。无人值守定时会话同样生效 —— 开关开即无限等待(产品决策,见 task PRD)。 |
| 重复 `permission_response` | 后端 `HashMap<rid, Sender>`:`send().ok()` 失败(rid 不存在)=> 返回 `Ok(false)`,日志 warn。no-op。 |
| Session 在等待时被删除 | 当前 MVP `cancel_session_asks` 实现是"清空所有 pending"(rid 没绑 session_id);这会让所有 session 的 oneshot 失败。后续 PR 应改成 `HashMap<(session_id, rid), Sender>` 精细清理。 |
| `rid` 过期/无效 | 后端校验 rid 存在性,无效 → 日志 warn + no-op + 返回 `Ok(false)`。 |
| `mode` 字段在 wire 中是 `background` (enum 保留值) | 后端 lenient parse → 翻 `chat`;`set_session_mode` 不报错(用 Chat 写回)。 |

### 8. Tests Required

**Backend (`cargo test`)**(PR1 已有,PR3 不新增):

| Test | Asserts |
|---|---|
| `permissions::tests::risk_for_tool_categorization` | per-tool static map correct |
| `permissions::tests::risk_label_cn_is_full_text` | 中文 label 完整 |
| `permissions::tests::mode_as_str_round_trip` | 5 个 mode 都 round-trip |
| `permissions::tests::mode_from_str_unknown_defaults_to_chat` | lenient parse |
| `permissions::tests::filter_tools_for_mode_drops_writes_in_plan` | ⑧a tool filter |
| `permissions::tests::filter_tools_for_mode_keeps_full_for_chat_yolo` | ⑧a full tool list |
| `permissions::tests::mode_system_prefix_is_non_empty` | 5 个 mode 都有 prefix |
| `permissions::tests::audit_kind_round_trip` | 20 类 AuditKind 都 serializable |
| `permissions::dangerous::tests::kill_list_blocks_rm_rf_root` | Tier 2 命中 |
| `permissions::dangerous::tests::kill_list_blocks_fork_bomb` | Tier 2 命中 |
| `permissions::dangerous::tests::kill_list_normal_dev_commands_pass` | Tier 2 不误杀 |
| ...(总计 20 个 permission 测试,见 PR1 落地) |

**Frontend (`pnpm test`)**(PR3 新增):

| Test | Asserts |
|---|---|
| `usePermissionsStore — start() registers a permission:ask listener` | listener 注册 |
| `usePermissionsStore — setPending populates pendingPermission` | 写入 slot |
| `usePermissionsStore — a new ask replaces the prior` | 单 slot 语义 |
| `usePermissionsStore — respond fires permission_response IPC` | IPC wire |
| `usePermissionsStore — respond with allow_always / deny` | 3 decision 字符串 |
| `usePermissionsStore — respond does NOT clear pendingPermission if rid doesn't match` | race-guard |
| `usePermissionsStore — 120s timer fires deny + toast` | ASK_TIMEOUT_MS |
| `usePermissionsStore — stop() tears down the listener + clears state` | lifecycle |
| `PermissionModal — renders 3 buttons when ask pending` | UI |
| `PermissionModal — clicking 拒绝/仅一次/始终允许 calls store.respond` | 3 button wire |
| `PermissionModal — Esc / X / backdrop click → deny` | Q6 spec |
| `PermissionModal — Enter on non-critical → allow_once; critical → deny` | audit §6.2 |
| `PermissionModal — critical modifier class when risk==='critical'` | 3px 红 border |
| `PermissionModal — copy button writes JSON to clipboard` | UX |
| `PermissionModal — Chinese risk label per level` | audit §6.2 |
