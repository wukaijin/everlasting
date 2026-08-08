//! run_chat_loop 单轮驱动段(拆分自 chat_loop.rs,08-08-a-class-chat-loop-split)。
//!
//! `DriveTurnOutcome` struct + `drive_turn` 函数:hub turn 循环体内、一轮的
//! C3 compaction → provider.send → 事件流处理 → persist_turn。早返路径
//! (StillOver / cancel / error)的 emit 留在函数内部,`return Err(())` 通知 hub
//! 退出;正常路径返回 [`DriveTurnOutcome`]。hub 全量 re-export 符号。

#![allow(unused_imports)]

use std::collections::VecDeque;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use futures_util::StreamExt;
use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

use crate::agent::helpers::{
    build_synthetic_tool_result_message, emit_chat_event_via_sink, persist_turn_cwd,
    CANCELLED_MARKER, ERROR_MARKER,
};
use crate::agent::loop_detection;
use crate::agent::permissions::{self, PermissionContext};
use crate::agent::thinking::{
    flush_ordered_thinking, flush_pending_text, flush_pending_thinking, PendingThinking,
};
use crate::background_shell::BackgroundShellRegistry;
use crate::llm::retry::{retry_open, OpenOutcome, RetryPolicy, RetrySink};
use crate::llm::{
    ChatEvent, ChatMessage, ContentBlock, LlmErrorCategory, MessageContent, Provider, Role, ToolDef,
};
use crate::state::{ChatEventSink, ToolCallPayload};
use crate::tools::ToolContext;

use super::{
    build_turn_latency, emit_persist_failure, finalize_pending_tool_results, LlmRetrySink,
};

