//! ⑨ 关 Permission decision layer + ⑧a Mode check (A2 + B7, re-grill 2026-06-13).
//!
//! Sits between the agent loop's `provider.send()` stream and
//! `tools::execute_tool`. On every tool_use block the agent
//! loop calls [`check`] which produces a [`Decision`] that
//! either allows the call, denies it (silent or with a reason),
//! or asks the user via a oneshot channel + Tauri event.
//!
//! ## 5-tier evaluation order — RE-GRILL 2026-06-13 (SOT — see
//! `.trellis/tasks/06-13-a2-b7-regrill-path-based/prd.md` §1)
//!
//! ```text
//! Tier 1. Hooks           — pre-call interface (MVP: no-op)
//! Tier 2. Deny rules      — hard kill list (shell 9 regex,
//!                            always silent — Yolo included)
//! Tier 3. Mode check      — Plan blocks write_file/edit_file
//!                            (file writes only; text error, NO
//!                            modal). shell NOT blocked here —
//!                            it's heterogenous (git diff vs git
//!                            push), so its Mode decision lives
//!                            in Tier 4 (三档分类 2026-06-14).
//! Tier 4. Path / Prefix / External policy
//!         ├─ Path tools (read_file / write_file /
//!         │   edit_file / list_dir / grep / glob):
//!         │   - parse path → is_within_root(session.cwd, path)?
//!         │     - YES → check session_tool_permissions
//!         │             (match_kind='path') → hit → Allow
//!         │                                       miss → Allow (silent)
//!         │     - NO  → check session_tool_permissions
//!         │             (match_kind='path') → hit → Allow
//!         │                                       miss → emit ask
//!         ├─ Shell (三档 2026-06-14):
//!         │   - check prefix grant → Allow (始终允许 命中)
//!         │   - else classify_prefix →
//!         │     - ReadOnly   → Allow (silent; Plan included)
//!         │     - SideEffect → Plan: emit ask / Edit: Allow
//!         │     - Ask        → emit ask (Plan & Edit)
//!         ├─ Web Fetch:
//!         │   - always external → check tool grant
//!         │                     → hit → Allow
//!         │                       miss → emit ask
//!         │
//!         │ Yolo: bypass entire Tier 4 (always Allow).
//!         │ Still subject to Tier 2 hard-kill.
//! Tier 5. Allow rules     — default allow-all (MVP)
//! Tier 6. Audit           — write session_audit_events
//! ```
//!
//! See `docs/_reviews/REVIEW-a2-b7-regrill-path-based-2026-06-13.md`
//! for the 10 re-grill decisions; see
//! `docs/IMPLEMENTATION.md §4` for the ADR.

pub mod dangerous;
pub mod shell_trust;

use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter};
use tokio::sync::{oneshot, Mutex};

use crate::db::Mode;

// ---------------------------------------------------------------------------
// Risk enum (serialized to IPC in `permission:ask` payload)
// ---------------------------------------------------------------------------

/// Risk level for a `(tool_name, tool_input)` pair. Per-tool
/// static map; see [`risk_for_tool`]. Serializes lowercase to
/// match the PRD's TypeScript type (`"low" | "medium" | "high"
/// | "critical"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
// `Critical` is reserved for the PermissionModal "极高" 风险
// variant (3px red border + shield-x icon per UX spec). The
// MVP's per-tool static map only ever returns Low / Medium /
// High, so the variant is never constructed in PR1. The dead-
// code allow is forward-compat — PR3 (PermissionModal) reads
// the wire payload and renders the critical styling on this
// variant.
#[allow(dead_code)]
pub enum Risk {
 Low,
 Medium,
 High,
 Critical,
}

impl Risk {
 // `as_str` is intentionally not used in PR1 — the IPC payload
 // gets the lowercase string from `#[serde(rename_all = ...)]`
 // on the enum itself. Kept as a method for any future caller
 // that wants the string without going through serde.
 #[allow(dead_code)]
 pub fn as_str(&self) -> &'static str {
 match self {
 Risk::Low => "low",
 Risk::Medium => "medium",
 Risk::High => "high",
 Risk::Critical => "critical",
 }
 }

 /// Chinese label for the PermissionModal UI (per audit §6.2
 /// — unify on 中文).
 #[allow(dead_code)]
 pub fn label_cn(&self) -> &'static str {
 match self {
 Risk::Low => "低",
 Risk::Medium => "中",
 Risk::High => "高",
 Risk::Critical => "极高",
 }
 }
}

/// Per-tool risk level. Static mapping — `shell` is always High,
/// `write_file` / `edit_file` are Medium, the read-only tools
/// (`read_file` / `grep` / `glob` / `list_dir` / `web_fetch`)
/// are Low. Reserved for memory-file-driven overrides in a
/// future PR.
pub fn risk_for_tool(tool_name: &str) -> Risk {
 match tool_name {
 "shell" => Risk::High,
 "write_file" | "edit_file" => Risk::Medium,
 // `web_fetch` is Low at the risk-permission layer; its own
 // SSRF blocklist (in `tools/web_fetch.rs`) is the relevant
 // defense for network egress.
 _ => Risk::Low,
 }
}

// ---------------------------------------------------------------------------
// AuditKind enum (serialized into `session_audit_events.kind`)
// ---------------------------------------------------------------------------

/// Audit event kinds. Serialized lowercase (matches DB column).
/// 10 variants — see PRD `## A2 后端` "审计 `kind` 枚举" section.
///
/// `ModeChanged` / `YoloEntered` / `YoloExited` are written
/// directly by the `set_session_mode` Tauri command via
/// `db::record_audit_event(.., "mode_changed", ..)` (the
/// command path uses string literals for the kind, not this
/// enum, to keep the cross-module call graph tight). The
/// variants are kept here as the typed single source of truth
/// for the audit log schema — PR3's C4 audit-log UI will
/// match on these.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditKind {
 /// ⑨ 关拒绝 (Tier 2 hit, Tier 3 timeout, Tier 3 user deny)
 ToolDenied,
 /// ⑨ 关放行 (Tier 5 默认 OR Tier 3 "始终允许" 命中 OR Tier 3 user "仅一次")
 ToolAllowed,
 /// ⑨ 关弹窗询问 (Tier 3 emit permission:ask)
 ToolPermissionAsk,
 /// 用户选"始终允许"(后端写了 session_tool_permissions)
 PermissionGranted,
 /// Mode 切换 (set_session_mode 触发)
 ModeChanged,
 /// 进入 Yolo (mode → Yolo)
 YoloEntered,
 /// 退出 Yolo (mode != Yolo 且之前是 Yolo)
 YoloExited,
 /// Yolo 模式下仍被 Tier 2 deny 拦截 (硬墙)
 ToolDeniedYolo,
 /// Tier 3 120s 超时 (user 没响应)
 PermissionTimeout,
 /// C1 cancel 触发的请求终止 (与 Tier 3 deny 区分)
 RequestCancelled,
 /// ⑩ tool 执行完成 (C4 任务 PR1, 2026-06-14): payload 携带
 /// `tool_name` / `tool_input` / `duration_ms` / `exit_code`,
 /// 用于"哪步最慢 / 哪步报错"的事后回看。落表点在 agent
 /// loop 拿到 `execute_tool` 返回值之后 (duration + exit_code
 /// 已知), 见 `agent/chat.rs` 的 tool 执行循环。
 ToolExecuted,
}

impl AuditKind {
 pub fn as_str(&self) -> &'static str {
 match self {
 Self::ToolDenied => "tool_denied",
 Self::ToolAllowed => "tool_allowed",
 Self::ToolPermissionAsk => "tool_permission_ask",
 Self::PermissionGranted => "permission_granted",
 Self::ModeChanged => "mode_changed",
 Self::YoloEntered => "yolo_entered",
 Self::YoloExited => "yolo_exited",
 Self::ToolDeniedYolo => "tool_denied_yolo",
 Self::PermissionTimeout => "permission_timeout",
 Self::RequestCancelled => "request_cancelled",
 Self::ToolExecuted => "tool_executed",
 }
 }
}

