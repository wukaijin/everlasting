//! Tunnel 集成测试:fake remote(tokio-tungstenite WSS 服务端)+ fake
//! loopback daemon(axum,模拟 `localhost:{local_port}`)。
//!
//! 覆盖(design 附录 A 验收):
//! - 连接 → remote 发 `Request` 帧 → dispatcher 打 loopback → `Response`
//!   帧回 remote(非流式链路)
//! - 配对 RPC:`/internal/pairing/generate` 经 pending 表路由回
//! - SSE:`Content-Type: text/event-stream` → `Stream::Chunk` × N → `End`
//! - auth 拒绝(握手 401)→ 停止重连,status 显示 `auth_failed`
//! - config 变更 → 旧连接优雅关闭 → 新 URL 重连(Q-T4)
//! - remote 断线 → 指数退避重连(第二次握手发生)
//!
//! 注:测试不依赖 S1 remote 二进制,全部本地 fake —— 单元级闭环,不占
//! 7457 端口,`cargo test --lib` 可并行。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::routing::get;
use everlasting_remote_protocol::Frame;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;

use super::manager::TunnelManager;
use super::TunnelConfig;

fn test_cfg(remote_port: u16) -> TunnelConfig {
    TunnelConfig {
        remote_url: format!("ws://127.0.0.1:{remote_port}"),
        shared_secret: "test-secret".into(),
        node_id: "test-pc".into(),
        display_name: "Test PC".into(),
    }
}

/// fake loopback daemon(模拟 PC daemon 自己的 axum):
/// - `GET /api/v1/health` → 200 `{"ok":true}`
/// - `GET /api/v1/stream` → SSE(2 个 chunk)
async fn spawn_fake_loopback() -> u16 {
    async fn health() -> axum::Json<serde_json::Value> {
        axum::Json(serde_json::json!({ "ok": true }))
    }

    async fn sse_stream() -> axum::response::Response {
        let chunks = vec![
            axum::body::Bytes::from_static(b"id:1\ndata:{\"a\":1}\n\n"),
            axum::body::Bytes::from_static(b"id:2\ndata:{\"b\":2}\n\n"),
        ];
        let stream = tokio_stream::iter(chunks.into_iter().map(Ok::<_, std::io::Error>));
        axum::response::Response::builder()
            .header("content-type", "text/event-stream")
            .body(axum::body::Body::from_stream(stream))
            .expect("build sse body")
    }

    let router = axum::Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/stream", get(sse_stream));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let port = listener.local_addr().expect("local_addr").port();
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve loopback");
    });
    port
}

/// 轮询 manager 状态直到 connected。
async fn wait_for_connected(manager: &Arc<TunnelManager>, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if manager.status().connected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("tunnel did not connect within {timeout:?}");
}

/// 从 fake remote 侧读下一个文本帧并解析成 `Frame`。
async fn next_frame(ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) -> Frame {
    let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("frame timeout")
        .expect("stream ended")
        .expect("ws error");
    let Message::Text(text) = msg else {
        panic!("expected text frame, got {msg:?}");
    };
    serde_json::from_str(&text).expect("parse frame")
}

// ---------------------------------------------------------------------------
// 1. 非流式链路 + 配对 RPC
// ---------------------------------------------------------------------------

/// remote 发 `Request`(GET /api/v1/health)→ dispatcher 打 loopback →
/// `Response` 帧回 remote;同时验证配对 RPC 走 pending 表路由。
#[tokio::test]
async fn connect_dispatch_and_pairing_roundtrip() {
    let loopback_port = spawn_fake_loopback().await;

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind remote");
    let remote_port = listener.local_addr().expect("local_addr").port();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        let mut ws = tokio_tungstenite::accept_async(stream)
            .await
            .expect("handshake");

        // 1. 配对 RPC:客户端发 `/internal/pairing/generate` → 回 Response 帧
        let frame = next_frame(&mut ws).await;
        let Frame::Request { id, path, .. } = frame else {
            panic!("expected pairing Request, got {frame:?}");
        };
        assert_eq!(path, "/internal/pairing/generate");
        let reply = Frame::Response {
            id,
            status: 200,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: br#"{"code":"123456","expires_in":60}"#.to_vec(),
        };
        ws.send(Message::Text(serde_json::to_string(&reply).unwrap()))
            .await
            .expect("send pairing reply");

        // 2. remote → PC 发 health Request
        let req = Frame::Request {
            id: 77,
            method: "GET".into(),
            path: "/api/v1/health".into(),
            headers: vec![("Accept".into(), "application/json".into())],
            body: vec![],
        };
        ws.send(Message::Text(serde_json::to_string(&req).unwrap()))
            .await
            .expect("send health request");

        // 3. 等 dispatcher 的 Response
        let frame = next_frame(&mut ws).await;
        let Frame::Response {
            id, status, body, ..
        } = frame
        else {
            panic!("expected Response, got {frame:?}");
        };
        assert_eq!(id, 77);
        assert_eq!(status, 200);
        assert_eq!(body, br#"{"ok":true}"#);

        let _ = ws.close(None).await;
    });

    let manager = TunnelManager::new();
    manager.set_local_port(loopback_port);
    manager.start();
    manager.set_config(Some(test_cfg(remote_port)));
    wait_for_connected(&manager, Duration::from_secs(5)).await;

    // 配对 RPC(design §2.5):经 WSS 调 remote,拿 `{code, expires_in}`
    let frame = manager
        .send_rpc_and_wait("POST", "/internal/pairing/generate", Vec::new())
        .await
        .expect("pairing rpc");
    let Frame::Response { status, body, .. } = frame else {
        panic!("expected Response from pairing rpc");
    };
    assert_eq!(status, 200);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json body");
    assert_eq!(json["code"], "123456");
    assert_eq!(json["expires_in"], 60);

    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .expect("fake remote completed")
        .expect("fake remote panicked");

    manager.stop();
}

