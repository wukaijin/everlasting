//! `POST /api/v1/question/<command>` handlers for the question domain.
//!
//! Phase 2.2 B5 skeleton. Each handler deserializes a JSON body
//! into the same args the Tauri command takes, forwards to
//! `crate::commands::question::xxx_inner` (Q0 decision — single
//! source of truth), and wraps the result in `Json(...)`. Errors
//! flow through `AppCommandError`'s `IntoResponse` impl.

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;

use crate::error::AppCommandError;
use crate::state::AppState;
use crate::db;
use crate::agent::question_store::PendingInteractionEntry;
use crate::agent::question_store::QuestionAnswer;
use crate::agent::question_store::ToolQuestionPayload;
use crate::commands::question::{resolve_tool_question_inner, resolve_mode_change_inner, get_pending_interaction_inner, get_pending_question_inner, resolve_task_state_transition_inner};

#[derive(Debug, Deserialize)]
pub struct ResolveToolQuestionRequest {
    pub session_id: String,
    pub tool_use_id: String,
    pub answer: Option<Vec<QuestionAnswer>>,
    pub cancelled: Option<bool>,
}

pub async fn resolve_tool_question(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ResolveToolQuestionRequest>,
) -> Result<Json<()>, AppCommandError> {
    let result = resolve_tool_question_inner(&state, req.session_id, req.tool_use_id, req.answer, req.cancelled).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct ResolveModeChangeRequest {
    pub session_id: String,
    pub tool_use_id: String,
    pub target_mode: String,
    pub allow: bool,
}

pub async fn resolve_mode_change(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ResolveModeChangeRequest>,
) -> Result<Json<db::SessionRow>, AppCommandError> {
    let result = resolve_mode_change_inner(&state, req.session_id, req.tool_use_id, req.target_mode, req.allow).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct GetPendingInteractionRequest {
    pub session_id: String,
}

pub async fn get_pending_interaction(
    State(state): State<Arc<AppState>>,
    Json(req): Json<GetPendingInteractionRequest>,
) -> Result<Json<Option<PendingInteractionEntry>>, AppCommandError> {
    let result = get_pending_interaction_inner(&state, req.session_id).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct GetPendingQuestionRequest {
    pub session_id: String,
}

pub async fn get_pending_question(
    State(state): State<Arc<AppState>>,
    Json(req): Json<GetPendingQuestionRequest>,
) -> Result<Json<Option<ToolQuestionPayload>>, AppCommandError> {
    let result = get_pending_question_inner(&state, req.session_id).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct ResolveTaskStateTransitionRequest {
    pub session_id: String,
    pub tool_use_id: String,
    pub target_state: String,
    pub slug: String,
    pub allow: bool,
}

pub async fn resolve_task_state_transition(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ResolveTaskStateTransitionRequest>,
) -> Result<Json<db::SessionRow>, AppCommandError> {
    let result = resolve_task_state_transition_inner(&state, req.session_id, req.tool_use_id, req.target_state, req.slug, req.allow).await?;
    Ok(Json(result))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/resolve_tool_question", post(resolve_tool_question))
        .route("/resolve_mode_change", post(resolve_mode_change))
        .route("/get_pending_interaction", post(get_pending_interaction))
        .route("/get_pending_question", post(get_pending_question))
        .route("/resolve_task_state_transition", post(resolve_task_state_transition))
        .with_state(state)
}
