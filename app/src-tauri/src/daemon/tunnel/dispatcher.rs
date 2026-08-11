//! loopback 转发(design §2.2 / implement.md Step 4)。
//!
//! 收到 remote 的 `Frame::Request { id, method, path, headers, body }`:
//! 1. reqwest 打 `http://localhost:{local_port}{path}`(Q-T6:端口由
//!    `TunnelManager.local_port` 传入,**不硬编码 7456**;不调 handler
//!    函数、不绕过 axum —— Q7 决策)
//! 2. 非流式 → 读完 body → `Frame::Response { id, status, headers, body }`
//! 3. 流式(`Content-Type: text/event-stream`)→ [`sse_bridge`] 逐 chunk
//!    转 `Stream::Chunk` → `End` / `Error`
//!
//! header 透传规则(design §2.2):
//! - `Authorization` / `?access_token=` 由 **remote 侧**剥净(S1 契约单源,
//!   P1-3 修订 —— dispatcher 不重复实现剥离逻辑,残留就原样透传);
//! - `Content-Type` / `Accept` / `Last-Event-ID`(SSE 续传关键)透传;
//! - `Host` / `Connection` / `Transfer-Encoding` 不转发(loopback 的
//!   Host 由 reqwest 按 URL 生成,hop-by-hop 头跨隧道无意义)。
//!
//! 失败模式(design §6.2):loopback 不通(不该发生,同进程)→ 502;WSS 已断
//! → 帧发送失败,丢弃响应(remote 侧已给手机返 502)。

use axum::http::{HeaderName, HeaderValue, Method};
use everlasting_remote_protocol::Frame;
use reqwest::header::CONTENT_TYPE;
use tokio::sync::mpsc;

use crate::daemon::tunnel::{sse_bridge, TUNNEL_TARGET};

/// 单请求转发(serve loop 每收一个 `Frame::Request` spawn 一个)。
#[allow(clippy::too_many_arguments)]
pub async fn dispatch_one(
    id: u64,
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    local_port: u16,
    client: reqwest::Client,
    tx: mpsc::UnboundedSender<Frame>,
) {
    tracing::info!(target: TUNNEL_TARGET, id, method = %method, path = %path, "tunnel_request");

    let url = format!("http://localhost:{local_port}{path}");
    let Ok(method) = Method::from_bytes(method.as_bytes()) else {
        send_response(&tx, id, 400, Vec::new(), b"invalid method".to_vec()).await;
        return;
    };
    let mut req = client.request(method, &url);
    for (name, value) in &headers {
        let lower = name.to_ascii_lowercase();
        // Host 由 reqwest 按 loopback URL 生成;hop-by-hop 头不转发。
        if matches!(
            lower.as_str(),
            "host" | "connection" | "transfer-encoding" | "keep-alive" | "upgrade"
        ) {
            continue;
        }
        let Ok(name) = HeaderName::from_bytes(name.as_bytes()) else {
            continue;
        };
        let Ok(value) = HeaderValue::from_str(value) else {
            continue;
        };
        req = req.header(name, value);
    }
    if !body.is_empty() {
        req = req.body(body);
    }

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            // loopback 不通(不该发生,同进程)→ 502 给 remote
            tracing::warn!(target: TUNNEL_TARGET, id, error = %e, "loopback request failed");
            send_response(
                &tx,
                id,
                502,
                Vec::new(),
                format!("loopback request failed: {e}").into_bytes(),
            )
            .await;
            return;
        }
    };

    let status = resp.status().as_u16();
    let is_sse = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.starts_with("text/event-stream"))
        .unwrap_or(false);

    if is_sse {
        // SSE 流式路径(design §2.3):纯字节透传,不解析 SSE 语义
        sse_bridge::forward_stream(id, resp, tx).await;
    } else {
        let headers = response_headers(resp.headers());
        match resp.bytes().await {
            Ok(bytes) => {
                send_response(&tx, id, status, headers, bytes.to_vec()).await;
            }
            Err(e) => {
                tracing::warn!(target: TUNNEL_TARGET, id, error = %e, "loopback body read failed");
                send_response(
                    &tx,
                    id,
                    502,
                    Vec::new(),
                    format!("loopback body read failed: {e}").into_bytes(),
                )
                .await;
            }
        }
    }
}

/// 非流式回包。发送失败 = WSS 已断 → 丢弃(design §6.2)。
async fn send_response(
    tx: &mpsc::UnboundedSender<Frame>,
    id: u64,
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
) {
    tracing::info!(target: TUNNEL_TARGET, id, status, "tunnel_response");
    let frame = Frame::Response {
        id,
        status,
        headers,
        body,
    };
    if tx.send(frame).is_err() {
        tracing::debug!(target: TUNNEL_TARGET, id, "response dropped (tunnel connection gone)");
    }
}

/// 响应 header → 帧头列表:跳 hop-by-hop 头(缓冲完 body 后连接级头无意义),
/// 其余原样(含 `Content-Type` / `Last-Event-ID` 等)。
fn response_headers(headers: &reqwest::header::HeaderMap) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (name, value) in headers {
        let lower = name.as_str().to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "connection"
                | "transfer-encoding"
                | "keep-alive"
                | "upgrade"
                | "proxy-connection"
                | "te"
        ) {
            continue;
        }
        if let Ok(v) = value.to_str() {
            out.push((name.as_str().to_string(), v.to_string()));
        }
    }
    out
}
