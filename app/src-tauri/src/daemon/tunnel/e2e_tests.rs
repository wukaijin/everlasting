//! S3 端到端 harness(design §2.1/§2.2/§2.3 + implement.md Step 5,P2-2 2a)。
//!
//! **真 remote(everlasting-remote crate,dev-dep)+ 真 TunnelManager +
//! 真 dispatcher + fake loopback(SSE 用真实 `SseRegistry`)+ reqwest 手机**
//! —— 全链路端到端,端口全 0,`cargo test --lib` 可并行。
//!
//! 5 场景(design 附录 A):
//! 1. 非流式:手机 GET /api/v1/proxy/api/v1/health → 200(S1 回归)
//! 2. SSE 流式:手机 GET .../api/v1/stream(Accept: text/event-stream)
//!    → 实时收到 chunk 序列拼接还原
//! 3. 取消停转发 + **agent 不停**:持续流 + 手机中途断 → remote 发 End
//!    → PC 停转发(loopback 订阅断);双订阅者断言 broadcast 未被破坏
//! 4. 慢手机剔除(P1-1):不读 body → mpsc 满 → 剔除 → End → PC 停转发;
//!    流灌期间配对 RPC 仍即时响应(接收循环未被阻塞)
//! 5. PC 断线 → 手机 in-flight stream 关闭;PC 重连后新请求恢复
//! 6. shared_secret 错 → 握手 401(S1/S2 复用)
//!
//! fake loopback 的 `/api/v1/stream` 用**真实 `SseRegistry`**(daemon 公共
//! API,与 `routes/stream.rs` 同款 subscribe + KeepAlive)—— 相比固定
//! chunk 序列,它能验证"隧道是 SseRegistry 的普通订阅者"这一 D1 前提。

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use axum::Router;
use everlasting_remote::config::{RemoteConfig, RemoteState};
use everlasting_remote::db::{crud, pool, schema};
use everlasting_remote::pending::PendingTable;
use everlasting_remote::ratelimit::RateLimiter;
use everlasting_remote::server::build_router;
use everlasting_remote::tunnel_registry::{HeartbeatConfig, TunnelRegistry};
use futures_util::StreamExt;
use tokio::net::TcpListener;
use tokio_stream::wrappers::ReceiverStream;

use crate::daemon::sse::{SseFrame, SseRegistry, SseSubscription};
use crate::daemon::tunnel::TunnelManager;

use super::tests::{test_cfg, wait_for_connected};

// ---------------------------------------------------------------------------
// 基础设施:真 remote + 真 SseRegistry loopback + 广播 producer + 手机
// ---------------------------------------------------------------------------

/// 起真 remote 服务器(tempdir db + 默认心跳/60s pending 超时)。
/// tempdir 故意 leak —— pool 连接还开着,测试期间目录必须活着。
async fn spawn_remote() -> (u16, Arc<RemoteState>) {
    let dir = Box::leak(Box::new(tempfile::tempdir().expect("tempdir")));
    let config = RemoteConfig {
        port: 0,
        db_path: dir.path().join("remote.db"),
        shared_secret: "test-secret".into(),
    };
    let pool = pool::init_pool(&config.db_path).await.expect("init pool");
    schema::run_migrations(&pool).await.expect("migrations");
    let state = Arc::new(RemoteState {
        db: pool,
        shared_secret: config.shared_secret,
        node_connections: Arc::new(TunnelRegistry::new()),
        heartbeat: HeartbeatConfig::default(),
        pending: Arc::new(PendingTable::new(Duration::from_secs(60))),
        pairing_ratelimit: Arc::new(RateLimiter::new(1000, Duration::from_secs(60))),
    });
    let router = build_router(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind remote");
    let port = listener.local_addr().expect("local_addr").port();
    tokio::spawn(async move {
        // ws_handler 提取 ConnectInfo<SocketAddr>(与 remote 自身测试同款)
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("serve remote");
    });
    (port, state)
}

/// 预置 node + device,返回 (token, node_id)。
async fn seed_device(state: &RemoteState) -> (String, String) {
    crud::upsert_node(&state.db, "test-pc", "Test PC")
        .await
        .expect("upsert node");
    let token = "b".repeat(64);
    crud::insert_device(&state.db, &token, "test-pc", Some("test phone"))
        .await
        .expect("insert device");
    (token, "test-pc".to_string())
}

/// fake loopback daemon(模拟 PC daemon 自己的 axum):
/// - `GET /api/v1/health` → 200 `{"ok":true}`
/// - `GET /api/v1/stream` → **真实 `SseRegistry`** 订阅流(与
///   `routes/stream.rs` 同款:replay + live + KeepAlive 30s)
async fn spawn_registry_loopback(reg: Arc<SseRegistry>) -> u16 {
    async fn health() -> axum::Json<serde_json::Value> {
        axum::Json(serde_json::json!({ "ok": true }))
    }

    async fn sse_stream(
        State(reg): State<Arc<SseRegistry>>,
        headers: HeaderMap,
    ) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
        let last_event_id = headers
            .get("last-event-id")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());
        let SseSubscription { replay, live } = reg.subscribe(last_event_id);
        let frames = tokio_stream::iter(replay)
            .chain(ReceiverStream::new(live))
            .map(frame_to_event);
        Sse::new(frames).keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(30))
                .text("ping"),
        )
    }

    fn frame_to_event(f: SseFrame) -> Result<Event, Infallible> {
        Ok(Event::default()
            .event(f.event)
            .data(f.data)
            .id(f.id.to_string()))
    }

    let router = Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/stream", get(sse_stream))
        .with_state(reg);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let port = listener.local_addr().expect("local_addr").port();
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve loopback");
    });
    port
}

