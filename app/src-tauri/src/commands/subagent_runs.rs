//! B6 PR3a (2026-06-20): Tauri commands for the subagent_runs
//! list/detail API surface used by the PR3 frontend
//! `<SubagentDrawer>`.
//!
//! Two commands live here, both thin wrappers around the
//! `db::subagent_runs` module:
//!
//! - [`list_subagent_runs_by_session`] — returns a projected
//!   [`db::subagent_runs::SubagentRunSummary`] list (no
//!   transcript column) for cheap per-session render. The
//!   frontend's `subagentRuns.fetchForSession` calls this on
//!   session switch + on the user opening the drawer for the
//!   first time per session.
//! - [`get_subagent_run`] — returns the full
//!   [`db::subagent_runs::SubagentRunRow`] (with
//!   `transcript_json` + `transcript_truncated`) for the
//!   drawer body. Called when the user clicks a specific
//!   `dispatch_subagent` tool card; the frontend store caches
//!   the result keyed by `runId`.
//!
//! Both commands return `Result<T, String>` per the project's
//! IPC convention (see `commands/permissions.rs::list_session_audit_events`
//! for the reference pattern). DB errors are wrapped as
//! `String` so the frontend's `invoke` rejection handler can
//! toast without needing a typed error.

use std::sync::Arc;

use tauri::State;

use crate::db;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// list_subagent_runs_by_session — projected list (no transcript)
// ---------------------------------------------------------------------------

/// List all `subagent_runs` for `session_id`, newest first, as
/// the projected [`db::subagent_runs::SubagentRunSummary`] (no
/// transcript column). The 4 MiB-cap'd transcript lives on the
/// per-run detail path (see `get_subagent_run`); the list view
/// stays small enough to ship on every session switch.
///
/// Empty session → empty `Vec` (NOT an error). DB failure →
/// wrapped `String` for the frontend toast path.
///
/// `allow(dead_code)` on the `#[tauri::command]` attribute
/// would be wrong here — the macro generates the IPC handler
/// that the frontend invokes. The `dead_code` allow is on the
/// `#[allow(dead_code)]` for `_state: &State<'_, Arc<AppState>>`
/// is NOT applied because the parameter is consumed by the
/// Tauri framework even though we only touch `state.db`.
#[tauri::command]
pub async fn list_subagent_runs_by_session(
    session_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<db::subagent_runs::SubagentRunSummary>, String> {
    db::subagent_runs::list_runs_summary_by_session(&state.db, &session_id)
        .await
        .map_err(|e| format!("list_subagent_runs_by_session failed: {}", e))
}

// ---------------------------------------------------------------------------
// get_subagent_run — full row including transcript
// ---------------------------------------------------------------------------

/// Fetch a single `subagent_runs` row by id. Returns
/// `Ok(Some(row))` for known ids, `Ok(None)` for unknown ids
/// (NOT an error — the frontend renders "run not found" on
/// `null`). DB failure → wrapped `String`.
///
/// The returned row carries `transcript_json` +
/// `transcript_truncated` — can be up to 4 MiB. The frontend's
/// `subagentRuns.fetchRun` caches the result keyed by `runId`
/// so opening the same drawer twice doesn't re-fetch.
#[tauri::command]
pub async fn get_subagent_run(
    run_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<Option<db::subagent_runs::SubagentRunRow>, String> {
    db::subagent_runs::get_run(&state.db, &run_id)
        .await
        .map_err(|e| format!("get_subagent_run failed: {}", e))
}
