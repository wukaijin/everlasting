//! `GET /health` + `GET /api/v1/health` —— remote 健康检查(design §3.1)。
//!
//! 无认证(nginx 健康检查用;与 daemon `/api/v1/health` 对齐 —— 双路径
//! 都通,`/health` 是给 ad-hoc curl / 外部探针的短路径)。
//!
//! 契约只有 **200** 本身(nginx `proxy_pass` 探测);body 字段仅供人工
//! 排障。对齐 daemon `HealthResponse` 的 shape:
//! ```json
//! 200 OK
//! { "remoteId": "uuid-v4", "remoteVersion": "0.1.0",
//!   "apiVersions": ["v1"], "uptimeSeconds": 3600 }
//! ```
//!
//! `remote_id` 进程内稳定(首次请求生成,重启变化) —— 与 daemon
//! `daemon_id` 同语义,供上层判断"remote 是否重启过"。

use std::sync::OnceLock;
use std::time::Instant;

use axum::{response::IntoResponse, Json};
use serde::Serialize;
use uuid::Uuid;

/// remote 说的 API 版本。S1 只有 `v1`;加 v2 是破坏性变更(与 daemon
/// `SUPPORTED_API_VERSIONS` 同约定)。
pub const SUPPORTED_API_VERSIONS: &[&str] = &["v1"];

/// Crate 版本,`env!("CARGO_PKG_VERSION")` 编译期注入。
pub const REMOTE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 进程启动时刻(首次请求捕获)。`Instant` 本机计时,无跨机时钟问题。
static START_TIME: OnceLock<Instant> = OnceLock::new();

/// 进程唯一 id(首次请求生成)。
static REMOTE_ID: OnceLock<String> = OnceLock::new();

/// `GET /health` + `GET /api/v1/health` 共用 handler。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    /// 进程唯一 id(UUID v4)。进程内稳定,重启变化。
    pub remote_id: String,
    /// Crate 版本(Cargo.toml)。
    pub remote_version: String,
    /// 支持的 API 版本。手机 PWA 要求 `v1` 存在。
    pub api_versions: Vec<&'static str>,
    /// 首次 health 请求以来的秒数。
    pub uptime_seconds: u64,
}

/// `GET /health` + `GET /api/v1/health` handler。
///
/// 无状态(不依赖 `RemoteState`)—— 健康检查在 DB/WSS 就绪前也要能答,
/// 与 daemon 的 bare-bones health 同理(nginx 探针只认 200)。
pub async fn health() -> impl IntoResponse {
    let start = *START_TIME.get_or_init(Instant::now);
    let remote_id = REMOTE_ID.get_or_init(|| Uuid::new_v4().to_string()).clone();
    let uptime_seconds = start.elapsed().as_secs();

    Json(HealthResponse {
        remote_id,
        remote_version: REMOTE_VERSION.to_string(),
        api_versions: SUPPORTED_API_VERSIONS.to_vec(),
        uptime_seconds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    /// 端到端冒烟:health handler 装配进 router 后,`/health` 与
    /// `/api/v1/health` 都返 200 + 契约 JSON 字段(design §3.1 双路径)。
    #[tokio::test]
    async fn health_returns_canonical_shape_on_both_paths() {
        let app = axum::Router::new()
            .route("/health", axum::routing::get(health))
            .route("/api/v1/health", axum::routing::get(health));

        for uri in ["/health", "/api/v1/health"] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .expect("router succeeded");
            assert_eq!(response.status(), StatusCode::OK, "path {uri} must be 200");
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body collected");
            let json: serde_json::Value = serde_json::from_slice(&body).expect("body is JSON");
            assert!(json.get("remoteId").is_some(), "remoteId present");
            assert!(json.get("remoteVersion").is_some(), "remoteVersion present");
            let api_versions: Vec<String> =
                serde_json::from_value(json.get("apiVersions").cloned().unwrap_or_default())
                    .expect("apiVersions deserializes");
            assert!(
                api_versions.iter().any(|v| v == "v1"),
                "apiVersions contains v1"
            );
            assert!(
                json.get("uptimeSeconds").and_then(|v| v.as_u64()).is_some(),
                "uptimeSeconds is a non-negative integer"
            );
        }
    }

    /// `remote_id` 进程内稳定(两次请求同 id),重启变化 —— 与 daemon
    /// `daemon_id` 同语义。
    #[tokio::test]
    async fn remote_id_is_stable_within_process() {
        let app = axum::Router::new().route("/health", axum::routing::get(health));

        let get_id = || async {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/health")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .expect("router succeeded");
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body collected");
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            json["remoteId"].as_str().unwrap().to_string()
        };

        assert_eq!(get_id().await, get_id().await);
    }

    /// `SUPPORTED_API_VERSIONS` 含 `v1` —— 手机 PWA 的版本门禁依赖它。
    #[test]
    fn supported_api_versions_contains_v1() {
        assert!(SUPPORTED_API_VERSIONS.contains(&"v1"));
    }
}
