//! Agent Loop body — testable variant (P1 RULE-A-006, 2026-06-14).
//!
//! This file is a test-friendly counterpart of the spawn closure
//! inside `chat.rs::chat`. The two implementations are NOT shared:
//! production keeps its ~1000-line inline closure (battle-tested,
//! every emit is a wire contract) and dispatches all four event
//! channels (`chat-event` / `tool:call` / `tool:result` /
//! `permission:ask`) through `app_handle.emit`. The test path
//! (`run_chat_loop` here) is a separate, structurally-parallel
//! implementation that dispatches the same four channels through a
//! `dyn ChatEventSink`, so integration tests can record events
//! without a Tauri `AppHandle`.
//!
//! Why a separate function instead of refactoring `chat.rs`:
//!
//! - The agent loop body is ~1000 lines of densely commented
//!   turn-orchestration code. Refactoring it for trait dispatch
//!   in a single PR would inflate the diff to 4× the size and
//!   risk subtle behavior changes (every emit is a critical
//!   wire contract).
//! - The testable variant focuses on the **turn-loop invariants**
//!   the audit called out: turn count, cancel race, max_turns
//!   fallback, error path emit, and provider dispatch. It does
//!   NOT re-test the protocol layer (covered by 105 dedicated
//!   tests) or the boundary / ReadGuard layers.
//! - The Tauri command path keeps its 1-emit = 1-line shape
//!   that's already battle-tested; the testable path uses the
//!   same `ChatEventSink` trait (introduced for the production
//!   `permissions::check` Tier 3 `permission:ask` dispatch —
//!   that path IS already routed through `AppHandleSink` in
//!   production) so the test variant gets a single `MockEmitter`
//!   sink for all four channels.
//!
//! # Drift hazard
//!
//! Because `chat.rs` and `chat_loop.rs` are two implementations
//! of the same invariants, any change to one MUST be mirrored in
//! the other or the integration tests lose their regression
//! protection. The audit's RULE-A-006 fix is therefore
//! **partial**: the test surface locks the turn-orchestration
//! invariants (cancel / max_turns / C3 / error path), but
//! production drift in `chat.rs` (e.g. a new emit site, a
//! reordered persist) won't be caught until a future PR migrates
//! `chat.rs` to dispatch through `run_chat_loop` itself.
//!
//! # What this function does NOT do
//!
//! - Does NOT run the catalog lookup / pre-flight. Callers must
//!   resolve a `Provider` themselves and pass it in.
//! - Does NOT own the `AppHandle` / cancellation token
//!   registration — callers register the token in the
//!   `cancellations` map and pass the clone here.
//! - Does NOT call `tauri::async_runtime::spawn`. The caller
//!   decides whether to run inline (tests) or in a background
//!   task (production). Production callers MUST `spawn` to
//!   preserve the existing Tauri command's "return immediately"
//!   semantic.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use futures_util::StreamExt;
use sqlx::SqlitePool;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::agent::helpers::{
    build_synthetic_tool_result_message, emit_chat_event_via_sink, persist_turn_cwd,
    CANCELLED_MARKER,
};
use crate::agent::permissions::{self, Decision, PermissionContext};
use crate::agent::thinking::{flush_pending_thinking, PendingThinking};
use crate::agent::MAX_TURNS;
use crate::llm::{
    ChatEvent, ChatMessage, ContentBlock, LlmErrorCategory, MessageContent, Provider, Role,
    ToolDef,
};
use crate::memory::MemoryCache;
use crate::state::{ChatEventSink, ToolCallPayload};
use crate::tools::read_guard::ReadGuard;
use crate::tools::ToolContext;

