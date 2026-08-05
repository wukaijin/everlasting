//! `POST /api/v1/memory/<command>` handlers for the memory domain.
//!
//! Phase 2.2 B5 skeleton. Each handler deserializes a JSON body
//! into the same args the Tauri command takes, forwards to
//! `crate::commands::memory::xxx_inner` (Q0 decision — single
//! source of truth), and wraps the result in `Json(...)`. Errors
//! flow through `AppCommandError`'s `IntoResponse` impl.

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;

use crate::commands::memory::{
    delete_autonomous_memory_inner, list_autonomous_memories_inner, open_memory_in_editor_inner,
    read_memory_content_inner, read_memory_layers_inner, update_autonomous_memory_inner,
    update_autonomous_memory_status_inner,
};
use crate::error::AppCommandError;
use crate::memory::types::MemoryLayerInfo;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ReadMemoryLayersRequest {
    pub project_id: String,
}

pub async fn read_memory_layers(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ReadMemoryLayersRequest>,
) -> Result<Json<Vec<MemoryLayerInfo>>, AppCommandError> {
    let result = read_memory_layers_inner(&state, req.project_id).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct ReadMemoryContentRequest {
    pub project_id: String,
    pub path: String,
}

pub async fn read_memory_content(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ReadMemoryContentRequest>,
) -> Result<Json<String>, AppCommandError> {
    let result = read_memory_content_inner(&state, req.project_id, req.path).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct OpenMemoryInEditorRequest {
    pub project_id: String,
    pub path: String,
}

pub async fn open_memory_in_editor(
    State(state): State<Arc<AppState>>,
    Json(req): Json<OpenMemoryInEditorRequest>,
) -> Result<Json<()>, AppCommandError> {
    open_memory_in_editor_inner(&state, req.project_id, req.path).await?;
    Ok(Json(()))
}

#[derive(Debug, Deserialize)]
pub struct ListAutonomousMemoriesRequest {
    pub project_id: String,
}

pub async fn list_autonomous_memories(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ListAutonomousMemoriesRequest>,
) -> Result<Json<Vec<crate::db::memories::MemoryRow>>, AppCommandError> {
    let result = list_autonomous_memories_inner(&state, req.project_id).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct DeleteAutonomousMemoryRequest {
    pub memory_id: String,
}

pub async fn delete_autonomous_memory(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DeleteAutonomousMemoryRequest>,
) -> Result<Json<u64>, AppCommandError> {
    let result = delete_autonomous_memory_inner(&state, req.memory_id).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct UpdateAutonomousMemoryStatusRequest {
    pub memory_id: String,
    pub new_status: String,
    pub demoted_reason: Option<String>,
}

pub async fn update_autonomous_memory_status(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateAutonomousMemoryStatusRequest>,
) -> Result<Json<()>, AppCommandError> {
    update_autonomous_memory_status_inner(
        &state,
        req.memory_id,
        req.new_status,
        req.demoted_reason,
    )
    .await?;
    Ok(Json(()))
}

#[derive(Debug, Deserialize)]
pub struct UpdateAutonomousMemoryRequest {
    pub memory_id: String,
    pub title: String,
    pub content: String,
}

pub async fn update_autonomous_memory(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateAutonomousMemoryRequest>,
) -> Result<Json<crate::db::memories::MemoryRow>, AppCommandError> {
    let result =
        update_autonomous_memory_inner(&state, req.memory_id, req.title, req.content).await?;
    Ok(Json(result))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/read_memory_layers", post(read_memory_layers))
        .route("/read_memory_content", post(read_memory_content))
        .route("/open_memory_in_editor", post(open_memory_in_editor))
        .route("/list_autonomous_memories", post(list_autonomous_memories))
        .route("/delete_autonomous_memory", post(delete_autonomous_memory))
        .route(
            "/update_autonomous_memory_status",
            post(update_autonomous_memory_status),
        )
        .route("/update_autonomous_memory", post(update_autonomous_memory))
        .with_state(state)
}
