//! `POST /api/v1/panel/<command>` handlers for the panel domain.
//!
//! Phase 2.2 B5 skeleton. Each handler deserializes a JSON body
//! into the same args the Tauri command takes, forwards to
//! `crate::commands::panel::xxx_inner` (Q0 decision — single
//! source of truth), and wraps the result in `Json(...)`. Errors
//! flow through `AppCommandError`'s `IntoResponse` impl.

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;

use crate::error::AppCommandError;
use crate::state::AppState;
use crate::commands::panel::{list_subagents_inner, list_panel_items_inner, get_skill_body_inner, PanelItem, SubagentInfo};

#[derive(Debug, Deserialize)]
pub struct ListSubagentsRequest {
    pub project_id: Option<String>,
}

pub async fn list_subagents(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ListSubagentsRequest>,
) -> Result<Json<Vec<SubagentInfo>>, AppCommandError> {
    let result = list_subagents_inner(&state, req.project_id).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct ListPanelItemsRequest {
    pub project_id: Option<String>,
}

pub async fn list_panel_items(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ListPanelItemsRequest>,
) -> Result<Json<Vec<PanelItem>>, AppCommandError> {
    let result = list_panel_items_inner(&state, req.project_id).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct GetSkillBodyRequest {
    pub name: String,
    pub project_id: Option<String>,
}

pub async fn get_skill_body(
    State(state): State<Arc<AppState>>,
    Json(req): Json<GetSkillBodyRequest>,
) -> Result<Json<Option<String>>, AppCommandError> {
    let result = get_skill_body_inner(&state, req.name, req.project_id).await?;
    Ok(Json(result))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/list_subagents", post(list_subagents))
        .route("/list_panel_items", post(list_panel_items))
        .route("/get_skill_body", post(get_skill_body))
        .with_state(state)
}
