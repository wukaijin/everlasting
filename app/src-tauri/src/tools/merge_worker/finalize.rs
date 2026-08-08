//! Post-merge cleanup + parent-worktree lazy-attach helpers.
//!
//! Split out of `tools/merge_worker.rs` (2026-08-08 batch3).

use std::path::{Path, PathBuf};

use crate::db;
use crate::git;

/// Lazy auto-attach helper for the merge entry points (06-30
/// follow-up). Called from BOTH the `merge_worker` tool's `execute`
/// AND the `merge_worker_run` IPC command before they invoke
/// `do_merge_blocking`. Same policy on both paths so behavior is
/// deterministic regardless of whether the merge was triggered by
/// the user clicking the drawer's Merge button or by the LLM
/// deciding to merge.
///
/// Returns:
/// - `Ok(false)` — parent is `Active` (already has a worktree;
///   nothing to do) **OR** parent is `Detached` (the user explicitly
///   tore down their worktree; we MUST NOT silently re-attach —
///   forcing re-attachment would override user intent and could
///   pull the session into a different branch state than they
///   expect). The merge will then fail at `do_merge_blocking` with
///   a clean error that the UI surfaces back.
/// - `Ok(true)` — parent was `None`, and we lazily created a fresh
///   worktree via [`crate::git::worktree::attach_session`]. The
///   caller's NEXT step must reload the parent's `worktree_path`
///   from the DB; the value cached on `ctx.worktree_path` (or
///   captured before the call) is now stale because attach creates
///   a brand-new tree under `<data_dir>/worktrees/<pid>/<sid>`.
/// - `Err(reason)` — the lazy attach was attempted but failed.
///   Common reasons are "project not a git repository" (the
///   project isn't a git repo at all) or "dirty project root"
///   (uncommitted changes in the project dir would be silently
///   bypassed by branching from HEAD). The returned `String` is
///   the upstream `attach_session` error verbatim — the IPC layer
///   prefixes it with `"merge_worker_run: cannot auto-attach
///   parent worktree: "` for user-facing display, and the tool
///   layer wraps it in a tuple `(..., true, ...)` as the tool
///   result content.
pub async fn ensure_parent_worktree_attached(
    db: &sqlx::SqlitePool,
    data_dir: &Path,
    parent_session_id: &str,
) -> Result<bool, String> {
    let loaded = db::sessions::load_session(db, parent_session_id)
        .await
        .map_err(|e| format!("failed to load parent session: {}", e))?
        .ok_or_else(|| format!("parent session '{}' not found", parent_session_id))?;
    match loaded.session.worktree_state {
        db::WorktreeState::Active | db::WorktreeState::Detached => Ok(false),
        db::WorktreeState::None => {
            let project = db::projects::get_project(db, &loaded.session.project_id)
                .await
                .map_err(|e| format!("failed to load parent project: {}", e))?
                .ok_or_else(|| {
                    format!("parent project '{}' not found", loaded.session.project_id)
                })?;
            crate::git::worktree::attach_session(db, &project, parent_session_id, data_dir)
                .await
                .map_err(|e| e.to_string())?;
            tracing::info!(
                parent_session_id = %parent_session_id,
                branch = %crate::git::worktree::branch_name(parent_session_id),
                "merge_worker: auto-attached parent worktree for merge"
            );
            Ok(true)
        }
    }
}

/// Post-merge cleanup + DB row update. Called by `execute`
/// after a successful `do_merge`. The function:
/// 1. Loads the `subagent_runs` row to find the worker
///    worktree path (the path on disk for `destroy_worker`).
/// 2. Loads the project row for the project path
///    (`destroy_worker` needs the project root, not the
///    parent worktree, because libgit2 looks up the
///    worktree metadata by name from the main repo).
/// 3. Calls [`git::worktree::destroy_worker`].
/// 4. Clears the `subagent_runs.worktree_path` column.
///
/// Best-effort: if the destroy fails (e.g. branch already
/// gone from a manual `git branch -D`), the worktree_path
/// column is still cleared so the row doesn't display a
/// stale path. A `tracing::warn!` carries the failure
/// context.
pub async fn finalize_merge(
    pool: &sqlx::SqlitePool,
    parent_session_id: &str,
    run_id: &str,
) -> Result<(), String> {
    let run_row = db::subagent_runs::get_run(pool, run_id)
        .await
        .map_err(|e| format!("merge_worker: failed to load subagent_runs row: {}", e))?
        .ok_or_else(|| format!("worker run not found: {}", run_id))?;
    let worktree_path_str = run_row.worktree_path.as_deref().ok_or_else(|| {
        "worker has no worktree to merge (already merged or discarded)".to_string()
    })?;
    let worker_wt = PathBuf::from(worktree_path_str);

    // ----- Load the project row for the destroy_worker call -----
    let session_row = db::load_session(pool, parent_session_id)
        .await
        .map_err(|e| format!("merge_worker: failed to load session: {}", e))?
        .ok_or_else(|| format!("parent session not found: {}", parent_session_id))?;
    let project = db::get_project(pool, &session_row.session.project_id)
        .await
        .map_err(|e| format!("merge_worker: failed to load project: {}", e))?
        .ok_or_else(|| {
            format!(
                "merge_worker: project '{}' not found",
                session_row.session.project_id
            )
        })?;
    let project_path = std::path::Path::new(&project.path);

    // ----- Destroy worker worktree + branch (best-effort) -----
    if let Err(e) = git::worktree::destroy_worker(project_path, &worker_wt, run_id) {
        tracing::warn!(
            run_id = %run_id,
            worktree = %worker_wt.display(),
            error = %e,
            "merge_worker: destroy_worker failed (non-fatal; DB row still updated)"
        );
    }

    // ----- Clear the worktree_path column -----
    if let Err(e) = db::subagent_runs::set_worktree_path(pool, run_id, None).await {
        tracing::warn!(
            run_id = %run_id,
            error = %e,
            "merge_worker: set_worktree_path(NULL) failed (non-fatal)"
        );
    }

    Ok(())
}
