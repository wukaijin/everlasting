//! `POST /api/v1/checkpoint/<command>` handlers for the checkpoint
//! domain (N2 PR2, task `09-20-n2-checkpoint-revert`).
//!
//! Each handler deserializes a JSON body into the same args the Tauri
//! command takes, forwards to `crate::commands::checkpoint::xxx_inner`
//! (Q0 decision — single source of truth), and wraps the result in
//! `Json(...)`. Errors flow through `AppCommandError`'s `IntoResponse`
//! impl — including the typed `CheckpointsUnavailable` /
//! `CheckpointBroken` kinds (the frontend's entry-visibility key).

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;

use crate::commands::checkpoint::{
    get_turn_checkpoint_diff_inner, list_turn_checkpoints_inner,
    revert_to_checkpoint_execute_inner, revert_to_checkpoint_preview_inner, RevertPreview,
    RevertResult, TurnCheckpointSummary,
};
use crate::error::AppCommandError;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ListTurnCheckpointsRequest {
    pub session_id: String,
}

pub async fn list_turn_checkpoints(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ListTurnCheckpointsRequest>,
) -> Result<Json<Vec<TurnCheckpointSummary>>, AppCommandError> {
    let result = list_turn_checkpoints_inner(&state, req.session_id).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct GetTurnCheckpointDiffRequest {
    pub session_id: String,
    pub seq: i64,
}

pub async fn get_turn_checkpoint_diff(
    State(state): State<Arc<AppState>>,
    Json(req): Json<GetTurnCheckpointDiffRequest>,
) -> Result<Json<crate::git::diff::DiffResult>, AppCommandError> {
    let result = get_turn_checkpoint_diff_inner(&state, req.session_id, req.seq).await?;
    Ok(Json(result))
}

// N2 PR3: revert 两步(dangerous;execute 的 preview_token 由前端
// 从 preview 响应原样带回)。
#[derive(Debug, Deserialize)]
pub struct RevertPreviewRequest {
    pub session_id: String,
    pub target_seq: i64,
}

pub async fn revert_to_checkpoint_preview(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RevertPreviewRequest>,
) -> Result<Json<RevertPreview>, AppCommandError> {
    let result = revert_to_checkpoint_preview_inner(&state, req.session_id, req.target_seq).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct RevertExecuteRequest {
    pub session_id: String,
    pub target_seq: i64,
    pub preview_token: String,
}

pub async fn revert_to_checkpoint_execute(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RevertExecuteRequest>,
) -> Result<Json<RevertResult>, AppCommandError> {
    let result = revert_to_checkpoint_execute_inner(
        &state,
        req.session_id,
        req.target_seq,
        req.preview_token,
    )
    .await?;
    Ok(Json(result))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/list_turn_checkpoints", post(list_turn_checkpoints))
        .route("/get_turn_checkpoint_diff", post(get_turn_checkpoint_diff))
        .route(
            "/revert_to_checkpoint_preview",
            post(revert_to_checkpoint_preview),
        )
        .route(
            "/revert_to_checkpoint_execute",
            post(revert_to_checkpoint_execute),
        )
        .with_state(state)
}
