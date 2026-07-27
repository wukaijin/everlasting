//! `POST /api/v1/review/<command>` handlers for the review domain.
//!
//! C2 (review visualization view, 2026-07-26). Each handler
//! deserializes a JSON body into the same args the Tauri command
//! takes, forwards to `crate::commands::review::xxx_inner` (Q0
//! decision — single source of truth), and wraps the result in
//! `Json(...)`. Errors flow through `AppCommandError`'s
//! `IntoResponse` impl.
//!
//! Two routes mirror the two Tauri commands in `commands::review`:
//! - `POST /api/v1/review/get_review_state` — three-state payload
//!   for `<task>/review-state.json`.
//! - `POST /api/v1/review/get_current_task_slug` — `Option<{slug,
//!   id, title, status}>` via `resolve_current_task`.

use axum::{routing::post, Json, Router};
use serde::Deserialize;

use crate::commands::review::{
    get_current_task_slug_inner, get_review_state_inner, CurrentTaskInfo, ReviewStatePayload,
};
use crate::error::AppCommandError;

#[derive(Debug, Deserialize)]
pub struct GetReviewStateRequest {
    pub project_path: String,
    pub task_slug: String,
}

pub async fn get_review_state(
    Json(req): Json<GetReviewStateRequest>,
) -> Result<Json<ReviewStatePayload>, AppCommandError> {
    // `get_review_state_inner` is a pure blocking FS read — it
    // doesn't need AppState. Run it directly; the cost is one
    // file read + one serde_json parse, well under the axum
    // worker's blocking budget.
    Ok(Json(get_review_state_inner(
        &req.project_path,
        &req.task_slug,
    )))
}

#[derive(Debug, Deserialize)]
pub struct GetCurrentTaskSlugRequest {
    pub project_path: String,
}

pub async fn get_current_task_slug(
    Json(req): Json<GetCurrentTaskSlugRequest>,
) -> Result<Json<Option<CurrentTaskInfo>>, AppCommandError> {
    let result = get_current_task_slug_inner(req.project_path).await?;
    Ok(Json(result))
}

pub fn router() -> Router {
    Router::new()
        .route("/get_review_state", post(get_review_state))
        .route("/get_current_task_slug", post(get_current_task_slug))
}
