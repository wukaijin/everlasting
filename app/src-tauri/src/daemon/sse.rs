//! Phase 2.3 C1 — SSE event sink + global registry + replay buffer
//! (task `07-20-remote-access-daemon-split`, 2026-07-21).
//!
//! The daemon's answer to Tauri's `app.emit`. The parent agent loop
//! (`run_chat_loop`) and the worker dispatch (`run_subagent`) emit
//! through [`crate::state::ChatEventSink`] / [`SubagentEventSink`]
//! trait objects; in the Tauri build those are `AppHandleSink` /
//! `AppHandleSubagentSink` (forwarding to `app.emit`). In the daemon
//! build they are [`HttpSseSink`] / [`HttpSseSubagentSink`] defined
//! here, which push each event into a single [`SseRegistry`]. The
//! `GET /api/v1/stream` handler (C2) pulls frames out of the registry
//! and writes them as SSE frames to every connected `EventSource`.
//!
//! # 设计要点(相对 design.md §C1 的两处简化)
//!
//! 调研稿假设 payload 带 `session_id`、registry 按
//! `event_name → Vec<Sender>` 分发。代码核实后两者都不成立 / 不
//! 必要,本实现做了对应简化(详见 [`SseRegistry`] 文档):
//!
//! 1. **全局单流 + `request_id` 路由**:`ChatEventPayload` /
//!    `ToolCallPayload` / `ToolResultPayload` 等 7 类 chat 事件
//!    payload 只带 `request_id`,不带 `session_id`。因此 buffer
//!    是**全局的**(所有 session 的事件混排),前端靠
//!    `streamController` 现有的 `request_id → session` 路由过滤,
//!    与 Tauri 全局 `emit` 语义完全一致。design §1.3 设想的
//!    "session_id 双重过滤"在当前 payload 形状下不成立。
//! 2. **`Vec<Sender>` 而非 `HashMap<event_name, Vec<Sender>>`**:
//!    `/api/v1/stream` 是单全局流,每条事件广播给所有连接;
//!    event-name 维度的订阅过滤在**前端** `httpTransport.listen`
//!    的分发表里完成(一个 `EventSource` 收全部 event,按 name
//!    路由到 handler)。
//!
//! # replay 不占 live channel 容量
//!
//! [`SseRegistry::subscribe`] 返回 `(replay, live)`:重连时应先行
//! 重放的帧切片 + 后续 live 事件的 channel。C2 的 stream handler
//! 用 `futures::stream::iter(replay).chain(ReceiverStream::new(live))`
//! 拼成单个 SSE 流——相比"把 replay 也 `try_send` 进 channel",
//! 这样重连回放一整段 buffer 时不会因为 channel 满而误踢自己。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tokio::sync::mpsc;

use crate::agent::permissions::PermissionAskPayload;
use crate::agent::question_store::{
    ModeChangePayload, TaskStateTransitionPayload, ToolQuestionPayload,
};
use crate::agent::subagent::{
    build_subagent_event_payload, build_subagent_finished_payload, SubagentEventSink, TranscriptKind,
};
use crate::state::{ChatEventPayload, ChatEventSink, ToolCallPayload, ToolResultPayload};

/// Replay buffer 上限(design §C1 / §2.2)。超过则淘汰最旧的;
/// 此时若某重连客户端的 `Last-Event-ID` 落在被淘汰的范围里,
/// [`SseRegistry::subscribe`] 改发 [`SENTINEL_EVENT`] resync 信号。
pub const BUFFER_CAPACITY: usize = 512;

/// 单条事件 > 此阈值则**不入** replay buffer(仍直推 live
/// channel,但不参与断网重连回放)。design §C1 / §2.2:5 MiB
/// shell 输出这类大 message 期间断网不应撑爆 buffer;前端重连
/// 后走 snapshot 重拉,而非 buffer 回放。
pub const LARGE_PAYLOAD_THRESHOLD: usize = 256 * 1024;

