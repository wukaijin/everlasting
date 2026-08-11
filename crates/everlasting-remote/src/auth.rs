//! 认证双通道(design §3.1,implement.md Step 4):
//!
//! - **shared_secret**:WSS 握手(Step 5 ws.rs 用),`subtle::ConstantTimeEq`
//!   常时比较(P3-4),防 timing side-channel
//! - **device_token**:手机 HTTP 请求。`Authorization: Bearer <token>`
//!   header 优先(普通 fetch),无则 `?access_token=<token>` query
//!   (P1-2 修订 —— 浏览器 `EventSource` 无法设 header,SSE 路径必然
//!   走 query)。token → `devices` 表查绑定 → 注入
//!   [`AuthenticatedDevice`] 到 request extension
//!
//! **剥离时机在 Step 6 proxy**:构造 Request 帧时剥 `Authorization` header
//! + `access_token` query(P2-1)—— 转发给 PC daemon 的帧是纯净的,
//! PC 侧看不到也不该看到手机 token。本中间件只认证 + 注入。

use std::sync::Arc;

use axum::extract::{Query, Request, State};
use axum::http::{HeaderMap, Uri};
use axum::middleware::Next;
use axum::response::Response;
use serde::Deserialize;

use crate::config::RemoteState;
use crate::db;
use crate::error::AppError;

/// 认证通过的设备(注入 request extension,下游 handler 提取 `node_id`)。
#[derive(Debug, Clone)]
pub struct AuthenticatedDevice {
    /// 本次请求携带的 token(header 或 query 提取的原始值)。
    pub token: String,
    /// `devices` 表绑定的节点 —— 反向代理找 WSS 连接用(Step 6)。
    pub node_id: String,
}

/// shared_secret 常时比较(P3-4):`subtle::ConstantTimeEq`。
/// 长度不同直接返 false —— 长度信息经 TLS 传输不敏感,不额外处理。
pub fn verify_shared_secret(actual: &str, expected: &str) -> bool {
    use subtle::ConstantTimeEq;
    actual.as_bytes().ct_eq(expected.as_bytes()).into()
}

/// 提取 device_token:**header `Authorization: Bearer` 优先,无则
/// query `?access_token=`**(P1-2)。两个通道都无 → `None`。
pub fn device_token_from_request(headers: &HeaderMap, uri: &Uri) -> Option<String> {
    bearer_token(headers).or_else(|| query_access_token(uri))
}

/// `Authorization: Bearer <token>`;大小写/空白容忍(`Bearer` 大小写
/// 敏感是 HTTP 惯例,直接按字面匹配)。
fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?;
    value
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// `?access_token=<token>`。`Query::try_from_uri`(axum 0.7)在
/// middleware 场景解析 query,serde_urlencoded 自动 percent-decode
/// (P2-1 编码反向)。
fn query_access_token(uri: &Uri) -> Option<String> {
    #[derive(Deserialize)]
    struct Params {
        access_token: Option<String>,
    }
    let params = Query::<Params>::try_from_uri(uri).ok()?;
    params.0.access_token.filter(|s| !s.is_empty())
}

