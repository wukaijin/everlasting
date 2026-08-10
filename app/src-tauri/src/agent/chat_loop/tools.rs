//! run_chat_loop 工具派发段(拆分自 chat_loop.rs,08-08-a-class-chat-loop-split)。
//!
//! `DispatchOutcome` struct + `dispatch_tool_calls` + `finalize_turn`:
//! hub turn 循环体内、drive_turn 之后的 tool_use 派发(L2 并行 / serial /
//! dispatch_subagent 拦截)与单轮收尾(loop hint 追加 + tool_result 持久化)。
//! hub 全量 re-export 符号。

#![allow(unused_imports)]

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use sqlx::SqlitePool;
use std::pin::Pin;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::agent::helpers::{emit_chat_event_via_sink, persist_turn_cwd};
use crate::agent::permissions::{self, Decision, PermissionContext};
use crate::agent::subagent::SubagentEventSink;
use crate::background_shell::BackgroundShellRegistry;
use crate::llm::{ChatEvent, ChatMessage, ContentBlock, MessageContent, Provider, Role};
use crate::memory::MemoryCache;
use crate::skill::loader::SkillCache;
use crate::state::{ChatEventSink, ProviderCatalog};
use crate::tools::read_guard::ReadGuard;
use crate::tools::ToolContext;

use super::{
    classify_dispatch_batch, delegation_max_concurrent_children, emit_persist_failure,
    is_parallel_eligible, DispatchBatch,
};