/// 每个订阅者 live channel 的容量。订阅者跟得上时几乎不积压;
/// 持续落后填满后,`broadcast` 的 `try_send` 失败 → 该订阅者被
/// 剔除(下次重连靠 `Last-Event-ID` 回放或 resync)。
const LIVE_CHANNEL_CAPACITY: usize = 128;

/// Buffer 被淘汰导致 `Last-Event-ID` 失效时下发的 sentinel 事件
/// 名。前端 `httpTransport` 监听该事件 → GET
/// `/api/v1/sessions/{current}/snapshot` → 用快照替换 store 后
/// 重画 UI(design §2.4)。注意是全局信号(payload 无
/// `session_id`,前端用当前活跃 session 自己调 snapshot)。
const SENTINEL_EVENT: &str = "stream-resync";

const SENTINEL_DATA: &str = r#"{"reason":"buffer_overrun"}"#;

/// 一条已序列化的 SSE 帧。`id` 是全局单调递增的 u64(SSE `id:`
/// 行,供客户端 `Last-Event-ID` 头回传);`event` 是前端 `listen`
/// 订阅的事件名(如 `chat-event` / `tool:call` / `subagent:event`);
/// `data` 是 JSON 字符串。C2 handler 把它转成 `axum::response::sse::Event`。
#[derive(Clone, Debug)]
pub struct SseFrame {
    pub id: u64,
    pub event: String,
    pub data: String,
}

/// [`SseRegistry::subscribe`] 的返回值:重连时应先行重放的帧
/// (buffer 的相关切片,或单条 resync sentinel)+ 后续 live 事件
/// 的 channel。详见模块文档"replay 不占 live channel 容量"。
pub struct SseSubscription {
    pub replay: Vec<SseFrame>,
    pub live: mpsc::Receiver<SseFrame>,
}

struct RegistryInner {
    senders: Vec<mpsc::Sender<SseFrame>>,
    buffer: VecDeque<SseFrame>,
    next_id: u64,
}

/// 进程级 SSE 分发中心。daemon 单实例,挂在 `AppState` 上(C4),
/// 被 [`HttpSseSink`] / [`HttpSseSubagentSink`] 以 `Arc` 引用持有。
///
/// 所有状态藏在一把 `std::sync::Mutex` 后面。`broadcast` /
/// `subscribe` 都是**同步**操作(锁内只做内存操作 + 非阻塞
/// `try_send`,绝不跨 `.await`),所以用轻量的 std `Mutex` 而非
/// `tokio::sync::Mutex`。Poison 时显式 `into_inner` 取回(不 panic)
/// ——agent loop 的一次 emit 失败不该拖垮整个 daemon。
pub struct SseRegistry {
    inner: Mutex<RegistryInner>,
}

