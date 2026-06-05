//! Everlasting Tauri app entry point.
//!
//! Step 3a adds SQLite persistence: every assistant/tool_result turn is
//! written to disk at the turn boundary, sessions are listed/created/
//! loaded/deleted via dedicated commands.
//!
//! Step 6 adds extended-thinking support: the agent loop forwards
//! `ThinkingDelta` / `SignatureDelta` / `RedactedThinkingDelta` events to
//! the frontend `chat-event` channel (so the UI can stream the thinking
//! summary), and assembles `ContentBlock::Thinking` /
//! `ContentBlock::RedactedThinking` blocks at the turn boundary so the
//! signature blobs are persisted to the DB and echoed back to the LLM on
//! the next turn.

mod db;
mod llm;
mod tools;

use std::sync::Arc;

use futures_util::StreamExt;
use serde::Serialize;
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter, Manager, State};
use tracing_subscriber::{fmt, EnvFilter};

use llm::{
    chat_stream_with_tools, ChatEvent, ChatMessage, ContentBlock, LlmConfig, LlmErrorCategory,
    MessageContent, Role, ToolDef,
};

/// Maximum agent loop turns before forced stop (safety limit).
const MAX_TURNS: usize = 20;

// ---------------------------------------------------------------------------
// AppState
// ---------------------------------------------------------------------------

/// Process-wide state.
struct AppState {
    config: LlmConfig,
    tools: Vec<ToolDef>,
    db: SqlitePool,
}

impl AppState {
    async fn load(app: &AppHandle) -> Self {
        let config = LlmConfig::from_env().unwrap_or_else(|e| {
            tracing::warn!(
                error = %e,
                "ANTHROPIC_API_KEY not set; chat requests will return an auth error"
            );
            LlmConfig::unconfigured()
        });
        let tools = tools::builtin_tools();
        tracing::info!(
            base_url = %config.base_url,
            model = %config.model,
            tools_count = tools.len(),
            thinking_effort = %config.thinking_effort,
            "LLM config loaded"
        );

        // Resolve app_data_dir, then open SQLite there.
        let app_data_dir = app
            .path()
            .app_data_dir()
            .expect("failed to resolve app_data_dir");
        let db_path = app_data_dir.join("everlasting.db");
        let db = db::init_pool(&db_path)
            .await
            .expect("failed to open sqlite pool");
        db::run_migrations(&db)
            .await
            .expect("failed to run migrations");
        tracing::info!(db_path = %db_path.display(), "sqlite ready");

        Self { config, tools, db }
    }
}

// ---------------------------------------------------------------------------
// Event payloads
// ---------------------------------------------------------------------------

/// Event payload for the high-frequency `chat-event` channel
/// (start / delta / thinking_delta / signature_delta /
/// redacted_thinking_delta / done / error).
#[derive(Serialize, Clone)]
struct ChatEventPayload {
    request_id: String,
    #[serde(flatten)]
    event: ChatEvent,
}

/// Event payload for the low-frequency `tool:call` channel.
#[derive(Serialize, Clone)]
struct ToolCallPayload {
    request_id: String,
    id: String,
    name: String,
    input: serde_json::Value,
}

/// Event payload for the low-frequency `tool:result` channel.
#[derive(Serialize, Clone)]
struct ToolResultPayload {
    request_id: String,
    tool_use_id: String,
    content: String,
    is_error: bool,
}

/// Frontend-safe view of the LLM config.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicLlmConfig {
    model: String,
    base_url: String,
    configured: bool,
}

// ---------------------------------------------------------------------------
// Tauri commands — config
// ---------------------------------------------------------------------------

#[tauri::command]
fn get_llm_config(state: State<'_, Arc<AppState>>) -> PublicLlmConfig {
    let c = &state.config;
    PublicLlmConfig {
        model: c.model.clone(),
        base_url: c.base_url.clone(),
        configured: !c.is_unconfigured(),
    }
}

// ---------------------------------------------------------------------------
// Tauri commands — session management
// ---------------------------------------------------------------------------

