//! `POST /api/v1/providers/<command>` handlers for the providers domain.
//!
//! Phase 2.2 B5 skeleton. Each handler deserializes a JSON body
//! into the same args the Tauri command takes, forwards to
//! `crate::commands::providers::xxx_inner` (Q0 decision — single
//! source of truth), and wraps the result in `Json(...)`. Errors
//! flow through `AppCommandError`'s `IntoResponse` impl.

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;

use crate::commands::providers::{
    add_model_inner, add_provider_inner, delete_model_inner, delete_provider_inner,
    get_default_model_inner, list_models_inner, list_providers_inner, set_default_model_inner,
    test_model_inner, update_model_inner, update_provider_inner, update_session_model_id_inner,
};
use crate::db;
use crate::error::AppCommandError;
use crate::state::AppState;

pub async fn list_providers(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<db::ProviderRow>>, AppCommandError> {
    let result = list_providers_inner(&state).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct AddProviderRequest {
    pub protocol: String,
    pub display_name: String,
    pub base_url: String,
    pub api_key: String,
}

pub async fn add_provider(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddProviderRequest>,
) -> Result<Json<db::ProviderRow>, AppCommandError> {
    let result = add_provider_inner(
        &state,
        req.protocol,
        req.display_name,
        req.base_url,
        req.api_key,
    )
    .await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct DeleteProviderRequest {
    pub id: String,
}

pub async fn delete_provider(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DeleteProviderRequest>,
) -> Result<Json<bool>, AppCommandError> {
    let result = delete_provider_inner(&state, req.id).await?;
    Ok(Json(result))
}

/// `POST /api/v1/providers/update_provider` — edit an existing
/// provider row. Phase 2.2 follow-up (2026-07-21): this handler
/// was missing from the initial route table even though the
/// `update_provider` Tauri command + `update_provider_inner`
/// existed. Body matches the Tauri command's args; `api_key` is
/// `Option<String>` per RULE-D-001 (`None` = keep existing key,
/// `Some(v)` = encrypted override).
#[derive(Debug, Deserialize)]
pub struct UpdateProviderRequest {
    pub id: String,
    pub protocol: String,
    pub display_name: String,
    pub base_url: String,
    pub api_key: Option<String>,
}

pub async fn update_provider(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateProviderRequest>,
) -> Result<Json<Option<db::ProviderRow>>, AppCommandError> {
    let result = update_provider_inner(
        &state,
        req.id,
        req.protocol,
        req.display_name,
        req.base_url,
        req.api_key,
    )
    .await?;
    Ok(Json(result))
}

pub async fn list_models(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<db::ModelWithProvider>>, AppCommandError> {
    let result = list_models_inner(&state).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct AddModelRequest {
    pub provider_id: String,
    pub model_name: String,
    pub display_name: String,
    pub max_tokens: Option<u32>,
    pub thinking_effort: Option<String>,
    pub supports_thinking: bool,
    pub context_window: u32,
}

pub async fn add_model(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddModelRequest>,
) -> Result<Json<db::ModelRow>, AppCommandError> {
    let result = add_model_inner(
        &state,
        req.provider_id,
        req.model_name,
        req.display_name,
        req.max_tokens,
        req.thinking_effort,
        req.supports_thinking,
        req.context_window,
    )
    .await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct UpdateModelRequest {
    pub id: String,
    pub provider_id: String,
    pub model_name: String,
    pub display_name: String,
    pub max_tokens: Option<u32>,
    pub thinking_effort: Option<String>,
    pub supports_thinking: bool,
    pub context_window: u32,
}

pub async fn update_model(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateModelRequest>,
) -> Result<Json<Option<db::ModelRow>>, AppCommandError> {
    let result = update_model_inner(
        &state,
        req.id,
        req.provider_id,
        req.model_name,
        req.display_name,
        req.max_tokens,
        req.thinking_effort,
        req.supports_thinking,
        req.context_window,
    )
    .await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct DeleteModelRequest {
    pub id: String,
}

pub async fn delete_model(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DeleteModelRequest>,
) -> Result<Json<bool>, AppCommandError> {
    let result = delete_model_inner(&state, req.id).await?;
    Ok(Json(result))
}

pub async fn get_default_model(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Option<db::ModelWithProvider>>, AppCommandError> {
    let result = get_default_model_inner(&state).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct SetDefaultModelRequest {
    pub model_id: String,
}

pub async fn set_default_model(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetDefaultModelRequest>,
) -> Result<Json<()>, AppCommandError> {
    let result = set_default_model_inner(&state, req.model_id).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct UpdateSessionModelIdRequest {
    pub session_id: String,
    pub model_id: String,
}

pub async fn update_session_model_id(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateSessionModelIdRequest>,
) -> Result<Json<()>, AppCommandError> {
    let result = update_session_model_id_inner(&state, req.session_id, req.model_id).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct TestModelRequest {
    pub model_id: String,
}

pub async fn test_model(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TestModelRequest>,
) -> Result<Json<serde_json::Value>, AppCommandError> {
    let result = test_model_inner(&state, req.model_id).await?;
    Ok(Json(result))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/list_providers", post(list_providers))
        .route("/add_provider", post(add_provider))
        .route("/update_provider", post(update_provider))
        .route("/delete_provider", post(delete_provider))
        .route("/list_models", post(list_models))
        .route("/add_model", post(add_model))
        .route("/update_model", post(update_model))
        .route("/delete_model", post(delete_model))
        .route("/get_default_model", post(get_default_model))
        .route("/set_default_model", post(set_default_model))
        .route("/update_session_model_id", post(update_session_model_id))
        .route("/test_model", post(test_model))
        .with_state(state)
}
