//! `POST /api/v1/config/<command>` handlers for the config domain.
//!
//! Phase 2.2 B5 skeleton. Each handler deserializes a JSON body
//! into the same args the Tauri command takes, forwards to
//! `crate::commands::config::xxx_inner` (Q0 decision — single
//! source of truth), and wraps the result in `Json(...)`. Errors
//! flow through `AppCommandError`'s `IntoResponse` impl.

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};

use crate::error::AppCommandError;
use crate::state::AppState;
use crate::commands::config::{get_llm_config_inner, PublicLlmConfig};

pub async fn get_llm_config(
    State(state): State<Arc<AppState>>,
) -> Result<Json<PublicLlmConfig>, AppCommandError> {
    let result = get_llm_config_inner(&state).await?;
    Ok(Json(result))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/get_llm_config", post(get_llm_config))
        .with_state(state)
}
