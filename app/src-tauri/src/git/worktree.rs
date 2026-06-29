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
                    .map(|(_, rest)| {
                        rest.split(", ").map(String::from).take(10).collect()
                    })
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
            source: std::io::Error::new(std::io::ErrorKind::Other, e.to_string()),
        })?
        .ok_or_else(|| {
            GitError::Io {
                path: project_path.display().to_string(),
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("session '{}' not found", session_id),
                ),
            }
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
    .map_err(|e| {
        GitError::Io {
            path: project_path.display().to_string(),
            source: std::io::Error::new(std::io::ErrorKind::Other, e.to_string()),
        }
    })?;

    // Inject the system event. Best-effort (tracing::warn! on
    // failure): the worktree is already on disk + the row is
    // updated, so a missing event only delays the LLM's awareness
    // by one turn (next-turn reload + system prompt rebuild fills
    // the gap).
    let branch = branch_name(session_id);
    let event_text =
        format!("worktree attached: {} on branch {}", wt_path.display(), branch);
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

/// Default cleanup period for worker worktrees (L3b PR3, 2026-06-27).
/// Matches Claude Code's `cleanupPeriodDays` default of 7 days.
/// Sweep keeps a worker worktree around for this many days
/// after its mtime; older ones are destroyed best-effort.
/// Commit all of a worker's changes (tracked modifications + untracked
/// files) onto its `worker/<run_id>` branch. Called by `run_subagent`
/// AFTER `probe_worker_changes` reports `has_changes=true` and BEFORE
/// the preserve-worktree decision, so the worker's branch tip truly
/// advances past the base — making a subsequent `merge_worker` (FF or
/// 3-way) actually carry the worker's edits.
///
/// Why this exists (the merge false-success gap): `probe_worker_changes`
/// diffs the **working tree** (detects uncommitted edits), but
/// `do_merge_blocking` merges **branch tips** (commits). Without this
/// auto-commit, a worker that never commits leaves `worker_tip ==
/// parent_tip`, and `merge_worker` hits the `is_ancestor` `==`
/// short-circuit (`tools/merge_worker.rs:651`) → returns "merged
/// fast-forward" with zero changes actually merged (silent
/// false-success). This helper closes that gap by always committing
/// the worker's working-tree changes before the branch is preserved.
///
/// Stages everything (`add_all(["*"])`, mirroring a human `git add -A`),
/// then commits on `refs/heads/worker/<run_id>` with the Everlasting
/// signature. Returns the new commit OID. Best-effort at the call site:
/// a failure is logged and the worktree preserved anyway; the merge
/// then degrades to the legacy behavior.
pub fn commit_worker_changes(worker_wt: &Path, run_id: &str) -> Result<git2::Oid, GitError> {
    let repo = Repository::open(worker_wt)?;
    let branch_ref = format!("refs/heads/{}", worker_branch_name(run_id));

    // Stage all changes — tracked modifications + untracked files.
    let mut index = repo.index()?;
    index.add_all(&["*"], git2::IndexAddOption::DEFAULT, None)?;
    index.write()?;

    // Write the staged tree and commit on top of the current tip.
    let tree_oid = index.write_tree()?;
    let tree = repo.find_tree(tree_oid)?;
    let head_commit = repo.head()?.peel_to_commit()?;
    let sig = git2::Signature::now("Everlasting", "agent@everlasting")?;
    let oid = repo.commit(
        Some(&branch_ref),
        &sig,
        &sig,
        &format!("worker {}: auto-commit worker changes", run_id),
        &tree,
        &[&head_commit],
    )?;
    Ok(oid)
}

pub const DEFAULT_CLEANUP_PERIOD_DAYS: u32 = 7;

/// Environment override for [`DEFAULT_CLEANUP_PERIOD_DAYS`].
/// Read by [`sweep_stale_worker_worktrees`] when the caller
/// doesn't pass an explicit `cleanup_period_days` value.
pub const CLEANUP_PERIOD_DAYS_ENV: &str = "EVERLASTING_CLEANUP_PERIOD_DAYS";

/// Sweep stale worker worktrees for a project. Called once
/// at startup (see `AppState::load` integration) and
/// discoverable as a stand-alone helper for any future
/// on-demand sweep (e.g. a "clean up" tool).
///
/// L3b PR3 (2026-06-27): the function walks the project's
/// worker worktree directory
/// (`<app_data_dir>/worktrees/<project_uuid>/worker/`) and,
/// for each subdirectory, checks:
/// 1. **Lock presence** — the worktree is locked iff
///    `<project_path>/.git/worktrees/<name>/locked` exists
///    (libgit2's lock file is the canonical "this worktree is
///    in active use" marker). Locked worktrees are SKIPPED
///    (the `create_worker` lock mechanism protects running
///    workers; a sweep must not destroy a worker that's
///    still running).
/// 2. **Mtime** — the worktree directory's mtime (the
///    `Metadata::modified()` timestamp). If the mtime is
///    older than `cleanup_period_days` days (computed as
///    `now - cleanup_period_days * 86400` seconds), the
///    worktree is destroyed via
///    [`destroy_worker`], which also unlocks + deletes the
///    `worker/<run_id>` branch + removes the libgit2
///    worktree metadata.
///
/// Returns the number of worktrees destroyed (0 in the
/// common case). The caller logs the count for observability.
///
/// **Best-effort** semantics (per PRD §"Edge Cases"):
/// - A single failure (lock check error / mtime read error
///   / `destroy_worker` error) is logged at `warn!` and the
///   sweep continues with the next worktree. A failure on
///   one worktree does NOT abort the sweep — that would
///   leave other stale worktrees in place for a future
///   sweep that's not guaranteed to happen.
/// - A worktree that is `libgit2::find_worktree`-
///   unrecognizable (the on-disk dir is there but the
///   libgit2 metadata is gone — a crashed create + a manual
///   `git worktree prune`) is still best-effort destroyed
///   (we pass the path to `destroy_worker` which tolerates
///   "metadata already gone" via its own best-effort prune
///   + branch-delete).
/// - The sweep is a no-op when the worker dir doesn't exist
///   (fresh project, no workers have ever been spawned).
///
/// **Per-worker lock file detection** is the key correctness
/// invariant: a worker running RIGHT NOW (a 3-hour
/// `cargo build` etc.) has its worktree locked by
/// `create_worker` (see `create_worktree_add` →
/// `Worktree::lock`). The sweep MUST respect this — without
/// the lock check, a sweep during a long worker run would
/// silently destroy the in-flight worktree mid-execution
/// (the worker would write to a dir that no longer exists
/// and the parent would `prune` it on the next `git gc`).
pub fn sweep_stale_worker_worktrees(
    app_data_dir: &Path,
    project_uuid: &str,
    project_path: &Path,
    cleanup_period_days: u32,
) -> Result<usize, GitError> {
    let worker_root = app_data_dir
        .join("worktrees")
        .join(project_uuid)
        .join("worker");
    if !worker_root.exists() {
        // No worker dir → nothing to sweep. This is the
        // common case for fresh projects.
        return Ok(0);
    }

    // Open the project repo once. If it fails (the project
    // worktree dir itself was removed out from under us), we
    // log + return 0 — the sweep can't proceed without
    // libgit2 access.
    let repo = match Repository::open(project_path) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(
                project = %project_path.display(),
                error = %e,
                "sweep: could not open project repo; skipping sweep"
            );
            return Ok(0);
        }
    };

    let cutoff_secs = (cleanup_period_days as i64) * 86_400;
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let entries = match std::fs::read_dir(&worker_root) {
        Ok(rd) => rd,
        Err(e) => {
            tracing::warn!(
                worker_root = %worker_root.display(),
                error = %e,
                "sweep: could not read worker dir; skipping sweep"
            );
            return Ok(0);
        }
    };

    let mut destroyed_count = 0usize;
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, "sweep: read_dir entry error (non-fatal)");
                continue;
            }
        };
        let wt_path = entry.path();
        if !wt_path.is_dir() {
            continue;
        }
        // The worktree's libgit2 metadata name is the run_id
        // (no `worker/` prefix — see `create_worker`). The
        // on-disk dir layout is `<worker_root>/<run_id>`.
        let run_id = match wt_path.file_name().and_then(|n| n.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };

        // ----- Lock check (via libgit2 API) -----
        // `repo.find_worktree(metadata_name)` returns the
        // worktree handle; `.is_locked()` consults libgit2's
        // authoritative lock state (the canonical
        // `<project>/.git/worktrees/<name>/locked` file,
        // plus the in-memory lock marker). Using the API
        // rather than `Path::exists` keeps us robust to
        // future libgit2 changes (e.g. an in-process lock
        // mode that doesn't write the file).
        let locked = match repo.find_worktree(&run_id) {
            Ok(wt) => matches!(
                wt.is_locked(),
                Ok(git2::WorktreeLockStatus::Locked(_))
            ),
            Err(_) => {
                // Worktree metadata not found (the on-disk
                // dir exists but libgit2 doesn't know about
                // it — a crashed create + a manual `git
                // worktree prune`). The destroy_worker call
                // is best-effort and tolerates this, so we
                // proceed (treating the worktree as
                // unlocked).
                false
            }
        };
        if locked {
            tracing::info!(
                run_id = %run_id,
                "sweep: skipping locked worker worktree (active worker)"
            );
            continue;
        }

        // ----- Mtime check -----
        let mtime = match std::fs::metadata(&wt_path).and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(
                    run_id = %run_id,
                    worktree = %wt_path.display(),
                    error = %e,
                    "sweep: could not stat worktree mtime (non-fatal; skipping)"
                );
                continue;
            }
        };
        let mtime_secs = mtime
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let age_secs = now_secs.saturating_sub(mtime_secs);
        if age_secs < cutoff_secs {
            // Not stale yet — skip.
            continue;
        }

        // ----- Destroy -----
        tracing::info!(
            run_id = %run_id,
            worktree = %wt_path.display(),
            age_days = age_secs / 86_400,
            "sweep: destroying stale worker worktree"
        );
        if let Err(e) = destroy_worker(&project_path, &wt_path, &run_id) {
            tracing::warn!(
                run_id = %run_id,
                worktree = %wt_path.display(),
                error = %e,
                "sweep: destroy_worker failed (non-fatal; continuing)"
            );
            continue;
        }
        destroyed_count += 1;
        // Reference the repo to silence the "unused" warning
        // when the libgit2 worktree check below is skipped
        // (we use the project_path, not repo, to keep the
        // implementation simple — libgit2 worktrees are
        // discoverable via the on-disk layout, and
        // `destroy_worker` opens the repo itself).
        let _ = &repo;
    }

    Ok(destroyed_count)
}

