//! B6 Subagent — worker dispatch (`run_subagent`).
//!
//! Split out of `chat_loop.rs` on 2026-06-23 so the main loop file
//! stays focused on turn orchestration. `run_subagent` is the
//! interceptor helper called from
//! [`crate::agent::chat_loop::run_chat_loop`]'s serial-path tool
//! dispatch when `name == "dispatch_subagent"`; it owns the nested
//! `run_chat_loop` call that drives the worker agent.

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
    lookup_subagent, summarize_worker_tool_actions, truncate_transcript_for_persistence,
    SubagentBufferSink, SubagentStatus, TRANSCRIPT_MAX_BYTES,
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
    // L3a (2026-06-24): when `true`, the worker's toolset is
    // additionally forced down to read-only tools
    // (`filter_tools_readonly`) on top of `filter_tools_for_subagent`.
    // Used by the concurrent dispatch branch in `chat_loop.rs`
    // (pure dispatch batch, ≥2 workers running in parallel). The
    // serial path (single dispatch or mixed batch) passes `false`
    // — `general-purpose` in the serial path keeps its full
    // write/shell/web toolset (gated by `is_worker: true` at the
    // ⑨ permission layer). This is the 2nd layer of the 3-layer
    // read-only guarantee (PRD §"只读保证三层").
    force_readonly: bool,
) -> (String, bool, bool, Option<i32>) {
    // Parse the LLM-supplied { subagent, task } arguments.
    let subagent_name = input
        .get("subagent")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let task = input.get("task").and_then(|v| v.as_str()).unwrap_or("");
    let tool_use_id_owned = tool_use_id.to_string();

    // Resolve the SubagentDef. Unknown name → error tool_result
    // (keeps the tool_use/tool_result pairing invariant).
    let Some(def) = lookup_subagent(subagent_name) else {
        let content = format!(
            "Unknown subagent '{}'. Available: researcher, general-purpose.",
            subagent_name
        );
        return (content, true, false, None);
    };
    if task.trim().is_empty() {
        let content = "Missing or empty 'task' parameter. The delegation task must be a                        non-empty string."
            .to_string();
        return (content, true, false, None);
    }

    // Build the worker's toolset (allowlist + structural-disabled
    // strip). The worker's run_chat_loop call gets this filtered
    // Vec; the parent's tool_defs is unaffected.
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

    // Resolve the parent session's project_id + path so the worker
    // reads the same memory cache slots the parent uses.
    let project_id = resolve_project_id(db, parent_session_id).await;
    let project_path = current_ctx.worktree_path.to_string_lossy().to_string();

    // Build the worker's messages: [memory_blocks (cache_control),
    // delegation_task]. The task is APPENDed (prompt-cache invariant
    // — see PRD §Decisions 6 + research §10.5).
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
    let worker_run_id_opt: Option<String> = match crate::db::subagent_runs::insert_run(
        db,
        parent_session_id,
        &worker_rid,
        subagent_name,
        Some(task),
    )
    .await
    {
        Ok(id) => Some(id),
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
        read_guard.clone(),
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
    // **Streaming token usage** (the parent's live counter
    // updating as the worker burns tokens) is handled by the
    // `add_token_usage` call at chat_loop.rs:907 — which was
    // decoupled from `skip_persist` in this same PR. The sink's
    // per-turn accumulator is parallel: the sink still records
    // the per-turn `TokenUsage` so `cumulative_usage()` can
    // produce the worker-run-level total for `token_usage_json`
    // even after the streaming path has already folded individual
    // turn values into the parent's running total.
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
    let (content, is_error) =
        format_dispatch_result(status, &worker_text, partial_actions.as_deref());
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