impl SseRegistry {
    /// 新建空 registry(`next_id` 从 1 开始,`id: 0` 留给 sentinel)。
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(RegistryInner {
                senders: Vec::new(),
                buffer: VecDeque::new(),
                next_id: 1,
            }),
        }
    }

    /// 广播一条事件:分配全局递增 `id` → 序列化 payload →
    /// (大 message 除外)入 replay buffer + 淘汰超限 → fan-out
    /// `try_send` 给每个订阅者,发送失败(接收端 dropped 或
    /// channel 满)的订阅者直接剔除。
    ///
    /// 同步、非阻塞,适合在 agent loop 的同步 `emit_*` 路径里
    /// 直接调用。锁持有时间 = 序列化 + buffer 操作 + N 次
    /// `try_send`(O(订阅者数));单用户场景订阅者数极小。
    pub fn broadcast(&self, event: &str, payload: &impl Serialize) {
        let data = serde_json::to_string(payload).unwrap_or_default();
        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let id = inner.next_id;
        inner.next_id = inner.next_id.wrapping_add(1);
        let frame = SseFrame {
            id,
            event: event.to_string(),
            data,
        };
        // 大 message 不入 buffer(参见 LARGE_PAYLOAD_THRESHOLD)。
        if frame.data.len() <= LARGE_PAYLOAD_THRESHOLD {
            inner.buffer.push_back(frame.clone());
            while inner.buffer.len() > BUFFER_CAPACITY {
                inner.buffer.pop_front();
            }
        }
        // fan-out:try_send 失败的订阅者剔除——下次重连靠
        // Last-Event-ID / resync。
        inner
            .senders
            .retain(|tx| tx.try_send(frame.clone()).is_ok());
    }

    /// 注册一个新订阅者。`last_event_id` 来自客户端重连时的
    /// `Last-Event-ID` HTTP 头(首次连接为 `None`)。
    ///
    /// 返回 [`SseSubscription`]:replay 切片 + live channel。
    /// - `None` → 空 replay(新连接从"现在"开始,当前状态由前端
    ///   自己在 `httpTransport.listen` 连上后 GET snapshot 拉取)。
    /// - `Some(last)` 且 buffer 非空:
    ///   - `last + 1 < buffer_oldest_id`(last 与 oldest 之间存在被
    ///     淘汰的 gap)→ replay = 单条 [`SENTINEL_EVENT`] sentinel
    ///     (前端走 snapshot 重拉)。`last + 1 >= oldest`(含相邻与
    ///     `last == 0` 首次回放)视为连续。
    ///   - 否则 → replay = buffer 中 `id > last` 的帧(可能为空,
    ///     表示客户端已是最新)。
    pub fn subscribe(&self, last_event_id: Option<u64>) -> SseSubscription {
        let (tx, rx) = mpsc::channel(LIVE_CHANNEL_CAPACITY);
        let replay = {
            let mut inner = match self.inner.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            let replay = compute_replay(&inner.buffer, last_event_id);
            inner.senders.push(tx);
            replay
        };
        SseSubscription { replay, live: rx }
    }

    /// 活跃订阅者数(health / 监控 metric 用,P2.5 可挂到 health
    /// 端点的 `metrics` 字段)。
    pub fn subscriber_count(&self) -> usize {
        match self.inner.lock() {
            Ok(g) => g.senders.len(),
            Err(p) => p.into_inner().senders.len(),
        }
    }
}

impl Default for SseRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// 计算 `subscribe` 时应回放的帧切片(模块文档"replay 协议")。
fn compute_replay(buffer: &VecDeque<SseFrame>, last_event_id: Option<u64>) -> Vec<SseFrame> {
    let Some(last) = last_event_id else {
        // 首次连接:不回放历史。
        return Vec::new();
    };
    if buffer.is_empty() {
        return Vec::new();
    }
    let oldest = buffer.front().expect("non-empty checked").id;
    // overrun 判定:客户端 `last` 与 buffer `oldest` 之间存在 gap
    // (被淘汰的帧)→ 整段无法完整回放,发 sentinel 让前端走 snapshot
    // 重拉。`last + 1 < oldest` 等价于"last 之后到 oldest 之间至少缺
    // 一帧";`last + 1 >= oldest`(含 `last == oldest - 1` 的相邻情形,
    // 以及 `last == 0` 首次回放)视为连续,回放 (last, newest]。
    if last.saturating_add(1) < oldest {
        return vec![SseFrame {
            id: 0,
            event: SENTINEL_EVENT.to_string(),
            data: SENTINEL_DATA.to_string(),
        }];
    }
    buffer.iter().filter(|f| f.id > last).cloned().collect()
}

// ---------------------------------------------------------------------------
// HttpSseSink — parent agent loop 的 `ChatEventSink` 实现
// ---------------------------------------------------------------------------

/// HTTP/SSE 版 [`crate::state::AppHandleSink`]。注入 `run_chat_loop`
/// (C5),把 7 个 channel 的事件全部 `broadcast` 到 [`SseRegistry`];
/// 前端 `httpTransport` 的单全局 `EventSource` 收到后按 event name
/// 分发。`emit_permission_ask_resolved` 沿用 trait 默认 no-op(只
/// `SubagentBufferSink` 覆盖它,parent sink 不需要)。
pub struct HttpSseSink {
    pub registry: Arc<SseRegistry>,
}

