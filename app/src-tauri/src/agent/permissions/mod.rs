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
//! ## Module layout (split 2026-06-23 out of a single `mod.rs`)
//!
//! - [`types`] — `Risk` / `Decision` / `PermissionContext` /
//!   `PermissionResponse`
//! - [`store`] — `PermissionStore` + `register_ask` / `resolve_ask`
//!   / `cancel_session_asks`
//! - [`payload`] — `PermissionAskPayload` (the `permission:ask` IPC
//!   wire shape)
//! - [`mode`] — `mode_system_prefix` + `filter_tools_for_mode`
//!   (⑧a Mode helpers)
//! - [`audit`] — `AuditKind` (17 variants, single enum) + the 3
//!   `record_*_audit` writers
//! - [`check`] — the 5-tier `check` + Tier 4 helpers (classify /
//!   extract_path_arg / grant checks / sqlite_glob_match /
//!   match_value_for_allow_always)
//! - [`ask`] — `ask_path` (the interactive Tier 4 round-trip) +
//!   `build_ask_reason` + `ASK_TIMEOUT`
//! - [`dangerous`] / [`shell_trust`] — self-contained pure-fn
//!   siblings (unchanged by the 2026-06-23 split)
//!
//! See `docs/_reviews/REVIEW-a2-b7-regrill-path-based-2026-06-13.md`
//! for the 10 re-grill decisions; see
//! `docs/IMPLEMENTATION.md §4` for the ADR.

pub mod dangerous;
pub mod shell_trust;
pub mod types;
pub mod store;
pub mod payload;
pub mod mode;
pub mod audit;
pub mod check;
pub mod ask;

// Test files (flat layout — aligned with `agent/tests_*.rs` style
// from the 2026-06-23 split batch).
pub mod tests_common;
pub mod tests_types;
pub mod tests_store;
pub mod tests_payload;
pub mod tests_mode;
pub mod tests_audit;
pub mod tests_check;
pub mod tests_ask;

// Re-export — keeps the `permissions::<item>` short path stable for
// external callers (`state.rs` / `chat_loop.rs` / `commands/*` /
// `subagent/sink.rs` etc. all reach these via the flat path).
pub use ask::ASK_TIMEOUT;
pub use audit::{record_message_resend_audit, record_tool_executed_audit, AuditKind};
pub use check::check;
pub use mode::{filter_tools_for_mode, mode_system_prefix};
pub use payload::PermissionAskPayload;
pub use store::{cancel_session_asks, new_permission_store, register_ask, resolve_ask, PendingAsk, PermissionStore};
pub use types::{risk_for_tool, Decision, PermissionContext, PermissionResponse, Risk};
