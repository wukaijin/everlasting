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
Tier 2. Deny rules      (硬 kill list,shell 10 个 regex,Yolo 走 — 静默拒)
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
| Tier 2 kill list | 9 regex → 10 regex | **不变**(RULE-B-004 加 find -delete/-exec 后 10 条) |
| `PermissionAskPayload` | rid + tool + input + risk + reason | + **`path: Option<String>`** (新, `skip_serializing_if`) |
| `Risk` 字段 | 4 档 | 不变(4 档,UI 视觉) |

详细 ⑨ 关 contract 见 `tool-contract.md §"Scenario: Path-based
Permission Layer"`,包括 `shell_trust::classify_prefix` 的
whitelist / asklist 完整表。

#### 4.2. Tier 1 Hooks 实际实现路径 — P3 工具执行前召回(2026-06-29, 06-29-am-p3-tool-recall)

> **P3 收口前** Tier 1 Hooks 一直是 no-op(MVP 留口);P3 落地时**没有**改
> `permissions::check()` 内部(保持 5-tier 拦截链纯净),而是把"工具执行前
> 召回 pitfall"挂到了 `chat_loop.rs` 的"check → execute_tool"seam 上。

- **函数**:`agent::permissions::recall_pitfall_footnote(pool, tool_name, tool_input) -> Result<Option<String>, sqlx::Error>`
- **调用点**:`chat_loop.rs` parallel-batch L2 path(line ~1792) + serial path(line ~2361)
  - 时机:`permissions::check()` 返回 `Allow` **之后**、`execute_tool()` **之前**
  - 不走 `check()` 内部 → 5-tier 拦截链顺序未被打乱;P3 recall 是旁路
- **行为**:
  - 调 `db::memories::find_pitfalls_by_trigger(pool, tool_name, command, path)`(P1 产出)
  - 过滤 `status == 'active'`(`verified` 留 P5 软拦截,本阶段不消费)
  - 命中后构造 `⚠️ Memory: 此前在本项目执行类似操作时踩过坑 —\n• [title] content\n...` 注脚文本
  - `bump_hit_count` 走 `tokio::spawn` fire-and-forget,不阻塞 recall 步骤
  - 注脚 prepend 到 `tool_result.content` **在 envelope wrap 之前** — `tool_use_id` 配对与 `is_error` 语义不变
- **降级**:`Err(sqlx::Error)` → `tracing::warn!` + 返回 `None`,工具照常执行。**Recall failure 永不阻断工具执行**(PRD hard rule)
- **Decision 语义**:`check()` 仍返回 `Decision::Allow/Ask/Deny`,recall 仅产出 `Option<String>` 注脚;不参与决策链

**为什么挂在 seam 而非 `check()` 内部**:
1. **5-tier 纯净性**:`check()` 是决策层,recall 是"信息注入",职责不同
2. **P5 扩展性**:P5 的 verified 软拦截需要返回结构化 `Decision`,可直接进 `check()` Tier 1;P3 的 active 注脚是旁路 hint,放 seam 简化 P5 落地
3. **可测性**:`recall_pitfall_footnote` 是纯函数(pool + 字符串入参 → `Result<Option<String>>`),`tests_check.rs` 直接单测无需 mock 决策链

**Tests**(6 个,在 `permissions/tests_check.rs`):
- `recall_pitfall_footnote_active_hit_returns_text`
- `recall_pitfall_footnote_unrelated_tool_returns_none`
- `recall_pitfall_footnote_verified_hit_returns_none_for_p3`(verified 是 P5 范围,P3 严格排除)
- `recall_pitfall_footnote_candidate_hit_returns_none`(candidate 是 P2 范围,P3 严格排除)
- `recall_pitfall_footnote_command_pattern_mismatch_returns_none`
- `recall_pitfall_footnote_empty_db_returns_none`

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

### 5b. Worker (subagent) tool_use — interactive ask round-trip

> 2026-06-22 (RULE-FrontSubagent-003 修复):worker 的 Tier 4 ask 从
> "自动拒绝"(RULE-A-014 止血)升级为完整交互 round-trip。本节是
> **跨层契约**——前后端两端必须一致,否则 UI 是死代码(见下方
> Wrong vs Correct 的 B1 教训)。

