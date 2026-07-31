//! P2.3 C9 — SSE 集成测试(task `07-20-remote-access-daemon-split`)。
//!
//! `daemon::sse` 自带 7 个单测,用 `DummyPayload` 直接 `broadcast`
//! 覆盖 `SseRegistry` 本身(broadcast / buffer / replay / sentinel /
//! large / slow)。本文件补**集成层**:真实 `run_chat_loop` 经
//! [`HttpSseSink`] 把事件 `broadcast` 到 [`SseRegistry`],一个
//! `subscribe` 客户端消费 —— 验证 `HttpSseSink` 的 7 个 `emit_*`
//! 接线正确(chat_loop emit → 正确 event name → registry → 客户端),
//! 以及 `Last-Event-ID` 重连回放协议。
//!
//! 不起真实 HTTP socket(axum serve + `EventSource` 客户端)—— 那是
//! P2.5 `e2e.rs` 的活;本文件直接消费 `SseRegistry::subscribe` 返回的
//! [`SseSubscription`](与 C2 stream handler 同构:`replay.chain(live)`)。

#![cfg(test)]

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::tests_common::{make_harness, test_messages, TestHarness};
use crate::agent::chat_loop::run_chat_loop;
use crate::daemon::sse::{HttpSseSink, SseFrame, SseRegistry};
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, TokenUsage};
use crate::state::ChatEventSink;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Drive `run_chat_loop` with an injectable `sink`(本文件传 `HttpSseSink`)。
/// 参数序列与 `tests_c2plus::run_loop` 完全一致,仅第 7 参(sink)参数化。
async fn run_loop_with_sink(
    tool_defs: Vec<crate::llm::types::ToolDef>,
    mock: Arc<MockProvider>,
    sink: Arc<dyn ChatEventSink>,
    rid: &str,
    h: TestHarness,
    token: CancellationToken,
) {
    run_chat_loop(
        tool_defs,
        mock.clone(),
        200_000,
        rid.into(),
        h.session_id.clone(),
        test_messages(),
        sink,
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        token,
        None,
        h.background_shells.clone(),
        None,
        false,
        false,
        Some(false),
        None,
        std::sync::Arc::new(crate::agent::subagent::ThreadLocalSubagentSink), // worker_event_sink
        None,
        None,
        h.subagent_cache.clone(),
        None,
        None,
        None, // project_main_override (2026-07-29)
        h.app_data_dir.clone(),
        None,
        h.question_store.clone(),
        None,
        None,
None,)
    .await;
}

/// 抽干 live channel 的所有缓冲帧。`run_chat_loop` 是直接 `await`
/// (非 spawn),返回时所有 `broadcast` 已 `try_send` 进 channel;
/// 这里立即读完全部缓冲,随后 100ms 无新帧即视为结束(sender 未
/// close,空 channel 上 `recv` 会 block,靠 timeout 退出)。
async fn drain_live(rx: &mut mpsc::Receiver<SseFrame>) -> Vec<SseFrame> {
    let mut frames = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
            Ok(Some(f)) => frames.push(f),
            Ok(None) | Err(_) => break,
        }
    }
    frames
}

// ---------------------------------------------------------------------------
// C9 集成测试
// ---------------------------------------------------------------------------

/// text-only 单轮:chat_loop 的 `Start` / `Delta` / `Done` 经
/// [`HttpSseSink`] 全部以 `"chat-event"` 广播,subscribe 客户端按
/// 全局 id 单调递增收到。
#[tokio::test]
async fn sse_text_only_round_emits_chat_event_sequence() {
    let h = make_harness().await;
    let registry = Arc::new(SseRegistry::new());
    let mut sub = registry.subscribe(None); // 先订阅,拿 live rx
    let sink: Arc<dyn ChatEventSink> = Arc::new(HttpSseSink {
        registry: Arc::clone(&registry),
    });
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta { text: "hi".into() }),
        Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: Some(TokenUsage::default()),
        }),
    ])]));
    run_loop_with_sink(vec![], mock, sink, "rid-sse-1", h, CancellationToken::new()).await;

    let frames = drain_live(&mut sub.live).await;
    // 至少 Start + Delta + Done(F5 latency / TurnComplete 等后续变体
    // 可能追加,故只断言下界 + event name + 单调 id)。
    assert!(
        frames.len() >= 3,
        "expected ≥3 chat-event frames, got {}: {:?}",
        frames.len(),
        frames
    );
    assert!(
        frames.iter().all(|f| f.event == "chat-event"),
        "every frame should be a chat-event: {:?}",
        frames.iter().map(|f| f.event.as_str()).collect::<Vec<_>>()
    );
    for w in frames.windows(2) {
        assert!(
            w[0].id < w[1].id,
            "ids must monotonically increase: {:?}",
            frames
        );
    }
}