pub(crate) struct DispatchOutcome {
    pub(crate) result_blocks: Vec<ContentBlock>,
    pub(crate) cancelled: bool,
    pub(crate) current_ctx: ToolContext,
    pub(crate) last_cwd: Option<PathBuf>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn dispatch_tool_calls(
    tool_calls: Vec<(String, String, serde_json::Value)>,
    permission_ctx: PermissionContext,
    provider: Arc<dyn Provider>,
    db: SqlitePool,
    sink: Arc<dyn ChatEventSink>,
    rid: String,
    session_id: String,
    read_guard: ReadGuard,
    skill_cache: Arc<SkillCache>,
    current_ctx: ToolContext,
    last_cwd: Option<PathBuf>,
    cancelled: bool,
    token: CancellationToken,
    permission_asks: crate::agent::permissions::PermissionStore,
    memory_cache: Arc<MemoryCache>,
    cancellations: Arc<Mutex<std::collections::HashMap<String, CancellationToken>>>,
    session_active_request: Arc<Mutex<std::collections::HashMap<String, String>>>,
    background_shells: crate::background_shell::DefaultRegistry,
    worker_event_sink: Arc<dyn SubagentEventSink>,
    worker_catalog: Option<Arc<RwLock<ProviderCatalog>>>,
    context_window: u32,
    workflow_ctx: &Option<crate::agent::workflow::WorkflowCtx>,
    subagent_cache: Arc<crate::agent::subagent::SubagentCache>,
    app_data_dir: PathBuf,
    question_store: crate::agent::question_store::QuestionStore,
    group_chat_state: &Option<crate::tools::nominate_speaker::SharedTurnState>,
    session_mode: crate::db::Mode,
    failure_tracker: Arc<Mutex<crate::agent::auto_reflect::FailureTracker>>,
    soft_blocked: Arc<Mutex<std::collections::HashSet<String>>>,
    seq: i64,
    skip_persist: bool,
) -> DispatchOutcome {
    let mut cancelled = cancelled;
    let mut current_ctx = current_ctx;
    let mut last_cwd = last_cwd;
    let mut result_blocks: Vec<ContentBlock> = Vec::new();
    if is_parallel_eligible(&tool_calls, &permission_ctx.project_main_path) {
        // ---- L2 parallel path (read-only batch) ----
        //
        // All tool_use blocks in this turn are in the
        // {read_file, grep, glob, list_dir, use_skill}
        // whitelist AND every path tool's `path` resolves
        // inside the project root (`permission_ctx.project_main_path`)
        // → run them concurrently via `FuturesUnordered`.
        // `web_fetch` is excluded (Q2) because its Tier 4
        // default is `ask`, which would fire multiple
        // concurrent `permission:ask` modals. Path tools
        // with an out-of-root `path` are also excluded by
        // the same rule (RULE-A-013 follow-up, 2026-06-19)
        // — see `is_parallel_eligible` doc.
        //
        // Anchor note (2026-07-29): the root MUST be
        // `project_main_path`, not `cwd`/`worktree_path`. For an
        // isolated worker the latter both point at the worker's
        // checkout subtree, so reads of the project's source
        // files (by their original absolute path) would fail
        // `is_within_root` → the whole batch is demoted to serial
        // → each read tool hits `check()` separately → magnified
        // permission prompts. See `PermissionContext.project_main_path`.
        //
        // Permission-silence invariant (Q2 design +
        // RULE-A-013 closure): the concurrent set is
        // ALWAYS silent in every mode —
        //   - `use_skill` is `ToolKind::Other` → Tier 5
        //     default-allow in every mode;
        //   - path tools (`read_file`/`grep`/`glob`/
        //     `list_dir`) with `path` inside the project
        //     root hit Tier 4.1 path-grant or Tier 4.2
        //     inside-root silent Allow in `Edit`/`Yolo`
        //     (and in `Plan` too — `filter_tools_for_mode`
        //     only drops write/edit/shell, so read tools
        //     reach Tier 4 and resolve silently when the
        //     path is inside the project root).
        // The `is_parallel_eligible` predicate guarantees
        // this: a path tool with `path` outside the
        // project root (no `session_tool_permissions`
        // path-glob grant) is pulled back to the serial
        // path, where the existing single-modal UX
        // applies. See DEBT.md RULE-A-013 for the
        // previous open issue now closed.
        //
        // Result ordering: `result_slots` is pre-
        // allocated to the tool_use count and each task
        // writes its block at its OWN index. The LLM
        // context sees tool_results in the SAME order as
        // the tool_use blocks regardless of which task
        // finishes first. `emit_tool_result` fires as
        // each task completes (streaming, matching the
        // serial path's per-iteration emit).
        //
        // Cancel: every task takes `token.clone()` so the
        // execute_tool's `tokio::select!` wrapper cancels
        // each in-flight task independently. RULE-A-004
        // (cancelled tool skips audit) is preserved per-
        // task: a task whose `token.is_cancelled()` is
        // true after execute sets the shared `cancelled`
        // flag and skips the `tool_executed` audit write.
        // Already-completed tasks still get their audit
        // row. The shared flag is read after the join to
        // drive the existing cancel path.
        let n = tool_calls.len();
        let mut result_slots: Vec<Option<ContentBlock>> = (0..n).map(|_| None).collect();
        let cancelled_flag = Arc::new(AtomicBool::new(false));
        let mut fu: FuturesUnordered<_> = tool_calls
            .iter()
            .enumerate()
            .map(|(i, (id, name, input))| {
                let cancelled_flag = cancelled_flag.clone();
                let sink = sink.clone();
                let rid = rid.clone();
                let id = id.clone();
                let name = name.clone();
                let input = input.clone();
                let permission_ctx = permission_ctx.clone();
                let permission_asks = permission_asks.clone();
                let db = db.clone();
                let read_guard = read_guard.clone();
                let session_id = session_id.clone();
                let skill_cache = skill_cache.clone();
                let current_ctx = current_ctx.clone();
                let token = token.clone();
                // P4 (2026-06-29, 06-29-am-p4-event-reflect):
                // the per-session failure tracker + provider
                // + project_id are cloned into the parallel
                // task so the L2 batch's tool outcomes feed
                // the same in-session state machine as the
                // serial path. The reflection is fire-and-
                // forget (tokio::spawn inside) so the parallel
                // path's perf is unaffected.
                let failure_tracker = failure_tracker.clone();
                let provider = provider.clone();
                let project_id = current_ctx.project_id.clone();
                // P5 (2026-06-29, 06-29-am-p5-quality): clone
                // the soft-block ledger so the parallel task
                // can both read (recall decision) + write
                // (record a freshly-soft-blocked memory_id) it.
                let soft_blocked = soft_blocked.clone();
                async move {
                    // check + execute live in the SAME task
                    // (Q2 rationale: no ask risk in the
                    // parallel set → no need to split into
                    // a two-phase check-then-execute).
                    let decision = permissions::check(
                        &permission_ctx,
                        &permission_asks,
                        &db,
                        &sink,
                        &name,
                        &input,
                        &id,
                        &token,
                    )
                    .await;
                    if let Decision::Deny {
                        reason,
                        critical: _,
                    } = decision
                    {
                        let envelope = crate::agent::helpers::tool_result_envelope(
                            &reason,
                            &current_ctx.worktree_path,
                        );
                        sink.emit_tool_result(&crate::state::ToolResultPayload {
                            request_id: rid.clone(),
                            tool_use_id: id.clone(),
                            content: envelope.clone(),
                            is_error: true,
                        });
                        return Some((
                            i,
                            ContentBlock::ToolResult {
                                tool_use_id: id,
                                content: envelope,
                                is_error: true,
                            },
                        ));
                    }

                    // P3 + P5 (2026-06-29, 06-29-am-p3-tool-recall
                    // + 06-29-am-p5-quality): Tier 1 Hooks —
                    // pre-tool pitfall recall. Runs AFTER
                    // `check()` returns Allow and BEFORE
                    // `execute_tool()`. P5 introduces the
                    // tiered `PitfallRecall`:
                    //   - SoftBlock { hint, memory_id } —
                    //     verified + full trigger-key match +
                    //     not-yet-blocked this session → short-
                    //     circuit execute_tool, record memory_id,
                    //     surface hint as is_error=false tool_result.
                    //   - Footnote(text) — active / candidate /
                    //     partial / second-hit → execute normally,
                    //     prepend text to the result content.
                    //   - None — no recall.
                    // The dead-loop guard (design D1): the second
                    // hit on the same SoftBlock'd pitfall
                    // degrades to Footnote because the memory_id
                    // is now in `soft_blocked`.
                    // 07-06 (am-observability-panel R2b / A9):
                    // the rows-aware sibling carries the raw hit
                    // set so we can emit a `ChatEvent::Recall`
                    // for the frontend's "本次召回" chip.
                    // Worker isolation: this chat loop is the
                    // PARENT path — worker's `SubagentBufferSink`
                    // does not forward to the main chat IPC.
                    let blocked_snapshot = soft_blocked.lock().await.clone();
                    let (pitfall_recall, pitfall_rows) = permissions::recall_pitfall_with_hits(
                        &db,
                        &name,
                        &input,
                        &blocked_snapshot,
                    )
                    .await;
                    // Emit pre-tool pitfall recall hit (R2b).
                    // None → skip (no rows).
                    if !pitfall_rows.is_empty() {
                        crate::agent::memory_recall::emit_recall_event(
                            sink.as_ref(),
                            &rid,
                            &pitfall_rows,
                            "pitfall",
                        );
                    }
                    // SoftBlock: short-circuit. The tool is NOT
                    // executed; we surface the hint as a non-error
                    // tool_result so the LLM re-judges (adjust /
                    // abandon / insist). Mirrors Decision::Deny's
                    // "skip execute + return ToolResult" structure
                    // (above), but `is_error: false` so the LLM
                    // doesn't think the tool is broken.
                    if let permissions::PitfallRecall::SoftBlock { hint, memory_id } =
                        pitfall_recall
                    {
                        // Record in the session ledger (D1).
                        soft_blocked.lock().await.insert(memory_id);
                        let envelope = crate::agent::helpers::tool_result_envelope(
                            &hint,
                            &current_ctx.worktree_path,
                        );
                        sink.emit_tool_result(&crate::state::ToolResultPayload {
                            request_id: rid.clone(),
                            tool_use_id: id.clone(),
                            content: envelope.clone(),
                            is_error: false,
                        });
                        // No tool_executed audit (tool didn't run
                        // — RULE-A-004's intent applies: don't
                        // lie to the audit log). No P4 reflect
                        // either (no real outcome to learn from).
                        return Some((
                            i,
                            ContentBlock::ToolResult {
                                tool_use_id: id,
                                content: envelope,
                                is_error: false,
                            },
                        ));
                    }
                    // Footnote or None: extract the optional text
                    // for the prepend-after-execute path.
                    let pitfall_footnote = match pitfall_recall {
                        permissions::PitfallRecall::Footnote(text) => Some(text),
                        permissions::PitfallRecall::None => None,
                        permissions::PitfallRecall::SoftBlock { .. } => {
                            // Unreachable — handled above.
                            None
                        }
                    };

                    let tool_exec_start = Instant::now();
                    let (content, is_error, _update, exit_code) = crate::tools::execute_tool(
                        &name,
                        &input,
                        &current_ctx,
                        Some(&read_guard),
                        Some(&session_id),
                        Some(&skill_cache),
                        token.clone(),
                    )
                    .await;
                    // P3: prepend the pitfall footnote (if any)
                    // to the tool result content BEFORE the
                    // envelope wrap, so the LLM reads the hint
                    // together with the tool output. is_error
                    // is preserved (an error message preceded
                    // by a pitfall hint is still an error).
                    let content = if let Some(footnote) = pitfall_footnote {
                        format!("{}{}", footnote, content)
                    } else {
                        content
                    };
                    // P4 (2026-06-29, 06-29-am-p4-event-reflect):
                    // record this tool_use outcome into the
                    // in-session failure tracker. On the
                    // "≥2 fails then success" pattern, the
                    // tracker fires a fire-and-forget LLM
                    // reflection that produces a pitfall memory
                    // row. Skipped on cancel (the in-flight tool
                    // was interrupted, not a real outcome) — same
                    // intent as the audit-skip above.
                    if !token.is_cancelled() {
                        crate::agent::auto_reflect::try_record_outcome(
                            &failure_tracker,
                            provider.clone(),
                            db.clone(),
                            &rid,
                            &session_id,
                            &project_id,
                            &name,
                            &input,
                            is_error,
                            &content,
                        )
                        .await;
                    }
                    let duration_ms = tool_exec_start.elapsed().as_millis();
                    // RULE-A-004 (2026-06-15): a tool cancelled
                    // mid-flight MUST NOT leave a `tool_executed`
                    // audit row. The check + the skip are
                    // back-to-back (no `.await` between them).
                    // We broadcast via the shared AtomicBool so
                    // the main loop flips its local `cancelled`
                    // after the join and drives the existing
                    // cancel path.
                    if token.is_cancelled() {
                        cancelled_flag.store(true, Ordering::SeqCst);
                    } else if !skip_persist {
                        // B6 PR1b: skip the tool_executed audit
                        // write in worker mode. The
                        // SubagentBufferSink transcript is the
                        // worker's audit record; PR2 will
                        // persist it into `subagent_runs`.
                        if let Err(e) = permissions::record_tool_executed_audit(
                            &db,
                            &session_id,
                            &name,
                            &input,
                            duration_ms,
                            exit_code,
                            Some(seq),
                        )
                        .await
                        {
                            tracing::warn!(
                                error = %e,
                                "chat: record_tool_executed_audit failed (non-fatal)"
                            );
                        }
                    }
                    // Parallel batch is read-only by
                    // construction (is_parallel_eligible),
                    // so `update.new_cwd` is None for every
                    // task — no `current_ctx.cwd` mutation to
                    // apply. (`use_skill` doesn't cd; only
                    // `shell` does, and shell is excluded.)
                    let envelope_str = crate::agent::helpers::tool_result_envelope(
                        &content,
                        &current_ctx.worktree_path,
                    );
                    sink.emit_tool_result(&crate::state::ToolResultPayload {
                        request_id: rid.clone(),
                        tool_use_id: id.clone(),
                        content: envelope_str.clone(),
                        is_error,
                    });
                    Some((
                        i,
                        ContentBlock::ToolResult {
                            tool_use_id: id,
                            content: envelope_str,
                            is_error,
                        },
                    ))
                }
            })
            .collect();
        while let Some(maybe_block) = fu.next().await {
            if let Some((i, block)) = maybe_block {
                result_slots[i] = Some(block);
            }
        }
        // Collapse the slots into ordered result_blocks.
        // Every slot is Some (every task returns a block on
        // every branch — success, deny, cancel-after-execute
        // all emit + return a block); the only way a slot
        // could stay None is if the task panicked, in which
        // case `fu.next()` would have propagated the panic.
        result_blocks = result_slots.into_iter().flatten().collect();
        if cancelled_flag.load(Ordering::SeqCst) {
            cancelled = true;
        }
    } else {
        // ---- Serial path (write / shell / web_fetch /
        //       update_checklist / mixed batch) ----
        // Unchanged from pre-L2 behavior. Any batch that
        // contains a tool outside the read-only whitelist
        // falls back here; web_fetch is excluded from the
        // parallel set (Q2) precisely so its Tier 4 ask can
        // fire through the normal single-modal flow.
        //
        // L3a (2026-06-24) / L3b PR2 (2026-06-27): before the
        // regular serial `for` loop, classify the batch for
        // the concurrent dispatch_subagent path. A **pure**
        // batch of ≥2 dispatch_subagent tool_uses (no other
        // tools mixed in) within
        // `DELEGATION_MAX_CONCURRENT_CHILDREN` (env, default 10)
        // runs concurrently via `FuturesUnordered` (each worker
        // in its own per-worker worktree — L3b PR1 isolation,
        // not the L3a read-only scope). A pure batch OVER the
        // limit is hard-rejected (every tool_use gets a
        // tool_error tool_result — no truncation, no queuing,
        // mirrors Hermes). Anything else (single dispatch, or
        // a mixed batch) falls through to the regular serial
        // `for` loop unchanged.
        let dispatch_batch =
            classify_dispatch_batch(&tool_calls, delegation_max_concurrent_children());
        match dispatch_batch {
            DispatchBatch::OverLimit {
                count,
                max_concurrent,
            } => {
                // Hard reject: every dispatch_subagent tool_use
                // gets a tool_error tool_result. None execute.
                // The LLM sees N uniform failure signals + a
                // hint to re-plan (reduce the batch or split).
                tracing::warn!(
                    count,
                    max_concurrent,
                    "L3a: pure dispatch batch over concurrent limit — hard rejecting all"
                );
                for (id, _name, _input) in &tool_calls {
                    let reject_content = format!(
                        "Concurrent dispatch limit reached: {count} dispatch_subagent \
                         calls in one turn exceeds the limit of {max_concurrent}. Reduce \
                         the number of concurrent subagents per turn (dispatch fewer at \
                         once, or split across turns), or raise the limit via the \
                         DELEGATION_MAX_CONCURRENT_CHILDREN environment variable. No \
                         subagents were dispatched."
                    );
                    let envelope_str = crate::agent::helpers::tool_result_envelope(
                        &reject_content,
                        &current_ctx.worktree_path,
                    );
                    sink.emit_tool_result(&crate::state::ToolResultPayload {
                        request_id: rid.clone(),
                        tool_use_id: id.clone(),
                        content: envelope_str.clone(),
                        is_error: true,
                    });
                    result_blocks.push(ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: envelope_str,
                        is_error: true,
                    });
                }
            }
            DispatchBatch::Concurrent { count: _ } => {
                // ---- L3b PR2 concurrent dispatch path (pure
                //      dispatch_subagent batch, ≥2 workers,
                //      each in its own worker worktree) ----
                //
                // L3a (2026-06-24) launched this branch with
                // `force_readonly = true` to dissolve the 3
                // concurrent write races via a read-only
                // scope (no writes = no race). L3b PR1
                // (2026-06-27) introduced per-worker worktree
                // isolation; L3b PR2 (2026-06-27, this change)
                // **removes the read-only scope** for the
                // concurrent path — each concurrent worker now
                // runs in its own `worker/<run_id>` worktree
                // (general-purpose builtin defaults to
                // `isolation: Some(true)`, so writes land on
                // the worker's branch, not the parent's).
                //
                // Mirror the L2 parallel-read path's structure
                // (FuturesUnordered + result_slots[i] + shared
                // cancelled flag), but each task runs
                // `run_subagent` (with `force_readonly = false`,
                // serial-only param; the worker worktree
                // isolation takes its place). The 3-layer
                // read-only guarantee is **no longer the
                // concurrent path's safety argument** — the
                // per-worker worktree is.
                //
                // Permission: every dispatch_subagent tool_use
                // goes through the existing ⑨ check BEFORE the
                // task is spawned (mirrors the serial path's
                // pre-execute permission check). A Deny short-
                // circuits into a tool_result tool_use pairing
                // without spawning the worker.
                //
                // Cancel: each task takes `token.clone()`; the
                // worker's nested run_chat_loop sees the parent
                // cancel via `parent_token.child_token()` (the
                // existing fan-out mechanism). The shared
                // `cancelled_flag` is set if any worker
                // returned `cancel_parent = true` (parent-
                // propagated cancel detected).
                //
                // Audit: each successful worker dispatch
                // records its own `tool_executed` audit row
                // (same as the serial path's
                // `record_tool_executed_audit` call). Cancelled
                // workers skip the audit (RULE-A-004).
                //
                // Result ordering: `result_slots[i]` is pre-
                // allocated to the tool_use count; each task
                // writes at its OWN index so the LLM context
                // sees tool_results in tool_use order regardless
                // of completion order. `emit_tool_result` fires
                // as each task completes (streaming).
                let n = tool_calls.len();
                let mut result_slots: Vec<Option<ContentBlock>> = (0..n).map(|_| None).collect();
                let cancelled_flag = Arc::new(AtomicBool::new(false));
                let mut fu: FuturesUnordered<_> = tool_calls
                    .iter()
                    .enumerate()
                    .map(|(i, (id, name, input))| {
                        let sink = sink.clone();
                        let rid = rid.clone();
                        let id = id.clone();
                        let name = name.clone();
                        let input = input.clone();
                        let permission_ctx = permission_ctx.clone();
                        let permission_asks = permission_asks.clone();
                        let db = db.clone();
                        let session_id = session_id.clone();
                        let token = token.clone();
                        let provider = provider.clone();
                        let memory_cache = memory_cache.clone();
                        let read_guard = read_guard.clone();
                        let skill_cache = skill_cache.clone();
                        let cancellations = cancellations.clone();
                        let session_active_request = session_active_request.clone();
                        let background_shells = background_shells.clone();
                        let current_ctx = current_ctx.clone();
                        let worker_event_sink = worker_event_sink.clone();
                        let worker_catalog = worker_catalog.clone();
                        let cancelled_flag = cancelled_flag.clone();
                        // W1 Step 2.4: workflow role-gate
                        // needs WorkflowCtx inside the
                        // closure (the concurrent batch
                        // path). Clone like the other
                        // captured variables so the
                        // closure doesn't move the
                        // outer-scope `workflow_ctx`
                        // (which is also borrowed by the
                        // per-turn breadcrumb injection
                        // on line 1532 — moving would
                        // break that site on iteration
                        // N+1).
                        let workflow_ctx = workflow_ctx.clone();
                        // W1 Step 2.4: workflow role-gate
                        // needs WorkflowCtx inside the
                        // closure (the concurrent batch
                        // path). Clone like the other
                        // captured variables so the
                        // closure doesn't move the
                        // outer-scope `workflow_ctx`
                        // (which is also borrowed by the
                        // per-turn breadcrumb injection
                        // on line 1532 — moving would
                        // break that site on iteration
                        // N+1).
                        let workflow_ctx = workflow_ctx.clone();
                        let subagent_cache = subagent_cache.clone();
                        let app_data_dir = app_data_dir.clone();
                        // 2026-06-30 (`ask_user_question` task):
                        // clone the QuestionStore for this task
                        // body. The worker's toolset strips
                        // `ask_user_question` via
                        // `STRUCTURALLY_DISABLED`, so this clone
                        // is never actually used by the worker's
                        // run_subagent on this concurrent branch —
                        // pass for signature uniformity with the
                        // serial dispatch path. (The cargo borrow
                        // checker requires clone() here regardless
                        // because each concurrent task takes
                        // ownership of its slot via `async move`.)
                        let question_store = question_store.clone();
                        async move {
                            // Pre-execute ⑨ permission check
                            // (mirrors the serial path's
                            // permissions::check before execute).
                            let decision = permissions::check(
                                &permission_ctx,
                                &permission_asks,
                                &db,
                                &sink,
                                &name,
                                &input,
                                &id,
                                &token,
                            )
                            .await;
                            if let Decision::Deny { reason, critical: _ } = decision {
                                let envelope = crate::agent::helpers::tool_result_envelope(
                                    &reason,
                                    &current_ctx.worktree_path,
                                );
                                sink.emit_tool_result(&crate::state::ToolResultPayload {
                                    request_id: rid.clone(),
                                    tool_use_id: id.clone(),
                                    content: envelope.clone(),
                                    is_error: true,
                                });
                                return Some((
                                    i,
                                    ContentBlock::ToolResult {
                                        tool_use_id: id,
                                        content: envelope,
                                        is_error: true,
                                    },
                                ));
                            }

                            let tool_exec_start = Instant::now();
                            let (content, is_error, cancel_parent, exit_code) =
                                crate::agent::subagent::dispatch::run_subagent(
                                    &provider,
                                    worker_catalog.clone(),
                                    context_window,
                                    &rid,
                                    &session_id,
                                    &memory_cache,
                                    &read_guard,
                                    &skill_cache,
                                    &permission_asks,
                                    &cancellations,
                                    &session_active_request,
                                    &background_shells,
                                    &db,
                                    &current_ctx,
                                    &id,
                                    &input,
                                    &token,
                                    &sink,
                                    worker_event_sink.clone(),
                                    // L3b PR2 (2026-06-27): the
                                    // concurrent branch no longer
                                    // forces read-only. Per-worker
                                    // worktree isolation (PR1) is
                                    // the new safety argument;
                                    // `force_readonly` is now
                                    // serial-only. `general-purpose`
                                    // builtin defaults to
                                    // `isolation: Some(true)`, so
                                    // each concurrent worker lands
                                    // its writes on its own
                                    // `worker/<run_id>` branch.
                                    false,
                                    // L3d (2026-06-25): thread the
                                    // subagent cache so the worker
                                    // resolves across builtin + user
                                    // + project layers.
                                    &subagent_cache,
                                    // L3b (2026-06-27): thread the
                                    // app data dir for worker
                                    // worktree path computation.
                                    &app_data_dir,
                                    // B (2026-06-30): concurrent batch →
                                    // `parallel=true` force-isolates
                                    // writable workers onto their own
                                    // `worker/<run_id>` branch.
                                    true,
                                    // 2026-06-30 (`ask_user_question`):
                                    // thread the parent's QuestionStore.
                                    // Worker never reaches the
                                    // intercept (tool is in
                                    // STRUCTURALLY_DISABLED) — pass-
                                    // through only.
                                    &question_store,
                                    // W1 Step 2.4: workflow
                                    // role-gate. Same as the
                                    // forced-dispatch path: pass
                                    // the session's WorkflowCtx
                                    // (None for non-workflow).
                                    workflow_ctx.as_ref(),
                                )
                                .await;
                            let duration_ms = tool_exec_start.elapsed().as_millis();
                            // RULE-A-004 + audit (same shape as
                            // the serial dispatch path).
                            if token.is_cancelled() || cancel_parent {
                                cancelled_flag.store(true, Ordering::SeqCst);
                            } else if !skip_persist {
                                if let Err(e) = permissions::record_tool_executed_audit(
                                    &db,
                                    &session_id,
                                    &name,
                                    &input,
                                    duration_ms,
                                    exit_code,
                                Some(seq),
                                )
                                .await
                                {
                                    tracing::warn!(
                                        error = %e,
                                        "L3a concurrent dispatch: record_tool_executed_audit failed (non-fatal)"
                                    );
                                }
                            }
                            let envelope_str = crate::agent::helpers::tool_result_envelope(
                                &content,
                                &current_ctx.worktree_path,
                            );
                            sink.emit_tool_result(&crate::state::ToolResultPayload {
                                request_id: rid.clone(),
                                tool_use_id: id.clone(),
                                content: envelope_str.clone(),
                                is_error,
                            });
                            Some((
                                i,
                                ContentBlock::ToolResult {
                                    tool_use_id: id,
                                    content: envelope_str,
                                    is_error,
                                },
                            ))
                        }
                    })
                    .collect();
                while let Some(maybe_block) = fu.next().await {
                    if let Some((i, block)) = maybe_block {
                        result_slots[i] = Some(block);
                    }
                }
                result_blocks = result_slots.into_iter().flatten().collect();
                if cancelled_flag.load(Ordering::SeqCst) {
                    cancelled = true;
                }
            }
            DispatchBatch::Serial => {
                // Regular serial path (existing behavior, unchanged).
                for (id, name, input) in &tool_calls {
                    // Run the full 5-tier permission check (matches
                    // production). Tests that want a clean
                    // tool-execute-and-continue path should pre-load
                    // an Allow for the test tool into
                    // `session_tool_permissions`, or use a read tool
                    // that hits Tier 5 default-allow.
                    let decision = permissions::check(
                        &permission_ctx,
                        &permission_asks,
                        &db,
                        &sink,
                        name,
                        input,
                        id,
                        &token,
                    )
                    .await;
                    if let Decision::Deny {
                        reason,
                        critical: _,
                    } = decision
                    {
                        let envelope = crate::agent::helpers::tool_result_envelope(
                            &reason,
                            &current_ctx.worktree_path,
                        );
                        sink.emit_tool_result(&crate::state::ToolResultPayload {
                            request_id: rid.clone(),
                            tool_use_id: id.clone(),
                            content: envelope.clone(),
                            is_error: true,
                        });
                        result_blocks.push(ContentBlock::ToolResult {
                            tool_use_id: id.clone(),
                            content: envelope,
                            is_error: true,
                        });
                        continue;
                    }

                    // 2026-06-30 (`ask_user_question` task): block the
                    // current turn on a reverse-question card. This is
                    // the same "control-flow tool" interception pattern as
                    // `dispatch_subagent` below — the tool needs
                    // QuestionStore + ChatEventSink access that
                    // `execute_tool_inner` doesn't have. We route to
                    // `ask_user_question::execute_blocking`, which
                    // internally:
                    //   1. validates the schema (short-circuits with
                    //      `is_error: true` on boundary violations),
                    //   2. registers a oneshot in QuestionStore,
                    //   3. emits `tool:question` to the frontend,
                    //   4. `tokio::select! { cancel | oneshot }` until
                    //      resolve.
                    //
                    // `is_parallel_eligible` (the L2 whitelist of
                    // `read_file` / `grep` / `glob` / `list_dir` /
                    // `use_skill`) does NOT include this name → mixed
                    // batches fall to the serial path (this branch)
                    // automatically (per design §6.3 + the
                    // `dispatch_subagent` precedent).
                    //
                    // The interception here is structurally identical to
                    // the dispatch_subagent one just below: same audit
                    // row, same `tool:result` IPC emit, same
                    // `ContentBlock::ToolResult` push, same cancel
                    // propagation. The only difference is the work
                    // function — `execute_blocking` instead of
                    // `run_subagent` — and `execute_blocking` does NOT
                    // need the `cancel_parent` separate flag (it
                    // collapses into the session cancel token directly).
                    // We accept +1 turn counter cost for the blocking
                    // tool's recover (v1 trade-off — see PRD §R3).
                    if name == "ask_user_question" {
                        let tool_exec_start = Instant::now();
                        let (content, is_error, _update, exit_code) =
                            crate::tools::ask_user_question::execute_blocking(
                                input,
                                &session_id,
                                id,
                                &question_store,
                                &sink,
                                &token,
                            )
                            .await;
                        let duration_ms = tool_exec_start.elapsed().as_millis();
                        if token.is_cancelled() {
                            cancelled = true;
                        } else if !skip_persist {
                            if let Err(e) = permissions::record_tool_executed_audit(
                                &db,
                                &session_id,
                                name,
                                input,
                                duration_ms,
                                exit_code,
                                Some(seq),
                            )
                            .await
                            {
                                tracing::warn!(error = %e, "chat: record_tool_executed_audit failed for ask_user_question (non-fatal)");
                            }
                        }
                        let envelope_str = crate::agent::helpers::tool_result_envelope(
                            &content,
                            &current_ctx.worktree_path,
                        );
                        sink.emit_tool_result(&crate::state::ToolResultPayload {
                            request_id: rid.clone(),
                            tool_use_id: id.clone(),
                            content: envelope_str.clone(),
                            is_error,
                        });
                        result_blocks.push(ContentBlock::ToolResult {
                            tool_use_id: id.clone(),
                            content: envelope_str,
                            is_error,
                        });
                        if cancelled {
                            break;
                        }
                        continue;
                    }

                    // Group chat (07-29-group-chat): the
                    // moderator calls `nominate_speaker` to hand
                    // the floor to a participant, or
                    // `end_discussion` to stop. Both are SIGNAL
                    // tools (non-blocking) — they record into the
                    // shared `group_chat_state` (read by
                    // `run_group_chat_loop` after this loop
                    // returns) and emit a confirmation as
                    // tool_result so the moderator's turn ends
                    // at a clean boundary.
                    //
                    // No audit row + no oneshot (unlike
                    // ask_user_question): these are pure
                    // in-process signals. When `group_chat_state`
                    // is `None` (classic chat / worker), the call
                    // is a misuse — return an error tool_result.
                    if name == crate::tools::nominate_speaker::NOMINATE_SPEAKER_TOOL_NAME
                        || name == crate::tools::end_discussion::END_DISCUSSION_TOOL_NAME
                    {
                        let (content, is_error) = match &group_chat_state {
                            Some(state) if name
                                == crate::tools::nominate_speaker::NOMINATE_SPEAKER_TOOL_NAME =>
                            {
                                crate::tools::nominate_speaker::execute_intercept(
                                    state, input,
                                )
                                .await
                            }
                            Some(state) if name
                                == crate::tools::end_discussion::END_DISCUSSION_TOOL_NAME =>
                            {
                                crate::tools::end_discussion::execute_intercept(
                                    state, input,
                                )
                                .await
                            }
                            _ => (
                                format!(
                                    "{}: this tool is only available in a group chat session.",
                                    name
                                ),
                                true,
                            ),
                        };
                        let envelope_str = crate::agent::helpers::tool_result_envelope(
                            &content,
                            &current_ctx.worktree_path,
                        );
                        sink.emit_tool_result(&crate::state::ToolResultPayload {
                            request_id: rid.clone(),
                            tool_use_id: id.clone(),
                            content: envelope_str.clone(),
                            is_error,
                        });
                        result_blocks.push(ContentBlock::ToolResult {
                            tool_use_id: id.clone(),
                            content: envelope_str,
                            is_error,
                        });
                        continue;
                    }

                    // 2026-07-07 (`request_mode_change` task):
                    // block the current turn on a mode-switch
                    // request card. Same "control-flow tool"
                    // interception pattern as `ask_user_question`
                    // / `dispatch_subagent` below — the tool
                    // needs `QuestionStore` + `ChatEventSink` +
                    // `db` access that `execute_tool_inner`
                    // doesn't have. We route to
                    // `request_mode_change::execute_blocking`,
                    // which internally:
                    //   1. validates the schema (short-circuits
                    //      with `is_error: true` on boundary
                    //      violations),
                    //   2. noop-checks `target == current` (skips
                    //      card + IPC when nothing to do, PRD R7),
                    //   3. records a `mode_change_requested`
                    //      audit row,
                    //   4. registers a oneshot in
                    //      `QuestionStore` (tagged
                    //      `PendingInteraction::ModeChange`),
                    //   5. emits `mode:change:request` to the
                    //      frontend,
                    //   6. `tokio::select! { cancel | oneshot }`
                    //      until resolve. The actual mode
                    //      application happens in the
                    //      `resolve_mode_change` IPC handler
                    //      (single source of truth for mode
                    //      change side effects, per design §5.5
                    //      "Yolo 二次守门的一致性").
                    //
                    // `is_parallel_eligible` (the L2 whitelist
                    // of `read_file` / `grep` / `glob` /
                    // `list_dir` / `use_skill`) does NOT
                    // include this name → mixed batches fall to
                    // the serial path (this branch) automatically
                    // (per design §6.3 + the `dispatch_subagent`
                    // precedent).
                    //
                    // We accept +1 turn counter cost for the
                    // blocking tool's recover (v1 trade-off —
                    // see PRD §R3).
                    if name == "request_mode_change" {
                        let tool_exec_start = Instant::now();
                        let (content, is_error, _update, exit_code) =
                            crate::tools::request_mode_change::execute_blocking(
                                input,
                                &session_id,
                                id,
                                session_mode,
                                &db,
                                &question_store,
                                &sink,
                                &token,
                                Some(seq),
                            )
                            .await;
                        let duration_ms = tool_exec_start.elapsed().as_millis();
                        if token.is_cancelled() {
                            cancelled = true;
                        } else if !skip_persist {
                            if let Err(e) = permissions::record_tool_executed_audit(
                                &db,
                                &session_id,
                                name,
                                input,
                                duration_ms,
                                exit_code,
                                Some(seq),
                            )
                            .await
                            {
                                tracing::warn!(error = %e, "chat: record_tool_executed_audit failed for request_mode_change (non-fatal)");
                            }
                        }
                        let envelope_str = crate::agent::helpers::tool_result_envelope(
                            &content,
                            &current_ctx.worktree_path,
                        );
                        sink.emit_tool_result(&crate::state::ToolResultPayload {
                            request_id: rid.clone(),
                            tool_use_id: id.clone(),
                            content: envelope_str.clone(),
                            is_error,
                        });
                        result_blocks.push(ContentBlock::ToolResult {
                            tool_use_id: id.clone(),
                            content: envelope_str,
                            is_error,
                        });
                        if cancelled {
                            break;
                        }
                        continue;
                    }

                    // 2026-07-08 (`07-08-workflow-integration`
                    // Phase 3 Step 3.1): block the current turn
                    // on a workflow state-transition request
                    // card. Mirrors the `request_mode_change`
                    // interception branch above — same blocking-
                    // tool pattern, same
                    // `name == "..."` short-circuit BEFORE the
                    // normal `execute_tool` path.
                    //
                    // The tool needs (a) the
                    // `current_state` + `current_slug` from
                    // the active workflow session's
                    // `WorkflowCtx.current_task` (read once per
                    // turn at the top of the loop), (b) the
                    // `QuestionStore` + `ChatEventSink
                    // ::emit_task_state_transition` (already
                    // threaded through), and (c) the DB pool for
                    // audit. The actual `task.json` mutation +
                    // `from → to` Rust hook dispatch happens in
                    // the `resolve_task_state_transition` IPC
                    // handler (single source of truth for
                    // state-transition side effects — see design
                    // §5.2 M-A / Q9). The tool stays a "request
                    // → wait → return" carrier.
                    //
                    // `is_parallel_eligible` does NOT include
                    // this name → mixed batches fall to the
                    // serial path (this branch) automatically
                    // (mirrors the `request_mode_change`
                    // posture).
                    //
                    // We accept +1 turn counter cost for the
                    // blocking tool's recover (v1 trade-off —
                    // matches the request_mode_change branch).
                    if name == "request_task_state_transition" {
                        let tool_exec_start = Instant::now();
                        // Pull the workflow session's current
                        // task state + slug for the noop check.
                        // `build_workflow_ctx` is called once
                        // per turn at the top of the loop; we
                        // reuse the snapshot here. When the
                        // session is non-workflow the
                        // `WorkflowCtx` is `None` and the tool
                        // gates with `is_error: true` (the
                        // structured message
                        // "no active workflow task").
                        // R3 (07-10-workflow-task-json-hardening):
                        // resolve fresh off disk instead of the frozen
                        // workflow_ctx.current_task snapshot. The agent may
                        // have created / mutated the task earlier in this
                        // same loop (create_task tool / write_file), and the
                        // transition gate must see the real on-disk state —
                        // matches the apply-side resolve_task_state_transition
                        // "read fresh off disk" posture (question.rs). Only
                        // workflow sessions can transition; a workflow session
                        // with no on-disk task short-circuits to (None, None)
                        // → "no active workflow task".
                        let (current_state, current_slug) = if workflow_ctx.is_some() {
                            let fresh = crate::agent::workflow::inject::resolve_current_task(
                                &current_ctx.worktree_path,
                            )
                            .await;
                            match fresh {
                                Some(t) => (Some(t.status.clone()), Some(t.slug.clone())),
                                None => (None, None),
                            }
                        } else {
                            (None, None)
                        };
                        let (content, is_error, _update, exit_code) =
                            crate::tools::request_task_state_transition::execute_blocking(
                                input,
                                &session_id,
                                id,
                                current_state,
                                current_slug,
                                &db,
                                &question_store,
                                &sink,
                                &token,
                                // C5: validate transitions against the TASK's
                                // owning plugin, not the session plugin — a dev
                                // task keeps dev's transition rules even when
                                // the session is switched to review.
                                workflow_ctx.as_ref().map(|c| &c.task_workflow_def),
                                Some(seq),
                            )
                            .await;
                        let duration_ms = tool_exec_start.elapsed().as_millis();
                        if token.is_cancelled() {
                            cancelled = true;
                        } else if !skip_persist {
                            if let Err(e) = permissions::record_tool_executed_audit(
                                &db,
                                &session_id,
                                name,
                                input,
                                duration_ms,
                                exit_code,
                                Some(seq),
                            )
                            .await
                            {
                                tracing::warn!(error = %e, "chat: record_tool_executed_audit failed for request_task_state_transition (non-fatal)");
                            }
                        }
                        let envelope_str = crate::agent::helpers::tool_result_envelope(
                            &content,
                            &current_ctx.worktree_path,
                        );
                        sink.emit_tool_result(&crate::state::ToolResultPayload {
                            request_id: rid.clone(),
                            tool_use_id: id.clone(),
                            content: envelope_str.clone(),
                            is_error,
                        });
                        result_blocks.push(ContentBlock::ToolResult {
                            tool_use_id: id.clone(),
                            content: envelope_str,
                            is_error,
                        });
                        if cancelled {
                            break;
                        }
                        continue;
                    }

                    // B6 Subagent (2026-06-19): intercept dispatch_subagent
                    // BEFORE the normal execute_tool path. This is an
                    // agent-layer control-flow tool — it needs the parent
                    // loop's full closure dependencies (provider / db /
                    // cancellations / ...) which `execute_tool_inner` does
                    // NOT have access to (see `agent::subagent` docstring +
                    // PRD §"Technical Approach" review #3). The interceptor
                    // builds the worker context, calls run_chat_loop
                    // recursively, and turns the worker's final state into a
                    // tool_result that pairs with the dispatch_subagent
                    // tool_use (RULE-A-007 pairing invariant preserved).
                    //
                    // dispatch_subagent is structurally excluded from the
                    // L2 parallel set (it's not in `is_parallel_eligible`'s
                    // NAME_ELIGIBLE list), so the entire batch falls into
                    // this serial path whenever the model emits it. MVP runs
                    // dispatches serially (one worker at a time); parallel
                    // fan-out is v2 / L3.
                    if name == crate::agent::subagent::DISPATCH_TOOL_NAME {
                        let tool_exec_start = Instant::now();
                        let (content, is_error, cancel_parent, exit_code) =
                            crate::agent::subagent::dispatch::run_subagent(
                                &provider,
                                worker_catalog.clone(),
                                context_window,
                                &rid,
                                &session_id,
                                &memory_cache,
                                &read_guard,
                                &skill_cache,
                                &permission_asks,
                                &cancellations,
                                &session_active_request,
                                &background_shells,
                                &db,
                                &current_ctx,
                                id,
                                input,
                                &token,
                                &sink,
                                // P2.4 C5 (2026-07-22): inject the worker's
                                // `SubagentEventSink` (replaces the forwarded
                                // `app_handle`). See the `worker_event_sink`
                                // param doc on `run_chat_loop`.
                                worker_event_sink.clone(),
                                // L3a (2026-06-24): serial path keeps the
                                // worker's full toolset (write/shell/web for
                                // general-purpose), gated by `is_worker: true`
                                // at the ⑨ permission layer. The concurrent
                                // branch below passes `true` to force read-only.
                                false,
                                // L3d (2026-06-25): thread the subagent cache so
                                // `run_subagent` can look up the dispatched
                                // subagent across builtin + user + project layers
                                // (replaces the static `lookup_subagent(name)`).
                                &subagent_cache,
                                // L3b (2026-06-27): thread the app data dir so
                                // `run_subagent` can compute the worker worktree
                                // path when isolation is active.
                                &app_data_dir,
                                // B (2026-06-30): serial dispatch → `parallel=false`,
                                // isolation falls back to the subagent's default
                                // (general-purpose now shared).
                                false,
                                // 2026-06-30 (`ask_user_question`): thread the
                                // parent's QuestionStore. Worker can't reach it
                                // (STRUCTURALLY_DISABLED) but the signature
                                // requires it.
                                &question_store,
                                // W1 Step 2.4: workflow role-gate.
                                // Serial dispatch — same contract
                                // as the concurrent + forced paths:
                                // pass the session's WorkflowCtx
                                // (None for non-workflow sessions).
                                workflow_ctx.as_ref(),
                            )
                            .await;
                        let duration_ms = tool_exec_start.elapsed().as_millis();
                        // Audit dispatch_subagent like any other tool so
                        // the C4 audit log records "subagent ran". This
                        // lands AFTER the worker's full turn sequence +
                        // the worker's own audit rows already landed
                        // (they're tied to the same session_id by design —
                        // workers don't have their own sessions).
                        if token.is_cancelled() {
                            cancelled = true;
                        } else if !skip_persist {
                            // B6 PR1b: the parent (NOT the worker) records
                            // its own dispatch_subagent audit. The worker
                            // passes skip_persist=true on its nested
                            // run_chat_loop call, so this site is only
                            // reached for the parent's own dispatch —
                            // the worker's run_subagent returns BEFORE
                            // any nested run_chat_loop call sees this code.
                            if let Err(e) = permissions::record_tool_executed_audit(
                                &db,
                                &session_id,
                                name,
                                input,
                                duration_ms,
                                exit_code,
                                Some(seq),
                            )
                            .await
                            {
                                tracing::warn!(error = %e, "chat: record_tool_executed_audit failed for dispatch_subagent (non-fatal)");
                            }
                        }
                        let envelope_str = crate::agent::helpers::tool_result_envelope(
                            &content,
                            &current_ctx.worktree_path,
                        );
                        sink.emit_tool_result(&crate::state::ToolResultPayload {
                            request_id: rid.clone(),
                            tool_use_id: id.clone(),
                            content: envelope_str.clone(),
                            is_error,
                        });
                        result_blocks.push(ContentBlock::ToolResult {
                            tool_use_id: id.clone(),
                            content: envelope_str,
                            is_error,
                        });
                        if cancel_parent {
                            cancelled = true;
                        }
                        if cancelled {
                            break;
                        }
                        continue;
                    }

                    // P3 + P5 (2026-06-29, 06-29-am-p3-tool-recall +
                    // 06-29-am-p5-quality): Tier 1 Hooks — pre-tool pitfall
                    // recall. Runs AFTER `check()` returns Allow and the
                    // dispatch_subagent intercept, BEFORE `execute_tool()`.
                    // P5 introduces the tiered `PitfallRecall`:
                    //   - SoftBlock { hint, memory_id } — verified + full
                    //     trigger-key match + not-yet-blocked this session →
                    //     short-circuit execute_tool, record memory_id,
                    //     surface hint as is_error=false tool_result.
                    //   - Footnote(text) — active / candidate / partial /
                    //     second-hit → execute normally, prepend text.
                    //   - None — no recall.
                    // The dead-loop guard (design D1): the second hit on the
                    // same SoftBlock'd pitfall degrades to Footnote because
                    // the memory_id is now in `soft_blocked`.
                    // 07-06 (am-observability-panel R2b / A9): the
                    // rows-aware sibling carries the raw hit set so we
                    // can emit a `ChatEvent::Recall` for the frontend's
                    // "本次召回" chip. Worker isolation is the same as
                    // the parallel path above.
                    let blocked_snapshot = soft_blocked.lock().await.clone();
                    let (pitfall_recall, pitfall_rows) =
                        permissions::recall_pitfall_with_hits(&db, name, input, &blocked_snapshot)
                            .await;
                    if !pitfall_rows.is_empty() {
                        crate::agent::memory_recall::emit_recall_event(
                            sink.as_ref(),
                            &rid,
                            &pitfall_rows,
                            "pitfall",
                        );
                    }
                    // SoftBlock: short-circuit. The tool is NOT executed; we
                    // surface the hint as a non-error tool_result so the LLM
                    // re-judges (adjust / abandon / insist). `is_error=false`
                    // so the LLM doesn't think the tool is broken — semantics
                    // are "experience hint", not "tool failed".
                    if let permissions::PitfallRecall::SoftBlock { hint, memory_id } =
                        pitfall_recall
                    {
                        // Record in the session ledger (D1).
                        soft_blocked.lock().await.insert(memory_id);
                        let envelope = crate::agent::helpers::tool_result_envelope(
                            &hint,
                            &current_ctx.worktree_path,
                        );
                        sink.emit_tool_result(&crate::state::ToolResultPayload {
                            request_id: rid.clone(),
                            tool_use_id: id.clone(),
                            content: envelope.clone(),
                            is_error: false,
                        });
                        result_blocks.push(ContentBlock::ToolResult {
                            tool_use_id: id.clone(),
                            content: envelope,
                            is_error: false,
                        });
                        // No tool_executed audit (tool didn't run — RULE-
                        // A-004's intent). No P4 reflect either (no real
                        // outcome). Proceed to the next tool_use.
                        continue;
                    }
                    // Footnote or None: extract optional text for the
                    // prepend-after-execute path.
                    let pitfall_footnote = match pitfall_recall {
                        permissions::PitfallRecall::Footnote(text) => Some(text),
                        permissions::PitfallRecall::None => None,
                        permissions::PitfallRecall::SoftBlock { .. } => {
                            // Unreachable — handled above.
                            None
                        }
                    };

                    let tool_exec_start = Instant::now();
                    let (content, is_error, update, exit_code) = crate::tools::execute_tool(
                        name,
                        input,
                        &current_ctx,
                        Some(&read_guard),
                        Some(&session_id),
                        Some(&skill_cache),
                        token.clone(),
                    )
                    .await;
                    // P3: prepend the pitfall footnote (if any) to the
                    // tool result content BEFORE the envelope wrap, so the
                    // LLM reads the hint together with the tool output.
                    // is_error is preserved (an error message preceded by
                    // a pitfall hint is still an error).
                    let content = if let Some(footnote) = pitfall_footnote {
                        format!("{}{}", footnote, content)
                    } else {
                        content
                    };
                    // P4 (2026-06-29, 06-29-am-p4-event-reflect):
                    // record this tool_use outcome into the
                    // in-session failure tracker. On the "≥2 fails
                    // then success" pattern, the tracker fires a
                    // fire-and-forget LLM reflection that produces
                    // a pitfall memory row. Skipped on cancel (the
                    // in-flight tool was interrupted, not a real
                    // outcome) — same intent as the audit-skip
                    // below.
                    if !token.is_cancelled() {
                        crate::agent::auto_reflect::try_record_outcome(
                            &failure_tracker,
                            provider.clone(),
                            db.clone(),
                            &rid,
                            &session_id,
                            &current_ctx.project_id,
                            name,
                            input,
                            is_error,
                            &content,
                        )
                        .await;
                    }
                    let duration_ms = tool_exec_start.elapsed().as_millis();
                    // RULE-A-004 (2026-06-15): audit AFTER the cancel
                    // check. Previously `record_tool_executed_audit` ran
                    // before the `token.is_cancelled()` test, so a tool
                    // whose execution was interrupted by a cancel (token
                    // fired during `execute_tool`) still got a
                    // `tool_executed` audit row — lying to the audit log
                    // (the tool did not complete from the user's intent;
                    // they hit Stop). Now a cancelled-in-flight tool is
                    // marked `cancelled` and skipped for auditing. The two
                    // checks are back-to-back with no `.await` between
                    // them, so the token state is identical across both.
                    if token.is_cancelled() {
                        cancelled = true;
                    } else if !skip_persist {
                        // B6 PR1b: skip the tool_executed audit write in
                        // worker mode (SubagentBufferSink transcript is
                        // the worker's record; PR2 persists it).
                        if let Err(e) = permissions::record_tool_executed_audit(
                            &db,
                            &session_id,
                            name,
                            input,
                            duration_ms,
                            exit_code,
                            Some(seq),
                        )
                        .await
                        {
                            tracing::warn!(error = %e, "chat: record_tool_executed_audit failed (non-fatal)");
                        }
                    }
                    if let Some(new_cwd) = update.new_cwd.clone() {
                        current_ctx.cwd = new_cwd.clone();
                        last_cwd = Some(new_cwd);
                    }
                    let envelope_str = crate::agent::helpers::tool_result_envelope(
                        &content,
                        &current_ctx.worktree_path,
                    );
                    sink.emit_tool_result(&crate::state::ToolResultPayload {
                        request_id: rid.clone(),
                        tool_use_id: id.clone(),
                        content: envelope_str.clone(),
                        is_error,
                    });
                    result_blocks.push(ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: envelope_str,
                        is_error,
                    });
                    if cancelled {
                        break;
                    }
                }
            } // close DispatchBatch::Serial => { … }
        } // close match dispatch_batch
    }
    DispatchOutcome {
        result_blocks,
        cancelled,
        current_ctx,
        last_cwd,
    }
}

