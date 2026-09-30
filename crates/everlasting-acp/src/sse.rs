//! SSE 消费任务:GET `/api/v1/stream`(design §3/§4)。
//!
//! **进程级单例,main 启动即挂、全程保持、永不主动断**(缺口 1 的 MVP 规避:
//! 在途 permission ask 无恢复面,daemon 对零订阅者的 ask 快拒 —— shim 必须让
//! daemon 恒有 ≥1 个订阅者)。断线自动重连(指数退避 1s→30s 封顶,携带
//! `Last-Event-ID` 让 daemon 走 512 帧 replay buffer 回放);重连失败只记日志
//! 持续重试,绝不退出进程(编辑器 session 可能长存)。
//!
//! wire 契约(对照 `app/src-tauri/src/daemon/sse.rs` + `routes/stream.rs`):
//! - 帧格式 `id: <u64>` / `event: <name>` / `data: <json 单行>`;daemon 侧
//!   data 恒单行(serde_json::to_string),解析仍按 SSE 规范容忍多 data 行;
//! - keepalive 是 `:ping` 注释行(30s),解析层天然丢弃;
//! - 断线 sentinel `stream-resync`(id: 0,payload `{"reason":...}`)按
//!   spec `pattern-sse-resync-and-tests.md` 决策表由 daemon 下发 —— shim
//!   转成 [`DaemonEvent::Resync`] 供消费方记日志,**不推进** last_event_id
//!   (daemon 语义:sentinel 不推进 Last-Event-ID,见 sse.rs:98-100);
//! - 重连时 `Last-Event-ID` 头携带最后收到的正 id,回放/哨兵由 daemon 的
//!   `compute_replay` 裁决,shim 侧不做去重:每个帧按 wire id 推进
//!   `last_event_id`,而回放只含 `id > last` 的未收帧,每个事件跨重连至多
//!   投递一次;陈旧 turn 的回放帧不带活跃 rid,消费方过滤天然免疫。
//!
//! 归一后的 [`DaemonEvent`] 经 tokio broadcast 分发:prompt 消费循环、PR3
//! 权限桥各自订阅。**消费模型选型 = broadcast 方案(design §4 正选),不用
//! per-prompt 直连备选**:「启动即挂、全程保持」要求 prompt 间隙也有订阅者,
//! per-prompt 直连仍需一个常驻保活连接,等价于全局任务;且 PR3 的权限桥
//! 与 prompt 循环是两个并行消费者,broadcast 天然匹配。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use serde::Deserialize;
use tokio::sync::broadcast;

use crate::daemon::DaemonClient;
use crate::translate::{ChatEventDto, PermissionAskDto, ToolCallPayloadDto, ToolResultPayloadDto};

/// broadcast channel 容量。prompt 消费循环每收到一条立即转 ACP 通知(不做
/// 慢消费),正常永远打不满;给足余量让「消费方短暂让出」不丢帧。
const BROADCAST_CAPACITY: usize = 1024;

/// 重连退避:错误路径 1s 起指数翻倍,30s 封顶;干净 EOF 走固定短退避
/// (daemon 正常关流后立刻回来是常态,但别 hot-spin)。
const RETRY_BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const RETRY_BACKOFF_MAX: Duration = Duration::from_secs(30);
const RETRY_FLOOR: Duration = Duration::from_millis(250);