#[tauri::command]
async fn list_sessions(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<db::SessionSummary>, String> {
    db::list_sessions(&state.db)
        .await
        .map_err(|e| format!("list_sessions failed: {}", e))
}

#[tauri::command]
async fn create_session(
    state: State<'_, Arc<AppState>>,
    model: Option<String>,
) -> Result<db::SessionRow, String> {
    let model = model.unwrap_or_else(|| state.config.model.clone());
    db::create_session(&state.db, &model)
        .await
        .map_err(|e| format!("create_session failed: {}", e))
}

#[tauri::command]
async fn load_session(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Option<db::LoadedSession>, String> {
    db::load_session(&state.db, &session_id)
        .await
        .map_err(|e| format!("load_session failed: {}", e))
}

#[tauri::command]
async fn delete_session(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<(), String> {
    db::delete_session(&state.db, &session_id)
        .await
        .map_err(|e| format!("delete_session failed: {}", e))
}

// ---------------------------------------------------------------------------
// Tauri command — chat (agent loop)
// ---------------------------------------------------------------------------

/// Per-turn accumulator for a single in-flight thinking block. We finalize
/// into a `ContentBlock::Thinking` (or push into `finalized_thinking`) as
/// soon as the model moves on to a text / tool_use block, and we always
/// flush whatever's still pending at the end of the turn.
#[derive(Default)]
struct PendingThinking {
    text: String,
    signature: String,
}

fn flush_pending_thinking(
    pending: &mut Option<PendingThinking>,
    finalized: &mut Vec<(String, String)>,
) {
    if let Some(p) = pending.take() {
        // We persist even if text is empty — what matters is that the
        // signature is preserved verbatim, so the LLM can validate the
        // round-trip. A thinking block whose text was streamed as empty
        // (e.g. `display: "omitted"`) is still a valid block.
        finalized.push((p.text, p.signature));
    }
}

#[tauri::command]
async fn chat(
    request_id: String,
    session_id: String,
    messages: Vec<ChatMessage>,
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> Result<(), String> {
    let config = state.config.clone();
    let tool_defs = state.tools.clone();
    let db = state.db.clone();
    let rid = request_id;
    let app_handle = app.clone();

    if config.is_unconfigured() {
        let payload = ChatEventPayload {
            request_id: rid,
            event: ChatEvent::Error {
                message: "ANTHROPIC_API_KEY 未设置,请在启动应用前配置环境变量".to_string(),
                category: LlmErrorCategory::Auth,
            },
        };
        app.emit("chat-event", payload).map_err(|e| e.to_string())?;
        return Ok(());
    }

    tauri::async_runtime::spawn(async move {
        let mut messages = messages;
        // Start seq from the highest existing seq in this session + 1.
        let next_seq = match db::load_session(&db, &session_id).await {
            Ok(Some(loaded)) => loaded
                .messages
                .iter()
                .map(|m| m.seq)
                .max()
                .map(|s| s + 1)
                .unwrap_or(0),
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
        let mut seq = next_seq;

        // Persist the most recent user-typed message before the agent loop
        // runs. Without this, the user message only lives in the frontend's
        // `messages.value` and the history sent to the LLM — never in the
        // DB — so it disappears the moment the user switches sessions.
        // The last User-role message in the history is always the new
        // typed one; earlier user turns (text or tool_result containers)
        // are already in the DB from previous turns.
        if let Some(last_user) =
            messages.iter().rev().find(|m| m.role == Role::User)
        {
            let msg = last_user.clone();
            if let Err(e) =
                db::persist_turn(&db, &session_id, msg.role, &msg.content, seq).await
            {
                tracing::error!(error = %e, "failed to persist user turn");
            }
            seq += 1;
        }

        for turn in 1..=MAX_TURNS {
            let mut stream = Box::pin(chat_stream_with_tools(
                config.clone(),
                messages.clone(),
                tool_defs.clone(),
            ));

            // Accumulate text, tool_calls, thinking blocks, and
            // redacted_thinking payloads from this LLM turn.
            let mut text_parts: Vec<String> = Vec::new();
            let mut tool_calls: Vec<(String, String, serde_json::Value)> = Vec::new();
            // Each finalized thinking block is `(thinking_text, signature)`.
            // Order matches the order the model emitted them — required by
            // the Anthropic API (see HACKING-llm.md "thinking note").
            let mut finalized_thinking: Vec<(String, String)> = Vec::new();
            let mut redacted_thinking_data: Vec<String> = Vec::new();
            let mut pending_thinking: Option<PendingThinking> = None;
            let mut stop_reason: Option<String> = None;
            let mut had_error = false;

            while let Some(event_result) = stream.next().await {
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
                        // A text delta means the model is done with
                        // thinking blocks for now. Finalize any pending
                        // thinking so it gets persisted in the right
                        // position relative to the text.
                        flush_pending_thinking(&mut pending_thinking, &mut finalized_thinking);
                        text_parts.push(text.clone());
                        emit_chat_event(&app_handle, &rid, &event);
                    }
                    ChatEvent::ThinkingDelta { text } => {
                        // Append to the currently-open thinking block, or
                        // open a new one if the model started fresh.
                        let p = pending_thinking
                            .get_or_insert_with(PendingThinking::default);
                        p.text.push_str(text);
                        emit_chat_event(&app_handle, &rid, &event);
                    }
                    ChatEvent::SignatureDelta { signature } => {
                        // The SSE parser buffers signature fragments and
                        // emits a single `SignatureDelta` on
                        // `content_block_stop` for the thinking block, so
                        // `signature` here is the full assembled blob.
                        // We still don't finalize on this event because
                        // the model can emit more thinking blocks
                        // (interleaved thinking with tool_use), so we
                        // wait for the first non-thinking event (Delta /
                        // ToolCall) or the end of the turn to commit.
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
                        // A tool_use block means the model is past its
                        // thinking phase for this turn. Finalize pending
                        // thinking so the order is correct.
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
                    ChatEvent::Done { stop_reason: sr } => {
                        stop_reason = sr.clone();
                    }
                    ChatEvent::Error { .. } => {
                        emit_chat_event(&app_handle, &rid, &event);
                        had_error = true;
                    }
                    ChatEvent::ToolResult { .. } => {
                        // Not expected from LLM stream; only used internally.
                    }
                }

                if matches!(event, ChatEvent::Done { .. } | ChatEvent::Error { .. }) {
                    break;
                }
            }

            if had_error {
                return;
            }

            // Make sure any still-open thinking block (signature received
            // but no subsequent text/tool_use to flush it) is captured.
            flush_pending_thinking(&mut pending_thinking, &mut finalized_thinking);

            // Build assistant message with collected content blocks. The
            // ordering follows the Anthropic "thinking → text → tool_use"
            // convention per turn, with thinking blocks first, then the
            // visible text, then tool_use, then any redacted_thinking
            // blocks (they can appear at the end or interleaved; we keep
            // them grouped at the tail to match the streaming order we
            // saw when they arrived).
            let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
            for (thinking, signature) in &finalized_thinking {
                assistant_blocks.push(ContentBlock::Thinking {
                    thinking: thinking.clone(),
                    signature: signature.clone(),
                });
            }
            let full_text = text_parts.join("");
            if !full_text.is_empty() {
                assistant_blocks.push(ContentBlock::Text { text: full_text });
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
                    db::persist_turn(&db, &session_id, msg.role, &msg.content, seq).await
                {
                    tracing::error!(error = %e, "failed to persist assistant turn");
                }
                messages.push(msg);
                seq += 1;
            }

            // Decide whether to continue the agent loop.
            let should_continue =
                stop_reason.as_deref() == Some("tool_use") && !tool_calls.is_empty();

            if !should_continue {
                // Bump session's updated_at to reflect activity.
                if let Err(e) = db::touch_session(&db, &session_id).await {
                    tracing::warn!(error = %e, "failed to touch session");
                }
                emit_chat_event(
                    &app_handle,
                    &rid,
                    &ChatEvent::Done { stop_reason },
                );
                return;
            }

            // Execute tools and build tool_result message.
            let mut result_blocks: Vec<ContentBlock> = Vec::new();
            for (id, name, input) in &tool_calls {
                let (content, is_error) = tools::execute_tool(name, input).await;

                let _ = app_handle.emit(
                    "tool:result",
                    ToolResultPayload {
                        request_id: rid.clone(),
                        tool_use_id: id.clone(),
                        content: content.clone(),
                        is_error,
                    },
                );

                result_blocks.push(ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content,
                    is_error,
                });
            }

            let tool_result_msg = ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(result_blocks),
            };
            if let Err(e) =
                db::persist_turn(&db, &session_id, tool_result_msg.role, &tool_result_msg.content, seq)
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
        let _ = db::touch_session(&db, &session_id).await;
        emit_chat_event(
            &app_handle,
            &rid,
            &ChatEvent::Done {
                stop_reason: Some("max_turns".to_string()),
            },
        );
    });

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn emit_chat_event(app: &AppHandle, rid: &str, event: &ChatEvent) {
    let payload = ChatEventPayload {
        request_id: rid.to_string(),
        event: event.clone(),
    };
    if let Err(e) = app.emit("chat-event", payload) {
        tracing::warn!(error = %e, "failed to emit chat-event");
    }
}

// ---------------------------------------------------------------------------
// App bootstrap
// ---------------------------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();

    tauri::Builder::default()
        .setup(|app| {
            let app_handle = app.handle().clone();
            let state = tauri::async_runtime::block_on(async move {
                Arc::new(AppState::load(&app_handle).await)
            });
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            chat,
            get_llm_config,
            list_sessions,
            create_session,
            load_session,
            delete_session,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).init();
}