impl ChatEventSink for HttpSseSink {
    fn emit_chat_event(&self, payload: &ChatEventPayload) {
        self.registry.broadcast("chat-event", payload);
    }
    fn emit_tool_call(&self, payload: &ToolCallPayload) {
        self.registry.broadcast("tool:call", payload);
    }
    fn emit_tool_result(&self, payload: &ToolResultPayload) {
        self.registry.broadcast("tool:result", payload);
    }
    fn emit_permission_ask(&self, payload: PermissionAskPayload) {
        self.registry.broadcast("permission:ask", &payload);
    }
    fn emit_tool_question(&self, payload: &ToolQuestionPayload) {
        self.registry.broadcast("tool:question", payload);
    }
    fn emit_mode_change_request(&self, payload: &ModeChangePayload) {
        self.registry.broadcast("mode:change:request", payload);
    }
    fn emit_task_state_transition(&self, payload: &TaskStateTransitionPayload) {
        self.registry
            .broadcast("task:state:transition:request", payload);
    }
}

// ---------------------------------------------------------------------------
// HttpSseSubagentSink — worker 的 `SubagentEventSink` 实现
// ---------------------------------------------------------------------------

/// HTTP/SSE 版 [`crate::agent::subagent::AppHandleSubagentSink`]。
/// 注入 `run_subagent`(C5 完整版),复用
/// [`build_subagent_event_payload`] / [`build_subagent_finished_payload`]
/// 保证与 Tauri 路径**完全一致**的 `subagent:event` /
/// `subagent:finished` wire shape(design §7"Phase 1 契约对齐")。
pub struct HttpSseSubagentSink {
    pub registry: Arc<SseRegistry>,
}