/// daemon 全局流的一条归一事件。字段命名不对称在此收口(缺口 5):
/// chat-event / tool 事件是 snake_case,permission:ask 是 camelCase,
/// 各自 DTO 锚死,casing 单测在 translate.rs。
#[derive(Debug, Clone)]
pub enum DaemonEvent {
    /// `chat-event`(snake_case payload,`ChatEventPayload` = rid + sid +
    /// flattened ChatEvent)。
    ChatEvent {
        request_id: String,
        session_id: String,
        event: ChatEventDto,
    },
    /// `tool:call`(终态一次性,`state.rs:770-776`)→ ACP tool_call(pending)。
    ToolCall(ToolCallPayloadDto),
    /// `tool:result`(`state.rs:786-794`)→ ACP tool_call_update(completed/failed)。
    ToolResult(ToolResultPayloadDto),
    /// `permission:ask`(**camelCase**,`agent/permissions/payload.rs:20-62`)。
    /// PR2 只立变体 + 记日志,反向请求环是 PR3(implement.md)。
    PermissionAsk(PermissionAskDto),
    /// `stream-resync` sentinel:daemon 无法证明事件连续(重启 / ring 淘汰 /
    /// stale id),消费方当信号处理。`reason == "restart"` 意味着 daemon 进程
    /// 已重启 —— 在途 turn 随进程死亡,done/error 永不再来,prompt 循环据此
    /// 立即报错收束(「prompt 必须最终 respond」,session.rs);唯一已知的
    /// 「继续等」分支是 `buffer_overrun`(ring 淘汰,daemon 仍活着,终态
    /// 随后续事件到达)。shim 不做 snapshot 自愈(无 GUI 状态)。
    Resync { reason: String },
    /// 其余事件(subagent:event / subagent:finished / mode:* / task:* 等)。
    /// shim 无消费面,保留事件名供日志定位。
    Other { event: String },
}

/// SSE 任务句柄:进程级单例的订阅入口 + 窗口化健康判据。
///
/// 健康判据 = **最近 [`HEALTHY_WINDOW`] 内收到过 daemon 的字节**(建连后的
/// 首个 chunk 起算,后续任意数据帧含 30s `:ping` 都刷新)。刻意不是「此刻
/// 在活连接内」的瞬时 flag:秒级断线由 daemon 的 Last-Event-ID replay
/// buffer 覆盖(512 帧),事件并不丢 —— prompt 真正该报错的是「长断线 /
/// daemon 宕机」(无字节持续到窗口过期),判据刻画的正是它。
#[derive(Clone)]
pub struct SseHandle {
    tx: broadcast::Sender<DaemonEvent>,
    last_ok_ms: Arc<AtomicU64>,
}

/// 健康窗口:最近这么久内收到过字节即视为流可用。**必须大于** daemon 的
/// keepalive 间隔(30s `:ping`,`routes/stream.rs` KEEPALIVE_INTERVAL_SECS):
/// 活连接在无事件空闲期只靠 ping 刷活,窗口若小于 ping 间隔(初版 10s 的
/// 实病),任何空闲超过窗口的健康连接都会被误判为死,prompt 全部被拒。
/// 上限取 ping 间隔 + 0.5× 余量:真断线(退避重连、无字节)最多 45s 判死。
const HEALTHY_WINDOW: Duration = Duration::from_secs(45);

impl SseHandle {
    /// spawn 全局消费任务(main 启动即调;重连失败不退进程)。
    pub fn spawn(daemon: DaemonClient) -> Self {
        let (tx, _rx) = broadcast::channel(BROADCAST_CAPACITY);
        let last_ok_ms = Arc::new(AtomicU64::new(0));
        let last_event_id = Arc::new(AtomicU64::new(0));
        tokio::spawn(run_reconnect_loop(
            daemon,
            tx.clone(),
            last_ok_ms.clone(),
            last_event_id,
        ));
        Self { tx, last_ok_ms }
    }

    /// 订阅归一事件流(消费方在 POST chat **之前**调用 —— RULE-SMOKE-001
    /// 的 shim 侧等价物:broadcast 订阅只能收到订阅之后的事件)。
    pub fn subscribe(&self) -> broadcast::Receiver<DaemonEvent> {
        self.tx.subscribe()
    }

    /// 窗口化健康:最近 [`HEALTHY_WINDOW`] 内收到过字节(建连 chunk 或
    /// 任意数据帧)即健康。prompt 前置检查用这个。
    pub fn is_healthy(&self) -> bool {
        let last = self.last_ok_ms.load(Ordering::Relaxed);
        fresh_within(last, system_millis(), HEALTHY_WINDOW)
    }