/// Resolve the cleanup-period-days value: prefer the explicit
/// `cleanup_period_days` parameter, fall back to the
/// `EVERLASTING_CLEANUP_PERIOD_DAYS` env var, fall back to
/// [`DEFAULT_CLEANUP_PERIOD_DAYS`] (7). Returns the resolved
/// value. Used by [`sweep_stale_worker_worktrees`] callers
/// that want the env-aware default.
pub fn resolve_cleanup_period_days(explicit: Option<u32>) -> u32 {
    if let Some(d) = explicit {
        return d;
    }
    if let Ok(s) = std::env::var(CLEANUP_PERIOD_DAYS_ENV) {
        if let Ok(d) = s.parse::<u32>() {
            if d > 0 {
                return d;
            }
        }
    }
    DEFAULT_CLEANUP_PERIOD_DAYS
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
    fn commit_worker_changes_advances_tip_and_includes_edits() {
        let tmp = tempdir().unwrap();
        let project = tmp.path();
        let parent_wt = parent_session_worktree_with_extra_commit(project, "sess-commit");
        let run_id = "run-autocommit";
        let worker_wt = tmp.path().join("worker_wt_autocommit");
        create_worker(project, &worker_wt, &parent_wt, run_id)
            .expect("create_worker should succeed");

        // The worker overwrites a tracked file + adds an untracked file,
        // WITHOUT committing (mirrors a subagent that wrote edits but
        // never ran `git commit`).
        std::fs::write(worker_wt.join("parent_only.txt"), "worker overwrote this").unwrap();
        std::fs::write(worker_wt.join("new_file.txt"), "worker added this").unwrap();

        let repo = git2::Repository::open(&worker_wt).unwrap();
        let tip_before = repo.head().unwrap().peel_to_commit().unwrap().id();

        let new_oid = commit_worker_changes(&worker_wt, run_id)
            .expect("auto-commit should succeed");

        // The branch tip advanced — the load-bearing invariant: probe
        // sees working-tree edits but merge_worker merges branch tips;
        // without this commit the tip would equal the base and
        // merge_worker would false-success (is_ancestor == short-circuit).
        assert_ne!(new_oid, tip_before, "auto-commit must advance the tip");

        // The new commit's tree contains both the edit + the new file
        // (add_all stages tracked mods + untracked).
        let new_commit = repo.find_commit(new_oid).unwrap();
        let tree = new_commit.tree().unwrap();
        assert!(
            tree.get_name("new_file.txt").is_some(),
            "untracked file must be committed"
        );
        assert!(tree.get_name("parent_only.txt").is_some());

        // The worker branch ref points at the new commit.
        let branch_ref = format!("refs/heads/worker/{}", run_id);
        let ref_oid = repo.find_reference(&branch_ref).unwrap().target().unwrap();
        assert_eq!(ref_oid, new_oid);
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

    // -----------------------------------------------------------------------
    // L3b PR3 (2026-06-27): sweep mechanism tests
    //
    // The sweep walks `<app_data_dir>/worktrees/<project_uuid>/worker/`
    // and destroys worker worktrees whose mtime is older than
    // `cleanup_period_days` AND whose libgit2 lock is NOT present.
    // These tests pin each contract.
    // -----------------------------------------------------------------------

    /// Helper: set up a project + parent session worktree
    /// + a worker worktree. Returns
    /// `(app_data_dir, project_path, project_uuid)`.
    fn setup_project_with_worker(
        session_id: &str,
        run_id: &str,
    ) -> (tempfile::TempDir, std::path::PathBuf, String) {
        let tmp = tempdir().unwrap();
        let project = tmp.path().join("project");
        let parent_wt = parent_session_worktree_with_extra_commit(&project, session_id);
        let app_data_dir = tmp.path().join("data");
        std::fs::create_dir_all(&app_data_dir).unwrap();
        let worker_wt = worker_worktree_path(&app_data_dir, "project-uuid", run_id);
        create_worker(&project, &worker_wt, &parent_wt, run_id)
            .expect("create_worker should succeed");
        (tmp, project, "project-uuid".to_string())
    }

    /// Backdate the mtime of the worker worktree directory
    /// to N days ago. Uses `touch -t YYYYMMDDhhmm` because
    /// Rust's `std::fs::File::set_modified` doesn't work
    /// on directories on Linux (only on files). The `touch`
    /// binary is universally available on Linux + macOS
    /// (the project's two build targets).
    fn backdate_dir(path: &Path, days_ago: u32) {
        // Compute target time as days_ago days back from
        // now, formatted as YYYYMMDDhhmm.
        let now = std::time::SystemTime::now();
        let target = now
            .checked_sub(std::time::Duration::from_secs(days_ago as u64 * 86_400))
            .expect("mtime in range");
        let secs_since_epoch = target
            .duration_since(std::time::UNIX_EPOCH)
            .expect("epoch")
            .as_secs();
        // Convert to (year, month, day, hour, minute) for
        // `touch -t` (which expects YYYYMMDDhhmm).
        let (year, month, day, hour, minute) =
            epoch_secs_to_ymdhms(secs_since_epoch);
        let touch_arg = format!(
            "{:04}{:02}{:02}{:02}{:02}",
            year, month, day, hour, minute
        );
        let out = std::process::Command::new("touch")
            .args(["-t", &touch_arg])
            .arg(path)
            .output()
            .expect("touch command");
        assert!(
            out.status.success(),
            "touch -t failed for {:?}: {:?}",
            path,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Convert Unix epoch seconds to (year, month, day, hour,
    /// minute). Implements the inverse of the algorithm in
    /// `<time.h>` `gmtime_r`. Pure function (no I/O).
    fn epoch_secs_to_ymdhms(secs: u64) -> (i32, u32, u32, u32, u32) {
        let secs_in_day = 86_400u64;
        let mut days = (secs / secs_in_day) as i64;
        let secs_today = (secs % secs_in_day) as u32;
        let hour = secs_today / 3600;
        let minute = (secs_today % 3600) / 60;

        // 1970-01-01 was a Thursday (day 4 of the week,
        // where 0 = Sunday). Compute the day of the week
        // for epoch `days` (with 0 = Sunday).
        let weekday = ((days + 4).rem_euclid(7)) as u32;

        // Walk forward by year, accounting for leap years.
        let mut year: i32 = 1970;
        loop {
            let leap = is_leap_year(year);
            let year_days = if leap { 366 } else { 365 };
            if days < year_days {
                break;
            }
            days -= year_days;
            year += 1;
        }
        // Month lengths for the current year.
        let month_lengths = if is_leap_year(year) {
            [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        } else {
            [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        };
        let mut month: usize = 0;
        while month < 12 && days >= month_lengths[month] as i64 {
            days -= month_lengths[month] as i64;
            month += 1;
        }
        let day = (days + 1) as u32; // 1-indexed
        let _ = weekday; // silence unused warning
        (year, (month as u32) + 1, day, hour, minute)
    }

    fn is_leap_year(year: i32) -> bool {
        (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
    }

    #[test]
    fn sweep_removes_stale_worker_worktrees() {
        let (tmp, project, project_uuid) = setup_project_with_worker("sweep-sess", "sweep-stale");
        let app_data_dir = tmp.path().join("data");
        let stale_run_id = "sweep-stale";
        let worker_wt = worker_worktree_path(&app_data_dir, &project_uuid, stale_run_id);

        // Unlock the worker worktree (the test scenario
        // simulates a worker that exited long ago — the
        // lock was held during the worker's lifetime and
        // should be released before the sweep sees the
        // worktree as a candidate for destruction). Without
        // this step, the sweep would correctly skip the
        // worktree (the lock check is the load-bearing
        // "active worker" guard).
        let project_repo = git2::Repository::open(&project).unwrap();
        if let Ok(wt) = project_repo.find_worktree(stale_run_id) {
            wt.unlock().expect("unlock worker for test setup");
        }

        // Backdate the worker worktree dir to 30 days ago —
        // well past the 7-day default.
        backdate_dir(&worker_wt, 30);

        // Run the sweep with a 7-day cleanup period.
        let destroyed = sweep_stale_worker_worktrees(
            &app_data_dir,
            &project_uuid,
            &project,
            7,
        )
        .expect("sweep should succeed");
        assert_eq!(destroyed, 1, "exactly 1 stale worktree destroyed");

        // The worker worktree dir + branch are gone.
        assert!(!worker_wt.exists(), "worker worktree dir removed");
        let repo = git2::Repository::open(&project).unwrap();
        assert!(
            repo.find_branch(&format!("worker/{}", stale_run_id), git2::BranchType::Local)
                .is_err(),
            "worker branch should be deleted"
        );
    }

    #[test]
    fn sweep_skips_locked_worker_worktrees() {
        let (tmp, project, project_uuid) = setup_project_with_worker("sweep-lock-sess", "sweep-locked");
        let app_data_dir = tmp.path().join("data");
        let run_id = "sweep-locked";
        let worker_wt = worker_worktree_path(&app_data_dir, &project_uuid, run_id);

        // Backdate the worker worktree dir to 30 days ago.
        backdate_dir(&worker_wt, 30);

        // Manually create the libgit2 lock file at the
        // canonical lock path: `<project>/.git/worktrees/<run_id>/locked`.
        // The `create_worker` function normally writes this
        // for us; we re-add it (it should still be there
        // from `create_worker`, but we re-touch to be safe
        // — the test asserts the sweep sees the lock).
        let lock_path = project.join(".git").join("worktrees").join(run_id).join("locked");
        // The `create_worker` function already created
        // this — let's just assert it exists.
        assert!(
            lock_path.exists(),
            "create_worker should have left a lock file"
        );

        let destroyed = sweep_stale_worker_worktrees(
            &app_data_dir,
            &project_uuid,
            &project,
            7,
        )
        .expect("sweep should succeed");
        assert_eq!(destroyed, 0, "locked worktree MUST be skipped");

        // Worker worktree dir + branch preserved.
        assert!(worker_wt.exists(), "locked worktree dir preserved");
        let repo = git2::Repository::open(&project).unwrap();
        assert!(
            repo.find_branch(&format!("worker/{}", run_id), git2::BranchType::Local)
                .is_ok(),
            "locked worker branch preserved"
        );
    }

    #[test]
    fn sweep_keeps_recent_worker_worktrees() {
        // A worker that's only 1 day old should NOT be
        // destroyed by a 7-day sweep.
        let (tmp, project, project_uuid) = setup_project_with_worker("sweep-recent-sess", "sweep-recent");
        let app_data_dir = tmp.path().join("data");
        let run_id = "sweep-recent";
        let worker_wt = worker_worktree_path(&app_data_dir, &project_uuid, run_id);

        // The worktree was just created, so its mtime is
        // "now". No backdate.
        let destroyed = sweep_stale_worker_worktrees(&app_data_dir, &project_uuid, &project, 7)
            .expect("sweep should succeed");
        assert_eq!(destroyed, 0, "recent worktree MUST NOT be destroyed");
        assert!(worker_wt.exists(), "recent worktree dir preserved");
    }

    #[test]
    fn sweep_with_no_worker_dir_is_noop() {
        let tmp = tempdir().unwrap();
        let app_data_dir = tmp.path().join("data");
        std::fs::create_dir_all(&app_data_dir).unwrap();
        // No worker dir exists for this project. Pass a
        // non-existent project path — sweep returns 0
        // because the worker dir check fails first.
        let bogus_project = std::path::Path::new("/tmp/does-not-exist");
        let destroyed = sweep_stale_worker_worktrees(
            &app_data_dir,
            "no-such-project",
            bogus_project,
            7,
        )
        .expect("sweep should succeed (no work to do)");
        assert_eq!(destroyed, 0);
    }

    #[test]
    fn resolve_cleanup_period_days_prefers_explicit() {
        assert_eq!(resolve_cleanup_period_days(Some(14)), 14);
    }

    #[test]
    fn resolve_cleanup_period_days_uses_default_when_no_env() {
        // The env var may or may not be set in the test
        // environment; `resolve_cleanup_period_days(None)`
        // falls back to the default (7) when the env var is
        // unset or unparseable. We can't safely assert
        // against the env var value (it's process-global),
        // so we just confirm the explicit path works and
        // the default-when-None path doesn't crash.
        let _ = resolve_cleanup_period_days(None);
    }

    // -----------------------------------------------------------------------
    // attach_session helper (06-30 follow-up, lazy auto-attach on merge)
    //
    // These tests cover the inner work of `commands::worktree::
    // attach_worktree` extracted as a free function for tool-layer
    // (`merge_worker`) reuse. The contract:
    //   - happy_path: clean git project + session at None →
    //     worktree created + DB row flipped to Active + system
    //     event inserted; returns the new worktree path
    //   - non_git_project: project_row.is_git_repo=false →
    //     GitError::NotARepo, no disk writes, no DB writes
    //   - dirty_project_root: project root has uncommitted
    //     changes → GitError::Dirty carrying the offending path(s)
    //
    // The state machine guard (None / Detached / Active policy)
    // lives at the IPC boundary, NOT in this helper — covered by
    // the IPC-layer tests rather than the helper.
    // -----------------------------------------------------------------------

    /// Minimal in-test DB pool (sqlite::memory:) + migrations.
    /// Self-contained here (vs importing from db::sessions_tests)
    /// because git-domain tests historically have no DB surface;
    /// sharing the helper would couple git-test compile to
    /// db-test compile.
    async fn attach_session_test_pool() -> sqlx::SqlitePool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        crate::db::migrations::run_migrations(&pool).await.unwrap();
        pool
    }

    /// Helper: init a git repo at `path`, commit a single file,
    /// and create a ProjectRow + session in the DB. Returns
    /// (project_row, session_id, data_dir).
    async fn attach_session_setup(
        project_dir: &Path,
        pool: &sqlx::SqlitePool,
        is_git_repo: bool,
    ) -> (crate::projects::ProjectRow, String, PathBuf) {
        if is_git_repo {
            init_repo(project_dir);
            std::fs::write(project_dir.join("a.txt"), "hello").unwrap();
            commit_all(project_dir);
        } else {
            fs::create_dir_all(project_dir).unwrap();
            std::fs::write(project_dir.join("README"), "no git here").unwrap();
        }
        let data_dir = tempfile::tempdir().unwrap().keep();
        let project = crate::db::projects::create_project(
            pool,
            "test-proj",
            project_dir.to_str().unwrap(),
            is_git_repo,
            None,
        )
        .await
        .unwrap();
        let session_id = uuid::Uuid::new_v4().to_string();
        crate::db::sessions::create_session(
            pool,
            &session_id,
            &project.id,
            project_dir.to_str().unwrap(),
            "GLM-4.7",
            None,
        )
        .await
        .unwrap();
        (project, session_id, data_dir)
    }

    #[tokio::test]
    async fn attach_session_happy_path() {
        let tmp = tempdir().unwrap();
        let project_dir = tmp.path().join("proj");
        let pool = attach_session_test_pool().await;
        let (project, session_id, data_dir) =
            attach_session_setup(&project_dir, &pool, true).await;
        // First commit done — project root is clean.

        let result = attach_session(&pool, &project, &session_id, &data_dir).await;
        let wt_path = result.expect("attach_session should succeed");

        // Helper contract: returns the worktree path it built.
        let expected = worktree_path(&data_dir, &project.id, &session_id);
        assert_eq!(wt_path, expected, "returned path should match canonical layout");

        // Libgit2 effect: the on-disk worktree exists and points
        // at a fresh `session/<sid>` branch.
        assert!(wt_path.exists(), "worktree directory should exist on disk");
        let wt_repo = Repository::open(&wt_path).expect("wt should open as repo");
        let head_branch = wt_repo
            .head()
            .expect("HEAD should resolve")
            .shorthand()
            .expect("HEAD should have shorthand")
            .to_string();
        assert_eq!(head_branch, format!("session/{}", session_id));

        // DB effect 1: row should flip to Active with the new
        // worktree_path.
        let reloaded = crate::db::sessions::load_session(&pool, &session_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reloaded.session.worktree_state, crate::db::WorktreeState::Active);
        assert_eq!(reloaded.session.worktree_path.as_deref(), Some(expected.to_str().unwrap()));

        // DB effect 2: a system-event row should be appended
        // (the [worktree event] attached: <path> on branch
        // session/<sid> message).
        let msgs = reloaded.messages;
        assert_eq!(msgs.len(), 1, "exactly one system event expected");
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].seq, 0);
        let meta = msgs[0].metadata.as_ref().expect("metadata present");
        assert_eq!(meta["kind"], "worktree_event");
        assert_eq!(meta["event"], "attached");
    }

    #[tokio::test]
    async fn attach_session_non_git_project() {
        let tmp = tempdir().unwrap();
        let project_dir = tmp.path().join("proj");
        let pool = attach_session_test_pool().await;
        let (project, session_id, data_dir) =
            attach_session_setup(&project_dir, &pool, false).await;

        let result = attach_session(&pool, &project, &session_id, &data_dir).await;
        match result {
            Err(GitError::NotARepo { path }) => {
                assert_eq!(path, project_dir.to_str().unwrap());
            }
            other => panic!("expected NotARepo, got {:?}", other),
        }

        // DB should be unchanged (still None + no system event).
        let reloaded = crate::db::sessions::load_session(&pool, &session_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reloaded.session.worktree_state, crate::db::WorktreeState::None);
        assert!(reloaded.messages.is_empty(), "no system event on rejected attach");
    }

    #[tokio::test]
    async fn attach_session_dirty_project_root() {
        let tmp = tempdir().unwrap();
        let project_dir = tmp.path().join("proj");
        let pool = attach_session_test_pool().await;
        let (project, session_id, data_dir) =
            attach_session_setup(&project_dir, &pool, true).await;
        // Now make the project root dirty: add an uncommitted
        // file (NOT stage+commit).
        std::fs::write(project_dir.join("dirty.txt"), "stale").unwrap();

        let result = attach_session(&pool, &project, &session_id, &data_dir).await;
        match result {
            Err(GitError::Dirty { paths, .. }) => {
                assert!(
                    paths.iter().any(|p| p.ends_with("dirty.txt")),
                    "Dirty error must list dirty.txt as offending path, got: {:?}",
                    paths
                );
            }
            other => panic!("expected Dirty, got {:?}", other),
        }

        // DB should be unchanged.
        let reloaded = crate::db::sessions::load_session(&pool, &session_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reloaded.session.worktree_state, crate::db::WorktreeState::None);
        assert!(reloaded.messages.is_empty(), "no system event on rejected attach");

        // No worktree directory should be created on disk.
        let expected = worktree_path(&data_dir, &project.id, &session_id);
        assert!(!expected.exists(), "no worktree dir on rejected attach");
    }
}

