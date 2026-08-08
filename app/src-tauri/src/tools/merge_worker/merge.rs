//! Merge bodies + per-session merge serialization lock.
//!
//! Split out of `tools/merge_worker.rs` (2026-08-08 batch3).
//! The `merge_lock_for` static `LOCKS` + both merge functions MUST stay
//! in the same module — the lock is what serializes concurrent merges.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use git2::{MergeOptions, Repository};

use crate::git;

/// Per-parent-session merge serialization (L3b PR3 B2 fix, 2026-06-28).
///
/// `do_merge_blocking` is reached from two `spawn_blocking` sites — the
/// `merge_worker` tool's `execute` and the `merge_worker_run` IPC
/// command — both of which merge into the SAME parent session branch.
/// libgit2 is not thread-safe across `Repository` handles that back the
/// same `.git` dir, so two concurrent merges (e.g. the user clicking
/// Merge on two drawers at once) could corrupt the index / leave a
/// half-merged state. This lock serializes per `parent_session_id`;
/// independent sessions still merge in parallel.
///
/// `std::sync::Mutex` (not tokio) because `do_merge_blocking` is a sync
/// fn on the blocking pool with no `.await` in scope. The outer map
/// lock is held only for the HashMap lookup/insert and released before
/// the inner per-session lock is acquired — fixed order, no deadlock.
fn merge_lock_for(parent_session_id: &str) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    let map = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    map.lock()
        .unwrap()
        .entry(parent_session_id.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// Synchronous merge body. Returns `Ok(message)` on success,
/// `Err(tool_result_content)` on any failure mode (validation,
/// conflict, or libgit2 error). The function takes the parent
/// worktree path + parent session id + worker run id and:
/// 1. Performs a libgit2 three-way / fast-forward merge of
///    `worker/<run_id>` into `session/<id>`.
/// 2. Returns a human-readable message describing the merge
///    outcome (fast-forward, three-way, or conflict).
///
/// The post-merge DB cleanup (worktree destroy + `worktree_path`
/// column clear) is done in [`super::finalize::finalize_merge`]
/// separately because the tool layer doesn't carry a DB pool; the
/// IPC command layer (which does) calls `finalize_merge` after a
/// successful `do_merge`.
///
/// ⑨ 关 enforcement happens upstream; this function trusts the
/// call site to have already passed the permission check.
pub fn do_merge_blocking(
    parent_wt: &Path,
    parent_session_id: &str,
    run_id: &str,
) -> Result<String, String> {
    // Serialize per parent session (see `merge_lock_for`). The guard
    // spans the whole libgit2 merge, covering both `spawn_blocking`
    // call sites (tool `execute` + IPC `merge_worker_run`). The Arc is
    // bound to its own `let` so it outlives the guard (the guard borrows
    // the Mutex inside the Arc).
    let _merge_lock = merge_lock_for(parent_session_id);
    let _merge_guard = _merge_lock.lock().unwrap();
    // Open the parent worktree repo (libgit2's
    // `Repository::open` works for both full repos and
    // linked worktrees; the resulting handle can read
    // `session/<id>` from `.git/worktrees/<sid>/refs/`).
    let repo = Repository::open(parent_wt).map_err(|e| {
        format!(
            "merge_worker: could not open parent worktree at '{}': {}",
            parent_wt.display(),
            e
        )
    })?;
    let parent_branch_name = git::worktree::branch_name(parent_session_id);

    // Resolve "ours" (parent's session branch tip) and
    // "theirs" (worker's branch tip).
    let parent_branch = repo
        .find_branch(&parent_branch_name, git2::BranchType::Local)
        .map_err(|e| {
            format!(
                "merge_worker: parent branch '{}' not found (parent session has no worktree?): {}",
                parent_branch_name, e
            )
        })?;
    let parent_tip_oid = parent_branch
        .get()
        .peel_to_commit()
        .map_err(|e| format!("merge_worker: could not resolve parent branch tip: {}", e))?;

    let worker_branch_name = git::worktree::worker_branch_name(run_id);
    let worker_branch = repo
        .find_branch(&worker_branch_name, git2::BranchType::Local)
        .map_err(|e| {
            format!(
                "merge_worker: worker branch '{}' not found (already merged / discarded?): {}",
                worker_branch_name, e
            )
        })?;
    let worker_tip_oid = worker_branch
        .get()
        .peel_to_commit()
        .map_err(|e| format!("merge_worker: could not resolve worker branch tip: {}", e))?;

    // Fast-forward path: if the parent branch tip is an
    // ancestor of the worker branch tip, we just move the
    // parent branch forward (no merge commit). This is the
    // common case after an isolated worker that didn't
    // touch the parent checkout.
    if is_ancestor(&repo, parent_tip_oid.id(), worker_tip_oid.id())? {
        // Move the parent branch ref to the worker tip.
        // `repo.reference(name, oid, force, ...)` gives us a
        // mutable handle we can write through. Passing
        // `force=true` overwrites the existing ref (without
        // it, libgit2 refuses to move a branch to a non-
        // descendant commit).
        let mut parent_ref = repo
            .reference(
                &format!("refs/heads/{}", parent_branch_name),
                worker_tip_oid.id(),
                true,
                "merge_worker: fast-forward",
            )
            .map_err(|e| format!("merge_worker: could not fast-forward parent branch: {}", e))?;
        // Touch the variable so the compiler doesn't warn
        // about the unused mut (the `repo.reference` call
        // itself performs the write; the handle is just a
        // guard to keep the ref alive across the call).
        let _ = &mut parent_ref;
        // Update the parent worktree's HEAD + index to
        // match the new branch tip. libgit2's
        // `Repository::checkout_head` walks the index and
        // updates the workdir.
        let mut checkout_opts = git2::build::CheckoutBuilder::new();
        checkout_opts.force();
        repo.checkout_head(Some(&mut checkout_opts))
            .map_err(|e| format!("merge_worker: post-fast-forward checkout failed: {}", e))?;
        return Ok(format!(
            "merged {} (fast-forward, 0 merge commit)",
            worker_branch_name
        ));
    }

    // Three-way merge path. Resolve AnnotatedCommit for
    // theirs (ours is implicit via HEAD in libgit2's
    // `Repository::merge`). git2-rs 0.20 has no
    // `AnnotatedCommit::lookup`; the only way to build an
    // AnnotatedCommit from a branch is via
    // `reference_to_annotated_commit`. The worker's
    // `Reference` is the branch's tip ref.
    let worker_annotated = {
        let worker_ref = worker_branch.get();
        repo.reference_to_annotated_commit(worker_ref)
            .map_err(|e| {
                format!(
                    "merge_worker: could not build annotated commit for worker branch: {}",
                    e
                )
            })?
    };

    // Set up merge options with conflict-style detection
    // (file_favor: Normal — when a conflict happens, the
    // resulting tree contains conflict markers in the
    // conflicted files; the workdir is left in a
    // half-merged state, which we use to detect conflicts
    // after the merge).
    let mut merge_opts = MergeOptions::new();
    let mut checkout_opts = git2::build::CheckoutBuilder::new();
    checkout_opts.allow_conflicts(true);
    checkout_opts.conflict_style_diff3(false);
    checkout_opts.force();
    repo.merge(
        &[&worker_annotated],
        Some(&mut merge_opts),
        Some(&mut checkout_opts),
    )
    .map_err(|e| {
        format!(
            "merge_worker: libgit2 merge failed: {} (likely a conflict; check the parent worktree state)",
            e
        )
    })?;

    // After a merge, `repo.index()` may have unresolved
    // conflicts (`index.has_conflicts()` is true). We
    // detect this and return a structured error WITHOUT
    // committing — the user must resolve manually.
    let mut index = repo
        .index()
        .map_err(|e| format!("merge_worker: could not load index after merge: {}", e))?;
    if index.has_conflicts() {
        // Collect the conflict paths so the LLM can
        // surface them to the user.
        let conflicts = collect_conflict_paths(&index);
        let file_list = if conflicts.is_empty() {
            "(unknown)".to_string()
        } else {
            conflicts.join(", ")
        };
        // Reset the merge to a clean HEAD (so the
        // worktree isn't left in a half-merged state
        // for the user's next tool call). The
        // alternative — leaving the worktree with
        // conflict markers — would corrupt the next
        // `edit_file` / `read_file` round-trip.
        let parent_commit = match parent_branch.get().peel_to_commit() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "merge_worker: post-conflict peel failed; skipping reset"
                );
                return Err(format!(
                    "merge conflict: [{}]. The worker branch '{}' and parent branch '{}' both modified these files. \
                     Resolve manually, then call merge_worker again (or discard_worker to drop the changes).",
                    file_list, worker_branch_name, parent_branch_name
                ));
            }
        };
        let parent_obj = parent_commit.into_object();
        let mut reset_checkout = git2::build::CheckoutBuilder::new();
        reset_checkout.force();
        reset_checkout.remove_untracked(true);
        if let Err(e) = repo.reset(
            &parent_obj,
            git2::ResetType::Hard,
            Some(&mut reset_checkout),
        ) {
            tracing::warn!(
                error = %e,
                "merge_worker: post-conflict reset failed (worktree may be in half-merged state)"
            );
            // Don't fail the tool; the conflict result
            // is the user-visible signal.
        }

        return Err(format!(
            "merge conflict: [{}]. The worker branch '{}' and parent branch '{}' both modified these files. \
             Resolve manually, then call merge_worker again (or discard_worker to drop the changes).",
            file_list, worker_branch_name, parent_branch_name
        ));
    }

    // Merge succeeded cleanly. Commit the merge.
    let merge_oid = {
        let sig = repo
            .signature()
            .unwrap_or_else(|_| git2::Signature::now("Everlasting", "agent@everlasting").unwrap());
        let tree_oid = index
            .write_tree()
            .map_err(|e| format!("merge_worker: could not write merge tree: {}", e))?;
        let tree = repo
            .find_tree(tree_oid)
            .map_err(|e| format!("merge_worker: could not load merge tree: {}", e))?;
        let parent_commit = repo
            .find_commit(parent_tip_oid.id())
            .map_err(|e| format!("merge_worker: could not load parent commit: {}", e))?;
        let worker_commit = repo
            .find_commit(worker_tip_oid.id())
            .map_err(|e| format!("merge_worker: could not load worker commit: {}", e))?;
        repo.commit(
            Some(&format!("refs/heads/{}", parent_branch_name)),
            &sig,
            &sig,
            &format!(
                "merge_worker: merge {} into {}",
                worker_branch_name, parent_branch_name
            ),
            &tree,
            &[&parent_commit, &worker_commit],
        )
        .map_err(|e| format!("merge_worker: could not write merge commit: {}", e))?
    };

    // Clean up the merge state (resets the index to
    // match HEAD; the user can now proceed).
    repo.cleanup_state()
        .map_err(|e| tracing::warn!(error = %e, "merge_worker: cleanup_state failed (non-fatal)"))
        .ok();

    Ok(format!(
        "merged {} into {} (3-way, merge commit {})",
        worker_branch_name, parent_branch_name, merge_oid
    ))
}