/// axum middleware:提取 token → 查 `devices` 表 → 有效则注入
/// [`AuthenticatedDevice`] 并放行;无效 / 吊销 → 401 Auth(统一 wire
/// shape,error.rs)。
///
/// 挂载方式(axum 0.7 官方模式,`State` 在 middleware 里从 router
/// state 提取):
/// ```rust,ignore
/// Router::new().route("/api/v1/nodes", get(...))
///     .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_device_token))
///     .with_state(state)
/// ```
pub async fn require_device_token(
    State(state): State<Arc<RemoteState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let token = device_token_from_request(req.headers(), req.uri()).ok_or_else(|| {
        AppError::auth("missing device token:需要 Authorization: Bearer <token> 或 ?access_token=<token>")
    })?;

    let device = db::crud::get_device_by_token(&state.db, &token)
        .await
        .map_err(|e| AppError::server(format!("device lookup failed: {e}")))?;
    let Some(device) = device else {
        return Err(AppError::auth("invalid device token"));
    };
    if device.revoked != 0 {
        return Err(AppError::auth("device token revoked"));
    }

    tracing::debug!(node_id = %device.node_id, "device authenticated");
    req.extensions_mut().insert(AuthenticatedDevice {
        token,
        node_id: device.node_id,
    });
    Ok(next.run(req).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{Method, StatusCode};
    use axum::routing::get;
    use axum::{Extension, Json, Router, middleware};
    use tower::ServiceExt;

    /// 回显 extension 里的 `AuthenticatedDevice`(验证中间件注入成功)。
    async fn echo_device(
        Extension(dev): Extension<AuthenticatedDevice>,
    ) -> Json<serde_json::Value> {
        Json(serde_json::json!({ "node_id": dev.node_id, "token": dev.token }))
    }

    async fn test_state() -> Arc<RemoteState> {
        let dir = tempfile::tempdir().expect("create tempdir");
        // dir 在 state 之后 drop(局部变量逆序 drop)—— pool 存活期内
        // 文件不被删。
        let config = crate::config::RemoteConfig {
            port: 0,
            db_path: dir.path().join("remote.db"),
            shared_secret: "test".into(),
        };
        RemoteState::load(&config).await.expect("state loads")
    }

    /// 预置一个 node + 一个未吊销设备,返回 (state, token, node_id)。
    async fn seed_device() -> (Arc<RemoteState>, String, String) {
        let state = test_state().await;
        db::crud::upsert_node(&state.db, "pc-1", "公司 PC").await.unwrap();
        let token = "a".repeat(64);
        db::crud::insert_device(&state.db, &token, "pc-1", Some("test phone"))
            .await
            .unwrap();
        (state, token, "pc-1".to_string())
    }

    fn build_app(state: Arc<RemoteState>) -> Router {
        Router::new()
            .route("/test", get(echo_device))
            .route_layer(middleware::from_fn_with_state(
                state.clone(),
                require_device_token,
            ))
            .with_state(state)
    }

    // ---- verify_shared_secret ----

    #[test]
    fn secret_match_and_mismatch() {
        assert!(verify_shared_secret("s3cret", "s3cret"));
        assert!(!verify_shared_secret("wrong", "s3cret"));
        assert!(!verify_shared_secret("s3cret", "wrong"));
        // 长度不同 → false(ct_eq 对不等长返回 false)
        assert!(!verify_shared_secret("short", "a-very-long-secret"));
        // 空串:expected 空(配置缺失场景,防 `verify("", "") == true`
        // 的意外放行 —— 但 main 已强制 secret 非空,Q-S1)
        assert!(verify_shared_secret("", ""));
        assert!(!verify_shared_secret("x", ""));
    }

    // ---- device_token_from_request ----

    fn req_with(headers: Option<(&'static str, &'static str)>, query: Option<&'static str>) -> (HeaderMap, Uri) {
        let mut h = HeaderMap::new();
        if let Some((k, v)) = headers {
            h.insert(k, v.parse().unwrap());
        }
        let uri: Uri = match query {
            Some(q) => format!("/test?{q}").parse().unwrap(),
            None => "/test".parse().unwrap(),
        };
        (h, uri)
    }

    #[test]
    fn token_from_header_bearer() {
        let (h, u) = req_with(Some(("authorization", "Bearer abc123")), None);
        assert_eq!(device_token_from_request(&h, &u).as_deref(), Some("abc123"));
    }

    #[test]
    fn token_from_query_access_token() {
        let (h, u) = req_with(None, Some("access_token=xyz789"));
        assert_eq!(device_token_from_request(&h, &u).as_deref(), Some("xyz789"));
    }

    #[test]
    fn header_wins_over_query() {
        let (h, u) = req_with(Some(("authorization", "Bearer from-header")), Some("access_token=from-query"));
        assert_eq!(
            device_token_from_request(&h, &u).as_deref(),
            Some("from-header")
        );
    }

    #[test]
    fn token_missing_is_none() {
        let (h, u) = req_with(None, None);
        assert_eq!(device_token_from_request(&h, &u), None);
        // query 有别的参数但没有 access_token
        let (h, u) = req_with(None, Some("foo=1&bar=2"));
        assert_eq!(device_token_from_request(&h, &u), None);
        // header 有但不是 Bearer(如 Basic)
        let (h, u) = req_with(Some(("authorization", "Basic dXNlcjpwYXNz")), None);
        assert_eq!(device_token_from_request(&h, &u), None);
    }

    #[test]
    fn empty_token_values_are_none() {
        let (h, u) = req_with(Some(("authorization", "Bearer ")), None);
        assert_eq!(device_token_from_request(&h, &u), None);
        let (h, u) = req_with(None, Some("access_token="));
        assert_eq!(device_token_from_request(&h, &u), None);
    }

    // ---- require_device_token 中间件(oneshot)----

    /// 有效 header token → 200 + extension 注入正确(node_id/token 回显)。
    #[tokio::test]
    async fn middleware_allows_valid_header_token() {
        let (state, token, node_id) = seed_device().await;
        let app = build_app(state);
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .method(Method::GET)
                    .uri("/test")
                    .header("authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(res.into_body(), 1024).await.unwrap(),
        )
        .unwrap();
        assert_eq!(body["node_id"], node_id);
        assert_eq!(body["token"], token);
    }

    /// EventSource 场景:query token(P1-2)→ 200。
    #[tokio::test]
    async fn middleware_allows_query_token() {
        let (state, token, node_id) = seed_device().await;
        let app = build_app(state);
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .method(Method::GET)
                    .uri(format!("/test?access_token={token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(res.into_body(), 1024).await.unwrap(),
        )
        .unwrap();
        assert_eq!(body["node_id"], node_id);
    }

    /// 无效 token → 401 Auth + 统一 wire shape。
    #[tokio::test]
    async fn middleware_rejects_unknown_token() {
        let (state, _token, _) = seed_device().await;
        let app = build_app(state);
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .method(Method::GET)
                    .uri("/test")
                    .header("authorization", "Bearer b".repeat(64))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(res.into_body(), 1024).await.unwrap(),
        )
        .unwrap();
        assert_eq!(body["category"], "Auth");
    }

    /// 缺 token → 401。
    #[tokio::test]
    async fn middleware_rejects_missing_token() {
        let (state, _, _) = seed_device().await;
        let app = build_app(state);
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .method(Method::GET)
                    .uri("/test")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    /// 吊销 token → 401(CRUD 层不过滤 revoked,语义在中间件 ——
    /// design-step3.md §10 契约)。
    #[tokio::test]
    async fn middleware_rejects_revoked_token() {
        let (state, token, _) = seed_device().await;
        sqlx::query("UPDATE devices SET revoked = 1 WHERE token = ?")
            .bind(&token)
            .execute(&state.db)
            .await
            .unwrap();
        let app = build_app(state);
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .method(Method::GET)
                    .uri("/test")
                    .header("authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(res.into_body(), 1024).await.unwrap(),
        )
        .unwrap();
        assert_eq!(body["category"], "Auth");
    }
}
