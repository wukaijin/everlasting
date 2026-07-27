//! `POST /api/v1/command_palette/<command>` handlers for the command_palette domain.
//!
//! Phase 2.2 B5 skeleton. Each handler deserializes a JSON body
//! into the same args the Tauri command takes, forwards to
//! `crate::commands::command_palette::xxx_inner` (Q0 decision — single
//! source of truth), and wraps the result in `Json(...)`. Errors
//! flow through `AppCommandError`'s `IntoResponse` impl.

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;

use crate::commands::command_palette::{get_command_body_inner, list_commands_inner};
use crate::error::AppCommandError;
use crate::resource_loader::CommandInfo;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ListCommandsRequest {
    pub project_id: Option<String>,
}

pub async fn list_commands(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ListCommandsRequest>,
) -> Result<Json<Vec<CommandInfo>>, AppCommandError> {
    let result = list_commands_inner(&state, req.project_id).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct GetCommandBodyRequest {
    pub name: String,
    pub project_id: Option<String>,
}

pub async fn get_command_body(
    State(state): State<Arc<AppState>>,
    Json(req): Json<GetCommandBodyRequest>,
) -> Result<Json<Option<String>>, AppCommandError> {
    let result = get_command_body_inner(&state, req.name, req.project_id).await?;
    Ok(Json(result))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/list_commands", post(list_commands))
        .route("/get_command_body", post(get_command_body))
        .with_state(state)
}
