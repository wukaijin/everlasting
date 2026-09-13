//! `POST /api/v1/files/<command>` handlers for the files domain, plus
//! the domain's binary GET routes (`/image` + `/raw`, see below).
//!
//! Phase 2.2 B5 skeleton. Each POST handler deserializes a JSON body
//! into the same args the Tauri command takes, forwards to
//! `crate::commands::files::xxx_inner` (Q0 decision — single
//! source of truth), and wraps the result in `Json(...)`. Errors
//! flow through `AppCommandError`'s `IntoResponse` impl.
//!
//! `GET /api/v1/files/image?path=<abs|~前缀>`(2026-09-13 图片路径预览)
//! 与 `GET /api/v1/files/raw?path=<abs|~前缀>`(同日文件通道:文本类 +
//! pdf)是本域的两条 binary GET 路由,模式照 attachments 域的
//! `get_attachment`:返回原始字节 + Content-Type,供前端弹层直连
//! (pwa-remote 经 remote proxy catch-all 转发,`?access_token=` query
//! 鉴权)。错误分类 match `ReadImageError` / `ReadRawError` →
//! 400/404/413/500。

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use crate::commands::files::{
    list_files_at_inner, list_files_inner, read_image_at_inner, read_raw_at_inner, ReadImageError,
    ReadRawError,
};
use crate::error::AppCommandError;
use crate::state::AppState;

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

#[derive(Debug, Deserialize)]
pub struct ReadImageQuery {
    pub path: String,
}

