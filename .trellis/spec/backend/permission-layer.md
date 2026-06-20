<!-- Per-Session Mode + ⑨ 关 Permission Layer scenario. Moved from llm-contract.md 2026-06-21 (A2 + B7, 2026-06-13) -->

# Per-Session Mode + ⑨ 关 Permission Layer (A2 + B7, 2026-06-13)

> **Source**: extracted from `.trellis/spec/backend/llm-contract.md` §"Scenario: Per-Session Mode + ⑨ 关 Permission Layer" (2026-06-21 doc-trim task).
>
> **Cross-references**:
> - Main LLM contract: [llm-contract.md](./llm-contract.md)
> - ⑨ 关 tool-side contract (hard kill list + IPC + path-based): [tool-contract.md](./tool-contract.md) §"Scenario: Path-based Permission Layer"

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
change — see ADR in `docs/IMPLEMENTATION.md §4`.)

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

### 4. ⑨ 关 5-Tier Decision Order

PR1's `agent::permissions::check` runs a 5-tier evaluation
in this exact order (SOT, matches Claude Code's
`deny > ask > mode > allow` rule from `permissions.md`):

```
Tier 1. Hooks           — pre-call interface (MVP: no-op)
       │ 命中 hook override? → 用 hook 决定(本期不实现)
       ↓
Tier 2. Deny rules      — hard kill list (dangerous::is_kill_listed)
       │ 命中 → 直接 Decision::Deny { critical: true, reason: ... }
       │ Yolo 模式也走 — 静默拒绝,不弹窗
       │ → Tier 6 写 audit (kind="tool_denied" 或 "tool_denied_yolo")
       ↓
Tier 3. Ask rules       — session_tool_permissions + emit + await
       │ 查 session_tool_permissions:
       │   有 "始终允许" 记录 → 跳过弹窗, 直接 Allow
       │   无 → emit("permission:ask", { rid, tool_name, tool_input, risk, reason? })
       │       等前端 permission_response (120s 超时 → 自动 Deny)
       │ 收到响应:
       │   allow_once  → 放行(不写表)
       │   allow_always → 放行 + INSERT INTO session_tool_permissions
       │   deny        → Deny { reason: "user denied" }
       │   timeout     → Deny { reason: "permission timed out after 120s, treat as denied" }
       │                + audit kind="permission_timeout"
       ↓
Tier 4. Mode check      — Plan 拦截 (3 档化 2026-06-13: Review 移除)
       │ Plan 模式 + tool ∈ {write_file, edit_file, shell}
       │ → Deny { reason: "I cannot execute X in Plan mode" }
       ↓
Tier 5. Allow rules     — 默认 allow-all (MVP 阶段)
       ↓
Tier 6. Audit hook      — 每个决策路径写 session_audit_events
       │ kind: tool_allowed / tool_denied / tool_permission_ask /
       │       permission_granted / permission_timeout / request_cancelled
       ↓
   → execute_tool(若 Allow) / 构造 is_error tool_result(若 Deny)
```

**关键行为**:
- **Deny 优先于 Ask**:`rm -rf /` 在 Yolo 模式下也是静默拒绝
  (Tier 2 在 Tier 3 之前)
- **Tier 3 拒绝 ≠ Cancel 整轮**:Deny 只跳该 tool_use,LLM 收到
  `is_error: true` 可自决;CancellationToken (C1) 才是整轮终止
- **超时 vs 主动 deny** 在 audit log 区分:`reason` 字段不同
  ("user denied" vs "permission timed out after 120s, treat as denied")

#### 4.1. Re-grill update 2026-06-13: 5-tier 重排 + path-based 决策

> Supersedes 4 节上述旧设计。Source of truth:
> `.trellis/tasks/06-13-a2-b7-regrill-path-based/prd.md` §1。

旧设计 ⑨ 关 Tier 3 "总是弹窗" 在 Edit 模式下读 README
都要弹,反直觉;Tier 4 Mode check 在 Ask 之后让 Plan
模式下"用户点始终允许,然后被 Mode 拒"成为坏交互。
re-grill 锁定 10 决策,把决策层重构:

**新 Tier 顺序**:

```
Tier 1. Hooks           (MVP no-op)
Tier 2. Deny rules      (硬 kill list,shell 9 个 regex,Yolo 走 — 静默拒)
Tier 3. Mode check      (Plan 拦截 write/edit/shell,text 错,不发 modal)
Tier 4. Path / Prefix / External policy
       ├─ Path 工具:is_within_root → 查 session_tool_permissions
       │   (match_kind='path') → hit Allow / miss silent(in) / miss ask(out)
       ├─ Shell:classify_prefix → Allow(whitelist) / Ask(asklist + 未知)
       └─ Web Fetch:查 match_kind='tool' for 'web_fetch' → hit Allow / miss ask
       (Yolo:整段 bypass,直接 Allow;Tier 2 仍 hard wall)
Tier 5. Allow rules     (default allow-all)
Tier 6. Audit           (写 session_audit_events)
```

