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

/// Compute the platform-appropriate app data dir for our worktrees.
///
/// WSL/Linux first (the project's primary dev target per
/// `docs/HACKING-wsl.md`). Cross-platform will be added when we
/// ship to Windows / macOS — the right primitive there is
/// Tauri's `app.path().app_data_dir()` rather than `std::env::var`.
pub fn data_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("everlasting");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home).join(".local").join("share").join("everlasting");
        }
    }
    // Last-resort fallback. Should not happen on supported platforms.
    tracing::warn!(
        "neither XDG_DATA_HOME nor HOME is set; falling back to /tmp/everlasting"
    );
    PathBuf::from("/tmp/everlasting")
}

/// The on-disk directory where this session's worktree is checked
/// out. Layout: `<data_dir>/worktrees/<project_uuid>/<session_uuid>`.
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

    // 2. Refuse to clobber an existing worktree. Two scenarios
    //    where this fires: (a) UUID collision (effectively
    //    impossible), (b) a previous run died mid-create and left
    //    the dir behind. The user can clear it manually.
    if worktree_path.exists() {
        return Err(GitError::WorktreeExists {
            path: worktree_path.display().to_string(),
        });
    }

    // 3. Parent dir may not exist yet on a fresh install. We make
    //    it here so the libgit2 call below has a writable parent.
    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| GitError::Io {
            path: parent.display().to_string(),
            source: e,
        })?;
    }

    // 4. The actual worktree add.
    //
    //    Design note: libgit2's `Repository::worktree(name, path, opts)`
    //    takes `name` as BOTH the worktree metadata name (the
    //    directory under `<commondir>/worktrees/`) AND the new
    //    branch name when no `reference` is set. The CLI's
    //    `git worktree add -b <branch> <path>` does NOT have this
    //    coupling — it derives the metadata name from `<path>`'s
    //    basename and only uses `<branch>` for the branch side.
    //
    //    When the branch has slashes (e.g. `session/<uuid>`), the
    //    libgit2-coupled name tries to mkdir
    //    `<commondir>/worktrees/session/<uuid>/`. The first fix
    //    (commit 4930408) pre-created the `session/` intermediate
    //    dir, which made `git worktree list` treat `session/` as a
    //    stale worktree and `git worktree prune` would remove it,
    //    orphaning the real worktree metadata. The fix is to
    //    separate the two names: pass `name = session_id` (no
    //    slashes, the metadata dir under `.git/worktrees/`) and
    //    pass the new branch through `WorktreeAddOptions::reference`.
    //
    //    The branch is pre-created on the main repo via
    //    `Repository::branch` so the Reference object we hand to
    //    libgit2 is real. This means the branch shows up in the
    //    main repo's `git branch` listing too — that's fine; the
    //    branch is shared between the main repo and the worktree.
    let repo = Repository::open(project_path)?;
    let branch_full = branch_name(session_id);
    let head_commit = repo.head()?.peel_to_commit()?;
    let branch_obj = repo.branch(&branch_full, &head_commit, false)?;
    let branch_ref = branch_obj.into_reference();

    let mut opts = git2::WorktreeAddOptions::new();
    opts.reference(Some(&branch_ref));
    repo.worktree(session_id, &worktree_path, Some(&opts))?;

    tracing::info!(
        project = %project_path.display(),
        worktree = %worktree_path.display(),
        branch = %branch_full,
        "created session worktree"
    );
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
