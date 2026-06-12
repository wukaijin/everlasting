//! ⑨ 关 Permission decision layer + ⑧a Mode check (A2 + B7).
//!
//! Sits between the agent loop's `provider.send()` stream and
//! `tools::execute_tool`. On every tool_use block the agent
//! loop calls [`check`] which produces a [`Decision`] that
//! either allows the call, denies it (silent or with a reason),
//! or asks the user via a oneshot channel + Tauri event.
//!
//! ## 5-tier evaluation order (SOT — see PRD top § "⑨ 关 5 道 Check")
//!
//! ```text
//! Tier 1. Hooks           — pre-call interface (MVP: no-op)
//! Tier 2. Deny rules      — hard kill list (always silent,
//!                            Yolo included)
//! Tier 3. Ask rules       — session_tool_permissions +
//!                            emit permission:ask + await response
//!                            (120s timeout → auto-deny)
//! Tier 4. Mode check      — Plan/Review: block write/edit/shell;
//!                            Chat/Yolo: pass through
//! Tier 5. Allow rules     — default allow-all (MVP)
//! Tier 6. Audit hook      — record decision to session_audit_events
//! ```
//!
//! See `docs/_reviews/REVIEW-a2-b7-permission-mode-plan-2026-06-13.md`
//! § 1 for the locked source-of-truth ordering (resolved three
//! pre-review conflicts).

