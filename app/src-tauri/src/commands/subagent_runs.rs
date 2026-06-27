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

// ---------------------------------------------------------------------------
// L3b PR3 (2026-06-27): merge / discard worker IPC commands
//
// `merge_worker_run` and `discard_worker_run` are the Tauri
// command surface for the PR3 frontend `<SubagentDrawer>` merge /
// discard buttons (PR4). The LLM-side invocation path is the
// `merge_worker` / `discard_worker` tools (which route through
// `tools::execute_tool_inner`); these commands exist so the
// drawer can dispatch the same operations via IPC when the user
// clicks the button directly (no LLM round-trip).
//
// Both commands share the same backend helper as the tool layer
// (`tools::merge_worker::finalize_merge` and
// `tools::discard_worker::do_discard`); the IPC layer is a thin
// adapter that opens a blocking-task libgit2 work on the
// `parent_session_id`'s worktree + calls the helper.
// ---------------------------------------------------------------------------

/// Merge a worker's preserved `worker/<run_id>` branch into
/// the parent session's `session/<id>` branch. See
/// `tools::merge_worker` for the full contract (fast-forward /
/// 3-way merge, conflict returns the file list and leaves
/// both branches intact).
///
/// Wire shape: `(rid: String, run_id: String) -> String` —
/// the `rid` is the chat request id (unused by the merge
/// itself; reserved for future audit / correlation). The
/// `run_id` is the `subagent_runs.id` UUID.
#[tauri::command]
pub async fn merge_worker_run(
    _rid: String,
    run_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    // Look up the run row → find parent session id + worktree
    // path. We need the parent session id to look up the
    // session branch name (the parent is the one whose
    // `session/<id>` branch receives the merge).
    let run_row = db::subagent_runs::get_run(&state.db, &run_id)
        .await
        .map_err(|e| format!("merge_worker_run: failed to load run: {}", e))?
        .ok_or_else(|| format!("worker run not found: {}", run_id))?;
    let parent_session_id = run_row.parent_session_id.clone();
    let worker_wt = run_row
        .worktree_path
        .as_deref()
        .ok_or_else(|| {
            "worker has no worktree to merge (already merged or discarded)".to_string()
        })?
        .to_string();

    // Load the parent session row → resolve the worktree path
    // (the one whose `session/<id>` branch is the merge
    // target).
    let session_row = db::load_session(&state.db, &parent_session_id)
        .await
        .map_err(|e| format!("merge_worker_run: failed to load session: {}", e))?
        .ok_or_else(|| {
            format!(
                "merge_worker_run: parent session '{}' not found",
                parent_session_id
            )
        })?;
    let parent_wt = session_row
        .session
        .worktree_path
        .as_deref()
        .ok_or_else(|| "parent session has no worktree".to_string())?;
    let parent_wt = std::path::Path::new(parent_wt);

    // Stage 1: libgit2 merge (off-thread, since libgit2 is
    // blocking I/O).
    let run_id_for_task = run_id.clone();
    let parent_session_id_for_task = parent_session_id.clone();
    let parent_wt_for_task = parent_wt.to_path_buf();
    let merge_result = tauri::async_runtime::spawn_blocking(move || {
        crate::tools::merge_worker::do_merge_blocking(
            &parent_wt_for_task,
            &parent_session_id_for_task,
            &run_id_for_task,
        )
    })
    .await
    .map_err(|e| format!("merge_worker_run: task join failed: {}", e))?;

    match merge_result {
        Ok(msg) => {
            // Stage 2: post-merge cleanup (best-effort).
            let cleanup_result = crate::tools::merge_worker::finalize_merge(
                &state.db,
                &parent_session_id,
                &run_id,
            )
            .await;
            if let Err(e) = cleanup_result {
                tracing::warn!(
                    run_id = %run_id,
                    error = %e,
                    "merge_worker_run: post-merge cleanup failed (non-fatal)"
                );
            }
            // The message references the worker worktree path
            // by design — PR4's drawer can use it to
            // re-render. Currently unused; kept for parity
            // with the tool-layer shape.
            let _ = worker_wt;
            Ok(msg)
        }
        Err(msg) => Err(msg),
    }
}

/// Discard a worker's preserved branch + worktree. See
/// `tools::discard_worker` for the full contract (fail-fast on
/// already-destroyed; no idempotency in MVP).
#[tauri::command]
pub async fn discard_worker_run(
    _rid: String,
    run_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    crate::tools::discard_worker::do_discard(&state.db, &run_id).await
}
