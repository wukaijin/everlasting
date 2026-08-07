//! Worker worktree 创建与变更探测(拆分自 dispatch.rs,
//! 08-07-large-file-splitting)。

use std::path::PathBuf;

use sqlx::SqlitePool;

use super::resolve::resolve_project_main_path;

/// tool_result. Built by scanning the worker worktree's diff against
/// its base commit (the `worker/<run_id>` branch tip vs its parent).
/// When non-empty, the worker's branch + worktree are PRESERVED so a
/// future PR3 `merge_worker` / `discard_worker` tool can act on them;
/// when empty, the worktree is destroyed immediately.
pub(crate) struct WorkerChanges {
    /// True iff the worker's worktree has any tracked or untracked
    /// changes vs its base commit.
    pub(crate) has_changes: bool,
    /// A short, LLM-friendly summary of the changes (file list +
    /// per-file +/- counts). Empty when `has_changes` is false.
    pub(crate) summary: String,
}

/// Probe the worker worktree for changes vs its base commit. Used by
/// `run_subagent` after the worker exits to decide:
/// 1. **No changes** → destroy the worktree immediately (the branch
///    carries nothing useful); clear `subagent_runs.worktree_path`.
/// 2. **Has changes** → preserve the worktree + branch; the diff
///    summary is appended to the dispatch_subagent tool_result so
///    the parent LLM knows where the worker's edits live.
///
/// Implementation: delegates to `git::diff::diff_worktree`, which
/// already handles tracked + untracked files. We pass a synthetic
/// `session_id` of `<run_id>` so the diff is computed against the
/// `worker/<run_id>` branch (NOT the project's `session/<id>`
/// branch). On any error we conservatively report "has changes"
/// (preserving the worktree is the safe fallback — destroying it
/// could lose the worker's work).
pub(crate) fn probe_worker_changes(
    worker_worktree_path: &std::path::Path,
    run_id: &str,
) -> WorkerChanges {
    match crate::git::diff::diff_worker_worktree(worker_worktree_path, run_id) {
        Ok(result) => {
            if result.files.is_empty() {
                WorkerChanges {
                    has_changes: false,
                    summary: String::new(),
                }
            } else {
                // Build a compact summary: file list + per-file +/-
                // counts. Cap at 10 files to keep the tool_result
                // scannable (the full diff lives on the branch).
                let mut lines: Vec<String> = Vec::new();
                for f in result.files.iter().take(10) {
                    lines.push(format!(
                        "- {} ({}, +{}/-{})",
                        f.path, f.status, f.added, f.removed
                    ));
                }
                let omitted = result.files.len().saturating_sub(10);
                if omitted > 0 {
                    lines.push(format!("... and {} more", omitted));
                }
                WorkerChanges {
                    has_changes: true,
                    summary: lines.join("\n"),
                }
            }
        }
        Err(e) => {
            tracing::warn!(
                worker_worktree = %worker_worktree_path.display(),
                run_id = %run_id,
                error = %e,
                "probe_worker_changes: diff failed; preserving worktree as conservative fallback"
            );
            // Conservative fallback: assume changes exist so we
            // don't destroy a worktree that might hold the worker's
            // edits.
            WorkerChanges {
                has_changes: true,
                summary: "(diff probe failed; changes status unknown)".to_string(),
            }
        }
    }
}

/// L3b (2026-06-27): create the worker's isolated git worktree.
/// Returns the on-disk path on success.
///
/// Resolves:
/// 1. The project's main repo path (`.git/` lives here) — needed
///    for `git::worktree::create_worker`'s libgit2 open.
/// 2. The worker worktree path (`<app_data_dir>/worktrees/
///    <project_uuid>/worker/<run_id>`).
/// 3. The base worktree (the parent session's worktree) — the
///    worker's branch is based off this worktree's HEAD commit.
///
/// On ANY error we return `Err` — the caller (`run_subagent`) fails
/// the dispatch (no fallback to non-isolated, per the PRD's Edge
/// Cases). Errors include: project not found, project main path
/// not a git repo, worktree creation libgit2 failure.
pub(crate) async fn create_worker_worktree(
    db: &SqlitePool,
    parent_session_id: &str,
    project_id: &str,
    worker_run_id: &str,
    app_data_dir: &std::path::Path,
    parent_worktree_path: &std::path::Path,
) -> Result<PathBuf, String> {
    // 1. Resolve the project's main repo path.
    let project_main_path = resolve_project_main_path(db, parent_session_id).await;
    if project_main_path.is_empty() {
        return Err("could not resolve the project's main repo path for the session".to_string());
    }
    let project_main = std::path::Path::new(&project_main_path);
    if !project_main.join(".git").exists() {
        return Err(format!(
            "project main path '{}' is not a git repository (no .git found)",
            project_main.display()
        ));
    }

    // 2. Compute the worker worktree path.
    let worker_wt_path =
        crate::git::worktree::worker_worktree_path(app_data_dir, project_id, worker_run_id);

    // 3. Create the worktree. `create_worker` self-heals any stale
    //    state for this run_id (orphan dir / stale branch / stale
    //    metadata), then creates branch `worker/<run_id>` off the
    //    parent session's worktree HEAD + checks out the worktree.
    crate::git::worktree::create_worker(
        project_main,
        &worker_wt_path,
        parent_worktree_path,
        worker_run_id,
    )
    .map_err(|e| e.to_string())?;

    Ok(worker_wt_path)
}

// ---------------------------------------------------------------------------
// W1 (Workflow integration, Step 2.4 — 2026-07-08):
// pure workflow role-gate. Lives outside `run_subagent`
// so the gate logic is unit-testable without standing up
// the 25-arg signature.
//
// **Returns**: `Some(content)` when the dispatch must be
// denied (content is the tool_error body — the agent
// reads it and self-corrects on the next turn);
// `None` when the dispatch is allowed to proceed.
//
// **Side effect**: a single `tracing::warn!` per denial
// + per `force=true` bypass, so the audit log captures
// the overstep. No I/O, no LLM, no DB.
