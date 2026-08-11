//! 手机反向代理骨架(design §2.2 / implement.md Step 6,非流式)。
//!
//! `* /api/v1/proxy/{*path}`(catch-all,`require_device_token` middleware):
//!
//! ```text
//! 手机 HTTP → auth 中间件(device_token → node_id)
//!   → tunnel_registry.get(node_id)(离线 → 502 node_offline)
//!   → request_id + pending.insert(id, Oneshot(tx))
//!   → Frame::Request{ method, path(剥 proxy 前缀 + access_token),
//!                     headers(剥 Authorization/Host/Content-Length),
//!                     body }
//!   → conn.send_frame → timeout(60s, P2-2)等 Response 帧
//!   → 解 frame 构造 HTTP 响应返手机
//! ```
//!
//! **Stream 帧(SSE 桥接)留 S3** —— 本模块只做非流式;`PendingReply::Stream`
//! 分支不实例化,为 S3 预留。
//!
//! 剥离契约(P2-1 / Q-T2 / P1-2,跨任务,S2 dispatcher 原样透传剩余):
//! - `Authorization` header:已被 remote 消费,device_token 不流向 PC
//! - `Host` / `Content-Length`:reqwest(S2)自己管理,带了反而冲突
//! - query `access_token`:EventSource 无法设 header 的认证通道,同样
//!   不流向 PC

use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{OriginalUri, Path, State};
use axum::http::header::{HeaderName, HeaderValue};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Extension, Router};
use everlasting_remote_protocol::Frame;
use tokio::sync::oneshot;

use crate::auth::AuthenticatedDevice;
use crate::config::RemoteState;
use crate::error::AppError;
use crate::pending::PendingReply;

/// 单条在途请求的等待超时(design §3.2.1 P2-2:60s,超时 502 Network)。
pub const PENDING_TIMEOUT: Duration = Duration::from_secs(60);

/// 挂载 `* /api/v1/proxy/*path` + device_token 中间件(axum 官方
/// from_fn_with_state 模式,见 auth.rs)。catch-all 语法注意:axum
/// 0.7(matchit 0.7)是 `*path`(无花括号),`{*path}` 是 0.8 语法。
pub fn router(state: Arc<RemoteState>) -> Router {
    Router::new()
        .route("/api/v1/proxy/*path", any(proxy_handler))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            crate::auth::require_device_token,
        ))
        .with_state(state)
}

/// 非流式反向代理 handler。
///
/// `OriginalUri` 而非 `Uri`:middleware 不改写 URI,两者等价,但
/// `OriginalUri` 明示"这是手机原始请求"(含 query —— `Path` 捕获值
/// 不含 query)。
pub async fn proxy_handler(
    State(state): State<Arc<RemoteState>>,
    Extension(device): Extension<AuthenticatedDevice>,
    OriginalUri(uri): OriginalUri,
    Path(path): Path<String>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    // 1. 找 node 的隧道连接;离线 → 502(design §2.2 node_offline)
    let conn = state
        .node_connections
        .get(&device.node_id)
        .ok_or_else(|| AppError::network("node_offline"))?;

    // 2. request_id + pending 占位(Oneshot;conn_id 供 S3 离线清理
    //    按连接精确匹配,P1-2)
    let id = state.pending.next_id();
    let (tx, rx) = oneshot::channel();
    state.pending.insert(id, conn.conn_id, PendingReply::Oneshot(tx));

    // 3. 构造 Request 帧(path 剥 token / headers 剥认证,P2-1)
    let frame = Frame::Request {
        id,
        method: method.to_string(),
        path: frame_path(&uri, &path),
        headers: forward_headers(&headers),
        body: body.to_vec(),
    };

    // 4. 发送;写失败 = 连接已死 → 清理 pending + 502
    if let Err(e) = conn.send_frame(&frame).await {
        state.pending.remove(id);
        return Err(AppError::network(format!("tunnel send failed: {e}")));
    }

    // 5. 等 Response 帧(60s 超时,超时 502 —— 等待方负责 remove,
    //    PendingTable 无泄漏)
    match tokio::time::timeout(state.pending.timeout, rx).await {
        Ok(Ok(Frame::Response { status, headers, body, .. })) => {
            state.pending.remove(id);
            Ok(build_http_response(status, headers, body))
        }
        Ok(Ok(other)) => {
            // 非流式请求收到非 Response 帧 = 协议异常(S3 的 Stream
            // 帧只会出现在流式请求的 pending 上)
            state.pending.remove(id);
            tracing::warn!(id, kind = ?other, "unexpected frame kind for non-streaming request");
            Err(AppError::server("unexpected frame kind from PC tunnel"))
        }
        Ok(Err(_recv_err)) => {
            // Sender drop:PC 连接断开且 pending 未被清理(60s 超时兜底
            // 场景之一);也有可能是 remove 后 send 的竞态 —— 都按
            // 网络错误处理
            state.pending.remove(id);
            Err(AppError::network("tunnel closed while awaiting response"))
        }
        Err(_elapsed) => {
            state.pending.remove(id);
            Err(AppError::network("request timed out"))
        }
    }
}

