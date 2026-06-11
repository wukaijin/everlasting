//! The `chat` Tauri command + the spawned agent loop.
//!
//! The Tauri command itself is a thin wrapper that:
//! 1. Clones `AppState` handles into a `tauri::async_runtime::spawn`
//!    task.
//! 2. Performs pre-flight catalog resolution (so a missing /
//!    misconfigured model surfaces a clean user-facing error
//!    instead of a stream-time 401).
//! 3. Registers the cancellation token + session→request mapping
//!    (the in-flight cancel hook used by destructive commands).
//!
//! The spawned task is where the work happens:
//! - Load session + project, build the per-turn `ToolContext`,
//!   resolve the session root via `assert_within_root`, build the
//!   system prompt (Step 4 follow-up Bug 3).
//! - Persist the latest user message before the agent loop runs.
//! - Run up to [`MAX_TURNS`] agent loop iterations:
//!   - Issue one `Provider::send` call → `Pin<Box<dyn Stream<...>>>`
//!   - `tokio::select!` between the stream and the cancellation
//!     token (PR5 cancel mechanism).
//!   - Accumulate text / tool_calls / thinking blocks / redacted
//!     payloads; on each `Delta` / `ToolCall` event, flush any
//!     pending thinking block so the persisted order matches
//!     what the LLM emitted.
//!   - On `tool_use`, run the tools and persist a
//!     `user(tool_result)` message; loop again.
//!   - On terminal `Done` (or cancel), persist the assistant
//!     turn + emit a `Done` event with the appropriate
//!     `stop_reason`.

use std::path::PathBuf;
use std::sync::Arc;

use futures_util::StreamExt;
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter, State};
use tokio_util::sync::CancellationToken;

use crate::agent::helpers::{
    build_synthetic_tool_result_message, emit_chat_event, persist_turn_cwd, CANCELLED_MARKER,
};
use crate::agent::provider::{resolve_chat_provider, PreFlightError};
use crate::agent::system_prompt::{build_system_prompt, lookup_head_sha};
use crate::agent::thinking::{flush_pending_thinking, PendingThinking};
use crate::agent::MAX_TURNS;
use crate::llm::{
    ChatEvent, ChatMessage, ContentBlock, LlmErrorCategory, MessageContent, Role,
};
use crate::memory::loader::{build_instructions_blocks, load_for_session};
use crate::state::{AppState, CancellationGuard, ChatEventPayload, ToolCallPayload};
use crate::tools::ToolContext;

