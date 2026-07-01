//! L3b PR3 (2026-06-27): `discard_worker` tool.
//!
//! Discards a worker's preserved changes (the
//! `worker/<run_id>` branch + worktree) without merging. Used
//! when the user/parent LLM has decided the worker's output
//! isn't wanted (e.g. the worker made a wrong change, or the
//! task was exploratory and the results are noise).
//!
//! On success: calls PR1's
//! [`crate::git::worktree::destroy_worker`] to remove the
//! worker worktree + delete the `worker/<run_id>` branch +
//! clear the `subagent_runs.worktree_path` column. The parent
//! session's `session/<id>` branch is unaffected — the
//! worker's commits never made it into the parent's history
//! in the first place (they live on the worker's separate
//! branch).
//!
//! Errors:
//! - `run_id` is unknown → "worker run not found"
//! - The worker has no `worktree_path` set (already merged or
//!   previously discarded) → "worker already destroyed"
//!   (per PRD §"Edge Cases" fail-fast, MVP 不做幂等).
//!
//! ⑨ 关 routing: `Risk::High`, classified as `ToolKind::GitMutation`
//! (same Tier 4 tool-level grant + ask path as `merge_worker`); Plan
//! mode filters it out via `filter_tools_for_mode`. Worker subagents
//! cannot invoke it (`STRUCTURALLY_DISABLED`).
//!
//! Concurrency: unlike `merge_worker`, NO per-session mutex —
//! `do_discard` only calls `destroy_worker` (the worker branch +
//! worktree); it does NOT touch the parent session's git index, so two
//! concurrent discards of the same run_id are safe (the 2nd sees
//! `worktree_path` already NULL → "worker already destroyed"). The DB
//! row is the authoritative state.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::db;
use crate::git;
use crate::llm::types::ToolDef;
use crate::tools::{ToolContext, ToolContextUpdate};

/// `discard_worker` tool definition (registered in
/// `builtin_tools()`).
pub fn definition() -> ToolDef {
    ToolDef {
        name: "discard_worker".to_string(),
        description: Some(
            "Discard a completed worker subagent's preserved changes (the `worker/<run_id>` \
             branch + worktree) without merging. Use this when the worker's output isn't \
             wanted — the task was exploratory, the changes are wrong, or the user explicitly \
             rejected them. The parent session's branch is never affected.\n\n\
             On success, the worker worktree + branch are destroyed and the \
             `subagent_runs.worktree_path` column is cleared.\n\n\
             Errors:\n\
             - `run_id` is unknown → \"worker run not found\"\n\
             - The worker has no worktree to discard (already merged or previously discarded) \
             → \"worker already destroyed\". **Do not retry on this error.**"
                .to_string(),
        ),
        input_schema: json!({
            "type": "object",
            "properties": {
                "run_id": {
                    "type": "string",
                    "description": "The subagent run id (the `subagent_runs.id` UUID from the worker dispatch)."
                }
            },
            "required": ["run_id"]
        }),
    }
}

/// Execute the tool. Returns `(content, is_error, update,
/// exit_code)`. The function takes the `ToolContext` for
/// signature parity with other tools; `ctx.db` is the per-turn
/// SQLite pool used to read the `subagent_runs` row + parent
/// project row for the worktree destroy. `_session_id` is
/// unused (the run's own `parent_session_id` is the source of
/// truth for which project to look up).
pub async fn execute(
    input: &serde_json::Value,
    ctx: &ToolContext,
    _session_id: Option<&str>,
) -> (String, bool, ToolContextUpdate, Option<i32>) {
    let run_id = match input.get("run_id").and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => {
            return (
                "Missing required parameter: run_id".to_string(),
                true,
                ToolContextUpdate::default(),
                None,
            )
        }
    };

    match do_discard(&ctx.db, &run_id).await {
        Ok(msg) => (msg, false, ToolContextUpdate::default(), None),
        Err(msg) => (msg, true, ToolContextUpdate::default(), None),
    }
}

/// Synchronous discard body. Runs on the IPC command side
/// (which has the DB pool). The Tauri command resolves
/// the run_id → worktree_path / project_path / session_id
/// → calls `git::worktree::destroy_worker` → clears the
/// `worktree_path` column.
///
/// Best-effort ordering: destroy FIRST (so even if the DB
/// write fails, the on-disk artifact is gone — the row
/// becomes orphaned but the `destroy` did its job). Then
/// clear the column (so a subsequent `discard_worker` call
/// returns the "already destroyed" error and doesn't try to
/// re-destroy a non-existent worktree). The column clear
/// is best-effort: a failure logs at `warn!` and continues
/// (the next call's `worktree_path IS NULL` check still
/// catches the "already destroyed" case via the DB read).
///
/// **MVP is NOT idempotent**: a second call after a
/// successful first call returns
/// `worker already destroyed` (per PRD §"Edge Cases" — "幂等
/// follow-up"). The function is fail-fast: if the row has
/// no worktree, we surface the error to the LLM so it
/// doesn't silently no-op a retry.
pub async fn do_discard(
    pool: &sqlx::SqlitePool,
    run_id: &str,
) -> Result<String, String> {
    // ----- Load the run row -----
    let run_row = db::subagent_runs::get_run(pool, run_id)
        .await
        .map_err(|e| format!("discard_worker: failed to load subagent_runs row: {}", e))?
        .ok_or_else(|| format!("worker run not found: {}", run_id))?;

    // Fail-fast: already destroyed or never had a worktree.
    let worktree_path_str = run_row.worktree_path.as_deref().ok_or_else(|| {
        "worker already destroyed".to_string()
    })?;
    let worker_wt = PathBuf::from(worktree_path_str);

    // ----- Load the project row for the destroy_worker call -----
    let session_row = db::load_session(pool, &run_row.parent_session_id)
        .await
        .map_err(|e| format!("discard_worker: failed to load parent session: {}", e))?
        .ok_or_else(|| {
            format!(
                "discard_worker: parent session '{}' not found",
                run_row.parent_session_id
            )
        })?;
    let project = db::get_project(pool, &session_row.session.project_id)
        .await
        .map_err(|e| format!("discard_worker: failed to load project: {}", e))?
        .ok_or_else(|| {
            format!(
                "discard_worker: project '{}' not found",
                session_row.session.project_id
            )
        })?;
    let project_path = Path::new(&project.path);

    // ----- Destroy the worker worktree + branch -----
    if let Err(e) = git::worktree::destroy_worker(project_path, &worker_wt, run_id) {
        tracing::warn!(
            run_id = %run_id,
            worktree = %worker_wt.display(),
            error = %e,
            "discard_worker: destroy_worker failed (non-fatal; DB row still updated)"
        );
    }

    // ----- Clear the worktree_path column -----
    if let Err(e) = db::subagent_runs::set_worktree_path(pool, run_id, None).await {
        tracing::warn!(
            run_id = %run_id,
            error = %e,
            "discard_worker: set_worktree_path(NULL) failed (non-fatal)"
        );
    }

    Ok(format!(
        "discarded worker branch {} (worktree + branch destroyed)",
        git::worktree::worker_branch_name(run_id)
    ))
}
