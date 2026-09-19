<!-- Moved from permission-layer.md 2026-09-19 (doc-split) -->

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