/// Request 帧的 path:`*path` 捕获值(已剥 `/api/v1/proxy` 前缀)+
/// 原始 query 剥掉 `access_token`(P1-2)。注意 axum 0.7 的 catch-all
/// 捕获值**不带前导 `/`**,而 PC 侧 reqwest 拼 URL 需要完整 path ——
/// 统一补 `/`;空捕获(打裸 `/api/v1/proxy`)归一为 `/`。
fn frame_path(uri: &axum::http::Uri, path: &str) -> String {
    let base = if path.is_empty() {
        "/".to_string()
    } else if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    let Some(query) = uri.query() else {
        return base.to_string();
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|kv| !kv.starts_with("access_token="))
        .collect();
    if kept.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", kept.join("&"))
    }
}

/// Request 帧 headers:剥 `Authorization`(P2-1)+ `Host` / `Content-Length`
/// (reqwest 重算);非 UTF-8 值跳过(罕见,不 fail 请求)。
fn forward_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .filter(|(name, _)| {
            // name: &&HeaderName(iter + filter 双引用),as_str() auto-deref
            !matches!(
                name.as_str(),
                "authorization" | "host" | "content-length"
            )
        })
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|v| (name.as_str().to_string(), v.to_string()))
        })
        .collect()
}

/// Response 帧 → HTTP 响应。非法 header 宽容跳过(不 fail 整个响应,
/// 保持隧道韧性);builder 失败(不可能发生,status 合法)兜底 502。
fn build_http_response(status: u16, headers: Vec<(String, String)>, body: Vec<u8>) -> Response {
    let mut builder = Response::builder().status(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY));
    for (k, v) in headers {
        match (HeaderName::try_from(k), HeaderValue::try_from(v)) {
            (Ok(name), Ok(value)) => {
                builder = builder.header(name, value);
            }
            _ => tracing::debug!("skip invalid header in tunneled response"),
        }
    }
    builder
        .body(axum::body::Body::from(body))
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "failed to build tunneled response");
            AppError::network("invalid response from PC tunnel").into_response()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RemoteConfig;
    use crate::db::pool;
    use crate::db::schema;
    use crate::pending::PendingTable;
    use crate::tunnel_registry::HeartbeatConfig;
    use axum::http::StatusCode as HttpStatus;
    use futures_util::{SinkExt, StreamExt};
    use std::net::SocketAddr;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message as ClientMessage;
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

    type ClientWs = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

    /// 起 server(tempdir db + 自定义心跳/pending 超时)。tempdir 故意
    /// leak —— pool 连接还开着,测试期间目录必须活着。
    async fn start_server(
        heartbeat: HeartbeatConfig,
        pending_timeout: Duration,
    ) -> (u16, Arc<RemoteState>) {
        let dir = Box::leak(Box::new(tempfile::tempdir().expect("tempdir")));
        let config = RemoteConfig {
            port: 0,
            db_path: dir.path().join("remote.db"),
            shared_secret: "test".into(),
        };
        let pool = pool::init_pool(&config.db_path).await.expect("init pool");
        schema::run_migrations(&pool).await.expect("migrations");
        let state = Arc::new(RemoteState {
            db: pool,
            shared_secret: config.shared_secret,
            node_connections: Arc::new(crate::tunnel_registry::TunnelRegistry::new()),
            heartbeat,
            pending: Arc::new(PendingTable::new(pending_timeout)),
            pairing_ratelimit: Arc::new(crate::ratelimit::RateLimiter::new(
                1000,
                Duration::from_secs(60),
            )),
        });
        let router = crate::server::build_router(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("local_addr").port();
        tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .expect("serve");
        });
        (port, state)
    }

    async fn connect(port: u16, query: &str) -> Result<ClientWs, tokio_tungstenite::tungstenite::Error> {
        let url = format!("ws://127.0.0.1:{port}/ws{query}");
        tokio_tungstenite::connect_async(url).await.map(|(ws, _)| ws)
    }

    /// 预置 node + device,返回 (token, node_id)。
    async fn seed_device(state: &RemoteState) -> (String, String) {
        crate::db::crud::upsert_node(&state.db, "pc-1", "公司 PC")
            .await
            .expect("upsert node");
        let token = "a".repeat(64);
        crate::db::crud::insert_device(&state.db, &token, "pc-1", Some("test phone"))
            .await
            .expect("insert device");
        (token, "pc-1".to_string())
    }

    /// 手机侧 HTTP 请求 helper。走 `build_router` 的 oneshot(router
    /// 是 Clone 的)—— 与 spawn 的 server 共享同一 state,免 reqwest
    /// 依赖,全链路(middleware + handler)照常执行。
    async fn phone_request(
        router: axum::Router,
        method: &str,
        uri: &str,
        token: Option<&str>,
    ) -> (HttpStatus, String) {
        use tower::ServiceExt;
        let mut req = axum::http::Request::builder()
            .method(method)
            .uri(uri)
            .body(axum::body::Body::empty())
            .expect("build request");
        if let Some(t) = token {
            req.headers_mut()
                .insert("authorization", format!("Bearer {t}").parse().expect("valid header"));
        }
        let resp = router.oneshot(req).await.expect("oneshot");
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .expect("read body");
        (status, String::from_utf8_lossy(&bytes).to_string())
    }

    // ---- 认证边界 ----

    #[tokio::test]
    async fn proxy_requires_device_token() {
        let (_port, state) = start_server(HeartbeatConfig::default(), PENDING_TIMEOUT).await;
        let router = crate::server::build_router(state.clone());
        let (status, body) = phone_request(router, "GET", "/api/v1/proxy/api/v1/health", None).await;
        assert_eq!(status, HttpStatus::UNAUTHORIZED);
        assert!(body.contains("\"category\":\"Auth\""), "got {body}");
    }

    // ---- 离线 502 ----

    /// device 绑定 node 但 WSS 未连(或已断)→ 502 node_offline。
    #[tokio::test]
    async fn node_offline_returns_502() {
        let (_port, state) = start_server(HeartbeatConfig::default(), PENDING_TIMEOUT).await;
        let (token, _node_id) = seed_device(&state).await;
        let router = crate::server::build_router(state.clone());
        let (status, body) = phone_request(router, "GET", "/api/v1/proxy/api/v1/health", Some(&token)).await;
        assert_eq!(status, HttpStatus::BAD_GATEWAY);
        assert!(body.contains("\"category\":\"Network\""), "got {body}");
        assert!(body.contains("node_offline"), "got {body}");
    }

    // ---- 完整转发 round-trip ----

    /// 真实链路:手机 POST → remote → WSS Request 帧 → 模拟 PC 回
    /// Response 帧 → 手机拿 200 + body。同时断言帧内容契约:
    /// path 剥了 proxy 前缀、无 Authorization。
    #[tokio::test]
    async fn forward_request_and_reply_roundtrip() {
        let (port, state) = start_server(HeartbeatConfig::default(), PENDING_TIMEOUT).await;
        let (token, _) = seed_device(&state).await;
        let mut pc = connect(port, "?secret=test&node_id=pc-1").await.expect("pc connect");

        // 手机发起请求(oneshot router 与 spawn 的 server 共享 state)
        let phone_state = state.clone();
        let phone = tokio::spawn(async move {
            let router = crate::server::build_router(phone_state);
            phone_request(
                router,
                "POST",
                "/api/v1/proxy/api/v1/sessions/list",
                Some(&token),
            )
            .await
        });

        // PC 侧收 Request 帧
        let frame = tokio::time::timeout(Duration::from_secs(2), pc.next())
            .await
            .expect("pc should receive request frame")
            .expect("message")
            .expect("no error");
        let ClientMessage::Text(text) = frame else {
            panic!("expected Text frame, got {frame:?}");
        };
        let received: Frame = serde_json::from_str(&text).expect("parse frame");
        let Frame::Request {
            id, method, path, headers, body,
        } = received
        else {
            panic!("expected Request frame");
        };
        assert_eq!(method, "POST");
        assert_eq!(path, "/api/v1/sessions/list");
        assert!(body.is_empty(), "oneshot 请求 body 为空: got {body:?}");
        assert!(
            !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("authorization")),
            "Authorization 必须被剥掉: {headers:?}"
        );

        // PC 回 Response 帧
        let reply = Frame::Response {
            id,
            status: 200,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: br#"{"sessions":[]}"#.to_vec(),
        };
        pc.send(ClientMessage::Text(serde_json::to_string(&reply).unwrap()))
            .await
            .expect("send reply");

        // 手机拿响应
        let (status, body) = phone.await.expect("phone request done");
        assert_eq!(status, HttpStatus::OK);
        assert_eq!(body, r#"{"sessions":[]}"#);
    }

    /// access_token query 被剥(P1-2),其余 query 保留 —— PC 收到的
    /// path 不含手机 token。
    #[tokio::test]
    async fn access_token_stripped_from_query() {
        let (port, state) = start_server(HeartbeatConfig::default(), PENDING_TIMEOUT).await;
        let (token, _) = seed_device(&state).await;
        let mut pc = connect(port, "?secret=test&node_id=pc-1").await.expect("pc connect");

        let phone_state = state.clone();
        let phone = tokio::spawn(async move {
            let router = crate::server::build_router(phone_state);
            // query token,无 header(EventSource 场景)
            phone_request(
                router,
                "GET",
                &format!("/api/v1/proxy/api/v1/stream?access_token={token}&foo=1"),
                None,
            )
            .await
        });

        let frame = tokio::time::timeout(Duration::from_secs(2), pc.next())
            .await
            .expect("request frame")
            .expect("message")
            .expect("no error");
        let ClientMessage::Text(text) = frame else { panic!("expected text") };
        let Frame::Request { id, path, .. } = serde_json::from_str(&text).expect("parse") else {
            panic!("expected Request");
        };
        assert_eq!(path, "/api/v1/stream?foo=1");

        // PC 回一个非流式响应收尾(防 phone task 悬挂)
        pc.send(ClientMessage::Text(
            serde_json::to_string(&Frame::Response {
                id,
                status: 200,
                headers: vec![],
                body: vec![],
            })
            .unwrap(),
        ))
        .await
        .expect("reply");
        let _ = phone.await.expect("phone done");
    }

    // ---- 超时 502 ----

    /// PC 在线但不回 Response → pending 超时(小 timeout)→ 502 Network。
    #[tokio::test]
    async fn pending_timeout_returns_502() {
        let (port, state) = start_server(
            HeartbeatConfig::default(),
            Duration::from_millis(150), // pending 超时 150ms
        )
        .await;
        let (token, _) = seed_device(&state).await;
        let mut pc = connect(port, "?secret=test&node_id=pc-1").await.expect("pc connect");

        let router = crate::server::build_router(state.clone());
        let (status, body) =
            phone_request(router, "GET", "/api/v1/proxy/api/v1/health", Some(&token)).await;
        assert_eq!(status, HttpStatus::BAD_GATEWAY);
        assert!(body.contains("\"category\":\"Network\""), "got {body}");

        // pending 已清理,无泄漏
        assert!(state.pending.is_empty());
        // PC 侧确实收到了帧(只是不回)
        let _ = tokio::time::timeout(Duration::from_secs(2), pc.next())
            .await
            .expect("request frame received");
    }
}