/// Test entry point for the agent loop. Currently consumed ONLY
/// by `agent/tests.rs` integration tests (P1 RULE-A-006). The
/// production `chat` Tauri command in `chat.rs` does NOT yet
/// route through this function — the architecture direction
/// (whether `chat.rs` should call `run_chat_loop` with an
/// `AppHandleSink`, or keep its existing inline closure) is
/// pending test results + diff audit.
///
/// Until that decision lands, the function is dead code in the
/// non-test build. `#[allow(dead_code)]` silences the warning
/// without `#[cfg(test)]` (which would force every consumer to
/// re-gate imports).
#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
pub async fn run_chat_loop(
    tool_defs: Vec<ToolDef>,
    provider: Arc<dyn Provider>,
    context_window: u32,
    rid: String,
    session_id: String,
    messages: Vec<ChatMessage>,
    sink: Arc<dyn ChatEventSink>,
    db: SqlitePool,
    cancellations: Arc<Mutex<std::collections::HashMap<String, CancellationToken>>>,
    session_active_request: Arc<Mutex<std::collections::HashMap<String, String>>>,
    read_guard: ReadGuard,
    memory_cache: Arc<MemoryCache>,
    permission_asks: crate::agent::permissions::PermissionStore,
    token: CancellationToken,
) {
    // RAII: removes the (rid → token) AND (session_id → rid)
    // entries on every exit path. Mirrors the original closure's
    // guard. The `tauri::async_runtime::spawn` inside `Drop` is
    // a no-op in the in-process test path (it just enqueues to
    // the global Tokio runtime), but it does no harm and keeps
    // the cancellation-map invariant identical to production.
    let _cancel_guard = crate::state::CancellationGuard {
        cancellations: cancellations.clone(),
        session_active_request: session_active_request.clone(),
        request_id: rid.clone(),
        session_id: session_id.clone(),
    };
    let mut messages = messages;

    // Start seq from the highest existing seq in this session + 1.
    let loaded_session = match crate::db::load_session(&db, &session_id).await {
        Ok(Some(loaded)) => loaded,
        Ok(None) => {
            tracing::warn!(session_id = %session_id, "session not found");
            sink.emit_chat_event(&crate::state::ChatEventPayload {
                request_id: rid.clone(),
                event: ChatEvent::Error {
                    message: format!("session {} not found", session_id),
                    category: LlmErrorCategory::InvalidRequest,
                },
            });
            return;
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to load session");
            return;
        }
    };
    let next_seq = loaded_session
        .messages
        .iter()
        .map(|m| m.seq)
        .max()
        .map(|s| s + 1)
        .unwrap_or(0);
    let mut seq = next_seq;

    // The agent loop uses a directory-bound worktree + cwd. The
    // test setup creates a project whose `path` we use directly
    // (no worktree); we read it from the session's project.
    let project = match crate::db::get_project(&db, &loaded_session.session.project_id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            sink.emit_chat_event(&crate::state::ChatEventPayload {
                request_id: rid.clone(),
                event: ChatEvent::Error {
                    message: format!(
                        "project {} not found for this session",
                        loaded_session.session.project_id
                    ),
                    category: LlmErrorCategory::InvalidRequest,
                },
            });
            return;
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to load project");
            return;
        }
    };
    let session_root_raw = loaded_session
        .session
        .worktree_path
        .clone()
        .unwrap_or_else(|| project.path.clone());
    let worktree_path = match crate::projects::boundary::assert_within_root(
        std::path::Path::new(&session_root_raw),
        std::path::Path::new(&session_root_raw),
    ) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(session_id = %session_id, error = %e, "session root invalid");
            sink.emit_chat_event(&crate::state::ChatEventPayload {
                request_id: rid.clone(),
                event: ChatEvent::Error {
                    message: format!("session root is invalid: {}", e),
                    category: LlmErrorCategory::InvalidRequest,
                },
            });
            return;
        }
    };
    let session_cwd_raw = if loaded_session.session.current_cwd.is_empty() {
        worktree_path.to_string_lossy().to_string()
    } else {
        loaded_session.session.current_cwd.clone()
    };
    let session_cwd = match crate::projects::boundary::assert_within_root(
        &worktree_path,
        std::path::Path::new(&session_cwd_raw),
    ) {
        Ok(p) => p,
        Err(_) => worktree_path.clone(),
    };
    let turn_ctx = ToolContext {
        worktree_path: worktree_path.clone(),
        cwd: session_cwd.clone(),
    };
    let mut current_ctx = turn_ctx;
    let mut last_cwd: Option<PathBuf> = None;

    let session_mode = loaded_session.session.mode;
    let permission_ctx = PermissionContext {
        session_id: session_id.clone(),
        mode: session_mode,
        cwd: session_cwd.clone(),
    };
    let mode_prefix = permissions::mode_system_prefix(session_mode);

    // B5 memory is empty in tests (no memory files written to the
    // temp project dir). Skip the synthetic user/assistant
    // inserts when `load_for_session` returns no layers.
    let memory_layers = load_for_session(&memory_cache, &project.id, &project.path).await;
    let instructions_blocks =
        crate::memory::loader::build_instructions_blocks(&memory_layers);
    if !instructions_blocks.is_empty() {
        messages.insert(
            0,
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(instructions_blocks),
            },
        );
        messages.insert(
            1,
            ChatMessage {
                role: Role::Assistant,
                content: MessageContent::Text(
                    "Understood. I will follow these instructions throughout our session."
                        .to_string(),
                ),
            },
        );
    }

    // System prompt is required even in tests; we build a minimal
    // one with empty session metadata. The real builder requires
    // a ProjectRow + SessionRow; tests can use the real ones.
    let head_sha = crate::agent::system_prompt::lookup_head_sha(&worktree_path);
    let base_prompt = crate::agent::system_prompt::build_system_prompt(
        &loaded_session.session,
        &project,
        &worktree_path,
        &head_sha,
    );
    let system_prompt = format!("{}\n\n{}", mode_prefix, base_prompt);
    let _ = &base_prompt;

    // Persist the most recent user message before the agent loop runs.
    if let Some(last_user) = messages.iter().rev().find(|m| m.role == Role::User) {
        let msg = last_user.clone();
        if let Err(e) = crate::db::persist_turn(&db, &session_id, msg.role, &msg.content, seq, None)
            .await
        {
            tracing::error!(error = %e, "failed to persist user turn");
        }
        seq += 1;
    }

    for turn in 1..=MAX_TURNS {
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
        //
        // DRIFT HAZARD: this block mirrors `chat.rs::chat`'s C3
        // block 1:1. The emit Error text, the tracing log line, the
        // `InvalidRequest` category, and the early return are
        // intentionally identical to production. See the module
        // docstring's "Drift hazard" section.
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
                    return;
                }
            }
        }

        let mut turn_send_at: Option<Instant> = None;
        let mut turn_first_delta_at: Option<Instant> = None;
        let mut turn_thinking_start: Option<Instant> = None;
        let mut turn_thinking_done: Option<Instant> = None;
        let mut turn_done_at: Option<Instant> = None;
        let _ = turn_send_at;

        let turn_tool_defs = permissions::filter_tools_for_mode(tool_defs.clone(), session_mode);
        let mut stream = provider.send(
            Some(system_prompt.clone()),
            messages.clone(),
            turn_tool_defs,
        );
        turn_send_at = Some(Instant::now());

        let mut text_parts: Vec<String> = Vec::new();
        let mut tool_calls: Vec<(String, String, serde_json::Value)> = Vec::new();
        let mut finalized_thinking: Vec<(String, String)> = Vec::new();
        let mut redacted_thinking_data: Vec<String> = Vec::new();
        let mut pending_thinking: Option<PendingThinking> = None;
        let mut stop_reason: Option<String> = None;
        let mut last_usage: Option<crate::llm::types::TokenUsage> = None;
        let mut had_error = false;
        let mut cancelled = false;

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
                        Err(err) => ChatEvent::Error {
                            message: err.user_message(),
                            category: err.category(),
                        },
                    };
                    match &event {
                        ChatEvent::Start => {
                            emit_chat_event_via_sink(&sink, &rid, &event);
                        }
                        ChatEvent::Delta { text } => {
                            flush_pending_thinking(&mut pending_thinking, &mut finalized_thinking);
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
                            redacted_thinking_data.push(data.clone());
                            emit_chat_event_via_sink(&sink, &rid, &event);
                        }
                        ChatEvent::ToolCall { id, name, input } => {
                            flush_pending_thinking(&mut pending_thinking, &mut finalized_thinking);
                            if turn_thinking_start.is_some() && turn_thinking_done.is_none() {
                                turn_thinking_done = Some(Instant::now());
                            }
                            tool_calls.push((id.clone(), name.clone(), input.clone()));
                            sink.emit_tool_call(&ToolCallPayload {
                                request_id: rid.clone(),
                                id: id.clone(),
                                name: name.clone(),
                                input: input.clone(),
                            });
                        }
                        ChatEvent::Done { stop_reason: sr, usage } => {
                            stop_reason = sr.clone();
                            last_usage = usage.clone();
                            turn_done_at = Some(Instant::now());
                            if turn_thinking_start.is_some() && turn_thinking_done.is_none() {
                                turn_thinking_done = Some(Instant::now());
                            }
                            if let Some(t) = usage {
                                if let Err(e) = crate::db::add_token_usage(&db, &session_id, t).await {
                                    tracing::warn!(error = %e, "chat: failed to accumulate token usage (non-fatal)");
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
                        ChatEvent::ToolResult { .. } => {}
                        ChatEvent::TurnComplete { .. } => {
                            tracing::warn!(request_id = %rid, "chat: unexpected TurnComplete in LLM stream");
                        }
                    }
                    if matches!(event, ChatEvent::Done { .. } | ChatEvent::Error { .. }) {
                        break;
                    }
                }
            }
        }

        if had_error {
            return;
        }

        if cancelled {
            flush_pending_thinking(&mut pending_thinking, &mut finalized_thinking);
            tracing::info!(
                request_id = %rid,
                "chat: cancelled — persisting partial turn"
            );
        }

        flush_pending_thinking(&mut pending_thinking, &mut finalized_thinking);

        let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
        for (thinking, signature) in &finalized_thinking {
            assistant_blocks.push(ContentBlock::Thinking {
                thinking: thinking.clone(),
                signature: signature.clone(),
            });
        }
        let mut full_text = text_parts.join("");
        if cancelled {
            if full_text.is_empty() {
                full_text = CANCELLED_MARKER.to_string();
            } else {
                full_text.push_str("\n\n");
                full_text.push_str(CANCELLED_MARKER);
            }
        }
        if !full_text.is_empty() {
            assistant_blocks.push(ContentBlock::Text { text: full_text, cache_control: None });
        }
        for (id, name, input) in &tool_calls {
            assistant_blocks.push(ContentBlock::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
            });
        }
        for data in &redacted_thinking_data {
            assistant_blocks.push(ContentBlock::RedactedThinking { data: data.clone() });
        }

        if !assistant_blocks.is_empty() {
            let msg = ChatMessage {
                role: Role::Assistant,
                content: MessageContent::Blocks(assistant_blocks),
            };
            let turn_latency = build_turn_latency(
                turn_send_at,
                turn_first_delta_at,
                turn_thinking_start,
                turn_thinking_done,
                turn_done_at,
            );
            if let Err(e) = crate::db::persist_turn(
                &db,
                &session_id,
                msg.role,
                &msg.content,
                seq,
                Some(&turn_latency),
            )
            .await
            {
                tracing::error!(error = %e, "failed to persist assistant turn");
            } else {
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
                if let Err(e) = crate::db::persist_turn(
                    &db,
                    &session_id,
                    tool_result_msg.role,
                    &tool_result_msg.content,
                    seq,
                    None,
                )
                .await
                {
                    tracing::error!(error = %e, "failed to persist synthetic tool_result turn after cancel");
                }
                messages.push(tool_result_msg);
            }
            persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
            let _ = crate::db::touch_session(&db, &session_id).await;
            emit_chat_event_via_sink(
                &sink,
                &rid,
                &ChatEvent::Done {
                    stop_reason: Some("cancelled".to_string()),
                    usage: None,
                },
            );
            return;
        }

        let should_continue =
            stop_reason.as_deref() == Some("tool_use") && !tool_calls.is_empty();

        if !should_continue {
            persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
            let _ = crate::db::touch_session(&db, &session_id).await;
            emit_chat_event_via_sink(
                &sink,
                &rid,
                &ChatEvent::Done { stop_reason, usage: last_usage },
            );
            return;
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
        let mut result_blocks: Vec<ContentBlock> = Vec::new();
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
                result_blocks.push(ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content: envelope,
                    is_error: true,
                });
                continue;
            }

            let tool_exec_start = Instant::now();
            let (content, is_error, update, exit_code) = crate::tools::execute_tool(
                name,
                input,
                &current_ctx,
                Some(&read_guard),
                Some(&session_id),
                token.clone(),
            )
            .await;
            let duration_ms = tool_exec_start.elapsed().as_millis();
            if let Err(e) = permissions::record_tool_executed_audit(
                &db,
                &session_id,
                name,
                input,
                duration_ms,
                exit_code,
            )
            .await
            {
                tracing::warn!(error = %e, "chat: record_tool_executed_audit failed (non-fatal)");
            }
            if token.is_cancelled() {
                cancelled = true;
            }
            if let Some(new_cwd) = update.new_cwd.clone() {
                current_ctx.cwd = new_cwd.clone();
                last_cwd = Some(new_cwd);
            }
            let envelope_str =
                crate::agent::helpers::tool_result_envelope(&content, &current_ctx.worktree_path);
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

        if cancelled {
            let result_count = result_blocks.len();
            if !result_blocks.is_empty() {
                let tool_result_msg = ChatMessage {
                    role: Role::User,
                    content: MessageContent::Blocks(result_blocks),
                };
                if let Err(e) = crate::db::persist_turn(
                    &db,
                    &session_id,
                    tool_result_msg.role,
                    &tool_result_msg.content,
                    seq,
                    None,
                )
                .await
                {
                    tracing::error!(error = %e, "failed to persist cancelled tool_result turn");
                }
                messages.push(tool_result_msg);
                tracing::info!(
                    request_id = %rid,
                    tool_results = result_count,
                    "chat_loop: cancelled during tool execution — persisted partial results"
                );
            }
            persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
            let _ = crate::db::touch_session(&db, &session_id).await;
            emit_chat_event_via_sink(
                &sink,
                &rid,
                &ChatEvent::Done {
                    stop_reason: Some("cancelled".to_string()),
                    usage: None,
                },
            );
            return;
        }

        let tool_result_msg = ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(result_blocks),
        };
        if let Err(e) = crate::db::persist_turn(
            &db,
            &session_id,
            tool_result_msg.role,
            &tool_result_msg.content,
            seq,
            None,
        )
        .await
        {
            tracing::error!(error = %e, "failed to persist tool_result turn");
        }
        messages.push(tool_result_msg);
        seq += 1;
    }

    tracing::warn!(max_turns = MAX_TURNS, "agent loop: max turns reached");
    persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
    let _ = crate::db::touch_session(&db, &session_id).await;
    emit_chat_event_via_sink(
        &sink,
        &rid,
        &ChatEvent::Done {
            stop_reason: Some("max_turns".to_string()),
            usage: None,
        },
    );
}

