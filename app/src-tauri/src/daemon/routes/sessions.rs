//! `POST /api/v1/sessions/<command>` handlers for the sessions domain.
//!
//! Phase 2.2 B5 skeleton. Each handler deserializes a JSON body
//! into the same args the Tauri command takes, forwards to
//! `crate::commands::sessions::xxx_inner` (Q0 decision — single
//! source of truth), and wraps the result in `Json(...)`. Errors
//! flow through `AppCommandError`'s `IntoResponse` impl.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::agent::question_store::PendingInteractionEntry;
use crate::commands::question::get_pending_interaction_inner;
use crate::commands::sessions::{
    clear_session_messages_inner, create_session_inner, delete_session_inner, diff_worktree_inner,
    update_session_metadata_inner,
    edit_user_message_inner, list_sessions_inner, load_session_inner, record_tool_duration_inner,
    rename_session_inner, set_session_color_inner, set_session_plugin_name_inner,
    set_session_workflow_enabled_inner, update_message_latency_inner,
};
use crate::db;
use crate::error::AppCommandError;
use crate::git;
use crate::llm::types::MessageContent;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ListSessionsRequest {
    pub project_id: String,
}

pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ListSessionsRequest>,
) -> Result<Json<Vec<db::SessionSummary>>, AppCommandError> {
    let result = list_sessions_inner(&state, req.project_id).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub project_id: String,
    pub initial_cwd: String,
    pub model: Option<String>,
    // Group chat (07-29-group-chat, Phase 4 Step 3 TODO-E2):
    // session type discriminator + per-session JSON metadata.
    // Serpent-case to match the `CreateSession` Tauri command
    // + the `serde_json::Value` shape. `None` for classic chat.
    pub session_type: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

pub async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<db::SessionRow>, AppCommandError> {
    let result = create_session_inner(
        &state,
        req.project_id,
        req.initial_cwd,
        req.model,
        req.session_type,
        req.metadata,
    )
    .await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct LoadSessionRequest {
    pub session_id: String,
}

pub async fn load_session(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LoadSessionRequest>,
) -> Result<Json<Option<db::LoadedSession>>, AppCommandError> {
    let result = load_session_inner(&state, req.session_id).await?;
    Ok(Json(result))
}

/// `GET /api/v1/sessions/{id}/snapshot` — resync 快照(P2.3 C3)。
///
/// 前端 `httpTransport` 收到 `stream-resync` sentinel 后调此端点:
/// 一次拿回完整 session(`load_session_inner` = session 元数据 +
/// 全部 messages)+ 当前 pending interaction(permission /
/// question / mode_change / task_state_transition 卡片),用快照
/// 替换 store 后重画 UI(design §2.4)。复用既有 `_inner`,无新
/// 业务逻辑——snapshot 是 `load_session` + `get_pending_interaction`
/// 的合并 GET。
#[derive(Debug, Serialize)]
pub struct SessionSnapshot {
    pub session: Option<db::LoadedSession>,
    pub pending_interaction: Option<PendingInteractionEntry>,
}

pub async fn snapshot(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<SessionSnapshot>, AppCommandError> {
    let session = load_session_inner(&state, id.clone()).await?;
    let pending_interaction = get_pending_interaction_inner(&state, id).await?;
    Ok(Json(SessionSnapshot {
        session,
        pending_interaction,
    }))
}

#[derive(Debug, Deserialize)]
pub struct DiffWorktreeRequest {
    pub session_id: String,
}

pub async fn diff_worktree(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DiffWorktreeRequest>,
) -> Result<Json<git::diff::DiffResult>, AppCommandError> {
    let result = diff_worktree_inner(&state, req.session_id).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct DeleteSessionRequest {
    pub session_id: String,
}

pub async fn delete_session(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DeleteSessionRequest>,
) -> Result<Json<()>, AppCommandError> {
    let result = delete_session_inner(&state, req.session_id).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct ClearSessionMessagesRequest {
    pub session_id: String,
}

pub async fn clear_session_messages(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ClearSessionMessagesRequest>,
) -> Result<Json<()>, AppCommandError> {
    let result = clear_session_messages_inner(&state, req.session_id).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct RenameSessionRequest {
    pub session_id: String,
    pub new_title: String,
}

pub async fn rename_session(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RenameSessionRequest>,
) -> Result<Json<()>, AppCommandError> {
    let result = rename_session_inner(&state, req.session_id, req.new_title).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct SetSessionColorRequest {
    pub session_id: String,
    pub color_tag: Option<i32>,
}

pub async fn set_session_color(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetSessionColorRequest>,
) -> Result<Json<()>, AppCommandError> {
    let result = set_session_color_inner(&state, req.session_id, req.color_tag).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct SetSessionWorkflowEnabledRequest {
    pub session_id: String,
    pub enabled: bool,
}

pub async fn set_session_workflow_enabled(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetSessionWorkflowEnabledRequest>,
) -> Result<Json<()>, AppCommandError> {
    let result = set_session_workflow_enabled_inner(&state, req.session_id, req.enabled).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct SetSessionPluginNameRequest {
    pub session_id: String,
    pub name: String,
}

pub async fn set_session_plugin_name(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetSessionPluginNameRequest>,
) -> Result<Json<()>, AppCommandError> {
    let result = set_session_plugin_name_inner(&state, req.session_id, req.name).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct UpdateMessageLatencyRequest {
    pub session_id: String,
    pub seq: i64,
    pub ttfb_ms: Option<i64>,
    pub gen_ms: Option<i64>,
    pub total_ms: Option<i64>,
    pub thinking_ms: Option<i64>,
}

pub async fn update_message_latency(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateMessageLatencyRequest>,
) -> Result<Json<bool>, AppCommandError> {
    let result = update_message_latency_inner(
        &state,
        req.session_id,
        req.seq,
        req.ttfb_ms,
        req.gen_ms,
        req.total_ms,
        req.thinking_ms,
    )
    .await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct RecordToolDurationRequest {
    pub session_id: String,
    pub tool_use_id: String,
    pub duration_ms: i64,
}

pub async fn record_tool_duration(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RecordToolDurationRequest>,
) -> Result<Json<bool>, AppCommandError> {
    let result =
        record_tool_duration_inner(&state, req.session_id, req.tool_use_id, req.duration_ms)
            .await?;
    Ok(Json(result))
}

// Group chat (07-29-group-chat, Phase 4 Step 3 TODO-E4):
// REST mirror of the `update_session_metadata` Tauri command.
// The IPC body carries the per-session JSON metadata blob
// (typically `{participants: [...]}` for group-chat sessions);
// backend writes it verbatim to `sessions.metadata` (TEXT
// column, JSON string).
#[derive(Debug, Deserialize)]
pub struct UpdateSessionMetadataRequest {
    pub session_id: String,
    pub metadata: serde_json::Value,
}

pub async fn update_session_metadata(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateSessionMetadataRequest>,
) -> Result<Json<()>, AppCommandError> {
    update_session_metadata_inner(&state, req.session_id, req.metadata).await?;
    Ok(Json(()))
}

#[derive(Debug, Deserialize)]
pub struct EditUserMessageRequest {
    pub session_id: String,
    pub message_seq: i64,
    pub new_content: MessageContent,
}

pub async fn edit_user_message(
    State(state): State<Arc<AppState>>,
    Json(req): Json<EditUserMessageRequest>,
) -> Result<Json<()>, AppCommandError> {
    let result =
        edit_user_message_inner(&state, req.session_id, req.message_seq, req.new_content).await?;
    Ok(Json(result))
}

/// `POST /api/v1/sessions/list_workflow_plugins` — discover
/// workflow plugins under `<project>/.everlasting/workflow/`.
/// Phase 2.2 follow-up (2026-07-21): this handler was missing
/// from the initial route table even though the Tauri command
/// existed. No `AppState` dependency — the lookup is a pure
/// filesystem read via `crate::agent::workflow::list_plugins`.
#[derive(Debug, Deserialize)]
pub struct ListWorkflowPluginsRequest {
    pub project_path: String,
}

pub async fn list_workflow_plugins(
    Json(req): Json<ListWorkflowPluginsRequest>,
) -> Result<Json<Vec<String>>, AppCommandError> {
    Ok(Json(crate::agent::workflow::list_plugins(
        &req.project_path,
    )))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/list_sessions", post(list_sessions))
        .route("/create_session", post(create_session))
        .route("/load_session", post(load_session))
        .route("/update_session_metadata", post(update_session_metadata))
        .route("/:id/snapshot", get(snapshot))
        .route("/diff_worktree", post(diff_worktree))
        .route("/delete_session", post(delete_session))
        .route("/clear_session_messages", post(clear_session_messages))
        .route("/rename_session", post(rename_session))
        .route("/set_session_color", post(set_session_color))
        .route(
            "/set_session_workflow_enabled",
            post(set_session_workflow_enabled),
        )
        .route("/set_session_plugin_name", post(set_session_plugin_name))
        .route("/list_workflow_plugins", post(list_workflow_plugins))
        .route("/update_message_latency", post(update_message_latency))
        .route("/record_tool_duration", post(record_tool_duration))
        .route("/edit_user_message", post(edit_user_message))
        .with_state(state)
}