/// [`DispatchOutcome`] carrying the `result_blocks` Vec (tool_results in
/// tool_use order) + the mutated `cancelled` / `current_ctx` / `last_cwd`
/// (a shell-class tool may change the working directory; any cancel /
/// cancel_parent sets `cancelled`). The caller persists + feeds the blocks
/// back into the next turn.
///
/// Split off `run_chat_loop` (08-08-a-class-chat-loop-split). No behavior
/// change — pure lift; the block contained no `return` statements.
/// Per-turn LLM drive: head_sha/system_prompt refresh → C3 compaction →
/// checklist/background injection → LLM retry/stream → event loop → post-stream
/// flush + assistant persist → no-tools terminal check → loop detection (C2/C2+
/// intervention via QuestionStore). Twelve early-return paths surface as
/// `Err(())` (each emits its own terminal event first — Error / Done
/// cancelled / Done loop_terminated / Done end_turn — then the hub returns).
/// On the normal path returns [`DriveTurnOutcome`] carrying this turn's
/// `tool_calls` + `loop_hint` + the mutated cross-turn state.
///
/// Split off `run_chat_loop` (08-08-a-class-chat-loop-split). No behavior
/// change — pure lift.
pub(crate) struct DriveTurnOutcome {
    pub(crate) tool_calls: Vec<(String, String, serde_json::Value)>,
    pub(crate) loop_hint: Option<String>,
    pub(crate) messages: Vec<ChatMessage>,
    pub(crate) seq: i64,
    pub(crate) head_sha: String,
    pub(crate) system_prompt: String,
    pub(crate) permission_ctx: PermissionContext,
    pub(crate) loop_window: VecDeque<loop_detection::ToolCall>,
    pub(crate) loop_hit_count: u32,
    pub(crate) last_usage_terminal: Option<crate::llm::types::TokenUsage>,
    pub(crate) workflow_ctx: Option<crate::agent::workflow::WorkflowCtx>,
    pub(crate) cancelled: bool,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn drive_turn(
    turn: usize,
    messages: Vec<ChatMessage>,
    seq: i64,
    head_sha: String,
    system_prompt: String,
    permission_ctx: PermissionContext,
    loop_window: VecDeque<loop_detection::ToolCall>,
    loop_hit_count: u32,
    last_usage_terminal: Option<crate::llm::types::TokenUsage>,
    workflow_ctx: Option<crate::agent::workflow::WorkflowCtx>,
    loaded_session: &crate::db::LoadedSession,
    project: crate::projects::ProjectRow,
    worktree_path: PathBuf,
    last_cwd: &Option<PathBuf>,
    current_ctx: &ToolContext,
    mode_prefix: &'static str,
    model_briefs: Vec<crate::agent::subagent::ModelBrief>,
    session_mode: crate::db::Mode,
    effective_is_worker: bool,
    system_prompt_override: &Option<String>,
    tool_defs: Vec<ToolDef>,
    subagent_cache: Arc<crate::agent::subagent::SubagentCache>,
    provider: Arc<dyn Provider>,
    context_window: u32,
    rid: String,
    session_id: String,
    sink: Arc<dyn ChatEventSink>,
    db: SqlitePool,
    token: CancellationToken,
    background_shells: &crate::background_shell::DefaultRegistry,
    skip_persist: bool,
    current_speaker: &Option<String>,
    question_store: &crate::agent::question_store::QuestionStore,
) -> Result<DriveTurnOutcome, ()> {
    let mut messages = messages;
    let mut seq = seq;
    let mut head_sha = head_sha;
    let mut system_prompt = system_prompt;
    let mut permission_ctx = permission_ctx;
    let mut loop_window = loop_window;
    let mut loop_hit_count = loop_hit_count;
    let mut last_usage_terminal = last_usage_terminal;
    let mut workflow_ctx = workflow_ctx;
    permission_ctx.turn_seq = Some(seq);
    // P2 RULE-A-005 (2026-06-24, fix 1 of 3 P2 open rules):
    // refresh `head_sha` + rebuild `system_prompt` at the start of
    // EVERY turn. The LLM only consumes `system_prompt` once per
    // turn (at `provider.send`), so refreshing at turn entry is
    // equivalent to "after every tool execute" — the next
    // `provider.send` (this turn, or the next turn's) sees the
    // current HEAD. Pre-fix: `head_sha` was a one-shot `let` at
    // chat_loop.rs:492 (pre-fix line number), so the LLM saw a
    // stale SHA on turn 2+ even after a tool call committed. The
    // `system_prompt_override` worker path is unchanged: when the
    // 23rd param is `Some(p)`, the worker's
    // `SubagentDef.system_prompt` is the canonical prompt and the
    // parent's per-turn rebuild is skipped (workers don't observe
    // the parent's HEAD anyway — the worker's own lookup is
    // handled inside its nested `run_chat_loop` invocation).
    //
    // Cost: 1 extra `lookup_head_sha` per turn (libgit2
    // `Repository::open` + `head().peel_to_commit()` —
    // sub-millisecond for a local repo, negligible relative to
    // LLM network latency). Memory cache is NOT busted — the
    // instructions blocks live in a separate user-role synthetic
    // message with their own `cache_control: Ephemeral`
    // breakpoint (see prd §6.1 + the `build_instructions_blocks`
    // docstring in `memory/loader.rs`).
    if system_prompt_override.as_ref().is_none() {
        head_sha = crate::agent::system_prompt::lookup_head_sha(&worktree_path);
        let base_prompt = crate::agent::system_prompt::build_system_prompt(
            &loaded_session.session,
            &project,
            &worktree_path,
            &head_sha,
        );
        system_prompt =
            crate::agent::system_prompt::assemble_system_prompt(mode_prefix, &base_prompt);
    }

    // C3 compaction (test pass-through: if messages don't exceed
    // the test's tiny context_window, dropped_count == 0 and
    // the messages vec is unchanged).
    //
    // RULE-A-002 (2026-06-14): `compact_messages` now returns a
    // `DegradationKind` signal. `StillOver` means every safe
    // droppable candidate was exhausted but the budget is still
    // over target — sending the list would 400 on `prompt is
    // too long`. The agent loop emits an `Error` event +
    // terminates the chat instead of silently firing the
    // over-budget request. `None` / `NoCandidates` are safe-to-
    // proceed.
    {
        let compacted =
            crate::agent::context::compact_messages(messages.clone(), context_window).await;
        if compacted.dropped_count > 0 {
            tracing::info!(
                request_id = %rid,
                session_id = %session_id,
                turn,
                tokens_before = compacted.tokens_before,
                tokens_after = compacted.tokens_after,
                dropped_count = compacted.dropped_count,
                context_window,
                "agent loop: context compressed (C3)"
            );
        }
        // E2 trace (2026-07-14): record C3 compaction observation
        // (both normal compaction + StillOver error). Always-on
        // emit + persist; best-effort on the DB write.
        if compacted.dropped_count > 0
            || matches!(
                compacted.degradation,
                crate::agent::context::DegradationKind::StillOver { .. }
            )
        {
            crate::agent::trace::record_compaction(&sink, &db, &rid, &session_id, seq, &compacted)
                .await;
        }
        match compacted.degradation {
            crate::agent::context::DegradationKind::None
            | crate::agent::context::DegradationKind::NoCandidates => {
                messages = compacted.messages;
            }
            crate::agent::context::DegradationKind::StillOver {
                tokens_after,
                target,
            } => {
                // FAIL FAST: surface the over-budget state to
                // the frontend as a typed Error. Do NOT call
                // `provider.send` — the response would 400 on
                // `prompt is too long`. Identical message /
                // tracing / category to production `chat.rs`.
                tracing::error!(
                    request_id = %rid,
                    session_id = %session_id,
                    turn,
                    tokens_after,
                    target,
                    "agent loop: C3 compaction exhausted but still over target — aborting turn"
                );
                let msg = format!(
                    "Context window exceeded after compaction ({} tokens, target {}). \
                     A single tool_result or message may be too large — try a narrower query.",
                    tokens_after, target
                );
                sink.emit_chat_event(&crate::state::ChatEventPayload {
                    request_id: rid.clone(),
                    event: ChatEvent::Error {
                        message: msg,
                        category: LlmErrorCategory::InvalidRequest,
                    },
                });
                return Err(());
            }
        }
    }

    let mut turn_first_delta_at: Option<Instant> = None;
    let mut turn_thinking_start: Option<Instant> = None;
    let mut turn_thinking_done: Option<Instant> = None;
    let mut turn_done_at: Option<Instant> = None;

    // B12 (2026-06-19): ephemeral checklist injection. Each turn,
    // AFTER C3 compaction and BEFORE `provider.send`, if the
    // checklist Vec is non-empty, build a synthetic user block
    // carrying the full current list + an explicit "in progress"
    // focus marker, and APPEND it to a CLONE of `messages`. The
    // clone is the request body; the persisted `messages` Vec is
    // NEVER mutated by this injection — the block is regenerated
    // from the live Vec every turn.
    //
    // Why APPEND (not prepend)?
    // - **Cache correctness (load-bearing):** the memory
    //   instructions block lives at `messages[0]` and carries a
    //   `cache_control: Ephemeral` breakpoint on its banner block
    //   (see `memory/loader.rs::build_instructions_blocks`). The
    //   breakpoint is part of Anthropic's cache key — everything
    //   BEFORE it must be byte-identical across turns to hit. A
    //   per-turn-mutating checklist block at position 0 would
    //   sit IN FRONT of the memory breakpoint, busting the memory
    //   cache every turn (50 turns × ~100 KiB of instruction
    //   files = the exact cost explosion the B5 memory-caching
    //   work was built to eliminate). Appending keeps the
    //   checklist AFTER the memory breakpoint so the memory cache
    //   window stays intact. This mirrors why the B4 skill block
    //   was placed AFTER the memory pair (position 2), not at
    //   the head — same cache-preservation principle.
    // - Anthropic accepts consecutive user-role messages, so
    //   appending a user block after the user's latest prompt is
    //   wire-legal.
    // - The checklist content being the LAST thing in context is
    //   arguably better for recency: the model sees its current
    //   todo right before generating.
    //
    // Why not push into `messages` (the persisted Vec)?
    // - Replay correctness: the canonical checklist state lives
    //   in the `update_checklist` tool_results (persisted in
    //   history). A reload reconstructs from those tool_results;
    //   an injection block in `messages` would be a duplicate
    //   source of truth that drifts the moment the Vec changes.
    // - Context window: each turn's injection is per-turn-only;
    //   keeping it out of `messages` keeps the persisted history
    //   lean.
    //
    // No `cache_control` on the checklist block itself: the block
    // changes every turn (the LLM mutates the list), so a cache
    // breakpoint would never hit.
    //
    // Empty Vec (turn 1, before any `update_checklist` call) →
    // skip injection entirely, symmetric to memory/skill empty-
    // skip. We use the same `messages.clone()` for `provider.send`
    // whether or not we injected, so the non-checklist path is a
    // single extra `.clone()` per turn (cheap relative to LLM
    // network latency).

    // L1a (2026-06-19): drain completion notifications from the
    // background-shell registry. Each notification is appended
    // as a `user`-role message at the END of the request clone
    // (mirroring the checklist injection rule: APPEND, not
    // prepend, so the memory cache breakpoint at `messages[0]`
    // stays intact — see `.trellis/spec/backend/tool-contract.md`
    // §7 "Wrong vs Correct — injection placement"). The agent
    // loop drains ONCE per turn (not per tool_use): background
    // tasks may complete between turns, but the queue is
    // consumed on the next turn's request. Drained notifications
    // are GONE from the registry (drain_notifications is
    // destructive — see `background_shell::BackgroundShellRegistry`).
    //
    // Each notification produces ONE user message; the LLM tracks
    // multiple completions more reliably when they're separated
    // (a single merged message risks being read as a single
    // event with garbled exit codes).
    //
    // Format (per L1 PRD Q3 + Q4 decisions):
    //   `[system] 后台 shell <shell_session_id> 已完成,exit code <N>。调 shell_status(session_id="<id>") 看输出。`
    // Notifications are kept lean — only exit code + session id;
    // the LLM calls `shell_status` to pull stdout/stderr. Keeps
    // the per-turn context cost bounded for builds that fan out
    // into many background shells.
    let background_notifications = background_shells.drain_notifications(&session_id).await;
    let turn_messages = {
        let checklist_snapshot = current_ctx.checklist.lock().await.clone();
        let mut req = messages.clone();
        if !checklist_snapshot.is_empty() {
            let block = crate::tools::update_checklist::render_checklist(&checklist_snapshot);
            let text = format!(
                "<current-checklist>\nThis is your running progress checklist for the current task. \
                 Items marked `[~]` are in progress; `[x]` are done; `[ ]` are pending. Use the \
                 `update_checklist` tool to mark items done / add new items / reorder as your plan \
                 evolves. The list is re-injected every turn so you don't lose track.\n{}\n</current-checklist>",
                block
            );
            let checklist_msg = ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![ContentBlock::Text {
                    text,
                    cache_control: None,
                }]),
                speaker: None,
            };
            // APPEND, never prepend — see cache-correctness note
            // above. Prepending would bust the memory cache
            // breakpoint at messages[0].
            req.push(checklist_msg);
        }
        // L1a notifications: APPEND after the (optional)
        // checklist block. Same cache-correctness rule — keep
        // the memory breakpoint at messages[0] intact. Each
        // notification gets ONE message so the LLM sees
        // multiple completions as distinct events.
        for note in &background_notifications {
            let text = format!(
                "[system] 后台 shell {} 已完成,exit code {}。调 shell_status(session_id=\"{}\") 看输出。",
                note.shell_session_id,
                note.exit_code
                    .map(|c: i32| c.to_string())
                    .unwrap_or_else(|| "N/A".to_string()),
                note.shell_session_id,
            );
            let msg = ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![ContentBlock::Text {
                    text,
                    cache_control: None,
                }]),
                speaker: None,
            };
            req.push(msg);
        }
        // P2 (2026-06-29): autonomous-memory session-start
        // recall. Per-turn (PRD decision 6): query = the most
        // recent user message text. The recall text is
        // appended to the instruction message's block list
        // (messages[0]) in the REQUEST clone — the persisted
        // `messages` Vec is byte-identical across turns (the
        // recall block is per-turn-only, like the B12
        // checklist). The banner + instruction-body prefix
        // stays stable so the Anthropic cache window stays
        // warm. See `agent::memory_recall` for the cache-
        // correctness rationale + the candidate-inclusion
        // ADR-lite (P5 tightens back to active/verified).
        //
        // Skip when `skip_persist` (worker path): the worker
        // reuses the parent's session_id, and surfacing the
        // parent's memories in the worker's context would
        // (a) confuse the worker's focused task and (b) bump
        // hit_count on rows the worker didn't actually
        // contribute to. The worker has its own context.
        if !skip_persist {
            let query = messages
                .iter()
                .rev()
                .find(|m| m.role == Role::User)
                .map(|m| m.content.to_text())
                .unwrap_or_default();
            if !query.trim().is_empty() {
                // 07-06 (am-observability-panel R2b / A8): use
                // the rows-aware sibling so we can emit a
                // `ChatEvent::Recall` after the recall block
                // is appended. The original 4 P2 unit tests
                // keep working because `build_recall_text` is
                // a thin wrapper that drops the rows.
                if let Some((recall_text, recall_rows)) =
                    crate::agent::memory_recall::build_recall_text_with_rows(
                        &db,
                        &project.id,
                        &query,
                    )
                    .await
                {
                    crate::agent::memory_recall::inject_recall_into_turn(&mut req, recall_text);
                    // Emit the R2b chat-event for the frontend's
                    // "本次召回" chip. Worker sink
                    // (SubagentBufferSink) does not forward
                    // to the IPC channel — AC7 is enforced by
                    // the sink abstraction, not by an extra
                    // check here.
                    crate::agent::memory_recall::emit_recall_event(
                        sink.as_ref(),
                        &rid,
                        &recall_rows,
                        "fts",
                    );
                }
            }

            // W1 (Workflow integration, Phase 0 Step 0.5
            // — 2026-07-08): per-turn breadcrumb injection.
            // Sibling to the recall-injection block above
            // (not nested inside `if
            // !query.trim().is_empty()`) because the
            // breadcrumb is unconditional on recall state
            // — even when there's no user query, a
            // workflow session opening a new turn still
            // wants the state breadcrumb in front of the
            // LLM.
            //
            // Runs AFTER `inject_recall_into_turn` so the
            // recall text (when present) lands at the
            // head of `messages[0]`'s block list
            // (chronologically first per-turn) and the
            // breadcrumb sits just below it
            // (chronologically last).
            //
            // Both injectors share `messages[0]` and rely
            // on the SAME S-B guard (skip-not-prepend);
            // see
            // `agent::workflow::inject::append_workflow_breadcrumb`
            // for the rationale.
            //
            // Nested inside `if !skip_persist` because
            // workers reuse the parent's session_id and
            // the parent's workflow state is NOT what
            // the worker should be reminded of. Workers
            // call this function with `workflow_ctx =
            // None`, so the inner `if workflow_ctx` gate
            // would also short-circuit — keeping them
            // together makes the intent clear in one
            // block ("non-worker path mutations on
            // messages[0]").
            // R4 (07-10-workflow-task-json-hardening): refresh
            // `current_task` off disk at turn top so the breadcrumb
            // reflects mid-loop state changes from the previous turn
            // (a transition the user allowed, or a create_task tool
            // call). `workflow_ctx` is an owned `Option` param (mut-
            // bound); the mut borrow ends at the block's `}`, before
            // the `append_workflow_breadcrumb` read below — so the
            // immutable re-borrow there is fine. Only workflow
            // sessions have a ctx to refresh; non-workflow stays None.
            if let Some(ref mut ctx) = workflow_ctx {
                ctx.current_task = crate::agent::workflow::inject::resolve_current_task(
                    &current_ctx.worktree_path,
                )
                .await;
            }
            if let Some(ref ctx) = workflow_ctx {
                let appended =
                    crate::agent::workflow::inject::append_workflow_breadcrumb(&mut req, ctx);
                // E2 trace (2026-07-14): record breadcrumb snapshot.
                // Lives in chat_loop (not inject.rs) so it has access
                // to seq + db + sink. Only fires when the breadcrumb
                // was actually appended (S-B guard passed).
                if appended {
                    let slug = ctx.current_task.as_ref().map(|t| t.slug.clone());
                    let status = ctx
                        .current_task
                        .as_ref()
                        .map(|t| t.status.as_str().to_string());
                    let text = crate::agent::workflow::inject::breadcrumb_body(ctx);
                    crate::agent::trace::record_breadcrumb(
                        &sink,
                        &db,
                        &rid,
                        &session_id,
                        seq,
                        slug.as_deref(),
                        status.as_deref(),
                        &text,
                    )
                    .await;
                }
            }
        }
        req
    };

    let mut turn_tool_defs = crate::tools::filter_tools_for_workflow(
        permissions::filter_tools_for_mode(tool_defs.clone(), session_mode),
        workflow_ctx.is_some(),
    );
    // L3d (2026-06-25): append the dynamic `dispatch_subagent`
    // ToolDef so the enum reflects builtin + user + project
    // subagents merged by `SubagentCache` (mtime-fenced scan).
    // The static `dispatch_subagent` definition is no longer in
    // `builtin_tools()` (it would freeze the enum at startup);
    // we rebuild it here every turn so a freshly-written `.md`
    // is picked up on the next chat turn. `filter_tools_for_mode`
    // keeps dispatch_subagent in every mode (it is a
    // `Risk::Low` discovery tool — the worker's actual writes /
    // shells go through their own Tier 4 permission check).
    //
    // WORKER NESTING GUARD (permission-layer.md §"Subagent
    // availability"): a worker (`effective_is_worker == true`)
    // MUST NOT see `dispatch_subagent` in its turn tool list.
    // The B6 `filter_tools_for_subagent` strips
    // `dispatch_subagent` from the worker's *initial*
    // `worker_tool_defs` (`dispatch/prepare.rs::prepare_worker`), but that filter
    // only applies to the seed list — this per-turn append runs
    // inside the nested `run_chat_loop` and would otherwise
    // re-introduce the ToolDef on every turn, defeating the
    // `STRUCTURALLY_DISABLED` no-nesting invariant. Skip the
    // append when we are inside a worker run.
    //
    // `worktree_path` is in scope from `run_chat_loop`'s top-level
    // session load (canonicalized via `assert_within_root`) — it
    // matches what `MemoryCache` / `SkillCache` use, so the
    // subagent `<project>/.everlasting/agents/*.md` dir lines up
    // with the project's other namespace dirs.
    if !effective_is_worker {
        let project_path = worktree_path.to_string_lossy().to_string();
        // C5: thread the active plugin name so plugin-layer agents
        // (e.g. review's `reviewer`) reach the dispatch enum.
        let workflow_name = workflow_ctx.as_ref().map(|c| c.workflow_def.name.as_str());
        let dispatch_def = crate::agent::subagent::definition_with_cache(
            &subagent_cache,
            &project_path,
            workflow_name,
            &model_briefs,
        )
        .await;
        turn_tool_defs.push(dispatch_def);
    }
    let turn_tool_defs = turn_tool_defs;
    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<(String, String, serde_json::Value)> = Vec::new();
    let mut finalized_thinking: Vec<(String, String)> = Vec::new();
    let mut pending_thinking: Option<PendingThinking> = None;
    // 交错思考(interleaved thinking): `ordered_blocks` 按 LLM 真实
    // 流式到达顺序累积 ContentBlock,落库时用它替代旧的"按类型分桶
    // 硬编码排序"(thinking→text→tool_use→redacted)。这样 DB 里
    // 保留 [think→text→tool] 的真实流序,reload 后前端可据此渲染
    // 出 Claude.ai/Cursor 式的连续流动形态。
    //
    // 配套的 `pending_text` 是"当前正在累积的文本块"——`Delta` 事件
    // 是逐段来的,不能每段都 push 一个 Text 块(会产生碎片)。遇到
    // 非文本边界(thinking flush / tool call / redacted / turn end)
    // 时把 pending_text flush 成一个 Text 块,实现"文本与思考/工具
    // 按真实顺序交错"。多个相邻 Text 块在语义上等价(Anthropic 接受
    // 多 Text 块),但保留了"思考夹在两段文本之间"这种流序信息。
    //
    // 红线:`ToolResult` 永远不进 `ordered_blocks`(它只进 user-role
    // message,见 §1.1 ToolResult 边界)。本累加器只装 assistant
    // 允许的 Thinking/Text/ToolUse/RedactedThinking。
    let mut ordered_blocks: Vec<ContentBlock> = Vec::new();
    let mut pending_text: Option<String> = None;
    let mut stop_reason: Option<String> = None;
    let mut last_usage: Option<crate::llm::types::TokenUsage> = None;
    let mut had_error = false;
    let mut cancelled = false;
    // A5+ (2026-07-04): first-byte-safe retry around `provider.send`.
    // See llm/retry.rs + docs/research/llm-network-resilience-survey.md.
    // On retryable first-byte failure (Network/Server/RateLimit) the
    // request is re-issued with Full Jitter backoff, bounded by budget;
    // the instant any Ok event arrives retry stops (prd R3 — tools only
    // execute after the stream completes, so pre-first-byte retry is
    // provably side-effect-free, no idempotency key needed).
    let mut rng = fastrand::Rng::new();
    let retry_sink = LlmRetrySink {
        sink: sink.clone(),
        rid: rid.clone(),
    };
    let outcome = retry_open(
        provider.as_ref(),
        Some(system_prompt.clone()),
        turn_messages,
        turn_tool_defs,
        &RetryPolicy::default(),
        &token,
        &retry_sink,
        &mut rng,
    )
    .await;
    // P2 RULE-A-009: `turn_send_at` marks when the LLM stream became
    // ready (post-retry). The other 4 `turn_*_at` vars stay declared at
    // the top of the loop body (conditionally assigned; `None` default
    // is load-bearing for `is_none()` checks).
    let turn_send_at = Some(Instant::now());
    let mut stream: Pin<
        Box<
            dyn futures_util::Stream<
                    Item = Result<crate::llm::types::ChatEvent, crate::llm::error::LlmError>,
                > + Send,
        >,
    > = match outcome {
        OpenOutcome::Stream(s) => s,
        OpenOutcome::Cancelled => {
            // Retry gave up because the user cancelled during open
            // or backoff. Set the cancel flag and feed an empty
            // stream so the per-event loop below exits immediately
            // (None arm); the post-loop persist handles
            // CANCELLED_MARKER. The biased `token.cancelled()` arm
            // also fires on the first select iteration as backup.
            cancelled = true;
            Box::pin(futures_util::stream::empty())
        }
    };

    loop {
        tokio::select! {
            biased;
            _ = token.cancelled() => {
                tracing::info!(request_id = %rid, "chat: cancellation requested by client");
                cancelled = true;
                break;
            }
            event_result = stream.next() => {
                let Some(event_result) = event_result else { break; };
                let event = match event_result {
                    Ok(e) => e,
                    // RULE-A-011 (2026-06-19): previously this arm
                    // silently wrapped `LlmError` into a
                    // `ChatEvent::Error` with NO tracing. The
                    // 2026-06-18 incident (`mz8s3hqwx6rmqjswgte`,
                    // messages.seq=37) hit exactly this: the
                    // reqwest 60s total-deadline fired mid-
                    // thinking, the partial turn was persisted,
                    // and the user saw a toast with no Rust-side
                    // breadcrumb. Add `tracing::warn!` so the
                    // next streaming failure is grep-able.
                    // See `.trellis/spec/backend/error-handling.md`
                    // §RULE-A-011.
                    Err(err) => {
                        tracing::warn!(
                            request_id = %rid,
                            turn,
                            // `LlmErrorCategory` only derives Debug
                            // (not Display), so use `?` (Debug)
                            // instead of `%` (Display) — produces the
                            // same five variant names (Auth /
                            // RateLimit / InvalidRequest / Server /
                            // Network) for grep purposes.
                            category = ?err.category(),
                            error = %err,
                            "chat: LLM stream errored"
                        );
                        ChatEvent::Error {
                            message: err.user_message(),
                            category: err.category(),
                        }
                    }
                };
                match &event {
                    ChatEvent::Start => {
                        emit_chat_event_via_sink(&sink, &rid, &event);
                    }
                    ChatEvent::Delta { text } => {
                        // 流序: 文本到达前先把可能 pending 的 thinking
                        // finalize,并按真实顺序(thinking 在前)填入
                        // ordered_blocks。文本累积到 pending_text,
                        // 等下一个非文本边界再 flush 成 Text 块。
                        flush_pending_thinking(&mut pending_thinking, &mut finalized_thinking);
                        flush_ordered_thinking(&mut finalized_thinking, &mut ordered_blocks);
                        pending_text.get_or_insert_with(String::new).push_str(text);
                        text_parts.push(text.clone());
                        if turn_first_delta_at.is_none() {
                            turn_first_delta_at = Some(Instant::now());
                        }
                        if turn_thinking_start.is_some() && turn_thinking_done.is_none() {
                            turn_thinking_done = Some(Instant::now());
                        }
                        emit_chat_event_via_sink(&sink, &rid, &event);
                    }
                    ChatEvent::ThinkingDelta { text } => {
                        // 流序: 一个 thinking 块开始前,先把之前累积的
                        // 文本 flush 成 Text 块(思考夹在文本之间时,
                        // 前段文本应排在思考之前)。
                        flush_pending_text(&mut pending_text, &mut ordered_blocks);
                        let p = pending_thinking.get_or_insert_with(PendingThinking::default);
                        p.text.push_str(text);
                        if turn_thinking_start.is_none() {
                            turn_thinking_start = Some(Instant::now());
                        }
                        emit_chat_event_via_sink(&sink, &rid, &event);
                    }
                    ChatEvent::SignatureDelta { signature } => {
                        let p = pending_thinking.get_or_insert_with(PendingThinking::default);
                        p.signature.push_str(signature);
                        emit_chat_event_via_sink(&sink, &rid, &event);
                    }
                    ChatEvent::RedactedThinkingDelta { data } => {
                        // 流序: redacted 到达前先 flush 可能 pending 的
                        // thinking + text,保持顺序。
                        flush_pending_thinking(&mut pending_thinking, &mut finalized_thinking);
                        flush_ordered_thinking(&mut finalized_thinking, &mut ordered_blocks);
                        flush_pending_text(&mut pending_text, &mut ordered_blocks);
                        ordered_blocks.push(ContentBlock::RedactedThinking { data: data.clone() });
                        emit_chat_event_via_sink(&sink, &rid, &event);
                    }
                    ChatEvent::ToolCall { id, name, input } => {
                        // 流序: 工具调用前先 flush pending thinking + text。
                        flush_pending_thinking(&mut pending_thinking, &mut finalized_thinking);
                        flush_ordered_thinking(&mut finalized_thinking, &mut ordered_blocks);
                        flush_pending_text(&mut pending_text, &mut ordered_blocks);
                        if turn_thinking_start.is_some() && turn_thinking_done.is_none() {
                            turn_thinking_done = Some(Instant::now());
                        }
                        tool_calls.push((id.clone(), name.clone(), input.clone()));
                        ordered_blocks.push(ContentBlock::ToolUse {
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                        });
                        sink.emit_tool_call(&ToolCallPayload {
                            request_id: rid.clone(),
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                        });
                    }
                    ChatEvent::Done { stop_reason: sr, usage } => {
                        stop_reason = sr.clone();
                        last_usage = *usage;
                        // 2026-06-21 (R3): mirror the per-turn
                        // `last_usage` to the function-scope
                        // `last_usage_terminal` so the
                        // synthetic `max_turns` terminal site
                        // (chat_loop.rs:1797-1820) can forward
                        // it to the sink. The sink's R3 guard
                        // ensures the value reaches
                        // `cumulative_usage()` exactly once
                        // (no double-count). Pre-R3 this
                        // mirror did not exist; the terminal
                        // hard-coded `usage: None`, which
                        // produced the all-zero
                        // `subagent_runs.token_usage_json`
                        // regression.
                        last_usage_terminal = *usage;
                        turn_done_at = Some(Instant::now());
                        if turn_thinking_start.is_some() && turn_thinking_done.is_none() {
                            turn_thinking_done = Some(Instant::now());
                        }
                        if let Some(t) = usage {
                            // 2026-06-26 (token-usage snapshot fix +
                            // RULE-A-015 reversal): the per-turn
                            // `update_last_turn_usage` is now BACK
                            // inside the `!skip_persist` gate.
                            //
                            // PR1b originally gated this under
                            // `!skip_persist`. PR2a (RULE-A-015)
                            // pulled it OUT, citing "token-usage
                            // metadata lives on `sessions`, not
                            // `messages`, so the worker should
                            // still stream its per-turn usage into
                            // the parent's accumulator." That was
                            // correct under the A4 cumulative
                            // model — the worker's tokens added to
                            // the parent's running total.
                            //
                            // The snapshot model reverses this.
                            // `update_last_turn_usage` OVERWRITES
                            // the parent's `last_*` columns (not
                            // accumulates). If the worker (which
                            // reuses the parent's `session_id` —
                            // see dispatch.rs) ran unguarded, every
                            // worker turn would OVERWRITE the
                            // parent's snapshot with worker
                            // numbers. The parent UI would
                            // oscillate between parent-turn and
                            // worker-turn values; on a multi-worker
                            // dispatch the last-writer-wins
                            // outcome would be arbitrary. Worker
                            // token usage stays isolated in
                            // `subagent_runs.token_usage_json`
                            // (written at worker exit by
                            // `dispatch.rs::run_subagent`).
                            if !skip_persist {
                                if let Err(e) = crate::db::update_last_turn_usage(&db, &session_id, t).await {
                                    tracing::warn!(error = %e, "chat: failed to update last-turn token usage (non-fatal)");
                                }
                                // E2 trace (2026-07-14): persist per-turn
                                // token usage to turn_trace (worker-gated
                                // by !skip_persist, same as
                                // update_last_turn_usage — RULE-A-015).
                                if let Err(e) = crate::db::trace::upsert_turn_trace_token(&db, &session_id, seq, t).await {
                                    tracing::warn!(error = %e, "trace: upsert_turn_trace_token failed (non-fatal)");
                                }
                            }
                        }
                    }
                    ChatEvent::Error { .. } => {
                        if turn_thinking_start.is_some() && turn_thinking_done.is_none() {
                            turn_thinking_done = Some(Instant::now());
                        }
                        emit_chat_event_via_sink(&sink, &rid, &event);
                        had_error = true;
                    }
                    ChatEvent::TurnComplete { .. } => {
                        tracing::warn!(request_id = %rid, "chat: unexpected TurnComplete in LLM stream");
                    }
                    // B2 PR3: `FileInjections` is emitted ONCE per
                    // user turn from the agent loop's pre-turn
                    // hook (right after `inject_at_tokens` runs) —
                    // NOT from the LLM stream. A `FileInjections`
                    // arriving inside the per-event stream loop
                    // would mean the wire shape leaked (e.g. a
                    // provider re-emitted it). Drop it; the
                    // controller already received the legitimate
                    // one above.
                    ChatEvent::FileInjections { .. } => {
                        tracing::warn!(
                            request_id = %rid,
                            "chat: unexpected FileInjections in LLM stream (ignoring — already emitted pre-turn)"
                        );
                    }
                    // A5+ (2026-07-04): `Retrying` is emitted
                    // directly by `LlmRetrySink` (NOT via this
                    // per-event stream loop), so reaching this
                    // arm means a provider somehow re-emitted a
                    // retrying notice we already pushed to the
                    // frontend. Drop it (the legitimate one
                    // already shipped via `LlmRetrySink::emit_retrying`).
                    ChatEvent::Retrying { .. } => {
                        tracing::warn!(
                            request_id = %rid,
                            "chat: unexpected Retrying in LLM stream (ignoring — emitted via LlmRetrySink)"
                        );
                    }
                    // 07-06 (am-observability-panel R2b): `Recall`
                    // is emitted via `emit_recall_event` at the
                    // recall-injection seam (FTS) and the
                    // pre-tool pitfall seam (per tool_use).
                    // Reaching this arm means the LLM stream
                    // somehow re-emitted a recall notice we
                    // already pushed. Drop it (the legitimate
                    // one is on the chat-event channel; the
                    // controller will dedup by `rid`).
                    ChatEvent::Recall { .. } => {
                        tracing::warn!(
                            request_id = %rid,
                            "chat: unexpected Recall in LLM stream (ignoring — emitted via emit_recall_event)"
                        );
                    }
                    // 08-04 group-chat follow-up: `Speaker` is
                    // emitted by the orchestrator
                    // (`run_group_chat_loop`) before each inner
                    // speaker turn — NOT by a provider. Reaching
                    // this arm means the wire shape leaked. Drop
                    // it (the controller already received the
                    // legitimate one on the chat-event channel).
                    ChatEvent::Speaker { .. } => {
                        tracing::warn!(
                            request_id = %rid,
                            "chat: unexpected Speaker in LLM stream (ignoring — emitted by group_chat orchestrator)"
                        );
                    }
                    // E2 trace (2026-07-14): the 3 trace events are
                    // emitted by `agent::trace::record_*` (NOT by the
                    // LLM stream), so reaching this arm means a
                    // provider somehow re-emitted a trace event we
                    // already pushed. Drop it (same pattern as
                    // `Recall` / `Retrying` / `FileInjections`).
                    ChatEvent::ContextCompacted { .. }
                    | ChatEvent::LoopHint { .. }
                    | ChatEvent::WorkflowBreadcrumb { .. } => {
                        tracing::warn!(
                            request_id = %rid,
                            "chat: unexpected trace event in LLM stream (ignoring)"
                        );
                    }
                }
                if matches!(event, ChatEvent::Done { .. } | ChatEvent::Error { .. }) {
                    break;
                }
            }
        }
    }

    // RULE-A-007 (2026-06-17): the error path no longer bails
    // out with raw `return`. Instead — symmetric with the
    // cancel path below — the agent loop flushes any pending
    // thinking, appends an `ERROR_MARKER` to the text, and
    // persists the partial turn so a reload shows the user
    // where the turn broke. Previously the error arm returned
    // immediately, dropping already-rendered
    // `text_parts` / `finalized_thinking` / `tool_calls`
    // — an asymmetry vs the cancel path that did persist.
    if cancelled {
        flush_pending_thinking(&mut pending_thinking, &mut finalized_thinking);
        tracing::info!(
            request_id = %rid,
            "chat: cancelled — persisting partial turn"
        );
    } else if had_error {
        flush_pending_thinking(&mut pending_thinking, &mut finalized_thinking);
        tracing::info!(
            request_id = %rid,
            "chat: errored — persisting partial turn"
        );
    }

    flush_pending_thinking(&mut pending_thinking, &mut finalized_thinking);

    // 交错思考: 落库用 `ordered_blocks`(按真实流序累积),替代旧的
    // "按类型分桶硬编码排序"。这里做 turn-end 兜底 flush:
    // 1. 把所有已 finalize 的 thinking 按序填入
    // 2. 把最后一段 pending_text flush 成一个 Text 块
    // 之后再追加 cancel/error marker(独立 Text 块,见下)。
    // `finalized_thinking` / `pending_text` 在循环内的每个
    // 非文本边界已被增量 flush 过,这里只兜底"turn 结束时仍
    // pending 的尾部"(正常 turn 的最后一段文本/思考)。
    flush_ordered_thinking(&mut finalized_thinking, &mut ordered_blocks);
    flush_pending_text(&mut pending_text, &mut ordered_blocks);

    // RULE-A-007 (2026-06-17) + 交错思考调整: cancel/error marker
    // 追加为一个**独立 Text 块**到 ordered_blocks 末尾(而非旧
    // 逻辑里追加到 `full_text` 字符串内)。语义保持等价:
    //   - 空 turn(无文本) → 只有 marker 一个 Text 块
    //   - 非空 turn → 前段文本块 + marker 块
    // marker 文本带 `\n\n` 前缀(非空时),使 `to_text()` 把多个
    // Text 块 join 后的字符串与旧逻辑(`full_text + "\n\n" + marker`)
    // 完全一致 —— 前端用 `includes`/`endsWith` 识别 marker
    // (chat.ts ERROR_MARKER_LOCAL)的逻辑不受影响。
    // marker 作为独立块,渲染层未来可选择单独样式(对齐 §6.2)。
    let had_text = !text_parts.is_empty();
    if cancelled {
        let marker = if had_text {
            format!("\n\n{}", CANCELLED_MARKER)
        } else {
            CANCELLED_MARKER.to_string()
        };
        ordered_blocks.push(ContentBlock::Text {
            text: marker,
            cache_control: None,
        });
    } else if had_error {
        let marker = if had_text {
            format!("\n\n{}", ERROR_MARKER)
        } else {
            ERROR_MARKER.to_string()
        };
        ordered_blocks.push(ContentBlock::Text {
            text: marker,
            cache_control: None,
        });
    }

    // `assistant_blocks` 直接复用流序累积的 `ordered_blocks`。
    // 旧的分桶循环(thinking→text→tool_use→redacted 硬编码)已删除
    // —— 所有块在循环内已按真实到达顺序填入。
    let assistant_blocks = ordered_blocks;

    if !assistant_blocks.is_empty() {
        let msg = ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Blocks(assistant_blocks),
            // Group chat (Phase 4 TODO-A): carry the originating
            // speaker into the assistant persist site. `None` for
            // classic chat / subagent / review (no behavior change
            // vs. pre-Phase 4); `Some("moderator" | participant.name)`
            // for group chat turns. The DB column is nullable
            // (Phase 1 migration) so existing rows / sessions are
            // unaffected.
            speaker: current_speaker.clone(),
        };
        let turn_latency = build_turn_latency(
            turn_send_at,
            turn_first_delta_at,
            turn_thinking_start,
            turn_thinking_done,
            turn_done_at,
        );
        // RULE-A-003 (2026-06-15): assistant turn persist
        // failure → emit Error + abort. Previously this was a
        // silent log, but the `messages.push` + `seq += 1`
        // below it still ran, drifting the in-memory seq out
        // of sync with the DB. TurnComplete stays on the
        // success path only (unchanged).
        //
        // RULE-A-007 (2026-06-17): on the **error path**,
        // persist failure is log-only (NOT
        // `emit_persist_failure`). The loop already emitted a
        // terminal `ChatEvent::Error` from the per-event arm
        // at line ~598; emitting a second Error here would be
        // a conflicting double-terminal event. The pattern
        // mirrors the cancel path's synthetic tool_result
        // persist (log-only, see below at the `if cancelled`
        // block).
        if !skip_persist {
            if let Err(e) = crate::db::persist_turn(
                &db,
                &session_id,
                msg.role,
                &msg.content,
                seq,
                Some(&turn_latency),
                msg.speaker.as_deref(),
            )
            .await
            {
                if had_error {
                    tracing::error!(
                        error = %e,
                        request_id = %rid,
                        "failed to persist errored partial assistant turn (log-only — Error already emitted)"
                    );
                    return Err(());
                } else {
                    emit_persist_failure(&sink, &rid, &e);
                    return Err(());
                }
            }
        }
        // TurnComplete fires on the success path for every
        // mode (normal / cancel / error). The error path's
        // TurnComplete coexists with the pre-emit Error event
        // (RULE-A-007 decision C): Error = "something went
        // wrong", TurnComplete = "this seq's partial turn is
        // now in the DB + here's the latency breakdown". The
        // controller routes each event independently. In the
        // worker path (skip_persist=true) we skip the
        // TurnComplete emit too — the parent never sees the
        // worker's internal turn sequence, only the final
        // dispatch_subagent tool_result.
        if !skip_persist {
            emit_chat_event_via_sink(
                &sink,
                &rid,
                &ChatEvent::TurnComplete {
                    seq,
                    ttfb_ms: turn_latency.ttfb_ms,
                    gen_ms: turn_latency.gen_ms,
                    total_ms: turn_latency.total_ms,
                    thinking_ms: turn_latency.thinking_ms,
                },
            );
        }
        messages.push(msg);
        seq += 1;
    }

    if cancelled {
        if !tool_calls.is_empty() {
            let tool_result_msg = build_synthetic_tool_result_message(&tool_calls);
            // B6 PR1b: skip the synthetic tool_result persist in
            // worker mode (the worker's intermediate turn is
            // captured by the SubagentBufferSink transcript).
            if !skip_persist {
                // RULE-A-003 (2026-06-15): cancel path —
                // log-only, NOT emit_persist_failure. The loop
                // is about to emit its terminal cancelled `Done`;
                // an Error here would be a second terminal event
                // conflicting with it. The user already knows
                // they cancelled.
                if let Err(e) = crate::db::persist_turn(
                    &db,
                    &session_id,
                    tool_result_msg.role,
                    &tool_result_msg.content,
                    seq,
                    None,
                    None,
                )
                .await
                {
                    tracing::error!(error = %e, "failed to persist synthetic tool_result turn after cancel");
                }
            }
            messages.push(tool_result_msg);
        }
        if !skip_persist {
            persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
            let _ = crate::db::touch_session(&db, &session_id).await;
        }
        // B6 PR1b: always emit terminal `Done { cancelled }` —
        // the SubagentBufferSink reads it to set `was_cancelled`
        // (so `run_subagent` can format the dispatch_subagent
        // tool_result with `status=cancelled`).
        emit_chat_event_via_sink(
            &sink,
            &rid,
            &ChatEvent::Done {
                stop_reason: Some("cancelled".to_string()),
                usage: None,
            },
        );
        return Err(());
    }

    // RULE-A-007 (2026-06-17): the error path persisted its
    // partial assistant turn above (with ERROR_MARKER + a
    // TurnComplete event). The loop has already emitted its
    // terminal `ChatEvent::Error` from the per-event arm;
    // emitting another terminal `Done` would conflict. Exit
    // without further tool execution / next-turn dispatch —
    // symmetric with the cancel `return` above. The frontend
    // treats the Error event as terminal; no follow-up Done
    // is required.
    if had_error {
        // Symmetric with the cancel path above (chat_loop.rs
        // ~1457): if the model emitted tool_use before the
        // stream errored, the assistant(tool_use) turn pushed
        // at line ~1453 would be orphaned (no matching
        // tool_result) → the next request fails upstream with
        // HTTP 400 "insufficient tool messages following
        // tool_calls" (OpenAI) / 2013 (Anthropic). Push one
        // synthetic is_error tool_result per emitted tool_use
        // so the pair stays atomic (llm-contract.md §469).
        // Persist is log-only (RULE-A-007 decision B): the
        // terminal Error already fired, a persist failure here
        // must not emit a second terminal.
        if !tool_calls.is_empty() {
            let tool_result_msg = build_synthetic_tool_result_message(&tool_calls);
            if !skip_persist {
                if let Err(e) = crate::db::persist_turn(
                    &db,
                    &session_id,
                    tool_result_msg.role,
                    &tool_result_msg.content,
                    seq,
                    None,
                    None,
                )
                .await
                {
                    tracing::error!(
                        error = %e,
                        "failed to persist synthetic tool_result turn after error"
                    );
                }
            }
            messages.push(tool_result_msg);
        }
        // B6 PR1b: skip the cwd/touch_session writes in worker
        // mode (the parent's session row is not the worker's
        // to update — the parent owns the lifetime).
        if !skip_persist {
            persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
            let _ = crate::db::touch_session(&db, &session_id).await;
        }
        return Err(());
    }

    // 08-07-group-chat-role-history-isolation follow-up fix: the
    // pre-fix predicate required `stop_reason == Some("tool_use")`
    // in addition to a non-empty `tool_calls`. That is too strict:
    // an OpenAI-compatible provider (Console Go) can end a stream
    // that emitted tool_calls with a DIFFERENT finish_reason
    // (e.g. "stop" → normalized "end_turn"; some providers even
    // omit it). The old predicate then took the `!should_continue`
    // return below — the assistant(tool_use) row was already
    // persisted, but the tools were never executed and no
    // tool_result was persisted → the next reload built a context
    // with an orphan assistant(tool_calls) (llm-contract.md §469
    // violated) → every subsequent provider call 400'd with
    // "An assistant message with 'tool_calls' must be followed by
    // tool messages responding to each 'tool_call_id'" and the
    // group chat died after MAX_ORCHESTRATION_ROUNDS of
    // [生成出错中断] retries (DB session d7fe451c: seq 5 emitted 3
    // read_file tool_uses, zero tool_results, 26 consecutive
    // error turns).
    //
    // The correct signal is the tool_calls themselves: if the
    // model emitted ANY tool_use, they MUST be executed and their
    // results fed back (otherwise the pair is broken for every
    // future request). stop_reason is informational only — it
    // decides the terminal `Done` value, not whether tools run.
    let should_continue = !tool_calls.is_empty();

    if !should_continue {
        // B6 PR1b: skip the cwd/touch_session writes in worker
        // mode (the parent's session row is not the worker's
        // to update — the parent owns the lifetime).
        if !skip_persist {
            persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
            let _ = crate::db::touch_session(&db, &session_id).await;
        }
        // B6 PR2: emit the terminal `Done` to the sink
        // UNCONDITIONALLY (regardless of `skip_persist`).
        // The worker's `SubagentBufferSink` needs the terminal
        // `Done` in its transcript so PR3's expand UI can
        // render the worker's stop_reason / usage correctly.
        // PR1b bundled the emit with the persist block under
        // `!skip_persist`; PR2 splits them because the emit
        // is a wire-shape concern (not a DB write) and is
        // load-bearing for the worker's transcript.
        emit_chat_event_via_sink(
            &sink,
            &rid,
            &ChatEvent::Done {
                stop_reason,
                usage: last_usage,
            },
        );
        return Err(());
    }

    // Execute tools. We intentionally take a simplified
    // permission path for tests: read tools bypass the
    // ask/allow UI, write tools go through the same ⑨ 关
    // check (the test can stub `permissions::check` via
    // the `permission_asks` map being empty — Tier 5
    // default-allow applies to read tools, Tier 3 fires
    // for write tools. Tests that exercise a specific
    // permission denial can pre-populate
    // `permission_asks` with a no-sender entry — the 120s
    // timeout fires and the test exits).
    // ⑬ loop detection (C2): feed this turn's tool_calls into the
    // sliding window, then run the two-level detector. On a hit we
    // keep a hint string to prepend to the result message (soft —
    // we never skip execution and never terminate; MAX_TURNS stays
    // the hard backstop). Per §2.5.8 this is tracing-only, no
    // AuditKind row.
    for (_id, name, input) in &tool_calls {
        loop_window.push_back(loop_detection::ToolCall::new(name.clone(), input.clone()));
    }
    while loop_window.len() > loop_detection::SOFT_WINDOW {
        loop_window.pop_front();
    }
    let loop_verdict = loop_detection::detect(&loop_window.iter().cloned().collect::<Vec<_>>());
    // C2+ (2026-07-05): maintain the consecutive-hit counter and
    // trigger active intervention at >= 3. The counter is
    // per-`run_chat_loop`-local (declared next to `loop_window`
    // outside the turn loop) so it accumulates across turns; on
    // any non-loop turn it resets to 0 (consecutiveness is the
    // signal). See `design.md §2` for the full state machine.
    let verdict_kind_str: Option<&'static str> = match &loop_verdict {
        loop_detection::LoopVerdict::HardLoop { .. } => Some("hard"),
        loop_detection::LoopVerdict::SoftLoop { .. } => Some("soft"),
        loop_detection::LoopVerdict::None => None,
    };
    let mut loop_hint: Option<String> = loop_verdict.hint_text();
    if verdict_kind_str.is_some() {
        tracing::warn!(verdict = ?loop_verdict, "agent loop ⑬: loop detected (soft hint)");
    }

    // C2+ active-intervention state machine. Only the main loop
    // drives the QuestionStore ask — worker subagents (which
    // reuse `run_chat_loop` with `effective_is_worker = true`)
    // take a direct-break short-circuit below so they don't
    // occupy the parent's QuestionStore slot or interrupt the
    // user. The worker's break surfaces to the parent via its
    // `Done { stop_reason: "loop_terminated" }` and the parent
    // sees the result in `dispatch_subagent`'s tool_result.
    if verdict_kind_str.is_some() {
        loop_hit_count = loop_hit_count.saturating_add(1);
    } else {
        loop_hit_count = 0;
    }

    // E2 trace (2026-07-14): record C2 soft hint (1-2 consecutive
    // hits, before the ≥3 active-intervention threshold). The ≥3
    // path already writes `loop_intervention` audit rows; this
    // trace covers the pre-intervention turns only.
    if verdict_kind_str.is_some() && loop_hit_count < 3 {
        if let Some(vk) = verdict_kind_str {
            crate::agent::trace::record_loop_hint(
                &sink,
                &db,
                &rid,
                &session_id,
                seq,
                loop_hit_count,
                vk,
            )
            .await;
        }
    }

    if loop_hit_count >= 3 {
        let Some(verdict_kind_str_expect) = verdict_kind_str else {
            return Err(());
        };
        // Reached the consecutive-hit threshold on a loop turn.
        // Worker path: direct break (R5) — no QuestionStore
        // round-trip, no audit row. The worker's loop_terminated
        // Done will be observed by `run_subagent` and surfaced
        // to the parent LLM via `format_dispatch_result*` (PR3
        // extends the formatter to detect `loop_terminated` and
        // append the "worker 因循环被终止" line; for PR2 we
        // only need the stop_reason itself to terminate the
        // worker's loop cleanly).
        if effective_is_worker {
            tracing::info!(
                hit_count = loop_hit_count,
                verdict = ?loop_verdict,
                "C2+ worker path: breaking loop (direct break, no ask)"
            );
            if !skip_persist {
                persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
                let _ = crate::db::touch_session(&db, &session_id).await;
            }
            emit_chat_event_via_sink(
                &sink,
                &rid,
                &ChatEvent::Done {
                    stop_reason: Some("loop_terminated".to_string()),
                    usage: last_usage,
                },
            );
            return Err(());
        }

        // Main loop: build the fixed payload (PRD R2) and drive
        // the QuestionStore round-trip.
        let payload = crate::agent::question_store::ToolQuestionPayload {
            session_id: session_id.clone(),
            tool_use_id: format!("loop_intervention_{}", turn),
            questions: vec![crate::agent::question_store::Question {
                question: "检测到 agent 似乎在循环重复相同操作（已连续 3 次触发循环检测，\
                           注入的软提示未能让模型纠正）。是否终止本次 agent loop？"
                    .to_string(),
                header: Some("循环检测干预".to_string()),
                options: vec![
                    crate::agent::question_store::QuestionOption {
                        label: "终止 loop".into(),
                        description: Some("停止本次 agent loop，保留已生成的内容".to_string()),
                        preview: None,
                    },
                    crate::agent::question_store::QuestionOption {
                        label: "继续".into(),
                        description: Some("清零计数器继续，给模型再次自我纠正的机会".to_string()),
                        preview: None,
                    },
                ],
                multi_select: false,
                allow_custom: false,
            }],
            ts: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
        };

        // Audit: action = "asked" lands immediately after
        // register succeeds (best-effort; helper is warn+swallow
        // on DB error — matches record_audit / record_tool_executed_audit).
        let _ = crate::agent::permissions::audit::record_loop_intervention_audit(
            &db,
            &session_id,
            None,
            loop_hit_count,
            verdict_kind_str_expect,
            "asked",
            Some(seq),
        )
        .await;

        // Try to register the pending question. `AlreadyPending`
        // (the LLM concurrently drove an `ask_user_question`
        // tool_use that's still waiting on a resolve) → log +
        // fall through to the original hint path (don't block
        // the loop; next turn we'll try again).
        match question_store
            .register(
                &session_id,
                &format!("loop_intervention_{}", turn),
                // C2+ registers as `LoopIntervention` (NOT
                // `Question`) so the frontend can render it as a
                // floating card. `Question` would require an
                // `ask_user_question` tool_use block anchor that
                // doesn't exist for a synthetic intervention →
                // the card would never render (2026-07-28
                // incident, session e8a1ad96…).
                crate::agent::question_store::PendingInteraction::LoopIntervention(payload.clone()),
            )
            .await
        {
            Ok(rx) => {
                sink.emit_tool_question(&payload);
                tracing::info!(
                    hit_count = loop_hit_count,
                    verdict = ?loop_verdict,
                    "C2+ active intervention: question asked"
                );
                // Three-arm select: cancel / answer / dropped.
                tokio::select! {
                    biased;
                    _ = token.cancelled() => {
                        // User hit Stop while the question was
                        // pending. Same cancel-cleanup as the
                        // ask_user_question tool path: clear the
                        // slot, emit Done{cancelled} (NOT
                        // Done{loop_terminated} — the user
                        // initiated this exit, not the C2+
                        // intervention).
                        question_store.remove(&session_id).await;
                        // 3b: synthesize the tool_result for the
                        // assistant's already-persisted tool_use
                        // blocks (this is the cancel-during-
                        // intervention arm — the per-stream
                        // `cancelled` flag is NOT set here, so
                        // the existing tail-pair repair at line
                        // 2101 doesn't fire; only this helper
                        // does).
                        finalize_pending_tool_results(
                            &db,
                            &session_id,
                            &tool_calls,
                            seq,
                            skip_persist,
                        )
                        .await;
                        if !skip_persist {
                            persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
                            let _ = crate::db::touch_session(&db, &session_id).await;
                        }
                        emit_chat_event_via_sink(
                            &sink,
                            &rid,
                            &ChatEvent::Done {
                                stop_reason: Some("cancelled".to_string()),
                                usage: None,
                            },
                        );
                        return Err(());
                    }
                    resp = rx => {
                        match resp {
                            Ok(crate::agent::question_store::InteractionResponse::Answered(value)) => {
                                // Inspect the first answer's
                                // selected options. Treat
                                // "终止 loop" (or empty / no
                                // match) as terminate, "继续"
                                // as continue. `Cancelled` is
                                // handled in the next arm.
                                //
                                // The `Answered` value is a
                                // `serde_json::Value` (unified
                                // `InteractionResponse`); the
                                // C2+ intervention uses the
                                // question shape (we registered
                                // `PendingInteraction::Question`
                                // above) so the value is a
                                // JSON-serialized
                                // `Vec<QuestionAnswer>`.
                                let answers: Vec<crate::agent::question_store::QuestionAnswer> =
                                    serde_json::from_value(value).unwrap_or_default();
                                let chosen = answers
                                    .first()
                                    .map(|a| a.options.first().cloned().unwrap_or_default())
                                    .unwrap_or_default();
                                if chosen == "继续" {
                                    let _ = crate::agent::permissions::audit::record_loop_intervention_audit(
                                        &db,
                                        &session_id,
                                        None,
                                        loop_hit_count,
                                        verdict_kind_str_expect,
                                        "continued",
                                    Some(seq),
                                    ).await;
                                    // Reset the counter so the
                                    // model gets a fresh 3-strike
                                    // budget. Replace the soft
                                    // hint with a stronger one
                                    // that tells the model the
                                    // user explicitly confirmed
                                    // the loop — repeating the
                                    // same call will not make
                                    // progress.
                                    loop_hit_count = 0;
                                    loop_hint = Some(
                                        "loop intervention: 用户已确认你在循环重复操作并选择继续。\
                                         请立即改变策略或停止 — 重复相同调用不会取得进展。"
                                            .to_string(),
                                    );
                                    // Fall through to the
                                    // normal result_blocks
                                    // construction (the
                                    // enhanced hint above will
                                    // be prepended like any
                                    // other loop hint).
                                } else {
                                    // "终止 loop" (or any
                                    // non-"继续" selection —
                                    // defensive: defaults to
                                    // terminate so a malformed
                                    // payload doesn't loop
                                    // forever).
                                    let _ = crate::agent::permissions::audit::record_loop_intervention_audit(
                                        &db,
                                        &session_id,
                                        None,
                                        loop_hit_count,
                                        verdict_kind_str_expect,
                                        "terminated",
                                    Some(seq),
                                    ).await;
                                    // 3b: synthesize the
                                    // tool_result for the
                                    // assistant's already-
                                    // persisted tool_use blocks.
                                    finalize_pending_tool_results(
                                        &db,
                                        &session_id,
                                        &tool_calls,
                                        seq,
                                        skip_persist,
                                    )
                                    .await;
                                    if !skip_persist {
                                        persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
                                        let _ = crate::db::touch_session(&db, &session_id).await;
                                    }
                                    emit_chat_event_via_sink(
                                        &sink,
                                        &rid,
                                        &ChatEvent::Done {
                                            stop_reason: Some("loop_terminated".to_string()),
                                            usage: None,
                                        },
                                    );
                                    return Err(());
                                }
                            }
                            Ok(crate::agent::question_store::InteractionResponse::Cancelled) => {
                                // User clicked "跳过" on the
                                // intervention card → treat as
                                // "终止 loop" (same rationale
                                // as the cancel arm of
                                // ask_user_question: the user
                                // dismissed the question, the
                                // safe default is to stop).
                                let _ = crate::agent::permissions::audit::record_loop_intervention_audit(
                                    &db,
                                    &session_id,
                                    None,
                                    loop_hit_count,
                                    verdict_kind_str_expect,
                                    "terminated",
                                Some(seq),
                                ).await;
                                // 3b: synthesize the tool_result
                                // for the assistant's already-
                                // persisted tool_use blocks.
                                finalize_pending_tool_results(
                                    &db,
                                    &session_id,
                                    &tool_calls,
                                    seq,
                                    skip_persist,
                                )
                                .await;
                                if !skip_persist {
                                    persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
                                    let _ = crate::db::touch_session(&db, &session_id).await;
                                }
                                emit_chat_event_via_sink(
                                    &sink,
                                    &rid,
                                    &ChatEvent::Done {
                                        stop_reason: Some("loop_terminated".to_string()),
                                        usage: None,
                                    },
                                );
                                return Err(());
                            }
                            Err(_recv_err) => {
                                // Sender dropped (e.g. resolve
                                // ran on a stale session id
                                // after the cancel arm cleaned
                                // the entry). Treat as session-
                                // cancelled — safe default
                                // matching the permission-store /
                                // ask_user_question parity.
                                tracing::warn!(
                                    "C2+ oneshot dropped without response — treating as cancelled"
                                );
                                // 3b: synthesize the tool_result
                                // the assistant turn already
                                // emitted (line 2097) so the DB
                                // does not end with an orphan
                                // tool_use that crashes the next
                                // LLM call.
                                finalize_pending_tool_results(
                                    &db,
                                    &session_id,
                                    &tool_calls,
                                    seq,
                                    skip_persist,
                                )
                                .await;
                                if !skip_persist {
                                    persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
                                    let _ = crate::db::touch_session(&db, &session_id).await;
                                }
                                emit_chat_event_via_sink(
                                    &sink,
                                    &rid,
                                    &ChatEvent::Done {
                                        stop_reason: Some("cancelled".to_string()),
                                        usage: None,
                                    },
                                );
                                return Err(());
                            }
                        }
                    }
                }
            }
            Err(crate::agent::question_store::QuestionStoreError::AlreadyPending) => {
                // LLM concurrently drove an ask_user_question
                // that's still pending — the single-pending gate
                // refuses our register. Per design §5 we
                // gracefully degrade: skip C2+ this turn (the
                // soft hint still lands), try again next turn.
                tracing::warn!(
                    session_id = %session_id,
                    "C2+ skipped: a question is already pending (LLM-driven ask_user_question)"
                );
            }
            Err(e) => {
                // `NotFound` is not reachable from `register`
                // (defensive branch matching ask_user_question).
                tracing::error!(
                    error = %e,
                    "C2+ register: unexpected store error"
                );
            }
        }
    }
    Ok(DriveTurnOutcome {
        tool_calls,
        loop_hint,
        messages,
        seq,
        head_sha,
        system_prompt,
        permission_ctx,
        loop_window,
        loop_hit_count,
        last_usage_terminal,
        workflow_ctx,
        cancelled,
    })
}