/// D (2026-06-30): merge the session's `session/<id>` branch into
/// `main` (local only — never pushes). Called by the
/// `publish_session_to_main` Tauri command (the "Publish → main"
/// chat-header button). Structurally a sibling of `do_merge_blocking`
/// (FF → 3-way → conflict), but the target is `main` and the source
/// is `session/<id>` (vs `do_merge_blocking`'s `session/<parent>` ←
/// `worker/<run_id>`). Reuses this module's private helpers
/// (`merge_lock_for` / `is_ancestor` / `collect_conflict_paths`).
///
/// On conflict: returns a structured error naming the files and resets
/// `main` to a clean HEAD (no half-merged dirty state — same contract
/// as `do_merge_blocking`). The session worktree is left untouched
/// (the user can keep working in the session; only `main` advances).
pub fn merge_session_into_main(project_path: &Path, session_id: &str) -> Result<String, String> {
    // Per-session lock mirrors `do_merge_blocking` (prevents the same
    // session racing two publishes; cross-session main races are
    // acceptable — last writer wins, git ref move is atomic).
    let _merge_lock = merge_lock_for(session_id);
    let _merge_guard = _merge_lock.lock().unwrap();
    let repo = Repository::open(project_path).map_err(|e| {
        format!(
            "merge_session: could not open project repo at '{}': {}",
            project_path.display(),
            e
        )
    })?;

    let main_branch_name = "main";
    let session_branch_name = git::worktree::branch_name(session_id);

    let main_branch = repo
        .find_branch(main_branch_name, git2::BranchType::Local)
        .map_err(|e| {
            format!(
                "merge_session: '{}' branch not found: {}",
                main_branch_name, e
            )
        })?;
    let main_tip = main_branch
        .get()
        .peel_to_commit()
        .map_err(|e| format!("merge_session: main tip: {}", e))?;
    let session_branch = repo
        .find_branch(&session_branch_name, git2::BranchType::Local)
        .map_err(|e| {
            format!(
                "merge_session: session branch '{}' not found (session has no worktree?): {}",
                session_branch_name, e
            )
        })?;
    let session_tip = session_branch
        .get()
        .peel_to_commit()
        .map_err(|e| format!("merge_session: session tip: {}", e))?;

    // Fast-forward: main is an ancestor of session → just move main
    // forward to the session tip (no merge commit).
    if is_ancestor(&repo, main_tip.id(), session_tip.id())? {
        let mut main_ref = repo
            .reference(
                &format!("refs/heads/{}", main_branch_name),
                session_tip.id(),
                true,
                "merge_session: fast-forward",
            )
            .map_err(|e| format!("merge_session: fast-forward main ref: {}", e))?;
        let _ = &mut main_ref;
        let mut checkout_opts = git2::build::CheckoutBuilder::new();
        checkout_opts.force();
        repo.checkout_head(Some(&mut checkout_opts))
            .map_err(|e| format!("merge_session: post-fast-forward checkout: {}", e))?;
        return Ok(format!(
            "published {} → main (fast-forward)",
            session_branch_name
        ));
    }

    // 3-way merge: session into main.
    let session_annotated = {
        let session_ref = session_branch.get();
        repo.reference_to_annotated_commit(session_ref)
            .map_err(|e| format!("merge_session: annotated commit: {}", e))?
    };
    let mut merge_opts = MergeOptions::new();
    let mut checkout_opts = git2::build::CheckoutBuilder::new();
    checkout_opts.allow_conflicts(true);
    checkout_opts.conflict_style_diff3(false);
    checkout_opts.force();
    repo.merge(
        &[&session_annotated],
        Some(&mut merge_opts),
        Some(&mut checkout_opts),
    )
    .map_err(|e| {
        format!(
            "merge_session: libgit2 merge failed: {} (likely a conflict)",
            e
        )
    })?;

    let mut index = repo
        .index()
        .map_err(|e| format!("merge_session: could not load index: {}", e))?;
    if index.has_conflicts() {
        let conflicts = collect_conflict_paths(&index);
        let file_list = if conflicts.is_empty() {
            "(unknown)".to_string()
        } else {
            conflicts.join(", ")
        };
        // Reset main to a clean HEAD so the workdir isn't left
        // half-merged (mirrors `do_merge_blocking`'s conflict arm).
        let main_commit = match main_branch.get().peel_to_commit() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "merge_session: post-conflict peel failed; skipping reset");
                return Err(format!(
                    "merge conflict: [{}]. session '{}' and main both modified these files. \
                     Resolve manually, then publish again.",
                    file_list, session_branch_name
                ));
            }
        };
        let main_obj = main_commit.into_object();
        let mut reset_checkout = git2::build::CheckoutBuilder::new();
        reset_checkout.force();
        reset_checkout.remove_untracked(true);
        if let Err(e) = repo.reset(&main_obj, git2::ResetType::Hard, Some(&mut reset_checkout)) {
            tracing::warn!(error = %e, "merge_session: post-conflict reset failed (worktree may be half-merged)");
        }
        return Err(format!(
            "merge conflict: [{}]. session '{}' and main both modified these files. \
             Resolve manually, then publish again.",
            file_list, session_branch_name
        ));
    }

    // Clean 3-way merge → commit on main.
    let merge_oid = {
        let sig = repo
            .signature()
            .unwrap_or_else(|_| git2::Signature::now("Everlasting", "agent@everlasting").unwrap());
        let tree_oid = index
            .write_tree()
            .map_err(|e| format!("merge_session: write_tree: {}", e))?;
        let tree = repo
            .find_tree(tree_oid)
            .map_err(|e| format!("merge_session: find_tree: {}", e))?;
        let main_commit = repo
            .find_commit(main_tip.id())
            .map_err(|e| format!("merge_session: find main commit: {}", e))?;
        let session_commit = repo
            .find_commit(session_tip.id())
            .map_err(|e| format!("merge_session: find session commit: {}", e))?;
        repo.commit(
            Some(&format!("refs/heads/{}", main_branch_name)),
            &sig,
            &sig,
            &format!("merge_session: merge {} into main", session_branch_name),
            &tree,
            &[&main_commit, &session_commit],
        )
        .map_err(|e| format!("merge_session: write merge commit: {}", e))?
    };
    repo.cleanup_state()
        .map_err(|e| tracing::warn!(error = %e, "merge_session: cleanup_state failed (non-fatal)"))
        .ok();
    Ok(format!(
        "published {} → main (3-way, merge commit {})",
        session_branch_name, merge_oid
    ))
}

