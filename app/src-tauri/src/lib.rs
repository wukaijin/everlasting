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
//!
//! Step 3b-1 adds project binding + a `ToolContext` that is injected into
//! every tool call, plus the 7-project Tauri command surface
//! (`list_projects` / `create_project` / `update_project_path` / etc.).
//! See `docs/PROPOSAL-project-binding-and-top-tabs.md` and
//! `.trellis/spec/backend/project-cwd-boundary.md`.

mod db;
mod llm;
mod projects;
mod tools;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::StreamExt;
use serde::Serialize;
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{fmt, EnvFilter};

use llm::{
    chat_stream_with_tools, ChatEvent, ChatMessage, ContentBlock, LlmConfig, LlmErrorCategory,
    MessageContent, Role, ToolDef,
};
use tools::ToolContext;

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
    /// Active chat request cancellation tokens, keyed by `request_id`.
    /// PR5 (cancel mechanism): the frontend's Stop button calls
    /// `cancel_chat(request_id)` which looks up the token and calls
    /// `.cancel()`. The agent loop is wrapped in `tokio::select!` and
    /// listens for cancellation between events. The entry is removed
    /// by the spawn task on every exit path (normal / error / cancel).
    cancellations: Arc<Mutex<HashMap<String, CancellationToken>>>,
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

        Self {
            config,
            tools,
            db,
            cancellations: Arc::new(Mutex::new(HashMap::new())),
        }
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

/// Return the user's home directory (the path the frontend will
/// shorten to `~` when rendering the cwd chip in the chat panel
/// header). Resolves to `None` when the platform has no notion of a
/// home directory (e.g. a sandboxed container without `$HOME`); the
/// frontend falls back to rendering the full path in that case.
///
/// We use `AppHandle::path()` (Tauri 2's public `PathResolver`)
/// rather than the `dirs` crate directly. The `dirs` crate is a
/// transitive dependency of Tauri 2, but Rust 2018+ does not
/// auto-expose transitive deps, so calling `dirs::home_dir()` would
/// require adding it to `Cargo.toml`. `app.path().home_dir()` is
/// the same call wrapped by Tauri's API and matches the existing
/// `app_data_dir` pattern in `AppState::load`.
#[tauri::command]
fn get_home_dir(app: AppHandle) -> Option<String> {
    app.path()
        .home_dir()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------
// Tauri commands — session management
// ---------------------------------------------------------------------------

#[tauri::command]
async fn list_sessions(
    state: State<'_, Arc<AppState>>,
    project_id: String,
) -> Result<Vec<db::SessionSummary>, String> {
    db::list_sessions(&state.db, &project_id)
        .await
        .map_err(|e| format!("list_sessions failed: {}", e))
}

#[tauri::command]
async fn create_session(
    state: State<'_, Arc<AppState>>,
    project_id: String,
    initial_cwd: String,
    model: Option<String>,
) -> Result<db::SessionRow, String> {
    let model = model.unwrap_or_else(|| state.config.model.clone());
    // Defensive: every session is bound to a project. The frontend is
    // expected to gate this with a "no project = no chat" check, but a
    // stray IPC call should not silently create a legacy-bound session.
    if project_id.trim().is_empty() {
        return Err("create_session: project_id must not be empty".to_string());
    }
    // Sanity: the project must exist. We do NOT error out if it doesn't
    // (the user could be racing a delete); instead we let `db::create_session`
    // surface the foreign-key violation as a clear error.
    db::create_session(&state.db, &project_id, &initial_cwd, &model)
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
// Tauri commands — project management (PROPOSAL §4.2)
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, serde::Deserialize)]
struct ListProjectsFilter {
    #[serde(default)]
    hidden: Option<bool>,
}

#[tauri::command]
async fn list_projects(
    state: State<'_, Arc<AppState>>,
    filter: Option<ListProjectsFilter>,
) -> Result<Vec<projects::ProjectRow>, String> {
    // `filter = { hidden: true }` returns the "recently hidden" list used
    // by the empty-state panel. The default (`hidden: false` or
    // `filter = null`) is the main Tab bar.
    let include_hidden = filter
        .as_ref()
        .and_then(|f| f.hidden)
        .unwrap_or(false);
    db::list_projects(&state.db, include_hidden)
        .await
        .map_err(|e| format!("list_projects failed: {}", e))
}

