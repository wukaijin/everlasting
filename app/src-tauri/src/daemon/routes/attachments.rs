//! B1 (2026-08-16) attachments HTTP surface.
//!
//! - `POST /api/v1/attachments/save` — paste-image upload (the JSON
//!   mirror of the `save_attachment` Tauri command).
//! - `GET  /api/v1/attachments/:session_id/:file` — the `<img>` fetch
//!   path. This is the daemon's **first binary GET route**: unlike the
//!   other domains (all POST + Json), it returns raw bytes with the
//!   correct `Content-Type`, and it is what makes attachments visible
//!   in browser / PWA mode where local file paths don't exist
//!   (design §3.2). Both path components are strictly validated in
//!   [`crate::attachments`] before any filesystem touch.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use crate::attachments::{self, AttachmentError};
use crate::commands::attachments::{save_attachment_inner, SaveAttachmentResponse};
use crate::error::AppCommandError;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct SaveAttachmentRequest {
    pub session_id: String,
    pub media_type: String,
    pub data_base64: String,
}

pub async fn save_attachment(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SaveAttachmentRequest>,
) -> Result<Json<SaveAttachmentResponse>, AppCommandError> {
    let result =
        save_attachment_inner(&state, req.session_id, req.media_type, req.data_base64).await?;
    Ok(Json(result))
}

pub async fn get_attachment(
    State(state): State<Arc<AppState>>,
    Path((session_id, file)): Path<(String, String)>,
) -> Response {
    match attachments::read_image(&state.app_data_dir, &session_id, &file).await {
        Ok((media_type, bytes)) => (
            StatusCode::OK,
            [
                // Attachment names are immutable UUIDs — cacheable.
                (header::CONTENT_TYPE, media_type),
                (
                    header::CACHE_CONTROL,
                    "private, max-age=86400, immutable".to_string(),
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(AttachmentError::NotFound) => {
            (StatusCode::NOT_FOUND, "attachment not found").into_response()
        }
        Err(AttachmentError::InvalidSessionId | AttachmentError::InvalidFileName) => {
            (StatusCode::BAD_REQUEST, "invalid attachment path").into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("read attachment failed: {}", e),
        )
            .into_response(),
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        // 命令名即路径段(全项目惯例:httpTransport 拼
        // `/api/v1/{domain}/{cmd}`,如 files/list_files)— 写 `/save`
        // 会让前端 invoke 打到不存在的 `/save_attachment` 而 404。
        .route("/save_attachment", post(save_attachment))
        .route("/:session_id/:file", get(get_attachment))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attachments::save_image;
    use crate::state::AppState;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt; // oneshot

    /// Route-level smoke via `AppState::load_from_dir` (same pattern
    /// as `state.rs` `state_load_path_consistency`; `multi_thread`
    /// flavor because the load-time backfill spawn uses the Tauri
    /// async-runtime shim).
    #[tokio::test(flavor = "multi_thread")]
    async fn get_attachment_serves_bytes_with_content_type() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);
        let file = save_image(
            &state.app_data_dir,
            "sessRouteTest1",
            "image/png",
            b"\x89PNG\r\n\x1a\n test".as_slice(),
        )
        .await
        .unwrap();
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/{}/{}", "sessRouteTest1", file))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/png"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn get_attachment_rejects_traversal_and_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);
        let app = router(state);
        // Not the strict uuid-hex shape → 400 (never touches the fs).
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/sessRouteTest1/evil.png.exe")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        // Well-formed name that doesn't exist → 404.
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/sessRouteTest1/ffffffffffffffffffffffffffffffff.png")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