/// `GET /api/v1/files/image?path=...` — 本地图片字节直连(前端弹层
/// `<img>` 消费)。handler 是薄壳:query 提取 + 转发
/// [`read_image_at_inner`] + 状态码映射;校验逻辑全部在 commands 层。
/// 缺 `path` query 时 axum 的 `Query` extractor 自身回 400。
pub async fn read_image(Query(q): Query<ReadImageQuery>) -> Response {
    match read_image_at_inner(q.path).await {
        Ok((content_type, bytes)) => (
            StatusCode::OK,
            [
                // 路径指向的文件可被覆盖重写(ui-review 每次跑都重生成),
                // 不用 attachments 的 immutable;短缓存兼顾弹层重开。
                (header::CONTENT_TYPE, content_type.to_string()),
                (header::CACHE_CONTROL, "private, max-age=60".to_string()),
            ],
            bytes,
        )
            .into_response(),
        Err(ReadImageError::InvalidRequest(msg)) => (StatusCode::BAD_REQUEST, msg).into_response(),
        Err(ReadImageError::NotFound) => (StatusCode::NOT_FOUND, "image not found").into_response(),
        Err(ReadImageError::TooLarge) => {
            (StatusCode::PAYLOAD_TOO_LARGE, "image too large").into_response()
        }
        Err(ReadImageError::Io(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("read image failed: {}", e),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct ReadRawQuery {
    pub path: String,
}

/// `GET /api/v1/files/raw?path=...` — 本地文本/pdf 文件直连(FileViewerModal
/// fetch 消费;pdf 不进弹层,前端拿同一 URL 开新标签)。handler 薄壳同
/// [`read_image`]:query 提取 + 转发 [`read_raw_at_inner`] + 状态码映射;
/// 校验逻辑全部在 commands 层。文本类 Content-Type 由 commands 层强制
/// `text/plain; charset=utf-8`(.html/.htm 也一样,MIME 即闸门)。
pub async fn read_raw(Query(q): Query<ReadRawQuery>) -> Response {
    match read_raw_at_inner(q.path).await {
        Ok((content_type, bytes)) => (
            StatusCode::OK,
            [
                // 同 image:产物可被覆盖重写,短缓存兼顾弹层重开。
                (header::CONTENT_TYPE, content_type.to_string()),
                (header::CACHE_CONTROL, "private, max-age=60".to_string()),
            ],
            bytes,
        )
            .into_response(),
        Err(ReadRawError::InvalidRequest(msg)) => (StatusCode::BAD_REQUEST, msg).into_response(),
        Err(ReadRawError::NotFound) => (StatusCode::NOT_FOUND, "file not found").into_response(),
        Err(ReadRawError::TooLarge) => {
            (StatusCode::PAYLOAD_TOO_LARGE, "file too large").into_response()
        }
        Err(ReadRawError::Io(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("read file failed: {}", e),
        )
            .into_response(),
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/list_files", post(list_files))
        .route("/list_files_at", post(list_files_at))
        .route("/image", get(read_image))
        .route("/raw", get(read_raw))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt; // oneshot

    /// 同 attachments.rs 路由测试的 oneshot 模式(`multi_thread` 因
    /// load-time backfill spawn 用 Tauri async-runtime shim)。
    #[tokio::test(flavor = "multi_thread")]
    async fn image_route_serves_bytes_with_content_type() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("shot.png");
        tokio::fs::write(&file, b"\x89PNG\r\n\x1a\n test".as_slice())
            .await
            .unwrap();
        let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/image?path={}", file.to_str().unwrap()))
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
    async fn image_route_rejects_bad_extension_relative_and_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);
        let app = router(state);
        // 非白名单扩展 → 400。
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/image?path=/etc/passwd")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        // 相对路径 → 400(契约:相对路径由前端按 cwd 解析,daemon 不猜)。
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/image?path=out/ui-review/1.png")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        // 白名单扩展但文件不存在 → 404。
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/image?path=/nonexistent/definitely-missing.png")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    /// 超限走稀疏文件(set_len 不落盘 33 MiB 实字节),只验 metadata
    /// 检查臂;读后复核臂(TOCTOU)由 413 分类共享同一枚举,不重复测。
    #[tokio::test(flavor = "multi_thread")]
    async fn image_route_rejects_oversized_file() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("huge.png");
        let f = std::fs::File::create(&file).unwrap();
        f.set_len(crate::commands::files::MAX_IMAGE_BYTES + 1)
            .unwrap();
        drop(f);
        let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/image?path={}", file.to_str().unwrap()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    // --- /raw(2026-09-13 文件通道,镜像上方 image 三条)---------------------

    /// 200 臂:文本(pdf 之外的每种关键形态)与 pdf 各自的 Content-Type。
    /// `.html` 断言的是安全契约 —— 它在白名单里但必须按 text/plain 下发,
    /// 绝不出现 text/html 下发路径(MIME 即闸门)。
    #[tokio::test(flavor = "multi_thread")]
    async fn raw_route_serves_text_and_pdf_with_content_type() {
        let tmp = tempfile::tempdir().unwrap();
        let md = tmp.path().join("report.md");
        tokio::fs::write(&md, "# 标题\n\n正文".as_bytes())
            .await
            .unwrap();
        let html = tmp.path().join("page.html");
        tokio::fs::write(&html, b"<html><body>hi</body></html>")
            .await
            .unwrap();
        let pdf = tmp.path().join("doc.pdf");
        tokio::fs::write(&pdf, b"%PDF-1.4 fake-bytes")
            .await
            .unwrap();
        let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);
        let app = router(state);
        for (file, expect_ct) in [
            (md, "text/plain; charset=utf-8"),
            (html, "text/plain; charset=utf-8"),
            (pdf, "application/pdf"),
        ] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/raw?path={}", file.to_str().unwrap()))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{}", file.display());
            assert_eq!(
                resp.headers().get(header::CONTENT_TYPE).unwrap(),
                expect_ct,
                "{}",
                file.display()
            );
            assert_eq!(
                resp.headers().get(header::CACHE_CONTROL).unwrap(),
                "private, max-age=60"
            );
        }
    }

    /// 400/404 臂:非白名单扩展、相对路径、非法 UTF-8 文本 → 400;
    /// 白名单扩展但文件不存在 → 404。非法 UTF-8 单独成臂 —— 这是 /raw
    /// 特有分类(image 无内容校验)。
    #[tokio::test(flavor = "multi_thread")]
    async fn raw_route_rejects_bad_extension_relative_bad_utf8_and_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let binary_txt = tmp.path().join("binary.txt");
        tokio::fs::write(&binary_txt, [0xff, 0xfe, 0x00, 0xd8])
            .await
            .unwrap();
        let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);
        let app = router(state);
        // 非白名单扩展(.exe 不在表内)→ 400。
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/raw?path=/tmp/evil.exe")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        // 相对路径 → 400(契约:相对路径由前端按 cwd 解析,daemon 不猜)。
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/raw?path=src/main.rs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        // 白名单扩展但内容非法 UTF-8 → 400(二进制误命名当拒,不给
        // replacement char 糊弄弹层的字符串消费)。
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/raw?path={}", binary_txt.to_str().unwrap()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        // 白名单扩展但文件不存在 → 404。
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/raw?path=/nonexistent/definitely-missing.md")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    /// 文本档 413:稀疏文件 >2 MiB(MAX_TEXT_BYTES + 1)只验 metadata
    /// 检查臂,同 image 的做法。pdf 档沿用 MAX_IMAGE_BYTES,分类共享
    /// 同一枚举臂,不重复测。
    #[tokio::test(flavor = "multi_thread")]
    async fn raw_route_rejects_oversized_text_file() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("huge.md");
        let f = std::fs::File::create(&file).unwrap();
        f.set_len(crate::commands::files::MAX_TEXT_BYTES + 1)
            .unwrap();
        drop(f);
        let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/raw?path={}", file.to_str().unwrap()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }
}
