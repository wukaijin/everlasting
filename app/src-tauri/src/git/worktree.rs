//! Git worktree lifecycle: create on session create, destroy on
//! session delete.
//!
//! Why worktrees (recap of `docs/ARCHITECTURE.md §3`):
//! - Different sessions can be active simultaneously (per-session
//!   independence is a first-class concern — see PR3 in
//!   `streamController`).
//! - worktree shares `.git/` but working dir is independent.
//! - Each session gets its own branch `session/<session_id>`; the
//!   user sees the diff between their worktree and the project's
//!   main branch.
//!
//! Why `git2-rs` (recap of
//! `.trellis/tasks/06-07-step-4-git-worktree-auto-commit/research/git-backend.md`):
//! - libgit2 covers `worktree add/list/find/lock/unlock/prune/validate`
//!   100% of what we need for step 4.
//! - libgit2 C API has no `worktree remove` (this is the real reason
//!   ARCH §3 warned about "worktree API not complete"). We work
//!   around it with `std::fs::remove_dir_all` + `Worktree::prune`.
//! - Branch delete is a separate libgit2 call (`Branch::delete`).

use std::path::{Path, PathBuf};

use git2::{BranchType, Repository};

use crate::git::error::GitError;

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

/// Create a worktree at `worktree_path` for the given session, on a
/// new branch `session/<session_id>` based on the project's current
/// HEAD.
///
/// `project_path` must point at a git working directory. The
/// function will:
///
/// 1. Verify the project is a git repo (`.git/` dir or `.git` file
///    for worktrees-of-worktrees).
/// 2. Verify the target worktree path does not yet exist.
/// 3. Create the parent directory of `worktree_path` (typically
///    `.../worktrees/<project_uuid>/`) if missing.
/// 4. Open the repo with libgit2 and call `Repository::worktree()`
///    which both creates the worktree directory and checks out the
///    new branch.
///
/// On success, the worktree is a fully checked-out working tree
/// the user (and the LLM's tools) can read/write.
pub fn create(
    project_path: &Path,
    worktree_path: &Path,
    session_id: &str,
) -> Result<(), GitError> {
    // 1. Repo sanity check. We accept both bare (.git/ directory)
    //    and linked-worktree (.git file pointing at parent's
    //    .git/worktrees/<name>/) layouts. The cheap probe avoids
    //    paying the libgit2 open cost on obviously-non-git inputs.
    if !project_path.join(".git").exists() {
        return Err(GitError::NotARepo {
            path: project_path.display().to_string(),
        });
    }

    let repo = Repository::open(project_path)?;
    let branch_full = branch_name(session_id);

    // 2. Self-heal stale state (3 roots: stale metadata / stale
    //    branch / orphan dir). Shared with the worker variant via
    //    `self_heal_for_create` — see that function for the full
    //    rationale.
    self_heal_for_create(&repo, project_path, worktree_path, session_id, &branch_full)?;

    // 3. Parent dir may not exist yet on a fresh install.
    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| GitError::Io {
            path: parent.display().to_string(),
            source: e,
        })?;
    }

    // 4. The actual worktree add. The branch is created off the
    //    project's current HEAD (the session variant's base). The
    //    worker variant (`create_worker`) instead bases the branch
    //    off an arbitrary commit (the parent session worktree's HEAD).
    let head_commit = repo.head()?.peel_to_commit()?;
    create_worktree_add(
        &repo,
        worktree_path,
        session_id,
        &branch_full,
        &head_commit,
    )?;

    tracing::info!(
        project = %project_path.display(),
        worktree = %worktree_path.display(),
        branch = %branch_full,
        "created session worktree"
    );
    Ok(())
}

