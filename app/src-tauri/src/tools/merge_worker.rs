//! L3b PR3 (2026-06-27): `merge_worker` tool.
//!
//! Merges a worker's `worker/<run_id>` branch (left behind by an
//! isolated worker run that exited with changes) into the parent
//! session's `session/<id>` branch. Reuses libgit2's three-way
//! merge API (`Repository::merge`); on conflict, returns an
//! `is_error: true` tool_result with the conflict file list and
//! leaves both branches intact (the worker branch + worktree
//! stay preserved for the user to inspect / resolve manually).
//!
//! On success, calls PR1's [`crate::git::worktree::destroy_worker`]
//! to remove the worker worktree + delete the `worker/<run_id>`
//! branch + clear the `subagent_runs.worktree_path` column. The
//! fast-forward path is preferred (the typical case after a
//! general-purpose worker that wrote to its own checkout without
//! touching the parent branch).
//!
//! Why this is a **tool** (not just a Tauri command): the LLM
//! drives the call. After a worker reports it changed `a.rs` /
//! `b.rs`, the parent LLM decides to merge the changes back. The
//! tool is the LLM's seam for that decision; the dedicated Tauri
//! command (`merge_worker_run`) exists only so the frontend
//! `<SubagentDrawer>` PR4 can expose a manual button.
//!
//! ⑨ 关 routing: `Risk::High` (per `permissions::types::risk_for_tool`).
//! The Tier 4 path branch classifies it as `ToolKind::GitMutation`
//! (tool-level grant + ask, mirroring WebFetch — the `run_id` is a
//! database key, not a filesystem path, so the modal renders no
//! path-scope row). Plan mode filters it out (`filter_tools_for_mode`
//! lists `merge_worker`/`discard_worker`).
//!
//! Concurrency: per-parent-session merge serialization is enforced in
//! [`do_merge_blocking`] via [`merge_lock_for`] (a `std::sync::Mutex`
//! keyed by `parent_session_id`). Both `spawn_blocking` call sites
//! (this tool's [`execute`] + the `merge_worker_run` IPC command) flow
//! through it, so concurrent merges into the same parent branch are
//! serialized; independent sessions still merge in parallel.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use git2::{MergeOptions, Repository};
use serde_json::json;

use crate::db;
use crate::git;
use crate::llm::types::ToolDef;
use crate::tools::{ToolContext, ToolContextUpdate};

/// `merge_worker` tool definition (registered in `builtin_tools()`).
pub fn definition() -> ToolDef {
    ToolDef {
        name: "merge_worker".to_string(),
        description: Some(
            "Merge a completed worker subagent's `worker/<run_id>` branch back into the parent \
             session's branch. Use this after an isolated worker run (one that ran in its own \
             git worktree) reported leaving changes you want to keep.\n\n\
             The merge is a fast-forward or three-way merge (whichever libgit2 picks). If the \
             merge would conflict (the worker and the parent both modified the same lines of \
             the same file), the tool returns an `is_error: true` result with a list of \
             conflicting file paths; the worker branch + worktree stay intact so you can \
             inspect / resolve manually. **Do not retry the merge after a conflict** — the \
             worker branch is preserved for you to handle.\n\n\
             On a successful merge, the worker worktree + branch are destroyed automatically \
             and the `subagent_runs.worktree_path` column is cleared.\n\n\
             Errors:\n\
             - `run_id` is unknown → \"worker run not found\"\n\
             - The parent session has no worktree attached → \"parent session has no worktree\"\n\
             - The worker has no `worktree_path` set (already merged / discarded) → \
             \"worker has no worktree to merge (already merged or discarded)\"\n\
             - The parent branch cannot be opened (e.g. detached HEAD) → \"parent branch not \
             found\"\n\
             - libgit2 reports a merge conflict → returns the conflict file list, leaves \
             both branches intact."
                .to_string(),
        ),
        input_schema: json!({
            "type": "object",
            "properties": {
                "run_id": {
                    "type": "string",
                    "description": "The subagent run id (the `subagent_runs.id` UUID from the worker dispatch). \
                                    The LLM should have received this in the dispatch_subagent tool_result."
                }
            },
            "required": ["run_id"]
        }),
    }
}

