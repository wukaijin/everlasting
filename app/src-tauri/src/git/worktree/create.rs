//! Worktree creation: session + worker variants + shared self-heal.
//!
//! Relocated verbatim from the pre-split `worktree.rs`.

use std::path::Path;

use git2::{BranchType, Repository};

use super::naming::{branch_name, worker_branch_name};
use crate::git::error::GitError;

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
pub fn create(project_path: &Path, worktree_path: &Path, session_id: &str) -> Result<(), GitError> {
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
    create_worktree_add(&repo, worktree_path, session_id, &branch_full, &head_commit)?;

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
        if worktrees.iter().any(|name| name == Some(metadata_name)) {
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