/// Create a **worker** worktree at `worktree_path` for the given
/// worker run, on a new branch `worker/<run_id>` based on the
/// `base_worktree_path`'s current HEAD commit (L3b, 2026-06-27).
///
/// This is the worker-isolation counterpart to [`create`]. Differences
/// from the session variant:
///
/// - **Branch name**: `worker/<run_id>` (via [`worker_branch_name`]),
///   NOT `session/<id>`. Distinct namespace so concurrent workers
///   never collide and a future PR3 sweep can target `worker/*`
///   without filtering session branches.
/// - **Base commit**: the HEAD of `base_worktree_path` (typically the
///   parent session's worktree), NOT the project main repo's HEAD.
///   This makes the worker start from the parent's current commit
///   (parent progress is inherited at the commit level). Note: git
///   worktree base is commit-level — the parent's uncommitted WIP is
///   NOT visible to the worker (git worktree's inherent limitation).
/// - **Worktree metadata name**: the run_id (no `worker/` prefix),
///   mirroring the session variant's `session_id` (no `session/`
///   prefix). libgit2's worktree metadata dir lives under
///   `.git/worktrees/<name>/`; slashes there would create nested
///   dirs that confuse `git worktree prune`.
///
/// Reuses [`self_heal_for_create`] for the 3-state self-heal
/// (stale metadata / stale branch / orphan dir) — identical
/// recovery semantics to the session variant.
///
/// `project_path` is the project's main repo path (the `.git/`
/// directory is shared across all linked worktrees). The function
/// does NOT need `base_worktree_path` to be openable as a separate
/// repo — it only reads the base commit id from it.
pub fn create_worker(
    project_path: &Path,
    worktree_path: &Path,
    base_worktree_path: &Path,
    run_id: &str,
) -> Result<(), GitError> {
    if !project_path.join(".git").exists() {
        return Err(GitError::NotARepo {
            path: project_path.display().to_string(),
        });
    }

    let repo = Repository::open(project_path)?;
    let branch_full = worker_branch_name(run_id);

    // Self-heal any stale worker state for this run_id (the same 3
    // roots as the session variant; the metadata name is the run_id).
    self_heal_for_create(&repo, project_path, worktree_path, run_id, &branch_full)?;

    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| GitError::Io {
            path: parent.display().to_string(),
            source: e,
        })?;
    }

    // Resolve the base commit from the parent session's worktree
    // HEAD. `base_worktree_path` is a linked worktree of the same
    // repo, so opening it gives us a `Repository` whose `head()`
    // resolves to the parent session branch's tip commit.
    //
    // libgit2 invariant: a `Commit` object is owned by the
    // `Repository` it was peeled from. `repo.branch(name, &commit,
    // false)` requires `git_commit_owner(commit) == repository`, so
    // we CANNOT pass the parent-worktree's `Commit` directly to the
    // project-main repo's `branch()` call. Instead we read the OID
    // from the parent and re-look it up on the project-main repo
    // (the commit is shared across all linked worktrees of the same
    // repo, so the lookup always succeeds).
    let base_repo = Repository::open(base_worktree_path)?;
    let base_oid = base_repo.head()?.peel_to_commit()?.id();
    let base_commit = repo.find_commit(base_oid)?;

    create_worktree_add(&repo, worktree_path, run_id, &branch_full, &base_commit)?;

    // L3b (2026-06-27): lock the worktree for the duration of the
    // worker run. `git worktree lock` prevents external prune
    // operations (sweep, manual) from removing the worktree while
    // the worker is actively using it. The matching `unlock` is
    // in `destroy_worker` (before `prune`), so a successful destroy
    // also clears the lock. Failure here is non-fatal: the worktree
    // is fully usable without the lock; the worst case is a manual
    // `git worktree prune` sweeping it before the worker finishes.
    if let Ok(wt_lock) = repo.find_worktree(run_id) {
        if let Err(e) = wt_lock.lock(Some("L3b worker active")) {
            tracing::warn!(
                run_id = %run_id,
                error = %e,
                "worker worktree lock failed (non-fatal)"
            );
        }
    }

    tracing::info!(
        project = %project_path.display(),
        worktree = %worktree_path.display(),
        branch = %branch_full,
        base = %base_commit.id(),
        "created worker worktree"
    );
    Ok(())
}

