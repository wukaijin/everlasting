//! `POST /api/v1/subagents/<command>` handlers for the subagents domain.
//!
//! Phase 2.2 B5 skeleton. Each handler deserializes a JSON body
//! into the same args the Tauri command takes, forwards to
//! `crate::commands::subagents::xxx_inner` (Q0 decision — single
//! source of truth), and wraps the result in `Json(...)`. Errors
//! flow through `AppCommandError`'s `IntoResponse` impl.

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;

use crate::error::AppCommandError;
use crate::state::AppState;
use crate::commands::subagents::{list_subagents_with_model_inner, set_subagent_model_inner, SubagentWithModelRow};

#[derive(Debug, Deserialize)]
pub struct ListSubagentsWithModelRequest {
    pub project_path: String,
}

pub async fn list_subagents_with_model(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ListSubagentsWithModelRequest>,
) -> Result<Json<Vec<SubagentWithModelRow>>, AppCommandError> {
    let result = list_subagents_with_model_inner(req.project_path, &state).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct SetSubagentModelRequest {
    pub name: String,
    pub source: String,
    pub project_path: String,
    pub model_id: Option<String>,
}

pub async fn set_subagent_model(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetSubagentModelRequest>,
) -> Result<Json<SubagentWithModelRow>, AppCommandError> {
    let result = set_subagent_model_inner(req.name, req.source, req.project_path, req.model_id, &state).await?;
    Ok(Json(result))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/list_subagents_with_model", post(list_subagents_with_model))
        .route("/set_subagent_model", post(set_subagent_model))
        .with_state(state)
}
