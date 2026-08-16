//! B1 (2026-08-16) image-attachment storage.
//!
//! Attachments live on the FILESYSTEM, keyed by session:
//! `<app_data_dir>/attachments/<session_id>/<uuid-hex>.<ext>`.
//! The DB only ever stores text content + a metadata reference
//! (design §0: "DB 永远只存文本 content + metadata 引用;Image 块每轮
//! 从磁盘引用即时生成"), so the SQLite file never carries image bytes.
//!
//! Security invariants (PRD R4/R7):
//! - filenames are server-generated UUID hex — client-supplied names
//!   never touch the path;
//! - `session_id` and `file` are strictly validated BEFORE any path
//!   join (traversal defense), and the canonicalized final path must
//!   still live under the session dir (double lock);
//! - media types are whitelisted (png / jpeg / webp) and each image is
//!   capped at 5 MiB (aligned with the stricter of the Anthropic /
//!   OpenAI per-image limits).

use std::path::{Path, PathBuf};

/// Hard per-image cap (PRD R6). Aligns with Anthropic's 5 MiB image
/// limit (the stricter of the two providers).
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

/// Allowed image media types (PRD R7 whitelist). bmp / tiff / heic /
/// gif are rejected — the user is told to convert instead.
pub const ALLOWED_IMAGE_MEDIA_TYPES: &[&str] = &["image/png", "image/jpeg", "image/webp"];

#[derive(Debug, thiserror::Error)]
pub enum AttachmentError {
    #[error("invalid session id")]
    InvalidSessionId,
    #[error("invalid attachment file name")]
    InvalidFileName,
    #[error("media type {0} not allowed (png / jpeg / webp only)")]
    MediaTypeNotAllowed(String),
    #[error("image exceeds the {0} byte cap")]
    TooLarge(usize),
    #[error("attachment not found")]
    NotFound,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid base64: {0}")]
    Base64(#[from] base64::DecodeError),
}

/// `<app_data_dir>/attachments` — session-keyed root.
pub fn attachments_root(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("attachments")
}

/// `<app_data_dir>/attachments/<session_id>`
pub fn session_attachments_dir(app_data_dir: &Path, session_id: &str) -> PathBuf {
    attachments_root(app_data_dir).join(session_id)
}

/// Session ids are generated (frontend `genId` / DB uuid shapes);
/// accept the conservative `[A-Za-z0-9_-]{8..64}` envelope so a path
/// traversal payload (`../x`, absolute paths, separators) can never
/// pass validation.
pub fn is_valid_session_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Attachment file names are server-generated `<32-hex>.<ext>`.
/// The strict shape doubles as the traversal guard for the GET route.
pub fn is_valid_attachment_filename(s: &str) -> bool {
    let Some((stem, ext)) = s.rsplit_once('.') else {
        return false;
    };
    let ext_ok = matches!(ext, "png" | "jpg" | "jpeg" | "webp");
    stem.len() == 32 && stem.chars().all(|c| c.is_ascii_hexdigit()) && ext_ok
}

fn ext_for_media_type(media_type: &str) -> Option<&'static str> {
    match media_type {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

fn media_type_for_ext(ext: &str) -> Option<&'static str> {
    match ext {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// Persist one image for `session_id`. Returns the generated file name
/// (e.g. `"a1b2…32-hex….png"`) that callers store in message metadata
/// and later hand to [`read_image`] / the GET route.
///
/// The write is a fresh UUID name — re-saving the same bytes creates a
/// new file (content-hash dedup was explicitly cut from MVP scope,
/// design §3.1).
pub async fn save_image(
    app_data_dir: &Path,
    session_id: &str,
    media_type: &str,
    bytes: &[u8],
) -> Result<String, AttachmentError> {
    if !is_valid_session_id(session_id) {
        return Err(AttachmentError::InvalidSessionId);
    }
    if !ALLOWED_IMAGE_MEDIA_TYPES.contains(&media_type) {
        return Err(AttachmentError::MediaTypeNotAllowed(media_type.to_string()));
    }
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(AttachmentError::TooLarge(MAX_IMAGE_BYTES));
    }
    let ext = ext_for_media_type(media_type).expect("media type whitelisted above");
    let file = format!("{}.{}", uuid::Uuid::new_v4().simple(), ext);
    let dir = session_attachments_dir(app_data_dir, session_id);
    tokio::fs::create_dir_all(&dir).await?;
    tokio::fs::write(dir.join(&file), bytes).await?;
    Ok(file)
}

/// Read one attachment back. Validates both path components, then
/// double-checks the canonicalized path still lives under the session
/// dir (defense in depth against a crafted-but-shape-valid name).
/// Returns `(media_type, bytes)` for the GET route / wire resolve.
pub async fn read_image(
    app_data_dir: &Path,
    session_id: &str,
    file: &str,
) -> Result<(String, Vec<u8>), AttachmentError> {
    if !is_valid_session_id(session_id) {
        return Err(AttachmentError::InvalidSessionId);
    }
    if !is_valid_attachment_filename(file) {
        return Err(AttachmentError::InvalidFileName);
    }
    let dir = session_attachments_dir(app_data_dir, session_id);
    let path = dir.join(file);
    // Double lock: canonicalize and require the session dir prefix.
    // A nonexistent file fails here as NotFound — callers map that to
    // a 404.
    let canon = match tokio::fs::canonicalize(&path).await {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(AttachmentError::NotFound)
        }
        Err(e) => return Err(e.into()),
    };
    let canon_dir = tokio::fs::canonicalize(&dir)
        .await
        .map_err(|_| AttachmentError::NotFound)?;
    if !canon.starts_with(&canon_dir) {
        return Err(AttachmentError::InvalidFileName);
    }
    let ext = file.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    let media_type = media_type_for_ext(ext).ok_or(AttachmentError::InvalidFileName)?;
    let bytes = tokio::fs::read(&canon).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AttachmentError::NotFound
        } else {
            e.into()
        }
    })?;
    Ok((media_type.to_string(), bytes))
}

