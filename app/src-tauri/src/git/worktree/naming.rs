//! Worktree path + branch naming helpers.
//!
//! Pure path/branch arithmetic — zero I/O, zero git2 deps beyond the
//! const strings. Relocated verbatim from the pre-split `worktree.rs`.

use std::path::{Path, PathBuf};

/// Branch prefix for all session worktrees. Combined with the
/// session id (UUID v4) the full branch name is `session/<uuid>`.
/// The slash creates a "namespace" so `git branch` listings show
/// `session/xxx` as a flat group.
pub const SESSION_BRANCH_PREFIX: &str = "session/";

/// Branch prefix for all **worker** worktrees (L3b, 2026-06-27).
/// Combined with the worker run id (the `subagent_runs.id` UUID)
/// the full branch name is `worker/<run_id>`. Distinct from
/// [`SESSION_BRANCH_PREFIX`] so concurrent workers never collide
/// on a branch name (each worker run id is unique per dispatch).
pub const WORKER_BRANCH_PREFIX: &str = "worker/";

/// The on-disk directory where this session's worktree is checked
/// out. Layout: `<app_data_dir>/worktrees/<project_uuid>/<session_uuid>`.
///
/// Note: we use the project UUID (not path slug) because project
/// paths can change via `update_project_path`; the UUID is the
/// stable identifier that survives renames.
pub fn worktree_path(data_dir: &Path, project_id: &str, session_id: &str) -> PathBuf {
    data_dir.join("worktrees").join(project_id).join(session_id)
}

/// The branch name to use for this session's worktree. We use
/// `session/<session_id>` (slash-separated) so `git branch` lists
/// show all session branches as a flat group.
pub fn branch_name(session_id: &str) -> String {
    format!("{}{}", SESSION_BRANCH_PREFIX, session_id)
}

/// The branch name for a **worker** worktree (L3b, 2026-06-27):
/// `worker/<run_id>`. Distinct from `branch_name` (which produces
/// `session/<id>`) so worker branches never collide with session
/// branches or with each other (the run id is unique per dispatch).
pub fn worker_branch_name(run_id: &str) -> String {
    format!("{}{}", WORKER_BRANCH_PREFIX, run_id)
}

/// The on-disk directory for a **worker** worktree (L3b, 2026-06-27).
/// Layout: `<app_data_dir>/worktrees/<project_uuid>/worker/<run_id>`.
/// Sibling to the session worktrees dir but under a `worker/`
/// sub-namespace so a single `ls worktrees/<project_uuid>/` cleanly
/// separates session-owned trees from worker-owned trees (and a
/// future PR3 sweep over `worker/` does not need to filter out
/// session trees).
pub fn worker_worktree_path(data_dir: &Path, project_id: &str, run_id: &str) -> PathBuf {
    data_dir
        .join("worktrees")
        .join(project_id)
        .join("worker")
        .join(run_id)
}