/// Shared self-heal logic for [`create`] and [`create_worker`].
///
/// The 3 stale-state roots are documented in [`create`]'s doc
/// comment; this helper exists so the worker variant doesn't
/// copy-paste the recovery logic (code-reuse-thinking-guide:
/// asymmetric mechanisms producing the same cleanup must share a
/// single source of truth).
///
/// `metadata_name` is the worktree's libgit2 metadata name (the
/// session_id for session worktrees, the run_id for worker
/// worktrees — no prefix in either case). `branch_full` is the
/// full branch ref (`session/<id>` or `worker/<run_id>`).
fn self_heal_for_create(
    repo: &Repository,
    project_path: &Path,
    worktree_path: &Path,
    metadata_name: &str,
    branch_full: &str,
) -> Result<(), GitError> {
    // 2a. Stale worktree metadata.
    if let Ok(worktrees) = repo.worktrees() {
        if worktrees
            .iter()
            .any(|name| name.as_deref() == Some(metadata_name))
        {
            tracing::warn!(
                project = %project_path.display(),
                metadata = %metadata_name,
                "self-heal: found stale worktree metadata; pruning"
            );
            if let Ok(wt) = repo.find_worktree(metadata_name) {
                // Unlock before prune: libgit2 refuses to prune a
                // locked worktree without the `force` option. Stale
                // locks can outlive a crashed worker run (the lock
                // file on disk is picked up by the next self-heal
                // when the worktree itself has been removed but the
                // libgit2 metadata persists).
                if let Err(e) = wt.unlock() {
                    tracing::warn!(
                        metadata = %metadata_name,
                        error = %e,
                        "self-heal: worktree unlock failed (non-fatal)"
                    );
                }
                if let Err(e) = wt.prune(None) {
                    tracing::warn!(
                        metadata = %metadata_name,
                        error = %e,
                        "self-heal: worktree metadata prune failed (non-fatal)"
                    );
                }
            }
        }
    }

    // 2b. Stale branch.
    if let Ok(mut existing_branch) = repo.find_branch(branch_full, BranchType::Local) {
        tracing::warn!(
            project = %project_path.display(),
            branch = %branch_full,
            "self-heal: found stale branch; deleting"
        );
        if let Err(e) = existing_branch.delete() {
            tracing::warn!(
                branch = %branch_full,
                error = %e,
                "self-heal: branch delete failed (non-fatal)"
            );
        }
    }

    // 2c. Orphan worktree_path directory.
    if worktree_path.exists() {
        tracing::warn!(
            project = %project_path.display(),
            metadata = %metadata_name,
            worktree = %worktree_path.display(),
            "self-heal: found orphan worktree directory; removing"
        );
        std::fs::remove_dir_all(worktree_path).map_err(|e| GitError::Io {
            path: worktree_path.display().to_string(),
            source: e,
        })?;
    }

    Ok(())
}

