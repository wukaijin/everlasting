//! 手机反向代理(design §2.2 / implement.md Step 6 非流式 + S3 流式分支)。
//!
//! `* /api/v1/proxy/{*path}`(catch-all,`require_device_token` middleware):
//!
//! ```text
//! 手机 HTTP → auth 中间件(device_token → node_id)
//!   → tunnel_registry.get(node_id)(离线 → 502 node_offline)
//!   → request_id + pending.insert(id, conn_id, reply)
//!   → Frame::Request{ method, path(剥 proxy 前缀 + access_token),
//!                     headers(剥 Authorization/Host/Content-Length),
//!                     body }
//!   → conn.send_frame
//!   → 非流式:timeout(60s, P2-2)等 Response 帧 → 解帧构 HTTP 响应
//!   → 流式(Accept 含 text/event-stream):mpsc + Body::from_stream
//!     裸字节透传(design §2.1,S3 实现)
//! ```
//!
//! **SSE 识别按 Accept header 而非 path**(design §3.1 权衡):EventSource
//! 原生带 `Accept: text/event-stream`,无需前端配合;path 判定耦合 PC
//! 路由,proxy 应通用。
//!
//! **流式分支返回**:`Body::from_stream`(裸字节,非 axum `Sse<Event>`)
//! —— `StreamEvent::Chunk` 已是 SSE body 原文,axum `Sse` 会要求 Event
//! 帧化(重新构造 `id:/event:/data:`),破坏透传契约。
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
use axum::http::header::{HeaderName, HeaderValue, ACCEPT};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Extension, Router};
use everlasting_remote_protocol::{Frame, StreamEvent};
use futures_util::stream::{once, unfold};
use futures_util::StreamExt;
use tokio::sync::{mpsc, oneshot};

use crate::auth::AuthenticatedDevice;
use crate::config::RemoteState;
use crate::error::AppError;
use crate::pending::PendingReply;
use crate::tunnel_registry::ConnHandle;

/// 单条在途请求的等待超时(design §3.2.1 P2-2:60s,超时 502 Network)。
pub const PENDING_TIMEOUT: Duration = Duration::from_secs(60);

/// SSE 流式分支:手机 body 与 ws 接收循环之间的 mpsc 容量。
///
/// **仅限内存,非端到端背压**(P2-1 修订):慢手机不是被背压而是被
/// **剔除**(dispatch_frame `try_send` 满 → 发 End 给 PC 停转发,
/// 手机 EventSource 重连靠 Last-Event-ID 回放补),与 daemon 侧
/// `SseRegistry` 的慢订阅者剔除语义对齐。
const STREAM_CHANNEL_CAPACITY: usize = 128;

/// 流式请求的首帧超时(P2-4):PC 永不回(loopback 挂死 / reqwest 无
/// 超时)时手机 body 不永久悬挂、pending 条目不永久泄漏。首帧之后由
/// PC loopback SSE 的 KeepAlive(30s `:ping`)持续触发,无需流级超时。
pub const STREAM_FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(30);

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

/// 反向代理 handler:按 `Accept` header 分流。
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

    // 2. request_id + Request 帧构造(两种分支共用)
    let id = state.pending.next_id();
    let frame = Frame::Request {
        id,
        method: method.to_string(),
        path: frame_path(&uri, &path),
        headers: forward_headers(&headers),
        body: body.to_vec(),
    };

    // 3. SSE 判定(design §3.1:Accept 含 text/event-stream → 流式分支;
    //    EventSource 原生带此 header,普通请求不会误带)
    let is_sse = headers
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|accept| accept.contains("text/event-stream"))
        .unwrap_or(false);

    if is_sse {
        proxy_stream(&state, &conn, id, &frame).await
    } else {
        proxy_oneshot(&state, &conn, id, &frame).await
    }
}

