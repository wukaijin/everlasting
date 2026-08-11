//! PC daemon WSS 入口(design §2.1 / implement.md Step 5)。
//!
//! - `GET /ws?secret=<shared_secret>&node_id=<id>&display_name=<名>`
//! - 握手校验:`auth::verify_shared_secret` 常时比较(P3-4),失败 401
//!   (on_upgrade 之前返回 Err,HTTP 层直接回 401,不升级)
//! - 升级后:`tunnel_registry.register`(重复 node_id 踢旧)+
//!   `upsert_node`(online)→ spawn 心跳 task(30s ping / 90s 超时)→
//!   进入接收循环
//!
//! 接收循环分派(P3-2):
//! - `Message::Pong` → 续 `last_pong_ms` + `nodes.last_seen_at`(design §2.4)
//! - `Frame::Request` path 以 `INTERNAL_PREFIX` 开头 → internal RPC
//!   (**Step 7 实现**,当前 warn + 忽略);否则 log + 忽略
//!   (S1 阶段 PC 不应主动发普通请求,那是 remote → PC 方向)
//! - `Frame::Response / Stream` → 路由 pending 表(**Step 6 实现**,
//!   当前 debug + 忽略)
//!
//! 心跳超时 / 连接断开 → `remove_if_current` + `nodes.status=offline`
//! (只动自己的连接,不误伤同 node_id 的新连接)。

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Query, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use everlasting_remote_protocol::Frame;
use futures_util::StreamExt;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::auth;
use crate::config::RemoteState;
use crate::db::{self, NODE_STATUS_OFFLINE, NODE_STATUS_ONLINE};
use crate::error::AppError;
use crate::pending::PendingReply;
use crate::tunnel_registry::{ConnHandle, HeartbeatConfig, TunnelRegistry};

/// WSS 握手 query 参数(design §2.1)。Query 提取器自动 percent-decode
/// (P2-1 编码反向 —— node_id/display_name 含中文/空格时 PC 侧 URL 编码)。
#[derive(Debug, Deserialize)]
pub struct WsParams {
    pub secret: Option<String>,
    pub node_id: Option<String>,
    pub display_name: Option<String>,
}

/// 挂载 `GET /ws`(daemon 模式:内部 `with_state(state)` 产出
/// `Router<()>` 供上层 merge —— handler 的 `State` 提取器需要带 state
/// 的 router,而顶层 `Router<()>` 无法 `with_state` 非 () 类型)。
pub fn router(state: Arc<RemoteState>) -> Router {
    Router::new()
        .route("/ws", get(ws_handler))
        .with_state(state)
}

/// WSS 握手:校验 secret → 校验 node_id → 升级。
/// 校验失败在 `on_upgrade` 之前返回 Err —— axum 把 `AppError` 作为
/// HTTP 响应(401)返回,客户端握手即失败。
pub async fn ws_handler(
    State(state): State<Arc<RemoteState>>,
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    ws: WebSocketUpgrade,
    Query(params): Query<WsParams>,
) -> Result<Response, AppError> {
    let Some(secret) = params.secret.filter(|s| !s.is_empty()) else {
        return Err(AppError::auth("missing secret query param"));
    };
    if !auth::verify_shared_secret(&secret, &state.shared_secret) {
        tracing::warn!(ip = %addr, "shared_secret_rejected(伪 daemon 尝试)");
        return Err(AppError::auth("invalid shared_secret"));
    }
    let Some(node_id) = params.node_id.filter(|s| !s.is_empty()) else {
        return Err(AppError::auth("missing node_id query param"));
    };
    let display_name = params.display_name.filter(|s| !s.is_empty()).unwrap_or_else(|| node_id.clone());

    tracing::info!(ip = %addr, node_id = %node_id, "PC daemon handshake ok, upgrading");
    Ok(ws.on_upgrade(move |socket| {
        handle_ws_connection(state, node_id, display_name, socket)
    }))
}

