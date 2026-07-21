//! `POST /api/v1/task/<command>` handlers for the task domain.
//!
//! Phase 2.2 B5 skeleton. Each handler deserializes a JSON body
//! into the same args the Tauri command takes, forwards to
//! `crate::commands::task::xxx_inner` (Q0 decision — single
//! source of truth), and wraps the result in `Json(...)`. Errors
//! flow through `AppCommandError`'s `IntoResponse` impl.

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;

use crate::error::AppCommandError;
use crate::state::AppState;
use crate::agent::workflow::TaskJson;
use crate::commands::task::{create_task_inner, archive_task_inner};

#[derive(Debug, Deserialize)]
pub struct CreateTaskRequest {
    pub project_id: String,
    pub title: String,
    pub slug: String,
    pub parent: Option<String>,
}

pub async fn create_task(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateTaskRequest>,
) -> Result<Json<TaskJson>, AppCommandError> {
    let result = create_task_inner(&state, req.project_id, req.title, req.slug, req.parent).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct ArchiveTaskRequest {
    pub project_id: String,
    pub slug: String,
    pub no_commit: bool,
}

pub async fn archive_task(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ArchiveTaskRequest>,
) -> Result<Json<TaskJson>, AppCommandError> {
    let result = archive_task_inner(&state, req.project_id, req.slug, req.no_commit).await?;
    Ok(Json(result))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/create_task", post(create_task))
        .route("/archive_task", post(archive_task))
        .with_state(state)
}
