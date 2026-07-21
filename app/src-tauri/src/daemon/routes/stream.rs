//! `GET /api/v1/stream` — 单全局 SSE 事件流(P2.3 C2).
//!
//! daemon 的所有 agent-loop 事件(parent 的 7 个 channel + worker 的
//! `subagent:event` / `subagent:finished`)都经 [`crate::daemon::sse`]
//! 的 [`SseRegistry`] 汇聚,本 handler 是它们流向浏览器的唯一出口。
//!
//! 连接生命周期:
//! 1. 解析客户端重连时回传的 `Last-Event-ID` 头(首次连接无此头
//!    → `None` → 不回放历史,前端自己在 `httpTransport.listen`
//!    连上后 GET snapshot 拉当前状态)。
//! 2. `state.sse.subscribe(last)` → [`SseSubscription`] `{ replay,
//!    live }`。replay 是重连时应先行重放的帧切片(或单条 resync
//!    sentinel),live 是后续实时事件的 mpsc channel。
//! 3. `tokio_stream::iter(replay).chain(ReceiverStream::new(live))`
//!    把两者拼成单个 SSE 流——**replay 不占 live channel 容量**
//!    (重连回放一整段 buffer 时不会因 channel 满误踢自己)。
//! 4. `KeepAlive` 每 30s 发一条 `:ping` 注释帧,防止代理 / LB 因
//!    空闲断连;断网后浏览器 `EventSource` 自动重连并带上最后的
//!    `Last-Event-ID`,registry 据此回放或发 sentinel。
//!
//! 每条 [`SseFrame`] 转成一个 `axum::response::sse::Event`:
//! `id:` = 全局递增 u64、`event:` = 前端 listen 订阅的事件名、
//! `data:` = JSON 字符串。前端 `httpTransport` 的单全局
//! `EventSource` 收到后按 `event` 名分发到对应 handler。

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use axum::Router;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use crate::daemon::sse::{SseFrame, SseSubscription};
use crate::state::AppState;

/// SSE 心跳间隔。design §C2:30s `: ping` 注释帧;浏览器
/// `EventSource` 默认无超时,但中间代理 / Cloudflare Tunnel(P3)
/// 可能在更长空闲后断连。
const KEEPALIVE_INTERVAL_SECS: u64 = 30;

/// `GET /api/v1/stream` handler。
///
/// 返回 `Sse<impl Stream<...>>`——axum 的 `Sse` 实现 `IntoResponse`,
/// 自动设 `Content-Type: text/event-stream` + `Cache-Control:
/// no-cache` + 逐帧 flush。stream body 的类型用 `impl Trait` 隐藏
/// `Chain<Map<...>>` 具体形态。
pub async fn stream(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let last_event_id = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());
    let SseSubscription { replay, live } = state.sse.subscribe(last_event_id);
    let frames = tokio_stream::iter(replay)
        .chain(ReceiverStream::new(live))
        .map(frame_to_event);
    Sse::new(frames).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(KEEPALIVE_INTERVAL_SECS))
            .text("ping"),
    )
}

/// 把一条 [`SseFrame`] 转成 axum SSE `Event`。`data` 是
/// `serde_json::to_string` 产物(无裸换行),`event` / `id` 也是
/// 固定形态,所以 `Event::data` 的换行断言不会触发。
fn frame_to_event(f: SseFrame) -> Result<Event, Infallible> {
    Ok(Event::default()
        .event(f.event)
        .data(f.data)
        .id(f.id.to_string()))
}

/// 子路由装配。对齐其他 domain 的 `router(state)` 模式
/// (`Router::new().route(...).with_state(state)`):在 state 未绑定时
/// 注册取 `State<Arc<AppState>>` 的 handler,再 `with_state` 绑定,
/// 返回 `Router<()>` 供 [`super::router`] `merge`。路由是绝对路径
/// `/api/v1/stream`(顶级 GET,不 nest 在 domain 前缀下),所以用
/// `merge` 而非 `nest`。
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/v1/stream", get(stream))
        .with_state(state)
}
