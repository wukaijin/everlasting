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

use crate::commands::cancel::{cancel_chat_inner, preempt_group_chat_inner};
use crate::error::AppCommandError;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct CancelChatRequest {
    pub request_id: String,
}

pub async fn cancel_chat(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CancelChatRequest>,
) -> Result<Json<crate::commands::cancel::CancelOutcome>, AppCommandError> {
    let outcome = cancel_chat_inner(&state, req.request_id).await?;
    Ok(Json(outcome))
}

// 09-06-gc-p0-preempt-min-semantics R2:session 域的体面打断(收束轮
// + summary + stop_reason=preempted)。与 `/cancel_chat`(rid 域硬停)
// 分域,见 `preempt_group_chat_inner` 文档。
#[derive(Debug, Deserialize)]
pub struct PreemptGroupChatRequest {
    pub session_id: String,
}

pub async fn preempt_group_chat(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PreemptGroupChatRequest>,
) -> Result<Json<crate::commands::cancel::PreemptOutcome>, AppCommandError> {
    let outcome = preempt_group_chat_inner(&state, req.session_id).await?;
    Ok(Json(outcome))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/cancel_chat", post(cancel_chat))
        .route("/preempt_group_chat", post(preempt_group_chat))
        .with_state(state)
}