/// tool_use 轮:chat_loop emit `chat-event`(Start / Done)+ 独立的
/// `tool:call` / `tool:result` 事件名 —— 验证 `HttpSseSink` 把不同
/// `emit_*` 路由到不同 SSE event name,且全局 id 跨事件名仍单调。
#[tokio::test]
async fn sse_tool_round_emits_distinct_event_names() {
    let h = make_harness().await;
    let registry = Arc::new(SseRegistry::new());
    let mut sub = registry.subscribe(None);
    let sink: Arc<dyn ChatEventSink> = Arc::new(HttpSseSink {
        registry: Arc::clone(&registry),
    });
    // turn 1:read_file(missing.txt → error tool_result)+ tool_use Done
    // turn 2:text-only end_turn
    let mock = Arc::new(MockProvider::new(vec![
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_sse_1".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "missing.txt"}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "done".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));
    let tool_defs = vec![crate::tools::read_file::definition()];
    run_loop_with_sink(
        tool_defs,
        mock,
        sink,
        "rid-sse-2",
        h,
        CancellationToken::new(),
    )
    .await;

    let frames = drain_live(&mut sub.live).await;
    let names: Vec<&str> = frames.iter().map(|f| f.event.as_str()).collect();
    assert!(
        names.iter().any(|n| *n == "tool:call"),
        "expected a tool:call frame, names = {:?}",
        names
    );
    assert!(
        names.iter().any(|n| *n == "tool:result"),
        "expected a tool:result frame, names = {:?}",
        names
    );
    assert!(
        names.iter().any(|n| *n == "chat-event"),
        "expected chat-event frames, names = {:?}",
        names
    );
    for w in frames.windows(2) {
        assert!(
            w[0].id < w[1].id,
            "ids must monotonically increase across event names: {:?}",
            frames
        );
    }
}

/// 重连回放:跑完一轮后,新客户端带 `Last-Event-ID` 订阅,registry
/// 回放 buffer 中 `id > last` 的帧(连续,无 gap,不发 sentinel);
/// 首次连接(`None`)不回放历史。
#[tokio::test]
async fn sse_replay_returns_frames_after_last_event_id() {
    let h = make_harness().await;
    let registry = Arc::new(SseRegistry::new());
    let sink: Arc<dyn ChatEventSink> = Arc::new(HttpSseSink {
        registry: Arc::clone(&registry),
    });
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta { text: "a".into() }),
        Ok(ChatEvent::Delta { text: "b".into() }),
        Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: Some(TokenUsage::default()),
        }),
    ])]));
    run_loop_with_sink(vec![], mock, sink, "rid-sse-3", h, CancellationToken::new()).await;

    // buffer 现有 N 条 chat-event(id 1..=N)。带 last=2 订阅 →
    // 回放 id > 2 的帧(2 与 oldest=1 之间无 gap → 连续,非 sentinel)。
    let replaying = registry.subscribe(Some(2));
    assert!(
        !replaying.replay.is_empty(),
        "replay should contain frames with id > 2"
    );
    assert!(
        replaying.replay.iter().all(|f| f.id > 2),
        "every replayed frame must have id > 2: {:?}",
        replaying.replay
    );
    assert!(
        replaying.replay.iter().all(|f| f.event == "chat-event"),
        "replay frames preserve the event name"
    );
    // replay 协议:首次连接(last=None)不回放 —— 前端连上后自己
    // GET snapshot 拉当前状态。
    let fresh = registry.subscribe(None);
    assert!(fresh.replay.is_empty(), "first connection replays nothing");
}