impl SubagentEventSink for HttpSseSubagentSink {
    fn emit_subagent_event(
        &self,
        run_id: &str,
        session_id: &str,
        kind: TranscriptKind,
        payload_json: serde_json::Value,
    ) {
        let ipc = build_subagent_event_payload(run_id, session_id, kind, payload_json);
        self.registry.broadcast("subagent:event", &ipc);
    }
    fn emit_subagent_finished(
        &self,
        run_id: &str,
        session_id: &str,
        status_db: &str,
        finished_at: &str,
    ) {
        let ipc = build_subagent_finished_payload(run_id, session_id, status_db, finished_at);
        self.registry.broadcast("subagent:finished", &ipc);
    }
    fn emit_permission_ask(&self, payload: &PermissionAskPayload) {
        self.registry.broadcast("permission:ask", payload);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    #[derive(Serialize)]
    struct DummyPayload {
        request_id: String,
        text: String,
    }

    fn payload(text: &str) -> DummyPayload {
        DummyPayload {
            request_id: "r1".to_string(),
            text: text.to_string(),
        }
    }

    /// `broadcast` 在零订阅者时不 panic,事件静默入 buffer。
    #[tokio::test]
    async fn broadcast_with_no_subscribers_just_buffers() {
        let reg = SseRegistry::new();
        reg.broadcast("chat-event", &payload("hello"));
        assert_eq!(reg.subscriber_count(), 0);
        // 首次连接 last=None 不回放,所以这里只能间接确认 buffer
        // 非空:订阅 last=0 会回放 id>0 的帧(此事件 id=1)。
        let sub = reg.subscribe(Some(0));
        assert_eq!(sub.replay.len(), 1);
        assert_eq!(sub.replay[0].event, "chat-event");
    }

    /// 活跃订阅者实时收到 live 广播。
    #[tokio::test]
    async fn subscriber_receives_live_broadcast() {
        let reg = SseRegistry::new();
        let mut sub = reg.subscribe(None);
        assert_eq!(reg.subscriber_count(), 1);
        reg.broadcast("chat-event", &payload("hi"));
        let frame = sub.live.recv().await.expect("got frame");
        assert_eq!(frame.event, "chat-event");
        assert!(frame.data.contains("hi"), "data carries payload: {}", frame.data);
        assert!(frame.id >= 1);
    }

    /// 首次连接(last=None)不回放历史。
    #[tokio::test]
    async fn replay_first_connection_is_empty() {
        let reg = SseRegistry::new();
        reg.broadcast("chat-event", &payload("m0"));
        reg.broadcast("chat-event", &payload("m1"));
        let sub = reg.subscribe(None);
        assert!(sub.replay.is_empty());
    }

    /// `Some(last)` 回放 buffer 中 `id > last` 的帧。
    #[tokio::test]
    async fn replay_returns_frames_after_last_event_id() {
        let reg = SseRegistry::new();
        for i in 0..5 {
            reg.broadcast("chat-event", &payload(&format!("m{i}")));
        }
        // id 1..=5;回放 id > 2 → id 3,4,5。
        let sub = reg.subscribe(Some(2));
        assert_eq!(sub.replay.len(), 3);
        assert_eq!(sub.replay[0].id, 3);
        assert_eq!(sub.replay[2].id, 5);
    }

    /// buffer 淘汰最旧帧:填入 `BUFFER_CAPACITY + 10` 条后,
    /// `oldest` 推进到 11;`last=10` 触发 sentinel,`last=11`
    /// 回放剩余 511 条(`id 12..=522`)。
    #[tokio::test]
    async fn buffer_cap_drops_oldest() {
        let reg = SseRegistry::new();
        for _ in 0..(BUFFER_CAPACITY + 10) {
            // id 1..=522;buffer keeps last 512 → id 11..=522.
            reg.broadcast("chat-event", &payload("x"));
        }
        // last=10 与 oldest(11)相邻 → 无 gap → 回放 (10, 522] = id 11..=522.
        let s_full = reg.subscribe(Some(10));
        assert_eq!(s_full.replay.len(), 512);
        assert_eq!(s_full.replay.first().unwrap().id, 11);
        assert_eq!(s_full.replay.last().unwrap().id, 522);
        // last=9 → 9 与 oldest(11) 之间缺 id=10(gap)→ sentinel.
        let s_sentinel = reg.subscribe(Some(9));
        assert_eq!(s_sentinel.replay.len(), 1);
        assert_eq!(s_sentinel.replay[0].event, SENTINEL_EVENT);
        assert_eq!(s_sentinel.replay[0].data, SENTINEL_DATA);
        // last=11(=oldest)→ 回放 (11, 522] = id 12..=522 (511 帧).
        let s_partial = reg.subscribe(Some(11));
        assert_eq!(s_partial.replay.len(), 511);
        assert_eq!(s_partial.replay.first().unwrap().id, 12);
    }

    /// 单条 > [`LARGE_PAYLOAD_THRESHOLD`] 的事件:live channel 照推,
    /// 但不入 buffer(重连后不回放,前端走 snapshot)。
    #[tokio::test]
    async fn large_payload_skips_buffer_but_reaches_live() {
        let reg = SseRegistry::new();
        let mut sub = reg.subscribe(None);
        let big_text = "x".repeat(LARGE_PAYLOAD_THRESHOLD + 1);
        reg.broadcast("tool:result", &payload(&big_text));
        // live 收到大帧。
        let frame = sub.live.recv().await.expect("live got large frame");
        assert_eq!(frame.event, "tool:result");
        assert!(frame.data.len() > LARGE_PAYLOAD_THRESHOLD);
        // 但 buffer 不含它:本次仅广播这一条(未入 buffer)→
        // buffer 空 → subscribe(Some(0)) 回放为空。
        let sub2 = reg.subscribe(Some(0));
        assert!(
            sub2.replay.is_empty(),
            "large payload must not enter the replay buffer"
        );
    }

    /// 持续落后填满 live channel 的订阅者被 fan-out 剔除。
    #[tokio::test]
    async fn slow_subscriber_is_dropped_from_fanout() {
        let reg = SseRegistry::new();
        let _sub = reg.subscribe(None); // 从不 recv
        assert_eq!(reg.subscriber_count(), 1);
        // 灌到远超 live channel 容量(128)。前 128 条填满 channel,
        // 第 129 条起 try_send 失败 → retain 剔除该订阅者。
        for _ in 0..(LIVE_CHANNEL_CAPACITY + BUFFER_CAPACITY + 10) {
            reg.broadcast("chat-event", &payload("flood"));
        }
        assert_eq!(reg.subscriber_count(), 0);
    }
}