/// Per-turn finalize: append the loop-detection hint (if any) to the
/// `result_blocks`, then persist the tool-result turn. Two early-return paths
/// surface as `Err(())` so the hub returns: (a) the cancel path (partial
/// results persisted + terminal `Done { cancelled }` emitted), (b) a
/// `persist_turn` failure (`emit_persist_failure`). On the normal path the
/// tool-result message is pushed to `messages` (the hub bumps `seq` after a
/// successful return; the cancel path skips the bump via the early return).
///
/// Split off `run_chat_loop` (08-08-a-class-chat-loop-split). No behavior
/// change — pure lift.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn finalize_turn(
    mut result_blocks: Vec<ContentBlock>,
    loop_hint: &Option<String>,
    cancelled: bool,
    skip_persist: bool,
    db: &SqlitePool,
    sink: &Arc<dyn ChatEventSink>,
    rid: &str,
    session_id: &str,
    seq: i64,
    messages: &mut Vec<ChatMessage>,
    last_cwd: &Option<PathBuf>,
) -> Result<(), ()> {
    // ⑬ loop detection (C2): if this turn tripped the detector,
    // append the hint as a Text block AT THE END of the
    // tool_results. Soft nudge only — execution was NOT skipped
    // and the loop is NOT terminated.
    //
    // WHY the END (not position 0): the user-role message built
    // from `result_blocks` is fed through `wire::chat_request_to_wire`,
    // whose `chat_message_to_wire_messages` fans out each block to a
    // separate wire message — Text → `WireMessage::User { content }`,
    // ToolResult → `WireMessage::Tool { tool_call_id, .. }`, in block
    // order. If the hint Text block sits at position 0, the fan-out
    // produces `user(text) → tool×N`, i.e. a `WireMessage::User`
    // inserted BETWEEN the preceding `assistant(tool_calls)` and the
    // `role: "tool"` messages. OpenAI Chat Completions rejects this
    // with HTTP 400 "An assistant message with 'tool_calls' must be
    // followed by tool messages responding to each 'tool_call_id'"
    // (Anthropic has the same Pair Atomicity rule, see
    // llm-contract.md §Pair Atomicity — but OpenAI enforces the *order*
    // strictly while Anthropic tolerates tool_results inside a user
    // message regardless of interleaving). Putting the hint at the
    // END yields `tool×N → user(text)`, which both protocols accept
    // (the tool pair stays contiguous; the trailing user message is a
    // normal follow-up).
    if let Some(hint) = &loop_hint {
        result_blocks.push(ContentBlock::Text {
            text: format!("⚠️  {}\n", hint),
            cache_control: None,
        });
    }

    if cancelled {
        let result_count = result_blocks.len();
        if !result_blocks.is_empty() {
            let tool_result_msg = ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(result_blocks),
                speaker: None,
            };
            // B6 PR1b: skip the cancelled tool_result persist
            // in worker mode (SubagentBufferSink transcript is
            // the worker's record).
            if !skip_persist {
                // RULE-A-003 (2026-06-15): cancel path —
                // log-only (see the synthetic tool_result
                // site above for why this stays tracing-only
                // instead of emit_persist_failure).
                if let Err(e) = crate::db::persist_turn(
                    db,
                    session_id,
                    tool_result_msg.role,
                    &tool_result_msg.content,
                    seq,
                    None,
                    None,
                )
                .await
                {
                    tracing::error!(error = %e, "failed to persist cancelled tool_result turn");
                }
            }
            messages.push(tool_result_msg);
            tracing::info!(
                request_id = %rid,
                tool_results = result_count,
                "chat_loop: cancelled during tool execution — persisted partial results"
            );
        }
        if !skip_persist {
            persist_turn_cwd(db, session_id, last_cwd.as_deref()).await;
            let _ = crate::db::touch_session(db, session_id).await;
        }
        // B6 PR1b: always emit terminal `Done { cancelled }` —
        // the SubagentBufferSink reads it to set `was_cancelled`
        // (so `run_subagent` can format the dispatch_subagent
        // tool_result with `status=cancelled`).
        emit_chat_event_via_sink(
            sink,
            rid,
            &ChatEvent::Done {
                stop_reason: Some("cancelled".to_string()),
                usage: None,
            },
        );
        return Err(());
    }

    let tool_result_msg = ChatMessage {
        role: Role::User,
        content: MessageContent::Blocks(result_blocks),
        speaker: None,
    };
    // B6 PR1b: skip the tool_result persist in worker mode
    // (SubagentBufferSink transcript is the worker's record).
    if !skip_persist {
        // RULE-A-003 (2026-06-15): tool_result persist
        // failure → emit Error + abort. Previously silent +
        // `seq += 1` drift; the next turn's LLM context would
        // otherwise be built on a tool_result the DB never
        // recorded.
        if let Err(e) = crate::db::persist_turn(
            db,
            session_id,
            tool_result_msg.role,
            &tool_result_msg.content,
            seq,
            None,
            None,
        )
        .await
        {
            emit_persist_failure(sink, rid, &e);
            return Err(());
        }
    }
    messages.push(tool_result_msg);
    Ok(())
}
