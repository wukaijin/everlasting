//! Worktree lifecycle: attach session + destroy session/worker.
//!
//! Relocated verbatim from the pre-split `worktree.rs`.

use std::path::{Path, PathBuf};

use git2::{BranchType, Repository};

use super::check::check_clean;
use super::create::create;
use super::naming::{branch_name, worker_branch_name, worktree_path};
use crate::git::error::GitError;

/// Attach a session's `session/<id>` worktree and write the
/// resulting state to the DB (06-30 follow-up, `merge_worker`
/// lazy-attach).
///
/// This is the inner work of `commands::worktree::attach_worktree`
/// extracted as a free function so tool-layer call sites
/// (`tools::merge_worker::ensure_parent_worktree_attached`) can
/// invoke it without dragging a Tauri `State`. It preserves every
/// invariant of the original IPC body:
///
/// - **State machine guard is the caller's responsibility.** This
///   helper unconditionally creates the worktree + writes Active
///   state. The IPC `attach_worktree` rejects `Active` (already
///   attached) and accepts `None` / `Detached`. The merge_worker
///   helper has its own tri-state policy. Putting the policy in
///   the helper would force one caller's contract onto the other,
///   so each caller enforces its own guard before calling here.
/// - **Dirty-project-root check** IS enforced here. A new
///   worktree branching from a dirty base would silently lose the
///   user's WIP, which is an unacceptable regression. We refuse
///   with `GitError::Dirty` (carrying up to 10 offending paths).
/// - **Disk first, then DB.** If the libgit2 worktree add fails,
///   we do not touch the DB; the user can retry with the same
///   `session_id` and the row state stays at whatever it was
///   (typically `None`, occasionally `Detached`).
/// - **System event injection** is best-effort (`tracing::warn!`
///   on failure): the worktree is already on disk + the DB row
///   is updated, so a missing event only delays the LLM's
///   awareness by one turn (the next turn will reload + run
///   `build_system_prompt` which embeds the current state).
///
/// Errors propagate as `GitError`. The `Dirty` variant carries a
/// pre-formatted, user-friendly message; `NotARepo` for non-git
/// projects; `Git2` for libgit2 failures (verbatim); `Io` for
/// filesystem errors. The IPC layer prefixes the helper error
/// with `"attach_worktree: "` for parity with the prior string
/// contract; tool-layer callers add their own prefix.
pub async fn attach_session(
    db: &sqlx::SqlitePool,
    project: &crate::projects::ProjectRow,
    session_id: &str,
    data_dir: &Path,
) -> Result<PathBuf, GitError> {
    let project_path = Path::new(&project.path);

    // Reject non-git projects up front. The IPC layer does this
    // too, but the helper needs to be self-sufficient (called from
    // tool-layer paths that have no State-derived guards).
    if !project.is_git_repo {
        return Err(GitError::NotARepo {
            path: project.path.clone(),
        });
    }

    // Dirty-project-root check. The new worktree would diverge
    // from the user's WIP if we branched off HEAD with uncommitted
    // changes in the project root. We have to convert from
    // `Result<(), String>` (check_clean's contract) to a typed
    // GitError::Dirty — the String carries the paths, so parse
    // out the trailing `": ...path..."` suffix when present.
    if project_path.exists() {
        match check_clean(project_path) {
            Ok(()) => {}
            Err(msg) => {
                // Surface up to 10 paths for an actionable message.
                // check_clean's string format is `"<path> has
                // uncommitted changes: <path1>, <path2>, ..."`.
                let paths: Vec<String> = msg
                    .rsplit_once(": ")
                    .map(|(_, rest)| rest.split(", ").map(String::from).take(10).collect())
                    .unwrap_or_default();
                return Err(GitError::Dirty {
                    path: project_path.display().to_string(),
                    paths,
                });
            }
        }
    }

    // Compute the worktree path (same layout as the IPC layer)
    // and run the libgit2 work. `create` already does the
    // 3-state self-heal (stale metadata / stale branch / orphan
    // dir) before the worktree add, so prior-crash recovery is
    // transparent.
    let wt_path = worktree_path(data_dir, &project.id, session_id);
    create(project_path, &wt_path, session_id)?;

    // Persist the new state. We update `last_worktree_path` only
    // when transitioning from Detached (preserving the previous
    // pointer); None → Active leaves it None (the prior value is
    // NULL anyway). The `as_deref()` is safe because project.id is
    // never None at the row level.
    let prev = crate::db::sessions::load_session(db, session_id)
        .await
        .map_err(|e| GitError::Io {
            path: project_path.display().to_string(),
            source: std::io::Error::other(e.to_string()),
        })?
        .ok_or_else(|| GitError::Io {
            path: project_path.display().to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("session '{}' not found", session_id),
            ),
        })?;
    let last_wt = prev
        .session
        .last_worktree_path
        .as_deref()
        .or(prev.session.worktree_path.as_deref());
    let wt_str = wt_path.to_str();
    crate::db::sessions::set_worktree_state(
        db,
        session_id,
        crate::db::WorktreeState::Active,
        wt_str,
        last_wt,
    )
    .await
    .map_err(|e| GitError::Io {
        path: project_path.display().to_string(),
        source: std::io::Error::other(e.to_string()),
    })?;

    // Inject the system event. Best-effort (tracing::warn! on
    // failure): the worktree is already on disk + the row is
    // updated, so a missing event only delays the LLM's awareness
    // by one turn (next-turn reload + system prompt rebuild fills
    // the gap).
    let branch = branch_name(session_id);
    let event_text = format!(
        "worktree attached: {} on branch {}",
        wt_path.display(),
        branch
    );
    if let Err(e) =
        crate::db::sessions::insert_system_event(db, session_id, &event_text, "attached").await
    {
        tracing::warn!(
            error = %e,
            session_id = %session_id,
            "attach_session: insert_system_event failed (non-fatal)"
        );
    }

    tracing::info!(
        session_id = %session_id,
        project = %project_path.display(),
        worktree = %wt_path.display(),
        branch = %branch,
        "attached session worktree"
    );

    Ok(wt_path)
}

