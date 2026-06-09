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
mod git;
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
    ChatEvent, ChatMessage, ContentBlock, LlmConfig, LlmErrorCategory, MessageContent, Role,
    ToolDef,
};
use tools::read_guard::ReadGuard;
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
    /// Step 4 follow-up: per-session → active request id, for the
    /// destructive in-flight cancel hook (`delete_session`,
    /// `detach_worktree`, `delete_worktree`). When the user invokes
    /// any of these, we look up the session's active `request_id`
    /// and cancel it BEFORE the destructive work runs, so the LLM
    /// can't write to a half-deleted worktree.
    session_active_request: Arc<Mutex<HashMap<String, String>>>,
    /// Per-session read fingerprints. The `edit_file` tool consults
    /// this guard to ensure the LLM (a) read the file in the current
    /// session and (b) the file hasn't been modified on disk since.
    /// Lives in process state, not persisted: the first edit on a
    /// fresh restart will fail with "must read first" and the LLM
    /// re-reads. The guard is session-isolated so switching back to
    /// an old session restores its fingerprints.
    read_guard: ReadGuard,
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

        // Startup batch backfill of pre-PR2 project rows. PR2
        // (commit 8f25b7f) added `is_git_repo` / `git_branch` to the
        // `projects` table; projects created before that migration
        // have `is_git_repo=0, git_branch=NULL` and the UI's chip
        // falls back to the literal string "git", defeating the
        // spike #2c requirement of "display the real branch".
        //
        // The fix: spawn a fire-and-forget task that re-probes the
        // git status of every stale project, writes the result, and
        // emits a Tauri event so the frontend can refresh its
        // in-memory list. The spawn happens AFTER migrations run
        // and is `tauri::async_runtime::spawn`-based (matches the
        // pattern used by the `chat` command at lib.rs:366), so
        // `AppState::load` returns immediately and chat panel
        // loading is not blocked. On error we `tracing::warn!` and
        // let the next startup retry. See
        // `.trellis/tasks/06-06-pr2-backfill-fix/prd.md` for the
        // locked decision and the deferred "real-time git refresh"
        // v2 candidates.
        let backfill_pool = db.clone();
        let backfill_app = app.clone();
        tauri::async_runtime::spawn(async move {
            match projects::store::batch_reprobe_git_metadata(&backfill_pool).await {
                Ok(updated) => {
                    if updated > 0 {
                        if let Err(e) = backfill_app.emit("projects:refreshed", updated) {
                            tracing::warn!(
                                error = %e,
                                updated,
                                "emit projects:refreshed failed"
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "git metadata backfill failed");
                }
            }
        });

        Self {
            config,
            tools,
            db,
            cancellations: Arc::new(Mutex::new(HashMap::new())),
            session_active_request: Arc::new(Mutex::new(HashMap::new())),
            read_guard: ReadGuard::new(),
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

/// Tauri command: return the user's effective LLM config for the
/// frontend's `useConfigStore`.
///
/// PR2 (multi-model): the source of truth is the catalog
/// (`app_config.default_model_id` → `models` → `providers`), not
/// the env path. Env (`LlmConfig::from_env`) is now only the
/// cold-start fallback (kept around in `AppState::config`); this
/// IPC reads the catalog so the frontend's `model` field always
/// reflects the user's actively-selected model. The `model` field
/// is the catalog `display_name` (see D1 in the PR2 PRD) so the
/// StatusBar dropdown and the store agree.
///
/// Fallback: if the catalog is empty / `default_model_id` is unset
/// / the model row was deleted / the provider was deleted, the
/// response shape is preserved with `model = ""`,
/// `base_url = ""`, `configured = false` — the frontend's existing
/// "no model configured" warning renders as before.
#[tauri::command]
async fn get_llm_config(state: State<'_, Arc<AppState>>) -> Result<PublicLlmConfig, String> {
    // Resolve the default model id from app_config. The
    // `get_default_model` Tauri command duplicates this logic
    // and returns the full `ModelWithProvider` row; we re-read
    // here so this IPC stays a thin read of the catalog without
    // a second helper function.
    let default_id = db::get_config_value(&state.db, "default_model_id")
        .await
        .map_err(|e| format!("get_llm_config failed: {}", e))?;
    let Some(model_id) = default_id else {
        return Ok(PublicLlmConfig {
            model: String::new(),
            base_url: String::new(),
            configured: false,
        });
    };
    let models = db::list_models(&state.db)
        .await
        .map_err(|e| format!("get_llm_config failed: {}", e))?;
    let Some(mwp) = models.into_iter().find(|m| m.model.id == model_id) else {
        return Ok(PublicLlmConfig {
            model: String::new(),
            base_url: String::new(),
            configured: false,
        });
    };
    // Look up the parent provider to get its base_url + api_key.
    // `list_models` denormalizes `provider_display_name` +
    // `provider_protocol` but NOT `provider.api_key` (api_key
    // should never leave the backend except via a dedicated
    // command, and we don't want a frontend-readable field
    // accidentally containing the secret). We re-read the
    // provider row here.
    let providers = db::list_providers(&state.db)
        .await
        .map_err(|e| format!("get_llm_config failed: {}", e))?;
    let provider = providers
        .into_iter()
        .find(|p| p.id == mwp.model.provider_id);
    let (base_url, configured) = match provider {
        Some(p) => (p.base_url, !p.api_key.is_empty()),
        None => (String::new(), false),
    };
    Ok(PublicLlmConfig {
        model: mwp.model.display_name,
        base_url,
        configured,
    })
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
// Tauri commands — providers / models / default model (PR1 of multi-model)
// ---------------------------------------------------------------------------
//
// Wire shape (camelCase on the JS side per Tauri's default):
//   - ProviderRow:    { id, protocol, displayName, baseUrl, apiKey, ... }
//   - ModelRow:       { id, providerId, modelName, displayName, maxTokens,
//                       thinkingEffort, supportsThinking, contextWindow, ... }
//   - ModelWithProvider: ModelRow + { providerDisplayName, providerProtocol }
//
// Args follow the same convention: snake_case in Rust, camelCase from JS.

#[tauri::command]
async fn list_providers(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<db::ProviderRow>, String> {
    db::list_providers(&state.db)
        .await
        .map_err(|e| format!("list_providers failed: {}", e))
}

#[tauri::command]
async fn add_provider(
    state: State<'_, Arc<AppState>>,
    protocol: String,
    display_name: String,
    base_url: String,
    api_key: String,
) -> Result<db::ProviderRow, String> {
    db::create_provider(&state.db, &protocol, &display_name, &base_url, &api_key)
        .await
        .map_err(|e| format!("add_provider failed: {}", e))
}

#[tauri::command]
async fn update_provider(
    state: State<'_, Arc<AppState>>,
    id: String,
    protocol: String,
    display_name: String,
    base_url: String,
    api_key: String,
) -> Result<Option<db::ProviderRow>, String> {
    db::update_provider(&state.db, &id, &protocol, &display_name, &base_url, &api_key)
        .await
        .map_err(|e| format!("update_provider failed: {}", e))
}

#[tauri::command]
async fn delete_provider(state: State<'_, Arc<AppState>>, id: String) -> Result<bool, String> {
    db::delete_provider(&state.db, &id)
        .await
        .map_err(|e| format!("delete_provider failed: {}", e))
}

#[tauri::command]
async fn list_models(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<db::ModelWithProvider>, String> {
    db::list_models(&state.db)
        .await
        .map_err(|e| format!("list_models failed: {}", e))
}

#[tauri::command]
async fn add_model(
    state: State<'_, Arc<AppState>>,
    provider_id: String,
    model_name: String,
    display_name: String,
    max_tokens: Option<u32>,
    thinking_effort: Option<String>,
    supports_thinking: bool,
    context_window: u32,
) -> Result<db::ModelRow, String> {
    db::create_model(
        &state.db,
        &provider_id,
        &model_name,
        &display_name,
        max_tokens,
        thinking_effort.as_deref(),
        supports_thinking,
        context_window,
    )
    .await
    .map_err(|e| format!("add_model failed: {}", e))
}

#[tauri::command]
async fn update_model(
    state: State<'_, Arc<AppState>>,
    id: String,
    provider_id: String,
    model_name: String,
    display_name: String,
    max_tokens: Option<u32>,
    thinking_effort: Option<String>,
    supports_thinking: bool,
    context_window: u32,
) -> Result<Option<db::ModelRow>, String> {
    db::update_model(
        &state.db,
        &id,
        &provider_id,
        &model_name,
        &display_name,
        max_tokens,
        thinking_effort.as_deref(),
        supports_thinking,
        context_window,
    )
    .await
    .map_err(|e| format!("update_model failed: {}", e))
}

#[tauri::command]
async fn delete_model(state: State<'_, Arc<AppState>>, id: String) -> Result<bool, String> {
    db::delete_model(&state.db, &id)
        .await
        .map_err(|e| format!("delete_model failed: {}", e))
}

#[tauri::command]
async fn get_default_model(
    state: State<'_, Arc<AppState>>,
) -> Result<Option<db::ModelWithProvider>, String> {
    let id = match db::get_config_value(&state.db, "default_model_id").await {
        Ok(Some(id)) => id,
        Ok(None) => return Ok(None),
        Err(e) => return Err(format!("get_default_model failed: {}", e)),
    };
    let models = db::list_models(&state.db)
        .await
        .map_err(|e| format!("get_default_model failed: {}", e))?;
    Ok(models.into_iter().find(|m| m.model.id == id))
}

#[tauri::command]
async fn set_default_model(
    state: State<'_, Arc<AppState>>,
    model_id: String,
) -> Result<(), String> {
    db::set_config_value(&state.db, "default_model_id", &model_id)
        .await
        .map_err(|e| format!("set_default_model failed: {}", e))
}

// ---------------------------------------------------------------------------
// PR4 of multi-model task: per-session model override + test_provider
// ---------------------------------------------------------------------------

/// Update the per-session model override. The frontend's StatusBar
/// dropdown calls this when the user selects a different model for a
/// specific session. The value is stored as `sessions.model_id`
/// (soft FK to `models.id`). The chat command's `resolve_chat_provider`
/// reads this column and falls back to the global default when NULL.
#[tauri::command]
async fn update_session_model_id(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    model_id: String,
) -> Result<(), String> {
    db::update_session_model_id(&state.db, &session_id, &model_id)
        .await
        .map_err(|e| format!("update_session_model_id failed: {}", e))
}

/// Test a provider's connectivity by sending a lightweight request
/// with the given `base_url`, `api_key`, and `protocol`. Returns
/// a JSON object with `success`, `latencyMs`, and optional `error`.
///
/// - Anthropic: `POST {base_url}/v1/messages` with `max_tokens=1`
///   and a minimal user message. A 200 means success; 401/403 means
///   auth failure.
/// - OpenAI: `GET {base_url}/models` with `Authorization: Bearer`.
///   A 200 means success; 401 means auth failure.
///
/// The function does NOT access `AppState` — it only makes an HTTP
/// request to validate the credentials, keeping the test isolated
/// from the app's DB and LLM dispatch.
#[tauri::command]
async fn test_provider(
    base_url: String,
    api_key: String,
    protocol: String,
) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("failed to build HTTP client: {}", e))?;
    let start = std::time::Instant::now();

    let (success, error) = match protocol.as_str() {
        "anthropic" => {
            let url = format!("{}/v1/messages", base_url.trim_end_matches('/'));
            let body = serde_json::json!({
                "model": "claude-sonnet-4-5",
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "hi"}]
            });
            let resp = client
                .post(&url)
                .header("x-api-key", &api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| format!("request failed: {}", e))?;

            let status = resp.status();
            if status.is_success() {
                (true, None)
            } else {
                let body_text = resp.text().await.unwrap_or_default();
                (false, Some(format!("HTTP {}: {}", status, body_text.chars().take(200).collect::<String>())))
            }
        }
        "openai" => {
            let url = format!("{}/models", base_url.trim_end_matches('/'));
            let resp = client
                .get(&url)
                .header("authorization", format!("Bearer {}", api_key))
                .send()
                .await
                .map_err(|e| format!("request failed: {}", e))?;

            let status = resp.status();
            if status.is_success() {
                (true, None)
            } else {
                let body_text = resp.text().await.unwrap_or_default();
                (false, Some(format!("HTTP {}: {}", status, body_text.chars().take(200).collect::<String>())))
            }
        }
        _ => {
            (false, Some(format!("unsupported protocol: {}", protocol)))
        }
    };

    let latency_ms = start.elapsed().as_millis() as u64;
    Ok(serde_json::json!({
        "success": success,
        "latencyMs": latency_ms,
        "error": error,
    }))
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

    // Step 4 follow-up: worktree is now opt-in. We no longer require
    // the project to be a git repo (that was the step 4 v1 hard
    // guard) and we no longer auto-create a worktree. The session is
    // created in `WorktreeState::None`; the user calls
    // `attach_worktree` separately if they want isolation. Non-git
    // projects can now create sessions and send messages — the
    // pre-follow-up "step 4 requires all session-bearing projects
    // to be git repos" constraint is gone.
    //
    // We still load the project so we (a) verify it exists and
    // (b) record the current cwd against the project's path. The
    // project-not-found case is surfaced as a clear error.
    let _project = match db::get_project(&state.db, &project_id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return Err(format!("create_session: project '{}' not found", project_id));
        }
        Err(e) => return Err(format!("create_session: failed to load project: {}", e)),
    };

    let session_id = uuid::Uuid::new_v4().to_string();
    db::create_session(&state.db, &session_id, &project_id, &initial_cwd, &model)
        .await
        .map_err(|e| format!("create_session: db insert failed: {}", e))
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
async fn diff_worktree(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<git::diff::DiffResult, String> {
    // Look up the session to find its worktree. Pre-step-4 sessions
    // (worktree_path NULL) have no diff to show — return an empty
    // result rather than an error so the UI can render "no changes
    // yet" gracefully.
    let loaded = db::load_session(&state.db, &session_id)
        .await
        .map_err(|e| format!("diff_worktree: failed to load session: {}", e))?
        .ok_or_else(|| format!("diff_worktree: session '{}' not found", session_id))?;

    let worktree_path = match loaded.session.worktree_path.as_deref() {
        Some(p) if !p.trim().is_empty() => p,
        _ => {
            // Pre-step-4 session: no worktree, no diff. Returning an
            // empty DiffResult (vs. an error) lets the frontend
            // render the same "no changes" state for old + new
            // sessions without branching on the cause.
            tracing::debug!(
                session_id = %session_id,
                "diff_worktree: pre-step-4 session, no worktree, returning empty"
            );
            return Ok(git::diff::DiffResult { files: vec![] });
        }
    };

    git::diff::diff_worktree(std::path::Path::new(worktree_path), &session_id)
        .map_err(|e| format!("diff_worktree: {}", e))
}

#[tauri::command]
async fn delete_session(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<(), String> {
    // Step 4 follow-up: in-flight cancel hook. If a chat stream is
    // running for this session, cancel it BEFORE the destructive
    // work. The frontend is expected to disable the delete button
    // while streaming (REQ-13) and to call `cancel_chat` first, but
    // the backend is the last line of defense — a race between
    // the IPC arrival and the agent loop's next event must not
    // leave a stream writing to a half-deleted session.
    cancel_inflight_for_session(&state.cancellations, &state.session_active_request, &session_id).await;

    // Clear the in-memory ReadGuard for this session so we don't leak
    // fingerprints for a session the user just deleted. The DB delete
    // is the source of truth; the guard is just a cache.
    state.read_guard.clear_session(&session_id).await;

    // Best-effort cleanup of disk-spilled shell outputs (PRD §R8).
    // We do this BEFORE the DB delete so we can still read the
    // session's `current_cwd`; `db::load_session` is cheap (one
    // indexed row + an `IN (...)` over messages) and a missing
    // session short-circuits cleanly. Failures here are logged by
    // `cleanup_outputs_dir` and do NOT cascade — the user's
    // primary intent is "delete the session", not "clean my disk";
    // surfacing a delete error because some leftover .txt file
    // was unlinked elsewhere would be a worse UX.
    let session_for_cleanup = db::load_session(&state.db, &session_id)
        .await
        .ok()
        .flatten();

    if let Some(ref loaded) = session_for_cleanup {
        let cwd = &loaded.session.current_cwd;
        if !cwd.trim().is_empty() {
            tools::shell::cleanup_outputs_dir(std::path::Path::new(cwd)).await;
        }
    }

    // Step 4 follow-up: best-effort worktree + branch cleanup.
    // Triggered when the session's `worktree_state` is `active`
    // (NOT `detached` — a detached session's worktree was
    // already removed; deleting a detached session should NOT
    // touch the on-disk artifacts, the user can re-attach to
    // the branch via a new session).
    //
    // Failure modes are tolerated: a stuck worktree dir is logged
    // and the DB delete still proceeds, mirroring the spirit of the
    // shell-outputs cleanup above. A user can `rm -rf` the path by
    // hand if it gets stuck.
    if let Some(ref loaded) = session_for_cleanup {
        if loaded.session.worktree_state == db::WorktreeState::Active {
            if let Some(wt_path) = loaded.session.worktree_path.as_deref() {
                if let Ok(Some(project)) =
                    db::get_project(&state.db, &loaded.session.project_id).await
                {
                    if let Err(e) = git::destroy_worktree(
                        std::path::Path::new(&project.path),
                        std::path::Path::new(wt_path),
                        &session_id,
                    ) {
                        tracing::warn!(
                            session_id = %session_id,
                            worktree = %wt_path,
                            error = %e,
                            "worktree cleanup failed during session delete (non-fatal)"
                        );
                    }
                }
            }
        }
    }

    db::delete_session(&state.db, &session_id)
        .await
        .map_err(|e| format!("delete_session failed: {}", e))
}

// ---------------------------------------------------------------------------
// Step 4 follow-up: opt-in worktree commands
// ---------------------------------------------------------------------------

/// Cancel an in-flight chat request for `session_id`, if any. Called
/// at the entry of `delete_session` / `detach_worktree` /
/// `delete_worktree` so a streaming LLM can't write into a
/// half-destroyed session/worktree. No-op when the session isn't
/// streaming. The cancellation is best-effort: the agent loop
/// notices on its next event boundary and bails out cleanly,
/// emitting a `done` event with `stop_reason: "cancelled"`.
async fn cancel_inflight_for_session(
    cancellations: &Arc<Mutex<HashMap<String, CancellationToken>>>,
    session_active_request: &Arc<Mutex<HashMap<String, String>>>,
    session_id: &str,
) {
    let request_id = {
        let map = session_active_request.lock().await;
        map.get(session_id).cloned()
    };
    let Some(rid) = request_id else {
        return;
    };
    let token = {
        let map = cancellations.lock().await;
        map.get(&rid).cloned()
    };
    if let Some(t) = token {
        t.cancel();
        tracing::info!(
            session_id = %session_id,
            request_id = %rid,
            "destructive op: cancelled in-flight chat"
        );
    }
    // The `session_active_request` entry is removed by the
    // `CancellationGuard` on Drop, after the agent loop exits.
    // We don't remove it here — the in-flight loop still uses it
    // during cleanup.
}

#[tauri::command]
async fn attach_worktree(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<db::SessionRow, String> {
    // Step 4 follow-up: opt-in worktree attach. Loads the session +
    // project, validates the project is a git repo (the only
    // attach-time requirement), refuses if the project root has
    // uncommitted changes (REQ-8), creates the worktree + branch
    // (disk-first), and writes the new state to the DB. Injects a
    // system event into the session's history so the next LLM
    // turn sees the worktree transition.
    let loaded = db::load_session(&state.db, &session_id)
        .await
        .map_err(|e| format!("attach_worktree: failed to load session: {}", e))?
        .ok_or_else(|| format!("attach_worktree: session '{}' not found", session_id))?;
    let project = db::get_project(&state.db, &loaded.session.project_id)
        .await
        .map_err(|e| format!("attach_worktree: failed to load project: {}", e))?
        .ok_or_else(|| {
            format!(
                "attach_worktree: project '{}' not found",
                loaded.session.project_id
            )
        })?;
    if !project.is_git_repo {
        return Err(format!(
            "attach_worktree: project '{}' is not a git repository",
            project.name
        ));
    }

    // State machine guard: attach only valid from `none` or
    // `detached`. A session in `active` state already has a
    // worktree — attaching again is a user error.
    match loaded.session.worktree_state {
        db::WorktreeState::None | db::WorktreeState::Detached => {}
        db::WorktreeState::Active => {
            return Err(format!(
                "attach_worktree: session '{}' already has an active worktree",
                session_id
            ));
        }
    }

    // Reject if the project root is dirty (REQ-8). The new worktree
    // would diverge from a dirty base, which silently loses the
    // user's WIP.
    let project_path = std::path::Path::new(&project.path);
    if let Err(msg) = git::check_clean(project_path) {
        return Err(format!("attach_worktree: {}", msg));
    }

    // Disk first, then DB. If the worktree creation fails we
    // don't touch the DB; the user can retry.
    let data_dir = git::data_dir();
    let wt_path = git::session_worktree_path(&data_dir, &project.id, &session_id);
    git::create_worktree(project_path, &wt_path, &session_id)
        .map_err(|e| format!("attach_worktree: worktree creation failed: {}", e))?;

    // Now write the new state to the DB.
    let wt_str = wt_path.to_str().map(str::to_string);
    db::set_worktree_state(
        &state.db,
        &session_id,
        db::WorktreeState::Active,
        wt_str.as_deref(),
        // Preserve last_worktree_path on a re-attach (detach
        // already populated it; attaching from `none` keeps NULL).
        loaded.session.last_worktree_path.as_deref(),
    )
    .await
    .map_err(|e| format!("attach_worktree: db update failed: {}", e))?;

    // Inject system event so the next LLM turn sees the transition.
    let branch = git::worktree::branch_name(&session_id);
    let wt_display = wt_path.display().to_string();
    let event_text = format!(
        "worktree attached: {} on branch {}",
        wt_display, branch
    );
    if let Err(e) =
        db::insert_system_event(&state.db, &session_id, &event_text, "attached").await
    {
        tracing::warn!(
            error = %e,
            session_id = %session_id,
            "attach_worktree: insert_system_event failed (non-fatal)"
        );
    }

    // Reload and return the canonical row.
    let updated = db::load_session(&state.db, &session_id)
        .await
        .map_err(|e| format!("attach_worktree: reload failed: {}", e))?
        .ok_or_else(|| {
            format!(
                "attach_worktree: session '{}' disappeared after attach",
                session_id
            )
        })?;
    Ok(updated.session)
}

#[tauri::command]
async fn detach_worktree(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<db::SessionRow, String> {
    // Step 4 follow-up: opt-in worktree detach. Refuses if the
    // session isn't `active` (detach is only valid FROM active),
    // refuses if the worktree is dirty (REQ-9: don't strand
    // WIP), preserves the path in `last_worktree_path`, and
    // clears `worktree_path` so the next tool call falls back
    // to `project.path` via the `WorktreeState::Detached`
    // fallback in the agent loop.
    cancel_inflight_for_session(
        &state.cancellations,
        &state.session_active_request,
        &session_id,
    )
    .await;

    let loaded = db::load_session(&state.db, &session_id)
        .await
        .map_err(|e| format!("detach_worktree: failed to load session: {}", e))?
        .ok_or_else(|| format!("detach_worktree: session '{}' not found", session_id))?;

    if loaded.session.worktree_state != db::WorktreeState::Active {
        return Err(format!(
            "detach_worktree: session '{}' is not in 'active' state (current: {:?})",
            session_id, loaded.session.worktree_state
        ));
    }
    let wt_path_str = loaded
        .session
        .worktree_path
        .clone()
        .ok_or_else(|| "detach_worktree: active session has no worktree_path".to_string())?;
    let wt_path = std::path::PathBuf::from(&wt_path_str);

    // REQ-9: refuse if the worktree has uncommitted changes.
    if let Err(msg) = git::check_clean(&wt_path) {
        return Err(format!("detach_worktree: {}", msg));
    }

    // Write the new state FIRST. If the DB update fails, we
    // haven't touched the disk; user can retry. The `git::
    // destroy_worktree` is intentionally NOT called here —
    // detach is "unbind from the session" not "delete the
    // artifacts". The branch + directory stay on disk; the
    // user can re-attach or inspect the branch via a new
    // session.
    db::set_worktree_state(
        &state.db,
        &session_id,
        db::WorktreeState::Detached,
        None,
        Some(&wt_path_str),
    )
    .await
    .map_err(|e| format!("detach_worktree: db update failed: {}", e))?;

    let branch = git::worktree::branch_name(&session_id);
    let event_text = format!(
        "worktree detached from {} (changes preserved on branch {})",
        wt_path.display(),
        branch
    );
    if let Err(e) =
        db::insert_system_event(&state.db, &session_id, &event_text, "detached").await
    {
        tracing::warn!(
            error = %e,
            session_id = %session_id,
            "detach_worktree: insert_system_event failed (non-fatal)"
        );
    }

    let updated = db::load_session(&state.db, &session_id)
        .await
        .map_err(|e| format!("detach_worktree: reload failed: {}", e))?
        .ok_or_else(|| {
            format!(
                "detach_worktree: session '{}' disappeared after detach",
                session_id
            )
        })?;
    Ok(updated.session)
}

#[tauri::command]
async fn delete_worktree(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<db::SessionRow, String> {
    // Step 4 follow-up: opt-in worktree delete (independent of
    // detach). Physically removes the worktree directory +
    // branch from disk and clears the worktree state columns.
    // Distinct from `detach_worktree` (which only unbinds):
    // a deleted worktree's branch is GONE; an LLM cannot
    // later re-attach to it.
    cancel_inflight_for_session(
        &state.cancellations,
        &state.session_active_request,
        &session_id,
    )
    .await;

    let loaded = db::load_session(&state.db, &session_id)
        .await
        .map_err(|e| format!("delete_worktree: failed to load session: {}", e))?
        .ok_or_else(|| format!("delete_worktree: session '{}' not found", session_id))?;

    // Delete is valid from `active` OR `detached`. A detached
    // session's worktree_path is NULL (the directory is still
    // on disk if the path was preserved as `last_worktree_path`,
    // but the branch is what we need to remove).
    if loaded.session.worktree_state != db::WorktreeState::Active
        && loaded.session.worktree_state != db::WorktreeState::Detached
    {
        return Err(format!(
            "delete_worktree: session '{}' has no worktree to delete (state: {:?})",
            session_id, loaded.session.worktree_state
        ));
    }

    let project = db::get_project(&state.db, &loaded.session.project_id)
        .await
        .map_err(|e| format!("delete_worktree: failed to load project: {}", e))?
        .ok_or_else(|| {
            format!(
                "delete_worktree: project '{}' not found",
                loaded.session.project_id
            )
        })?;

    // For `active` we have a worktree_path; for `detached` we
    // may only have last_worktree_path. The actual on-disk
    // directory path that `git::destroy_worktree` needs is
    // the worktree_path (active) or last_worktree_path
    // (detached). The branch is always `session/<id>` and
    // gets deleted regardless of state.
    let worktree_path_for_destroy: Option<std::path::PathBuf> = loaded
        .session
        .worktree_path
        .as_deref()
        .or(loaded.session.last_worktree_path.as_deref())
        .map(std::path::PathBuf::from);
    let branch = git::worktree::branch_name(&session_id);

    if let Some(wtp) = &worktree_path_for_destroy {
        if let Err(e) =
            git::destroy_worktree(std::path::Path::new(&project.path), wtp, &session_id)
        {
            tracing::warn!(
                session_id = %session_id,
                worktree = %wtp.display(),
                error = %e,
                "delete_worktree: destroy_worktree failed (non-fatal)"
            );
        }
    } else {
        // No worktree path stored but the state is active or
        // detached. Best-effort: still try to remove the
        // branch, since the on-disk directory may have been
        // manually removed.
        let worktree_lookup = &session_id;
        if let Ok(repo) = git2::Repository::open(std::path::Path::new(&project.path)) {
            if let Ok(mut b) =
                repo.find_branch(&branch, git2::BranchType::Local)
            {
                let _ = b.delete();
            }
            if let Ok(wt) = repo.find_worktree(worktree_lookup) {
                let _ = wt.prune(None);
            }
        }
    }

    // DB state: clear worktree_path AND last_worktree_path; the
    // branch is gone so re-attach is no longer meaningful.
    db::set_worktree_state(
        &state.db,
        &session_id,
        db::WorktreeState::None,
        None,
        None,
    )
    .await
    .map_err(|e| format!("delete_worktree: db update failed: {}", e))?;

    let event_text = format!(
        "worktree deleted: branch {} and dir {} removed",
        branch,
        worktree_path_for_destroy
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<unknown>".to_string())
    );
    if let Err(e) =
        db::insert_system_event(&state.db, &session_id, &event_text, "deleted").await
    {
        tracing::warn!(
            error = %e,
            session_id = %session_id,
            "delete_worktree: insert_system_event failed (non-fatal)"
        );
    }

    let updated = db::load_session(&state.db, &session_id)
        .await
        .map_err(|e| format!("delete_worktree: reload failed: {}", e))?
        .ok_or_else(|| {
            format!(
                "delete_worktree: session '{}' disappeared after delete",
                session_id
            )
        })?;
    Ok(updated.session)
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
/// it only holds an `Arc<Mutex<HashMap<...>>>` clones, which
/// themselves are `Send + Sync`.
struct CancellationGuard {
    cancellations: Arc<Mutex<HashMap<String, CancellationToken>>>,
    /// Step 4 follow-up: also clears the `session_active_request`
    /// entry for `session_id`, so a destructive command that
    /// looks up "is this session in-flight?" after the guard
    /// drops sees an empty answer (matches the agent loop's own
    /// exit semantics).
    session_active_request: Arc<Mutex<HashMap<String, String>>>,
    request_id: String,
    session_id: String,
}

impl Drop for CancellationGuard {
    fn drop(&mut self) {
        let cancellations = self.cancellations.clone();
        let session_active_request = self.session_active_request.clone();
        let request_id = self.request_id.clone();
        let session_id = self.session_id.clone();
        tauri::async_runtime::spawn(async move {
            let mut map = cancellations.lock().await;
            map.remove(&request_id);
            drop(map);
            let mut s2p = session_active_request.lock().await;
            s2p.remove(&session_id);
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
    // PR2 (multi-model): the chat command no longer reads
    // `state.config` for LLM dispatch. Instead we resolve a
    // `Box<dyn Provider>` from the catalog (default model →
    // provider) at the start of every chat invocation. The
    // `state.config` field is preserved on `AppState` for
    // cold-start fallback / `get_llm_config` (which itself
    // was switched to catalog in PR2 — see its impl).
    let tool_defs = state.tools.clone();
    let db = state.db.clone();
    let cancellations = state.cancellations.clone();
    let session_active_request = state.session_active_request.clone();
    let read_guard = state.read_guard.clone();
    let rid = request_id;
    let app_handle = app.clone();

    // PR2 pre-flight: resolve the catalog → provider. The
    // failure modes map 1:1 to PRD §Q2's locked-in user-facing
    // messages, surfaced as `ChatEvent::Error` so the frontend
    // can render the same toast path it uses for other LLM
    // errors. We do this BEFORE registering the cancellation
    // token + session_active_request entry because a pre-flight
    // failure is synchronous (no LLM call has started), so
    // there is nothing to cancel.
    let resolved = match resolve_chat_provider(&db).await {
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
                event: ChatEvent::Error {
                    message: msg,
                    category,
                },
            };
            app.emit("chat-event", payload).map_err(|e| e.to_string())?;
            return Ok(());
        }
    };
    let provider: Box<dyn llm::Provider> = resolved.provider;
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
    // Step 4 follow-up: also register this session → request_id
    // mapping so destructive operations (delete_session,
    // detach_worktree, delete_worktree) can find and cancel the
    // in-flight stream. The entry is removed by the
    // CancellationGuard on Drop.
    {
        let mut map = session_active_request.lock().await;
        map.insert(session_id.clone(), rid.clone());
    }

    tauri::async_runtime::spawn(async move {
        // The token's clone moves into this task; cancellation in
        // `cancel_chat` is observed via the original we just put in
        // the map. Both must outlive any `select!` arm that awaits
        // the token.
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
        // The agent's sandbox root: this is the directory the
        // boundary check is enforced against. For step 4 sessions
        // (every new session) it is the per-session worktree path
        // recorded in `sessions.worktree_path`. For pre-step-4
        // sessions (the column is NULL because they were created
        // before the migration ran) we fall back to the project
        // path, which is the legacy sandbox. Either way, this is a
        // canonical absolute path that has been validated by
        // `assert_within_root` (a self-check on the project path
        // itself, to surface bad project paths early).
        let session_root_raw = loaded_session
            .session
            .worktree_path
            .clone()
            .unwrap_or_else(|| project.path.clone());
        let worktree_path = match projects::boundary::assert_within_root(
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
        let session_cwd = match projects::boundary::assert_within_root(
            &worktree_path,
            std::path::Path::new(&session_cwd_raw),
        ) {
            Ok(p) => p,
            Err(e) => {
                // Defensive: if the stored cwd is no longer reachable
                // (e.g. user deleted a directory mid-session), fall
                // back to the worktree / project root. The next
                // shell tool call will move `turn_ctx.cwd` to
                // wherever it goes.
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

        // Step 4 follow-up Bug 3: build the LLM system prompt **once**
        // per chat invocation. The prompt describes the session's
        // working directory, worktree state, branch + HEAD SHA so the
        // model is explicitly grounded on every request. Pre-fix, the
        // `system` field was hard-coded to `None` and the only
        // worktree-state signal the model had was the post-hoc
        // `[worktree event]` user-role message in history — which the
        // model honestly described as "the user told me" rather than
        // "I'm running in a worktree".
        //
        // We compute the HEAD SHA from the on-disk session root
        // (which is either the worktree or the project root,
        // depending on `worktree_state`). The lookup is best-effort:
        // a non-git project or an empty repo gives a placeholder
        // string, never an error — the prompt is a hint, not a
        // contract field.
        //
        // Building once (before the `for turn in 1..=MAX_TURNS`
        // loop) instead of per-turn is intentional: the worktree
        // state and HEAD SHA cannot change between turns of the
        // same agent loop (the LLM only runs tools and the
        // `attach_worktree` / `detach_worktree` commands have a
        // cancel hook that aborts in-flight chats). Re-reading
        // `.git/HEAD` on every turn would be cheap but wasteful.
        let head_sha = lookup_head_sha(&worktree_path);
        let system_prompt = build_system_prompt(
            &loaded_session.session,
            &project,
            &worktree_path,
            &head_sha,
        );

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
            // PR2: dispatch through the catalog-resolved provider
            // rather than the legacy `chat_stream_with_tools`
            // free function. The provider was constructed once
            // before the spawn (above), so every turn of the
            // 20-turn agent loop uses the same `Box<dyn Provider>`
            // — no per-turn protocol re-resolution, and the
            // user's protocol choice is stable across turns.
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
                // BUG FIX (2013 tool_use orphan): if cancel hit
                // after the LLM emitted one or more `tool_use`
                // blocks, persist a synthetic `tool_result` user
                // message mirroring them. Pre-fix we `return`d
                // here without executing tools, so the DB ended
                // up with `assistant(tool_use)` but no matching
                // `user(tool_result)` — the next `send` built a
                // history where `tool_use` had no follow-up
                // `tool_result`, and the Anthropic API returned
                // 2013 ("tool call result does not follow tool
                // call"). Synthesizing the result with
                // `is_error: true` keeps the wire format
                // self-consistent and lets the model know the
                // tool did not actually run.
                //
                // The shape itself (one ToolResult block per
                // tool_use, role=User, is_error=true) is verified
                // by `tests::synthetic_tool_result_message_*`
                // below — we keep the persistence step here
                // because it depends on the live `db`, `seq`,
                // and `messages` locals.
                if !tool_calls.is_empty() {
                    let tool_result_msg = build_synthetic_tool_result_message(&tool_calls);
                    if let Err(e) = db::persist_turn(
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
                    // No `seq += 1` here — the cancel branch
                    // returns immediately, so the incremented
                    // seq would never be read. (The normal
                    // tool_result path at line ~1602 keeps the
                    // `+= 1` because the agent loop continues
                    // and uses the new seq for the next
                    // assistant turn.)
                    tracing::warn!(
                        request_id = %rid,
                        tool_count = tool_calls.len(),
                        "chat: cancelled — persisted synthetic tool_result blocks to keep history self-consistent (prevent 2013 on next send)"
                    );
                }

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
                let (content, is_error, update) = tools::execute_tool(
                    name,
                    input,
                    &current_ctx,
                    Some(&read_guard),
                    Some(&session_id),
                )
                .await;
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

                // Step 4 follow-up (REQ-16): wrap the tool result in
                // a JSON envelope that includes the worktree's
                // current cwd. The LLM uses the `cwd` field to
                // understand "this file was at this path on disk
                // when the tool ran" — important after worktree
                // transitions (attach/detach), when the agent's
                // mental model of the worktree can drift from the
                // actual on-disk state. The legacy `result` field
                // is the same string the LLM would have seen
                // pre-follow-up, so downstream tool-specific
                // prompts continue to work.
                let envelope_str = tool_result_envelope(&content, &current_ctx.worktree_path);

                let _ = app_handle.emit(
                    "tool:result",
                    ToolResultPayload {
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

/// Step 4 follow-up (REQ-16): wrap a tool result `content` string
/// in a JSON envelope that also carries the worktree's current
/// `cwd`. The LLM uses the `cwd` field to understand "this file
/// was at this path on disk when the tool ran" — important after
/// worktree transitions (attach/detach), when the agent's mental
/// model of the worktree can drift from the actual on-disk state.
/// The `result` field holds the legacy content string so
/// downstream tool-specific prompts continue to work without
/// re-parsing.
///
/// Extracted as a free function (not inlined in the chat loop)
/// so it can be unit-tested for the round-trip shape — see
/// `tests::tool_result_envelope_round_trip` below. The frontend
/// has a matching lenient parser in `extractToolResultDisplay`
/// (`app/src/utils/messageFormat.ts`).
fn tool_result_envelope(content: &str, worktree_path: &std::path::Path) -> String {
    let cwd_str = worktree_path.to_string_lossy().to_string();
    serde_json::json!({
        "result": content,
        "cwd": cwd_str,
    })
    .to_string()
}

/// BUG FIX (2013 tool_use orphan): build a synthetic
/// `user`-role [`ChatMessage`] carrying one
/// `ContentBlock::ToolResult` per `(id, name, _input)` triple
/// the LLM emitted before the user cancelled. The block's
/// `content` tells the LLM the tool never ran; `is_error: true`
/// makes the failure explicit on the wire.
///
/// Why a helper: the inline shape is verbose, and the
/// invariant we care about (one ToolResult block per tool_use,
/// with matching `tool_use_id`, is_error=true, role=User) is
/// what unblocks the Anthropic 2013 error on the next
/// `send()`. Extracting it lets a unit test assert the
/// invariant end-to-end without spinning up an LLM stream,
/// Tauri AppHandle, or real DB. See
/// `tests::synthetic_tool_result_message_mirrors_tool_calls`
/// and `tests::synthetic_tool_result_message_empty_when_no_tool_calls`
/// below.
///
/// The `Role` is `User` (not `Tool`) per the Anthropic
/// Messages API contract: `tool_result` blocks only ever
/// appear inside `role: "user"` messages.
fn build_synthetic_tool_result_message(
    tool_calls: &[(String, String, serde_json::Value)],
) -> ChatMessage {
    let blocks: Vec<ContentBlock> = tool_calls
        .iter()
        .map(|(id, name, _input)| {
            let content = format!(
                "Tool execution was interrupted: the user stopped the request or the \
session was cancelled before the tool could run. The tool {} did not run.",
                name
            );
            ContentBlock::ToolResult {
                tool_use_id: id.clone(),
                content,
                is_error: true,
            }
        })
        .collect();
    ChatMessage {
        role: Role::User,
        content: MessageContent::Blocks(blocks),
    }
}

/// Step 4 follow-up Bug 3: read the HEAD commit SHA of a git
/// working directory and return the first 7 characters (the
/// classic git short-SHA). Returns a placeholder string when the
/// path is not a git repo, libgit2 fails to open it, or the repo
/// has no commits yet (e.g. a freshly-`git init`'d empty repo).
///
/// Best-effort by design: this is consumed only by
/// `build_system_prompt` as a hint to the LLM about the current
/// HEAD; we never want a transient git error to surface as a
/// chat failure.
fn lookup_head_sha(path: &std::path::Path) -> String {
    if !path.join(".git").exists() {
        return "not a git repo".to_string();
    }
    let repo = match git2::Repository::open(path) {
        Ok(r) => r,
        Err(_) => return "not a git repo".to_string(),
    };
    let head = match repo.head() {
        Ok(h) => h,
        Err(_) => return "no commits yet".to_string(),
    };
    let commit = match head.peel_to_commit() {
        Ok(c) => c,
        Err(_) => return "no commits yet".to_string(),
    };
    let full = commit.id().to_string();
    // Classic git short-SHA: first 7 chars.
    full.chars().take(7).collect()
}

/// Step 4 follow-up Bug 3: construct the per-session system prompt
/// the LLM sees at the top of every chat request. The prompt
/// describes the session's project, working directory, and
/// worktree state so the model is grounded on every turn.
///
/// Pre-fix, the request body's `system` field was hard-coded to
/// `None` and the only worktree signal the model had was a
/// `[worktree event]` user-role message injected after a
/// successful `attach_worktree` / `detach_worktree` /
/// `delete_worktree`. The user-role injection works for "what
/// just happened" but the model honestly answered "no" when
/// asked "does your system prompt mention you're in a worktree"
/// — because the system prompt field was empty. This function
/// is the single source of truth for the prompt; any future
/// addition (e.g. "the project's main branch is `main`") goes
/// here.
///
/// Three worktree-state phrasings:
/// - `Active` → "ACTIVE on branch 'session/<id>' (HEAD <short_sha>)"
/// - `Detached` → "DETACHED — was on branch 'session/<id>'
///   (HEAD <short_sha>), currently in project root"
/// - `None` → "NONE — running in project root"
///
/// Non-git projects get an "N/A — non-git project" suffix on the
/// worktree line (and HEAD line shows whatever `lookup_head_sha`
/// returned).
///
/// **Privacy**: only the `session_id`, `project.name`, `project.path`,
/// `ctx_root`, and short HEAD SHA are emitted. No user messages or
/// tool inputs are echoed.
fn build_system_prompt(
    session: &db::SessionRow,
    project: &projects::ProjectRow,
    ctx_root: &std::path::Path,
    head_sha: &str,
) -> String {
    let branch = git::worktree::branch_name(&session.id);
    let worktree_line = if !project.is_git_repo {
        "N/A — non-git project".to_string()
    } else {
        match session.worktree_state {
            db::WorktreeState::Active => {
                format!("ACTIVE on branch '{}' (HEAD {})", branch, head_sha)
            }
            db::WorktreeState::Detached => format!(
                "DETACHED — was on branch '{}' (HEAD {}), currently in project root",
                branch, head_sha
            ),
            db::WorktreeState::None => "NONE — running in project root".to_string(),
        }
    };

    format!(
        "You are a coding agent. You have access to tools (read_file, write_file, \
edit_file, shell, grep, glob, list_dir). All file paths in tool inputs are \
relative to the session's working directory.\n\
\n\
Session context:\n\
- Session ID: {session_id}\n\
- Project: {project_name} ({project_path})\n\
- Working directory: {cwd}\n\
- Worktree: {worktree_line}\n\
- Available tool result envelope: {{\"result\": \"<content>\", \"cwd\": \"<worktree_path>\"}} \
— `cwd` tells you which root the tool ran against when worktree transitions happen mid-session.",
        session_id = session.id,
        project_name = project.name,
        project_path = project.path,
        cwd = ctx_root.display(),
        worktree_line = worktree_line,
    )
}

/// PR2 (multi-model) catalog resolution result. The chat command
/// calls [`resolve_chat_provider`] at the start of every chat
/// invocation to obtain the `Box<dyn Provider>` to use for the
/// 20-turn agent loop. The three error variants map 1:1 to the
/// pre-flight error messages the PR2 PRD §Q2 locked in (see
/// `PreFlightError::auth_message` / `invalid_request_message`).
struct ResolvedChatProvider {
    provider: Box<dyn llm::Provider>,
    model_display_name: String,
    provider_display_name: String,
}

#[derive(Debug)]
enum PreFlightError {
    /// The chosen default model is missing (no `default_model_id`
    /// in `app_config`, or the catalog has no matching `models` row).
    /// PRD Q2 #2: "没有可用 model,请到 Settings 选 default model".
    NoModel,
    /// The chosen default model points at a provider row that was
    /// deleted. PRD Q2 #3: "default model 指向的 provider 已被删除,
    /// 请到 Settings 重选".
    ProviderMissing,
    /// The chosen provider's `api_key` is empty. PRD Q2 #1:
    /// "请到 Settings 填 {provider_display_name} 的 api_key".
    EmptyApiKey { provider_display_name: String },
    /// The dispatch in `build_provider` refused the protocol
    /// (e.g. `openai` not implemented yet, or an unknown protocol
    /// string from a forward-compat DB write).
    BuildFailed(llm::ProviderBuildError),
}

impl PreFlightError {
    /// Return `(user_message, category)`. The user-facing message
    /// follows the locked-in PRD §Q2 copy.
    fn user_message_and_category(&self) -> (String, LlmErrorCategory) {
        match self {
            PreFlightError::NoModel => (
                "没有可用 model,请到 Settings 选 default model".to_string(),
                LlmErrorCategory::InvalidRequest,
            ),
            PreFlightError::ProviderMissing => (
                "default model 指向的 provider 已被删除,请到 Settings 重选".to_string(),
                LlmErrorCategory::InvalidRequest,
            ),
            PreFlightError::EmptyApiKey {
                provider_display_name,
            } => (
                format!(
                    "请到 Settings 填 {} 的 api_key",
                    provider_display_name
                ),
                LlmErrorCategory::Auth,
            ),
            PreFlightError::BuildFailed(e) => (
                format!("无法构造 LLM provider: {}", e),
                LlmErrorCategory::InvalidRequest,
            ),
        }
    }
}

/// PR2 catalog resolution. Reads the default model id from
/// `app_config`, finds the corresponding `ModelWithProvider` row,
/// looks up the parent provider's `api_key` (for the pre-flight
/// check), and constructs a `Box<dyn Provider>` via
/// [`llm::build_provider`].
///
/// Returns one of the four [`PreFlightError`] variants on
/// failure. The `chat` command turns the first three into the
/// locked-in PRD §Q2 user-facing messages; the fourth is a
/// generic dispatcher error (e.g. OpenAI not implemented yet)
/// wrapped as `InvalidRequest`.
///
/// Resolved once per `chat` invocation. All 20 turns of the
/// agent loop use the same provider instance so the wire
/// protocol is consistent within a single chat (the user
/// can't change protocol mid-loop — they have to start a new
/// chat for that).
async fn resolve_chat_provider(
    db: &SqlitePool,
) -> Result<ResolvedChatProvider, PreFlightError> {
    // 1. Find the default model id.
    let default_id = db::get_config_value(db, "default_model_id")
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "resolve_chat_provider: get_config_value failed");
            PreFlightError::NoModel
        })?
        .ok_or(PreFlightError::NoModel)?;

    // 2. Find the matching model row.
    let models = db::list_models(db).await.map_err(|e| {
        tracing::error!(error = %e, "resolve_chat_provider: list_models failed");
        PreFlightError::NoModel
    })?;
    let mwp = models
        .into_iter()
        .find(|m| m.model.id == default_id)
        .ok_or(PreFlightError::NoModel)?;

    // 3. Find the parent provider row. The denormalized
    //    `ModelWithProvider` carries `provider_display_name` +
    //    `provider_protocol` but NOT `api_key` (the secret stays
    //    server-side). We re-read via `list_providers` to get
    //    `api_key` for the pre-flight check.
    let providers = db::list_providers(db).await.map_err(|e| {
        tracing::error!(error = %e, "resolve_chat_provider: list_providers failed");
        PreFlightError::ProviderMissing
    })?;
    let provider_row = providers
        .into_iter()
        .find(|p| p.id == mwp.model.provider_id)
        .ok_or(PreFlightError::ProviderMissing)?;

    // 4. Pre-flight: empty api_key. The factory would still
    //    succeed (it doesn't check), but the resulting
    //    `LlmConfig` would 401 on the first request. Better to
    //    reject here with a clear message.
    if provider_row.api_key.is_empty() {
        return Err(PreFlightError::EmptyApiKey {
            provider_display_name: provider_row.display_name.clone(),
        });
    }

    // 5. Build the provider. `build_provider` itself can fail
    //    (OpenAI / unknown protocol), in which case we surface
    //    the typed `ProviderBuildError` for the chat command to
    //    wrap as an `InvalidRequest` IPC error.
    let provider = llm::build_provider(&provider_row, &mwp.model)
        .map_err(PreFlightError::BuildFailed)?;

    Ok(ResolvedChatProvider {
        provider,
        model_display_name: mwp.model.display_name.clone(),
        provider_display_name: provider_row.display_name.clone(),
    })
}

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
            list_providers,
            add_provider,
            update_provider,
            delete_provider,
            list_models,
            add_model,
            update_model,
            delete_model,
            get_default_model,
            set_default_model,
            update_session_model_id,
            test_provider,
            list_sessions,
            create_session,
            load_session,
            delete_session,
            attach_worktree,
            detach_worktree,
            delete_worktree,
            diff_worktree,
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
        let session_active_request: Arc<Mutex<HashMap<String, String>>> =
            Arc::new(Mutex::new(HashMap::new()));
        {
            let mut map = cancellations.lock().await;
            map.insert("rid-g".to_string(), CancellationToken::new());
        }
        {
            let mut s2p = session_active_request.lock().await;
            s2p.insert("sid-g".to_string(), "rid-g".to_string());
        }
        assert_eq!(cancellations.lock().await.len(), 1);
        {
            let _guard = CancellationGuard {
                cancellations: cancellations.clone(),
                session_active_request: session_active_request.clone(),
                request_id: "rid-g".to_string(),
                session_id: "sid-g".to_string(),
            };
            // _guard drops at end of block.
        }
        // Give the spawned cleanup task a moment to run.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            cancellations.lock().await.is_empty(),
            "guard's Drop should have removed the cancellations entry"
        );
        assert!(
            session_active_request.lock().await.is_empty(),
            "guard's Drop should have removed the session_active_request entry"
        );
    }

    /// Step 4 follow-up: `cancel_inflight_for_session` cancels the
    /// matching request token when the session has an in-flight
    /// request, and is a silent no-op otherwise.
    #[tokio::test]
    async fn cancel_inflight_for_session_cancels_token() {
        let cancellations: Arc<Mutex<HashMap<String, CancellationToken>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let session_active_request: Arc<Mutex<HashMap<String, String>>> =
            Arc::new(Mutex::new(HashMap::new()));
        // Register a fake request for session "s1".
        let token = CancellationToken::new();
        {
            let mut map = cancellations.lock().await;
            map.insert("rid-1".to_string(), token.clone());
        }
        {
            let mut s2p = session_active_request.lock().await;
            s2p.insert("s1".to_string(), "rid-1".to_string());
        }
        assert!(!token.is_cancelled());
        cancel_inflight_for_session(&cancellations, &session_active_request, "s1").await;
        assert!(
            token.is_cancelled(),
            "matching request's token should be cancelled"
        );
    }

    /// Step 4 follow-up: `cancel_inflight_for_session` is a no-op
    /// when the session has no in-flight request.
    #[tokio::test]
    async fn cancel_inflight_for_session_missing_session_is_noop() {
        let cancellations: Arc<Mutex<HashMap<String, CancellationToken>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let session_active_request: Arc<Mutex<HashMap<String, String>>> =
            Arc::new(Mutex::new(HashMap::new()));
        // Nothing registered.
        cancel_inflight_for_session(&cancellations, &session_active_request, "s-missing")
            .await;
        // No panic, no state change. (Asserts on the maps being
        // empty would be a tautology given the setup, but the
        // function returning is the actual contract.)
        assert!(cancellations.lock().await.is_empty());
        assert!(session_active_request.lock().await.is_empty());
    }

    /// Step 4 follow-up: `cancel_inflight_for_session` is a no-op
    /// when the session has a request_id but the matching
    /// cancellation token is already gone (rare race: the
    /// request finished between the map reads).
    #[tokio::test]
    async fn cancel_inflight_for_session_token_gone_is_noop() {
        let cancellations: Arc<Mutex<HashMap<String, CancellationToken>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let session_active_request: Arc<Mutex<HashMap<String, String>>> =
            Arc::new(Mutex::new(HashMap::new()));
        // session_active_request has the entry, but the
        // cancellations map doesn't (the request already
        // finished and the CancellationGuard cleaned up).
        {
            let mut s2p = session_active_request.lock().await;
            s2p.insert("s1".to_string(), "rid-gone".to_string());
        }
        cancel_inflight_for_session(&cancellations, &session_active_request, "s1").await;
        // No panic; the function is best-effort.
    }

    /// Step 4 follow-up (REQ-16): the tool result envelope has
    /// exactly the shape `{"result": <content>, "cwd": <path>}`.
    /// This is the LLM-facing contract — the LLM gets the cwd so
    /// it can correlate tool results with the worktree state.
    /// The frontend's `extractToolResultDisplay` parses this same
    /// shape; a regression here would leak the raw JSON into the
    /// UI.
    #[test]
    fn tool_result_envelope_round_trip() {
        let path = std::path::Path::new("/data/worktrees/p1/s1");
        let env = tool_result_envelope("hello world", path);
        let parsed: serde_json::Value = serde_json::from_str(&env).expect("envelope must be JSON");
        assert_eq!(parsed["result"], "hello world");
        assert_eq!(parsed["cwd"], "/data/worktrees/p1/s1");
        // No extra top-level keys — schema discipline matters
        // because the LLM is reading this.
        assert_eq!(
            parsed.as_object().unwrap().len(),
            2,
            "envelope must have exactly 2 keys: result, cwd"
        );
    }

    /// Step 4 follow-up: empty / unicode / special-char content
    /// all round-trip cleanly through the envelope. (Sanity — the
    /// envelope is built with `serde_json::json!` which handles
    /// escaping, but a hand-written string would not.)
    #[test]
    fn tool_result_envelope_handles_special_chars() {
        let path = std::path::Path::new("/data/wt");
        // Newline, quote, and backslash in the content.
        let content = "line 1\nline 2 with \"quote\" and \\ slash";
        let env = tool_result_envelope(content, path);
        let parsed: serde_json::Value = serde_json::from_str(&env).expect("envelope must be JSON");
        assert_eq!(parsed["result"], content);
        assert_eq!(parsed["cwd"], "/data/wt");
    }

    // -----------------------------------------------------------------------
    // build_system_prompt (Step 4 follow-up Bug 3)
    // -----------------------------------------------------------------------

    /// Helper to construct a [`db::SessionRow`] with overridable
    /// worktree fields. The other fields are hard-coded test
    /// fixtures; the production schema is exercised by the DB
    /// integration tests in `db.rs`, this helper only needs a
    /// well-formed value for the prompt tests below.
    fn make_session_row(
        id: &str,
        worktree_state: db::WorktreeState,
        worktree_path: Option<&str>,
    ) -> db::SessionRow {
        db::SessionRow {
            id: id.to_string(),
            title: "Test Session".to_string(),
            created_at: "2026-06-08T00:00:00Z".to_string(),
            updated_at: "2026-06-08T00:00:00Z".to_string(),
            model: "MiniMax-M2.7".to_string(),
            project_id: "proj-1".to_string(),
            current_cwd: "/test/cwd".to_string(),
            worktree_path: worktree_path.map(str::to_string),
            worktree_state,
            last_worktree_path: None,
            model_id: None,
        }
    }

    /// Helper to construct a [`projects::ProjectRow`] with overridable
    /// `is_git_repo` flag.
    fn make_project_row(is_git_repo: bool) -> projects::ProjectRow {
        projects::ProjectRow {
            id: "proj-1".to_string(),
            name: "everlasting".to_string(),
            path: "/home/carlos/code/everlasting".to_string(),
            is_git_repo,
            git_branch: if is_git_repo {
                Some("main".to_string())
            } else {
                None
            },
            is_legacy: false,
            created_at: "2026-06-01T00:00:00Z".to_string(),
            updated_at: "2026-06-08T00:00:00Z".to_string(),
            hidden: false,
            metadata: None,
        }
    }

    /// Step 4 follow-up Bug 3: with an active worktree the prompt
    /// names the branch (`session/<id>`), the short HEAD SHA, and
    /// the working directory (the worktree path).
    #[test]
    fn build_system_prompt_active_worktree() {
        let session = make_session_row(
            "test-id",
            db::WorktreeState::Active,
            Some("/data/worktrees/p1/test-id"),
        );
        let project = make_project_row(true);
        let prompt = build_system_prompt(
            &session,
            &project,
            std::path::Path::new("/data/worktrees/p1/test-id"),
            "abc1234",
        );
        assert!(
            prompt.contains("Session ID: test-id"),
            "prompt must name the session"
        );
        assert!(
            prompt.contains("ACTIVE on branch 'session/test-id'"),
            "prompt must label state ACTIVE and include branch name"
        );
        assert!(
            prompt.contains("HEAD abc1234"),
            "prompt must include the short HEAD SHA"
        );
        assert!(
            prompt.contains("Working directory: /data/worktrees/p1/test-id"),
            "prompt must list the worktree path as the working directory"
        );
        assert!(
            prompt.contains("Available tool result envelope"),
            "prompt must describe the tool result envelope"
        );
    }

    /// Step 4 follow-up Bug 3: with no worktree the prompt labels
    /// the state as NONE and uses the project root as the
    /// working directory. Does NOT mention "session/<id>" since
    /// no branch is active.
    #[test]
    fn build_system_prompt_no_worktree() {
        let session = make_session_row("test-id", db::WorktreeState::None, None);
        let project = make_project_row(true);
        let prompt = build_system_prompt(
            &session,
            &project,
            std::path::Path::new("/home/carlos/code/everlasting"),
            "abc1234",
        );
        assert!(
            prompt.contains("NONE — running in project root"),
            "prompt must label state NONE"
        );
        assert!(
            prompt.contains("Working directory: /home/carlos/code/everlasting"),
            "working directory must be project root"
        );
        assert!(
            !prompt.contains("ACTIVE"),
            "prompt must not say ACTIVE when state is None"
        );
        assert!(
            !prompt.contains("DETACHED"),
            "prompt must not say DETACHED when state is None"
        );
    }

    /// Step 4 follow-up Bug 3: a detached worktree retains the
    /// branch name + HEAD SHA so the LLM can reason about the
    /// previous worktree, but the working directory is the project
    /// root since the worktree is unbound.
    #[test]
    fn build_system_prompt_detached_worktree() {
        let session = make_session_row("det-id", db::WorktreeState::Detached, None);
        let project = make_project_row(true);
        let prompt = build_system_prompt(
            &session,
            &project,
            std::path::Path::new("/home/carlos/code/everlasting"),
            "deadbee",
        );
        assert!(
            prompt.contains("DETACHED — was on branch 'session/det-id'"),
            "prompt must label state DETACHED and reference the old branch"
        );
        assert!(
            prompt.contains("HEAD deadbee"),
            "prompt must include the HEAD short SHA"
        );
        assert!(
            prompt.contains("currently in project root"),
            "prompt must clarify the detached fallback"
        );
        assert!(
            prompt.contains("Working directory: /home/carlos/code/everlasting"),
            "detached's working directory is the project root"
        );
    }

    /// Step 4 follow-up Bug 3: a non-git project never gets a
    /// branch / SHA — the worktree line should say "N/A — non-git
    /// project" regardless of the session's `worktree_state`
    /// column. (Non-git projects can never have a worktree, but
    /// the column is a fact-of-record; we don't trust the column
    /// over the project flag.)
    #[test]
    fn build_system_prompt_non_git_project() {
        let session = make_session_row("ng-id", db::WorktreeState::None, None);
        let project = make_project_row(false);
        let prompt = build_system_prompt(
            &session,
            &project,
            std::path::Path::new("/some/non/git/dir"),
            "not a git repo",
        );
        assert!(
            prompt.contains("Worktree: N/A — non-git project"),
            "non-git project must show the N/A worktree line"
        );
        assert!(
            !prompt.contains("session/ng-id"),
            "non-git project must not reference a session branch"
        );
    }

    // -----------------------------------------------------------------------
    // build_synthetic_tool_result_message (BUG FIX: 2013 tool_use orphan)
    //
    // When the LLM emits one or more tool_use blocks and the user
    // cancels the request before the agent loop runs the tools, the
    // DB ends up with `assistant(tool_use)` but no matching
    // `user(tool_result)` — the next `send` builds a malformed
    // history and the Anthropic API returns 2013 ("tool call result
    // does not follow tool call"). The cancel path persists a
    // synthetic `user(tool_result)` block per tool_use with
    // `is_error: true` to keep the history self-consistent. The
    // shape itself is verified by these three tests.
    // -----------------------------------------------------------------------

    /// One `tool_call` → exactly one matching `ToolResult` block,
    /// with the same `id`, `name` echoed in the content, and
    /// `is_error: true`. The role is `User` (Anthropic contract:
    /// `tool_result` blocks only appear in user-role messages).
    #[test]
    fn synthetic_tool_result_message_mirrors_tool_calls() {
        let tool_calls = vec![(
            "toolu_abc".to_string(),
            "read_file".to_string(),
            serde_json::json!({"path": "/etc/hosts"}),
        )];
        let msg = build_synthetic_tool_result_message(&tool_calls);
        assert_eq!(msg.role, Role::User, "synthetic message must be User role");
        let blocks = match &msg.content {
            MessageContent::Blocks(b) => b,
            MessageContent::Text(_) => panic!("synthetic message must be Blocks, not Text"),
        };
        assert_eq!(blocks.len(), 1, "one tool_call must produce one ToolResult block");
        match &blocks[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "toolu_abc", "tool_use_id must match");
                assert!(is_error, "synthetic tool_result must be flagged is_error=true");
                assert!(
                    content.contains("read_file"),
                    "content must name the tool that did not run: {:?}",
                    content
                );
                assert!(
                    content.contains("interrupted"),
                    "content must say the tool was interrupted: {:?}",
                    content
                );
            }
            other => panic!("expected ToolResult, got {:?}", other),
        }
    }

    /// Three `tool_call`s in one turn → three matching `ToolResult`
    /// blocks in order, all flagged is_error=true, all in the
    /// same user-role message. Order matters because the LLM
    /// correlates results to calls by `tool_use_id`, but
    /// positional ordering must also match (Anthropic convention).
    #[test]
    fn synthetic_tool_result_message_preserves_order_for_multi_call() {
        let tool_calls = vec![
            ("id_1".to_string(), "read_file".to_string(), serde_json::json!({})),
            ("id_2".to_string(), "edit_file".to_string(), serde_json::json!({})),
            ("id_3".to_string(), "shell".to_string(), serde_json::json!({})),
        ];
        let msg = build_synthetic_tool_result_message(&tool_calls);
        let blocks = match &msg.content {
            MessageContent::Blocks(b) => b,
            _ => panic!("expected Blocks"),
        };
        assert_eq!(blocks.len(), 3);
        let names: Vec<&str> = blocks
            .iter()
            .map(|b| match b {
                ContentBlock::ToolResult { content, .. } => content.as_str(),
                _ => panic!("expected ToolResult"),
            })
            .collect();
        // Each block names the tool it represents, in the same
        // order as the tool_calls vec. We don't assert the full
        // sentence (the LLM-friendly phrase is locked, see the
        // BUG FIX doc on `build_synthetic_tool_result_message`)
        // — only that the tool name is the one we passed in.
        assert!(names[0].contains("read_file"));
        assert!(names[1].contains("edit_file"));
        assert!(names[2].contains("shell"));
    }

    /// Empty `tool_calls` → empty `Blocks` array (and still a User
    /// message, so the cancel branch's `if !tool_calls.is_empty()`
    /// guard is the only thing that needs to skip the
    /// `persist_turn` call). This documents the no-op behavior so
    /// a future refactor doesn't accidentally produce a stray
    /// user message with no blocks (which would also violate
    /// Anthropic's contract).
    #[test]
    fn synthetic_tool_result_message_empty_when_no_tool_calls() {
        let msg = build_synthetic_tool_result_message(&[]);
        assert_eq!(msg.role, Role::User);
        let blocks = match &msg.content {
            MessageContent::Blocks(b) => b,
            _ => panic!("expected Blocks even when empty"),
        };
        assert!(blocks.is_empty());
    }

    /// Wire shape: the synthetic block must round-trip through
    /// serde as a `tool_result` block with `is_error: true` and
    /// the expected `tool_use_id`. This is the property the
    /// Anthropic API actually validates — if we accidentally
    /// rename the field or drop `is_error`, the 2013 regression
    /// reappears.
    #[test]
    fn synthetic_tool_result_message_serializes_to_anthropic_wire_shape() {
        let tool_calls = vec![(
            "toolu_xyz".to_string(),
            "shell".to_string(),
            serde_json::json!({"command": "ls"}),
        )];
        let msg = build_synthetic_tool_result_message(&tool_calls);
        let json = serde_json::to_string(&msg).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v.get("role").and_then(|s| s.as_str()), Some("user"));
        let content = v.get("content").and_then(|c| c.as_array()).expect(
            "synthetic message content must be an array of blocks (Anthropic tool_result contract)",
        );
        assert_eq!(content.len(), 1);
        let block = &content[0];
        assert_eq!(
            block.get("type").and_then(|s| s.as_str()),
            Some("tool_result"),
            "wire type must be exactly `tool_result`"
        );
        assert_eq!(
            block.get("tool_use_id").and_then(|s| s.as_str()),
            Some("toolu_xyz")
        );
        assert_eq!(
            block.get("is_error").and_then(|b| b.as_bool()),
            Some(true),
            "is_error: true must serialize (the is_false skip filter only drops false)"
        );
        assert!(
            block.get("content").and_then(|s| s.as_str()).unwrap().contains("shell"),
            "wire content must mention the tool name"
        );
    }
}
