//! `POST /api/v1/files/<command>` handlers for the files domain.
//!
//! Phase 2.2 B5 skeleton. Each handler deserializes a JSON body
//! into the same args the Tauri command takes, forwards to
//! `crate::commands::files::xxx_inner` (Q0 decision — single
//! source of truth), and wraps the result in `Json(...)`. Errors
//! flow through `AppCommandError`'s `IntoResponse` impl.

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;

use crate::error::AppCommandError;
use crate::state::AppState;
use crate::commands::files::{list_files_inner, list_files_at_inner};

#[derive(Debug, Deserialize)]
pub struct ListFilesRequest {
    pub project_id: Option<String>,
    pub max_depth: Option<u32>,
}

pub async fn list_files(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ListFilesRequest>,
) -> Result<Json<Vec<String>>, AppCommandError> {
    let result = list_files_inner(&state, req.project_id, req.max_depth).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct ListFilesAtRequest {
    pub root: String,
    pub max_depth: Option<u32>,
}

pub async fn list_files_at(
    State(_state): State<Arc<AppState>>,
    Json(req): Json<ListFilesAtRequest>,
) -> Result<Json<Vec<String>>, AppCommandError> {
    // `list_files_at_inner` takes no AppState — the walk is fully
    // determined by its inputs. The State extractor is kept for
    // uniformity with the other handlers (so the router wires
    // `.with_state(state)` once for all routes in this module).
    let _ = _state;
    let result = list_files_at_inner(req.root, req.max_depth).await?;
    Ok(Json(result))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/list_files", post(list_files))
        .route("/list_files_at", post(list_files_at))
        .with_state(state)
}