// ---------------------------------------------------------------------------
// Decision enum (internal — NOT serialized)
// ---------------------------------------------------------------------------

/// ⑨ 关决策结果. 内部enum (不出 IPC — frontend 只看
/// `permission:ask` event 或 `tool_use → is_error: true`).
///
/// - `Allow`: 放行,Tier 6 写 `tool_allowed` 审计
/// - `Deny { reason, critical }`: 静默拒绝 (`is_error: true`),
/// 不弹窗 (Tier 2 路径 + Tier 3 user "拒绝"/超时)
/// - `Ask { reason, risk }`: 发 `permission:ask` event + 等
/// frontend `permission_response` (120s 超时 → 自动转 Deny)
///
/// The `Ask` variant is reserved for the PermissionModal
/// (PR3) — the intermediate state when `check()` has fired
/// the IPC but not yet received the user's response. In PR1
/// `check()` collapses `Ask` into `Allow` / `Deny` internally
/// before returning, so the variant is never observed by
/// the chat loop. Kept in the type so the PermissionModal
/// wire shape stays stable.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum Decision {
 Allow,
 Deny { reason: String, critical: bool },
 Ask { reason: String, risk: Risk },
}

impl Decision {
 #[allow(dead_code)]
 pub fn is_deny(&self) -> bool {
 matches!(self, Decision::Deny { .. })
 }
 #[allow(dead_code)]
 pub fn is_ask(&self) -> bool {
 matches!(self, Decision::Ask { .. })
 }
}

// ---------------------------------------------------------------------------
// PermissionContext — input to `check()`
// ---------------------------------------------------------------------------

/// Per-call context. Built once per agent-loop turn, passed
/// into each `check()` call. Cloned cheaply (a few strings +
/// a `PathBuf`).
///
/// **cwd** is the session's current working directory and is
/// the single containment anchor for the Tier 4 path-based
/// check (`is_within_root(ctx.cwd, path)` per re-grill PRD §1).
/// The project root is intentionally NOT plumbed here — the
/// permission layer's "inside the project?" question is
/// "inside the session's cwd?", which can be a subdir of the
/// project (e.g. when the user has navigated to
/// `~/repo/frontend`). The tool layer's
/// `assert_within_root(ctx.project_root, cwd)` is the source of
/// truth for the project boundary.
#[derive(Debug, Clone)]
pub struct PermissionContext {
 pub session_id: String,
 pub mode: Mode,
 pub cwd: std::path::PathBuf,
}

// ---------------------------------------------------------------------------
// Permission store — pending permission:ask IPC bridge
// ---------------------------------------------------------------------------

/// The user's response to a `permission:ask` event. Sent over
/// the oneshot channel from the Tauri command `permission_response`
/// (in [`crate::commands::permissions`]) to the awaiting agent
/// loop's `check()` future.
#[derive(Debug, Clone)]
pub enum PermissionResponse {
 AllowOnce,
 AllowAlways,
 Deny,
}

/// In-flight permission asks, keyed by `rid` (random request id
/// emitted with the `permission:ask` event). The agent loop
/// inserts a `(rid, oneshot::Sender)` pair before emitting;
/// the IPC `permission_response` handler looks up by `rid` and
/// sends the response. The sender is `Drop`-ed (and thus removed
/// from the map) on timeout.
pub type PermissionStore =
 Arc<Mutex<HashMap<String, oneshot::Sender<PermissionResponse>>>>;

pub fn new_permission_store() -> PermissionStore {
 Arc::new(Mutex::new(HashMap::new()))
}

/// Insert a pending ask. The `rid` is a UUID string (the agent
/// loop generates it). The returned `oneshot::Receiver` is the
/// future the agent loop awaits in `check()`.
pub async fn register_ask(
 store: &PermissionStore,
 rid: String,
) -> oneshot::Receiver<PermissionResponse> {
 let (tx, rx) = oneshot::channel();
 let mut map = store.lock().await;
 map.insert(rid, tx);
 rx
}

/// Resolve a pending ask. Called by the `permission_response`
/// IPC handler. Returns `true` if the rid was found and the
/// sender accepted the response; `false` if the rid was missing
/// (already timed out, or duplicate response).
pub async fn resolve_ask(
 store: &PermissionStore,
 rid: &str,
 response: PermissionResponse,
) -> bool {
 let mut map = store.lock().await;
 if let Some(tx) = map.remove(rid) {
 tx.send(response).is_ok()
 } else {
 false
 }
}

/// Cancel all pending asks for a session. Called from the
/// destructive-op cancel hook (`delete_session` etc.) — same
/// pattern as the `CancellationGuard` on the agent loop's
/// `cancellations` map. Reserved for the future
/// `delete_session` integration (the MVP `delete_session` IPC
/// doesn't call this yet — the hook lives in `commands/
/// sessions.rs` and will be wired in once the destructive-op
/// audit pass lands).
#[allow(dead_code)]
pub async fn cancel_session_asks(
 store: &PermissionStore,
 _session_id: &str,
) {
 // MVP: simple "drop all pending" — the rid key has no session
 // binding yet (the rids are UUIDs). For a future PR, key the
 // map by `(session_id, rid)` and iterate. Today, the oneshot
 // senders drop on the `clear()` and the receiver returns
 // `Err(RecvError)` which `check()` treats as Deny.
 let mut map = store.lock().await;
 map.clear();
}

// ---------------------------------------------------------------------------
// Ask payload (serialized to Tauri event `permission:ask`)
// ---------------------------------------------------------------------------

/// Wire payload for the `permission:ask` event. The frontend's
/// `usePermissionsStore.setPending` reads this and mounts the
/// `<PermissionModal>`. The modal's 3 buttons each invoke
/// `permission_response(rid, decision)` which resolves the
/// pending oneshot in [`PermissionStore`].
///
/// `path` is filled in for path-tools (read_file / write_file /
/// edit_file / list_dir / grep / glob) so the PermissionModal
/// can show a "path scope" row in the header (per re-grill
/// Q10 "保留 risk 字段作 UI 视觉,加 path 范围行"). It is
/// omitted for shell / web_fetch (the modal renders the
/// command / URL inline via `toolInput` instead).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionAskPayload {
 pub rid: String,
 pub tool_name: String,
 pub tool_input: serde_json::Value,
 pub risk: Risk,
 #[serde(skip_serializing_if = "Option::is_none")]
 pub reason: Option<String>,
 #[serde(skip_serializing_if = "Option::is_none")]
 pub path: Option<String>,
}

// ---------------------------------------------------------------------------
// ⑨ 关 entry point — 5-tier evaluation
// ---------------------------------------------------------------------------