/// Delete a session's whole attachments directory (best-effort).
/// Called from `delete_session_inner` next to the stub / digest
/// registry cleanup so a deleted session leaves no orphan files.
/// `session_id` is validated BEFORE the path is built — a bad id must
/// never reach `remove_dir_all`.
pub fn delete_session_attachments(app_data_dir: &Path, session_id: &str) {
    if !is_valid_session_id(session_id) {
        tracing::warn!(session_id = %session_id, "delete_session_attachments: invalid id, skipping");
        return;
    }
    let dir = session_attachments_dir(app_data_dir, session_id);
    if dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            tracing::warn!(dir = %dir.display(), error = %e, "delete_session_attachments failed (non-fatal)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("everlasting-att-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn tiny_png() -> Vec<u8> {
        b"\x89PNG\r\n\x1a\n tiny".to_vec()
    }

    #[tokio::test]
    async fn save_and_read_round_trip() {
        let root = tmp_root();
        let file = save_image(&root, "sess12345678", "image/png", &tiny_png())
            .await
            .unwrap();
        assert!(is_valid_attachment_filename(&file));
        let (media, bytes) = read_image(&root, "sess12345678", &file).await.unwrap();
        assert_eq!(media, "image/png");
        assert_eq!(bytes, tiny_png());
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn rejects_media_type_outside_whitelist() {
        let root = tmp_root();
        let err = save_image(&root, "sess12345678", "image/gif", &tiny_png())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not allowed"), "{err}");
        let err = save_image(&root, "sess12345678", "image/bmp", &tiny_png())
            .await
            .unwrap_err();
        assert!(matches!(err, AttachmentError::MediaTypeNotAllowed(_)));
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn rejects_oversize_image() {
        let root = tmp_root();
        let mut bytes = vec![0u8; MAX_IMAGE_BYTES + 1];
        bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        let err = save_image(&root, "sess12345678", "image/png", &bytes)
            .await
            .unwrap_err();
        assert!(matches!(err, AttachmentError::TooLarge(_)));
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn rejects_traversal_session_id_and_filename() {
        let root = tmp_root();
        // session id with separators / dots is invalid before any IO.
        for bad in ["../etc", "a/b", "..", ".hidden-dir"] {
            assert!(!is_valid_session_id(bad), "{bad} must be invalid");
            let err = save_image(&root, bad, "image/png", &tiny_png()).await;
            assert!(matches!(err, Err(AttachmentError::InvalidSessionId)));
        }
        // filename must be the strict uuid-hex shape.
        for bad in [
            "../../etc/passwd",
            "abc.png",
            "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6.sh",
            "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6.png.exe",
        ] {
            assert!(!is_valid_attachment_filename(bad), "{bad} must be invalid");
            let err = read_image(&root, "sess12345678", bad).await;
            assert!(matches!(err, Err(AttachmentError::InvalidFileName)));
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn read_missing_file_is_not_found() {
        let root = tmp_root();
        let file = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6.png";
        let err = read_image(&root, "sess12345678", file).await.unwrap_err();
        assert!(matches!(err, AttachmentError::NotFound));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn delete_removes_session_dir_only() {
        let root = tmp_root();
        let f1 = futures_lite_block(async {
            save_image(&root, "sessAAAA1111", "image/png", &tiny_png()).await
        });
        let f2 = futures_lite_block(async {
            save_image(&root, "sessBBBB2222", "image/png", &tiny_png()).await
        });
        assert!(f1.is_ok() && f2.is_ok());
        delete_session_attachments(&root, "sessAAAA1111");
        assert!(!session_attachments_dir(&root, "sessAAAA1111").exists());
        assert!(session_attachments_dir(&root, "sessBBBB2222").exists());
        // Invalid id must not delete anything.
        delete_session_attachments(&root, "../attachments");
        assert!(session_attachments_dir(&root, "sessBBBB2222").exists());
        std::fs::remove_dir_all(&root).ok();
    }

    // Minimal block-on for sync test contexts (the crate's test utils
    // pull in heavier harnesses; a plain tokio Runtime suffices here).
    fn futures_lite_block<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(fut)
    }
}