    /// 等待流变健康(重连循环一直在跑)。deadline 内恢复返回 true。
    pub async fn wait_healthy(&self, deadline: Duration) -> bool {
        let started = tokio::time::Instant::now();
        while !self.is_healthy() {
            if started.elapsed() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        true
    }
}

/// 重连主循环:连接 → 消费到 EOF/错误 → 退避 → 重连(带 Last-Event-ID)。
/// 干净 EOF(daemon graceful shutdown 关流)走固定短退避 —— 对端立即 EOF
/// 的场景(端口被别的服务占用等)不 hot-spin。
async fn run_reconnect_loop(
    daemon: DaemonClient,
    tx: broadcast::Sender<DaemonEvent>,
    last_ok_ms: Arc<AtomicU64>,
    last_event_id: Arc<AtomicU64>,
) {
    let mut backoff = RETRY_BACKOFF_INITIAL;
    loop {
        match connect_and_consume(&daemon, &tx, &last_ok_ms, &last_event_id).await {
            Ok(()) => {
                tracing::info!("daemon SSE stream ended cleanly, reconnecting");
                tokio::time::sleep(RETRY_FLOOR).await;
                backoff = RETRY_BACKOFF_INITIAL;
            }
            Err(err) => {
                tracing::warn!(
                    backoff_secs = backoff.as_secs(),
                    "daemon SSE stream error: {err}; retrying"
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(RETRY_BACKOFF_MAX);
            }
        }
    }
}

/// 单次连接的完整生命周期:GET → 逐帧解析 → broadcast。返回 Ok = 干净 EOF。
async fn connect_and_consume(
    daemon: &DaemonClient,
    tx: &broadcast::Sender<DaemonEvent>,
    last_ok_ms: &Arc<AtomicU64>,
    last_event_id: &Arc<AtomicU64>,
) -> Result<(), SseError> {
    let last = last_event_id.load(Ordering::Relaxed);
    let mut request = daemon
        .stream_request()
        .header("accept", "text/event-stream");
    if last > 0 {
        // id: 0 是 sentinel 专用(sentinel_frame,sse.rs:98-100),不回传。
        request = request.header("last-event-id", last.to_string());
    }
    let response = request.send().await?;
    let status = response.status();
    if !status.is_success() {
        return Err(SseError::Status(status.as_u16()));
    }
    touch_last_ok(last_ok_ms);
    tracing::info!(
        resumed_from = if last > 0 { Some(last) } else { None },
        "daemon SSE stream connected"
    );

    let mut stream = response.bytes_stream();
    let mut buf = String::new();
    loop {
        match stream.next().await {
            Some(Ok(chunk)) => {
                for event in ingest_chunk(&chunk, &mut buf, last_ok_ms, last_event_id) {
                    // 无订阅者(启动期/间隙)= SendErr(Closed),忽略。
                    let _ = tx.send(event);
                }
            }
            Some(Err(err)) => return Err(SseError::Transport(err.to_string())),
            None => return Ok(()),
        }
    }
}

/// 当前 UNIX 毫秒(时钟不可得时 0 = 视为从未刷新)。
fn system_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 刷新「最近收到 daemon 字节」时刻(建连与每个 chunk 各一次 —— 活连接
/// 空闲期靠 daemon 的 30s `:ping` 保活,不刷新就会把健康连接判死)。
fn touch_last_ok(last_ok_ms: &AtomicU64) {
    last_ok_ms.store(system_millis(), Ordering::Relaxed);
}

/// 窗口判据(纯函数,供测试):`last` 非 0 且 `now - last ≤ window`。
fn fresh_within(last_ok_ms: u64, now_ms: u64, window: Duration) -> bool {
    last_ok_ms != 0 && now_ms.saturating_sub(last_ok_ms) <= window.as_millis() as u64
}

/// 单个 chunk 的处理(纯逻辑,供测试):刷活性 → 解码累计 → 切帧归一。
/// daemon 是 UTF-8 JSON;非法字节经 lossy 替换后会被 JSON 解析拒绝并丢帧,
/// 不会 panic。
fn ingest_chunk(
    chunk: &[u8],
    buf: &mut String,
    last_ok_ms: &AtomicU64,
    last_event_id: &AtomicU64,
) -> Vec<DaemonEvent> {
    touch_last_ok(last_ok_ms);
    buf.push_str(&String::from_utf8_lossy(chunk));
    parse_sse_frames(buf)
        .into_iter()
        .filter_map(|frame| handle_frame(frame, last_event_id))
        .collect()
}

/// SSE 解析层错误(重连循环统一退避,调用方不区分细类)。
#[derive(Debug, thiserror::Error)]
pub enum SseError {
    #[error("daemon unreachable: {0}")]
    Transport(String),
    #[error("daemon stream endpoint returned HTTP {0}")]
    Status(u16),
}

impl From<reqwest::Error> for SseError {
    fn from(err: reqwest::Error) -> Self {
        SseError::Transport(err.to_string())
    }
}

/// 一条已解出的 SSE 帧(`id`/`event` 可缺省;`data` 多行以 `\n` 拼接)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseFrame {
    pub id: Option<u64>,
    pub event: Option<String>,
    pub data: String,
}

/// 行缓冲式 SSE 帧切分(纯函数,供测试)。
///
/// 输入累计 buffer,按 `\n\n` 切完整帧,不完整尾部(半行/单换行)留在 buf。
/// CRLF 容忍;`:` 开头注释行(keepalive `:ping`)与无 data 的帧丢弃;
/// `retry:` 行忽略;`data:` 多行按 SSE 规范以 `\n` 拼接;冒号后的单个空格
/// 按规范剥离。daemon 实际只发 `id:`+`event:`+单行 `data:`,宽容度是给
/// 中间代理可能的重写的余量。
pub fn parse_sse_frames(buf: &mut String) -> Vec<SseFrame> {
    // CRLF 规整:逐字节处理时 \r 可能与 \n 分属两个 chunk。
    if buf.contains("\r\n") {
        *buf = buf.replace("\r\n", "\n");
    }
    let mut frames = Vec::new();
    while let Some(end) = buf.find("\n\n") {
        let raw: String = buf.drain(..end + 2).collect();
        if let Some(frame) = parse_sse_frame(&raw) {
            frames.push(frame);
        }
    }
    frames
}

/// 解析单个完整帧(含结尾 `\n\n`;不带也容忍)。
fn parse_sse_frame(raw: &str) -> Option<SseFrame> {
    let mut id: Option<u64> = None;
    let mut event: Option<String> = None;
    let mut data_lines: Vec<&str> = Vec::new();
    for line in raw.trim_end_matches('\n').split('\n') {
        if line.is_empty() || line.starts_with(':') {
            continue; // 空行 / 注释(keepalive)
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""), // 冒号缺失 = 空值字段(SSE 规范)
        };
        match field {
            "event" => event = Some(value.to_string()),
            "data" => data_lines.push(value),
            "id" => id = value.parse::<u64>().ok(),
            // retry: 重连间隔建议 —— shim 自管退避,忽略。
            _ => {}
        }
    }
    if data_lines.is_empty() {
        return None; // 纯注释/空帧丢弃
    }
    Some(SseFrame {
        id,
        event,
        data: data_lines.join("\n"),
    })
}

/// 帧 → DaemonEvent 归一 + last_event_id 推进。malformed data 记日志丢帧,
/// 绝不 panic(「turn 不死」同款约束:解失败只影响一帧)。
fn handle_frame(frame: SseFrame, last_event_id: &AtomicU64) -> Option<DaemonEvent> {
    let event_name = frame.event.unwrap_or_else(|| "message".to_string());
    if event_name == "stream-resync" {
        let reason = serde_json::from_str::<ResyncPayload>(&frame.data)
            .map(|p| p.reason)
            .unwrap_or_else(|_| "unknown".to_string());
        tracing::warn!(reason, "daemon stream resync sentinel (continuity lost)");
        // sentinel id 恒 0:不推进 last_event_id(daemon 语义)。
        return Some(DaemonEvent::Resync { reason });
    }
    if let Some(id) = frame.id {
        if id > 0 {
            last_event_id.store(id, Ordering::Relaxed);
        }
    }
    match event_name.as_str() {
        "chat-event" => serde_json::from_str::<ChatEventEnvelope>(&frame.data)
            .map(|e| DaemonEvent::ChatEvent {
                request_id: e.request_id,
                session_id: e.session_id,
                event: e.event,
            })
            .inspect_err(|err| tracing::debug!(error = %err, "dropping malformed chat-event"))
            .ok(),
        "tool:call" => serde_json::from_str::<ToolCallPayloadDto>(&frame.data)
            .map(DaemonEvent::ToolCall)
            .inspect_err(|err| tracing::debug!(error = %err, "dropping malformed tool:call"))
            .ok(),
        "tool:result" => serde_json::from_str::<ToolResultPayloadDto>(&frame.data)
            .map(DaemonEvent::ToolResult)
            .inspect_err(|err| tracing::debug!(error = %err, "dropping malformed tool:result"))
            .ok(),
        "permission:ask" => serde_json::from_str::<PermissionAskDto>(&frame.data)
            .map(DaemonEvent::PermissionAsk)
            .inspect_err(|err| tracing::debug!(error = %err, "dropping malformed permission:ask"))
            .ok(),
        other => {
            tracing::debug!(event = other, "ignoring daemon SSE event");
            Some(DaemonEvent::Other {
                event: other.to_string(),
            })
        }
    }
}

/// `stream-resync` payload(`{"reason":"restart"|"buffer_overrun"}`)。
#[derive(Debug, Deserialize)]
struct ResyncPayload {
    reason: String,
}

/// `chat-event` 信封(`ChatEventPayload`:rid + sid + `#[serde(flatten)]`
/// ChatEvent,kind 为 internally-tagged 判别字段,snake_case)。
#[derive(Debug, Deserialize)]
struct ChatEventEnvelope {
    request_id: String,
    session_id: String,
    #[serde(flatten)]
    event: ChatEventDto,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 基本切帧:id/event/data 提取 + 单行 data。
    #[test]
    fn parses_daemon_shape_frames() {
        let mut buf = String::from(
            "id: 7\nevent: chat-event\ndata: {\"kind\":\"delta\",\"text\":\"hi\"}\n\n",
        );
        let frames = parse_sse_frames(&mut buf);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].id, Some(7));
        assert_eq!(frames[0].event.as_deref(), Some("chat-event"));
        assert_eq!(frames[0].data, "{\"kind\":\"delta\",\"text\":\"hi\"}");
        assert!(buf.is_empty());
    }

    /// 半行缓冲:跨 chunk 边界的帧留在 buf,直到补齐才出。
    #[test]
    fn keeps_partial_frames_in_buffer() {
        let mut buf = String::from("id: 1\nevent: chat-event\ndata: {\"kind\":\"sta");
        assert!(parse_sse_frames(&mut buf).is_empty(), "半帧不出");
        buf.push_str("rt\"}\n\nid: 2\nevent: chat-event\ndata: {}\n\n");
        let frames = parse_sse_frames(&mut buf);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].id, Some(1));
        assert_eq!(frames[1].id, Some(2));
    }

    /// ping 注释帧丢弃;无 data 的帧丢弃;`retry:` 忽略。
    #[test]
    fn drops_comment_and_dataless_frames() {
        let mut buf = String::from(
            ":ping\n\nretry: 5000\n\nevent: chat-event\n\nid: 9\nevent: x\ndata: 1\n\n",
        );
        let frames = parse_sse_frames(&mut buf);
        assert_eq!(frames.len(), 1, "只有带 data 的帧存活: {frames:?}");
        assert_eq!(frames[0].id, Some(9));
        assert_eq!(frames[0].event.as_deref(), Some("x"));
    }

    /// 多 data 行按规范以 \n 拼接;CRLF 容忍(跨 chunk 的 \r|\n 分裂)。
    #[test]
    fn joins_multi_data_lines_and_tolerates_crlf() {
        let mut buf = String::from("id: 3\r\nevent: e\r\ndata: line1\r\ndata: line2\r\n\r\n");
        let frames = parse_sse_frames(&mut buf);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, "line1\nline2");
    }

    /// 断线后 last_event_id 保持,重连请求携带 Last-Event-ID 头;
    /// sentinel(id: 0)不推进。
    #[tokio::test]
    async fn last_event_id_survives_and_ignores_sentinel() {
        let last = Arc::new(AtomicU64::new(0));

        let f = handle_frame(
            SseFrame {
                id: Some(42),
                event: Some("chat-event".into()),
                data: r#"{"request_id":"r","session_id":"s","kind":"other"}"#.into(),
            },
            &last,
        );
        assert!(f.is_some(), "合法 chat-event 信封应解析成事件");
        assert_eq!(last.load(Ordering::Relaxed), 42);

        let sentinel = handle_frame(
            SseFrame {
                id: Some(0),
                event: Some("stream-resync".into()),
                data: "{\"reason\":\"restart\"}".into(),
            },
            &last,
        );
        assert!(matches!(sentinel, Some(DaemonEvent::Resync { .. })));
        assert_eq!(last.load(Ordering::Relaxed), 42, "sentinel 不推进 id");

        // malformed payload 丢帧不 panic。
        let bad = handle_frame(
            SseFrame {
                id: Some(43),
                event: Some("chat-event".into()),
                data: "not-json".into(),
            },
            &last,
        );
        assert!(bad.is_none(), "malformed 丢弃");
        assert_eq!(
            last.load(Ordering::Relaxed),
            43,
            "id 推进独立于 payload 解析"
        );
    }

    /// 健康窗口必须大于 daemon keepalive 间隔(30s `:ping`):活连接空闲期
    /// 只靠 ping 刷活,窗口小于 ping 间隔会把健康连接判死(初版 10s 实病,
    /// 空闲 >10s 的 session 全部 prompt 被误拒)。
    #[test]
    fn health_window_exceeds_daemon_ping_interval() {
        assert!(
            HEALTHY_WINDOW > Duration::from_secs(30),
            "HEALTHY_WINDOW = {HEALTHY_WINDOW:?} 必须大于 daemon 30s ping 间隔"
        );
    }

    /// 窗口判据边界:从未刷新(0)恒假;恰在窗口内真;过期假。
    #[test]
    fn fresh_within_boundaries() {
        assert!(!fresh_within(0, 1_000, HEALTHY_WINDOW), "未刷新恒不健康");
        assert!(fresh_within(1_000, 1_000, HEALTHY_WINDOW));
        assert!(fresh_within(
            1_000,
            1_000 + HEALTHY_WINDOW.as_millis() as u64,
            HEALTHY_WINDOW
        ));
        assert!(!fresh_within(
            1_000,
            1_001 + HEALTHY_WINDOW.as_millis() as u64,
            HEALTHY_WINDOW
        ));
    }

    /// 每个 chunk 刷新活性时刻(修 is_healthy 判据的根基):即便连接再无
    /// 后续事件,last_ok 也被 ingest 推进到「刚刚」。
    #[test]
    fn chunk_ingest_refreshes_liveness() {
        let last_ok = Arc::new(AtomicU64::new(0));
        let last_id = Arc::new(AtomicU64::new(0));
        let mut buf = String::new();
        let events = ingest_chunk(
            b"id: 5\nevent: chat-event\ndata: {\"request_id\":\"r\",\"session_id\":\"s\",\"kind\":\"delta\",\"text\":\"x\"}\n\n",
            &mut buf,
            &last_ok,
            &last_id,
        );
        assert_eq!(events.len(), 1);
        let now = system_millis();
        assert!(
            fresh_within(last_ok.load(Ordering::Relaxed), now, Duration::from_secs(1)),
            "chunk 到达即刷活:last_ok = {}",
            last_ok.load(Ordering::Relaxed)
        );
        assert_eq!(last_id.load(Ordering::Relaxed), 5);
    }
}