/// Shared worktree-add step for [`create`] and [`create_worker`].
/// Pre-creates the branch off `base_commit` and then calls
/// `Repository::worktree(metadata_name, worktree_path, opts)` with
/// the branch ref as the `reference` (decouples the metadata name
/// from the branch name — see the design note in the original
/// `create` for why the slash in `session/<id>` / `worker/<run_id>`
/// forces this).
fn create_worktree_add(
    repo: &Repository,
    worktree_path: &Path,
    metadata_name: &str,
    branch_full: &str,
    base_commit: &git2::Commit,
) -> Result<(), GitError> {
    let branch_obj = repo.branch(branch_full, base_commit, false)?;
    let branch_ref = branch_obj.into_reference();

    let mut opts = git2::WorktreeAddOptions::new();
    opts.reference(Some(&branch_ref));
    repo.worktree(metadata_name, worktree_path, Some(&opts))?;
    Ok(())
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

/// Check that a git working directory (project root, worktree, or
/// any other tree) has **no uncommitted or untracked changes**.
/// Returns `Ok(())` for a clean tree, `Err(message)` for a dirty
/// one. The error message lists offending paths so the user knows
/// what to commit/stash.
///
/// Used by:
/// - `lib.rs::attach_worktree` — refuses to attach if the
///   project's main working directory has uncommitted changes
///   (the new worktree would diverge from a dirty base).
/// - `lib.rs::detach_worktree` — refuses to detach if the
///   worktree itself has uncommitted changes (detaching would
///   strand the user's WIP — the LLM's next tool call would
///   silently lose them).
///
/// Implementation: open the repo at `repo_path` and call
/// `repo.statuses(None)`. We classify any non-ignored entry with
/// a non-zero status bits (INDEX_NEW, WT_MODIFIED, etc.) as
/// "uncommitted". Ignored files (`include_ignored: false`) are
/// skipped — `.everlasting/outputs/` doesn't count.
pub fn check_clean(repo_path: &Path) -> Result<(), String> {
    if !repo_path.exists() {
        return Err(format!(
            "worktree path '{}' does not exist (it may have been deleted on disk)",
            repo_path.display()
        ));
    }
    let repo = Repository::open(repo_path).map_err(|e| {
        format!(
            "failed to open git repo at '{}': {}",
            repo_path.display(),
            e
        )
    })?;
    let mut opts = git2::StatusOptions::new();
    opts.include_ignored(false)
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_unmodified(false);
    let statuses = match repo.statuses(Some(&mut opts)) {
        Ok(s) => s,
        Err(e) => return Err(format!("failed to read git status: {}", e)),
    };
    if statuses.is_empty() {
        return Ok(());
    }
    // Collect up to 10 offending paths for a friendly error. The
    // libgit2 StatusEntry's `path()` is the worktree-relative
    // path (e.g. `src/main.rs`); good enough for the message.
    let mut paths: Vec<String> = Vec::new();
    for entry in statuses.iter() {
        if let Some(p) = entry.path() {
            paths.push(p.to_string());
            if paths.len() >= 10 {
                break;
            }
        }
    }
    Err(format!(
        "{} has uncommitted changes{}",
        repo_path.display(),
        if paths.is_empty() {
            String::new()
        } else {
            format!(": {}", paths.join(", "))
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command as StdCommand;
    use tempfile::tempdir;

    /// Helper: init a git repo at `path`, configure the user (so
    /// `commit` works in tests), and return the repo path. Tests
    /// using this can layer worktrees on top.
    fn init_repo(path: &Path) {
        fs::create_dir_all(path).unwrap();
        let init = StdCommand::new("git")
            .args(["init", "--initial-branch=main"])
            .current_dir(path)
            .output()
            .unwrap();
        assert!(init.status.success(), "git init failed: {:?}", init);
        let cfg_user = StdCommand::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(path)
            .output()
            .unwrap();
        assert!(cfg_user.status.success());
        let cfg_name = StdCommand::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(path)
            .output()
            .unwrap();
        assert!(cfg_name.status.success());
    }

    /// Helper: stage + commit everything in `path` with the
    /// message "init".
    fn commit_all(path: &Path) {
        let add = StdCommand::new("git")
            .args(["add", "-A"])
            .current_dir(path)
            .output()
            .unwrap();
        assert!(add.status.success());
        let commit = StdCommand::new("git")
            .args(["commit", "-m", "init", "--no-gpg-sign"])
            .current_dir(path)
            .output()
            .unwrap();
        assert!(commit.status.success(), "git commit failed: {:?}", commit);
    }

    #[test]
    fn check_clean_passes_on_clean_tree() {
        let tmp = tempdir().unwrap();
        let p = tmp.path();
        init_repo(p);
        std::fs::write(p.join("a.txt"), "hello").unwrap();
        commit_all(p);
        // No changes after the commit.
        check_clean(p).expect("clean tree should pass");
    }

    #[test]
    fn check_clean_detects_untracked_file() {
        let tmp = tempdir().unwrap();
        let p = tmp.path();
        init_repo(p);
        std::fs::write(p.join("a.txt"), "hello").unwrap();
        commit_all(p);
        // Add an untracked file.
        std::fs::write(p.join("b.txt"), "world").unwrap();
        let err = check_clean(p).expect_err("dirty tree should fail");
        assert!(err.contains("uncommitted"));
        assert!(err.contains("b.txt"));
    }

    #[test]
    fn check_clean_detects_modified_tracked_file() {
        let tmp = tempdir().unwrap();
        let p = tmp.path();
        init_repo(p);
        std::fs::write(p.join("a.txt"), "v1").unwrap();
        commit_all(p);
        // Modify the tracked file.
        std::fs::write(p.join("a.txt"), "v2").unwrap();
        let err = check_clean(p).expect_err("modified tree should fail");
        assert!(err.contains("uncommitted"));
        assert!(err.contains("a.txt"));
    }

    #[test]
    fn check_clean_ignores_gitignored_files() {
        let tmp = tempdir().unwrap();
        let p = tmp.path();
        init_repo(p);
        std::fs::write(p.join("a.txt"), "hello").unwrap();
        commit_all(p);
        // Add a .gitignore that ignores `output/`, then write into
        // that dir. The tool should NOT flag the ignored file.
        std::fs::write(p.join(".gitignore"), "output/\n").unwrap();
        std::fs::create_dir_all(p.join("output")).unwrap();
        std::fs::write(p.join("output/b.txt"), "ignored").unwrap();
        commit_all(p);
        // Now write a NEW ignored file post-commit. check_clean
        // should still pass because gitignored files are
        // excluded by `include_ignored(false)`.
        std::fs::write(p.join("output/c.txt"), "ignored").unwrap();
        check_clean(p).expect("ignored files should be excluded");
    }

    #[test]
    fn check_clean_rejects_missing_path() {
        let tmp = tempdir().unwrap();
        let bogus = tmp.path().join("does-not-exist");
        let err = check_clean(&bogus).expect_err("missing path should fail");
        assert!(err.contains("does not exist"));
    }

    // -----------------------------------------------------------------------
    // Self-heal: stale worktree / branch / orphan dir (Bug 2 fix)
    //
    // Real-world failure mode reported in the step 4 follow-up:
    // `attach_worktree` failed with libgit2 "worktree already exists"
    // even though our pre-check said the path was free. Three stale
    // states were the root cause; the create() function now self-heals
    // each of them BEFORE the worktree add. These tests pin the
    // behavior so a future refactor can't silently regress the
    // self-heal (in particular: silent re-introduction of the
    // "user must clear orphan dir manually" stance would re-open
    // the original bug).
    // -----------------------------------------------------------------------

    /// Helper: do a first successful `create` via libgit2 + return
    /// the worktree path. The subsequent test is responsible for
    /// tearing down pieces of state (or skipping the teardown) to
    /// simulate a crash mid-create. We use this for the metadata
    /// test and could use it for the branch test too, but the
    /// branch test setup is simpler when we pre-create the branch
    /// directly without a worktree.
    fn create_worktree_with_libgit2_first(project: &Path, session_id: &str) -> PathBuf {
        let wt = project.join(format!("first_wt_{}", session_id));
        create(project, &wt, session_id).expect("first create should succeed");
        wt
    }

    /// Helper: do a first create + commit so the project is a
    /// proper git repo with a HEAD for the worktree to point at.
    fn first_commit_setup(p: &Path) {
        init_repo(p);
        std::fs::write(p.join("a.txt"), "hello").unwrap();
        commit_all(p);
    }

    /// Stale worktree metadata: simulate the situation where
    /// `.git/worktrees/<session_id>/` still exists from a previous
    /// crashed create. We force this by:
    /// 1. Doing a successful `create()` (which writes metadata).
    /// 2. Manually removing the on-disk worktree directory + branch
    ///    but leaving the metadata dir untouched.
    /// 3. Calling `create()` again with the same session_id and a
    ///    fresh worktree path — the self-heal should prune the
    ///    stale metadata first so the second create succeeds.
    #[test]
    fn create_prunes_stale_metadata() {
        let tmp = tempdir().unwrap();
        let project = tmp.path();
        first_commit_setup(project);

        // First successful create.
        let session_id = "stale-meta-1";
        let wt1 = create_worktree_with_libgit2_first(project, session_id);

        // Simulate crash mid-cleanup: nuke the worktree dir and the
        // branch, but leave `.git/worktrees/<session_id>/` behind.
        std::fs::remove_dir_all(&wt1).unwrap();
        let repo = git2::Repository::open(project).unwrap();
        let mut b = repo
            .find_branch(&format!("session/{}", session_id), git2::BranchType::Local)
            .unwrap();
        let _ = b.delete();
        // Sanity: metadata dir is still there (we didn't touch it).
        let meta_dir = project.join(".git").join("worktrees").join(session_id);
        assert!(meta_dir.exists(), "stale metadata should be present");

        // Now create again at a different path with the same
        // session_id. The self-heal should prune the metadata so
        // this call succeeds.
        let wt2 = tmp.path().join("wt2");
        let result = create(project, &wt2, session_id);
        assert!(
            result.is_ok(),
            "second create should succeed after self-healing stale metadata, got: {:?}",
            result
        );
    }

    /// Stale `session/<id>` branch: simulate the situation where
    /// `.git/refs/heads/session/<id>` still exists from a previous
    /// crashed create, but the worktree metadata + dir are gone
    /// (the create function only got as far as the `Repository::
    /// branch` call). The next `create()` with the same
    /// session_id should delete the stale branch first, then
    /// re-create it, then succeed at the worktree add.
    #[test]
    fn create_deletes_stale_branch() {
        let tmp = tempdir().unwrap();
        let project = tmp.path();
        first_commit_setup(project);

        // Pre-stage: create a branch with the same name as the
        // session's worktree branch, but DO NOT create a worktree.
        // This mirrors the post-crash state where the branch is
        // present but the worktree isn't.
        let session_id = "stale-branch-1";
        let repo = git2::Repository::open(project).unwrap();
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        let _ = repo
            .branch(&format!("session/{}", session_id), &head, false)
            .unwrap();

        // Now `create` should self-heal by deleting the stale
        // branch and re-creating it.
        let wt = tmp.path().join("wt");
        let result = create(project, &wt, session_id);
        assert!(
            result.is_ok(),
            "create should self-heal stale session branch, got: {:?}",
            result
        );

        // Sanity: the worktree's HEAD points at the same commit.
        let wt_repo = git2::Repository::open(&wt).unwrap();
        let wt_head = wt_repo.head().unwrap().peel_to_commit().unwrap().id();
        assert_eq!(wt_head, head.id(), "worktree should point at HEAD");
    }

    /// Orphan worktree directory: the on-disk directory exists at
    /// the target `worktree_path` but is NOT a real worktree (no
    /// `.git` file, no libgit2 metadata for it). This is the
    /// third stale state from the step 4 follow-up: a partial
    /// create wrote the parent dir but never finished. The
    /// self-heal should `remove_dir_all` the orphan so the
    /// subsequent `Repository::worktree(...)` call has a clean
    /// slate. The user gets a `tracing::warn!` so the silent
    /// disk loss is visible in logs.
    #[test]
    fn create_cleans_orphan_dir() {
        let tmp = tempdir().unwrap();
        let project = tmp.path();
        first_commit_setup(project);

        // Lay down an orphan directory at the worktree path.
        // Contents can be anything (here: a stale file) — the
        // self-heal removes the whole tree.
        let wt = tmp.path().join("orphan");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join("stale.txt"), "leftover from previous run").unwrap();
        assert!(wt.exists(), "orphan dir should be in place");

        let session_id = "orphan-1";
        let result = create(project, &wt, session_id);
        assert!(
            result.is_ok(),
            "create should self-heal orphan dir, got: {:?}",
            result
        );

        // Sanity: the worktree is now a real, fully checked-out
        // tree. The `.git` *file* (not directory) is the canonical
        // marker of a linked worktree.
        assert!(wt.join(".git").exists(), "should be a real worktree now");
        let wt_repo = git2::Repository::open(&wt).expect("worktree should be a valid git repo");
        let wt_head = wt_repo
            .head()
            .expect("worktree should have a HEAD")
            .peel_to_commit()
            .expect("HEAD should peel to a commit");
        // The orphan dir contents are gone — `stale.txt` is no more.
        assert!(!wt.join("stale.txt").exists(), "orphan contents should be wiped");

        // And the worktree's HEAD points at the project's HEAD.
        let project_head = git2::Repository::open(project)
            .unwrap()
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id();
        assert_eq!(wt_head.id(), project_head, "worktree HEAD should match project HEAD");
    }

    // -----------------------------------------------------------------------
    // L3b (2026-06-27): worker worktree variants
    //
    // `create_worker` / `destroy_worker` are the worker-isolation
    // counterparts to `create` / `destroy`. The branch is
    // `worker/<run_id>` (distinct namespace), and the base commit is
    // the parent session worktree's HEAD (not the project main HEAD).
    // These tests pin the worker-specific invariants:
    //   1. Branch name is `worker/<run_id>` (not `session/<id>`).
    //   2. Base commit is the parent worktree HEAD (an extra commit
    //      on the parent session branch is visible to the worker).
    //   3. destroy_worker removes the branch + dir.
    //   4. Self-heal works for the worker namespace (orphan dir).
    // -----------------------------------------------------------------------

    /// Helper: build a parent session worktree (one commit ahead of
    /// project main) so the worker's base-commit inheritance is
    /// observable. Returns `(project_path, parent_worktree_path,
    /// parent_head_commit_id)`.
    fn parent_session_worktree_with_extra_commit(project: &Path, session_id: &str) -> PathBuf {
        // Bring the project to a clean HEAD with one commit.
        first_commit_setup(project);

        // Create the parent session worktree (uses `create`, the
        // session variant — the worker variant will base off this).
        let parent_wt = project.join(format!("parent_wt_{}", session_id));
        create(project, &parent_wt, session_id)
            .expect("parent session worktree create should succeed");

        // Make an extra commit on the parent session branch so the
        // worker's base-commit inheritance is observable (the worker
        // should see this commit, the project main should not).
        std::fs::write(parent_wt.join("parent_only.txt"), "from parent session")
            .unwrap();
        let add = StdCommand::new("git")
            .args(["add", "-A"])
            .current_dir(&parent_wt)
            .output()
            .unwrap();
        assert!(add.status.success());
        let commit = StdCommand::new("git")
            .args(["commit", "-m", "parent session commit", "--no-gpg-sign"])
            .current_dir(&parent_wt)
            .output()
            .unwrap();
        assert!(commit.status.success(), "parent commit failed: {:?}", commit);

        parent_wt
    }

    #[test]
    fn create_worker_uses_worker_branch_prefix() {
        let tmp = tempdir().unwrap();
        let project = tmp.path();
        let parent_wt = parent_session_worktree_with_extra_commit(project, "sess-1");

        let run_id = "run-abc";
        let worker_wt = tmp.path().join("worker_wt");
        create_worker(project, &worker_wt, &parent_wt, run_id)
            .expect("create_worker should succeed");

        // The worker branch exists as `worker/<run_id>` in the repo.
        let repo = git2::Repository::open(project).unwrap();
        let branch = repo
            .find_branch(&format!("worker/{}", run_id), git2::BranchType::Local)
            .expect("worker branch should exist");
        assert!(branch.get().is_branch(), "worker branch is a branch ref");

        // No `session/<run_id>` branch was created (distinct namespace).
        assert!(
            repo.find_branch(&format!("session/{}", run_id), git2::BranchType::Local)
                .is_err(),
            "no session/<run_id> branch should exist for a worker"
        );

        // The worktree is a real linked worktree.
        assert!(worker_wt.join(".git").exists());
    }

    #[test]
    fn create_worker_bases_off_parent_worktree_head() {
        let tmp = tempdir().unwrap();
        let project = tmp.path();
        let parent_wt = parent_session_worktree_with_extra_commit(project, "sess-2");

        // The parent session made one commit ahead of project main.
        let parent_repo = git2::Repository::open(&parent_wt).unwrap();
        let parent_head = parent_repo.head().unwrap().peel_to_commit().unwrap().id();
        let project_repo = git2::Repository::open(project).unwrap();
        let project_head = project_repo.head().unwrap().peel_to_commit().unwrap().id();
        assert_ne!(parent_head, project_head, "parent must be ahead of project main");

        let run_id = "run-bases-off-parent";
        let worker_wt = tmp.path().join("worker_wt_off_parent");
        create_worker(project, &worker_wt, &parent_wt, run_id)
            .expect("create_worker should succeed");

        // The worker's HEAD must equal the parent's HEAD, NOT the
        // project main HEAD. This is the load-bearing L3b invariant:
        // the worker inherits the parent session's progress at the
        // commit level.
        let worker_repo = git2::Repository::open(&worker_wt).unwrap();
        let worker_head = worker_repo.head().unwrap().peel_to_commit().unwrap().id();
        assert_eq!(
            worker_head, parent_head,
            "worker HEAD must match parent session HEAD (base-commit inheritance)"
        );
        assert_ne!(
            worker_head, project_head,
            "worker HEAD must NOT match project main HEAD"
        );

        // The worker can see the parent's extra file (it was part of
        // the parent's commit, so the worktree checkout has it).
        assert!(
            worker_wt.join("parent_only.txt").exists(),
            "worker should see the parent's committed file"
        );
    }

    #[test]
    fn destroy_worker_removes_branch_and_dir() {
        let tmp = tempdir().unwrap();
        let project = tmp.path();
        let parent_wt = parent_session_worktree_with_extra_commit(project, "sess-3");

        let run_id = "run-destroy";
        let worker_wt = tmp.path().join("worker_wt_destroy");
        create_worker(project, &worker_wt, &parent_wt, run_id)
            .expect("create_worker should succeed");
        assert!(worker_wt.exists());

        destroy_worker(project, &worker_wt, run_id)
            .expect("destroy_worker should succeed");

        // Directory is gone.
        assert!(!worker_wt.exists(), "worker worktree dir should be removed");

        // Branch is gone.
        let repo = git2::Repository::open(project).unwrap();
        assert!(
            repo.find_branch(&format!("worker/{}", run_id), git2::BranchType::Local)
                .is_err(),
            "worker branch should be deleted"
        );
    }

    #[test]
    fn create_worker_self_heals_orphan_dir() {
        let tmp = tempdir().unwrap();
        let project = tmp.path();
        let parent_wt = parent_session_worktree_with_extra_commit(project, "sess-4");

        let run_id = "run-orphan";
        let worker_wt = tmp.path().join("worker_wt_orphan");
        // Lay down an orphan dir at the worker worktree path.
        std::fs::create_dir_all(&worker_wt).unwrap();
        std::fs::write(worker_wt.join("stale.txt"), "leftover").unwrap();

        create_worker(project, &worker_wt, &parent_wt, run_id)
            .expect("create_worker should self-heal orphan dir");

        // Orphan contents are gone; the worktree is real now.
        assert!(!worker_wt.join("stale.txt").exists());
        assert!(worker_wt.join(".git").exists());
    }

    #[test]
    fn worker_branch_name_and_path_helpers() {
        assert_eq!(worker_branch_name("run-123"), "worker/run-123");
        assert_eq!(WORKER_BRANCH_PREFIX, "worker/");
        let data_dir = Path::new("/data");
        let p = worker_worktree_path(data_dir, "proj-uuid", "run-123");
        assert_eq!(
            p,
            Path::new("/data/worktrees/proj-uuid/worker/run-123")
        );
    }
}
