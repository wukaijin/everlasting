//! WSS 客户端主体(design §2.1 / implement.md Step 3)。
//!
//! - 连接:`{remote_url}/ws?secret=<S>&node_id=<ID>&display_name=<NAME>`,
//!   query 值 **percent-encode**(P2-1,中文 display_name / 特殊字符 secret
//!   直接拼会断握手)。
//! - 心跳:**remote 主导 ping,客户端只回 pong**(与 S1 契约一致,不是双端
//!   互 ping)。tungstenite `read()` 对入站 Ping 自动回 Pong(协议层行为,
//!   见 tungstenite `protocol::read` 的 `set_additional(Frame::pong)`),
//!   所以 serve loop 只需持续读流;读到任何帧都续 `last_frame`。
//! - 断线 / 90s 无帧(网络分区时 TCP 不报错)→ 指数退避重连
//!   (1s → 2s → … → cap 60s);shared_secret 校验失败(握手 401/403,
//!   auth 类错误)→ **停止重连**(配置错误,重连无意义,design §6.2)。
//! - 收到 `Frame::Request` → spawn `dispatcher::dispatch_one` 打 loopback。
//! - 收到 `Frame::Response` → 路由 `TunnelManager` 的 pending 表(配对 RPC)。
//! - 收到 `Frame::Stream` → remote → PC 方向的**取消信号**(S3,D3 不加
//!   新帧):remote 侧手机断 SSE / node 离线 → 发 `End/Error` →
//!   `manager.cancel_stream(id)` → sse_bridge 停转发;agent 不停。
//!
//! 日志 target `everlasting::daemon::tunnel`(design §6.1 文案)。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use everlasting_remote_protocol::Frame;
use futures_util::{SinkExt, StreamExt};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use tokio_util::sync::CancellationToken;

use super::manager::TunnelManager;
use super::{
    dispatcher, TunnelConfig, BACKOFF_CAP, BACKOFF_START, CONNECT_TIMEOUT, HEARTBEAT_STALE_MS,
    TUNNEL_TARGET,
};

type TunnelWs = WebSocketStream<MaybeTlsStream<TcpStream>>;
type TunnelSink = futures_util::stream::SplitSink<TunnelWs, Message>;

/// 重连循环的退出/分类错误(design §6.2 失败模式表)。
#[derive(Debug)]
pub enum TunnelError {
    /// shared_secret 校验失败(auth 类错误)→ 停止重连。
    AuthRejected,
    /// 其他连接级失败(不可达 / 断开 / 心跳超时 / 协议错)→ 退避重连。
    Transient(String),
}

/// 单调时钟的毫秒计数(用于 90s 无帧判定;`OnceLock` 锚定进程起点)。
fn now_monotonic_ms() -> i64 {
    static ZERO: OnceLock<std::time::Instant> = OnceLock::new();
    let zero = ZERO.get_or_init(std::time::Instant::now);
    zero.elapsed().as_millis() as i64
}

/// 连接 URL(design §2.1 / P2-1):query 值一律 percent-encode。
pub fn build_ws_url(cfg: &TunnelConfig) -> String {
    let secret = utf8_percent_encode(&cfg.shared_secret, NON_ALPHANUMERIC);
    let node_id = utf8_percent_encode(&cfg.node_id, NON_ALPHANUMERIC);
    let display_name = utf8_percent_encode(&cfg.display_name, NON_ALPHANUMERIC);
    format!(
        "{}/ws?secret={}&node_id={}&display_name={}",
        cfg.remote_url, secret, node_id, display_name
    )
}

