//! B1 (2026-08-16) attachments IPC — the paste-image upload path.
//!
//! The staged-clipboard flow: the frontend uploads each staged image
//! right before sending the chat turn (`save_attachment`), gets back
//! the generated file name, and includes it in the chat request's
//! `attachments` manifest. Storage lives in [`crate::attachments`];
//! this module is the Tauri-command + daemon-route shared entry
//! (`xxx_inner` single source of truth, same shape as the other
//! command domains).

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde::Serialize;
use tauri::State;

use crate::error::AppCommandError;
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct SaveAttachmentResponse {
    /// Server-generated file name inside
    /// `<app_data_dir>/attachments/<session_id>/`. The frontend keeps
    /// it in the message's `metadata.attachments` manifest.
    pub file: String,
}

pub async fn save_attachment_inner(
    state: &std::sync::Arc<AppState>,
    session_id: String,
    media_type: String,
    data_base64: String,
) -> Result<SaveAttachmentResponse, AppCommandError> {
    let bytes = B64
        .decode(data_base64.as_bytes())
        .map_err(|e| anyhow::anyhow!("save_attachment: invalid base64: {}", e))?;
    let file =
        crate::attachments::save_image(&state.app_data_dir, &session_id, &media_type, &bytes)
            .await
            .map_err(|e| anyhow::anyhow!("save_attachment failed: {}", e))?;
    Ok(SaveAttachmentResponse { file })
}

#[tauri::command]
pub async fn save_attachment(
    state: State<'_, std::sync::Arc<AppState>>,
    session_id: String,
    media_type: String,
    data_base64: String,
) -> Result<SaveAttachmentResponse, AppCommandError> {
    save_attachment_inner(&state, session_id, media_type, data_base64).await
}
