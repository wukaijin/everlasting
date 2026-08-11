//! SSE 流式转发(design §2.3 / implement.md Step 4)。
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
//! 取消传播(手机断开 → 停转发)留 S3 联调;本任务只保证取消**不**传到
//! agent loop(SseRegistry 是 broadcast,单订阅者断开不影响 agent)。

use everlasting_remote_protocol::{Frame, StreamEvent};
use futures_util::StreamExt;
use tokio::sync::mpsc;

use crate::daemon::tunnel::TUNNEL_TARGET;

/// 把 SSE 响应体逐 chunk 转 `Stream` 帧。发送失败(WSS 已断)即停。
pub async fn forward_stream(id: u64, resp: reqwest::Response, tx: mpsc::UnboundedSender<Frame>) {
    tracing::info!(target: TUNNEL_TARGET, id, "tunnel_stream_start");
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => {
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
            Err(e) => {
                let frame = Frame::Stream {
                    id,
                    event: StreamEvent::Error {
                        message: e.to_string(),
                    },
                };
                let _ = tx.send(frame);
                return;
            }
        }
    }
    let _ = tx.send(Frame::Stream {
        id,
        event: StreamEvent::End,
    });
}