/// Destroy the worktree at `worktree_path` and delete the session
/// branch. Best-effort: errors in the directory removal are
/// surfaced; the metadata prune and branch delete are
/// best-effort (a previous crash may have left the worktree
/// already removed from `.git/worktrees/`).
///
/// libgit2's C API has no `git_worktree_remove`, so we work around
/// it in two steps:
///
/// 1. `std::fs::remove_dir_all(worktree_path)` — physical cleanup.
/// 2. `Worktree::prune` (best-effort) + `Branch::delete` — metadata
///    cleanup. Both fail gracefully if the metadata is already
///    gone (which happens on a crash-during-create).
pub fn destroy(
    project_path: &Path,
    worktree_path: &Path,
    session_id: &str,
) -> Result<(), GitError> {
    let branch = branch_name(session_id);

    // 1. Physical cleanup. The caller is responsible for the safety
    //    check that `worktree_path` is under our data dir (see
    //    lib.rs::delete_session — it computes the path from the
    //    session id, not from user input). We still do a
    //    defensive check: refuse to remove "/" or empty paths.
    if worktree_path.as_os_str().is_empty() || worktree_path == Path::new("/") {
        return Err(GitError::Io {
            path: worktree_path.display().to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "refusing to remove system-critical path",
            ),
        });
    }

    if worktree_path.exists() {
        std::fs::remove_dir_all(worktree_path).map_err(|e| GitError::Io {
            path: worktree_path.display().to_string(),
            source: e,
        })?;
    }

    // 2. Metadata cleanup. We tolerate "not found" because a
    //    previous crash may have already removed the .git/worktrees
    //    entry but left the working dir (which we just cleaned up
    //    in step 1). Both prune and branch-delete are best-effort.
    //
    //    NB: since PR1's fix, the worktree's metadata name is the
    //    session_id (no `session/` prefix); the branch name keeps
    //    the prefix. We need to look up by session_id for the
    //    worktree and by `session/<id>` for the branch.
    let worktree_lookup = session_id;
    match Repository::open(project_path) {
        Ok(repo) => {
            if let Ok(wt) = repo.find_worktree(worktree_lookup) {
                if let Err(e) = wt.prune(None) {
                    tracing::warn!(
                        worktree = %worktree_lookup,
                        error = %e,
                        "worktree metadata prune failed (non-fatal)"
                    );
                }
            }
            match repo.find_branch(&branch, BranchType::Local) {
                Ok(mut b) => {
                    if let Err(e) = b.delete() {
                        tracing::warn!(
                            branch = %branch,
                            error = %e,
                            "session branch delete failed (non-fatal)"
                        );
                    }
                }
                Err(e) if e.code() == git2::ErrorCode::NotFound => {
                    // Branch was never created or already deleted — fine.
                }
                Err(e) => {
                    tracing::warn!(
                        branch = %branch,
                        error = %e,
                        "session branch lookup failed (non-fatal)"
                    );
                }
            }
        }
        Err(e) => {
            // The project path may have been deleted out from under
            // us (e.g. the user rm -rf'd the project). The worktree
            // cleanup is still done in step 1; we just can't reach
            // the .git metadata. Log and move on.
            tracing::warn!(
                project = %project_path.display(),
                error = %e,
                "could not open project repo to prune worktree metadata (non-fatal)"
            );
        }
    }

    tracing::info!(
        project = %project_path.display(),
        worktree = %worktree_path.display(),
        branch = %branch,
        "destroyed session worktree"
    );
    Ok(())
}