/// "agent"广播 producer:按 `interval` 持续向 SseRegistry 发 chat-event
/// (模拟 agent loop 的 emit;真实 broadcast 语义,agent 永不阻塞)。
/// 测试结束时随 runtime 一起 abort。
async fn producer(reg: Arc<SseRegistry>, interval: Duration) {
    let mut n = 0u64;
    loop {
        reg.broadcast(
            "chat-event",
            &serde_json::json!({ "request_id": "r1", "n": n }),
        );
        n += 1;
        tokio::time::sleep(interval).await;
    }
}

/// 轮询直到 `pred` 为真(断言带消息)。
async fn wait_until(what: &str, timeout: Duration, mut pred: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if pred() {
            return;
        }
        assert!(tokio::time::Instant::now() < deadline, "等不到: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 手机 SSE 请求(EventSource 风格:`?access_token=` query + Accept 头)。
async fn phone_sse(client: &reqwest::Client, remote_port: u16, token: &str) -> reqwest::Response {
    client
        .get(format!(
            "http://127.0.0.1:{remote_port}/api/v1/proxy/api/v1/stream?access_token={token}"
        ))
        .header("accept", "text/event-stream")
        .send()
        .await
        .expect("open phone sse")
}

/// 手机 body **读一段固定时长**后主动断开(读不完的流场景 —— 正常
/// 结束/断线时提前 EOF 退出)。
async fn phone_read_for(resp: reqwest::Response, total: Duration) -> Vec<u8> {
    let deadline = tokio::time::Instant::now() + total;
    let mut stream = resp.bytes_stream();
    let mut out = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break; // 读够时长,断开
        }
        match tokio::time::timeout(remaining, stream.next()).await {
            Err(_) => break,
            Ok(None) => break, // EOF(PC 正常结束 / 离线清理)
            Ok(Some(Ok(bytes))) => out.extend_from_slice(&bytes),
            Ok(Some(Err(e))) => panic!("body 读错: {e}"),
        }
    }
    out
}

/// 从 SSE body 提取 `data:` 行序列(忽略 `:ping` 注释帧;多行 data 取首行)。
fn sse_data_lines(body: &[u8]) -> Vec<String> {
    std::str::from_utf8(body)
        .unwrap_or_default()
        .split("\n\n")
        .filter_map(|frame| {
            frame
                .lines()
                .find_map(|l| l.strip_prefix("data:").map(|d| d.trim().to_string()))
        })
        .collect()
}

/// 从 data 行解析 `{"n": k}` 序列(serde_json 键序不保证,按 key 取)。
fn n_sequence(data: &[String]) -> Vec<u64> {
    data.iter()
        .map(|d| serde_json::from_str::<serde_json::Value>(d).expect("data 是 JSON"))
        .map(|v| v["n"].as_u64().expect("data 有 n 字段"))
        .collect()
}

// ---------------------------------------------------------------------------
// 1. 非流式(S1 回归)
// ---------------------------------------------------------------------------