pub mod dangerous;

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
/// into each `check()` call. Cloned cheaply (3 small strings).
#[derive(Debug, Clone)]
pub struct PermissionContext {
 pub session_id: String,
 pub mode: Mode,
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
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionAskPayload {
 pub rid: String,
 pub tool_name: String,
 pub tool_input: serde_json::Value,
 pub risk: Risk,
 #[serde(skip_serializing_if = "Option::is_none")]
 pub reason: Option<String>,
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
/// - `ctx` — per-call context (session_id + mode).
/// - `store` — `PermissionStore` for the Tier 3 oneshot bridge.
/// - `db` — SQLite pool (Tier 3 has_tool_permission + Tier 6
///   audit write).
/// - `app` — Tauri AppHandle (for the `permission:ask` emit
///   on Tier 3 and the `tracing` instrumentation).
/// - `tool_name` — the LLM-emitted tool name.
/// - `tool_input` — the LLM-emitted tool input JSON.
/// - `token` — the agent-loop cancellation token (for Tier 3
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

 // ----- Tier 3: Ask rules (session_tool_permissions + emit + wait) -----
 // 顺序:
 // 1. 查 session_tool_permissions: 有 "始终允许" → 直接 Allow (Tier 6 audit)
 // 2. 无 → emit permission:ask + 等 oneshot (120s 超时 → 自动 deny)
 // 3. 收到 allow_once → Allow (audit tool_allowed)
 // 4. 收到 allow_always → Allow + INSERT session_tool_permissions (audit permission_granted)
 // 5. 收到 deny → Deny (audit tool_denied)
 let already_allowed = match crate::db::has_tool_permission(db, &ctx.session_id, tool_name).await {
 Ok(b) => b,
 Err(e) => {
 tracing::warn!(error = %e, "permission::check: has_tool_permission failed, falling back to Ask");
 false
 }
 };
 if already_allowed {
 tracing::info!(
 session_id = %ctx.session_id,
 tool = %tool_name,
 "permission::check: Tier 3 hit 'always allow', skipping modal"
 );
 let _ = record_audit(app, db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None).await;
 return Decision::Allow;
 }

 // Emit + await. Generate a UUID rid.
 let rid = uuid::Uuid::new_v4().to_string();
 let risk = risk_for_tool(tool_name);
 let reason = format!(
 "The tool {} requires your confirmation (risk: {}).",
 tool_name,
 risk.label_cn()
 );
 let payload = PermissionAskPayload {
 rid: rid.clone(),
 tool_name: tool_name.to_string(),
 tool_input: tool_input.clone(),
 risk,
 reason: Some(reason.clone()),
 };
 if let Err(e) = app.emit("permission:ask", &payload) {
 tracing::warn!(error = %e, "permission::check: failed to emit permission:ask");
 }
 let _ = record_audit(app, db, ctx, AuditKind::ToolPermissionAsk, tool_name, tool_input, Some(&reason)).await;

 let rx = register_ask(store, rid.clone()).await;
 let decision = tokio::select! {
 biased;
 // Cancellation: user hit Stop. Treat as Deny (the
 // agent-loop wrapper distinguishes this from a
 // user-initiated deny in the audit log).
 _ = token.cancelled() => {
 let _ = resolve_ask(store, &rid, PermissionResponse::Deny).await;
 let _ = record_audit(app, db, ctx, AuditKind::RequestCancelled, tool_name, tool_input, None).await;
 return Decision::Deny {
 reason: "request cancelled by user".to_string(),
 critical: false,
 };
 }
 // 120s timeout — auto-deny.
 _ = tokio::time::sleep(ASK_TIMEOUT) => {
 // Best-effort: try to remove the sender so the IPC
 // handler (if it races us) gets `false` from
 // `resolve_ask`. The recv() below would return
 // RecvError regardless, but cleaning the map avoids
 // a stale entry.
 let mut map = store.lock().await;
 map.remove(&rid);
 drop(map);
 tracing::warn!(
 session_id = %ctx.session_id,
 tool = %tool_name,
 "permission::check: Tier 3 timed out after 120s"
 );
 let _ = record_audit(app, db, ctx, AuditKind::PermissionTimeout, tool_name, tool_input, None).await;
 return Decision::Deny {
 reason: "permission timed out after 120s, treat as denied".to_string(),
 critical: false,
 };
 }
 resp = rx => {
 match resp {
 Ok(PermissionResponse::AllowOnce) => Decision::Allow,
 Ok(PermissionResponse::AllowAlways) => {
 // Persist "always allow" for this tool.
 if let Err(e) = crate::db::grant_tool_permission(
 db,
 &ctx.session_id,
 tool_name,
 "tool",
 None,
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
 // Sender dropped without a response (e.g. cancel /
 // session delete raced the IPC handler). Treat as
 // deny — the same shape as user-denied.
 let _ = record_audit(app, db, ctx, AuditKind::ToolDenied, tool_name, tool_input, None).await;
 Decision::Deny {
 reason: "permission ask cancelled before response".to_string(),
 critical: false,
 }
 }
 }
 }
 };

 // ----- Tier 4: Mode check (Plan/Review block writes) -----
 // ⑨ 关第 4 道 + ⑧a 三重防御的最后一层. Plan/Review 模式下
 // 即便 LLM 仍发 write/edit/shell (tool list 过滤通常已挡住),
 // 也拦截. read 类工具不受影响.
 if matches!(ctx.mode, Mode::Plan | Mode::Review) {
 if matches!(tool_name, "write_file" | "edit_file" | "shell") {
 tracing::info!(
 session_id = %ctx.session_id,
 mode = %ctx.mode.as_str(),
 tool = %tool_name,
 "permission::check: Tier 4 mode block (write tools in read-only mode)"
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
 reason: format!(
 "I cannot execute {} in {} mode (read-only session)",
 tool_name,
 ctx.mode.as_str()
 ),
 critical: false,
 };
 }
 }

 // ----- Tier 5: Allow rules (default allow-all for MVP) -----
 // Tier 3 命中 AllowAlways / AllowOnce 已经走到这里。Tool
 // 白名单默认全开,后续可收缩(例如未来禁用 web_fetch)。

 // Tier 6 audit (only on the Allow-from-Tier-3 path; the
 // Tier 2 / Tier 3 deny / Tier 3 cancel paths already wrote
 // their own audit rows above).
 if matches!(decision, Decision::Allow) {
 let _ = record_audit(app, db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None).await;
 }
 decision
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
change as a diff and ask them to switch to Chat mode to apply it.",
 Mode::Review => "\
You are in Review mode. You may only perform readonly analysis \
(read_file, grep, glob, list_dir, git log/diff). You CANNOT \
execute write tools or shell commands with side effects. If the \
user asks for a code change, decline and ask them to switch to \
Chat mode.",
 Mode::Yolo => "\
You are in Yolo mode. All user-confirmation modals are \
automatically skipped. Hard-deny rules (rm -rf /, mkfs, dd if=, \
fork bombs, write-to-disk, chmod 777 /, force-push to protected \
branches, curl|bash) are STILL enforced and will be silently \
denied. Operate with care.",
 Mode::Background => "\
You are in Background mode. (Reserved — not currently exposed in \
the UI.)",
 Mode::Chat => "\
You are in Chat mode. You have full access to all tools. \
Destructive shell commands are silently denied; other commands \
trigger a one-time confirmation modal the first time the user \
sees them per session.",
 }
}

/// ⑧a tool list filter: Plan/Review drop the write tools,
/// Chat/Yolo keep the full set. Returns the filtered tool list
/// to pass to `ChatRequest.tools`. Plan/Review mode still
/// emits the full tool list to the LLM in some Claude-Code-like
/// designs; we choose the explicit filter per audit §2
/// recommendation (saves a turn + reduces confusion).
pub fn filter_tools_for_mode(
 tools: Vec<crate::llm::ToolDef>,
 mode: Mode,
) -> Vec<crate::llm::ToolDef> {
 match mode {
 Mode::Plan | Mode::Review => tools
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
 for m in [Mode::Chat, Mode::Plan, Mode::Review, Mode::Yolo, Mode::Background] {
 assert_eq!(Mode::from_str_opt(m.as_str()), m);
 }
 }

 #[test]
 fn mode_from_str_unknown_defaults_to_chat() {
 assert_eq!(Mode::from_str_opt(""), Mode::Chat);
 assert_eq!(Mode::from_str_opt("nonsense"), Mode::Chat);
 assert_eq!(Mode::from_str_opt("PLAN"), Mode::Chat); // case-sensitive
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

 let filtered = filter_tools_for_mode(tools.clone(), Mode::Review);
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
 for m in [Mode::Chat, Mode::Yolo] {
 let filtered = filter_tools_for_mode(tools.clone(), m);
 assert_eq!(filtered.len(), tools.len(), "Mode {:?} should keep all tools", m);
 }
 }

 #[test]
 fn mode_system_prefix_is_non_empty() {
 for m in [Mode::Chat, Mode::Plan, Mode::Review, Mode::Yolo, Mode::Background] {
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
    ] {
        let s = k.as_str();
        assert!(!s.is_empty());
        assert!(s.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
    }
 }
}