/// Default timeout for Tier 3 user response. Matches PRD
/// `### IPC 异常路径` "用户从不响应" → 120s auto-deny.
pub const ASK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Run the ⑨ 关 5-tier check on one tool_use.
///
/// # Parameters
///
/// - `ctx` — per-call context (session_id + mode + cwd).
/// - `store` — `PermissionStore` for the Tier 4 oneshot bridge.
/// - `db` — SQLite pool (Tier 4 has_tool_permission + Tier 6
///   audit write).
/// - `app` — Tauri AppHandle (for the `permission:ask` emit
///   on Tier 4 and the `tracing` instrumentation).
/// - `tool_name` — the LLM-emitted tool name.
/// - `tool_input` — the LLM-emitted tool input JSON.
/// - `token` — the agent-loop cancellation token (for Tier 4
///   wait interruption on user Stop — does NOT trigger on deny).
///
/// # Returns
///
/// - `Decision::Allow` → execute_tool runs.
/// - `Decision::Deny { reason, critical }` → skip tool, return
///   `(reason, true)` from the agent-loop wrapper.
/// - `Decision::Ask { reason, risk }` is INTERNAL ONLY — the
///   function resolves it internally (await the oneshot or
///   timeout) and returns the final `Allow` / `Deny`.
///
/// # Re-grill 2026-06-13: ordering
///
/// Tier 1 (Hooks) → Tier 2 (Deny) → Tier 3 (Mode) → Tier 4
/// (Path / Prefix / External) → Tier 5 (Allow) → Tier 6 (Audit).
/// The old Tier 3 (always ask) is gone; the new Tier 4 only asks
/// when the path / prefix / external policy says so. Mode
/// check (Plan block writes) was Tier 4 in the old design;
/// moving it to Tier 3 eliminates the "user clicks 始终允许,
/// then gets Mode-denied" bad interaction.
pub async fn check(
 ctx: &PermissionContext,
 store: &PermissionStore,
 db: &SqlitePool,
 app: &AppHandle,
 tool_name: &str,
 tool_input: &serde_json::Value,
 token: &tokio_util::sync::CancellationToken,
) -> Decision {
 // ----- Tier 1: Hooks (no-op for MVP — pre-call interface reserved) -----
 // Future PR may insert a hook override point here.

 // ----- Tier 2: Deny rules (hard kill list) -----
 // Yolo 也走这步 — 静默拒绝,不弹窗. Always silent (no Ask path).
 // This is INVARIANT: the 9 regex patterns in `dangerous.rs` are
 // not touched by the re-grill. The re-grill only restructures
 // Tier 3-5 ordering; Tier 2 is the hard wall.
 if let Some(reason) = dangerous::is_kill_listed(tool_name, tool_input) {
 let critical = true;
 let kind = if ctx.mode == Mode::Yolo {
 AuditKind::ToolDeniedYolo
 } else {
 AuditKind::ToolDenied
 };
 tracing::warn!(
 session_id = %ctx.session_id,
 mode = %ctx.mode.as_str(),
 tool = %tool_name,
 reason = %reason,
 "permission::check: Tier 2 deny"
 );
 let _ = record_audit(app, db, ctx, kind, tool_name, tool_input, Some(&reason)).await;
 return Decision::Deny { reason, critical };
 }

 // ----- Tier 3: Mode check (Plan blocks file writes) -----
 // ⑨ 关第 3 道 + ⑧a 三重防御的最后一层. Plan 模式拦截
 // write_file/edit_file (纯写工具, 无歧义, 直接 text error
 // 不弹窗 — 避免 "用户点始终允许 → 仍被 Mode 拒" 的鬼畜交互).
 //
 // **shell 不再在此层拦截 (三档分类 2026-06-14)**: shell 是
 // 异构工具 (git diff 读 / git push 写), 一刀切会把只读命令
 // 也禁掉且无放行口子. shell 的 mode 感知下沉到 Tier 4 的
 // Shell 分支: ReadOnly→Allow, SideEffect/Ask→弹窗 (Plan 下
 // 用户可当场放行). 见 shell_trust.rs 三档分类.
 if matches!(ctx.mode, Mode::Plan) {
 if matches!(tool_name, "write_file" | "edit_file") {
 tracing::info!(
 session_id = %ctx.session_id,
 mode = %ctx.mode.as_str(),
 tool = %tool_name,
 "permission::check: Tier 3 mode block (write tools in read-only mode)"
 );
 let reason = format!(
 "I cannot execute {} in {} mode (read-only session)",
 tool_name,
 ctx.mode.as_str()
 );
 let _ = record_audit(
 app,
 db,
 ctx,
 AuditKind::ToolDenied,
 tool_name,
 tool_input,
 Some(&format!("tool blocked in {} mode", ctx.mode.as_str())),
 )
 .await;
 return Decision::Deny {
 reason,
 critical: false,
 };
 }
 }

 // ----- Tier 4: Path / Prefix / External policy -----
 // Re-grill 2026-06-13: was the "always ask" tier. Now split
 // by tool type and bypassed by Yolo.
 //
 // Yolo bypasses the entire tier (Q4: "Yolo bypass 所有 modal").
 // Tier 2 still catches the hard-kill patterns. Tier 3 still
 // catches Plan write tools (we never reach Tier 4 in that case).
 if ctx.mode == Mode::Yolo {
 tracing::info!(
 session_id = %ctx.session_id,
 tool = %tool_name,
 "permission::check: Tier 4 bypassed (Yolo mode)"
 );
 // Tier 6 audit for the Allow path (Tier 2 / Tier 3 deny paths
 // already wrote their own audit rows above).
 let _ = record_audit(app, db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None).await;
 return Decision::Allow;
 }

 // Dispatch by tool type.
 match classify_tool(tool_name) {
 ToolKind::Path => {
 // Path tools: extract the `path` argument (and
 // optionally `cwd` / `working_directory` override),
 // check `is_within_root`, then consult
 // `session_tool_permissions` for a path-glob grant,
 // and emit `permission:ask` if needed.
 let path_str = extract_path_arg(tool_name, tool_input);
 match path_str {
 Some(p) => {
 // Normalize: the LLM may send relative paths.
 // For the permission layer, we treat the path as
 // relative to ctx.cwd unless it's already absolute.
 let abs_path = if std::path::Path::new(&p).is_absolute() {
 std::path::PathBuf::from(&p)
 } else {
 ctx.cwd.join(&p)
 };
 let inside = crate::projects::boundary::is_within_root(&ctx.cwd, &abs_path);
 // Tier 4.1: check session_tool_permissions
 // match_kind='path' for a grant. If hit, Allow.
 if let Ok(true) = check_path_grant(db, &ctx.session_id, tool_name, &abs_path).await {
 tracing::info!(
 session_id = %ctx.session_id,
 tool = %tool_name,
 path = %abs_path.display(),
 "permission::check: Tier 4 path grant hit"
 );
 let _ = record_audit(app, db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None).await;
 return Decision::Allow;
 }
 if inside {
 // Inside the project, no grant → silent Allow
 // (the user trusts the agent to work in the repo).
 tracing::info!(
 session_id = %ctx.session_id,
 tool = %tool_name,
 path = %abs_path.display(),
 "permission::check: Tier 4 path inside root, silent Allow"
 );
 let _ = record_audit(app, db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None).await;
 return Decision::Allow;
 }
 // Outside the project, no grant → modal.
 let path_owned = abs_path.to_string_lossy().to_string();
 return ask_path(
 app, db, store, ctx,
 tool_name, tool_input,
 &path_owned, Some(&path_owned), token,
 ).await;
 }
 None => {
 // Path tool without a `path` arg is a malformed
 // tool_use — let the tool layer surface the
 // error (it will produce is_error: true). For
 // the permission layer, default to Allow
 // (the tool layer's schema validation is the
 // real gate).
 let _ = record_audit(app, db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None).await;
 return Decision::Allow;
 }
 }
 }
 ToolKind::Shell => {
 let cmd = tool_input.get("command").and_then(|v| v.as_str()).unwrap_or("");
 // (a) "始终允许" prefix-grant hit → silent Allow. Closes the
 // old gap: match_value_for_allow_always wrote match_kind='prefix'
 // rows for shell but Tier 4 never queried them — a user's
 // AllowAlways on a shell command now sticks across turns.
 if let Ok(true) = check_prefix_grant(db, &ctx.session_id, &shell_trust::first_token_for_allow_always(cmd)).await {
 let _ = record_audit(app, db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None).await;
 return Decision::Allow;
 }
 // (b) Three-tier classification + per-Mode mapping. shell is
 // heterogenous (git diff vs git push), so the Mode decision
 // lives HERE in Tier 4, not in Tier 3.
 //   Plan: ReadOnly→silent Allow; SideEffect/Ask→modal.
 //   Edit: ReadOnly/SideEffect→silent Allow; Ask→modal.
 //   Yolo never reaches here (Tier 4 bypassed at the top).
 match shell_trust::classify_prefix(cmd) {
 shell_trust::ShellTrust::ReadOnly => {
 // Pure read — allow silently in every mode (Plan included).
 let _ = record_audit(app, db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None).await;
 return Decision::Allow;
 }
 shell_trust::ShellTrust::SideEffect => {
 if ctx.mode == Mode::Plan {
 // Plan is read-only; surface the side effect to the
 // user instead of silently allowing it.
 return ask_path(app, db, store, ctx, tool_name, tool_input, cmd, None, token).await;
 }
 // Edit: silent Allow (old whitelist behaviour).
 let _ = record_audit(app, db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None).await;
 return Decision::Allow;
 }
 shell_trust::ShellTrust::Ask => {
 // Asklist / unknown / structurally complex — modal in
 // every interactive mode. Shell commands are NOT path
 // tools: the modal renders the command inline via
 // `toolInput` (no "path scope" row). `path_for_modal =
 // None` keeps the `path` field OFF the wire so the
 // frontend's `v-if="hasPath"` does not render a
 // misleading scope row for a shell ask.
 return ask_path(app, db, store, ctx, tool_name, tool_input, cmd, None, token).await;
 }
 }
 }
 ToolKind::WebFetch => {
 // Web fetch is always external — check
 // session_tool_permissions match_kind='tool' for
 // `web_fetch`. If hit, Allow; else modal.
 if let Ok(true) = check_tool_grant(db, &ctx.session_id, "web_fetch").await {
 let _ = record_audit(app, db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None).await;
 return Decision::Allow;
 }
 return ask_path(
 app, db, store, ctx,
 tool_name, tool_input,
 tool_input.get("url").and_then(|v| v.as_str()).unwrap_or(""),
 // Web fetch is always external — the modal renders
 // the URL inline via `toolInput` (no "path scope"
 // row). `path_for_modal = None` keeps the `path` field
 // OFF the wire so the frontend's `v-if="hasPath"` does
 // not render a misleading scope row for a web_fetch
 // ask (it would otherwise show "仓库外" against a URL,
 // which is wrong — the URL is not a filesystem path).
 None, token,
 ).await;
 }
 ToolKind::Other => {
 // Unknown / future tool — default Allow (Tier 5).
 // The tool layer's own boundary checks (e.g.
 // ReadGuard for edit_file) are the real gate.
 let _ = record_audit(app, db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None).await;
 return Decision::Allow;
 }
 }
}