/// 手机 GET /api/v1/proxy/api/v1/health(经真 remote + 真 PC dispatcher)
/// → 200 + body。S1 非流式链路在 S3 改动后的回归。
#[tokio::test]
async fn e2e_non_streaming_request_roundtrip() {
    let (remote_port, state) = spawn_remote().await;
    let (token, _) = seed_device(&state).await;
    let loopback_port = spawn_registry_loopback(Arc::new(SseRegistry::new())).await;

    let manager = TunnelManager::new();
    manager.set_local_port(loopback_port);
    manager.start();
    manager.set_config(Some(test_cfg(remote_port)));
    wait_for_connected(&manager, Duration::from_secs(5)).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!(
            "http://127.0.0.1:{remote_port}/api/v1/proxy/api/v1/health?access_token={token}"
        ))
        .send()
        .await
        .expect("phone request");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.bytes().await.expect("read body");
    assert_eq!(&body[..], br#"{"ok":true}"#);

    manager.stop();
}

// ---------------------------------------------------------------------------
// 2. SSE 流式:手机实时收 agent 事件
// ---------------------------------------------------------------------------

/// 手机(EventSource 风格)经 remote GET /api/v1/stream → 实时收到
/// producer 广播的 chat-event 序列(拼接还原 + 顺序断言)。
#[tokio::test]
async fn e2e_sse_stream_reaches_phone() {
    let (remote_port, state) = spawn_remote().await;
    let (token, _) = seed_device(&state).await;
    let reg = Arc::new(SseRegistry::new());
    let loopback_port = spawn_registry_loopback(reg.clone()).await;
    tokio::spawn(producer(reg.clone(), Duration::from_millis(30)));

    let manager = TunnelManager::new();
    manager.set_local_port(loopback_port);
    manager.start();
    manager.set_config(Some(test_cfg(remote_port)));
    wait_for_connected(&manager, Duration::from_secs(5)).await;

    let client = reqwest::Client::new();
    let resp = phone_sse(&client, remote_port, &token).await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert!(
        resp.headers()["content-type"]
            .to_str()
            .unwrap_or_default()
            .starts_with("text/event-stream"),
        "content-type 应为 text/event-stream"
    );

    // 读 ~800ms(约 25 帧),然后主动断开
    let body = phone_read_for(resp, Duration::from_millis(800)).await;
    let data = sse_data_lines(&body);
    assert!(data.len() >= 5, "应收到 ≥5 帧,got {}", data.len());
    let ns = n_sequence(&data);
    // producer 从 n=0 连续广播;手机中途加入,序列应严格递增且连续
    for w in ns.windows(2) {
        assert_eq!(w[1], w[0] + 1, "chunk 顺序错乱: {ns:?}");
    }
    // 端到端清理:断开后 remote pending 无泄漏
    wait_until("remote pending 清空", Duration::from_secs(3), || {
        state.pending.is_empty()
    })
    .await;

    manager.stop();
}

// ---------------------------------------------------------------------------
// 3. 取消停转发 + agent 不停(D1,双订阅者断言)
// ---------------------------------------------------------------------------

