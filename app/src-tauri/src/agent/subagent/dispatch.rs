//! B6 Subagent — worker dispatch (`run_subagent`).
//!
//! Split out of `chat_loop.rs` on 2026-06-23 so the main loop file
//! stays focused on turn orchestration. `run_subagent` is the
//! interceptor helper called from
//! [`crate::agent::chat_loop::run_chat_loop`]'s serial-path tool
//! dispatch when `name == "dispatch_subagent"`; it owns the nested
//! `run_chat_loop` call that drives the worker agent.

use std::path::PathBuf;
use std::sync::Arc;

use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::agent::chat_loop::run_chat_loop;
use crate::llm::Provider;
use crate::memory::MemoryCache;
use crate::skill::loader::SkillCache;
use crate::state::ChatEventSink;
use crate::tools::read_guard::ReadGuard;
use crate::tools::ToolContext;

use super::{
    assemble_subagent_prompt, build_subagent_finished_payload, build_worker_messages,
    filter_tools_for_subagent, filter_tools_readonly, format_dispatch_result, format_final_text,
    summarize_worker_tool_actions, truncate_transcript_for_persistence,
    SubagentBufferSink, SubagentCache, SubagentStatus, TRANSCRIPT_MAX_BYTES,
};

// ---------------------------------------------------------------------------
// B6 Subagent (2026-06-19): worker dispatch
//
// `run_subagent` is the interceptor helper called from the
// serial-path tool dispatch loop when `name == "dispatch_subagent"`.
// It owns the nested `run_chat_loop` call that drives the worker
// agent. It was extracted from `chat_loop.rs` into this file on
// 2026-06-23, but it still needs the parent loop's closure
// dependencies (`provider` / `db` / `cancellations` / ...) — the
// alternative would be to thread 22+ parameters through a public
// function, which is the same "too many parameters" cost
// `run_chat_loop` itself pays (see RULE-A-006 docstring at the top
// of `chat_loop.rs`).
//
// The function returns a `(content, is_error, cancel_parent,
// exit_code)` tuple shaped to mirror the `execute_tool` return so
// the caller's serial-path code can treat it uniformly:
//   - `content` = the dispatch_subagent tool_result's content
//     string (status prefix + worker summary).
//   - `is_error` = whether the worker exited non-successfully
//     (cancelled / errored). The caller's serial path emits the
//     tool_result with this flag set so the LLM sees the failure.
//   - `cancel_parent` = whether the worker detected a parent-
//     propagated cancel (user Stop reached the worker). When
//     `true`, the caller's serial loop flips its local `cancelled`
//     flag and drives the existing cancel path — the user's Stop
//     propagates back up through the worker to the parent.
//   - `exit_code` = always `None` (no child process spawned);
//     matches the convention for non-shell tools.
// ---------------------------------------------------------------------------

/// Worker turn budget. Bounded independently of the parent's 50-turn
/// limit so a runaway subagent cannot burn the parent's full budget
/// (PRD §Decisions 8 + review #4). The worker still re-uses C3
/// compaction, so hitting this limit on a long task degrades to
/// compaction rather than an unbounded loop.
///
/// 2026-06-21 (R1): raised from 20 → 200. The original 20-turn
/// cap was sized for the B6 PR1 demo scenarios (small focused
/// tasks). Real `trellis-implement` runs burn 200+ tool calls
/// (code search + edit + verify + RUSTFLAGS / cargo test cycles
/// + DB inspection + spec re-reads), so 20 was an artificial
/// ceiling that hard-terminated workers mid-task. The 200
/// budget is empirically large enough for the heaviest observed
/// `trellis-implement` run while still bounded enough that a
/// runaway worker cannot burn the parent session's full 50-turn
/// budget (a single worker run at 200 turns is 4× the parent
/// budget — a real cost, but acceptable given R3's token-usage
/// fix (this PR) makes the burn visible). Future cost gates
/// (token / wall-clock second-stage) are explicitly deferred.
const SUBAGENT_MAX_TURNS: usize = 200;

// ---------------------------------------------------------------------------
// L3b (2026-06-27): worktree isolation merge + helpers
// ---------------------------------------------------------------------------

/// Resolve the worker's worktree-isolation decision by merging the
/// per-agent frontmatter default with the per-dispatch override.
///
/// Truth table (matches the PRD's "已闭合" merge semantics):
///
/// | frontmatter default | dispatch `isolation` | result |
/// |---------------------|----------------------|--------|
/// | `Some(true)`        | not specified        | isolated |
/// | `Some(true)`        | `Some(false)`        | shared (LLM opted out) |
/// | `Some(false)`/`None`| `Some(true)`         | isolated (LLM opted in) |
/// | `Some(false)`/`None`| not specified        | shared (legacy behavior) |
/// | `Some(false)`/`None`| `Some(false)`        | shared |
/// | `Some(true)`        | `Some(true)`         | isolated |
///
/// Precedence: **dispatch input > frontmatter default > not isolated**.
/// The dispatch input is the LLM's per-call override (`dispatch_subagent`'s
/// `isolation` parameter); the frontmatter default is the SubagentDef's
/// `isolation` field (builtin `general-purpose` = `Some(true)`,
/// `researcher` = `None`).
pub fn resolve_isolation(
    frontmatter_default: Option<bool>,
    dispatch_input: Option<bool>,
) -> bool {
    // Dispatch input wins if present; otherwise the frontmatter
    // default; otherwise `false` (legacy shared-cwd behavior).
    dispatch_input.or(frontmatter_default).unwrap_or(false)
}

/// A summary of the worker's changes for the dispatch_subagent
/// tool_result. Built by scanning the worker worktree's diff against
/// its base commit (the `worker/<run_id>` branch tip vs its parent).
/// When non-empty, the worker's branch + worktree are PRESERVED so a
/// future PR3 `merge_worker` / `discard_worker` tool can act on them;
/// when empty, the worktree is destroyed immediately.
struct WorkerChanges {
    /// True iff the worker's worktree has any tracked or untracked
    /// changes vs its base commit.
    has_changes: bool,
    /// A short, LLM-friendly summary of the changes (file list +
    /// per-file +/- counts). Empty when `has_changes` is false.
    summary: String,
}