/// tunnel task 主体:重连循环。`shutdown` 由 supervisor 持有(进程 shutdown /
/// config 变更都经它取消)。返回即 task 结束(仅两类:shutdown / auth 拒绝)。
pub async fn run_tunnel(
    cfg: TunnelConfig,
    manager: Arc<TunnelManager>,
    shutdown: CancellationToken,
) {
    // dispatcher 共用一个 reqwest client(连接池,design §2.2 —— 不每请求新建)。
    let client = reqwest::Client::new();
    let local_port = manager.local_port();
    let mut delay = BACKOFF_START;
    // 本次连接尝试是否已建立连接(register_conn 之后)。已建立 → 断线算
    // 「瞬时故障」,退避重置 —— 标准指数退避语义:1s→2s→…→60s 只针对
    // **连续失败**;长期连接后的断线不应沿用历史失败累积的长退避。
    let mut established = false;

    loop {
        if shutdown.is_cancelled() {
            return;
        }
        match connect_and_serve(
            &cfg,
            &manager,
            &client,
            local_port,
            &shutdown,
            &mut established,
        )
        .await
        {
            Ok(()) => {
                tracing::info!(target: TUNNEL_TARGET, node_id = %cfg.node_id, "tunnel stopped (shutdown)");
                return;
            }
            Err(TunnelError::AuthRejected) => {
                tracing::warn!(
                    target: TUNNEL_TARGET,
                    node_id = %cfg.node_id,
                    "tunnel auth failed (shared_secret rejected), stopping reconnect"
                );
                manager.mark_auth_failed(&cfg);
                return;
            }
            Err(TunnelError::Transient(reason)) => {
                if established {
                    delay = BACKOFF_START;
                }
                tracing::warn!(
                    target: TUNNEL_TARGET,
                    node_id = %cfg.node_id,
                    reason = %reason,
                    retry_in_ms = delay.as_millis(),
                    "tunnel_disconnected, reconnecting"
                );
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = shutdown.cancelled() => return,
                }
                delay = std::cmp::min(delay * 2, BACKOFF_CAP);
            }
        }
    }
}

/// 一次连接的完整生命周期:握手 → 注册 → 心跳监控 → serve loop → 清理。
/// `Ok(())` = shutdown 触发的优雅关闭;`Err` = 连接级失败(重连循环处理)。
/// `established` 反映本次尝试是否已建立连接(供 run_tunnel 做退避重置)。
async fn connect_and_serve(
    cfg: &TunnelConfig,
    manager: &Arc<TunnelManager>,
    client: &reqwest::Client,
    local_port: u16,
    shutdown: &CancellationToken,
    established: &mut bool,
) -> Result<(), TunnelError> {
    *established = false;
    let url = build_ws_url(cfg);
    tracing::info!(
        target: TUNNEL_TARGET,
        remote = %cfg.remote_url,
        node_id = %cfg.node_id,
        "tunnel_connecting"
    );

    // connect_async 默认无超时;套 `CONNECT_TIMEOUT`,并让 shutdown 能打断
    // (否则 config 变更/进程退出可能被 DNS/TCP 挂住)。
    let connect = tokio::time::timeout(CONNECT_TIMEOUT, connect_async(url.clone()));
    let (ws, _resp) = tokio::select! {
        res = connect => match res {
            Ok(Ok(conn)) => conn,
            Ok(Err(e)) => return Err(connect_error(e)),
            Err(_) => return Err(TunnelError::Transient("connect timeout".to_string())),
        },
        _ = shutdown.cancelled() => return Ok(()),
    };

    tracing::info!(target: TUNNEL_TARGET, node_id = %cfg.node_id, "tunnel_connected");

    let (sink, mut stream) = ws.split();
    let sink: Arc<Mutex<TunnelSink>> = Arc::new(Mutex::new(sink));
    let (frame_tx, mut frame_rx) = mpsc::unbounded_channel::<Frame>();
    manager.register_conn(frame_tx.clone(), cfg);
    *established = true;

    // 心跳监控:90s 无任何帧(remote 主导 ping,Ping 也算帧)→ 断线重连。
    let last_frame = Arc::new(AtomicI64::new(now_monotonic_ms()));
    let conn_shutdown = CancellationToken::new();
    let heartbeat = tokio::spawn(heartbeat_monitor(last_frame.clone(), conn_shutdown.clone()));

    let outcome = serve_loop(
        cfg,
        manager,
        client,
        local_port,
        &sink,
        &mut stream,
        &frame_tx,
        &mut frame_rx,
        &last_frame,
        &conn_shutdown,
        shutdown,
    )
    .await;

    conn_shutdown.cancel();
    let _ = heartbeat.await;
    manager.clear_conn();
    outcome
}

/// 握手错误分类:401/403 = auth 类错误(停止重连),其余瞬态。
fn connect_error(e: tokio_tungstenite::tungstenite::Error) -> TunnelError {
    match e {
        tokio_tungstenite::tungstenite::Error::Http(resp)
            if resp.status() == http_status_unauthorized()
                || resp.status() == http_status_forbidden() =>
        {
            TunnelError::AuthRejected
        }
        other => TunnelError::Transient(other.to_string()),
    }
}

