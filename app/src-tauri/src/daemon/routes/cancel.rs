//! `POST /api/v1/cancel/<command>` handlers for the cancel domain.
//!
//! Phase 2.2 B5 skeleton. Each handler deserializes a JSON body
//! into the same args the Tauri command takes, forwards to
//! `crate::commands::cancel::xxx_inner` (Q0 decision — single
//! source of truth), and wraps the result in `Json(...)`. Errors
//! flow through `AppCommandError`'s `IntoResponse` impl.

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;

use crate::commands::cancel::cancel_chat_inner;
use crate::error::AppCommandError;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct CancelChatRequest {
    pub request_id: String,
}

pub async fn cancel_chat(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CancelChatRequest>,
) -> Result<Json<()>, AppCommandError> {
    cancel_chat_inner(&state, req.request_id).await?;
    Ok(Json(()))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/cancel_chat", post(cancel_chat))
        .with_state(state)
}