/// Probe the worker worktree for changes vs its base commit. Used by
/// `run_subagent` after the worker exits to decide:
/// 1. **No changes** → destroy the worktree immediately (the branch
///    carries nothing useful); clear `subagent_runs.worktree_path`.
/// 2. **Has changes** → preserve the worktree + branch; the diff
///    summary is appended to the dispatch_subagent tool_result so
///    the parent LLM knows where the worker's edits live.
///
/// Implementation: delegates to `git::diff::diff_worktree`, which
/// already handles tracked + untracked files. We pass a synthetic
/// `session_id` of `<run_id>` so the diff is computed against the
/// `worker/<run_id>` branch (NOT the project's `session/<id>`
/// branch). On any error we conservatively report "has changes"
/// (preserving the worktree is the safe fallback — destroying it
/// could lose the worker's work).
fn probe_worker_changes(
    worker_worktree_path: &std::path::Path,
    run_id: &str,
) -> WorkerChanges {
    match crate::git::diff::diff_worker_worktree(worker_worktree_path, run_id) {
        Ok(result) => {
            if result.files.is_empty() {
                WorkerChanges {
                    has_changes: false,
                    summary: String::new(),
                }
            } else {
                // Build a compact summary: file list + per-file +/-
                // counts. Cap at 10 files to keep the tool_result
                // scannable (the full diff lives on the branch).
                let mut lines: Vec<String> = Vec::new();
                for f in result.files.iter().take(10) {
                    lines.push(format!(
                        "- {} ({}, +{}/-{})",
                        f.path, f.status, f.added, f.removed
                    ));
                }
                let omitted = result.files.len().saturating_sub(10);
                if omitted > 0 {
                    lines.push(format!("... and {} more", omitted));
                }
                WorkerChanges {
                    has_changes: true,
                    summary: lines.join("\n"),
                }
            }
        }
        Err(e) => {
            tracing::warn!(
                worker_worktree = %worker_worktree_path.display(),
                run_id = %run_id,
                error = %e,
                "probe_worker_changes: diff failed; preserving worktree as conservative fallback"
            );
            // Conservative fallback: assume changes exist so we
            // don't destroy a worktree that might hold the worker's
            // edits.
            WorkerChanges {
                has_changes: true,
                summary: "(diff probe failed; changes status unknown)".to_string(),
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_subagent(
    provider: &Arc<dyn Provider>,
    context_window: u32,
    parent_rid: &str,
    parent_session_id: &str,
    memory_cache: &Arc<MemoryCache>,
    read_guard: &ReadGuard,
    skill_cache: &Arc<SkillCache>,
    permission_asks: &crate::agent::permissions::PermissionStore,
    cancellations: &Arc<Mutex<std::collections::HashMap<String, CancellationToken>>>,
    _session_active_request: &Arc<Mutex<std::collections::HashMap<String, String>>>,
    background_shells: &crate::background_shell::DefaultRegistry,
    db: &SqlitePool,
    current_ctx: &ToolContext,
    tool_use_id: &str,
    input: &serde_json::Value,
    parent_token: &CancellationToken,
    _parent_sink: &Arc<dyn ChatEventSink>,
    // B6 PR3 (2026-06-20, PR2 hotfix): the parent's Tauri
    // `AppHandle`, threaded through `run_chat_loop`'s 22nd
    // parameter. Used to construct the worker's `SubagentBufferSink`
    // so the worker can emit the `subagent:event` IPC channel
    // live. `None` in unit tests (no Tauri runtime); the sink's
    // `new_without_app_handle` constructor is used in that case.
    app_handle: Option<AppHandle>,
    // L3a (2026-06-24) / L3b PR2 (2026-06-27): when `true`, the
    // worker's toolset is additionally forced down to read-only
    // tools (`filter_tools_readonly`) on top of
    // `filter_tools_for_subagent`. **Post-PR2 this is the
    // SERIAL-ONLY path** (the concurrent dispatch branch in
    // `chat_loop.rs` no longer passes `true` — see L3b PR2). The
    // serial path (single dispatch or mixed batch) keeps passing
    // `false`, and the L3a regression
    // (`l3a_single_dispatch_runs_serial_path_unchanged`) continues
    // to pin the behavior.
    //
    // **Why kept after PR2**: the parameter is retained (instead
    // of removed) for two reasons:
    // 1. L3a test compat — the regression test
    //    `l3a_single_dispatch_runs_serial_path_unchanged` was
    //    written against the `force_readonly=true` API shape;
    //    removing it would force that test to re-thread its mock
    //    fixtures.
    // 2. Future "force read-only at the subagent level" feature
    //    (e.g. an LLM opts `general-purpose` into read-only for a
    //    single dispatch) can repurpose this param instead of
    //    adding a new one.
    //
    // The concurrent branch's race-dissolution proof (see
    // `.trellis/spec/backend/agent-loop-architecture.md`
    // §"Pattern: Concurrent isolated dispatch (L3b PR2)") no
    // longer depends on the read-only scope; per-worker worktree
    // isolation (PR1) handles the write race. The `force_readonly`
    // arg remains a SERIAL-only behavioral switch.
    force_readonly: bool,
    // L3d (2026-06-25): the process-wide subagent cache, used to
    // look up the dispatched subagent across builtin + user +
    // project layers (replaces the static `lookup_subagent(name)`
    // — `cache.lookup(project_path, name)` returns a cloned
    // `LoadedSubagent` honoring the project > user > builtin
    // precedence + Q2 tools-inheritance). Read-through + mtime-
    // fenced, so a freshly-written `.md` is picked up on the next
    // chat turn without a reload command.
    subagent_cache: &Arc<SubagentCache>,
    // L3b (2026-06-27): the app's data directory, used to compute
    // the worker worktree path (`<app_data_dir>/worktrees/
    // <project_uuid>/worker/<run_id>`). Production threads the real
    // `AppState.app_data_dir`; tests pass an empty path
    // (`Path::new("")`) since worker isolation is opted into
    // per-subagent and most integration tests dispatch `researcher`
    // (no isolation) or `general-purpose` against a non-isolating
    // fixture. A test that wants to exercise isolation passes a
    // tempdir path + sets up a real git repo.
    app_data_dir: &std::path::Path,
) -> (String, bool, bool, Option<i32>) {
    // Parse the LLM-supplied { subagent, task } arguments.
    let subagent_name = input
        .get("subagent")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let task = input.get("task").and_then(|v| v.as_str()).unwrap_or("");
    let tool_use_id_owned = tool_use_id.to_string();

    // Resolve the parent session's project_id + path so the worker
    // reads the same memory cache slots the parent uses. The
    // `project_path` is also the key the `SubagentCache` uses to
    // scope its `<project>/.everlasting/agents/` dir.
    let project_id = resolve_project_id(db, parent_session_id).await;
    let project_path = current_ctx.worktree_path.to_string_lossy().to_string();

    // L3d (2026-06-25): resolve the SubagentDef via the cache
    // (builtin + user + project merged with project > user >
    // builtin precedence). Replaces the static `lookup_subagent`.
    // Unknown name → error tool_result (keeps the
    // tool_use/tool_result pairing invariant).
    let Some(loaded) = subagent_cache.lookup(&project_path, subagent_name).await else {
        // Build a friendly "available" hint by re-listing (cheap;
        // the cache is mtime-fenced so this is a HashMap lookup
        // when nothing changed since the dispatch_def was built).
        let available: Vec<String> = subagent_cache
            .list(&project_path)
            .await
            .into_iter()
            .map(|l| l.def.name)
            .collect();
        let content = format!(
            "Unknown subagent '{}'. Available: {}.",
            subagent_name,
            if available.is_empty() {
                "(none)".to_string()
            } else {
                available.join(", ")
            }
        );
        return (content, true, false, None);
    };
    let def = &loaded.def;
    if task.trim().is_empty() {
        let content = "Missing or empty 'task' parameter. The delegation task must be a                        non-empty string."
            .to_string();
        return (content, true, false, None);
    }

    // L3b (2026-06-27): resolve the worktree-isolation decision.
    // Merge the per-agent frontmatter default (`def.isolation`) with
    // the per-dispatch `isolation` input the LLM may have supplied.
    // Precedence: dispatch input > frontmatter default > not isolated.
    // When isolated, the worker runs in its own git worktree
    // (`<app_data_dir>/worktrees/<project_uuid>/worker/<run_id>`)
    // on branch `worker/<run_id>`, based off the parent session's
    // current worktree HEAD. When not isolated, the worker reuses
    // the parent session's worktree (legacy behavior).
    //
    // **L3a backward-compat**: when `force_readonly=true` (the
    // L3a concurrent dispatch path; the only call site that ever
    // passed `true`), isolation was historically forced off — the
    // concurrent branch was scoped to read-only + shared cwd per
    // the L3a race-dissolution proof. Post-L3b PR2, the concurrent
    // branch no longer passes `true`; isolation now propagates
    // from `def.isolation` + `dispatch.isolation` even in the
    // concurrent path. The short-circuit is retained for
    // `force_readonly=true` so the L3a serial-only regression
    // (`l3a_single_dispatch_runs_serial_path_unchanged`) + any
    // future explicit read-only call site preserve the old
    // "read-only + shared cwd" semantics.
    let dispatch_isolation = input
        .get("isolation")
        .and_then(|v| v.as_bool());
    let isolated = if force_readonly {
        // L3a pre-PR2 concurrent path; also any future explicit
        // read-only force (serial). Force isolation off so the
        // read-only + shared-cwd scope is preserved.
        false
    } else {
        resolve_isolation(def.isolation, dispatch_isolation)
    };

    // The worker_run_id is the `subagent_runs.id` we'll insert below
    // (a UUID). We need it BEFORE the insert to compute the worktree
    // path (the branch name + on-disk dir are derived from it). So
    // we pre-generate the UUID here and pass it into `insert_run`'s
    // slot. This is a small departure from the existing flow (which
    // let `insert_run` generate the id), but it keeps the worktree
    // path + DB row id in lockstep.
    let worker_run_id = uuid::Uuid::new_v4().to_string();
    let worker_branch = crate::git::worktree::worker_branch_name(&worker_run_id);

    // Compute the worker worktree path + create the worktree when
    // isolated. On any failure we FAIL the dispatch (return an error
    // tool_result) — per the PRD's Edge Cases: "worktree 创建失败 →
    // fail dispatch,不降级到不隔离" (avoids silent behavior
    // inconsistency where the LLM thinks isolation is active but
    // it isn't).
    //
    // `worker_worktree_opt` carries the path (Some) when isolation
    // is active + the worktree was created successfully. It's the
    // value threaded into `run_chat_loop`'s `worktree_override`
    // parameter (Some) below, and the value written to
    // `subagent_runs.worktree_path`.
    let worker_worktree_opt: Option<PathBuf> = if isolated {
        match create_worker_worktree(
            db,
            parent_session_id,
            &project_id,
            &worker_run_id,
            app_data_dir,
            &current_ctx.worktree_path,
        )
        .await
        {
            Ok(path) => Some(path),
            Err(e) => {
                tracing::warn!(
                    parent_session_id = %parent_session_id,
                    worker_run_id = %worker_run_id,
                    error = %e,
                    "run_subagent: worker worktree creation failed; failing dispatch (no fallback to non-isolated)"
                );
                let content = format!(
                    "[status: error]\nFailed to create isolated worker worktree on branch \
                     `worker/{}`: {}. The dispatch was aborted — the worker did not run. \
                     Either retry without isolation, or resolve the underlying git error.",
                    worker_run_id, e
                );
                return (content, true, false, None);
            }
        }
    } else {
        None
    };

    // L3b (2026-06-27): when isolated, RESET the ReadGuard for the
    // worker. The worker starts in a fresh checkout with no
    // inherited "already-read" file set — if we passed the parent's
    // ReadGuard through, the worker's edit_file would pass the
    // verify_read check for files the parent read (in a DIFFERENT
    // checkout), then fail at verify_fresh (the file doesn't exist
    // in the worker's tree). A fresh empty ReadGuard forces the
    // worker to read files in its own tree before editing.
    //
    // We construct a fresh guard and swap it in for the nested
    // run_chat_loop call; the parent's guard is borrowed (`&`) and
    // untouched. The fresh guard dies with this run_subagent call
    // (no shared state to clean up — ReadGuard is per-session and
    // the worker has no session of its own).
    let worker_read_guard: ReadGuard = if isolated {
        ReadGuard::new()
    } else {
        // Non-isolated: clone the parent's guard (legacy behavior).
        // The clone is cheap (Arc inside).
        read_guard.clone()
    };
    let worker_read_guard_ref = &worker_read_guard;

    // Build the worker's toolset (allowlist + structural-disabled
    // strip). The worker's run_chat_loop call gets this filtered
    // Vec; the parent's tool_defs is unaffected.
    //
    // L3d (2026-06-25): we clone the resolved `def` (the cache
    // returns an owned `LoadedSubagent`) so the worker's filter
    // can consume it. `filter_tools_for_subagent` takes `&SubagentDef`
    // so we just borrow.
    let worker_tool_defs = filter_tools_for_subagent(crate::tools::builtin_tools(), def);
    // L3a (2026-06-24): concurrent dispatch branch forces the
    // worker's toolset down to read-only tools. The serial path
    // passes `force_readonly = false` so `general-purpose` in
    // the serial path keeps its full write/shell/web toolset
    // (gated by `is_worker: true` at the ⑨ permission layer).
    // For `researcher` this is a no-op (its allowlist is already
    // exactly the 4 read-only tools).
    let worker_tool_defs = if force_readonly {
        filter_tools_readonly(worker_tool_defs)
    } else {
        worker_tool_defs
    };

    // Build the worker's messages: [memory_blocks (cache_control),
    // delegation_task]. The task is APPENDed (prompt-cache invariant
    // — see PRD §Decisions 6 + research §10.5). `project_id` +
    // `project_path` were resolved above (before the cache lookup,
    // since the cache scopes its `<project>/.everlasting/agents/`
    // dir by `project_path`).
    let worker_messages =
        build_worker_messages(memory_cache, &project_id, &project_path, task).await;

    // Assemble the worker's system prompt — fully replaces the
    // parent's behavior_prompt + mode_prefix + base_prompt layers.
    // The assembled prompt is threaded as the 23rd
    // `system_prompt_override` argument to the nested
    // `run_chat_loop` call below (was previously dead code
    // discarded at this site; see `docs/review/b6-subagent-assessment.md`
    // §2 + the doc comment on `run_chat_loop.system_prompt_override`).

    // Worker rid + token. The rid is registered into `cancellations`
    // (so user Stop propagates from the parent via the shared map)
    // but NOT into `session_active_request` — that map is
    // session→request 1:1 and a worker entry would evict the
    // parent's mapping, corrupting
    // `cancel_inflight_for_session` / RULE-E-005. The
    // CancellationGuard inside run_chat_loop is constructed with
    // `skip_session_active: true` for the worker path so its Drop
    // does NOT remove the parent's session_active_request entry.
    //
    // The rid suffix uses the tool_use_id so a future PR2
    // transcript row can correlate back to the parent's
    // dispatch_subagent tool_use.
    let worker_rid = format!("{}-sub-{}", parent_rid, tool_use_id_owned);
    let worker_token = parent_token.child_token();
    {
        let mut map = cancellations.lock().await;
        map.insert(worker_rid.clone(), worker_token.clone());
    }

    // B6 PR2: insert the worker's `running` row into
    // `subagent_runs` BEFORE the nested `run_chat_loop` call. The
    // returned id is the `worker_run_id` that the
    // `update_run_finished` call (after the worker returns)
    // targets. The insert is best-effort: a DB failure logs at
    // `warn!` and the worker still runs (the user's dispatch
    // experience is not gated on the audit row). A failed insert
    // leaves `worker_run_id_opt = None`; the post-loop
    // `update_run_finished` is then a no-op.
    //
    // L3b (2026-06-27): we pass the pre-generated `worker_run_id`
    // (computed above so the worktree path + branch name could be
    // derived from it BEFORE the insert). On success, the DB row's
    // id matches the worktree's branch name; on failure, the
    // worktree (if created) is orphaned and the post-loop cleanup
    // handles destruction via the `worker_worktree_opt` local
    // (independent of the DB row's existence).
    let worker_run_id_opt: Option<String> =
        match crate::db::subagent_runs::insert_run_with_id(
            db,
            &worker_run_id,
            parent_session_id,
            &worker_rid,
            subagent_name,
            Some(task),
        )
        .await
        {
            Ok(()) => Some(worker_run_id.clone()),
            Err(e) => {
                tracing::warn!(
                    parent_session_id = %parent_session_id,
                    worker_rid = %worker_rid,
                    error = %e,
                    "run_subagent: failed to insert subagent_runs row (non-fatal; worker still runs)"
                );
                None
            }
        };

    // L3b (2026-06-27): if isolation is active + the DB row was
    // inserted, record the worktree path on the row. Best-effort
    // (warn+continue on failure — the path is a forward-compat
    // breadcrumb for PR3's merge/discard tool).
    if let (Some(_), Some(ref wt_path)) = (&worker_run_id_opt, &worker_worktree_opt) {
        if let Err(e) = crate::db::subagent_runs::set_worktree_path(
            db,
            &worker_run_id,
            Some(&wt_path.to_string_lossy()),
        )
        .await
        {
            tracing::warn!(
                worker_run_id = %worker_run_id,
                error = %e,
                "run_subagent: failed to record worker worktree_path (non-fatal)"
            );
        }
    }

    // B6 PR2b (RULE-A-014, 2026-06-20): the worker's
    // `PermissionContext.is_worker` override is now threaded via
    // the 21st `is_worker: Option<bool>` parameter on the nested
    // `run_chat_loop` call below (passes `Some(true)`). The
    // pre-PR2b local `_worker_permission_ctx` constructed here
    // (PR1b) was dead code: `run_chat_loop` rebuilds its own
    // `PermissionContext` internally from the session row, so the
    // local value was never consulted on the worker path. PR2b
    // removes the local construction and the parallel comment
    // that documented the (now-resolved) deviation.

    // SubagentBufferSink: records the worker's emits into an in-
    // memory transcript AND (PR2 hotfix) emits each event on the
    // `subagent:event` Tauri IPC channel so the frontend
    // `<SubagentDrawer>` (PR3b) can stream the transcript live.
    // Does NOT forward to the parent sink — the parent's frontend
    // only sees the dispatch_subagent tool_call / tool_result
    // pair; the worker's stream stays isolated (Claude Code
    // convention). The `app_handle` is `Some` in production (the
    // `chat` Tauri command threads it through) and `None` in
    // unit tests (no Tauri runtime) — the IPC emit becomes a
    // silent no-op in the latter case, but the transcript
    // accumulation path is unaffected.
    //
    // We need TWO clones of `app_handle`: one for the sink (which
    // emits on the IPC channel) and one for the nested
    // `run_chat_loop` call (which the worker threads forward to
    // ITS OWN nested run_subagent call, if the worker itself
    // dispatches a sub-subagent — out of scope in MVP, but the
    // signature carries the parameter through anyway). The
    // double-clone is cheap (AppHandle is `Arc<Mutex<...>>` under
    // the hood).
    // Bug1 fix (2026-06-21): the sink's `run_id` becomes the
    // `subagent:event` payload's `runId`, which the frontend store
    // uses as the key for `liveTranscript` / `getRunCache`. It MUST
    // equal `summary.id` (= the DB row id `worker_run_id`), NOT the
    // human-readable `worker_rid` — otherwise the drawer opens with
    // `openRunId = summary.id` but the transcript cache is keyed by
    // `worker_rid`, so the drawer renders blank + stuck-on-running.
    // `worker_run_id_opt` is `None` only when `insert_run` failed
    // (no DB row → no summary → drawer can't open), so the
    // `worker_rid` fallback is unreachable in practice but keeps
    // the sink construction total.
    let event_run_id = worker_run_id_opt
        .clone()
        .unwrap_or_else(|| worker_rid.clone());
    let worker_sink: Arc<SubagentBufferSink> = match app_handle.as_ref() {
        Some(handle) => Arc::new(SubagentBufferSink::new(
            handle.clone(),
            event_run_id.clone(),
            parent_session_id.to_string(),
        )),
        None => Arc::new(SubagentBufferSink::new_without_app_handle(
            event_run_id.clone(),
            parent_session_id.to_string(),
        )),
    };
    let worker_sink_dyn: Arc<dyn ChatEventSink> = worker_sink.clone();

    // Nested run_chat_loop. The worker reuses the parent's
    // session_id for DB linkage (its turns land in the same
    // `messages` table), but:
    //   - `skip_session_active: true` so the guard's Drop does NOT
    //     evict the parent's session_active_request entry.
    //   - `max_turns: Some(SUBAGENT_MAX_TURNS)` to bound the worker's
    //     turn budget.
    //   - The worker_token is the parent_token's child, so a user
    //     Stop that reaches the parent also fires the worker
    //     (cancel propagation).
    //
    // Boxed: `run_subagent` → `run_chat_loop` → `run_subagent`
    // (worker dispatches its own subagent? No — workers have
    // `dispatch_subagent` stripped from their tools, so the
    // recursion is bounded at depth 1). Still, the async-fn
    // recursion is statically unbounded (the compiler cannot prove
    // the depth-1 invariant), so `Box::pin` breaks the size-
    // infinite Future chain. The cost is one heap allocation per
    // worker dispatch — negligible relative to the LLM round-trip.
    //
    // 2026-06-26 (task 06-26-subagent-per-run-grant): construct a
    // fresh per-run grant cache for THIS worker. The Arc dies with
    // this `run_chat_loop` call (no shared state across workers,
    // no leakage to the parent session). L3a concurrent dispatch
    // → each worker's `run_subagent` constructs its own Arc →
    // isolated caches (a grant on one worker's `cargo` does not
    // authorize another worker's `cargo`).
    let run_grants = std::sync::Arc::new(crate::agent::permissions::run_grant::RunGrantCache::new());
    Box::pin(run_chat_loop(
        worker_tool_defs,
        provider.clone(),
        context_window,
        worker_rid.clone(),
        parent_session_id.to_string(),
        worker_messages,
        worker_sink_dyn,
        db.clone(),
        cancellations.clone(),
        _session_active_request.clone(),
        // L3b (2026-06-27): pass the (possibly reset) worker
        // ReadGuard. When isolated, this is a fresh empty guard
        // (the worker starts in a new checkout with no inherited
        // reads); when not isolated, it's a clone of the parent's
        // guard (legacy behavior).
        worker_read_guard_ref.clone(),
        memory_cache.clone(),
        skill_cache.clone(),
        permission_asks.clone(),
        worker_token,
        None,
        background_shells.clone(),
        Some(SUBAGENT_MAX_TURNS),
        // B6 PR1b review #2: worker path — skip_session_active = true
        // so the worker's guard Drop does not evict the parent's
        // session_active_request[parent_session_id] entry.
        true,
        // B6 PR1b: worker path — skip_persist = true so the worker's
        // intermediate turns stay in-memory only. The
        // SubagentBufferSink captures them; PR2 will persist the
        // transcript into `subagent_runs`. Without this, the worker
        // would race the parent's persist_turn calls on the same
        // `(session_id, seq)` key (UNIQUE collision).
        true,
        // B6 PR2b (RULE-A-014, 2026-06-20): worker path — is_worker
        // = Some(true) so the nested run_chat_loop builds a
        // PermissionContext with is_worker: true. Pre-2026-06-22
        // (RULE-FrontSubagent-003) this collapsed Tier 4
        // ask_path / ask_shell to Decision::Deny (the worker had no
        // UI sink — a permission:ask would hang forever on the
        // oneshot); since 2026-06-22 worker asks route through the
        // `WorkerAskBanner` round-trip (see permission-layer.md §5b
        // — biased select over parent cancel / 120s timeout /
        // oneshot response). The `is_worker` flag now mainly scopes
        // the ask's internal session key (`"worker:{run_id}"`) and
        // stops a worker `AllowAlways` from persisting into the
        // parent's `session_tool_permissions` (cross-privilege
        // boundary). Pre-PR2b the worker path constructed
        // `_worker_permission_ctx` here but never threaded it into
        // the nested call, so the override was unreachable on the
        // worker's actual permission checks.
        Some(true),
        // B6 PR3 (2026-06-20, PR2 hotfix): forward the parent's
        // AppHandle so the worker's SubagentBufferSink can emit the
        // `subagent:event` IPC channel live. None in tests. Cloned
        // (not moved) so the post-loop `subagent:finished` emit
        // below can still borrow `app_handle` — AppHandle is an
        // `Arc` under the hood so the clone is cheap.
        app_handle.clone(),
        // 2026-06-21 fix (B6 review defect A): thread the
        // worker's `SubagentDef.system_prompt` (built via
        // `assemble_subagent_prompt` above) as the 23rd
        // `system_prompt_override` parameter. When `Some`, the
        // nested `run_chat_loop` uses this string directly and
        // skips the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` step. Pre-fix the worker was getting the
        // parent's system prompt (the worker's own prompt was
        // dead code), causing prompt / permission contradictions
        // (worker told "you can write" but `is_worker=true` made
        // Tier 4 ask_path collapse to Deny pre-2026-06-22).
        Some(assemble_subagent_prompt(def, task)),
        // 2026-06-22 (RULE-FrontSubagent-003 fix): thread the
        // worker's `subagent_runs.id` (DB row UUID) so the
        // nested run_chat_loop can build the worker-owned
        // permission session id and propagate `worker_run_id`
        // into `PermissionAskPayload.worker_run_id`. `None` when
        // `insert_run` failed (no DB row → no drawer can open →
        // worker ask interactive would have nothing to route to;
        // the ask_path worker branch will fall back to a logging
        // sentinel via the unwrap_or_else in permissions::ask_path,
        // but the practical case is "spawn failed" — the parent
        // gets an Error tool_result anyway).
        worker_run_id_opt.clone(),
        // L3d (2026-06-25): thread the subagent cache so the
        // worker's own per-turn tool list construction can append
        // the dynamic `dispatch_subagent` ToolDef (the worker's
        // `filter_tools_for_subagent` then strips it via
        // `STRUCTURALLY_DISABLED`, preventing nesting). Also
        // powers any future sub-subagent dispatch (also structurally
        // disabled in MVP). The cache is shared (Arc clone), so the
        // worker sees the same mtime-fenced view as the parent.
        subagent_cache.clone(),
        // 2026-06-26 (task 06-26-subagent-per-run-grant): per-run
        // grant cache for this worker. `Some(Arc<...>)` threads
        // the cache into the worker's `PermissionContext.run_grants`
        // so `check.rs` Tier 4 can consult it before falling through
        // to `ask_path`, and the worker's `AllowAlways` arm in
        // `ask_path` can write to it. Dies with this `run_chat_loop`
        // call — no persistence to `session_tool_permissions`.
        Some(run_grants),
        // L3b (2026-06-27): the worker's isolated worktree path.
        // When `Some(path)`, the nested `run_chat_loop` uses `path`
        // as the worker's worktree root (redirecting the worker's
        // tools into the isolated checkout) INSTEAD of the parent
        // session's worktree_path. When `None`, the loop builds the
        // worktree_path from the session row (legacy shared-cwd
        // behavior).
        worker_worktree_opt.clone(),
        // L3b (2026-06-27): thread the app_data_dir so the worker's
        // own (structurally-disabled) dispatch_subagent interceptor
        // would have it — in practice the worker never dispatches
        // a sub-subagent (STRUCTURALLY_DISABLED), so this is purely
        // for signature uniformity. We pass the same path the parent
        // passed us.
        app_data_dir.to_path_buf(),
    ))
    .await;

    // Drain the worker's accumulated state.
    //
    // 2026-06-21 (R2): the status picker now distinguishes
    // `max_turns` (soft-terminal, worker burned its 200-turn
    // budget without cleanly finishing) from `end_turn` /
    // `tool_use` (clean completion). The `was_incomplete` flag
    // is set by the sink's `Done{max_turns}` arm; the
    // `was_cancelled` flag is set by the `Done{cancelled}` arm;
    // `had_error` is set by the `Error` arm. The three are
    // mutually exclusive in practice (the agent loop's max_turns
    // branch is reached only when no cancel or error fired).
    let worker_text = worker_sink.final_text();
    let status = if worker_sink.was_cancelled() {
        SubagentStatus::Cancelled
    } else if worker_sink.had_error() {
        SubagentStatus::Error
    } else if worker_sink.was_incomplete() {
        // 2026-06-21 (R2): the `max_turns` soft-terminal path
        // is its own status (NOT `Completed` — the worker did
        // not cleanly finish). The DB `incomplete` row is the
        // signal for "useful partial output, did not exhaust
        // the task"; the `[status: incomplete]\n<partial>\n
        // [INCOMPLETE_MARKER]` wire shape makes it visible in
        // the parent's tool_result.
        SubagentStatus::Incomplete
    } else {
        SubagentStatus::Completed
    };

    // B6 PR2: persist the worker run to `subagent_runs`. The flow:
    // 1. Snapshot the transcript from the sink.
    // 2. Apply the 4 MiB cap (returns the head+tail truncated
    //    vector + a `truncated` flag).
    // 3. Build the terminal `TokenUsage` (sum of per-turn usage
    //    the sink accumulated from `ChatEvent::Done { usage }`
    //    events).
    // 4. UPDATE the `running` row to the terminal state.
    //
    // **Worker token isolation** (2026-06-26 reversal of
    // RULE-A-015/PR2a): the parent's `sessions.last_*` snapshot
    // is NOT updated by the worker. `update_last_turn_usage` is
    // back inside the `!skip_persist` gate at `chat_loop.rs`, so
    // worker turns (which run with `skip_persist=true`) don't
    // touch the parent's snapshot. The sink's per-turn
    // accumulator is the ONLY path by which the worker's
    // `TokenUsage` reaches disk — `cumulative_usage()` produces
    // the worker-run-level total for `token_usage_json`, written
    // here. Worker token usage is visible to the parent only via
    // `<SubagentDrawer>`.
    //
    // The UPDATE is best-effort: a DB failure logs at `warn!` and
    // continues (the dispatch_subagent tool_result is the
    // user-visible artifact; the DB row is for PR3's expand UI and
    // audit reads). Failing the dispatch on a DB error would mask
    // the worker's actual outcome and could re-fire the
    // tool_use/tool_result mismatch (RULE-A-007 invariant).
    if let Some(worker_run_id) = worker_run_id_opt.as_ref() {
        let transcript_snapshot = worker_sink.transcript_snapshot();
        let (truncated_transcript, transcript_truncated) = truncate_transcript_for_persistence(
            transcript_snapshot,
            TRANSCRIPT_MAX_BYTES,
        );
        let cumulative_usage = worker_sink.cumulative_usage();
        let finished_at = chrono::Utc::now().to_rfc3339();
        let status_db = match status {
            SubagentStatus::Completed => crate::db::subagent_runs::SubagentStatusDb::Completed,
            SubagentStatus::Cancelled => crate::db::subagent_runs::SubagentStatusDb::Cancelled,
            SubagentStatus::Error => crate::db::subagent_runs::SubagentStatusDb::Error,
            // 2026-06-21 (R2): max_turns soft-terminal. The DB
            // CHECK constraint was widened to include
            // `'incomplete'` by the
            // `widen_subagent_runs_status_check_for_incomplete`
            // migration; the `Incomplete` variant was added to
            // both `agent::subagent::SubagentStatus` and
            // `db::subagent_runs::SubagentStatusDb` in lockstep.
            SubagentStatus::Incomplete => crate::db::subagent_runs::SubagentStatusDb::Incomplete,
        };
        match crate::db::subagent_runs::update_run_finished(
            db,
            worker_run_id,
            status_db,
            &finished_at,
            &worker_text,
            // B6 redesign PR1 (2026-06-21): the prefix-stripped
            // final text that the drawer renders in its Reply
            // segment. `summary` carries the same string for
            // backward compat (the legacy wire field); `final_text`
            // is the new consumer-facing field. Both land in
            // distinct DB columns so legacy `summary` consumers
            // (e.g. PR3 list-view summaries) keep working unchanged.
            &format_final_text(status, &worker_text),
            &cumulative_usage,
            &truncated_transcript,
            transcript_truncated,
            // 2026-06-22 (RULE-FrontSubagent-004): thread the actual
            // completed turn count so the drawer's `statusDisplay`
            // can render "stopped at turn N" / "incomplete at turn N"
            // for the cancelled / incomplete terminal states.
            // Completed runs also carry the count (harmless; the
            // drawer only reads it for cancelled + incomplete).
            // The counter is the sink's REAL per-turn Done count
            // (synthetic cancelled / max_turns terminals don't
            // increment — see `SubagentBufferSink::turns_completed`).
            Some(worker_sink.turns_completed() as i64),
        )
        .await
        {
            Ok(()) => {
                // Bug2 fix (2026-06-21): emit a one-shot
                // `subagent:finished` terminal signal so the frontend
                // `<SubagentDrawer>` / `<ToolCallCard>` flip from
                // `running` to the terminal state without polling.
                // The frontend listener refetches `get_subagent_run`
                // (drawer: status + finishedAt + full transcript)
                // and `list_subagent_runs_by_session` (card: status).
                // Emitted only on the Ok arm — a DB failure leaves
                // the row `running`, so emitting here would cache a
                // stale `running` row as terminal. Best-effort: a
                // Tauri emit failure is non-fatal (the dispatch
                // tool_result is the user-visible terminal signal).
                if let Some(handle) = app_handle.as_ref() {
                    let payload = build_subagent_finished_payload(
                        worker_run_id,
                        parent_session_id,
                        status_db.as_str(),
                        &finished_at,
                    );
                    if let Err(e) = handle.emit("subagent:finished", payload) {
                        tracing::warn!(
                            worker_run_id = %worker_run_id,
                            error = %e,
                            "subagent:finished emit failed (non-fatal; DB row already terminal)"
                        );
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    worker_run_id = %worker_run_id,
                    error = %e,
                    "run_subagent: failed to persist subagent_runs update (non-fatal)"
                );
            }
        }
    }

    // Detect parent-driven cancel: the parent token fired while the
    // worker was running. The worker's own cancel_done event may
    // NOT have fired if the cancel arrived after the worker loop
    // already returned (e.g. worker finished turn 1 cleanly, then
    // parent cancel propagated before turn 2's select! polled).
    // The child_token relationship makes the worker_token fire when
    // the parent fires; check parent_token directly so the caller's
    // serial loop flips its `cancelled` flag and drives the existing
    // cancel path (matches the user's Stop intent).
    let cancel_parent =
        parent_token.is_cancelled() && status == SubagentStatus::Cancelled;

    // RULE-BackSubagent-001 (PR2): for non-completed terminal states,
    // summarize the worker's executed tool_calls so the parent LLM can
    // do compensatory repair (skip already-landed writes, retry failed
    // tools). Completed gets `None`; an empty summary (worker executed
    // no tool_calls before exiting) also gets `None` so no empty
    // "Worker partial actions:" header lands in the tool_result.
    let partial_actions = if matches!(status, SubagentStatus::Completed) {
        None
    } else {
        let summary = summarize_worker_tool_actions(
            &worker_sink.transcript_snapshot(),
        );
        if summary.is_empty() {
            None
        } else {
            Some(summary)
        }
    };

    // L3b (2026-06-27): worktree change-detection + lifecycle.
    //
    // When the worker ran in an isolated worktree (`worker_worktree_opt`
    // is Some), we probe the worktree for changes vs its base commit
    // after the worker exits:
    //   - **No changes** → destroy the worktree immediately (the
    //     branch carries nothing useful). Clear `subagent_runs.worktree_path`.
    //   - **Has changes** → preserve the worktree + branch; the diff
    //     summary is appended to the dispatch_subagent tool_result
    //     (below) so the parent LLM knows where the worker's edits
    //     live ("changes left on branch worker/<run_id>"). A future
    //     PR3 `merge_worker` / `discard_worker` tool acts on the
    //     preserved branch.
    //
    // The change-detection + destroy/preserve decision happens
    // REGARDLESS of terminal status (completed / cancelled / error /
    // incomplete) — per the PRD's Edge Cases: "worker 取消 → 按正常
    // 完成处理 (有 changes 保留 branch, 无 destroy)". A cancelled
    // worker that landed partial writes still has useful artifacts
    // worth preserving for inspection.
    let mut worker_changes_summary: Option<String> = None;
    if let Some(wt_path) = worker_worktree_opt.as_ref() {
        let changes = probe_worker_changes(wt_path, &worker_run_id);
        if changes.has_changes {
            // Preserve the worktree + branch. The DB row's
            // `worktree_path` column was already set to `wt_path`
            // above (after `insert_run`); leave it as-is.
            worker_changes_summary = Some(format!(
                "Worker changes left on branch `{}` (worktree at `{}`). \
                 Use `git diff` in that worktree to review, or merge/discard \
                 via a future tool.\n\n{}",
                worker_branch,
                wt_path.display(),
                changes.summary
            ));
        } else {
            // No changes — destroy the worktree + branch. Best-effort
            // (a destroy failure leaves a stale worktree; a future
            // sweep would clean it up — out of scope for PR1).
            let project_main_path = resolve_project_main_path(db, parent_session_id).await;
            if !project_main_path.is_empty() {
                if let Err(e) = crate::git::worktree::destroy_worker(
                    std::path::Path::new(&project_main_path),
                    wt_path,
                    &worker_run_id,
                ) {
                    tracing::warn!(
                        worker_run_id = %worker_run_id,
                        worktree = %wt_path.display(),
                        error = %e,
                        "run_subagent: destroy_worker failed on no-changes exit (non-fatal; stale worktree left behind)"
                    );
                }
            }
            // Clear the DB column (best-effort).
            if worker_run_id_opt.is_some() {
                if let Err(e) = crate::db::subagent_runs::set_worktree_path(
                    db,
                    &worker_run_id,
                    None,
                )
                .await
                {
                    tracing::warn!(
                        worker_run_id = %worker_run_id,
                        error = %e,
                        "run_subagent: failed to clear worktree_path after destroy (non-fatal)"
                    );
                }
            }
        }
    }

    let (content, is_error) =
        format_dispatch_result(status, &worker_text, partial_actions.as_deref());

    // L3b (2026-06-27): append the worker-changes summary to the
    // tool_result content when the worker left changes on its branch.
    // The summary tells the parent LLM where to find the worker's
    // edits (branch name + worktree path + diff file list). We
    // append AFTER `format_dispatch_result` so the existing
    // `[status: ...]` prefix + partial-actions section stay
    // unchanged; the changes summary is a new trailing section.
    let content = if let Some(summary) = worker_changes_summary {
        format!("{}\n\n{}", content, summary)
    } else {
        content
    };
    (content, is_error, cancel_parent, None)
}

/// Resolve the project_id for a session. Best-effort DB lookup of
/// `sessions.project_id` — the worker's memory loader needs the
/// project_id to slot into the right MemoryCache entry.
async fn resolve_project_id(db: &SqlitePool, session_id: &str) -> String {
    match crate::db::load_session(db, session_id).await {
        Ok(Some(loaded)) => loaded.session.project_id,
        _ => {
            tracing::warn!(
                session_id = %session_id,
                "run_subagent: failed to load session for project_id; falling back to empty"
            );
            String::new()
        }
    }
}

/// Resolve the project's MAIN repo path (the directory containing
/// `.git/`) for a session. L3b (2026-06-27): used by
/// `create_worker` / `destroy_worker` which need the main repo to
/// open libgit2 + manage linked worktrees.
///
/// This is distinct from `current_ctx.worktree_path` (which is the
/// PARENT SESSION's worktree — a linked worktree, NOT the main
/// repo). The project row's `path` field is the main repo path.
async fn resolve_project_main_path(db: &SqlitePool, session_id: &str) -> String {
    let project_id = resolve_project_id(db, session_id).await;
    if project_id.is_empty() {
        return String::new();
    }
    match crate::db::get_project(db, &project_id).await {
        Ok(Some(p)) => p.path,
        _ => {
            tracing::warn!(
                session_id = %session_id,
                project_id = %project_id,
                "run_subagent: failed to load project for main path; falling back to empty"
            );
            String::new()
        }
    }
}

/// L3b (2026-06-27): create the worker's isolated git worktree.
/// Returns the on-disk path on success.
///
/// Resolves:
/// 1. The project's main repo path (`.git/` lives here) — needed
///    for `git::worktree::create_worker`'s libgit2 open.
/// 2. The worker worktree path (`<app_data_dir>/worktrees/
///    <project_uuid>/worker/<run_id>`).
/// 3. The base worktree (the parent session's worktree) — the
///    worker's branch is based off this worktree's HEAD commit.
///
/// On ANY error we return `Err` — the caller (`run_subagent`) fails
/// the dispatch (no fallback to non-isolated, per the PRD's Edge
/// Cases). Errors include: project not found, project main path
/// not a git repo, worktree creation libgit2 failure.
async fn create_worker_worktree(
    db: &SqlitePool,
    parent_session_id: &str,
    project_id: &str,
    worker_run_id: &str,
    app_data_dir: &std::path::Path,
    parent_worktree_path: &std::path::Path,
) -> Result<PathBuf, String> {
    // 1. Resolve the project's main repo path.
    let project_main_path = resolve_project_main_path(db, parent_session_id).await;
    if project_main_path.is_empty() {
        return Err(
            "could not resolve the project's main repo path for the session".to_string(),
        );
    }
    let project_main = std::path::Path::new(&project_main_path);
    if !project_main.join(".git").exists() {
        return Err(format!(
            "project main path '{}' is not a git repository (no .git found)",
            project_main.display()
        ));
    }

    // 2. Compute the worker worktree path.
    let worker_wt_path = crate::git::worktree::worker_worktree_path(
        app_data_dir,
        project_id,
        worker_run_id,
    );

    // 3. Create the worktree. `create_worker` self-heals any stale
    //    state for this run_id (orphan dir / stale branch / stale
    //    metadata), then creates branch `worker/<run_id>` off the
    //    parent session's worktree HEAD + checks out the worktree.
    crate::git::worktree::create_worker(
        project_main,
        &worker_wt_path,
        parent_worktree_path,
        worker_run_id,
    )
    .map_err(|e| e.to_string())?;

    Ok(worker_wt_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests_common::{commit_all_for_test, init_repo_for_test};

    // -----------------------------------------------------------------------
    // resolve_isolation truth table (PRD §"已闭合" merge semantics)
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_isolation_frontmatter_true_no_override_isolates() {
        // frontmatter `isolation: worktree` + dispatch omits → isolated.
        assert_eq!(resolve_isolation(Some(true), None), true);
    }

    #[test]
    fn resolve_isolation_frontmatter_true_dispatch_false_opts_out() {
        // frontmatter `isolation: worktree` + dispatch `isolation: false`
        // → NOT isolated (LLM opted out).
        assert_eq!(resolve_isolation(Some(true), Some(false)), false);
    }

    #[test]
    fn resolve_isolation_frontmatter_none_dispatch_true_opts_in() {
        // frontmatter not declared + dispatch `isolation: true`
        // → isolated (LLM opted in).
        assert_eq!(resolve_isolation(None, Some(true)), true);
    }

    #[test]
    fn resolve_isolation_frontmatter_false_dispatch_false_stays_shared() {
        // frontmatter `isolation: false` + dispatch `isolation: false`
        // → NOT isolated.
        assert_eq!(resolve_isolation(Some(false), Some(false)), false);
    }

    #[test]
    fn resolve_isolation_no_default_no_override_is_legacy_shared() {
        // frontmatter not declared + dispatch omits → NOT isolated
        // (legacy shared-cwd behavior — the researcher builtin path).
        assert_eq!(resolve_isolation(None, None), false);
    }

    #[test]
    fn resolve_isolation_dispatch_input_wins_over_frontmatter() {
        // Dispatch input always wins (precedence rule).
        assert_eq!(resolve_isolation(Some(false), Some(true)), true);
        assert_eq!(resolve_isolation(Some(true), Some(false)), false);
    }

    // -----------------------------------------------------------------------
    // builtin SubagentDef isolation defaults
    // -----------------------------------------------------------------------

    #[test]
    fn builtin_general_purpose_defaults_to_isolated() {
        // The general-purpose builtin ships with isolation = Some(true)
        // (write-capable workers benefit most from worktree isolation).
        let g = super::super::lookup_subagent("general-purpose")
            .expect("general-purpose exists");
        assert_eq!(g.isolation, Some(true));
    }

    #[test]
    fn builtin_researcher_defaults_to_no_isolation() {
        // The researcher builtin ships with isolation = None (read-only
        // workers don't need a separate checkout — saves the per-
        // dispatch checkout cost).
        let r = super::super::lookup_subagent("researcher")
            .expect("researcher exists");
        assert_eq!(r.isolation, None);
    }

    // -----------------------------------------------------------------------
    // probe_worker_changes
    // -----------------------------------------------------------------------

    #[test]
    fn probe_worker_changes_empty_repo_reports_no_changes() {
        // A fresh worktree with no edits vs its base commit → no changes.
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        init_repo_for_test(project);
        // Seed an empty-repo-friendly initial commit so the worker
        // worktree has a base commit to branch from (create_worker
        // resolves `base_worktree_path`'s HEAD).
        std::fs::write(project.join("seed.txt"), "seed").unwrap();
        commit_all_for_test(project, "init");

        // Create a worker worktree off the project HEAD.
        let run_id = "probe-empty";
        let worker_wt = project.join("worker_empty");
        crate::git::worktree::create_worker(project, &worker_wt, project, run_id)
            .expect("create_worker should succeed");

        let changes = probe_worker_changes(&worker_wt, run_id);
        assert!(!changes.has_changes, "empty worktree should have no changes");
        assert!(changes.summary.is_empty());
    }

    #[test]
    fn probe_worker_changes_with_edits_reports_changes() {
        // A worker worktree with an edited file → reports changes.
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        init_repo_for_test(project);
        // Seed a tracked file so the worker can modify it.
        std::fs::write(project.join("a.txt"), "v1").unwrap();
        commit_all_for_test(project, "init");

        let run_id = "probe-edits";
        let worker_wt = project.join("worker_edits");
        crate::git::worktree::create_worker(project, &worker_wt, project, run_id)
            .expect("create_worker should succeed");

        // Edit the tracked file in the worker's checkout.
        std::fs::write(worker_wt.join("a.txt"), "v2-from-worker").unwrap();

        let changes = probe_worker_changes(&worker_wt, run_id);
        assert!(changes.has_changes, "edited worktree should report changes");
        assert!(
            changes.summary.contains("a.txt"),
            "summary should mention the changed file: {}",
            changes.summary
        );
    }

    #[test]
    fn probe_worker_changes_with_untracked_file_reports_changes() {
        // A worker worktree that added a new (untracked) file → reports changes.
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        init_repo_for_test(project);
        // Seed initial commit so create_worker has a base commit.
        std::fs::write(project.join("seed.txt"), "seed").unwrap();
        commit_all_for_test(project, "init");

        let run_id = "probe-untracked";
        let worker_wt = project.join("worker_untracked");
        crate::git::worktree::create_worker(project, &worker_wt, project, run_id)
            .expect("create_worker should succeed");

        // Add an untracked file in the worker's checkout.
        std::fs::write(worker_wt.join("new_file.txt"), "fresh").unwrap();

        let changes = probe_worker_changes(&worker_wt, run_id);
        assert!(changes.has_changes, "untracked file should count as a change");
        assert!(
            changes.summary.contains("new_file.txt"),
            "summary should mention the untracked file: {}",
            changes.summary
        );
    }
}