/// Check whether `ancestor_oid` is an ancestor of `descendant_oid`
/// in the commit graph. Used for the fast-forward detection.
fn is_ancestor(
    repo: &Repository,
    ancestor_oid: git2::Oid,
    descendant_oid: git2::Oid,
) -> Result<bool, String> {
    if ancestor_oid == descendant_oid {
        return Ok(true);
    }
    // `merge_base` returns the best common ancestor; if the
    // `ancestor_oid` IS a (strict) ancestor, `merge_base ==
    // ancestor_oid` and `descendant_oid != ancestor_oid`.
    let base = repo
        .merge_base(ancestor_oid, descendant_oid)
        .map_err(|e| format!("is_ancestor: merge_base failed: {}", e))?;
    Ok(base == ancestor_oid && ancestor_oid != descendant_oid)
}

/// Walk the index's conflict entries and return the
/// conflict file paths (deduped). Each conflict entry
/// in libgit2's index appears 3 times (ours / theirs /
/// ancestor); we dedupe to one path per file. The stage
/// bits live in the high 2 bits of `IndexEntry::flags`
/// (`GIT_INDEX_ENTRY_STAGE_MASK = 0x3000` in libgit2);
/// `flags & 0x3000 != 0` means the entry is a conflict
/// (stage 0 = normal, stages 1-3 = conflict stages).
fn collect_conflict_paths(index: &git2::Index) -> Vec<String> {
    const STAGE_MASK: u16 = 0x3000;
    let mut paths: Vec<String> = Vec::new();
    for entry in index.iter() {
        if entry.flags & STAGE_MASK != 0 {
            if let Ok(path) = std::str::from_utf8(&entry.path) {
                let path = path.to_string();
                if !paths.iter().any(|p| p == &path) {
                    paths.push(path);
                }
            }
        }
    }
    paths
}