/// Destroy a **worker** worktree (L3b, 2026-06-27) and delete its
/// `worker/<run_id>` branch. Best-effort like [`destroy`]: physical
/// dir removal is surfaced; metadata prune + branch delete are
/// best-effort.
///
/// Differences from the session variant:
/// - **Branch name**: `worker/<run_id>` (via [`worker_branch_name`]),
///   NOT `session/<id>`.
/// - **Metadata lookup name**: the run_id (no prefix), mirroring
///   the session variant's session_id.
///
/// Used in two paths:
/// 1. Worker exits with no changes → destroy immediately (the
///    branch carries nothing useful).
/// 2. A future PR3 `discard_worker` tool explicitly drops a
///    kept-changes worker branch.
pub fn destroy_worker(
    project_path: &Path,
    worktree_path: &Path,
    run_id: &str,
) -> Result<(), GitError> {
    let branch = worker_branch_name(run_id);

    if worktree_path.as_os_str().is_empty() || worktree_path == Path::new("/") {
        return Err(GitError::Io {
            path: worktree_path.display().to_string(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "refusing to remove system-critical path",
            ),
        });
    }

    if worktree_path.exists() {
        std::fs::remove_dir_all(worktree_path).map_err(|e| GitError::Io {
            path: worktree_path.display().to_string(),
            source: e,
        })?;
    }

    let worktree_lookup = run_id;
    match Repository::open(project_path) {
        Ok(repo) => {
            if let Ok(wt) = repo.find_worktree(worktree_lookup) {
                // Unlock before prune: the worktree is locked by
                // `create_worker` for the worker's lifetime, and
                // libgit2 refuses to prune a locked worktree without
                // the `force` option.
                if let Err(e) = wt.unlock() {
                    tracing::warn!(
                        worktree = %worktree_lookup,
                        error = %e,
                        "worker worktree unlock failed (non-fatal)"
                    );
                }
                if let Err(e) = wt.prune(None) {
                    tracing::warn!(
                        worktree = %worktree_lookup,
                        error = %e,
                        "worker worktree metadata prune failed (non-fatal)"
                    );
                }
            }
            match repo.find_branch(&branch, BranchType::Local) {
                Ok(mut b) => {
                    if let Err(e) = b.delete() {
                        tracing::warn!(
                            branch = %branch,
                            error = %e,
                            "worker branch delete failed (non-fatal)"
                        );
                    }
                }
                Err(e) if e.code() == git2::ErrorCode::NotFound => {
                    // Branch was never created or already deleted — fine.
                }
                Err(e) => {
                    tracing::warn!(
                        branch = %branch,
                        error = %e,
                        "worker branch lookup failed (non-fatal)"
                    );
                }
            }
        }
        Err(e) => {
            tracing::warn!(
                project = %project_path.display(),
                error = %e,
                "could not open project repo to prune worker worktree metadata (non-fatal)"
            );
        }
    }

    tracing::info!(
        project = %project_path.display(),
        worktree = %worktree_path.display(),
        branch = %branch,
        "destroyed worker worktree"
    );
    Ok(())
}
