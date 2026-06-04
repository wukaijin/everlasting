//! Everlasting Tauri app entry point.
//!
//! Step 2: The `chat` command implements an agent loop — stream LLM response,
//! if tool_use → execute tools → feed results back → repeat until text-only
//! response or max turns.

mod llm;
mod tools;

use std::sync::Arc;

use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
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

/// Process-wide state. The LLM config is loaded from env at startup; if
/// `ANTHROPIC_API_KEY` is missing, we still let the UI start and surface
/// the error in the chat response (better UX than refusing to launch).
struct AppState {
    config: LlmConfig,
    tools: Vec<ToolDef>,
}

impl AppState {
    fn load() -> Self {
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
        Self { config, tools }
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
// Tauri commands
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

#[tauri::command]
async fn chat(
    request_id: String,
    messages: Vec<ChatMessage>,
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> Result<(), String> {
    let config = state.config.clone();
    let tool_defs = state.tools.clone();
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
                        // Only emit Start on the first turn — frontend shows
                        // "thinking…" indicator once.
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
                        // Low-frequency: independent event channel.
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
                messages.push(ChatMessage {
                    role: Role::Assistant,
                    content: MessageContent::Blocks(assistant_blocks),
                });
            }

            // Decide whether to continue the agent loop.
            let should_continue =
                stop_reason.as_deref() == Some("tool_use") && !tool_calls.is_empty();

            if !should_continue {
                // All done — emit final Done event.
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

                // Low-frequency: independent event channel.
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

            messages.push(ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(result_blocks),
            });

            tracing::info!(turn, tool_count = tool_calls.len(), "agent loop: executing tools, continuing");
            // Loop back to LLM with tool results appended.
        }

        // Safety: max turns reached.
        tracing::warn!(max_turns = MAX_TURNS, "agent loop: max turns reached");
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

    let state = Arc::new(AppState::load());

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![chat, get_llm_config])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).init();
}
