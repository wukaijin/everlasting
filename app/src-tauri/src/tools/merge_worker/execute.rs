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

use serde_json::json;

use crate::llm::types::ToolDef;
use crate::tools::{ToolContext, ToolContextUpdate};

use super::finalize::{ensure_parent_worktree_attached, finalize_merge};
use super::merge::do_merge_blocking;

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
    // `ctx.worktree_path` may be the project root (parent never
    // attached a worktree) OR the actual worktree path (parent
    // already Active). We don't trust it directly — Stage 2a
    // below calls `ensure_parent_worktree_attached` to normalize
    // the parent's state (lazy-attaching if needed), then reloads
    // the parent session row to capture the authoritative
    // `worktree_path` for `do_merge_blocking`.
    //
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

    // ----- Stage 2a: lazy auto-attach parent worktree -----
    // (06-30 follow-up.) The parent session may be at
    // `WorktreeState::None` (no worktree ever attached). Without
    // this guard, `do_merge_blocking` would fail downstream with
    // the opaque "parent branch '<sid>' not found" error from
    // libgit2 — see design §3.4 + prd §"Goals". The helper is
    // shared with the IPC `merge_worker_run` so both paths
    // follow the exact same tri-state contract.
    match ensure_parent_worktree_attached(&ctx.db, &ctx.data_dir, &parent_session_id).await {
        Ok(true) | Ok(false) => {
            // Ok(true)  → we just attached a fresh worktree;
            //             `ctx.worktree_path` (captured above
            //             from before this call) is now stale
            //             and must be replaced before
            //             `do_merge_blocking` opens the repo.
            // Ok(false) → no-op (parent already Active, or
            //             Detached (skipped intentionally per
            //             INV-M3)); `ctx.worktree_path` is
            //             still valid IF parent was Active,
            //             but if Detached we need a fresh
            //             load to fail with a clean error
            //             instead of an opaque libgit2 one.
            // Either way: reload the parent session row to get
            // the authoritative `worktree_path`.
        }
        Err(e) => {
            return (
                format!("merge_worker: cannot auto-attach parent worktree: {}", e),
                true,
                ToolContextUpdate::default(),
                None,
            );
        }
    }
    let reloaded_parent = match crate::db::load_session(&ctx.db, &parent_session_id).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return (
                format!(
                    "merge_worker: parent session '{}' disappeared",
                    parent_session_id
                ),
                true,
                ToolContextUpdate::default(),
                None,
            );
        }
        Err(e) => {
            return (
                format!("merge_worker: failed to reload parent session: {}", e),
                true,
                ToolContextUpdate::default(),
                None,
            );
        }
    };
    let parent_wt = match reloaded_parent.session.worktree_path.as_deref() {
        Some(p) => std::path::PathBuf::from(p),
        None => {
            // Detached parent (INV-M3): we did NOT
            // re-attach. Surface an actionable error rather
            // than the cryptic libgit2 "parent branch not
            // found" downstream. The LLM sees this and can
            // instruct the user (or the user can attach via
            // the chat header manually).
            return (
                format!(
                    "merge_worker: parent session '{}' is detached (no worktree bound); call attach_worktree first or attach via the chat header",
                    parent_session_id
                ),
                true,
                ToolContextUpdate::default(),
                None,
            );
        }
    };

    // ----- Stage 2b: do the libgit2 merge on a blocking task -----
    // The blocking task takes ownership of `parent_wt`,
    // `parent_session_id`, and `run_id` (they're `Clone`-able
    // — `PathBuf` is, `String` is). The post-merge cleanup
    // uses clones of the same values (a `String` / `PathBuf`
    // clone is cheap — `String` is a heap-backed buffer,
    // `PathBuf` is the same).
    let parent_wt_for_task = parent_wt;
    let session_id_for_task = parent_session_id.clone();
    let run_id_for_task = run_id.clone();
    let merge_result = tokio::task::spawn_blocking(move || {
        do_merge_blocking(&parent_wt_for_task, &session_id_for_task, &run_id_for_task)
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
            if let Err(e) = finalize_merge(&ctx.db, &parent_session_id, &run_id).await {
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