#### Scope / Trigger

`dispatch_subagent` 截流后,`run_subagent`(`chat_loop.rs`)以
`is_worker: Some(true)` + `worker_run_id: Some(...)` 嵌套调用
`run_chat_loop`。worker 的 `PermissionContext` 继承 parent mode
(edit/plan/yolo),但 `is_worker=true`。当 worker tool_use 落到
Tier 4 `ask_path`(仓库外 path / shell Ask / web_fetch 无 grant),
**不再 collapse,而是走与 parent 同构的 register_ask + select
round-trip**——区别仅在 IPC 通道 + session key 命名(见下)。

#### Signatures

```rust
// PermissionContext 扩展(PR1)
struct PermissionContext {
    session_id: String,          // parent session id(worker 也继承 parent)
    mode: Mode,
    cwd: PathBuf,
    is_worker: bool,
    worker_run_id: Option<String>, // Some(run_id) 当且仅当 is_worker=true
}

// PermissionAskPayload 扩展(PR1,camelCase 序列化)
struct PermissionAskPayload {
    rid: String,
    session_id: String,            // ⚠ parent session id(非 composite!)
    tool_use_id: String,
    tool_name: String,
    tool_input: serde_json::Value,
    risk: Risk,
    reason: Option<String>,
    path: Option<String>,
    worker_run_id: Option<String>, // Some(run_id) → 前端路由到 drawer
}
```

#### Contracts(三端一致,任一端错 → 死代码)

**1. Dual-channel IPC emit**(worker ask 上两条通道,各自独立消费):

| 通道 | 消费方 | 用途 |
|---|---|---|
| `permission:ask` | `usePermissionsStore`(permissions.ts) | live pending entry → drawer 交互卡 + WorkerAskBanner |
| `subagent:event` | `useSubagentRunsStore`(subagentRuns.ts) | transcript → drawer 历史卡 |

两条通道携带**同一 rid**。前端 drawer 按 rid 去重:`getPendingByRid(rid)`
命中 → 渲染交互卡(Allow/Deny);未命中(已 resolved)→ 渲染历史卡。

**2. `session_id` 双重语义**(关键 gotcha):

| 位置 | 值 | 为什么 |
|---|---|---|
| IPC payload `session_id` | **parent session id** | 前端 `WorkerAskBanner` 按 `ask.sessionId === parentSessionId` 分组计数 |
| `register_ask`/`resolve_ask` 内部 store key | **composite `"worker:{run_id}"`** | oneshot 与 parent 的 pending 隔离(RULE-A-014:worker 不能污染/被 parent cancel 误杀) |

composite 字符串**绝不**序列化到 payload——前端永远看不到它(只看 rid + workerRunId)。

**3. Cancellation 路由**:parent session cancel → 派生 `child_token`
触发 → worker oneshot select 的 cancel 臂命中 → resolve(Deny)。
用户切 session(不 cancel)→ child_token 不触发 → ask 继续等(切回仍可响应)。

#### Validation & Error Matrix

| 条件 | 结果 |
|---|---|
| user Allow(120s 内) | oneshot 臂 → `Decision::Allow`,worker 继续 |
| user Deny(120s 内) | oneshot 臂 → `Decision::Deny { reason: "user-denied" }` |
| 120s 无响应 | timeout 臂 → `Decision::Deny { reason: "timeout" }`(安全网) |
| parent cancel | cancel 臂 → `Decision::Deny { reason: "cancelled" }` |
| `is_worker=true` 但 `worker_run_id=None` | tracing::error + fallback `"UNKNOWN_WORKER"`(防御性,不应发生) |
| worker `AllowAlways` | 后端写 **per-run grant cache**(`RunGrantCache`,内存,随 worker run 销毁,**绝不**写 `session_tool_permissions`,跨权限边界) → 前端 worker 卡**显示**"本次运行始终允许"按钮(文案与主对话"始终允许"区分;wire 仍 `allow_always`,语义在后端按 `is_worker` 分流);parent 路径 `AllowAlways` 仍写 DB |
| worker per-run cache 命中放行 | Tier 4 三分支(Path/Shell/WebFetch)在"session grant miss → 将 emit ask"之前查 `RunGrantCache`,命中 → `Decision::Allow`(同 session-grant-hit 模式,audit/transcript 遵 RULE-A-016;不写 ask resolve 端 parent audit,不写 DB) |

