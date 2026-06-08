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

    // 2. Self-heal stale state from previous failed / crashed runs.
    //
    //    Motivation: a real-world failure mode reported in the step 4
    //    follow-up (see
    //    `.trellis/tasks/06-08-step-4-followup-bugfix-attach-diff-systemprompt/prd.md`
    //    §Bug 2) is that `attach_worktree` rejected by libgit2 with
    //    "worktree already exists at <path>" even though our pre-check
    //    said the path was free. The three roots are:
    //
    //    a) Stale `.git/worktrees/<session_id>/` metadata left behind
    //       by a previous `create` that crashed between the libgit2
    //       write and the directory fsync. libgit2's `Repository::
    //       worktree(name, ...)` refuses to create a new worktree
    //       when its metadata name collides with an existing entry.
    //    b) Stale `session/<id>` branch from a previous create that
    //       crashed after `Repository::branch(...)` returned but
    //       before `Repository::worktree(...)` finished. The branch
    //       exists in `.git/refs/heads/session/<id>` but no worktree
    //       points at it; subsequent creates fail because
    //       `Repository::branch(..., force=false)` refuses to
    //       overwrite.
    //    c) Orphan `worktree_path` directory that is NOT tracked by
    //       libgit2 (e.g. a partial create wrote the parent dir
    //       but never finished). After (a) prunes the metadata and
    //       (b) deletes the branch, any directory still standing
    //       at `worktree_path` is by definition an orphan — a real
    //       worktree would have been torn down by (a) + (b).
    //
    //    We `tracing::warn!` on every self-heal action so the user
    //    knows stale state was discarded — silent auto-cleanup of
    //    disk contents would be a footgun (e.g. untracked-but-
    //    intentional files in the orphan dir would be lost). The
    //    `worktree_path.exists()` pre-check below still acts as a
    //    safety net in case self-heal fails for any reason.
    //
    //    Order matters: prune metadata BEFORE the worktree-add call
    //    (a), delete the branch BEFORE the `Repository::branch`
    //    recreate (b), and remove the orphan dir BEFORE step 4's
    //    worktree add (c). All three happen after `check_clean`
    //    (which is performed in the Tauri command, not here) so
    //    the project root's dirty state doesn't influence our
    //    self-heal.
    let repo = Repository::open(project_path)?;
    let branch_full = branch_name(session_id);

    // 2a. Stale worktree metadata: if libgit2's worktree list still
    //     has an entry for `session_id` but the on-disk directory is
    //     gone (or about to be re-created), `prune` cleans up the
    //     metadata so the next `Repository::worktree(...)` call
    //     succeeds. `prune` also unlinks the metadata from
    //     `.git/worktrees/<session_id>/` so future `git worktree
    //     list` won't see a ghost entry.
    if let Ok(worktrees) = repo.worktrees() {
        if worktrees.iter().any(|name| name.as_deref() == Some(session_id)) {
            tracing::warn!(
                project = %project_path.display(),
                session_id = %session_id,
                "self-heal: found stale worktree metadata; pruning"
            );
            if let Ok(wt) = repo.find_worktree(session_id) {
                if let Err(e) = wt.prune(None) {
                    tracing::warn!(
                        session_id = %session_id,
                        error = %e,
                        "self-heal: worktree metadata prune failed (non-fatal)"
                    );
                }
            }
        }
    }

    // 2b. Stale `session/<id>` branch: a previous crashed create
    //     may have left a branch reference in `.git/refs/heads/`.
    //     `Repository::branch(..., force=false)` will refuse to
    //     re-create, so we delete the old one first. We DO NOT
    //     force-update in-place because the user's WIP on the
    //     branch (if any) would be lost; deletion is the right
    //     move because the worktree that owned it is gone
    //     (2a pruned its metadata above). If the user really
    //     wanted to keep the WIP, they'd `git fetch` it from
    //     elsewhere before re-attaching.
    if let Ok(mut existing_branch) =
        repo.find_branch(&branch_full, BranchType::Local)
    {
        tracing::warn!(
            project = %project_path.display(),
            branch = %branch_full,
            "self-heal: found stale session branch; deleting"
        );
        if let Err(e) = existing_branch.delete() {
            tracing::warn!(
                branch = %branch_full,
                error = %e,
                "self-heal: session branch delete failed (non-fatal)"
            );
        }
    }

    // 2c. Orphan `worktree_path` directory. After (2a) pruned any
    //     libgit2 metadata and (2b) deleted any matching branch,
    //     anything still standing at `worktree_path` is by
    //     construction not a real worktree — it is a leftover
    //     directory from a partial / crashed run. We remove it so
    //     step 5's `Repository::worktree(...)` has a clean slate.
    //     We log loudly because the contents (if any) are about
    //     to be lost; in practice the contents are usually empty
    //     or contain only `README.md` / scaffolding from the
    //     previous attempt.
    if worktree_path.exists() {
        tracing::warn!(
            project = %project_path.display(),
            session_id = %session_id,
            worktree = %worktree_path.display(),
            "self-heal: found orphan worktree directory; removing"
        );
        std::fs::remove_dir_all(worktree_path).map_err(|e| GitError::Io {
            path: worktree_path.display().to_string(),
            source: e,
        })?;
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
}
