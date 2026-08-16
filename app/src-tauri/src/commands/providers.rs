//! Provider / Model / default-model Tauri commands (PR1 of
//! multi-model), plus `test_provider` (deprecated) and
//! `test_model` (PR5 follow-up).
//!
//! Wire shape (camelCase on the JS side per Tauri's default):
//!   - ProviderRow:    { id, protocol, displayName, baseUrl, apiKey, ... }
//!   - ModelRow:       { id, providerId, modelName, displayName, maxTokens,
//!                       thinkingEffort, supportsThinking, contextWindow, ... }
//!   - ModelWithProvider: ModelRow + { providerDisplayName, providerProtocol }
//!
//! Args follow the same convention: snake_case in Rust, camelCase
//! from JS.

use std::sync::Arc;

use tauri::State;

use crate::db;
use crate::error::AppCommandError;
use crate::state::AppState;

pub async fn list_providers_inner(
    state: &Arc<AppState>,
) -> Result<Vec<db::ProviderRow>, AppCommandError> {
    db::list_providers(&state.db)
        .await
        .map_err(|e| anyhow::anyhow!("list_providers failed: {}", e).into())
}

#[tauri::command]
pub async fn list_providers(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<db::ProviderRow>, AppCommandError> {
    list_providers_inner(&state).await
}

pub async fn add_provider_inner(
    state: &Arc<AppState>,
    protocol: String,
    display_name: String,
    base_url: String,
    api_key: String,
) -> Result<db::ProviderRow, AppCommandError> {
    let row = db::create_provider(&state.db, &protocol, &display_name, &base_url, &api_key)
        .await
        .map_err(|e| anyhow::anyhow!("add_provider failed: {}", e))?;
    state.rebuild_catalog().await;
    Ok(row)
}

#[tauri::command]
pub async fn add_provider(
    state: State<'_, Arc<AppState>>,
    protocol: String,
    display_name: String,
    base_url: String,
    api_key: String,
) -> Result<db::ProviderRow, AppCommandError> {
    add_provider_inner(&state, protocol, display_name, base_url, api_key).await
}

// RULE-D-001 (2026-06-24): `None` = 保持原 key (留空覆盖 UX);
// `Some(v)` = 加密覆盖. 前端编辑 provider 时 apiKey input 留空
// 传 undefined → Tauri 反序列化为 `None`.
//
// Phase 2.2 (2026-07-21, task `07-20-remote-access-daemon-split`):
// the body was extracted into `update_provider_inner` so the
// daemon axum handler (`daemon::routes::providers::update_provider`)
// can call the same logic without going through the Tauri
// `#[tauri::command]` wrapper. The Q0 decision (design.md §5
// "handler vs service") keeps the business logic single-sourced.
pub async fn update_provider_inner(
    state: &Arc<AppState>,
    id: String,
    protocol: String,
    display_name: String,
    base_url: String,
    api_key: Option<String>,
) -> Result<Option<db::ProviderRow>, AppCommandError> {
    let row = db::update_provider(
        &state.db,
        &id,
        &protocol,
        &display_name,
        &base_url,
        api_key.as_deref(),
    )
    .await
    .map_err(|e| anyhow::anyhow!("update_provider failed: {}", e))?;
    state.rebuild_catalog().await;
    Ok(row)
}

#[tauri::command]
pub async fn update_provider(
    state: State<'_, Arc<AppState>>,
    id: String,
    protocol: String,
    display_name: String,
    base_url: String,
    api_key: Option<String>,
) -> Result<Option<db::ProviderRow>, AppCommandError> {
    update_provider_inner(&state, id, protocol, display_name, base_url, api_key).await
}

pub async fn delete_provider_inner(
    state: &Arc<AppState>,
    id: String,
) -> Result<bool, AppCommandError> {
    let ok = db::delete_provider(&state.db, &id)
        .await
        .map_err(|e| anyhow::anyhow!("delete_provider failed: {}", e))?;
    state.rebuild_catalog().await;
    Ok(ok)
}

#[tauri::command]
pub async fn delete_provider(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<bool, AppCommandError> {
    delete_provider_inner(&state, id).await
}

pub async fn list_models_inner(
    state: &Arc<AppState>,
) -> Result<Vec<db::ModelWithProvider>, AppCommandError> {
    db::list_models(&state.db)
        .await
        .map_err(|e| anyhow::anyhow!("list_models failed: {}", e).into())
}

#[tauri::command]
pub async fn list_models(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<db::ModelWithProvider>, AppCommandError> {
    list_models_inner(&state).await
}

#[allow(clippy::too_many_arguments)]
pub async fn add_model_inner(
    state: &Arc<AppState>,
    provider_id: String,
    model_name: String,
    display_name: String,
    max_tokens: Option<u32>,
    thinking_effort: Option<String>,
    supports_thinking: bool,
    supports_images: bool,
    context_window: u32,
) -> Result<db::ModelRow, AppCommandError> {
    let display_name = if display_name.is_empty() {
        model_name.clone()
    } else {
        display_name
    };
    let row = db::create_model(
        &state.db,
        &provider_id,
        &model_name,
        &display_name,
        max_tokens,
        thinking_effort.as_deref(),
        supports_thinking,
        supports_images,
        context_window,
    )
    .await
    .map_err(|e| anyhow::anyhow!("add_model failed: {}", e))?;
    state.rebuild_catalog().await;
    Ok(row)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn add_model(
    state: State<'_, Arc<AppState>>,
    provider_id: String,
    model_name: String,
    display_name: String,
    max_tokens: Option<u32>,
    thinking_effort: Option<String>,
    supports_thinking: bool,
    supports_images: bool,
    context_window: u32,
) -> Result<db::ModelRow, AppCommandError> {
    add_model_inner(
        &state,
        provider_id,
        model_name,
        display_name,
        max_tokens,
        thinking_effort,
        supports_thinking,
        supports_images,
        context_window,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn update_model_inner(
    state: &Arc<AppState>,
    id: String,
    provider_id: String,
    model_name: String,
    display_name: String,
    max_tokens: Option<u32>,
    thinking_effort: Option<String>,
    supports_thinking: bool,
    supports_images: bool,
    context_window: u32,
) -> Result<Option<db::ModelRow>, AppCommandError> {
    let display_name = if display_name.is_empty() {
        model_name.clone()
    } else {
        display_name
    };
    let row = db::update_model(
        &state.db,
        &id,
        &provider_id,
        &model_name,
        &display_name,
        max_tokens,
        thinking_effort.as_deref(),
        supports_thinking,
        supports_images,
        context_window,
    )
    .await
    .map_err(|e| anyhow::anyhow!("update_model failed: {}", e))?;
    state.rebuild_catalog().await;
    Ok(row)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn update_model(
    state: State<'_, Arc<AppState>>,
    id: String,
    provider_id: String,
    model_name: String,
    display_name: String,
    max_tokens: Option<u32>,
    thinking_effort: Option<String>,
    supports_thinking: bool,
    supports_images: bool,
    context_window: u32,
) -> Result<Option<db::ModelRow>, AppCommandError> {
    update_model_inner(
        &state,
        id,
        provider_id,
        model_name,
        display_name,
        max_tokens,
        thinking_effort,
        supports_thinking,
        supports_images,
        context_window,
    )
    .await
}

pub async fn delete_model_inner(
    state: &Arc<AppState>,
    id: String,
) -> Result<bool, AppCommandError> {
    let ok = db::delete_model(&state.db, &id)
        .await
        .map_err(|e| anyhow::anyhow!("delete_model failed: {}", e))?;
    state.rebuild_catalog().await;
    Ok(ok)
}

#[tauri::command]
pub async fn delete_model(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<bool, AppCommandError> {
    delete_model_inner(&state, id).await
}

pub async fn get_default_model_inner(
    state: &Arc<AppState>,
) -> Result<Option<db::ModelWithProvider>, AppCommandError> {
    let id = match db::get_config_value(&state.db, "default_model_id").await {
        Ok(Some(id)) => id,
        Ok(None) => return Ok(None),
        Err(e) => {
            return Err(anyhow::anyhow!("get_default_model failed: {}", e).into());
        }
    };
    let models = db::list_models(&state.db)
        .await
        .map_err(|e| anyhow::anyhow!("get_default_model failed: {}", e))?;
    Ok(models.into_iter().find(|m| m.model.id == id))
}

#[tauri::command]
pub async fn get_default_model(
    state: State<'_, Arc<AppState>>,
) -> Result<Option<db::ModelWithProvider>, AppCommandError> {
    get_default_model_inner(&state).await
}

pub async fn set_default_model_inner(
    state: &Arc<AppState>,
    model_id: String,
) -> Result<(), AppCommandError> {
    db::set_config_value(&state.db, "default_model_id", &model_id)
        .await
        .map_err(|e| anyhow::anyhow!("set_default_model failed: {}", e).into())
}

#[tauri::command]
pub async fn set_default_model(
    state: State<'_, Arc<AppState>>,
    model_id: String,
) -> Result<(), AppCommandError> {
    set_default_model_inner(&state, model_id).await
}

// ---------------------------------------------------------------------------
// PR4 of multi-model task: per-session model override + test_provider
// ---------------------------------------------------------------------------

/// Update the per-session model override. The frontend's StatusBar
/// dropdown calls this when the user selects a different model for a
/// specific session. The value is stored as `sessions.model_id`
/// (soft FK to `models.id`). The chat command's `resolve_chat_provider`
/// reads this column and falls back to the global default when NULL.
pub async fn update_session_model_id_inner(
    state: &Arc<AppState>,
    session_id: String,
    model_id: String,
) -> Result<(), AppCommandError> {
    db::update_session_model_id(&state.db, &session_id, &model_id)
        .await
        .map_err(|e| anyhow::anyhow!("update_session_model_id failed: {}", e).into())
}

#[tauri::command]
pub async fn update_session_model_id(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    model_id: String,
) -> Result<(), AppCommandError> {
    update_session_model_id_inner(&state, session_id, model_id).await
}

/// Test a provider's connectivity by sending a lightweight request
/// with the given `base_url`, `api_key`, and `protocol`. Returns
/// a JSON object with `success`, `latencyMs`, and optional `error`.
///
/// - Anthropic: `POST {base_url}/v1/messages` with `max_tokens=1`
///   and a minimal user message. A 200 means success; 401/403 means
///   auth failure.
/// - OpenAI: `GET {base_url}/models` with `Authorization: Bearer`.
///   A 200 means success; 401 means auth failure.
///
/// The function does NOT access `AppState` — it only makes an HTTP
/// request to validate the credentials, keeping the test isolated
/// from the app's DB and LLM dispatch.
///
/// DEPRECATED (PR5 follow-up): the frontend no longer calls this
/// IPC. The user-perceived Test flow is now `test_model`, which
/// validates end-to-end that a specific catalog `model.model_name`
/// can be reached (this function used a hardcoded
/// `claude-sonnet-4-5` body and was therefore unable to surface
/// model-name 404s on a GLM-style proxy). Kept in the registry
/// for future catalog-resolution use cases that need a
/// protocol-only reachability probe.
#[tauri::command]
#[allow(dead_code)]
pub async fn test_provider(
    base_url: String,
    api_key: String,
    protocol: String,
) -> Result<serde_json::Value, AppCommandError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build HTTP client: {}", e))?;
    let start = std::time::Instant::now();

    let (success, error) = match protocol.as_str() {
        "anthropic" => {
            let url = format!("{}/v1/messages", base_url.trim_end_matches('/'));
            let body = serde_json::json!({
                "model": "claude-sonnet-4-5",
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "hi"}]
            });
            let resp = client
                .post(&url)
                .header("x-api-key", &api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("request failed: {}", e))?;

            let status = resp.status();
            if status.is_success() {
                (true, None)
            } else {
                let body_text = resp.text().await.unwrap_or_default();
                (
                    false,
                    Some(format!(
                        "HTTP {}: {}",
                        status,
                        body_text.chars().take(200).collect::<String>()
                    )),
                )
            }
        }
        "openai" => {
            let url = format!("{}/models", base_url.trim_end_matches('/'));
            let resp = client
                .get(&url)
                .header("authorization", format!("Bearer {}", api_key))
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("request failed: {}", e))?;

            let status = resp.status();
            if status.is_success() {
                (true, None)
            } else {
                let body_text = resp.text().await.unwrap_or_default();
                (
                    false,
                    Some(format!(
                        "HTTP {}: {}",
                        status,
                        body_text.chars().take(200).collect::<String>()
                    )),
                )
            }
        }
        _ => (false, Some(format!("unsupported protocol: {}", protocol))),
    };

    let latency_ms = start.elapsed().as_millis() as u64;
    Ok(serde_json::json!({
        "success": success,
        "latencyMs": latency_ms,
        "error": error,
    }))
}

/// Test a specific model (looked up in the catalog) by sending a
/// lightweight request to its provider using the real `model_name`
/// the user configured.
///
/// Replaces the user-perceived "Test" flow from `test_provider` —
/// the per-model test is what the user actually cares about (can
/// this model name be reached end-to-end?). `test_provider`
/// remains in the registry for catalog-resolution future use.
pub async fn test_model_inner(
    state: &Arc<AppState>,
    model_id: String,
) -> Result<serde_json::Value, AppCommandError> {
    let model = match db::get_model(&state.db, &model_id).await {
        Ok(Some(m)) => m,
        Ok(None) => {
            return Ok(serde_json::json!({
                "success": false,
                "latencyMs": 0,
                "error": format!("model '{}' not found", model_id),
            }));
        }
        Err(e) => {
            return Ok(serde_json::json!({
                "success": false,
                "latencyMs": 0,
                "error": format!("failed to load model: {}", e),
            }));
        }
    };

    let provider = match db::get_provider(&state.db, &model.provider_id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return Ok(serde_json::json!({
                "success": false,
                "latencyMs": 0,
                "error": format!("provider for model '{}' is missing", model.display_name),
            }));
        }
        Err(e) => {
            return Ok(serde_json::json!({
                "success": false,
                "latencyMs": 0,
                "error": format!("failed to load provider: {}", e),
            }));
        }
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build HTTP client: {}", e))?;
    let start = std::time::Instant::now();

    let (success, error) = match provider.protocol.as_str() {
        "anthropic" => {
            let url = format!("{}/v1/messages", provider.base_url.trim_end_matches('/'));
            let body = serde_json::json!({
                "model": model.model_name,
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "hi"}]
            });
            let resp = match client
                .post(&url)
                .header("x-api-key", &provider.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    let latency_ms = start.elapsed().as_millis() as u64;
                    return Ok(serde_json::json!({
                        "success": false,
                        "latencyMs": latency_ms,
                        "error": format!("request failed: {}", e),
                    }));
                }
            };

            let status = resp.status();
            if status.is_success() {
                (true, None)
            } else {
                let body_text = resp.text().await.unwrap_or_default();
                (
                    false,
                    Some(format!(
                        "HTTP {}: {}",
                        status,
                        body_text.chars().take(200).collect::<String>()
                    )),
                )
            }
        }
        "openai" => {
            let url = format!(
                "{}/chat/completions",
                provider.base_url.trim_end_matches('/')
            );
            let body = serde_json::json!({
                "model": model.model_name,
                "messages": [{"role": "user", "content": "hi"}],
                "max_tokens": 1
            });
            let resp = match client
                .post(&url)
                .header("authorization", format!("Bearer {}", provider.api_key))
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    let latency_ms = start.elapsed().as_millis() as u64;
                    return Ok(serde_json::json!({
                        "success": false,
                        "latencyMs": latency_ms,
                        "error": format!("request failed: {}", e),
                    }));
                }
            };

            let status = resp.status();
            if status.is_success() {
                (true, None)
            } else {
                let body_text = resp.text().await.unwrap_or_default();
                (
                    false,
                    Some(format!(
                        "HTTP {}: {}",
                        status,
                        body_text.chars().take(200).collect::<String>()
                    )),
                )
            }
        }
        _ => (
            false,
            Some(format!("unsupported protocol: {}", provider.protocol)),
        ),
    };

    let latency_ms = start.elapsed().as_millis() as u64;
    Ok(serde_json::json!({
        "success": success,
        "latencyMs": latency_ms,
        "error": error,
    }))
}

#[tauri::command]
pub async fn test_model(
    state: State<'_, Arc<AppState>>,
    model_id: String,
) -> Result<serde_json::Value, AppCommandError> {
    test_model_inner(&state, model_id).await
}