// ---------------------------------------------------------------------------
// 2. SSE 流式
// ---------------------------------------------------------------------------

/// SSE 响应 → `Stream::Chunk` × N → `Stream::End`(纯字节透传,Q-T3)。
#[tokio::test]
async fn sse_stream_forwarded_as_chunks() {
    let loopback_port = spawn_fake_loopback().await;

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind remote");
    let remote_port = listener.local_addr().expect("local_addr").port();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        let mut ws = tokio_tungstenite::accept_async(stream)
            .await
            .expect("handshake");

        // remote → PC:SSE 请求
        let req = Frame::Request {
            id: 88,
            method: "GET".into(),
            path: "/api/v1/stream".into(),
            headers: vec![("Last-Event-ID".into(), "0".into())],
            body: vec![],
        };
        ws.send(Message::Text(serde_json::to_string(&req).unwrap()))
            .await
            .expect("send sse request");

        // 收 Stream 帧直到 End;不得出现 Response 帧
        let mut total = Vec::new();
        let mut chunk_count = 0usize;
        loop {
            let frame = next_frame(&mut ws).await;
            match frame {
                Frame::Stream { id, event } => {
                    assert_eq!(id, 88);
                    match event {
                        everlasting_remote_protocol::StreamEvent::Chunk { bytes } => {
                            chunk_count += 1;
                            total.extend_from_slice(&bytes);
                        }
                        everlasting_remote_protocol::StreamEvent::End => break,
                        everlasting_remote_protocol::StreamEvent::Error { message } => {
                            panic!("unexpected Stream::Error: {message}")
                        }
                    }
                }
                other => panic!("expected Stream frame, got {other:?}"),
            }
        }
        // 纯字节透传:拼接结果等于 SSE 原文
        assert_eq!(total, b"id:1\ndata:{\"a\":1}\n\nid:2\ndata:{\"b\":2}\n\n");
        assert!(chunk_count >= 2, "chunk_count = {chunk_count}");

        let _ = ws.close(None).await;
    });

    let manager = TunnelManager::new();
    manager.set_local_port(loopback_port);
    manager.start();
    manager.set_config(Some(test_cfg(remote_port)));
    wait_for_connected(&manager, Duration::from_secs(5)).await;

    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .expect("fake remote completed")
        .expect("fake remote panicked");

    manager.stop();
}

// ---------------------------------------------------------------------------
// 3. auth 拒绝 → 停止重连
// ---------------------------------------------------------------------------

/// 握手 401(shared_secret 校验失败)→ status `auth_failed`,**不再重连**
/// (design §6.2:配置错误,重连无意义)。
///
/// `result_large_err`:tungstenite `accept_hdr_async` 的握手 callback 签名
/// 强制返回 `Result<Response, tauri::http::Response<Option<String>>>`(Err
/// 变体 136 字节)—— 由库签名决定,无法缩减,故就地 allow。
#[tokio::test]
#[allow(clippy::result_large_err)]
async fn auth_reject_stops_reconnect() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind remote");
    let remote_port = listener.local_addr().expect("local_addr").port();
    let attempts = Arc::new(AtomicUsize::new(0));
    let a2 = attempts.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            a2.fetch_add(1, Ordering::SeqCst);
            // 拒绝握手:401(auth 类错误)。ErrorResponse 的 body 类型是
            // `Option<String>`(tungstenite 0.24 握手错误体),builder 直接
            // 构造 `tauri::http::Response`(与 tungstenite 同 instance)。
            let cb = |_: &tokio_tungstenite::tungstenite::handshake::server::Request,
                      _: tokio_tungstenite::tungstenite::handshake::server::Response|
             -> Result<
                tokio_tungstenite::tungstenite::handshake::server::Response,
                tauri::http::Response<Option<String>>,
            > {
                Err(tauri::http::Response::builder()
                    .status(401)
                    .body(None)
                    .expect("build 401 response"))
            };
            let _ = tokio_tungstenite::accept_hdr_async(stream, cb).await;
        }
    });

    let manager = TunnelManager::new();
    manager.set_local_port(7456);
    manager.start();
    manager.set_config(Some(test_cfg(remote_port)));

    // 等 status 变成 auth_failed(客户端停止重连前会标记)
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if manager.status().last_error.as_deref() == Some("auth_failed") {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "status did not become auth_failed: {:?}",
            manager.status()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!manager.status().connected);

    // 越过第一个退避窗口(1s)仍无第二次连接尝试 —— 停止重连生效
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "auth failure must stop reconnecting"
    );

    manager.stop();
}

