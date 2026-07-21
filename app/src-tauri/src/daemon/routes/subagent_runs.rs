//! `POST /api/v1/subagent_runs/<command>` handlers for the subagent_runs domain.
//!
//! Phase 2.2 B5 skeleton. Each handler deserializes a JSON body
//! into the same args the Tauri command takes, forwards to
//! `crate::commands::subagent_runs::xxx_inner` (Q0 decision — single
//! source of truth), and wraps the result in `Json(...)`. Errors
//! flow through `AppCommandError`'s `IntoResponse` impl.

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;

use crate::error::AppCommandError;
use crate::state::AppState;
use crate::db;
use crate::commands::subagent_runs::{list_subagent_runs_by_session_inner, get_subagent_run_inner, merge_worker_run_inner, discard_worker_run_inner, MergeWorkerResult};

#[derive(Debug, Deserialize)]
pub struct ListSubagentRunsBySessionRequest {
    pub session_id: String,
}

pub async fn list_subagent_runs_by_session(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ListSubagentRunsBySessionRequest>,
) -> Result<Json<Vec<db::subagent_runs::SubagentRunSummary>>, AppCommandError> {
    // Note: `_inner` signature has `state` as the second arg (the
    // Tauri command wrapper puts `session_id` first for ergonomic
    // invoke calls); the daemon handler normalizes to
    // `(state, session_id)`.
    let result = list_subagent_runs_by_session_inner(req.session_id, &state).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct GetSubagentRunRequest {
    pub run_id: String,
}

pub async fn get_subagent_run(
    State(state): State<Arc<AppState>>,
    Json(req): Json<GetSubagentRunRequest>,
) -> Result<Json<Option<db::subagent_runs::SubagentRunRow>>, AppCommandError> {
    let result = get_subagent_run_inner(req.run_id, &state).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct MergeWorkerRunRequest {
    pub run_id: String,
}

pub async fn merge_worker_run(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MergeWorkerRunRequest>,
) -> Result<Json<MergeWorkerResult>, AppCommandError> {
    let result = merge_worker_run_inner(req.run_id, &state).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct DiscardWorkerRunRequest {
    pub run_id: String,
}

pub async fn discard_worker_run(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DiscardWorkerRunRequest>,
) -> Result<Json<String>, AppCommandError> {
    let result = discard_worker_run_inner(req.run_id, &state).await?;
    Ok(Json(result))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/list_subagent_runs_by_session", post(list_subagent_runs_by_session))
        .route("/get_subagent_run", post(get_subagent_run))
        .route("/merge_worker_run", post(merge_worker_run))
        .route("/discard_worker_run", post(discard_worker_run))
        .with_state(state)
}
