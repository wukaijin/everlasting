//! `POST /api/v1/worktree/<command>` handlers for the worktree domain.
//!
//! Phase 2.2 B5 skeleton. Each handler deserializes a JSON body
//! into the same args the Tauri command takes, forwards to
//! `crate::commands::worktree::xxx_inner` (Q0 decision — single
//! source of truth), and wraps the result in `Json(...)`. Errors
//! flow through `AppCommandError`'s `IntoResponse` impl.

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;

use crate::commands::worktree::{
    attach_worktree_inner, delete_worktree_inner, detach_worktree_inner,
    publish_session_to_main_inner,
};
use crate::db;
use crate::error::AppCommandError;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct PublishSessionToMainRequest {
    pub session_id: String,
}

pub async fn publish_session_to_main(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PublishSessionToMainRequest>,
) -> Result<Json<String>, AppCommandError> {
    let result = publish_session_to_main_inner(&state, req.session_id).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct AttachWorktreeRequest {
    pub session_id: String,
}

pub async fn attach_worktree(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AttachWorktreeRequest>,
) -> Result<Json<db::SessionRow>, AppCommandError> {
    let result = attach_worktree_inner(&state, req.session_id).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct DetachWorktreeRequest {
    pub session_id: String,
}

pub async fn detach_worktree(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DetachWorktreeRequest>,
) -> Result<Json<db::SessionRow>, AppCommandError> {
    let result = detach_worktree_inner(&state, req.session_id).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct DeleteWorktreeRequest {
    pub session_id: String,
}

pub async fn delete_worktree(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DeleteWorktreeRequest>,
) -> Result<Json<db::SessionRow>, AppCommandError> {
    let result = delete_worktree_inner(&state, req.session_id).await?;
    Ok(Json(result))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/publish_session_to_main", post(publish_session_to_main))
        .route("/attach_worktree", post(attach_worktree))
        .route("/detach_worktree", post(detach_worktree))
        .route("/delete_worktree", post(delete_worktree))
        .with_state(state)
}