/// 非流式分支(S1 原样保留,零行为变化):pending Oneshot → 发 Request
/// 帧 → 60s 内等 Response → 构 HTTP 响应。
async fn proxy_oneshot(
    state: &Arc<RemoteState>,
    conn: &Arc<ConnHandle>,
    id: u64,
    frame: &Frame,
) -> Result<Response, AppError> {
    let (tx, rx) = oneshot::channel();
    state
        .pending
        .insert(id, conn.conn_id, PendingReply::Oneshot(tx));

    // 发送;写失败 = 连接已死 → 清理 pending + 502
    if let Err(e) = conn.send_frame(frame).await {
        state.pending.remove(id);
        return Err(AppError::network(format!("tunnel send failed: {e}")));
    }

    // 等 Response 帧(60s 超时,超时 502 —— 等待方负责 remove,
    //   PendingTable 无泄漏)
    match tokio::time::timeout(state.pending.timeout, rx).await {
        Ok(Ok(Frame::Response {
            status,
            headers,
            body,
            ..
        })) => {
            state.pending.remove(id);
            Ok(build_http_response(status, headers, body))
        }
        Ok(Ok(other)) => {
            // 非流式请求收到非 Response 帧 = 协议异常(Stream 帧只会
            // 出现在流式请求的 pending 上)
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

/// 流式分支(S3 核心,design §2.1):mpsc + `Body::from_stream` 裸字节
/// 透传,手机 EventSource 自己按 `\n\n` 解析。
///
/// 生命周期:
/// 1. `pending.insert(id, conn.conn_id, Stream(tx))` —— conn_id 供
///    node 离线按连接精确清理(P1-2)。
/// 2. 发 `Frame::Request` 后**等首帧**(P2-4):30s 内无任何
///    Chunk/End/Error → remove + 504(手机拿到非 200,EventSource
///    fail;S4 前端显式重试)。
/// 3. 返回 200 + 流式 body:首帧(once)+ 后续(rx unfold);`End`/
///    `Error` 结束流(手机 body 关)。
///
/// 慢/断手机:dispatch_frame 的 `try_send` 满(rx 无人消费)时剔除
/// (remove + 发 End 给 PC),本分支的 rx 随 body drop —— 无需额外处理。
async fn proxy_stream(
    state: &Arc<RemoteState>,
    conn: &Arc<ConnHandle>,
    id: u64,
    frame: &Frame,
) -> Result<Response, AppError> {
    let (tx, mut rx) = mpsc::channel::<StreamEvent>(STREAM_CHANNEL_CAPACITY);
    state
        .pending
        .insert(id, conn.conn_id, PendingReply::Stream(tx));

    if let Err(e) = conn.send_frame(frame).await {
        state.pending.remove(id);
        return Err(AppError::network(format!("tunnel send failed: {e}")));
    }

    // 首帧超时(P2-4):手机不悬挂 + pending 不泄漏。
    let first = match tokio::time::timeout(STREAM_FIRST_FRAME_TIMEOUT, rx.recv()).await {
        Ok(Some(ev)) => ev,
        Ok(None) => {
            // Sender drop 且无帧:连接已死/被清理,按网络错误收尾
            state.pending.remove(id);
            return Err(AppError::network("tunnel closed before first stream frame"));
        }
        Err(_) => {
            state.pending.remove(id);
            return Err(AppError::network("first stream frame timed out"));
        }
    };

    let body_stream = once(async move { first })
        .chain(unfold(rx, |mut rx| async move {
            rx.recv().await.map(|ev| (ev, rx))
        }))
        // End / Error 是流的终止事件:送进 body 后结束(手机 body 关)。
        // 条目移除由 ws.rs dispatch_frame 负责(End/Error 分支)。
        .take_while(|ev| {
            futures_util::future::ready(!matches!(ev, StreamEvent::End | StreamEvent::Error { .. }))
        })
        .map(|ev| -> Result<Bytes, std::io::Error> {
            match ev {
                StreamEvent::Chunk { bytes } => Ok(Bytes::from(bytes)),
                StreamEvent::End | StreamEvent::Error { .. } => {
                    unreachable!("take_while 已滤掉终止事件")
                }
            }
        });

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(axum::body::Body::from_stream(body_stream))
        .expect("静态头构造不可能失败"))
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
            !matches!(name.as_str(), "authorization" | "host" | "content-length")
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
    let mut builder =
        Response::builder().status(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY));
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

    async fn connect(
        port: u16,
        query: &str,
    ) -> Result<ClientWs, tokio_tungstenite::tungstenite::Error> {
        let url = format!("ws://127.0.0.1:{port}/ws{query}");
        tokio_tungstenite::connect_async(url)
            .await
            .map(|(ws, _)| ws)
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
            req.headers_mut().insert(
                "authorization",
                format!("Bearer {t}").parse().expect("valid header"),
            );
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
        let (status, body) =
            phone_request(router, "GET", "/api/v1/proxy/api/v1/health", None).await;
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
        let (status, body) =
            phone_request(router, "GET", "/api/v1/proxy/api/v1/health", Some(&token)).await;
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
        let mut pc = connect(port, "?secret=test&node_id=pc-1")
            .await
            .expect("pc connect");

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
            id,
            method,
            path,
            headers,
            body,
        } = received
        else {
            panic!("expected Request frame");
        };
        assert_eq!(method, "POST");
        assert_eq!(path, "/api/v1/sessions/list");
        assert!(body.is_empty(), "oneshot 请求 body 为空: got {body:?}");
        assert!(
            !headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("authorization")),
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
        let mut pc = connect(port, "?secret=test&node_id=pc-1")
            .await
            .expect("pc connect");

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
        let ClientMessage::Text(text) = frame else {
            panic!("expected text")
        };
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
        let mut pc = connect(port, "?secret=test&node_id=pc-1")
            .await
            .expect("pc connect");

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

    // ---- S3:SSE 流式 ----

    /// 手机 SSE 请求(带 `Accept: text/event-stream` + Bearer token),
    /// 返回响应(不读 body —— 调用方决定消费方式)。
    async fn phone_sse_request(state: Arc<RemoteState>, token: &str) -> Response {
        use tower::ServiceExt;
        let router = crate::server::build_router(state);
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/api/v1/proxy/api/v1/stream")
            .header("accept", "text/event-stream")
            .header(
                "authorization",
                format!("Bearer {token}")
                    .parse::<axum::http::header::HeaderValue>()
                    .expect("valid header"),
            )
            .body(axum::body::Body::empty())
            .expect("build request");
        router.oneshot(req).await.expect("oneshot")
    }

    /// 把手机响应 body 读成完整字节(join 所有 data frames)。
    async fn read_body_to_end(body: axum::body::Body) -> Vec<u8> {
        let mut out = Vec::new();
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            out.extend_from_slice(&chunk.expect("body chunk"));
        }
        out
    }

    /// 从 fake PC 读下一帧并解析。
    async fn pc_next_frame(pc: &mut ClientWs) -> Frame {
        let msg = tokio::time::timeout(Duration::from_secs(3), pc.next())
            .await
            .expect("frame timeout")
            .expect("stream ended")
            .expect("ws error");
        let ClientMessage::Text(text) = msg else {
            panic!("expected Text frame, got {msg:?}");
        };
        serde_json::from_str(&text).expect("parse frame")
    }

    /// SSE 流式 round-trip(design §2.1):手机 GET(带 Accept)→ remote
    /// 发 Request 帧 → fake PC 回 Chunk×N+End → 手机 body 逐 chunk 收到,
    /// 拼接还原 = 原文(裸字节透传,不在隧道层解析 SSE 语义)。
    #[tokio::test]
    async fn sse_stream_roundtrip_chunks_to_phone() {
        let (port, state) = start_server(HeartbeatConfig::default(), PENDING_TIMEOUT).await;
        let (token, _) = seed_device(&state).await;
        let mut pc = connect(port, "?secret=test&node_id=pc-1")
            .await
            .expect("pc connect");

        let phone_state = state.clone();
        let phone_token = token.clone();
        let phone = tokio::spawn(async move {
            let resp = phone_sse_request(phone_state, &phone_token).await;
            let status = resp.status();
            let ct = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            let body = read_body_to_end(resp.into_body()).await;
            (status, ct, body)
        });

        // PC 收 Request 帧(path 已剥 proxy 前缀 + access_token)
        let Frame::Request { id, path, .. } = pc_next_frame(&mut pc).await else {
            panic!("expected Request frame");
        };
        assert_eq!(path, "/api/v1/stream");

        // PC 回 Chunk × N + End
        let chunks: Vec<&[u8]> = vec![
            b"id:1\nevent:chat-event\ndata:{\"a\":1}\n\n",
            b"id:2\nevent:chat-event\ndata:{\"b\":2}\n\n",
            b"id:3\nevent:chat-event\ndata:{\"c\":3}\n\n",
        ];
        for c in &chunks {
            pc.send(ClientMessage::Text(
                serde_json::to_string(&Frame::Stream {
                    id,
                    event: StreamEvent::Chunk { bytes: c.to_vec() },
                })
                .unwrap(),
            ))
            .await
            .expect("send chunk");
        }
        pc.send(ClientMessage::Text(
            serde_json::to_string(&Frame::Stream {
                id,
                event: StreamEvent::End,
            })
            .unwrap(),
        ))
        .await
        .expect("send end");

        let (status, ct, joined) = phone.await.expect("phone done");
        assert_eq!(status, HttpStatus::OK);
        assert!(ct.starts_with("text/event-stream"), "ct = {ct}");
        let expected: Vec<u8> = chunks.iter().flat_map(|c| c.iter().copied()).collect();
        assert_eq!(joined, expected, "裸字节透传:拼接必须还原 SSE 原文");
        assert!(state.pending.is_empty(), "End 后条目应清理");
    }

    /// 手机断开(读 2 chunk 后 drop body)→ remote 在下一个 Chunk 的
    /// `try_send` 失败时感知 → 发 `Frame::Stream{End}` 给 PC(取消信号,
    /// D3)—— PC 停转发的前提。
    #[tokio::test]
    async fn phone_disconnect_sends_end_to_pc() {
        let (port, state) = start_server(HeartbeatConfig::default(), PENDING_TIMEOUT).await;
        let (token, _) = seed_device(&state).await;
        let mut pc = connect(port, "?secret=test&node_id=pc-1")
            .await
            .expect("pc connect");

        // 手机:读 2 个 chunk 后丢弃 body(模拟断网/关页面)
        let phone_state = state.clone();
        let phone_token = token.clone();
        let phone = tokio::spawn(async move {
            let resp = phone_sse_request(phone_state, &phone_token).await;
            let mut stream = resp.into_body().into_data_stream();
            let _ = stream.next().await.expect("chunk 1");
            let _ = stream.next().await.expect("chunk 2");
            drop(stream); // 手机断
        });

        // PC 收 Request 帧
        let Frame::Request { id, .. } = pc_next_frame(&mut pc).await else {
            panic!("expected Request frame");
        };

        // PC 持续发 chunk(20ms 间隔;手机读完 2 个后已断,remote 的
        // 下一次 try_send 失败 → 剔除 + 发 End)
        for i in 0..10u32 {
            pc.send(ClientMessage::Text(
                serde_json::to_string(&Frame::Stream {
                    id,
                    event: StreamEvent::Chunk {
                        bytes: format!("data:{i}\n\n").into_bytes(),
                    },
                })
                .unwrap(),
            ))
            .await
            .expect("send chunk");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        // 手机断 → PC 应收到 End(取消信号)
        let Frame::Stream { id: end_id, event } = pc_next_frame(&mut pc).await else {
            panic!("expected Stream frame");
        };
        assert_eq!(end_id, id);
        assert!(
            matches!(event, StreamEvent::End),
            "手机断应触发 End,got {event:?}"
        );

        phone.await.expect("phone done");
        assert!(state.pending.is_empty(), "剔除后条目应清理");
    }

    /// 慢手机剔除(P1-1):手机不读 body → mpsc(128) 满 → try_send 失败
    /// → 剔除 + 发 End;**接收循环不被阻塞** —— 同连接上后续的配对 RPC
    /// 仍即时响应(若 Stream 分支 `await send`,接收循环会被满 mpsc 永久
    /// 拖住,RPC 必然超时)。
    #[tokio::test]
    async fn slow_phone_evicted_without_blocking_receive_loop() {
        let (port, state) = start_server(HeartbeatConfig::default(), PENDING_TIMEOUT).await;
        let (token, _) = seed_device(&state).await;
        let mut pc = connect(port, "?secret=test&node_id=pc-1")
            .await
            .expect("pc connect");
        // 等 node 注册(配对码落库绑定 node_id)
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            if crate::db::crud::get_node(&state.db, "pc-1")
                .await
                .expect("get_node")
                .is_some()
            {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline, "node 未注册");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        // 手机:发 SSE 请求但**不读 body**(慢手机,永不消费 mpsc)
        let phone_state = state.clone();
        let phone_token = token.clone();
        let phone = tokio::spawn(async move {
            let resp = phone_sse_request(phone_state, &phone_token).await;
            (resp.status(), resp.into_body()) // 持有 body 但不 poll
        });

        // PC 收 Request 帧
        let Frame::Request { id, .. } = pc_next_frame(&mut pc).await else {
            panic!("expected Request frame");
        };

        // 快速灌 300 个 chunk(远超 mpsc(128) 容量,必然触发剔除)
        for i in 0..300u32 {
            pc.send(ClientMessage::Text(
                serde_json::to_string(&Frame::Stream {
                    id,
                    event: StreamEvent::Chunk {
                        bytes: format!("data:{i}\n\n").into_bytes(),
                    },
                })
                .unwrap(),
            ))
            .await
            .expect("send chunk");
        }

        // 慢手机被剔除 → PC 收到 End(取消信号)
        let Frame::Stream { id: end_id, event } = pc_next_frame(&mut pc).await else {
            panic!("expected Stream frame");
        };
        assert_eq!(end_id, id);
        assert!(
            matches!(event, StreamEvent::End),
            "剔除应发 End,got {event:?}"
        );

        // P1-1 核心断言:剔除后接收循环未停摆,配对 RPC 即时响应
        pc.send(ClientMessage::Text(
            serde_json::to_string(&Frame::Request {
                id: 999,
                method: "POST".into(),
                path: "/internal/pairing/generate".into(),
                headers: vec![],
                body: vec![],
            })
            .unwrap(),
        ))
        .await
        .expect("send pairing rpc");
        let resp = tokio::time::timeout(Duration::from_secs(2), pc.next())
            .await
            .expect("pairing RPC 必须即时响应(接收循环未被慢流拖住)")
            .expect("stream alive")
            .expect("ws ok");
        let ClientMessage::Text(text) = resp else {
            panic!("expected text frame");
        };
        let Frame::Response {
            id: rpc_id, status, ..
        } = serde_json::from_str(&text).expect("parse")
        else {
            panic!("expected Response frame");
        };
        assert_eq!(rpc_id, 999);
        assert_eq!(status, 200);

        // 手机 body 最终读到缓冲的 chunk 后结束(不悬挂,条目已清)
        let (status, body) = phone.await.expect("phone done");
        assert_eq!(status, HttpStatus::OK);
        let read = tokio::time::timeout(Duration::from_secs(3), read_body_to_end(body))
            .await
            .expect("body 必须在剔除后结束(不悬挂)");
        assert!(!read.is_empty(), "应至少读到剔除前缓冲的 chunk");
        assert!(state.pending.is_empty());
    }

    /// node 离线(PC 断开)→ 离线清理按 conn_id 取消在途流(§2.3):
    /// 手机 body 收到 `Error{node_offline}` → 关闭,不悬挂。
    #[tokio::test]
    async fn node_offline_closes_phone_stream() {
        let (port, state) = start_server(HeartbeatConfig::default(), PENDING_TIMEOUT).await;
        let (token, _) = seed_device(&state).await;
        let mut pc = connect(port, "?secret=test&node_id=pc-1")
            .await
            .expect("pc connect");

        let phone_state = state.clone();
        let phone_token = token.clone();
        let phone = tokio::spawn(async move {
            let resp = phone_sse_request(phone_state, &phone_token).await;
            read_body_to_end(resp.into_body()).await
        });

        // PC 收 Request,回 2 个 chunk(流在途)
        let Frame::Request { id, .. } = pc_next_frame(&mut pc).await else {
            panic!("expected Request frame");
        };
        for c in [b"data:a\n\n".as_slice(), b"data:b\n\n".as_slice()] {
            pc.send(ClientMessage::Text(
                serde_json::to_string(&Frame::Stream {
                    id,
                    event: StreamEvent::Chunk { bytes: c.to_vec() },
                })
                .unwrap(),
            ))
            .await
            .expect("send chunk");
        }

        // PC 断开(clean disconnect → 接收循环退出 → 离线清理)
        let _ = pc.close(None).await;

        // 手机 body 收 Error{node_offline} → 关闭(不悬挂)
        let body = tokio::time::timeout(Duration::from_secs(3), phone)
            .await
            .expect("body 必须在 node 离线后关闭(不悬挂)")
            .expect("phone done");
        assert_eq!(body, b"data:a\n\ndata:b\n\n");
        assert!(state.pending.is_empty(), "离线清理应清空在途流");
    }

    /// P2-3:流式请求收到非流式 Response(PC loopback 回 404 等)
    /// → 转 `Error{status}` 送 body 关闭 + 条目清理;手机拿到 200 +
    /// 空 body 立即关闭(HTTP 状态码已发出无法改,错误经 body 关闭传达)。
    #[tokio::test]
    async fn non_stream_response_to_stream_request_closes_phone() {
        let (port, state) = start_server(HeartbeatConfig::default(), PENDING_TIMEOUT).await;
        let (token, _) = seed_device(&state).await;
        let mut pc = connect(port, "?secret=test&node_id=pc-1")
            .await
            .expect("pc connect");

        let phone_state = state.clone();
        let phone_token = token.clone();
        let phone = tokio::spawn(async move {
            let resp = phone_sse_request(phone_state, &phone_token).await;
            read_body_to_end(resp.into_body()).await
        });

        // PC 收 Request 帧
        let Frame::Request { id, .. } = pc_next_frame(&mut pc).await else {
            panic!("expected Request frame");
        };

        // PC 对 SSE 请求回非流式 Response(404)
        pc.send(ClientMessage::Text(
            serde_json::to_string(&Frame::Response {
                id,
                status: 404,
                headers: vec![],
                body: b"not found".to_vec(),
            })
            .unwrap(),
        ))
        .await
        .expect("send response");

        let body = tokio::time::timeout(Duration::from_secs(3), phone)
            .await
            .expect("body 必须关闭(不悬挂)")
            .expect("phone done");
        assert!(body.is_empty(), "错误经 body 关闭传达,无内容");
        assert!(state.pending.is_empty(), "P2-3 分支应清理条目");
    }
}