fn http_status_unauthorized() -> axum::http::StatusCode {
    axum::http::StatusCode::UNAUTHORIZED
}
fn http_status_forbidden() -> axum::http::StatusCode {
    axum::http::StatusCode::FORBIDDEN
}

/// 心跳监控:每 10s 检查 `last_frame`,超过 [`HEARTBEAT_STALE_MS`] 未更新
/// → 判连接死(网络分区),cancel 连接级令牌让 serve loop 退出重连。
async fn heartbeat_monitor(last_frame: Arc<AtomicI64>, conn_shutdown: CancellationToken) {
    let mut ticker = tokio::time::interval(Duration::from_secs(10));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await; // 跳过首个立即 tick(连接刚建立,last_frame 刚初始化)
    loop {
        tokio::select! {
            _ = conn_shutdown.cancelled() => return,
            _ = ticker.tick() => {
                let now = now_monotonic_ms();
                let last = last_frame.load(Ordering::Relaxed);
                if now - last > HEARTBEAT_STALE_MS {
                    tracing::warn!(
                        target: TUNNEL_TARGET,
                        last_frame_ms = last,
                        "heartbeat timeout (no frame within {}s), reconnecting",
                        HEARTBEAT_STALE_MS / 1000
                    );
                    conn_shutdown.cancel();
                    return;
                }
            }
        }
    }
}

/// 接收循环:收帧分派 / 出站帧写 sink / 心跳超时 / shutdown 打断。
#[allow(clippy::too_many_arguments)]
async fn serve_loop(
    cfg: &TunnelConfig,
    manager: &Arc<TunnelManager>,
    client: &reqwest::Client,
    local_port: u16,
    sink: &Arc<Mutex<TunnelSink>>,
    stream: &mut futures_util::stream::SplitStream<TunnelWs>,
    frame_tx: &mpsc::UnboundedSender<Frame>,
    frame_rx: &mut mpsc::UnboundedReceiver<Frame>,
    last_frame: &Arc<AtomicI64>,
    conn_shutdown: &CancellationToken,
    shutdown: &CancellationToken,
) -> Result<(), TunnelError> {
    loop {
        tokio::select! {
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        last_frame.store(now_monotonic_ms(), Ordering::Relaxed);
                        match serde_json::from_str::<Frame>(&text) {
                            Ok(Frame::Request { id, method, path, headers, body }) => {
                                // dispatch_one 内会记 `tunnel_request`(design §6.1),
                                // 这里不重复打日志。
                                let tx = frame_tx.clone();
                                let client = client.clone();
                                let manager = manager.clone();
                                tokio::spawn(async move {
                                    dispatcher::dispatch_one(
                                        id, method, path, headers, body, local_port, client, tx, manager,
                                    )
                                    .await;
                                });
                            }
                            Ok(frame @ Frame::Response { id, .. }) => {
                                // 配对 RPC 等内部请求的回复:pending 表路由
                                if let Some(tx) = manager.pending_remove(id) {
                                    let _ = tx.send(frame);
                                } else {
                                    tracing::warn!(target: TUNNEL_TARGET, id, "Response for unknown/unmatched request id");
                                }
                            }
                            Ok(Frame::Stream { id, event }) => {
                                // remote → PC 的 Stream 帧 = **取消信号**
                                // (S3,D3 不加新帧):remote 侧手机断 SSE /
                                // node 离线清理 → 发 End/Error → 停转发。
                                // 不存在该 id 则忽略(log,cancel_stream 幂等)。
                                match event {
                                    everlasting_remote_protocol::StreamEvent::End
                                    | everlasting_remote_protocol::StreamEvent::Error { .. } => {
                                        manager.cancel_stream(id);
                                    }
                                    everlasting_remote_protocol::StreamEvent::Chunk { .. } => {
                                        // PC 不消费 remote 推的 Chunk(协议上
                                        // 无此方向)—— 异常,只 log
                                        tracing::warn!(target: TUNNEL_TARGET, id, "unexpected Stream::Chunk from remote");
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!(target: TUNNEL_TARGET, error = %e, "dropping unparseable frame");
                            }
                        }
                    }
                    Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) | Some(Ok(Message::Binary(_))) => {
                        // 续期:Ping 由 tungstenite read() 自动回 Pong(协议层),
                        // 这里只需记帧。
                        last_frame.store(now_monotonic_ms(), Ordering::Relaxed);
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        return Err(TunnelError::Transient("remote closed connection".to_string()));
                    }
                    Some(Err(e)) => {
                        return Err(TunnelError::Transient(format!("ws read error: {e}")));
                    }
                    Some(Ok(_)) => {}
                }
            }
            frame = frame_rx.recv() => {
                match frame {
                    Some(frame) => send_frame(sink, &frame).await?,
                    None => {
                        return Err(TunnelError::Transient("frame channel closed".to_string()));
                    }
                }
            }
            _ = conn_shutdown.cancelled() => {
                return Err(TunnelError::Transient(format!(
                    "heartbeat timeout (no frame within {}s)",
                    HEARTBEAT_STALE_MS / 1000
                )));
            }
            _ = shutdown.cancelled() => {
                // 优雅关闭:先发 Close 让 remote 立刻感知,再退出
                tracing::info!(target: TUNNEL_TARGET, node_id = %cfg.node_id, "tunnel shutdown requested, closing connection");
                let _ = sink.lock().await.send(Message::Close(None)).await;
                return Ok(());
            }
        }
    }
}

