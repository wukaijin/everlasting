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

use crate::commands::model_metadata::{lookup_model_metadata_inner, probe_provider_models_inner};
use crate::commands::providers::{
    add_model_inner, add_provider_inner, delete_model_inner, delete_provider_inner,
    get_default_model_inner, list_models_inner, list_providers_inner, set_default_model_inner,
    set_model_disabled_inner, set_provider_disabled_inner, test_model_inner, update_model_inner,
    update_provider_inner, update_session_model_id_inner,
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
    pub supports_images: bool,
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
        req.supports_images,
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
    pub supports_images: bool,
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
        req.supports_images,
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

/// 2026-09-07 (provider-model-disable): 模型级禁用开关。Body 字段
/// snake_case(transport 顶层 key 已做 camelCase→snake_case 转换)。
#[derive(Debug, Deserialize)]
pub struct SetModelDisabledRequest {
    pub id: String,
    pub disabled: bool,
}

pub async fn set_model_disabled(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetModelDisabledRequest>,
) -> Result<Json<Option<db::ModelRow>>, AppCommandError> {
    let result = set_model_disabled_inner(&state, req.id, req.disabled).await?;
    Ok(Json(result))
}

/// 2026-09-07 (provider-model-disable): provider 级禁用开关。
#[derive(Debug, Deserialize)]
pub struct SetProviderDisabledRequest {
    pub id: String,
    pub disabled: bool,
}

pub async fn set_provider_disabled(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetProviderDisabledRequest>,
) -> Result<Json<Option<db::ProviderRow>>, AppCommandError> {
    let result = set_provider_disabled_inner(&state, req.id, req.disabled).await?;
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
    set_default_model_inner(&state, req.model_id).await?;
    Ok(Json(()))
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
    update_session_model_id_inner(&state, req.session_id, req.model_id).await?;
    Ok(Json(()))
}

#[derive(Debug, Deserialize)]
pub struct TestModelRequest {
    pub model_id: String,
}

pub async fn test_model(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TestModelRequest>,
) -> Result<Json<serde_json::Value>, AppCommandError> {
    let result = test_model_inner(&state.db, req.model_id).await?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// 2026-10-19 (task 10-19-merge-provider-model-settings):模型探测 +
// 在线能力匹配。wire 与 `test_model` 同族 —— 业务失败走 `{success:false,
// error}` JSON 而非 HTTP 错误,前端统一内联渲染。
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ProbeProviderModelsRequest {
    pub provider_id: String,
}

/// 拉一次供应商的模型列表端点(GET /models / /v1/models),附带在线
/// 能力富化。见 `commands::model_metadata`。
pub async fn probe_provider_models(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ProbeProviderModelsRequest>,
) -> Result<Json<serde_json::Value>, AppCommandError> {
    let result = probe_provider_models_inner(&state, req.provider_id).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct LookupModelMetadataRequest {
    pub model_name: String,
    #[serde(default)]
    pub provider_id: Option<String>,
}

/// 按模型名查 models.dev 能力目录,返回候选列表(首个=最佳匹配)。
pub async fn lookup_model_metadata(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LookupModelMetadataRequest>,
) -> Result<Json<serde_json::Value>, AppCommandError> {
    let result = lookup_model_metadata_inner(&state, req.model_name, req.provider_id).await?;
    Ok(Json(result))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/list_providers", post(list_providers))
        .route("/add_provider", post(add_provider))
        .route("/update_provider", post(update_provider))
        .route("/delete_provider", post(delete_provider))
        .route("/set_provider_disabled", post(set_provider_disabled))
        .route("/list_models", post(list_models))
        .route("/add_model", post(add_model))
        .route("/update_model", post(update_model))
        .route("/delete_model", post(delete_model))
        .route("/set_model_disabled", post(set_model_disabled))
        .route("/get_default_model", post(get_default_model))
        .route("/set_default_model", post(set_default_model))
        .route("/update_session_model_id", post(update_session_model_id))
        .route("/test_model", post(test_model))
        .route("/probe_provider_models", post(probe_provider_models))
        .route("/lookup_model_metadata", post(lookup_model_metadata))
        .with_state(state)
}
