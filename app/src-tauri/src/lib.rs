//! Everlasting Tauri app entry point.
//!
//! Step 3a adds SQLite persistence: every assistant/tool_result turn is
//! written to disk at the turn boundary, sessions are listed/created/
//! loaded/deleted via dedicated commands.

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

/// Event payload for the high-frequency `chat-event` channel (start/delta/done/error).
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

        for turn in 1..=MAX_TURNS {
            let mut stream = Box::pin(chat_stream_with_tools(
                config.clone(),
                messages.clone(),
                tool_defs.clone(),
            ));

            // Accumulate text and tool_calls from this LLM turn.
            let mut text_parts: Vec<String> = Vec::new();
            let mut tool_calls: Vec<(String, String, serde_json::Value)> = Vec::new();
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
                        text_parts.push(text.clone());
                        emit_chat_event(&app_handle, &rid, &event);
                    }
                    ChatEvent::ToolCall { id, name, input } => {
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

            // Build assistant message with collected content blocks.
            let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
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