/// 序列化 + 写 sink。
async fn send_frame(sink: &Arc<Mutex<TunnelSink>>, frame: &Frame) -> Result<(), TunnelError> {
    let text = serde_json::to_string(frame)
        .map_err(|e| TunnelError::Transient(format!("serialize frame: {e}")))?;
    sink.lock()
        .await
        .send(Message::Text(text))
        .await
        .map_err(|e| TunnelError::Transient(format!("ws send failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::build_ws_url;
    use crate::daemon::tunnel::TunnelConfig;

    fn cfg(remote_url: &str) -> TunnelConfig {
        TunnelConfig {
            remote_url: remote_url.to_string(),
            shared_secret: "s3cr3t/+&中".to_string(),
            node_id: "company-pc".to_string(),
            display_name: "公司 PC".to_string(),
        }
    }

    /// P2-1 回归:中文 display_name / 特殊字符 secret 必须 percent-encode,
    /// 且 URL 结构仍是 `{base}/ws?secret=&node_id=&display_name=`。
    #[test]
    fn build_ws_url_percent_encodes_query_values() {
        let url = build_ws_url(&cfg("wss://remote.example.com"));
        assert!(url.starts_with("wss://remote.example.com/ws?secret="));
        // secret "s3cr3t/+&中" —— 非字母数字全被编码,query 值不含裸 '/'
        assert!(url.contains("secret=s3cr3t%2F%2B%26%E4%B8%AD"));
        // node_id 中 '-' 也被 NON_ALPHANUMERIC 编码成 %2D(服务端
        // percent-decode 后无影响;用最保守的编码集,零解析风险)
        assert!(url.contains("node_id=company%2Dpc"));
        // display_name 中文必须编码(空格 → %20)
        assert!(url.contains("display_name=%E5%85%AC%E5%8F%B8%20PC"));
    }

    #[test]
    fn build_ws_url_keeps_ws_scheme_for_local_debug() {
        let url = build_ws_url(&cfg("ws://localhost:7457"));
        assert!(url.starts_with("ws://localhost:7457/ws?secret="));
    }

    /// 编码后的 query 能 percent-decode 回原值(S1 服务端 axum `Query`
    /// 提取器自动 decode,等价于这里的 `percent_decode_str`)。
    #[test]
    fn encoded_query_decodes_roundtrip() {
        let url = build_ws_url(&cfg("ws://localhost:7457"));
        let query = url.split('?').nth(1).unwrap();
        let decode = |key: &str| {
            let pair = query
                .split('&')
                .find(|p| p.starts_with(&format!("{key}=")))
                .expect("key present");
            percent_encoding::percent_decode_str(&pair[key.len() + 1..])
                .decode_utf8()
                .expect("valid utf-8")
                .into_owned()
        };
        assert_eq!(decode("secret"), "s3cr3t/+&中");
        assert_eq!(decode("node_id"), "company-pc");
        assert_eq!(decode("display_name"), "公司 PC");
    }
}