// ---------------------------------------------------------------------------
// Tier 4 helpers
// ---------------------------------------------------------------------------

/// Tool classification for Tier 4 dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolKind {
 /// Path-based tools (read_file / write_file / edit_file /
 /// list_dir / grep / glob). All extract a `path` argument and
 /// are subject to the path-glob check.
 Path,
 /// Shell tool. Classified by `shell_trust::classify_prefix`.
 Shell,
 /// Web fetch. Always external; uses `tool` match_kind grant.
 WebFetch,
 /// Unknown / future tools. Default Allow.
 Other,
}

fn classify_tool(tool_name: &str) -> ToolKind {
 match tool_name {
 "read_file" | "write_file" | "edit_file" | "list_dir" | "grep" | "glob" => {
 ToolKind::Path
 }
 "shell" => ToolKind::Shell,
 "web_fetch" => ToolKind::WebFetch,
 _ => ToolKind::Other,
 }
}

/// Extract the `path` argument from a path-tool's input. Most
/// tools use `path`; a future tool may use `cwd` or
/// `working_directory`. Returns the **string** (not PathBuf)
/// because the caller may need to send it on the wire as part
/// of the PermissionAskPayload.
fn extract_path_arg(tool_name: &str, input: &serde_json::Value) -> Option<String> {
 // Read tools / write_file / edit_file / list_dir / grep / glob
 // all use `path` (the schema is uniform across them — see
 // `tools/*.rs::definition()`).
 let _ = tool_name; // silence unused warning; reserved for future
 let p = input
 .get("path")
 .and_then(|v| v.as_str())
 .or_else(|| input.get("cwd").and_then(|v| v.as_str()))
 .or_else(|| input.get("working_directory").and_then(|v| v.as_str()))?;
 Some(p.to_string())
}

/// Check `session_tool_permissions` for a path-glob grant on
/// the given path. Returns `Ok(true)` if any row's
/// `match_value` (a sqlite GLOB) matches the path. Used by
/// Tier 4 to short-circuit the modal for "始终允许 path".
async fn check_path_grant(
 db: &SqlitePool,
 session_id: &str,
 tool_name: &str,
 path: &std::path::Path,
) -> Result<bool, sqlx::Error> {
 // Pull all `path` match_kind rows for this session+tool.
 // The path-glob uses sqlite GLOB syntax:
 // - `*` matches any sequence of characters NOT crossing `/`
 // - `?` matches exactly one character
 // - `**` is NOT supported (sqlite GLOB is single-asterisk
 //   only). The re-grill PRD explicitly accepts this
 //   limitation (§"Out of Scope").
 let rows: Vec<(String,)> = sqlx::query_as(
 r#"
 SELECT match_value FROM session_tool_permissions
 WHERE session_id = ? AND tool_name = ? AND match_kind = 'path'
 "#,
 )
 .bind(session_id)
 .bind(tool_name)
 .fetch_all(db)
 .await?;
 let path_str = path.to_string_lossy();
 for (glob,) in rows {
 // sqlite GLOB matcher (inlined). We use a simple
 // recursive matcher that respects the GLOB rule that
 // `*` does NOT cross `/`. The crate `glob` would also
 // work but the dependency was deemed overkill for
 // one-line matching.
 if sqlite_glob_match(&glob, &path_str) {
 return Ok(true);
 }
 }
 Ok(false)
}

/// Match a path against a sqlite-style GLOB pattern. Supports
/// `*` (zero or more non-`/` characters) and `?` (exactly one
/// non-`/` character). All other characters match literally.
/// Backslash-escape is NOT supported (sqlite GLOB doesn't
/// support it either). Case-sensitive (sqlite GLOB is
/// case-sensitive; the column was stored verbatim from
/// `Path::display()`).
pub(crate) fn sqlite_glob_match(pattern: &str, text: &str) -> bool {
 // Recursive matcher. We track `pi` (pattern index) and
 // `ti` (text index) and use a small stack of
 // backtrack positions for `*`.
 let pbytes = pattern.as_bytes();
 let tbytes = text.as_bytes();
 let mut pi = 0usize;
 let mut ti = 0usize;
 let mut star_pi: Option<usize> = None;
 let mut star_ti: usize = 0;
 while ti < tbytes.len() {
 if pi < pbytes.len() {
 match pbytes[pi] {
 b'*' => {
 // Record backtrack position and try matching
 // zero chars first.
 star_pi = Some(pi);
 star_ti = ti;
 pi += 1;
 continue;
 }
 b'?' => {
 // Single char; `*` rule: `?` does NOT cross `/`.
 if tbytes[ti] == b'/' {
 // Can't match — backtrack on `*` if any.
 if let Some(sp) = star_pi {
 // Skip one char via the previous `*`.
 // BUT: sqlite GLOB `*` doesn't cross `/`, so
 // if we just consumed a `/` we cannot
 // continue matching with `*`. Reset to
 // failure.
 if tbytes[ti] == b'/' {
 // Star matches don't cross `/` — fail.
 return false;
 }
 pi = sp;
 ti = star_ti + 1;
 star_ti += 1;
 continue;
 }
 return false;
 }
 pi += 1;
 ti += 1;
 continue;
 }
 c if c == tbytes[ti] => {
 pi += 1;
 ti += 1;
 continue;
 }
 _ => {
 // Literal mismatch — backtrack on `*` if any.
 if let Some(sp) = star_pi {
 pi = sp;
 ti = star_ti + 1;
 star_ti += 1;
 // But `*` cannot cross `/`; if we just stepped
 // past a `/`, fail.
 if ti > 0 && tbytes[ti - 1] == b'/' {
 return false;
 }
 continue;
 }
 return false;
 }
 }
 } else {
 // Pattern exhausted — backtrack on `*` if any.
 if let Some(sp) = star_pi {
 pi = sp;
 ti = star_ti + 1;
 star_ti += 1;
 if ti > 0 && tbytes[ti - 1] == b'/' {
 return false;
 }
 continue;
 }
 return false;
 }
 }
 // Pattern may have trailing `*`s — consume them.
 while pi < pbytes.len() && pbytes[pi] == b'*' {
 pi += 1;
 }
 pi == pbytes.len()
}