/// Execute the tool. Returns `(content, is_error, update, exit_code)`.
/// No exit code (no subprocess); the agent loop's `Option<i32>` is
/// `None`. No `new_cwd` update either (the merge doesn't change
/// the session's cwd; the parent worktree's checkout is updated
/// in place by libgit2).
///
/// ⑨ 关 is enforced upstream of this function by the agent
/// loop's `permissions::check` call (Tier 2 deny / Tier 4 ask).
/// Inside the tool, we do the per-row + per-branch validation
/// and the libgit2 merge.
pub async fn execute(
    input: &serde_json::Value,
    ctx: &ToolContext,
    session_id: Option<&str>,
) -> (String, bool, ToolContextUpdate, Option<i32>) {
    let run_id = match input.get("run_id").and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => {
            return (
                "Missing required parameter: run_id".to_string(),
                true,
                ToolContextUpdate::default(),
                None,
            )
        }
    };
    // `ctx.worktree_path` is the canonical on-disk path the
    // parent session should be operating on; we use it for the
    // libgit2 `Repository::open` and to look up the merge base
    // (parent's `session/<id>` branch lives in the same `.git/`
    // as any worktree, but the worktree's HEAD is the
    // authoritative "where the merge lands" location).
    let parent_wt = ctx.worktree_path.clone();
    // We need the parent session id to look up the parent branch
    // name. The chat session is the parent of this LLM-driven
    // merge call (the worker is the immediate subagent, but the
    // chat session is the *parent* of the merge decision).
    let parent_session_id = match session_id {
        Some(s) => s.to_string(),
        None => {
            return (
                "merge_worker called without a session_id; this is a bug.".to_string(),
                true,
                ToolContextUpdate::default(),
                None,
            )
        }
    };

    // ----- Stage 1: load + validate the subagent_runs row -----
    let run_row = match crate::db::subagent_runs::get_run(&ctx.db, &run_id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                format!("worker run not found: {}", run_id),
                true,
                ToolContextUpdate::default(),
                None,
            )
        }
        Err(e) => {
            return (
                format!("merge_worker: failed to load subagent_runs row: {}", e),
                true,
                ToolContextUpdate::default(),
                None,
            )
        }
    };
    // Early check: if the run has no worktree_path set,
    // there's nothing to merge (already merged or
    // discarded). Surface the error before paying the
    // libgit2 merge cost.
    if run_row.worktree_path.is_none() {
        return (
            "worker has no worktree to merge (already merged or discarded)".to_string(),
            true,
            ToolContextUpdate::default(),
            None,
        );
    }

    // ----- Stage 2: do the libgit2 merge on a blocking task -----
    // The blocking task takes ownership of `parent_wt`,
    // `parent_session_id`, and `run_id` (they're `Clone`-able
    // — `PathBuf` is, `String` is). The post-merge cleanup
    // uses clones of the same values (a `String` / `PathBuf`
    // clone is cheap — `String` is a heap-backed buffer,
    // `PathBuf` is the same).
    let parent_wt_for_task = parent_wt.clone();
    let session_id_for_task = parent_session_id.clone();
    let run_id_for_task = run_id.clone();
    let merge_result = tokio::task::spawn_blocking(move || {
        do_merge_blocking(
            &parent_wt_for_task,
            &session_id_for_task,
            &run_id_for_task,
        )
    })
    .await
    .unwrap_or_else(|e| Err(format!("merge_worker task panicked: {}", e)));

    match merge_result {
        Ok(msg) => {
            // ----- Stage 3: post-merge cleanup (best-effort) -----
            // We do the cleanup inline here so the LLM sees a
            // consistent "merged" result regardless of any
            // cleanup hiccup. `finalize_merge` is best-effort
            // (failures log + continue).
            if let Err(e) = finalize_merge(&ctx.db, &parent_session_id, &run_id).await
            {
                tracing::warn!(
                    run_id = %run_id,
                    error = %e,
                    "merge_worker: post-merge cleanup failed (non-fatal; merge already committed)"
                );
            }
            (msg, false, ToolContextUpdate::default(), None)
        }
        Err(msg) => (msg, true, ToolContextUpdate::default(), None),
    }
}

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
/// column clear) is done in [`finalize_merge`] separately
/// because the tool layer doesn't carry a DB pool; the IPC
/// command layer (which does) calls `finalize_merge` after a
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
    let parent_tip_oid = parent_branch.get().peel_to_commit().map_err(|e| {
        format!(
            "merge_worker: could not resolve parent branch tip: {}",
            e
        )
    })?;

    let worker_branch_name = git::worktree::worker_branch_name(run_id);
    let worker_branch = repo
        .find_branch(&worker_branch_name, git2::BranchType::Local)
        .map_err(|e| {
            format!(
                "merge_worker: worker branch '{}' not found (already merged / discarded?): {}",
                worker_branch_name, e
            )
        })?;
    let worker_tip_oid = worker_branch.get().peel_to_commit().map_err(|e| {
        format!(
            "merge_worker: could not resolve worker branch tip: {}",
            e
        )
    })?;

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
            .map_err(|e| {
                format!(
                    "merge_worker: could not fast-forward parent branch: {}",
                    e
                )
            })?;
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
            .map_err(|e| {
                format!(
                    "merge_worker: post-fast-forward checkout failed: {}",
                    e
                )
            })?;
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
        repo.reference_to_annotated_commit(&worker_ref).map_err(|e| {
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
    let mut index = repo.index().map_err(|e| {
        format!(
            "merge_worker: could not load index after merge: {}",
            e
        )
    })?;
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
        if let Err(e) = repo.reset(&parent_obj, git2::ResetType::Hard, Some(&mut reset_checkout)) {
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
            &format!("merge_worker: merge {} into {}", worker_branch_name, parent_branch_name),
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
