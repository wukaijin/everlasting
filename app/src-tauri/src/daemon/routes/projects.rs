//! `POST /api/v1/projects/<command>` handlers for the projects domain.
//!
//! Phase 2.2 B5 skeleton. Each handler deserializes a JSON body
//! into the same args the Tauri command takes, forwards to
//! `crate::commands::projects::xxx_inner` (Q0 decision — single
//! source of truth), and wraps the result in `Json(...)`. Errors
//! flow through `AppCommandError`'s `IntoResponse` impl.

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;

use crate::commands::projects::{
    create_project_inner, hide_project_inner, list_hidden_projects_inner, list_projects_inner,
    unhide_project_inner, update_project_name_inner, update_project_path_inner, ListProjectsFilter,
};
use crate::error::AppCommandError;
use crate::projects;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ListProjectsRequest {
    pub filter: Option<ListProjectsFilter>,
}

pub async fn list_projects(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ListProjectsRequest>,
) -> Result<Json<Vec<projects::ProjectRow>>, AppCommandError> {
    let result = list_projects_inner(&state, req.filter).await?;
    Ok(Json(result))
}

pub async fn list_hidden_projects(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<projects::ProjectRow>>, AppCommandError> {
    let result = list_hidden_projects_inner(&state).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct CreateProjectRequest {
    pub path: String,
}

pub async fn create_project(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateProjectRequest>,
) -> Result<Json<projects::ProjectRow>, AppCommandError> {
    let result = create_project_inner(&state, req.path).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct UpdateProjectPathRequest {
    pub id: String,
    pub new_path: String,
}

pub async fn update_project_path(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateProjectPathRequest>,
) -> Result<Json<projects::ProjectRow>, AppCommandError> {
    let result = update_project_path_inner(&state, req.id, req.new_path).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct UpdateProjectNameRequest {
    pub id: String,
    pub new_name: String,
}

pub async fn update_project_name(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateProjectNameRequest>,
) -> Result<Json<projects::ProjectRow>, AppCommandError> {
    let result = update_project_name_inner(&state, req.id, req.new_name).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct HideProjectRequest {
    pub id: String,
}

pub async fn hide_project(
    State(state): State<Arc<AppState>>,
    Json(req): Json<HideProjectRequest>,
) -> Result<Json<()>, AppCommandError> {
    hide_project_inner(&state, req.id).await?;
    Ok(Json(()))
}

#[derive(Debug, Deserialize)]
pub struct UnhideProjectRequest {
    pub id: String,
}

pub async fn unhide_project(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UnhideProjectRequest>,
) -> Result<Json<()>, AppCommandError> {
    unhide_project_inner(&state, req.id).await?;
    Ok(Json(()))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/list_projects", post(list_projects))
        .route("/list_hidden_projects", post(list_hidden_projects))
        .route("/create_project", post(create_project))
        .route("/update_project_path", post(update_project_path))
        .route("/update_project_name", post(update_project_name))
        .route("/hide_project", post(hide_project))
        .route("/unhide_project", post(unhide_project))
        .with_state(state)
}
