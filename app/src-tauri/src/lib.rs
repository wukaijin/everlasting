//! Everlasting Tauri app entry point.
//!
//! For step 1 we have one command (`chat`) which spawns an async task that
//! streams LLM completions back to the frontend via the `chat-event` event.
//! See HANDOFF §4.2 (Tauri IPC 桥).

mod llm;

use std::sync::Arc;

use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tracing_subscriber::{fmt, EnvFilter};

use llm::{chat_stream, ChatEvent, ChatMessage, LlmConfig};

/// Process-wide state. The LLM config is loaded from env at startup; if
/// `ANTHROPIC_API_KEY` is missing, we still let the UI start and surface
/// the error in the chat response (better UX than refusing to launch).
struct AppState {
    config: LlmConfig,
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
        Self { config }
    }
}

/// Event payload pushed to the frontend on the `chat-event` channel.
#[derive(Serialize, Clone)]
struct ChatEventPayload {
    request_id: String,
    #[serde(flatten)]
    event: ChatEvent,
}

/// Frontend-safe view of the LLM config. The API key is intentionally
/// omitted — the UI doesn't need it and we don't want it leakable via IPC.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicLlmConfig {
    model: String,
    base_url: String,
    configured: bool,
}

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
    let rid = request_id;
    let app_handle = app.clone();

    if config.is_unconfigured() {
        // Synchronous error — no need to spawn.
        let payload = ChatEventPayload {
            request_id: rid,
            event: ChatEvent::Error {
                message: "ANTHROPIC_API_KEY 未设置,请在启动应用前配置环境变量".to_string(),
                category: llm::LlmErrorCategory::Auth,
            },
        };
        app.emit("chat-event", payload).map_err(|e| e.to_string())?;
        return Ok(());
    }

    tauri::async_runtime::spawn(async move {
        let mut stream = Box::pin(chat_stream(config, messages));
        while let Some(event_result) = stream.next().await {
            let event = match event_result {
                Ok(e) => e,
                Err(err) => ChatEvent::Error {
                    message: err.user_message(),
                    category: err.category(),
                },
            };
            let payload = ChatEventPayload {
                request_id: rid.clone(),
                event: event.clone(),
            };
            if let Err(e) = app_handle.emit("chat-event", payload) {
                tracing::warn!(error = %e, "failed to emit chat-event");
                return;
            }
            if matches!(event, ChatEvent::Done { .. }) {
                return;
            }
        }
    });

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();

    let state = Arc::new(AppState::load());
    tracing::info!(
        base_url = %state.config.base_url,
        model = %state.config.model,
        "LLM config loaded"
    );

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