#### Tests Required(cargo test --lib,均已在 `permissions/mod.rs`)

| Test | Asserts |
|---|---|
| `worker_ask_uses_isolated_permission_session_id` | payload.session_id == parent(非 composite)+ worker_run_id 正确 + composite key 隔离(cancel parent 不影响 worker) |
| `worker_ask_allowed_resolves_allow` | oneshot Allow → Decision::Allow |
| `worker_ask_timeout_resolves_deny` | >120s → Decision::Deny |
| `worker_ask_cancelled_resolves_deny` | cancel token → Decision::Deny |
| `worker_ask_user_deny_resolves_deny` | oneshot Deny → Decision::Deny |
| `worker_ask_payload_carries_worker_run_id_camel_case` | wire `workerRunId` 字段 |
| `emit_permission_ask_populates_transcript_with_parent_session_id`(subagent.rs) | sink 双发:transcript entry + parent session_id |

> **Per-run grant cache (`RunGrantCache`,2026-06-26 task `06-26-subagent-per-run-grant`)** —— 后端单测在 `agent/permissions/`(`RunGrantCache` 模块)+ `agent/tests_subagent.rs`(集成),验证:
> - **web_fetch (match_kind=tool)**:worker 第 1 次 `web_fetch` AllowAlways → 写 cache(tool/NULL);同 run 第 2 次 `web_fetch` 命中 cache 直接 Allow,不弹 WorkerAskBanner。
> - **shell (match_kind=prefix)**:worker `cargo test` AllowAlways → 写 cache(prefix/`cargo`);同 run `cargo build`/`cargo check` 命中(`cargo` prefix);`npm install`(不同 prefix)仍弹。
> - **path (match_kind=path glob)**:worker 读仓库外 `/tmp/notes/a.md` AllowAlways → 写 cache(path/`/tmp/notes/*`);同 run `/tmp/notes/b.md` 命中 glob;`/tmp/other/c.md`(glob 外)仍弹。
> - **AllowOnce 不写 cache**:仅一次放行下一次同类调用仍弹窗。
> - **run 结束销毁**:worker run 终态(Cancelled/Error/Incomplete/Completed)→ `Arc<RunGrantCache>` 随 sink 销毁;新 worker 不继承上个 run 授权。
> - **concurrent dispatch(L3a)隔离**:两个并行 worker 各自独立 sink → 独立 cache,互不串扰。
> - **parent session 零污染**:全流程 `session_tool_permissions` 无新行。
> - **outcome 编码不变**:`emit_permission_ask_resolved` 仍 `"allow"`(worker AllowAlways 不再特殊编码)。
> - **parent 路径 `run_grants=None` 零回归**:parent chat 命令完全不查/不写 run cache。

> AppHandle `permission:ask` emit 路径无法单测(需 Tauri runtime),
> 走 `subagent_buffer_sink_emits_ipc_event_per_emit` 同款 warn-on-error
> 模式,集成测试覆盖。

#### Wrong vs Correct(B1 教训——曾让整个前端 UI 变死代码)

**Wrong**(PR1 原始实现 → PR2 check 发现 BLOCKING):

```rust
// ❌ emit_permission_ask 只写 transcript,不发 permission:ask IPC
fn emit_permission_ask(&self, payload: PermissionAskPayload) {
    self.record(TranscriptKind::PermissionAsk, payload_json); // 只 subagent:event
}
// ❌ payload.session_id 用了 composite
session_id: permission_session_id.clone(), // "worker:run-1"
```

后果:前端 `usePermissionsStore` 只听 `permission:ask`(从不触发)→
`pendingWorkerByRunId` 永远空 → drawer 永远历史卡 → banner count=0 永远隐藏。
**整个 PR2 前端交互 UI 是死代码。** 单测全绿是因为它们直接调
`setPending()` 绕过了 IPC 层——测了存储逻辑,没测线上传输契约。