/// `chat` Tauri command entry. Returns immediately after spawning
/// the agent loop; the actual work runs in the background and
/// communicates with the frontend via `chat-event` / `tool:call` /
/// `tool:result` Tauri events.
#[tauri::command]
pub async fn chat(
    request_id: String,
    session_id: String,
    messages: Vec<ChatMessage>,
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> Result<(), String> {
    let tool_defs = state.tools.clone();
    let db = state.db.clone();
    let catalog = state.catalog.clone();
    let cancellations = state.cancellations.clone();
    let session_active_request = state.session_active_request.clone();
    let read_guard = state.read_guard.clone();
    let memory_cache = state.memory_cache.clone();
    let rid = request_id;
    let app_handle = app.clone();

    // PR1 pre-flight: look up the catalog for the default model.
    // The failure modes map 1:1 to PRD §Q2's locked-in user-facing
    // messages, surfaced as `ChatEvent::Error` so the frontend can
    // render the same toast path it uses for other LLM errors. We
    // do this BEFORE registering the cancellation token +
    // session_active_request entry because a pre-flight failure
    // is synchronous (no LLM call has started), so there is
    // nothing to cancel.
    let resolved = match lookup_provider_for_session(&session_id, &db, &catalog).await {
        Ok(r) => r,
        Err(err) => {
            let (msg, category) = err.user_message_and_category();
            tracing::warn!(
                request_id = %rid,
                session_id = %session_id,
                error = %msg,
                "chat: pre-flight failed (catalog)"
            );
            let payload = ChatEventPayload {
                request_id: rid,
                event: ChatEvent::Error { message: msg, category },
            };
            app.emit("chat-event", payload).map_err(|e| e.to_string())?;
            return Ok(());
        }
    };
    let provider: Arc<dyn crate::llm::Provider> = resolved.provider;
    tracing::info!(
        request_id = %rid,
        session_id = %session_id,
        model = %resolved.model_display_name,
        provider = %resolved.provider_display_name,
        protocol = ?provider.protocol(),
        "chat: provider resolved"
    );

    // Register a cancellation token for this request. The frontend's
    // Stop button calls `cancel_chat(rid)` which fetches this token
    // and triggers it; the agent loop's `tokio::select!` notices and
    // bails out. The entry is removed by the spawn task on every
    // exit path (normal / error / cancel / max_turns) — see the
    // guard at the end of the spawn closure.
    let token = CancellationToken::new();
    {
        let mut map = cancellations.lock().await;
        map.insert(rid.clone(), token.clone());
    }
    // Also register this session → request_id mapping so
    // destructive operations (delete_session, detach_worktree,
    // delete_worktree) can find and cancel the in-flight stream.
    // The entry is removed by the CancellationGuard on Drop.
    {
        let mut map = session_active_request.lock().await;
        map.insert(session_id.clone(), rid.clone());
    }

    tauri::async_runtime::spawn(async move {
        // The token's clone moves into this task; cancellation in
        // `cancel_chat` is observed via the original we just put
        // in the map. Both must outlive any `select!` arm that
        // awaits the token.
        let token = token;
        // RAII: removes the (rid → token) AND (session_id → rid)
        // entries on every exit path.
        let _cancel_guard = CancellationGuard {
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
                let _ = app_handle.emit(
                    "chat-event",
                    ChatEventPayload {
                        request_id: rid.clone(),
                        event: ChatEvent::Error {
                            message: format!("session {} not found", session_id),
                            category: LlmErrorCategory::InvalidRequest,
                        },
                    },
                );
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

        // --- Build the per-turn ToolContext ---
        // The project's `path` is the root; the session's
        // `current_cwd` is the agent's working directory inside
        // it. Both go through `assert_within_root` so the values
        // we hand to tools are canonical and provably inside the
        // project.
        let project = match crate::db::get_project(&db, &loaded_session.session.project_id).await {
            Ok(Some(p)) => p,
            Ok(None) => {
                tracing::error!(
                    project_id = %loaded_session.session.project_id,
                    "project not found for session"
                );
                let _ = app_handle.emit(
                    "chat-event",
                    ChatEventPayload {
                        request_id: rid.clone(),
                        event: ChatEvent::Error {
                            message: format!(
                                "project {} not found for this session",
                                loaded_session.session.project_id
                            ),
                            category: LlmErrorCategory::InvalidRequest,
                        },
                    },
                );
                return;
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to load project");
                return;
            }
        };
        // The agent's sandbox root: this is the directory the
        // boundary check is enforced against. For step 4 sessions
        // (every new session) it is the per-session worktree path
        // recorded in `sessions.worktree_path`. For pre-step-4
        // sessions (the column is NULL because they were created
        // before the migration ran) we fall back to the project
        // path, which is the legacy sandbox. Either way, this is
        // a canonical absolute path that has been validated by
        // `assert_within_root`.
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
                let _ = app_handle.emit(
                    "chat-event",
                    ChatEventPayload {
                        request_id: rid.clone(),
                        event: ChatEvent::Error {
                            message: format!("session root is invalid: {}", e),
                            category: LlmErrorCategory::InvalidRequest,
                        },
                    },
                );
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
            Err(e) => {
                // Defensive: if the stored cwd is no longer
                // reachable (e.g. user deleted a directory
                // mid-session), fall back to the worktree /
                // project root.
                tracing::warn!(
                    session_cwd = %session_cwd_raw,
                    worktree_path = %worktree_path.display(),
                    error = %e,
                    "session cwd outside worktree path — falling back to worktree path"
                );
                worktree_path.clone()
            }
        };
        let turn_ctx = ToolContext {
            worktree_path: worktree_path.clone(),
            cwd: session_cwd,
        };
        // The mutable tool context is used as the "current" cwd
        // within the turn — the shell tool reports updates through
        // `ToolContextUpdate` and we apply them to this copy.
        let mut current_ctx = turn_ctx;
        // The final cwd value to persist at the end of the turn.
        let mut last_cwd: Option<PathBuf> = None;

        // Step 4 follow-up Bug 3: build the LLM system prompt
        // **once** per chat invocation. The prompt describes the
        // session's working directory, worktree state, branch +
        // HEAD SHA so the model is explicitly grounded on every
        // request.
        let head_sha = lookup_head_sha(&worktree_path);
        let base_prompt = build_system_prompt(
            &loaded_session.session,
            &project,
            &worktree_path,
            &head_sha,
        );

        // B5 Memory (V2 1 期, 2026-06-10, refactored 2026-06-11):
        // the 4 instruction files (User / Project × CLAUDE.md /
        // AGENTS.md) are injected as a **synthetic user message at
        // the head of the `messages` array** rather than into the
        // `system_prompt` string. The first text block of that
        // message carries `cache_control: Some(CacheControl::Ephemeral)`,
        // which the wire layer preserves as a separate content
        // block (does NOT concatenate with adjacent text) so
        // Anthropic can cache the instructions on turn 1 and read
        // them from cache on turns 2..MAX_TURNS. Failure modes
        // (missing file, permission error, > 100 KiB, non-UTF-8)
        // are absorbed by the loader — a missing layer just
        // doesn't appear in the payload.
        let memory_layers =
            load_for_session(&memory_cache, &project.id, &project.path).await;
        let instructions_blocks = build_instructions_blocks(&memory_layers);
        if !instructions_blocks.is_empty() {
            // Synthetic user message carrying the 4 instructions
            // files. The first text block has the cache_control
            // marker (see [`build_instructions_blocks`]).
            messages.insert(
                0,
                ChatMessage {
                    role: Role::User,
                    content: MessageContent::Blocks(instructions_blocks),
                },
            );
            // Assistant acknowledgment: tells the model it has
            // read the instructions and will follow them. Without
            // this, the next user-role message in the array would
            // be in an odd position (user → user with no
            // assistant turn in between). The Anthropic API
            // accepts the user → user pattern in some cases but
            // the explicit acknowledgment is what Claude Code /
            // Aider do, and it makes the wire shape
            // self-documenting.
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

        // The system_prompt is now just the worktree-anchored
        // base_prompt. Memory content has been moved into the
        // synthetic user message above; sending it again in
        // `system` would be redundant and would defeat the
        // cache-control design (system content has no
        // per-block cache_control — only the message-array
        // content does).
        let system_prompt = base_prompt;

        // Persist the most recent user-typed message before the
        // agent loop runs. Without this, the user message only
        // lives in the frontend's `messages.value` and the
        // history sent to the LLM — never in the DB — so it
        // disappears the moment the user switches sessions.
        if let Some(last_user) = messages.iter().rev().find(|m| m.role == Role::User) {
            let msg = last_user.clone();
            if let Err(e) =
                crate::db::persist_turn(&db, &session_id, msg.role, &msg.content, seq).await
            {
                tracing::error!(error = %e, "failed to persist user turn");
            }
            seq += 1;
        }

        for turn in 1..=MAX_TURNS {
            // Dispatch through the catalog-resolved provider.
            // The provider was constructed once before the spawn
            // (above), so every turn of the 20-turn agent loop
            // uses the same `Arc<dyn Provider>` — no per-turn
            // protocol re-resolution.
            let mut stream = provider.send(
                Some(system_prompt.clone()),
                messages.clone(),
                tool_defs.clone(),
            );

            // Accumulate text, tool_calls, thinking blocks, and
            // redacted_thinking payloads from this LLM turn.
            let mut text_parts: Vec<String> = Vec::new();
            let mut tool_calls: Vec<(String, String, serde_json::Value)> = Vec::new();
            // Each finalized thinking block is `(thinking_text, signature)`.
            let mut finalized_thinking: Vec<(String, String)> = Vec::new();
            let mut redacted_thinking_data: Vec<String> = Vec::new();
            let mut pending_thinking: Option<PendingThinking> = None;
            let mut stop_reason: Option<String> = None;
            let mut last_usage: Option<crate::llm::types::TokenUsage> = None;
            let mut had_error = false;
            // PR5: set when the user hits Stop mid-stream. We
            // bail out of both the per-event select! loop AND
            // the agent loop, but still persist whatever's been
            // collected so far.
            let mut cancelled = false;

            // PR5 cancellation: `tokio::select!` interleaves the
            // stream's `next()` with the cancellation token's
            // `cancelled()` future. `biased;` means the cancel
            // arm is polled first when both are ready.
            loop {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => {
                        tracing::info!(request_id = %rid, "chat: cancellation requested by client");
                        cancelled = true;
                        break;
                    }
                    event_result = stream.next() => {
                        let Some(event_result) = event_result else {
                            break;
                        };
                        let event = match event_result {
                            Ok(e) => e,
                            Err(err) => {
                                had_error = true;
                                ChatEvent::Error {
                                    message: err.user_message(),
                                    category: err.category(),
                                }
                            }
                        };

                        match &event {
                            ChatEvent::Start => {
                                if turn == 1 {
                                    emit_chat_event(&app_handle, &rid, &event);
                                }
                            }
                            ChatEvent::Delta { text } => {
                                // A text delta means the model is
                                // done with thinking blocks for
                                // now. Finalize any pending
                                // thinking so it gets persisted
                                // in the right position relative
                                // to the text.
                                flush_pending_thinking(&mut pending_thinking, &mut finalized_thinking);
                                text_parts.push(text.clone());
                                emit_chat_event(&app_handle, &rid, &event);
                            }
                            ChatEvent::ThinkingDelta { text } => {
                                // Append to the currently-open
                                // thinking block, or open a new
                                // one if the model started fresh.
                                let p = pending_thinking
                                    .get_or_insert_with(PendingThinking::default);
                                p.text.push_str(text);
                                emit_chat_event(&app_handle, &rid, &event);
                            }
                            ChatEvent::SignatureDelta { signature } => {
                                // The SSE parser buffers signature
                                // fragments and emits a single
                                // `SignatureDelta` on
                                // `content_block_stop` for the
                                // thinking block, so `signature`
                                // here is the full assembled blob.
                                let p = pending_thinking
                                    .get_or_insert_with(PendingThinking::default);
                                p.signature.push_str(signature);
                                emit_chat_event(&app_handle, &rid, &event);
                            }
                            ChatEvent::RedactedThinkingDelta { data } => {
                                redacted_thinking_data.push(data.clone());
                                emit_chat_event(&app_handle, &rid, &event);
                            }
                            ChatEvent::ToolCall { id, name, input } => {
                                // A tool_use block means the
                                // model is past its thinking
                                // phase for this turn. Finalize
                                // pending thinking so the order
                                // is correct.
                                flush_pending_thinking(&mut pending_thinking, &mut finalized_thinking);
                                tool_calls.push((id.clone(), name.clone(), input.clone()));
                                let _ = app_handle.emit(
                                    "tool:call",
                                    ToolCallPayload {
                                        request_id: rid.clone(),
                                        id: id.clone(),
                                        name: name.clone(),
                                        input: input.clone(),
                                    },
                                );
                            }
                            ChatEvent::Done { stop_reason: sr, usage } => {
                                stop_reason = sr.clone();
                                last_usage = usage.clone();
                                // A4 (Token Usage Tracking):
                                // accumulate the per-turn usage
                                // into the session's column
                                // totals. `None` means the
                                // stream ended without a usage
                                // report (cancel / error /
                                // network drop) — we skip the
                                // SQL write in that case. See
                                // `db::sessions::add_token_usage`
                                // for the column-additive
                                // implementation.
                                if let Some(t) = usage {
                                    if let Err(e) =
                                        crate::db::add_token_usage(&db, &session_id, t).await
                                    {
                                        tracing::warn!(
                                            error = %e,
                                            "chat: failed to accumulate token usage (non-fatal)"
                                        );
                                    }
                                } else {
                                    tracing::info!(
                                        request_id = %rid,
                                        "chat: skipping token accumulation (no usage in Done event)"
                                    );
                                }
                            }
                            ChatEvent::Error { .. } => {
                                emit_chat_event(&app_handle, &rid, &event);
                                had_error = true;
                            }
                            ChatEvent::ToolResult { .. } => {
                                // Not expected from LLM stream;
                                // only used internally.
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

            // PR5: cancel hits here. We must still persist
            // whatever was collected in this turn (text / tool
            // calls / thinking / redacted), then break out of the
            // agent loop without executing tools.
            if cancelled {
                flush_pending_thinking(&mut pending_thinking, &mut finalized_thinking);
                tracing::info!(
                    request_id = %rid,
                    text_len = text_parts.iter().map(|s| s.len()).sum::<usize>(),
                    tool_calls = tool_calls.len(),
                    thinking_blocks = finalized_thinking.len(),
                    "chat: cancelled — persisting partial turn"
                );
            }

            // Make sure any still-open thinking block (signature
            // received but no subsequent text/tool_use to flush
            // it) is captured.
            flush_pending_thinking(&mut pending_thinking, &mut finalized_thinking);

            // Build assistant message with collected content
            // blocks. The ordering follows the Anthropic
            // "thinking → text → tool_use" convention per turn,
            // with thinking blocks first, then the visible text,
            // then tool_use, then any redacted_thinking blocks
            // (they can appear at the end or interleaved; we keep
            // them grouped at the tail to match the streaming
            // order we saw when they arrived).
            let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
            for (thinking, signature) in &finalized_thinking {
                assistant_blocks.push(ContentBlock::Thinking {
                    thinking: thinking.clone(),
                    signature: signature.clone(),
                });
            }
            // PR5: on cancel, the partial text is still useful —
            // but mark it so the user (and the rehydrate path)
            // can tell the message was cut short.
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
                assistant_blocks.push(ContentBlock::RedactedThinking {
                    data: data.clone(),
                });
            }

            if !assistant_blocks.is_empty() {
                let msg = ChatMessage {
                    role: Role::Assistant,
                    content: MessageContent::Blocks(assistant_blocks),
                };
                if let Err(e) =
                    crate::db::persist_turn(&db, &session_id, msg.role, &msg.content, seq).await
                {
                    tracing::error!(error = %e, "failed to persist assistant turn");
                }
                messages.push(msg);
                seq += 1;
            }

            // PR5: on cancel we are done — don't run tools.
            if cancelled {
                // BUG FIX (2013 tool_use orphan): if cancel hit
                // after the LLM emitted one or more `tool_use`
                // blocks, persist a synthetic `tool_result` user
                // message mirroring them.
                if !tool_calls.is_empty() {
                    let tool_result_msg =
                        build_synthetic_tool_result_message(&tool_calls);
                    if let Err(e) = crate::db::persist_turn(
                        &db,
                        &session_id,
                        tool_result_msg.role,
                        &tool_result_msg.content,
                        seq,
                    )
                    .await
                    {
                        tracing::error!(
                            error = %e,
                            "failed to persist synthetic tool_result turn after cancel"
                        );
                    }
                    messages.push(tool_result_msg);
                    tracing::warn!(
                        request_id = %rid,
                        tool_count = tool_calls.len(),
                        "chat: cancelled — persisted synthetic tool_result blocks to keep history self-consistent (prevent 2013 on next send)"
                    );
                }

                persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
                if let Err(e) = crate::db::touch_session(&db, &session_id).await {
                    tracing::warn!(error = %e, "failed to touch session");
                }
                emit_chat_event(
                    &app_handle,
                    &rid,
                    &ChatEvent::Done {
                        stop_reason: Some("cancelled".to_string()),
                        usage: None,
                    },
                );
                return;
            }

            // Decide whether to continue the agent loop.
            let should_continue =
                stop_reason.as_deref() == Some("tool_use") && !tool_calls.is_empty();

            if !should_continue {
                // Persist the agent's final cwd for this turn
                // (one write per turn, not per shell call).
                persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
                // Bump session's updated_at to reflect activity.
                if let Err(e) = crate::db::touch_session(&db, &session_id).await {
                    tracing::warn!(error = %e, "failed to touch session");
                }
                // Emit Done to frontend AFTER persist is complete so
                // that `reloadAfterFinalize` reads the full DB state.
                emit_chat_event(
                    &app_handle,
                    &rid,
                    &ChatEvent::Done {
                        stop_reason,
                        usage: last_usage,
                    },
                );
                return;
            }

            // Execute tools and build tool_result message.
            let mut result_blocks: Vec<ContentBlock> = Vec::new();
            for (id, name, input) in &tool_calls {
                let (content, is_error, update) = crate::tools::execute_tool(
                    name,
                    input,
                    &current_ctx,
                    Some(&read_guard),
                    Some(&session_id),
                )
                .await;
                // The shell tool (and any future tool that wants
                // to move the agent's working directory) reports
                // its new cwd through `update.new_cwd`.
                if let Some(new_cwd) = update.new_cwd.clone() {
                    current_ctx.cwd = new_cwd.clone();
                    last_cwd = Some(new_cwd);
                }

                // Step 4 follow-up (REQ-16): wrap the tool
                // result in a JSON envelope that includes the
                // worktree's current cwd.
                let envelope_str = crate::agent::helpers::tool_result_envelope(
                    &content,
                    &current_ctx.worktree_path,
                );

                let _ = app_handle.emit(
                    "tool:result",
                    crate::state::ToolResultPayload {
                        request_id: rid.clone(),
                        tool_use_id: id.clone(),
                        content: envelope_str.clone(),
                        is_error,
                    },
                );

                result_blocks.push(ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content: envelope_str,
                    is_error,
                });
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
            )
            .await
            {
                tracing::error!(error = %e, "failed to persist tool_result turn");
            }
            messages.push(tool_result_msg);
            seq += 1;

            tracing::info!(turn, tool_count = tool_calls.len(), "agent loop: executing tools, continuing");
        }

        // Safety: max turns reached.
        tracing::warn!(max_turns = MAX_TURNS, "agent loop: max turns reached");
        persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
        let _ = crate::db::touch_session(&db, &session_id).await;
        emit_chat_event(
            &app_handle,
            &rid,
            &ChatEvent::Done {
                stop_reason: Some("max_turns".to_string()),
                usage: None,
            },
        );
    });

    Ok(())
}

/// PR1 catalog lookup for the default model.
///
/// Resolve the provider for a chat request, preferring the
/// session's own `model_id` (per-session model override) and
/// falling back to the global `default_model_id`.
///
/// Resolution chain:
/// 1. Read `sessions.model_id` from DB (if set → use it)
/// 2. If NULL or points to missing model → fall back to global
///    `app_config.default_model_id`
/// 3. If still not found → DB slow path (`resolve_chat_provider`)
async fn lookup_provider_for_session(
    session_id: &str,
    db: &SqlitePool,
    catalog: &Arc<tokio::sync::RwLock<crate::state::ProviderCatalog>>,
) -> Result<ResolvedChatProviderWrapper, PreFlightError> {
    // Determine which model_id to use: session override or global default.
    let model_id = resolve_model_id_for_session(session_id, db).await?;

    // Resolve display names + api_key pre-flight from DB.
    let models = crate::db::list_models(db).await.map_err(|e| {
        tracing::error!(error = %e, "lookup_provider_for_session: list_models failed");
        PreFlightError::NoModel
    })?;
    let mwp = models
        .into_iter()
        .find(|m| m.model.id == model_id)
        .ok_or(PreFlightError::NoModel)?;
    let providers = crate::db::list_providers(db).await.map_err(|e| {
        tracing::error!(error = %e, "lookup_provider_for_session: list_providers failed");
        PreFlightError::ProviderMissing
    })?;
    let provider_row = providers
        .into_iter()
        .find(|p| p.id == mwp.model.provider_id)
        .ok_or(PreFlightError::ProviderMissing)?;

    // Pre-flight: empty api_key still applies on the catalog
    // path (the catalog might have been built with an empty
    // key if the user just saved Settings).
    if provider_row.api_key.is_empty() {
        return Err(PreFlightError::EmptyApiKey {
            provider_display_name: provider_row.display_name.clone(),
        });
    }

    // Fast path: catalog hit. Acquire read lock (concurrent
    // reads don't block each other).
    {
        let guard = catalog.read().await;
        if let Some(arc_provider) = guard.get(&model_id) {
            return Ok(ResolvedChatProviderWrapper {
                provider: arc_provider.clone(),
                model_display_name: mwp.model.display_name.clone(),
                provider_display_name: provider_row.display_name.clone(),
            });
        }
    }

    // Slow path: catalog miss (e.g. model added/changed but
    // rebuild not yet complete). Fall back to the legacy DB
    // resolver and wrap the resulting Box into an Arc.
    tracing::warn!(
        model_id = %model_id,
        "lookup_provider_for_session: catalog miss, falling back to DB resolver"
    );
    let resolved = resolve_chat_provider(db).await?;
    Ok(ResolvedChatProviderWrapper {
        provider: Arc::from(resolved.provider),
        model_display_name: resolved.model_display_name,
        provider_display_name: resolved.provider_display_name,
    })
}

/// Resolve the effective model_id for a session: prefer the
/// session's own `model_id` override, fall back to the global
/// `default_model_id`.
async fn resolve_model_id_for_session(
    session_id: &str,
    db: &SqlitePool,
) -> Result<String, PreFlightError> {
    // Try session's own model_id first.
    let session = crate::db::load_session(db, session_id).await.map_err(|e| {
        tracing::error!(error = %e, "resolve_model_id_for_session: load_session failed");
        PreFlightError::NoModel
    })?;
    if let Some(mid) = session.and_then(|s| s.session.model_id) {
        // Verify the model still exists in the catalog (not deleted).
        let models = crate::db::list_models(db).await.map_err(|e| {
            tracing::error!(error = %e, "resolve_model_id_for_session: list_models failed");
            PreFlightError::NoModel
        })?;
        if models.iter().any(|m| m.model.id == mid) {
            return Ok(mid);
        }
        tracing::warn!(
            session_id = %session_id,
            model_id = %mid,
            "resolve_model_id_for_session: session model_id points to deleted model, falling back to default"
        );
    }

    // Fallback: global default.
    crate::db::get_config_value(db, "default_model_id")
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "resolve_model_id_for_session: get_config_value failed");
            PreFlightError::NoModel
        })?
        .ok_or(PreFlightError::NoModel)
}

/// Thin wrapper holding the resolved provider as an Arc (so we
/// can share the catalog's pre-built instance) plus the display
/// names used for logging.
pub struct ResolvedChatProviderWrapper {
    pub provider: Arc<dyn crate::llm::Provider>,
    pub model_display_name: String,
    pub provider_display_name: String,
}