/// Check `session_tool_permissions` for an exact-tool grant.
/// Returns `Ok(true)` if any row has
/// `match_kind = 'tool'` + `tool_name = ?` + `match_value IS NULL`.
async fn check_tool_grant(
 db: &SqlitePool,
 session_id: &str,
 tool_name: &str,
) -> Result<bool, sqlx::Error> {
 crate::db::has_tool_permission(db, session_id, tool_name).await
}

/// Check `session_tool_permissions` for a shell-prefix grant.
/// Returns `Ok(true)` if any row has `tool_name='shell'`,
/// `match_kind='prefix'`, and `match_value = first_token` (exact
/// match — prefix grants store the bare command name like
/// `cargo`, not a glob).
///
/// Closes the old gap where `match_value_for_allow_always` wrote
/// `match_kind='prefix'` rows for shell but Tier 4 never queried
/// them: a user's "始终允许" on a shell command now sticks.
async fn check_prefix_grant(
 db: &SqlitePool,
 session_id: &str,
 first_token: &str,
) -> Result<bool, sqlx::Error> {
 if first_token.is_empty() {
 return Ok(false);
 }
 let row: Option<(i64,)> = sqlx::query_as(
 r#"
 SELECT 1 FROM session_tool_permissions
 WHERE session_id = ?
   AND tool_name = 'shell'
   AND match_kind = 'prefix'
   AND match_value = ?
 LIMIT 1
 "#,
 )
 .bind(session_id)
 .bind(first_token)
 .fetch_optional(db)
 .await?;
 Ok(row.is_some())
}

/// Emit `permission:ask` + await the user's response (or
/// timeout). Centralizes the Tier 4 ask path so the three
/// branches (path / shell / web_fetch) share the same IPC
/// flow.
///
/// **Wire path-field semantics (re-grill §1, 2.4 check)**:
///
/// - `path_or_cmd` is the full argument string the user needs
///   to see in the modal (path for path tools, command for
///   shell, URL for web_fetch). It is ALWAYS used for
///   `build_ask_reason` and `match_value_for_allow_always` —
///   both need the "what did the LLM try to do" text, not
///   a path-only scope.
/// - `path_for_modal` is the **optional** string to surface in
///   the `PermissionAskPayload.path` field — the field the
///   frontend's `<PermissionModal>` reads to render the
///   "path scope" row (in-repo / out-of-repo badge). The
///   frontend's `v-if="hasPath"` hides the row entirely when
///   the field is absent (the struct has
///   `#[serde(skip_serializing_if = "Option::is_none")])`.
///   Per the re-grill spec, **only path tools** populate this
///   field (read_file / write_file / edit_file / list_dir /
///   grep / glob). Shell and web_fetch pass `None` because
///   the modal renders the command / URL inline via
///   `toolInput` (no separate "path scope" row) — surfacing
///   a misleading "仓库外" badge for a shell command or URL
///   is a UX bug.
async fn ask_path(
    app: &AppHandle,
    db: &SqlitePool,
    store: &PermissionStore,
    ctx: &PermissionContext,
    tool_name: &str,
    tool_input: &serde_json::Value,
    path_or_cmd: &str,
    path_for_modal: Option<&str>,
    token: &tokio_util::sync::CancellationToken,
) -> Decision {
 let rid = uuid::Uuid::new_v4().to_string();
 let risk = risk_for_tool(tool_name);
 let reason = build_ask_reason(tool_name, path_or_cmd, risk);
 let payload = PermissionAskPayload {
 rid: rid.clone(),
 tool_name: tool_name.to_string(),
 tool_input: tool_input.clone(),
 risk,
 reason: Some(reason.clone()),
 // The `path` field is populated ONLY for path tools
 // (the spec's Q10 "path 范围行" UX). For shell / web_fetch
 // the field is `None` and serde's `skip_serializing_if`
 // keeps it OFF the wire — so the frontend's `v-if="hasPath"`
 // does not render a misleading scope row for non-path
 // asks.
 path: path_for_modal.map(|p| p.to_string()),
 };
 if let Err(e) = app.emit("permission:ask", &payload) {
 tracing::warn!(error = %e, "permission::check: failed to emit permission:ask");
 }
 let _ = record_audit(
 app,
 db,
 ctx,
 AuditKind::ToolPermissionAsk,
 tool_name,
 tool_input,
 Some(&reason),
 )
 .await;
 let rx = register_ask(store, rid.clone()).await;
 let resp = tokio::select! {
 biased;
 _ = token.cancelled() => {
 let _ = resolve_ask(store, &rid, PermissionResponse::Deny).await;
 let _ = record_audit(app, db, ctx, AuditKind::RequestCancelled, tool_name, tool_input, None).await;
 return Decision::Deny {
 reason: "request cancelled by user".to_string(),
 critical: false,
 };
 }
 _ = tokio::time::sleep(ASK_TIMEOUT) => {
 let mut map = store.lock().await;
 map.remove(&rid);
 drop(map);
 tracing::warn!(
 session_id = %ctx.session_id,
 tool = %tool_name,
 "permission::check: Tier 4 timed out after 120s"
 );
 let _ = record_audit(app, db, ctx, AuditKind::PermissionTimeout, tool_name, tool_input, None).await;
 return Decision::Deny {
 reason: "permission timed out after 120s, treat as denied".to_string(),
 critical: false,
 };
 }
 resp = rx => resp,
 };
 match resp {
 Ok(PermissionResponse::AllowOnce) => {
 let _ = record_audit(app, db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None).await;
 Decision::Allow
 }
 Ok(PermissionResponse::AllowAlways) => {
 // Persist the "always allow" row with the
 // tool-specific match_kind. The match_value is
 // computed by `match_value_for_allow_always`
 // (path → parent/* glob; shell → first token;
 // web_fetch → tool/NULL).
 let (kind, value) = match_value_for_allow_always(tool_name, tool_input, path_or_cmd);
 if let Err(e) = crate::db::grant_tool_permission(
 db,
 &ctx.session_id,
 tool_name,
 kind,
 value.as_deref(),
 )
 .await
 {
 tracing::warn!(
 error = %e,
 "permission::check: grant_tool_permission failed (non-fatal)"
 );
 }
 let _ = record_audit(app, db, ctx, AuditKind::PermissionGranted, tool_name, tool_input, None).await;
 Decision::Allow
 }
 Ok(PermissionResponse::Deny) => {
 let _ = record_audit(app, db, ctx, AuditKind::ToolDenied, tool_name, tool_input, None).await;
 Decision::Deny {
 reason: "user denied".to_string(),
 critical: false,
 }
 }
 Err(_) => {
 let _ = record_audit(app, db, ctx, AuditKind::ToolDenied, tool_name, tool_input, None).await;
 Decision::Deny {
 reason: "permission ask cancelled before response".to_string(),
 critical: false,
 }
 }
 }
}

/// Build the human-readable reason string shown in the
/// PermissionModal header. Re-grill Q1 path-based: the
/// reason explicitly mentions the path / command / URL so
/// the user can decide without inspecting `toolInput` JSON.
fn build_ask_reason(tool_name: &str, path_or_cmd: &str, risk: Risk) -> String {
 if tool_name == "shell" {
 format!(
 "The tool {} requires your confirmation (risk: {}, command: {}).",
 tool_name,
 risk.label_cn(),
 path_or_cmd
 )
 } else if tool_name == "web_fetch" {
 format!(
 "The tool {} requires your confirmation (risk: {}, URL: {}).",
 tool_name,
 risk.label_cn(),
 path_or_cmd
 )
 } else {
 format!(
 "The tool {} requires your confirmation (risk: {}, path: {}).",
 tool_name,
 risk.label_cn(),
 path_or_cmd
 )
 }
}