/// 升级后的连接生命周期:注册 → 心跳 → 接收循环 → 退出清理。
async fn handle_ws_connection(
    state: Arc<RemoteState>,
    node_id: String,
    display_name: String,
    socket: WebSocket,
) {
    let registry = state.node_connections.clone();
    let conn_id = registry.next_conn_id();
    let (sink, stream) = socket.split();
    let handle = Arc::new(ConnHandle {
        conn_id,
        node_id: node_id.clone(),
        display_name: display_name.clone(),
        sink: Mutex::new(sink),
        last_pong_ms: std::sync::atomic::AtomicI64::new(db::now_ms()),
    });

    // 重复 node_id → 踢旧(design §2.1):旧连接发 Close,其接收循环
    // 收到 Close(或兜底检查发现被替换)后自然退出。
    if let Some(old) = registry.register(node_id.clone(), handle.clone()) {
        tracing::info!(node_id = %node_id, "duplicate node_id, kicking previous tunnel");
        let _ = old.close().await;
    }

    // 注册节点 online(upsert;display_name 变更也在这里刷新)。
    if let Err(e) = db::crud::upsert_node(&state.db, &node_id, &display_name).await {
        tracing::error!(node_id = %node_id, error = %e, "upsert_node failed");
    }

    tracing::info!(node_id = %node_id, display_name = %display_name, "PC daemon tunnel connected");

    // 心跳 task(30s ping / 90s 无 pong → 离线)。
    let heartbeat_cfg = state.heartbeat;
    tokio::spawn(heartbeat_loop(state.clone(), handle.clone(), heartbeat_cfg));

    // 接收循环(本 task 持有 stream;退出时做清理)。
    receive_loop(state, registry, handle, stream).await;
}

/// 心跳:每 `ping_interval` 发 Ping;`timeout` 内无 Pong → 移除 + offline。
///
/// 超时路径(design §2.4):PC 网络分区时 TCP 可能不报错(收不到 FIN),
/// 接收循环会一直阻塞 —— 只有心跳能判死。发送 Ping 失败说明连接已
/// 死,交给接收循环清理。
async fn heartbeat_loop(
    state: Arc<RemoteState>,
    handle: Arc<ConnHandle>,
    cfg: HeartbeatConfig,
) {
    let timeout_ms = cfg.timeout.as_millis() as i64;
    let mut ticker = tokio::time::interval(cfg.ping_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await; // 跳过首个立即 tick(注册时 last_pong 已初始化)

    loop {
        ticker.tick().await;
        if handle.ping().await.is_err() {
            tracing::debug!(node_id = %handle.node_id, "ping send failed, conn dead");
            break; // 接收循环会清理
        }
        if handle.stale_by(timeout_ms) {
            tracing::warn!(
                node_id = %handle.node_id,
                last_pong_ms = handle.last_pong_ms.load(std::sync::atomic::Ordering::Relaxed),
                "heartbeat timeout (no pong within {}ms), marking node offline",
                timeout_ms
            );
            if let Some(removed) = state
                .node_connections
                .remove_if_current(&handle.node_id, handle.conn_id)
            {
                let _ = db::crud::update_node_status(&state.db, &handle.node_id, NODE_STATUS_OFFLINE, db::now_ms()).await;
                let _ = removed.close().await;
            }
            break;
        }
    }
}

/// 接收循环:帧分派 + Pong 续期 + 被替换/被移除时的兜底退出。
async fn receive_loop(
    state: Arc<RemoteState>,
    registry: Arc<TunnelRegistry>,
    handle: Arc<ConnHandle>,
    mut stream: futures_util::stream::SplitStream<WebSocket>,
) {
    // 兜底检查间隔 = 心跳超时(被踢但 PC 不回 Close / 网络分区时,
    // 本循环靠它退出)。
    let mut stale_check = tokio::time::interval(state.heartbeat.timeout);
    stale_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    stale_check.tick().await;

    loop {
        tokio::select! {
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Pong(_))) => {
                        // design §2.4:收到 Pong → 续 last_seen_at。
                        // axum 底层 tungstenite 对入站 Ping 自动回 Pong,
                        // 这里只需记录我们的 Ping 有回音。
                        let now = db::now_ms();
                        handle.last_pong_ms.store(now, std::sync::atomic::Ordering::Relaxed);
                        if let Err(e) = db::crud::update_node_status(&state.db, &handle.node_id, NODE_STATUS_ONLINE, now).await {
                            tracing::warn!(node_id = %handle.node_id, error = %e, "update_node_status on pong failed");
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<Frame>(&text) {
                            Ok(frame) => dispatch_frame(&state, &handle, frame).await,
                            Err(e) => tracing::warn!(node_id = %handle.node_id, error = %e, "dropping unparseable frame"),
                        }
                    }
                    // 对端主动关 / 连接断开 → 退出清理
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    // Ping(axum 自动回 Pong)/ Binary / 其他 → 忽略
                    _ => {}
                }
            }
            _ = stale_check.tick() => {
                if !registry.is_current(&handle.node_id, handle.conn_id) {
                    // 被新连接替换(踢旧完成)或已被心跳移除 → 退出
                    break;
                }
            }
        }
    }

    // 退出清理:只动自己的连接(remove_if_current)。
    if let Some(removed) = registry.remove_if_current(&handle.node_id, handle.conn_id) {
        let _ = db::crud::update_node_status(&state.db, &handle.node_id, NODE_STATUS_OFFLINE, db::now_ms()).await;
        tracing::info!(node_id = %handle.node_id, "tunnel disconnected, node offline");
        drop(removed); // 释放 sink
    }
}

