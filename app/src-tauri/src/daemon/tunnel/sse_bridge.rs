//! SSE 流式转发(design §2.3 / implement.md Step 4 / S3 Step 4)。
//!
//! 检测到 `Content-Type: text/event-stream` 的 loopback 响应后:
//! reqwest body(`Stream<Item = Result<Bytes>>`,hyper chunked)逐 chunk
//! 读 → 每段包 `Stream::Chunk` 塞回 WSS → 正常结束 `Stream::End` →
//! 读错 `Stream::Error { message }`。
//!
//! **纯字节透传**:chunk 边界**不一定**对齐 SSE event 边界
//! (`id:...\nevent:...\ndata:...\n\n` 可能被拆到两个 chunk)—— 没关系,
//! remote 侧把 Chunk bytes 原样写给手机的 SSE HTTP body,浏览器
//! `EventSource` 自己按 `\n\n` 解析(Q-T3)。
//!
//! **取消(S3)**:外层 `select!` 监听 `CancellationToken` —— remote 侧
//! 手机断 SSE 后发 `Stream::End`(取消信号,D3),client.rs →
//! `manager.cancel_stream(id)` → token cancel → 本函数退出并
//! `drop(resp)` 断 loopback SSE 订阅。**agent 不停**(SseRegistry 是
//! broadcast,单订阅者断开只移除自己,D1 —— 停 agent 靠显式
//! `POST /api/v1/cancel`)。

use everlasting_remote_protocol::{Frame, StreamEvent};
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::daemon::tunnel::TUNNEL_TARGET;

/// 把 SSE 响应体逐 chunk 转 `Stream` 帧。发送失败(WSS 已断)即停;
/// `cancel` 触发(remote 发 End / 手机断)→ 停转发 + drop resp。
pub async fn forward_stream(
    id: u64,
    resp: reqwest::Response,
    tx: mpsc::UnboundedSender<Frame>,
    cancel: CancellationToken,
) {
    tracing::info!(target: TUNNEL_TARGET, id, "tunnel_stream_start");
    let mut stream = resp.bytes_stream();
    loop {
        tokio::select! {
            chunk = stream.next() => {
                match chunk {
                    Some(Ok(bytes)) => {
                        let frame = Frame::Stream {
                            id,
                            event: StreamEvent::Chunk {
                                bytes: bytes.to_vec(),
                            },
                        };
                        if tx.send(frame).is_err() {
                            // WSS 已断 —— 丢弃剩余流(remote 侧已给手机返 502)
                            return;
                        }
                    }
                    Some(Err(e)) => {
                        let frame = Frame::Stream {
                            id,
                            event: StreamEvent::Error {
                                message: e.to_string(),
                            },
                        };
                        let _ = tx.send(frame);
                        return;
                    }
                    None => {
                        let _ = tx.send(Frame::Stream {
                            id,
                            event: StreamEvent::End,
                        });
                        return;
                    }
                }
            }
            _ = cancel.cancelled() => {
                // 取消信号(remote 发 End / Error):停转发。drop(resp)
                // 断 reqwest 连接 → loopback SSE 订阅断(SseRegistry
                // broadcast 自动剔除该订阅者);agent 不停(D1)。
                tracing::debug!(target: TUNNEL_TARGET, id, "stream_cancelled reason=client_gone");
                return;
            }
        }
    }
}
