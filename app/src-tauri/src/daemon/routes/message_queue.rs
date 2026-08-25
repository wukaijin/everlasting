//! F1 消息队列(2026-08-25)— R8 IPC 三件的 REST 镜像。
//!
//! 路由约定 1:1 映射 Tauri 命令(`POST /api/v1/message_queue/{name}`),
//! 业务逻辑全部在 `commands::message_queue::_inner`,此处仅 transport。

use crate::commands::message_queue::{
    list_queued_messages_inner, recall_queued_message_inner, remove_queued_message_inner,
};
use crate::error::AppCommandError;
use crate::state::AppState;
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct SessionScopedRequest {
    #[serde(default, alias = "sessionId")]
    pub session_id: String,
}

#[derive(Debug, Deserialize)]
pub struct QueuedMessageRequest {
    #[serde(default, alias = "sessionId")]
    pub session_id: String,
    pub id: String,
}

pub async fn list_queued_messages(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SessionScopedRequest>,
) -> Result<Json<Vec<crate::agent::message_queue::QueuedMessage>>, AppCommandError> {
    let msgs = list_queued_messages_inner(&state, req.session_id).await?;
    Ok(Json(msgs))
}

pub async fn remove_queued_message(
    State(state): State<Arc<AppState>>,
    Json(req): Json<QueuedMessageRequest>,
) -> Result<Json<()>, AppCommandError> {
    remove_queued_message_inner(&state, req.session_id, req.id).await?;
    Ok(Json(()))
}

pub async fn recall_queued_message(
    State(state): State<Arc<AppState>>,
    Json(req): Json<QueuedMessageRequest>,
) -> Result<Json<crate::agent::message_queue::QueuedMessage>, AppCommandError> {
    let msg = recall_queued_message_inner(&state, req.session_id, req.id).await?;
    Ok(Json(msg))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/list_queued_messages", post(list_queued_messages))
        .route("/remove_queued_message", post(remove_queued_message))
        .route("/recall_queued_message", post(recall_queued_message))
        .with_state(state)
}