/// F5 per-turn latency helper — see `chat.rs::build_turn_latency`
/// for the production twin. Test-only here (used by
/// `run_chat_loop` which is itself test-gated).
#[allow(dead_code)]
fn build_turn_latency(
    turn_send_at: Option<Instant>,
    turn_first_delta_at: Option<Instant>,
    turn_thinking_start: Option<Instant>,
    turn_thinking_done: Option<Instant>,
    turn_done_at: Option<Instant>,
) -> crate::db::MessageLatency {
    crate::db::MessageLatency {
        ttfb_ms: instant_delta_ms(turn_send_at, turn_first_delta_at),
        gen_ms: instant_delta_ms(turn_first_delta_at, turn_done_at),
        total_ms: instant_delta_ms(turn_send_at, turn_done_at),
        thinking_ms: instant_delta_ms(turn_thinking_start, turn_thinking_done),
    }
}

#[allow(dead_code)]
fn instant_delta_ms(start: Option<Instant>, end: Option<Instant>) -> Option<i64> {
    match (start, end) {
        (Some(s), Some(e)) => {
            let d = e.saturating_duration_since(s);
            Some(i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        }
        _ => None,
    }
}

#[allow(dead_code)]
async fn load_for_session(
    cache: &Arc<MemoryCache>,
    project_id: &str,
    project_path: &str,
) -> Vec<crate::memory::MemoryLayer> {
    crate::memory::loader::load_for_session(cache, project_id, project_path).await
}