/// Compute the `(match_kind, match_value)` pair to write to
/// `session_tool_permissions` on a user's "始终允许" click.
/// Re-grill Q6: wire the 3 match_kind variants. Q8: path
/// uses parent-directory + `*` glob (sqlite GLOB `*` does
/// not cross `/`).
///
/// **Path tool**: parent directory + `/*` (Q8). E.g.
/// `/Users/me/Documents/notes.md` → `match_value = '/Users/me/Documents/*'`.
///
/// **Shell**: first whitespace token (Q7). E.g.
/// `cargo test` → `match_value = 'cargo'`.
///
/// **Web fetch**: tool match (Q6 "web_fetch 始终允许 = 整 tool");
/// per-domain persistence is OOS for the re-grill (deferred
/// to PR3+).
pub(crate) fn match_value_for_allow_always(
 tool_name: &str,
 _tool_input: &serde_json::Value,
 path_or_cmd: &str,
) -> (&'static str, Option<String>) {
 match classify_tool(tool_name) {
 ToolKind::Path => {
 // parent + /*  glob
 let p = std::path::Path::new(path_or_cmd);
 let glob = match p.parent() {
 Some(parent) if !parent.as_os_str().is_empty() => {
 format!("{}/*", parent.display())
 }
 _ => format!("{}/*", path_or_cmd),
 };
 ("path", Some(glob))
 }
 ToolKind::Shell => {
 // first token
 let prefix = shell_trust::first_token_for_allow_always(path_or_cmd);
 ("prefix", Some(prefix))
 }
 ToolKind::WebFetch => {
 // tool-level grant (per-domain deferred to PR3+)
 ("tool", None)
 }
 ToolKind::Other => {
 // Future tool — fall back to `tool` match (no glob).
 ("tool", None)
 }
 }
}

// ---------------------------------------------------------------------------
// Audit helper
// ---------------------------------------------------------------------------

/// Build the payload JSON for an audit row and write it. Errors
/// are logged at `warn!` but never propagated — the audit log
/// is best-effort (a write failure must not break the agent
/// loop).
///
/// The `critical` field is included in the payload so the
/// PermissionModal (PR3) / C4 audit-log UI can render the
/// 3px red border + shield-x icon styling on critical-risk
/// denials. The flag is `true` only for Tier 2 hard-kill-list
/// denials (where the kill list is intrinsically
/// catastrophic); Tier 4 mode denials are `false` (the LLM
/// is "just" in a read-only mode, not a catastrophic
/// operation). Tier 3 user-deny / timeout / cancel paths
/// are also `false` (the user opted out, nothing catastrophic).
async fn record_audit(
 _app: &AppHandle,
 db: &SqlitePool,
 ctx: &PermissionContext,
 kind: AuditKind,
 tool_name: &str,
 tool_input: &serde_json::Value,
 reason: Option<&str>,
) -> Result<(), sqlx::Error> {
 // Map audit kind to critical flag: only Tier 2 hard-kill
 // denials are critical. Everything else is a "normal" path.
 let critical = matches!(
 kind,
 AuditKind::ToolDenied | AuditKind::ToolDeniedYolo
 );
 let payload = serde_json::json!({
 "tool_name": tool_name,
 "tool_input": tool_input,
 "reason": reason,
 "mode": ctx.mode.as_str(),
 "critical": critical,
 });
 let payload_str = payload.to_string();
 crate::db::record_audit_event(
 db,
 &ctx.session_id,
 kind.as_str(),
 Some(&payload_str),
 )
 .await
}

/// C4 PR1 (2026-06-14): record a `tool_executed` audit row. Unlike
/// [`record_audit`], this row carries **duration + exit_code**
/// instead of the ⑨ 关 payload shape (`reason` / `mode` /
/// `critical`). The agent loop calls this from the tool-execution
/// loop right after `execute_tool` returns, with the wall-clock
/// delta measured in the loop and the exit code the tool reported.
///
/// **Best-effort** (same contract as `record_audit`): a DB write
/// failure is logged at `warn!` and swallowed — the agent loop
/// never sees the error and continues normally.
///
/// `duration_ms` is `u128` from `Duration::as_millis()`; JSON has
/// no problem serializing the wider type and the value in practice
/// is well under `u64::MAX` (a single tool call rarely exceeds
/// MAX_TIMEOUT_MS = 600_000ms).
///
/// `exit_code` is `None` for tools that don't produce one
/// (`read_file` / `write_file` / `edit_file` / `grep` / `glob` /
/// `list_dir` / `web_fetch`); `Some(code)` for `shell`. The C4
/// audit-log UI uses `Some(0)` vs `Some(non-zero)` to color the
/// icon, and `None` for "N/A" — don't hardcode 0 to represent
/// "no exit code", that would conflate "succeeded" with "N/A".
pub async fn record_tool_executed_audit(
    db: &SqlitePool,
    session_id: &str,
    tool_name: &str,
    tool_input: &serde_json::Value,
    duration_ms: u128,
    exit_code: Option<i32>,
) -> Result<(), sqlx::Error> {
    let payload = serde_json::json!({
        "tool_name": tool_name,
        "tool_input": tool_input,
        "duration_ms": duration_ms,
        "exit_code": exit_code,
    });
    let payload_str = payload.to_string();
    crate::db::record_audit_event(
        db,
        session_id,
        AuditKind::ToolExecuted.as_str(),
        Some(&payload_str),
    )
    .await
}

// ---------------------------------------------------------------------------
// ⑧a Mode check helpers — used by agent/chat.rs before every turn
// ---------------------------------------------------------------------------

/// Per-turn system prompt prefix for the active mode. Injected
/// at the head of the system prompt so the LLM is grounded on
/// the mode's behavioral contract on every request (this is the
/// "per-turn system prompt" layer of the ⑧a triple defense — the
/// other two are tool-list filtering and runtime intercept).
pub fn mode_system_prefix(mode: Mode) -> &'static str {
 match mode {
 Mode::Plan => "\
You are in Plan mode. You may read files, search, and run readonly \
commands (cat / grep / git log / etc.) to understand the codebase, \
but you CANNOT execute any write tool (write_file, edit_file, shell \
with side effects). If the user asks for an edit, propose the \
change as a diff and ask them to switch to Edit mode to apply it.",
 Mode::Yolo => "\
You are in Yolo mode. All user-confirmation modals are \
automatically skipped. Hard-deny rules (rm -rf /, mkfs, dd if=, \
fork bombs, write-to-disk, chmod 777 /, force-push to protected \
branches, curl|bash) are STILL enforced and will be silently \
denied. Operate with care.",
 Mode::Background => "\
You are in Background mode. (Reserved — not currently exposed in \
the UI.)",
 Mode::Edit => "\
You are in Edit mode (the default). You have full access to all \
tools. Destructive shell commands are silently denied; other \
commands trigger a one-time confirmation modal the first time the \
user sees them per session.",
 }
}