/// 手机断 SSE → remote 发 End → PC 停转发(loopback 隧道订阅断);
/// **第二个订阅者(本地浏览器)仍持续收到事件** —— broadcast 未被破坏,
/// agent 不会因手机断而停(停 agent 靠显式 cancel)。
#[tokio::test]
async fn e2e_phone_disconnect_cancels_forwarding_agent_continues() {
    let (remote_port, state) = spawn_remote().await;
    let (token, _) = seed_device(&state).await;
    let reg = Arc::new(SseRegistry::new());
    let loopback_port = spawn_registry_loopback(reg.clone()).await;
    tokio::spawn(producer(reg.clone(), Duration::from_millis(30)));

    // "本地浏览器"订阅者:先于手机注册,全程不消费 live 也不影响广播
    let mut local_live = reg.subscribe(None).live;
    assert_eq!(reg.subscriber_count(), 1, "初始仅本地订阅者");

    let manager = TunnelManager::new();
    manager.set_local_port(loopback_port);
    manager.start();
    manager.set_config(Some(test_cfg(remote_port)));
    wait_for_connected(&manager, Duration::from_secs(5)).await;

    // 手机:读 3 帧后断开(此时隧道订阅者已建立 → count=2)
    let client = reqwest::Client::new();
    let phone_token = token.clone();
    let phone = tokio::spawn(async move {
        let resp = phone_sse(&client, remote_port, &phone_token).await;
        let mut stream = resp.bytes_stream();
        let mut n = 0u32;
        while n < 3 {
            let Some(chunk) = stream.next().await else {
                break;
            };
            let _ = chunk.expect("chunk");
            n += 1;
        }
        drop(stream); // 手机断
    });
    wait_until(
        "隧道订阅者建立(count=2)",
        Duration::from_secs(5),
        || reg.subscriber_count() == 2,
    )
    .await;

    phone.await.expect("phone done");

    // 取消链路:remote 感知手机断 → End → PC cancel → drop resp →
    // loopback 隧道订阅被 SseRegistry 剔除(2 → 1)
    wait_until(
        "隧道订阅被剔除(count=1)",
        Duration::from_secs(5),
        || reg.subscriber_count() == 1,
    )
    .await;
    wait_until("remote pending 清空", Duration::from_secs(3), || {
        state.pending.is_empty()
    })
    .await;

    // ★ D1:本地订阅者仍持续收到事件(agent 不停,broadcast 未被破坏)
    for _ in 0..3 {
        let frame = tokio::time::timeout(Duration::from_secs(2), local_live.recv())
            .await
            .expect("本地订阅者必须继续收到事件(agent 未停)")
            .expect("live channel 未关闭");
        assert_eq!(frame.event, "chat-event");
    }

    manager.stop();
}

// ---------------------------------------------------------------------------
// 4. 流式期间 RPC 即时响应(P1-1 用户面)+ 断开清理传播
// ---------------------------------------------------------------------------

/// 流式转发进行中,同一 WSS 连接上的配对 RPC **即时响应**(接收循环
/// 不被流拖住 —— 慢手机剔除的确定性机制验证在 remote 单测
/// `slow_phone_evicted_without_blocking_receive_loop`,真 HTTP 下内核
/// TCP 缓冲会吸收写、mpsc 不会快速满,这里验用户面性质);RPC 期间
/// 手机持续收到事件(流不受影响);手机断开后取消传播完整落地。
#[tokio::test]
async fn e2e_stream_does_not_block_rpc_then_disconnect_cleans_up() {
    let (remote_port, state) = spawn_remote().await;
    let (token, _) = seed_device(&state).await;
    let reg = Arc::new(SseRegistry::new());
    let loopback_port = spawn_registry_loopback(reg.clone()).await;
    tokio::spawn(producer(reg.clone(), Duration::from_millis(30)));

    let manager = TunnelManager::new();
    manager.set_local_port(loopback_port);
    manager.start();
    manager.set_config(Some(test_cfg(remote_port)));
    wait_for_connected(&manager, Duration::from_secs(5)).await;

    // 手机:持续读流 3s
    let client = reqwest::Client::new();
    let phone_client = client.clone();
    let phone_token = token.clone();
    let phone = tokio::spawn(async move {
        let resp = phone_sse(&phone_client, remote_port, &phone_token).await;
        phone_read_for(resp, Duration::from_secs(3)).await
    });
    wait_until("隧道订阅者建立", Duration::from_secs(5), || {
        reg.subscriber_count() == 1
    })
    .await;

    // 流灌期间发配对 RPC(接收循环若被流拖住,这里必超时)
    let frame = tokio::time::timeout(
        Duration::from_secs(3),
        manager.send_rpc_and_wait("POST", "/internal/pairing/generate", Vec::new()),
    )
    .await
    .expect("配对 RPC 必须即时响应(接收循环未被流拖住)")
    .expect("rpc ok");
    let everlasting_remote_protocol::Frame::Response { status, body, .. } = frame else {
        panic!("expected Response frame");
    };
    assert_eq!(status, 200);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json body");
    assert_eq!(json["code"].as_str().expect("code").len(), 6);

    // 手机在整个 3s 里持续收到事件(RPC 不影响流)
    let body = phone.await.expect("phone done");
    assert!(!body.is_empty(), "流式期间手机应持续收到事件");
    assert!(sse_data_lines(&body).len() >= 5);

    // 手机断开 → 取消传播:remote 发 End → PC 停转发 → loopback 订阅断
    wait_until(
        "loopback 订阅断(取消传播落地)",
        Duration::from_secs(5),
        || reg.subscriber_count() == 0,
    )
    .await;
    wait_until("remote pending 清空", Duration::from_secs(3), || {
        state.pending.is_empty()
    })
    .await;

    manager.stop();
}