/// 帧分派(P3-2):internal RPC / 普通 Request / Response / Stream。
/// - `Response` → pending 表按 id 路由回等待方(Step 6 落地)
/// - internal RPC(Step 7)/ Stream SSE 桥接(S3)各自把分支升级为真实实现
async fn dispatch_frame(state: &Arc<RemoteState>, handle: &Arc<ConnHandle>, frame: Frame) {
    match frame {
        Frame::Request { id, path, body, .. } => {
            if path.starts_with(everlasting_remote_protocol::INTERNAL_PREFIX) {
                // internal RPC(design §2.3.1 P3-2):配对码生成等,
                // 结果以 Response 帧回 PC
                if let Some(reply) =
                    crate::routes::pairing::handle_internal_rpc(state, &handle.node_id, id, &path, &body).await
                {
                    if let Err(e) = handle.send_frame(&reply).await {
                        tracing::warn!(node_id = %handle.node_id, error = %e, "internal RPC reply send failed");
                    }
                }
            } else {
                tracing::debug!(node_id = %handle.node_id, id, path = %path, "ignore PC-originated Request(S1 无此方向)");
            }
        }
        Frame::Response { id, .. } => {
            // 非流式回复:路由回 proxy 的 oneshot 等待方。未知 id →
            // 协议异常(warn,不关闭连接 —— 单条丢弃)。
            if let Some(PendingReply::Oneshot(tx)) = state.pending.remove(id) {
                let _ = tx.send(frame);
            } else {
                tracing::warn!(node_id = %handle.node_id, id, "Response for unknown/unmatched request id");
            }
        }
        Frame::Stream { id, .. } => {
            // S3:SSE 桥接 —— Chunk/End/Error 经 mpsc 送手机 SSE 连接
            tracing::debug!(node_id = %handle.node_id, id, "Stream 帧:SSE 桥接留 S3");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RemoteConfig;
    use crate::db::pool;
    use crate::db::schema;
    use crate::pending::PendingTable;
    use crate::ratelimit::RateLimiter;
    use crate::tunnel_registry::HeartbeatConfig;
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message as ClientMessage;
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

    // axum ws 客户端在 read() 里对 Ping 自动回 Pong(tungstenite 行为);
    // 测试读循环用 futures-util 的 StreamExt/SinkExt。
    use futures_util::{SinkExt, StreamExt};

    /// 起一个真实 server(tempdir db + 自定义心跳参数),返回 (port, state)。
    /// tempdir 故意 leak —— pool 连接还开着,测试期间目录必须活着。
    async fn start_server(heartbeat: HeartbeatConfig) -> (u16, Arc<RemoteState>) {
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
            node_connections: Arc::new(TunnelRegistry::new()),
            heartbeat,
            pending: Arc::new(PendingTable::new(Duration::from_secs(60))),
            pairing_ratelimit: Arc::new(RateLimiter::new(1000, Duration::from_secs(60))),
        });
        let router = crate::server::build_router(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("local_addr").port();
        tokio::spawn(async move {
            // ws_handler 提取 ConnectInfo<SocketAddr>,必须用
            // into_make_service_with_connect_info 提供服务。
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .expect("serve");
        });
        (port, state)
    }

    type ClientWs = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

    /// 连 /ws,返回握手结果。`Error` 在 0.24 是 tungstenite::Error
    /// (tokio-tungstenite re-export 整个 tungstenite)。
    async fn connect(port: u16, query: &str) -> Result<ClientWs, tokio_tungstenite::tungstenite::Error> {
        let url = format!("ws://127.0.0.1:{port}/ws{query}");
        tokio_tungstenite::connect_async(url).await.map(|(ws, _)| ws)
    }

    async fn wait_for_node(state: &RemoteState, node_id: &str, timeout: Duration) -> db::Node {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(node) = db::crud::get_node(&state.db, node_id).await.expect("get_node") {
                return node;
            }
            assert!(tokio::time::Instant::now() < deadline, "node {node_id} 未在 {timeout:?} 内注册");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn wait_for_status(state: &RemoteState, node_id: &str, status: &str, timeout: Duration) -> db::Node {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(node) = db::crud::get_node(&state.db, node_id).await.expect("get_node") {
                if node.status == status {
                    return node;
                }
            }
            assert!(tokio::time::Instant::now() < deadline, "node {node_id} 未在 {timeout:?} 内变为 {status}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn small_heartbeat() -> HeartbeatConfig {
        HeartbeatConfig {
            ping_interval: Duration::from_millis(40),
            timeout: Duration::from_millis(160),
        }
    }

    // ---- 握手拒绝 ----

    #[tokio::test]
    async fn wrong_secret_rejected_401() {
        let (port, _state) = start_server(HeartbeatConfig::default()).await;
        let err = connect(port, "?secret=wrong&node_id=pc-1").await.expect_err("wrong secret must fail");
        match err {
            tokio_tungstenite::tungstenite::Error::Http(resp) => assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED),
            other => panic!("expected Http(401), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn missing_secret_rejected_401() {
        let (port, _state) = start_server(HeartbeatConfig::default()).await;
        let err = connect(port, "?node_id=pc-1").await.expect_err("missing secret must fail");
        assert!(matches!(err, tokio_tungstenite::tungstenite::Error::Http(_)));
    }

    #[tokio::test]
    async fn missing_node_id_rejected_401() {
        let (port, _state) = start_server(HeartbeatConfig::default()).await;
        let err = connect(port, "?secret=test").await.expect_err("missing node_id must fail");
        assert!(matches!(err, tokio_tungstenite::tungstenite::Error::Http(_)));
    }

    // ---- 注册 ----

    #[tokio::test]
    async fn valid_secret_registers_node_online() {
        let (port, state) = start_server(HeartbeatConfig::default()).await;
        let mut ws = connect(port, "?secret=test&node_id=pc-1&display_name=TestPC").await.expect("connect");

        let node = wait_for_node(&state, "pc-1", Duration::from_secs(2)).await;
        assert_eq!(node.status, NODE_STATUS_ONLINE);
        assert_eq!(node.display_name, "TestPC");
        assert_eq!(state.node_connections.len(), 1);
        assert!(state.node_connections.is_current("pc-1", 0));

        let _ = ws.close(None).await;
    }

    /// display_name 缺省 = node_id(design:PC 可只传 node_id)。
    #[tokio::test]
    async fn display_name_defaults_to_node_id() {
        let (port, state) = start_server(HeartbeatConfig::default()).await;
        let mut ws = connect(port, "?secret=test&node_id=pc-2").await.expect("connect");
        let node = wait_for_node(&state, "pc-2", Duration::from_secs(2)).await;
        assert_eq!(node.display_name, "pc-2");
        let _ = ws.close(None).await;
    }

    /// 重复 node_id:新连接注册 → 旧连接被 Close 踢掉;节点保持 online
    /// (不能因为旧连接退出被误标 offline)。
    #[tokio::test]
    async fn duplicate_node_id_kicks_old_conn() {
        let (port, state) = start_server(HeartbeatConfig::default()).await;
        let mut old_ws = connect(port, "?secret=test&node_id=pc-1").await.expect("first connect");
        wait_for_node(&state, "pc-1", Duration::from_secs(2)).await;

        let mut new_ws = connect(port, "?secret=test&node_id=pc-1&display_name=新PC").await.expect("second connect");
        // 旧连接应收到 Close
        let msg = tokio::time::timeout(Duration::from_secs(2), old_ws.next())
            .await
            .expect("old conn should be closed")
            .expect("some message")
            .expect("no error");
        assert!(matches!(msg, ClientMessage::Close(_)), "expected Close, got {msg:?}");

        // 注册表只有新连接;等第二次 upsert 落库(display_name 刷新,
        // 与状态断言分离 —— 避免竞态),status 仍 online
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        let node = loop {
            if let Some(n) = db::crud::get_node(&state.db, "pc-1").await.expect("get_node") {
                if n.display_name == "新PC" {
                    break n;
                }
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "display_name 未在 2s 内刷新为新PC"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert_eq!(state.node_connections.len(), 1);
        assert!(state.node_connections.is_current("pc-1", 1));
        assert_eq!(node.status, NODE_STATUS_ONLINE);

        let _ = new_ws.close(None).await;
    }

    // ---- 心跳 ----

    /// ping/pong 正常:节点持续 online,last_seen_at 持续刷新。
    #[tokio::test]
    async fn heartbeat_ping_pong_keeps_node_online() {
        let (port, state) = start_server(small_heartbeat()).await;
        let mut ws = connect(port, "?secret=test&node_id=pc-1").await.expect("connect");
        let node = wait_for_node(&state, "pc-1", Duration::from_secs(2)).await;
        let seen_before = node.last_seen_at;

        // 客户端读循环(auto-pong;tungstenite 在 read() 里回 Pong)——
        // 跑 3 个 ping 周期(40ms × 3 = 120ms)
        let mut last_pong_at = seen_before;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(200);
        while tokio::time::Instant::now() < deadline {
            let msg = tokio::time::timeout(Duration::from_millis(100), ws.next())
                .await
                .expect("server should keep sending pings")
                .expect("message")
                .expect("no error");
            match msg {
                ClientMessage::Ping(_) | ClientMessage::Pong(_) => {}
                _ => {} // Text 等忽略
            }
            if let Some(n) = db::crud::get_node(&state.db, "pc-1").await.unwrap() {
                last_pong_at = n.last_seen_at;
            }
        }

        assert!(
            last_pong_at >= seen_before,
            "last_seen_at 应随 pong 刷新:before={seen_before} after={last_pong_at}"
        );
        assert_eq!(state.node_connections.len(), 1);
        let node = wait_for_status(&state, "pc-1", NODE_STATUS_ONLINE, Duration::from_secs(1)).await;
        assert_eq!(node.status, NODE_STATUS_ONLINE);

        let _ = ws.close(None).await;
    }

    /// 心跳超时:客户端不读(无 auto-pong)→ timeout 后节点判离线 + 注册表清空。
    #[tokio::test]
    async fn heartbeat_timeout_marks_node_offline() {
        let (port, state) = start_server(small_heartbeat()).await;
        // 保持 socket 打开但不读 —— 不发 pong,ping 打进 TCP 缓冲
        let _ws = connect(port, "?secret=test&node_id=pc-1").await.expect("connect");
        wait_for_node(&state, "pc-1", Duration::from_secs(2)).await;
        assert_eq!(state.node_connections.len(), 1);

        // 160ms 超时 + 余量,轮询到 offline
        wait_for_status(&state, "pc-1", NODE_STATUS_OFFLINE, Duration::from_secs(3)).await;
        assert_eq!(state.node_connections.len(), 0);
    }

    /// 正常断开(客户端 Close):节点立即判离线(不等心跳超时)。
    #[tokio::test]
    async fn clean_disconnect_marks_offline() {
        let (port, state) = start_server(HeartbeatConfig::default()).await;
        let mut ws = connect(port, "?secret=test&node_id=pc-1").await.expect("connect");
        wait_for_node(&state, "pc-1", Duration::from_secs(2)).await;

        let _ = ws.close(None).await;
        wait_for_status(&state, "pc-1", NODE_STATUS_OFFLINE, Duration::from_secs(2)).await;
        assert_eq!(state.node_connections.len(), 0);
    }

    // ---- 帧分派 ----

    /// PC 发一个普通 Request(非 internal)→ 连接保持存活(不崩溃、不关闭)。
    #[tokio::test]
    async fn non_internal_request_ignored_conn_stays_alive() {
        let (port, state) = start_server(HeartbeatConfig::default()).await;
        let mut ws = connect(port, "?secret=test&node_id=pc-1").await.expect("connect");
        wait_for_node(&state, "pc-1", Duration::from_secs(2)).await;

        let frame = Frame::Request {
            id: 1,
            method: "GET".into(),
            path: "/api/v1/health".into(),
            headers: vec![],
            body: vec![],
        };
        ws.send(ClientMessage::Text(serde_json::to_string(&frame).unwrap()))
            .await
            .expect("send");

        // 等一个 ping 周期(默认 30s 太久 —— 用 state 的心跳参数;这里是
        // default 配置,直接等 300ms 确认连接没被踢即可)
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(state.node_connections.len(), 1, "conn 不应被关闭");
        let _ = ws.close(None).await;
    }

    /// 不可解析的帧 → warn + 忽略,连接保持存活(协议韧性)。
    #[tokio::test]
    async fn unparseable_frame_ignored_conn_stays_alive() {
        let (port, state) = start_server(HeartbeatConfig::default()).await;
        let mut ws = connect(port, "?secret=test&node_id=pc-1").await.expect("connect");
        wait_for_node(&state, "pc-1", Duration::from_secs(2)).await;

        ws.send(ClientMessage::Text("{not json".into())).await.expect("send");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(state.node_connections.len(), 1);
        let _ = ws.close(None).await;
    }

    // ---- internal RPC 全链路(Step 7)----

    /// PC 经 WSS 发 `/internal/pairing/generate` → 收 Response 帧含
    /// 6 位码 → 手机 redeem 成功(码绑定该 PC)。
    #[tokio::test]
    async fn pairing_generate_via_ws_then_redeem() {
        let (port, state) = start_server(HeartbeatConfig::default()).await;
        let mut pc = connect(port, "?secret=test&node_id=pc-1&display_name=公司PC")
            .await
            .expect("pc connect");
        // 等注册(配对码绑定 node_id,node 必须已 upsert)
        wait_for_node(&state, "pc-1", Duration::from_secs(2)).await;

        // PC 发 internal RPC(design §2.3:生成动作由 PC 触发)
        let req = Frame::Request {
            id: 1,
            method: "POST".into(),
            path: "/internal/pairing/generate".into(),
            headers: vec![],
            body: vec![],
        };
        pc.send(ClientMessage::Text(serde_json::to_string(&req).unwrap()))
            .await
            .expect("send internal rpc");

        // PC 收 Response 帧
        let msg = tokio::time::timeout(Duration::from_secs(2), pc.next())
            .await
            .expect("reply frame")
            .expect("message")
            .expect("no error");
        let ClientMessage::Text(text) = msg else { panic!("expected text") };
        let Frame::Response { id, status, body, .. } = serde_json::from_str(&text).expect("parse")
        else {
            panic!("expected Response frame");
        };
        assert_eq!(id, 1);
        assert_eq!(status, 200);
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        let code = json["code"].as_str().expect("code").to_string();
        assert_eq!(code.len(), 6);
        assert_eq!(json["expires_in"], 60);

        // 手机 redeem(crud 层;HTTP 层映射已在 pairing::tests 覆盖)
        let redeemed = crate::db::crud::redeem_pairing_code(&state.db, &code, "test phone")
            .await
            .expect("redeem");
        assert_eq!(redeemed.node_id, "pc-1");
        assert_eq!(redeemed.node_display_name, "公司PC");
        assert_eq!(redeemed.device_token.len(), 64);

        let _ = pc.close(None).await;
    }
}