// ---------------------------------------------------------------------------
// 4. config 变更 → 旧连接关、新 URL 重连
// ---------------------------------------------------------------------------

/// `set_config(Some(new_cfg))` → 旧 WSS 优雅关闭(发 Close 帧)→ 新 task
/// 用新 URL 连上(Q-T4:实时生效,不重启 daemon)。
#[tokio::test]
async fn config_change_reconnects_to_new_url() {
    let loopback_port = spawn_fake_loopback().await;

    // remote 1:接受连接 → 通知测试 → 等 Close → 通知测试
    let (conn1_tx, conn1_rx) = oneshot::channel();
    let (closed1_tx, closed1_rx) = oneshot::channel();
    let listener1 = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind remote1");
    let port1 = listener1.local_addr().expect("local_addr").port();
    tokio::spawn(async move {
        let (stream, _) = listener1.accept().await.expect("accept remote1");
        let mut ws = tokio_tungstenite::accept_async(stream)
            .await
            .expect("handshake remote1");
        let _ = conn1_tx.send(());
        loop {
            match ws.next().await {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            }
        }
        let _ = closed1_tx.send(());
    });

    // remote 2:接受连接 → 通知测试(保持连接)
    let (conn2_tx, conn2_rx) = oneshot::channel();
    let listener2 = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind remote2");
    let port2 = listener2.local_addr().expect("local_addr").port();
    tokio::spawn(async move {
        let (stream, _) = listener2.accept().await.expect("accept remote2");
        let mut ws = tokio_tungstenite::accept_async(stream)
            .await
            .expect("handshake remote2");
        let _ = conn2_tx.send(());
        let _ = ws.next().await; // 保持到测试结束
    });

    let manager = TunnelManager::new();
    manager.set_local_port(loopback_port);
    manager.start();
    manager.set_config(Some(test_cfg(port1)));
    tokio::time::timeout(Duration::from_secs(5), conn1_rx)
        .await
        .expect("conn1 established")
        .expect("conn1 signal");

    manager.set_config(Some(test_cfg(port2)));
    tokio::time::timeout(Duration::from_secs(5), closed1_rx)
        .await
        .expect("old connection closed")
        .expect("closed signal");
    tokio::time::timeout(Duration::from_secs(5), conn2_rx)
        .await
        .expect("conn2 established")
        .expect("conn2 signal");

    // 状态快照跟着新连接走(conn2 的 accept 信号先于客户端的
    // register_conn,需再轮询 connected)。
    wait_for_connected(&manager, Duration::from_secs(5)).await;
    let status = manager.status();
    assert!(status.connected);
    assert_eq!(
        status.remote_url.as_deref(),
        Some(format!("ws://127.0.0.1:{port2}").as_str())
    );

    manager.stop();
}

// ---------------------------------------------------------------------------
// 5. remote 断线 → 指数退避重连
// ---------------------------------------------------------------------------

/// remote 接受后立即断 → 客户端走退避重连,第二次连接发生(1s 后)。
#[tokio::test]
async fn remote_drop_triggers_backoff_reconnect() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind remote");
    let remote_port = listener.local_addr().expect("local_addr").port();
    let attempts = Arc::new(AtomicUsize::new(0));
    let a2 = attempts.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            a2.fetch_add(1, Ordering::SeqCst);
            drop(stream); // 立即断(模拟 remote 挂掉)
        }
    });

    let manager = TunnelManager::new();
    manager.set_local_port(7456);
    manager.start();
    manager.set_config(Some(test_cfg(remote_port)));

    // 第一个退避窗口 1s;4s 内应有 ≥2 次连接尝试
    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    while attempts.load(Ordering::SeqCst) < 2 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "expected >= 2 connection attempts (backoff reconnect), got {}",
            attempts.load(Ordering::SeqCst)
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    manager.stop();
}