#[tauri::command]
async fn list_hidden_projects(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<projects::ProjectRow>, String> {
    db::list_hidden_projects(&state.db)
        .await
        .map_err(|e| format!("list_hidden_projects failed: {}", e))
}

#[tauri::command]
async fn create_project(
    state: State<'_, Arc<AppState>>,
    path: String,
) -> Result<projects::ProjectRow, String> {
    projects::store::create_project(&state.db, &path).await
}

#[tauri::command]
async fn update_project_path(
    state: State<'_, Arc<AppState>>,
    id: String,
    new_path: String,
) -> Result<projects::ProjectRow, String> {
    projects::store::update_project_path(&state.db, &id, &new_path).await
}

#[tauri::command]
async fn update_project_name(
    state: State<'_, Arc<AppState>>,
    id: String,
    new_name: String,
) -> Result<projects::ProjectRow, String> {
    projects::store::update_project_name(&state.db, &id, &new_name).await
}

#[tauri::command]
async fn hide_project(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<(), String> {
    projects::store::hide_project(&state.db, &id).await
}

#[tauri::command]
async fn unhide_project(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<(), String> {
    projects::store::unhide_project(&state.db, &id).await
}

/// Show a native directory picker. Returns `Some(path)` if the user
/// picked a directory, `None` if they cancelled or the dialog is
/// unavailable.
///
/// The `fallback` argument is reserved for a future "show manual input
/// dialog" UX (review GLM §4.2) — for now the frontend uses it to
/// decide whether to surface the fallback input. We do not
/// short-circuit on it here, because the dialog itself either
/// succeeds or the frontend reads `None` and shows the manual input.
#[tauri::command]
async fn pick_project_dir(
    app: AppHandle,
    #[allow(unused_variables)] fallback: bool,
) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<PathBuf>>();
    app.dialog()
        .file()
        .set_title("选择项目目录")
        .pick_folder(move |folder| {
            // The callback may fire on the UI thread depending on the
            // platform; we just need to forward the value. `None` means
            // "cancelled or dialog unavailable".
            let path = folder.and_then(|fp| fp.into_path().ok());
            let _ = tx.send(path);
        });
    match rx.await {
        Ok(Some(p)) => Ok(Some(p.to_string_lossy().into_owned())),
        Ok(None) => Ok(None),
        Err(_) => Err("dialog channel closed".to_string()),
    }
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

/// Sentinel string appended to the assistant message's text on cancel.
/// The UI (rehydrate path) leaves the marker in place; the bubble just
/// renders it inline. A literal "🛑" was considered but it would be
/// inlined as part of markdown; the bracketed text survives DOMPurify
/// unchanged and is locale-friendly.
const CANCELLED_MARKER: &str = "[已停止]";

/// RAII guard that removes a request_id from the cancellations map
/// on Drop. We use a guard (not a bare `remove` call at every `return`
/// point) so a future refactor that adds a new early-return path
/// can't accidentally leak the entry. The guard is `Send` because
/// it only holds an `Arc<Mutex<HashMap<...>>>` clone, which itself
/// is `Send + Sync`.
struct CancellationGuard {
    cancellations: Arc<Mutex<HashMap<String, CancellationToken>>>,
    request_id: String,
}

impl Drop for CancellationGuard {
    fn drop(&mut self) {
        let cancellations = self.cancellations.clone();
        let request_id = self.request_id.clone();
        tauri::async_runtime::spawn(async move {
            let mut map = cancellations.lock().await;
            map.remove(&request_id);
        });
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
    let cancellations = state.cancellations.clone();
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

    tauri::async_runtime::spawn(async move {
        // The token's clone moves into this task; cancellation in
        // `cancel_chat` is observed via the original we just put in
        // the map. Both must outlive any `select!` arm that awaits
        // the token.
        let token = token;
        // RAII: removes the (rid → token) entry on every exit path.
        let _cancel_guard = CancellationGuard {
            cancellations: cancellations.clone(),
            request_id: rid.clone(),
        };
        let mut messages = messages;
        // Start seq from the highest existing seq in this session + 1.
        let loaded_session = match db::load_session(&db, &session_id).await {
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
        // `current_cwd` is the agent's working directory inside it.
        // Both go through `assert_within_root` so the values we hand
        // to tools are canonical and provably inside the project.
        let project = match db::get_project(&db, &loaded_session.session.project_id).await {
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
        let project_root = match projects::boundary::assert_within_root(
            std::path::Path::new(&project.path),
            std::path::Path::new(&project.path),
        ) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(project_id = %project.id, error = %e, "project path invalid");
                let _ = app_handle.emit(
                    "chat-event",
                    ChatEventPayload {
                        request_id: rid.clone(),
                        event: ChatEvent::Error {
                            message: format!("project path is invalid: {}", e),
                            category: LlmErrorCategory::InvalidRequest,
                        },
                    },
                );
                return;
            }
        };

        let session_cwd_raw = if loaded_session.session.current_cwd.is_empty() {
            project.path.clone()
        } else {
            loaded_session.session.current_cwd.clone()
        };
        let session_cwd = match projects::boundary::assert_within_root(
            &project_root,
            std::path::Path::new(&session_cwd_raw),
        ) {
            Ok(p) => p,
            Err(e) => {
                // Defensive: if the stored cwd is no longer reachable
                // (e.g. user deleted a directory mid-session), fall
                // back to the project root. The next shell tool call
                // will move `turn_ctx.cwd` to wherever it goes.
                tracing::warn!(
                    session_cwd = %session_cwd_raw,
                    project_root = %project_root.display(),
                    error = %e,
                    "session cwd outside project root — falling back to project root"
                );
                project_root.clone()
            }
        };
        let turn_ctx = ToolContext {
            project_root: project_root.clone(),
            cwd: session_cwd,
        };
        // The mutable tool context is used as the "current" cwd
        // within the turn — the shell tool reports updates through
        // `ToolContextUpdate` and we apply them to this copy.
        let mut current_ctx = turn_ctx;
        // The final cwd value to persist at the end of the turn.
        let mut last_cwd: Option<PathBuf> = None;

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
            // PR5: set when the user hits Stop mid-stream. We bail out
            // of both the per-event select! loop AND the agent loop,
            // but still persist whatever's been collected so far.
            let mut cancelled = false;

            // PR5 cancellation: `tokio::select!` interleaves the
            // stream's `next()` with the cancellation token's
            // `cancelled()` future. `biased;` means the cancel arm
            // is polled first when both are ready — the user expects
            // Stop to take effect immediately, not "next time the
            // stream happens to yield".
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
                }
            }

            if had_error {
                return;
            }

            // PR5: cancel hits here. We must still persist whatever was
            // collected in this turn (text / tool calls / thinking /
            // redacted), then break out of the agent loop without
            // executing tools. The frontend's `handleChatEvent` for
            // `done` will reset `sending` and `currentRequestId`, so
            // the user can immediately send a new message.
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
            // PR5: on cancel, the partial text is still useful — but
            // mark it so the user (and the rehydrate path) can tell
            // the message was cut short. The marker is appended only
            // to the visible Text block; thinking blocks and
            // tool_use calls are persisted as-is so the next LLM
            // request gets full context.
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

            // PR5: on cancel we are done — don't run tools (the user
            // asked to stop; don't make them watch a 5-min shell
            // command after they hit Stop). Emit a `done` with
            // `stop_reason: "cancelled"` so the frontend's
            // `handleChatEvent` for `done` resets `sending` /
            // `currentRequestId` exactly like a normal completion.
            if cancelled {
                persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
                if let Err(e) = db::touch_session(&db, &session_id).await {
                    tracing::warn!(error = %e, "failed to touch session");
                }
                emit_chat_event(
                    &app_handle,
                    &rid,
                    &ChatEvent::Done {
                        stop_reason: Some("cancelled".to_string()),
                    },
                );
                return;
            }

            // Decide whether to continue the agent loop.
            let should_continue =
                stop_reason.as_deref() == Some("tool_use") && !tool_calls.is_empty();

            if !should_continue {
                // Persist the agent's final cwd for this turn (one
                // write per turn, not per shell call — see PROPOSAL
                // §4.4 "turn 结束一次性写").
                persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
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
                let (content, is_error, update) =
                    tools::execute_tool(name, input, &current_ctx).await;
                // The shell tool (and any future tool that wants to
                // move the agent's working directory) reports its new
                // cwd through `update.new_cwd`. We track the latest
                // and persist it at the end of the turn — see
                // `docs/PROPOSAL-project-binding-and-top-tabs.md` §4.4
                // "turn 结束一次性写".
                if let Some(new_cwd) = update.new_cwd.clone() {
                    current_ctx.cwd = new_cwd.clone();
                    last_cwd = Some(new_cwd);
                }

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
        persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
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

/// Persist the final cwd of a turn. Called once at turn end (not after
/// every shell call). We compare against the DB-stored value to avoid
/// a no-op write when the agent stayed put.
///
/// `last_cwd` is the latest validated canonical path reported by the
/// shell tool's `ToolContextUpdate`. We store the path as a string —
/// the next turn's `assert_within_root` call will canonicalize it
/// again on read, so the DB stays canonical-encoding-agnostic.
async fn persist_turn_cwd(
    db: &SqlitePool,
    session_id: &str,
    last_cwd: Option<&std::path::Path>,
) {
    let Some(new_cwd) = last_cwd else {
        return;
    };
    let new_cwd_str = new_cwd.to_string_lossy().into_owned();
    // Cheap "did it change?" guard. We compare against the
    // just-loaded session rather than re-querying.
    if let Ok(Some(loaded)) = db::load_session(db, session_id).await {
        if loaded.session.current_cwd == new_cwd_str {
            return;
        }
    }
    if let Err(e) = db::update_session_cwd(db, session_id, &new_cwd_str).await {
        tracing::warn!(error = %e, "failed to persist turn cwd");
    }
}

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
// Tauri command — cancel chat (PR5)
// ---------------------------------------------------------------------------

/// Cancel an in-flight chat request. The frontend's Stop button
/// invokes this with the current `request_id`. Looks up the
/// matching `CancellationToken` and calls `.cancel()` on it; the
/// agent loop's `tokio::select!` notices on the next event boundary
/// and bails out cleanly (partial turn is persisted; a `done` event
/// with `stop_reason: "cancelled"` is emitted).
///
/// Idempotent: a missing `request_id` is a silent no-op (the user
/// may have clicked Stop after the stream already finished).
/// Re-cancelling an already-cancelled token is also a no-op.
#[tauri::command]
async fn cancel_chat(
    request_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let token = {
        let map = state.cancellations.lock().await;
        map.get(&request_id).cloned()
    };
    if let Some(t) = token {
        t.cancel();
        tracing::info!(request_id = %request_id, "cancel_chat: token cancelled");
    } else {
        tracing::debug!(
            request_id = %request_id,
            "cancel_chat: no active request (likely already finished)"
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// App bootstrap
// ---------------------------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_os::init())
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
            cancel_chat,
            get_llm_config,
            get_home_dir,
            list_sessions,
            create_session,
            load_session,
            delete_session,
            list_projects,
            list_hidden_projects,
            create_project,
            update_project_path,
            update_project_name,
            hide_project,
            unhide_project,
            pick_project_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).init();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Race a slow fake stream against a cancellation token. Mirrors
    /// the per-event select! loop in `chat` (minus the SSE plumbing).
    /// Asserts cancel wins when fired mid-stream.
    #[tokio::test]
    async fn select_loop_breaks_on_cancellation() {
        let token = CancellationToken::new();
        let cancelled_flag = Arc::new(std::sync::Mutex::new(false));
        let cancelled_flag_clone = cancelled_flag.clone();
        let token_clone = token.clone();

        // Simulate the per-event select! pattern. Each "event" is
        // tokio::time::sleep; the cancel arm races them.
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = token_clone.cancelled() => {
                        *cancelled_flag_clone.lock().unwrap() = true;
                        break;
                    }
                    _ = tokio::time::sleep(Duration::from_millis(200)) => {
                        // Stream "produced an event" — loop again.
                    }
                }
            }
        });

        // Give the loop a tick to start, then cancel.
        tokio::time::sleep(Duration::from_millis(50)).await;
        token.cancel();

        // The select! arm should win within a few ms.
        let joined = tokio::time::timeout(Duration::from_millis(500), handle)
            .await
            .expect("select loop should have broken within 500ms")
            .expect("task should not have panicked");
        assert!(
            *cancelled_flag.lock().unwrap(),
            "cancelled flag should be set when select! breaks on cancel"
        );
        // Silence the "joined result unused" warning — the function
        // already returns ().
        let _ = joined;
    }

    #[tokio::test]
    async fn cancellation_token_idempotent() {
        let token = CancellationToken::new();
        token.cancel();
        token.cancel();
        // Second cancel is a no-op; is_cancelled stays true; no panic.
        assert!(token.is_cancelled());
    }

    /// Mirrors the `cancel_chat` command's lookup logic, isolated
    /// from the Tauri State wrapper. Tests that a missing
    /// `request_id` is a silent Ok (idempotent) and a present one
    /// actually flips the token.
    #[tokio::test]
    async fn cancel_chat_idempotent_for_missing_and_present() {
        let cancellations: Arc<Mutex<HashMap<String, CancellationToken>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Missing request_id → no-op, returns Ok.
        let missing = {
            let map = cancellations.lock().await;
            map.get("does-not-exist").cloned()
        };
        assert!(missing.is_none(), "unknown id should not be in map");

        // Present request_id → token fetched, is_cancelled flips.
        let token = CancellationToken::new();
        {
            let mut map = cancellations.lock().await;
            map.insert("rid-1".to_string(), token.clone());
        }
        let fetched = {
            let map = cancellations.lock().await;
            map.get("rid-1").cloned()
        };
        assert!(fetched.is_some());
        let t = fetched.unwrap();
        assert!(!t.is_cancelled());
        t.cancel();
        assert!(t.is_cancelled());
    }

    /// Concurrent requests: two `request_id`s are independent. Cancel
    /// one; the other should not be affected.
    #[tokio::test]
    async fn two_concurrent_requests_are_independent() {
        let cancellations: Arc<Mutex<HashMap<String, CancellationToken>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let a = CancellationToken::new();
        let b = CancellationToken::new();
        {
            let mut map = cancellations.lock().await;
            map.insert("a".to_string(), a.clone());
            map.insert("b".to_string(), b.clone());
        }
        // Cancel A.
        {
            let map = cancellations.lock().await;
            let t = map.get("a").cloned();
            if let Some(t) = t {
                t.cancel();
            }
        }
        assert!(a.is_cancelled());
        assert!(!b.is_cancelled(), "B should not be affected by A's cancel");
    }

    /// CancellationGuard removes the entry on Drop. We construct a
    /// guard, drop it, and verify the map is empty. The Drop runs
    /// `tauri::async_runtime::spawn`, so the test is wrapped in
    /// `#[tokio::test]` to provide a runtime (the guard's spawn
    /// borrows the current Tokio runtime via the Tauri shim; in
    /// unit tests we route through the global runtime).
    #[tokio::test(flavor = "multi_thread")]
    async fn cancellation_guard_removes_entry_on_drop() {
        let cancellations: Arc<Mutex<HashMap<String, CancellationToken>>> =
            Arc::new(Mutex::new(HashMap::new()));
        {
            let mut map = cancellations.lock().await;
            map.insert("rid-g".to_string(), CancellationToken::new());
        }
        assert_eq!(cancellations.lock().await.len(), 1);
        {
            let _guard = CancellationGuard {
                cancellations: cancellations.clone(),
                request_id: "rid-g".to_string(),
            };
            // _guard drops at end of block.
        }
        // Give the spawned cleanup task a moment to run.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            cancellations.lock().await.is_empty(),
            "guard's Drop should have removed the entry"
        );
    }
}
