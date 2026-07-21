//! `POST /api/v1/ui/<command>` handlers for the ui domain.
//!
//! Phase 2.2 B5 skeleton. Each handler deserializes a JSON body
//! into the same args the Tauri command takes, forwards to
//! `crate::commands::ui::xxx_inner` (Q0 decision — single
//! source of truth), and wraps the result in `Json(...)`. Errors
//! flow through `AppCommandError`'s `IntoResponse` impl.

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;

use crate::error::AppCommandError;
use crate::state::AppState;
use crate::commands::ui::{apply_ui_diff_inner, ApplyUiDiffResult};

#[derive(Debug, Deserialize)]
pub struct ApplyUiDiffRequest {
    pub session_id: String,
    pub diff_text: String,
}

pub async fn apply_ui_diff(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ApplyUiDiffRequest>,
) -> Result<Json<ApplyUiDiffResult>, AppCommandError> {
    let result = apply_ui_diff_inner(&state, req.session_id, req.diff_text).await?;
    Ok(Json(result))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/apply_ui_diff", post(apply_ui_diff))
        .with_state(state)
}