**跟旧设计 diff**:

| 改动 | 旧 (PR1) | 新 (re-grill) |
|---|---|---|
| Tier 顺序 | Hooks → Deny → Ask → Mode → Allow → Audit | Hooks → Deny → **Mode → Path/Prefix** → Allow → Audit |
| 弹窗判定 | risk 等级 + 总是弹 | **path-based**:仓库内 silent,仓库外 ask |
| Mode check 时机 | Tier 4(在 Ask 之后) | **Tier 3(在 Ask 之前)** — 消除 Plan + 始终允许坏交互 |
| "始终允许"持久化 | 只 `tool` | **3 种 match_kind: tool + path-glob + prefix** |
| shell 策略 | 总是 Tier 3 | **白名单/asklist/未知 三档**(prefix 解析) |
| Yolo × 仓库外 | 走 Tier 3 modal | **silent**(Yolo bypass Tier 4) |
| Tier 2 kill list | 9 regex | **不变** |
| `PermissionAskPayload` | rid + tool + input + risk + reason | + **`path: Option<String>`** (新, `skip_serializing_if`) |
| `Risk` 字段 | 4 档 | 不变(4 档,UI 视觉) |

详细 ⑨ 关 contract 见 `tool-contract.md §"Scenario: Path-based
Permission Layer"`,包括 `shell_trust::classify_prefix` 的
whitelist / asklist 完整表。

### 5. ⑨ 关 ↔ `permission:ask` IPC 协议

**Server → Client**:后端 `agent::permissions::check` Tier 3 发:

```rust
app.emit("permission:ask", &PermissionAskPayload {
    rid: String,                // UUID
    tool_name: String,
    tool_input: serde_json::Value,
    risk: Risk,                  // Low | Medium | High | Critical (lowercase)
    reason: Option<String>,      // 人类可读原因
});
```

`PermissionAskPayload` uses `#[serde(rename_all = "camelCase")]`,
producing the wire shape:

```jsonc
{
  "rid": "uuid",
  "toolName": "shell",
  "toolInput": { "command": "ls -la" },
  "risk": "high",
  "reason": "The tool shell requires your confirmation (risk: 高)."
}
```

**Client → Server**:前端 `usePermissionsStore.respond(rid, decision)`:

```typescript
invoke("permission_response", { rid: "uuid", decision: "allow_once" | "allow_always" | "deny" })
```

后端 `commands::permissions::permission_response` (Tauri command)
查 `PermissionStore: Arc<Mutex<HashMap<rid, oneshot::Sender>>>`,
发响应到 `check()` 正在 await 的 oneshot,唤醒后续逻辑。

**Wire invariant**:frontend `respond(rid)` 必须用后端 emit
`permission:ask` 时附带的 `rid` — 后端 `HashMap<rid, Sender>`
的 key 是 emit 时刻的 UUID。客户端不能自己生成 rid;只能转发
后端给的 rid + decision。

### 6. Audit (`session_audit_events`) — 10 类 AuditKind

PR1 在 `agent::permissions::AuditKind` 实现了 10 类事件(见
`audit §3.4` 完整列表)。`payload_json` 字段统一结构:

```json
{
  "tool_name": "shell",
  "tool_input": { "command": "ls -la" },
  "reason": "matches denylist: rm -rf /",
  "mode": "edit",
  "critical": true
}
```

`critical: bool` 字段对前端 `PermissionModal` 的 3px 红左 border
+ shield-x icon 渲染至关重要(PR1 follow-up 加,PR3 使用)。

| AuditKind | 触发条件 |
|---|---|
| `tool_denied` | Tier 2 命中 + Tier 3 user deny + Tier 3 sender dropped |
| `tool_allowed` | Tier 3 AllowOnce / Tier 3 "始终允许" 命中 / Tier 5 默认 |
| `tool_permission_ask` | Tier 3 emit `permission:ask` |
| `permission_granted` | Tier 3 "始终允许" → 写 `session_tool_permissions` |
| `permission_timeout` | Tier 3 120s 超时 |
| `tool_denied_yolo` | Tier 2 命中 + mode = Yolo(audit 跟普通 tool_denied 区分) |
| `mode_changed` | `set_session_mode` 调用 |
| `yolo_entered` / `yolo_exited` | Mode 在 Yolo 之间切换 |
| `request_cancelled` | C1 cancel 触发(tier 3 await 被 cancel 打断) |

### 7. ⑨ 关 IPC 异常路径

| 异常场景 | 处理 |
|---|---|
| 用户从不响应 (>120s) | 后端 `tokio::time::sleep(ASK_TIMEOUT)` 触发 → 自动 deny + `is_error: true, content: "permission timed out after 120s, treat as denied"`(提醒 LLM 是超时不是 user 主动)。前端 store 也复制 120s timer 来关 modal + 弹 toast。 |
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
| `permissions::tests::audit_kind_round_trip` | 10 类 AuditKind 都 serializable |
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