/// ⑧a tool list filter: Plan drops the write tools, Edit/Yolo
/// keep the full set. Returns the filtered tool list to pass
/// to `ChatRequest.tools`. Plan mode still emits the full
/// tool list to the LLM in some Claude-Code-like designs; we
/// choose the explicit filter per audit §2 recommendation
/// (saves a turn + reduces confusion). 3 档化 2026-06-13:
/// Review 移除, 只剩 Plan 一个只读 mode。
pub fn filter_tools_for_mode(
 tools: Vec<crate::llm::ToolDef>,
 mode: Mode,
) -> Vec<crate::llm::ToolDef> {
 match mode {
 Mode::Plan => tools
 .into_iter()
 .filter(|t| !matches!(t.name.as_str(), "write_file" | "edit_file" | "shell"))
 .collect(),
 _ => tools,
 }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
 use super::*;

 #[test]
 fn mode_as_str_round_trip() {
 for m in [Mode::Edit, Mode::Plan, Mode::Yolo, Mode::Background] {
 assert_eq!(Mode::from_str_opt(m.as_str()), m);
 }
 }

 #[test]
 fn mode_from_str_unknown_defaults_to_chat() {
 assert_eq!(Mode::from_str_opt(""), Mode::Edit);
 assert_eq!(Mode::from_str_opt("nonsense"), Mode::Edit);
 assert_eq!(Mode::from_str_opt("PLAN"), Mode::Edit); // case-sensitive
 }

 #[test]
 fn risk_for_tool_categorization() {
 assert_eq!(risk_for_tool("read_file"), Risk::Low);
 assert_eq!(risk_for_tool("grep"), Risk::Low);
 assert_eq!(risk_for_tool("write_file"), Risk::Medium);
 assert_eq!(risk_for_tool("edit_file"), Risk::Medium);
 assert_eq!(risk_for_tool("shell"), Risk::High);
 assert_eq!(risk_for_tool("web_fetch"), Risk::Low);
 }

 #[test]
 fn risk_label_cn_is_full_text() {
 assert_eq!(Risk::Low.label_cn(), "低");
 assert_eq!(Risk::Medium.label_cn(), "中");
 assert_eq!(Risk::High.label_cn(), "高");
 assert_eq!(Risk::Critical.label_cn(), "极高");
 }

 #[test]
 fn filter_tools_for_mode_drops_writes_in_plan_review() {
 let tools = vec![
 crate::llm::ToolDef::new_for_test("read_file"),
 crate::llm::ToolDef::new_for_test("write_file"),
 crate::llm::ToolDef::new_for_test("shell"),
 crate::llm::ToolDef::new_for_test("grep"),
 ];
 let filtered = filter_tools_for_mode(tools.clone(), Mode::Plan);
 let names: Vec<&str> = filtered.iter().map(|t| t.name.as_str()).collect();
 assert!(names.contains(&"read_file"));
 assert!(names.contains(&"grep"));
 assert!(!names.contains(&"write_file"));
 assert!(!names.contains(&"shell"));

 let filtered = filter_tools_for_mode(tools.clone(), Mode::Plan);
 let names: Vec<&str> = filtered.iter().map(|t| t.name.as_str()).collect();
 assert!(!names.contains(&"write_file"));
 assert!(!names.contains(&"shell"));
 }

 #[test]
 fn filter_tools_for_mode_keeps_full_for_chat_yolo() {
 let tools = vec![
 crate::llm::ToolDef::new_for_test("read_file"),
 crate::llm::ToolDef::new_for_test("write_file"),
 crate::llm::ToolDef::new_for_test("shell"),
 ];
 for m in [Mode::Edit, Mode::Yolo] {
 let filtered = filter_tools_for_mode(tools.clone(), m);
 assert_eq!(filtered.len(), tools.len(), "Mode {:?} should keep all tools", m);
 }
 }

 #[test]
 fn mode_system_prefix_is_non_empty() {
 for m in [Mode::Edit, Mode::Plan, Mode::Yolo, Mode::Background] {
 assert!(!mode_system_prefix(m).is_empty());
 }
 }

 #[test]
 fn audit_kind_round_trip() {
 for k in [
        AuditKind::ToolDenied,
        AuditKind::ToolAllowed,
        AuditKind::ToolPermissionAsk,
        AuditKind::PermissionGranted,
        AuditKind::ModeChanged,
        AuditKind::YoloEntered,
        AuditKind::YoloExited,
        AuditKind::ToolDeniedYolo,
        AuditKind::PermissionTimeout,
        AuditKind::RequestCancelled,
        AuditKind::ToolExecuted,
    ] {
        let s = k.as_str();
        assert!(!s.is_empty());
        assert!(s.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
    }
    // C4 PR1 (2026-06-14): lock the new variant's wire string so a
    // future rename / typo here breaks the test instead of corrupting
    // audit rows the frontend can no longer dispatch on.
    assert_eq!(AuditKind::ToolExecuted.as_str(), "tool_executed");
 }

 // =====================================================================
 // Re-grill 2026-06-13: path-based / prefix / Yolo bypass / Plan
 // early-block / match_kind wiring tests.
 // =====================================================================

 /// classify_tool returns the right variant for every built-in
 /// tool. Locked list — a future tool addition must add a
 /// classify match arm + a test here.
 #[test]
 fn classify_tool_dispatch() {
 assert_eq!(super::classify_tool("read_file"), super::ToolKind::Path);
 assert_eq!(super::classify_tool("write_file"), super::ToolKind::Path);
 assert_eq!(super::classify_tool("edit_file"), super::ToolKind::Path);
 assert_eq!(super::classify_tool("list_dir"), super::ToolKind::Path);
 assert_eq!(super::classify_tool("grep"), super::ToolKind::Path);
 assert_eq!(super::classify_tool("glob"), super::ToolKind::Path);
 assert_eq!(super::classify_tool("shell"), super::ToolKind::Shell);
 assert_eq!(super::classify_tool("web_fetch"), super::ToolKind::WebFetch);
 assert_eq!(super::classify_tool("unknown_future_tool"), super::ToolKind::Other);
 }

 /// extract_path_arg reads the `path` key (with `cwd` /
 /// `working_directory` fallbacks).
 #[test]
 fn extract_path_arg_reads_path_key() {
 let v = serde_json::json!({"path": "/abs/path.txt"});
 assert_eq!(super::extract_path_arg("read_file", &v), Some("/abs/path.txt".to_string()));
 }

 #[test]
 fn extract_path_arg_falls_back_to_cwd() {
 let v = serde_json::json!({"cwd": "/fallback"});
 assert_eq!(super::extract_path_arg("read_file", &v), Some("/fallback".to_string()));
 }

 #[test]
 fn extract_path_arg_returns_none_when_missing() {
 let v = serde_json::json!({});
 assert_eq!(super::extract_path_arg("read_file", &v), None);
 }

 /// sqlite_glob_match: the *doesn't cross /* rule. This is
 /// the core invariant of Tier 4 path-grant matching — a
 /// glob `/foo/*` must NOT match `/foo/bar/baz`.
 #[test]
 fn sqlite_glob_match_star_does_not_cross_slash() {
 assert!(super::sqlite_glob_match("/foo/*", "/foo/notes.md"));
 assert!(super::sqlite_glob_match("/foo/*", "/foo/a"));
 // Negative: a nested dir is NOT matched by the parent's
 // single-asterisk glob (sqlite GLOB semantics).
 assert!(!super::sqlite_glob_match("/foo/*", "/foo/bar/notes.md"));
 assert!(!super::sqlite_glob_match("/foo/*", "/bar/notes.md"));
 }

 /// sqlite_glob_match: `?` matches exactly one char.
 #[test]
 fn sqlite_glob_match_question_mark() {
 assert!(super::sqlite_glob_match("/foo/?.txt", "/foo/a.txt"));
 assert!(!super::sqlite_glob_match("/foo/?.txt", "/foo/ab.txt"));
 }

 /// sqlite_glob_match: empty pattern matches only empty
 /// text.
 #[test]
 fn sqlite_glob_match_empty() {
 assert!(super::sqlite_glob_match("", ""));
 assert!(!super::sqlite_glob_match("", "x"));
 }

 /// sqlite_glob_match: literal pattern (no metachars).
 #[test]
 fn sqlite_glob_match_literal() {
 assert!(super::sqlite_glob_match("/foo/bar", "/foo/bar"));
 assert!(!super::sqlite_glob_match("/foo/bar", "/foo/baz"));
 }

 /// match_value_for_allow_always: path tools use parent + /*
 /// glob. (Q8)
 #[test]
 fn match_value_for_allow_always_path_uses_parent_glob() {
 let v = serde_json::json!({});
 let (kind, val) = super::match_value_for_allow_always(
 "read_file", &v, "/Users/me/Documents/notes.md",
 );
 assert_eq!(kind, "path");
 assert_eq!(val, Some("/Users/me/Documents/*".to_string()));
 }

 /// match_value_for_allow_always: path tools with a relative
 /// input still produce a sensible parent glob. (The caller
 /// would normally pass an absolute path because the
 /// permission layer resolves relative → cwd.join, but the
 /// function is robust to either.)
 #[test]
 fn match_value_for_allow_always_path_basename_only() {
 let v = serde_json::json!({});
 let (kind, val) = super::match_value_for_allow_always(
 "read_file", &v, "notes.md",
 );
 assert_eq!(kind, "path");
 assert_eq!(val, Some("notes.md/*".to_string()));
 }

 /// match_value_for_allow_always: shell uses first token (Q7).
 #[test]
 fn match_value_for_allow_always_shell_uses_first_token() {
 let v = serde_json::json!({});
 let (kind, val) = super::match_value_for_allow_always(
 "shell", &v, "cargo test --release",
 );
 assert_eq!(kind, "prefix");
 assert_eq!(val, Some("cargo".to_string()));
 }

 /// match_value_for_allow_always: web_fetch always grants
 /// the whole tool (per-domain is OOS).
 #[test]
 fn match_value_for_allow_always_web_fetch_uses_tool() {
 let v = serde_json::json!({});
 let (kind, val) = super::match_value_for_allow_always(
 "web_fetch", &v, "https://example.com",
 );
 assert_eq!(kind, "tool");
 assert_eq!(val, None);
 }

 /// build_ask_reason: path / shell / web_fetch produce
 /// different reason shapes (Q1 "path-based 弹窗判定",
 /// Q10 "保留 risk + path 范围行").
 #[test]
 fn build_ask_reason_mentions_path_for_path_tools() {
 let r = super::build_ask_reason("read_file", "/etc/passwd", Risk::High);
 assert!(r.contains("read_file"));
 assert!(r.contains("/etc/passwd"));
 assert!(r.contains("高"));
 }

 #[test]
 fn build_ask_reason_mentions_command_for_shell() {
 let r = super::build_ask_reason("shell", "rm -rf /tmp/foo", Risk::High);
 assert!(r.contains("rm -rf /tmp/foo"));
 }

 #[test]
 fn build_ask_reason_mentions_url_for_web_fetch() {
 let r = super::build_ask_reason("web_fetch", "https://example.com", Risk::Low);
 assert!(r.contains("https://example.com"));
 }

 /// PermissionAskPayload wire shape: `path` is camelCase
 /// (renamed from Rust `path`).
 #[test]
 fn permission_ask_payload_wire_shape_includes_path() {
 let p = PermissionAskPayload {
 rid: "test-rid".to_string(),
 tool_name: "read_file".to_string(),
 tool_input: serde_json::json!({"path": "/x"}),
 risk: Risk::High,
 reason: Some("test".to_string()),
 path: Some("/x".to_string()),
 };
 let s = serde_json::to_string(&p).unwrap();
 // The wire field is `path` (camelCase == snake_case for a
 // single-word field).
 assert!(s.contains("\"path\":\"/x\""), "wire shape: {}", s);
 assert!(s.contains("\"toolName\":\"read_file\""), "camelCase: {}", s);
 }

 /// PermissionAskPayload: `path` is omitted when None
 /// (`skip_serializing_if`).
 #[test]
 fn permission_ask_payload_omits_path_when_none() {
 let p = PermissionAskPayload {
 rid: "test-rid".to_string(),
 tool_name: "shell".to_string(),
 tool_input: serde_json::json!({"command": "ls"}),
 risk: Risk::High,
 reason: Some("test".to_string()),
 path: None,
 };
 let s = serde_json::to_string(&p).unwrap();
 assert!(!s.contains("\"path\""), "path should be skipped: {}", s);
 }

 // =====================================================================
 // 2.4 check (re-grill 2026-06-13): wire-shape guards for the
 // "path field is populated ONLY for path tools" rule.
 //
 // The bug was that the previous `ask_path` implementation
 // unconditionally set `path: Some(path_or_cmd.to_string())`
 // in the payload, so shell / web_fetch payloads also got a
 // `path` field on the wire. The frontend's `<PermissionModal>`
 // `v-if="hasPath"` then rendered a misleading "path scope" row
 // (with a "仓库外" badge) for shell commands and URLs.
 //
 // These tests lock the wire shape: shell and web_fetch must
 // NOT include the `path` key in the serialized
 // `PermissionAskPayload` (mimicking the `path_for_modal = None`
 // pass-through in the new `ask_path` signature). Path tools
 // MUST include it.
 // =====================================================================

 /// Wire shape: shell ask has NO `path` field (mimics
 /// `ask_path(.., cmd, None, ..)` from the Tier 4 Shell branch).
 /// The modal renders the command via `toolInput`, so a path
 /// scope row would be wrong.
 #[test]
 fn permission_ask_payload_omits_path_for_shell() {
 let p = PermissionAskPayload {
 rid: "shell-rid".to_string(),
 tool_name: "shell".to_string(),
 tool_input: serde_json::json!({"command": "rm -rf /tmp/foo"}),
 risk: Risk::High,
 reason: Some(
 "The tool shell requires your confirmation (risk: 高, command: rm -rf /tmp/foo).".to_string(),
 ),
 // Mirrors the new `ask_path` body: `path_for_modal = None`
 // for shell, so the payload's `path` field is `None`.
 path: None,
 };
 let s = serde_json::to_string(&p).unwrap();
 assert!(
 !s.contains("\"path\""),
 "shell payload must NOT include a path field (would confuse PermissionModal): {}",
 s
 );
 // The other fields are still present and correctly camelCased.
 assert!(s.contains("\"toolName\":\"shell\""), "toolName: {}", s);
 assert!(s.contains("\"command\":\"rm -rf /tmp/foo\""), "toolInput echoed: {}", s);
 assert!(s.contains("\"reason\":"), "reason still present: {}", s);
 }

 /// Wire shape: web_fetch ask has NO `path` field (mimics
 /// `ask_path(.., url, None, ..)` from the Tier 4 WebFetch
 /// branch). The modal renders the URL via `toolInput`, so a
 /// path scope row would be wrong (URL is not a filesystem
 /// path, so the in-repo / out-of-repo badge would be
 /// meaningless).
 #[test]
 fn permission_ask_payload_omits_path_for_web_fetch() {
 let p = PermissionAskPayload {
 rid: "webfetch-rid".to_string(),
 tool_name: "web_fetch".to_string(),
 tool_input: serde_json::json!({"url": "https://example.com/api"}),
 risk: Risk::Low,
 reason: Some(
 "The tool web_fetch requires your confirmation (risk: 低, URL: https://example.com/api).".to_string(),
 ),
 // Mirrors the new `ask_path` body: `path_for_modal = None`
 // for web_fetch, so the payload's `path` field is `None`.
 path: None,
 };
 let s = serde_json::to_string(&p).unwrap();
 assert!(
 !s.contains("\"path\""),
 "web_fetch payload must NOT include a path field (URL is not a path): {}",
 s
 );
 assert!(s.contains("\"toolName\":\"web_fetch\""), "toolName: {}", s);
 assert!(
 s.contains("\"url\":\"https://example.com/api\""),
 "toolInput echoed: {}",
 s
 );
 }

 /// Wire shape: path-tool ask DOES include the `path` field
 /// (mimics `ask_path(.., &path_owned, Some(&path_owned), ..)`
 /// from the Tier 4 Path branch — the only branch that
 /// populates `path_for_modal`). The modal reads this to
 /// render the in-repo / out-of-repo badge.
 #[test]
 fn permission_ask_payload_includes_path_for_path_tool() {
 let p = PermissionAskPayload {
 rid: "path-rid".to_string(),
 tool_name: "read_file".to_string(),
 tool_input: serde_json::json!({"path": "/Users/me/repo/src/foo.rs"}),
 risk: Risk::Low,
 reason: Some(
 "The tool read_file requires your confirmation (risk: 低, path: /Users/me/repo/src/foo.rs).".to_string(),
 ),
 // Mirrors the new `ask_path` body for path tools: the
 // `path_for_modal` argument is `Some(&path_owned)`, so the
 // payload's `path` field is `Some(...)`.
 path: Some("/Users/me/repo/src/foo.rs".to_string()),
 };
 let s = serde_json::to_string(&p).unwrap();
 assert!(
 s.contains("\"path\":\"/Users/me/repo/src/foo.rs\""),
 "path tool payload MUST include the path field (modal reads it for in-repo / out-of-repo badge): {}",
 s
 );
 assert!(s.contains("\"toolName\":\"read_file\""), "toolName: {}", s);
 }
}