**Correct**(PR1.5 修复):

```rust
// ✅ 双发:permission:ask(live store)+ subagent:event(transcript)
fn emit_permission_ask(&self, payload: PermissionAskPayload) {
    if let Some(handle) = &self.app_handle {
        let _ = handle.emit("permission:ask", payload.clone()); // live 通道
    }
    self.record(TranscriptKind::PermissionAsk, payload_json);   // transcript 通道
}
// ✅ payload.session_id 用 parent(banner 分组);composite 只留内部 keying
session_id: ctx.session_id.clone(), // parent
```

**教训**:跨层契约改动,单测每个端点是不够的——必须有一条端到端
trace 证明 wire 真的通了(emit → listen → store → render)。两端各自
绿 ≠ 整体绿。

#### Audit note + outcome transcript entry (2026-06-22)

worker ask 的 resolve 端**不写** `session_audit_events`(RULE-A-016:
worker 决策不污染 parent C4 审计)。transcript(`subagent_runs.transcript_json`)
是 worker 的审计式记录。4 个 `WorkerAskAllowed/Denied/TimedOut/Cancelled`
AuditKind 变体已加到 enum(forward-compat + round-trip test),但无 writer——
未来若要"parent 审计里看到 worker 活动",应新增独立 `subagent_audit_events` 表,
而非写 parent 表。

**Post-2026-06-22 (RULE-WorkerAsk-001) — `TranscriptKind::PermissionAskResolved` entry**:worker `ask_path` `tokio::select!{cancel, timeout, oneshot}` 返回后,`SubagentBufferSink::emit_permission_ask_resolved(rid, outcome)` 追加一个 `PermissionAskResolved` entry,`payload_json = { rid, outcome }`,outcome ∈ `{"allow","deny","timeout","cancel"}`(`OneshotDropped` → `"cancel"`;`timeout` / `cancel` 臂的合成 Deny 走 `tokio::select!` 体内部 emit,oneshot 臂的 Allow/Deny 走 match-arm 路径 emit,reason-based skip 防双累)。**2026-06-26 注**(task `06-26-subagent-per-run-grant`):原"worker `AllowAlways` 当 `AllowOnce` 统一 `"allow"`"已不再适用 —— worker AllowAlways 现写 per-run cache(`RunGrantCache`),outcome 仍记 `"allow"`(非特殊编码,与 AllowOnce 路径结果相同)。这是 **transcript-only** —— 不双发 `permission:ask` IPC(live 期间交互卡消失已由 permissions store rid removal 驱动,Session 62 `89e5ba1` 行为不变);不进父 `session_audit_events`(同 RULE-A-016 隔离原则);不进 `subagent_audit_events`(尚未存在的未来表)。`ChatEventSink` trait `emit_permission_ask_resolved` **默认 no-op**,仅 `SubagentBufferSink` override(避免 `Arc<dyn>` downcast,`AppHandleSink` / 测试 sink 零改动继承默认 —— 同 trait-default-no-op 模式可推广到未来 sink-only 合约)。前端 `pairSections` 按 `rid` 配对 → `PermissionAskBody.vue` historical 分支显 ✓/✗/⏱/⊘ outcome badge;pre-fix 老 transcript 降级中性。

**Per-run cache 命中放行**(2026-06-26,task `06-26-subagent-per-run-grant`):worker Tier 4 三分支(Path/Shell/WebFetch)在"session grant miss → 将 emit ask"之前各插一道 `RunGrantCache` 查询,命中 → `Decision::Allow`。cache 命中放行走 `check.rs` 现有 worker grant-hit 的 `record_audit(ToolAllowed)` 模式(与 session-grant-hit 一致),**不违反 RULE-A-016**(不写 ask resolve 端的 parent `session_audit_events`;不写 `session_tool_permissions`);cache 命中本身不出现在 transcript(只有首次 ask 的 `PermissionAsk` + resolve 的 `PermissionAskResolved` 在 transcript;后续 cache-hit 是静默 Allow,无新 transcript entry —— 同 session-grant-hit 在主对话中无新 audit 行)。

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