// ---------------------------------------------------------------------------
// 5. PC 断线 → 手机 in-flight stream 关闭;PC 重连后恢复
// ---------------------------------------------------------------------------

/// PC daemon 断线(配置停 tunnel → WSS 关闭)→ remote 离线清理 →
/// 手机 in-flight stream 收到 Error 关闭(不悬挂);PC 重连后新请求恢复。
#[tokio::test]
async fn e2e_pc_disconnect_closes_stream_then_recovers() {
    let (remote_port, state) = spawn_remote().await;
    let (token, _) = seed_device(&state).await;
    let reg = Arc::new(SseRegistry::new());
    let loopback_port = spawn_registry_loopback(reg.clone()).await;
    tokio::spawn(producer(reg.clone(), Duration::from_millis(30)));

    let manager = TunnelManager::new();
    manager.set_local_port(loopback_port);
    manager.start();
    manager.set_config(Some(test_cfg(remote_port)));
    wait_for_connected(&manager, Duration::from_secs(5)).await;

    // 手机:挂一条持续流。等收到首 chunk(流已建立、链路上数据在动)
    // 再断 PC —— 否则断线清理的 Error 可能赢过首 chunk,body 为空,
    // 测不出"断线前已有数据"。
    let client = reqwest::Client::new();
    let phone_client = client.clone();
    let phone_token = token.clone();
    let (first_tx, first_rx) = tokio::sync::oneshot::channel();
    let phone = tokio::spawn(async move {
        let resp = phone_sse(&phone_client, remote_port, &phone_token).await;
        let mut stream = resp.bytes_stream();
        let first = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .expect("首 chunk 应在 5s 内到达")
            .expect("stream 未提前结束")
            .expect("chunk ok");
        let _ = first_tx.send(first.len());
        let mut out = first.to_vec();
        loop {
            match tokio::time::timeout(Duration::from_secs(10), stream.next()).await {
                Err(_) => panic!("body 应在超时内结束(不悬挂)"),
                Ok(None) => break,
                Ok(Some(Ok(bytes))) => out.extend_from_slice(&bytes),
                Ok(Some(Err(e))) => panic!("body 读错: {e}"),
            }
        }
        out
    });
    // 首 chunk 到达 = 流已建立且数据在动
    tokio::time::timeout(Duration::from_secs(5), first_rx)
        .await
        .expect("手机未在 5s 内收到首 chunk")
        .expect("phone signal");

    // PC 断线(模拟 kill WSS:配置停 tunnel → 优雅 Close → 离线清理)
    manager.set_config(None);

    // 手机 in-flight stream 关闭(不悬挂);body 含断线前收到的数据
    let body = tokio::time::timeout(Duration::from_secs(10), phone)
        .await
        .expect("PC 断线后手机 stream 必须关闭")
        .expect("phone done");
    assert!(!body.is_empty(), "断线前应已收到数据");
    wait_until(
        "remote pending 清空(离线清理)",
        Duration::from_secs(3),
        || state.pending.is_empty(),
    )
    .await;

    // PC 重连 → 新请求恢复
    manager.set_config(Some(test_cfg(remote_port)));
    wait_for_connected(&manager, Duration::from_secs(10)).await;
    let resp = client
        .get(format!(
            "http://127.0.0.1:{remote_port}/api/v1/proxy/api/v1/health?access_token={token}"
        ))
        .send()
        .await
        .expect("health request");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.bytes().await.expect("read body");
    assert_eq!(&body[..], br#"{"ok":true}"#);

    manager.stop();
}

// ---------------------------------------------------------------------------
// 6. shared_secret 拒绝
// ---------------------------------------------------------------------------

/// 错误 secret 的 PC 握手被拒(401),remote 不注册。
#[tokio::test]
async fn e2e_wrong_secret_rejected() {
    let (remote_port, state) = spawn_remote().await;
    let err = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{remote_port}/ws?secret=wrong&node_id=test-pc"
    ))
    .await
    .expect_err("wrong secret must fail");
    assert!(
        matches!(err, tokio_tungstenite::tungstenite::Error::Http(_)),
        "expected Http(401), got {err:?}"
    );
    assert_eq!(state.node_connections.len(), 0, "被拒的连接不得注册");
}
