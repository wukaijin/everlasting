//! `POST /api/v1/background_shells/<command>` handlers for the
//! background_shells domain (2026-09-02, task `09-02-chat-task-panel`).
//!
//! Same shape as `routes::subagent_runs`: each handler deserializes
//! the JSON body (snake_case — no rename, the httpTransport already
//! converted the frontend's camelCase top-level args) into the same
//! scalar args the Tauri command takes, forwards to
//! `crate::commands::background_shells::xxx_inner`, and wraps the
//! result in `Json(...)`. Errors flow through `AppCommandError`'s
//! `IntoResponse` impl.

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;

use crate::background_shell::BackgroundShellSummary;
use crate::commands::background_shells::{
    kill_background_shell_inner, list_background_shells_inner,
};
use crate::error::AppCommandError;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ListBackgroundShellsRequest {
    pub session_id: String,
}

pub async fn list_background_shells(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ListBackgroundShellsRequest>,
) -> Result<Json<Vec<BackgroundShellSummary>>, AppCommandError> {
    let result = list_background_shells_inner(req.session_id, &state).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct KillBackgroundShellRequest {
    pub session_id: String,
    pub shell_session_id: String,
}

pub async fn kill_background_shell(
    State(state): State<Arc<AppState>>,
    Json(req): Json<KillBackgroundShellRequest>,
) -> Result<Json<()>, AppCommandError> {
    kill_background_shell_inner(req.session_id, req.shell_session_id, &state).await?;
    Ok(Json(()))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/list_background_shells", post(list_background_shells))
        .route("/kill_background_shell", post(kill_background_shell))
        .with_state(state)
}
