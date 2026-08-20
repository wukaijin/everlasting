#![cfg(test)]

// 08-20-turn-usage-event-quota-view WP1 — ChatEvent::TurnUsage 集成测试。
//
// 覆盖 AC1(正常完成轮:事件与 DB 落值同点同值)+ P2 边界(usage=None
// 的 Done 不发事件,前端退化为等 loadHistory 的现状)。worker 维度
// (AC2)由 sink 隔离结构性保证 —— worker 的事件进 SubagentBufferSink
// transcript、不达主 chat emitter,08-20-worker-turn-trace-persist 的
// 既有回归锁已覆盖 run 行落值;此处不再逼出 dispatch 机器。

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::tests_common::{make_harness, MockEmitter, TestHarness};
use crate::agent::chat_loop::run_chat_loop;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, ChatMessage, MessageContent, Role, TokenUsage};

const WINDOW: u32 = 20_000;

fn user(text: impl Into<String>) -> ChatMessage {
    ChatMessage {
        role: Role::User,
        content: MessageContent::Text(text.into()),
        speaker: None,
        attachments: None,
    }
}

fn done_response_with_usage(usage: TokenUsage) -> MockResponse {
    MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta {
            text: "answer".to_string(),
        }),
        Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: Some(usage),
        }),
    ])
}

/// 标准(非 worker / 非群聊)run_chat_loop 调用,对齐 budget.rs 模板。
async fn run_loop_with_emitter(
    h: &TestHarness,
    provider: Arc<MockProvider>,
    messages: Vec<ChatMessage>,
    emitter: Arc<MockEmitter>,
) {
    run_chat_loop(
        vec![],
        provider,
        WINDOW,
        None,
        "rid-turn-usage-it".into(),
        h.session_id.clone(),
        messages,
        emitter,
        h.db.clone(),
        h.cancellations.clone(),
        h.session_active_request.clone(),
        h.read_guard.clone(),
        h.memory_cache.clone(),
        h.skill_cache.clone(),
        h.permission_asks.clone(),
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        None,
        false,
        false,
        Some(false),
        None,
        std::sync::Arc::new(crate::agent::subagent::ThreadLocalSubagentSink),
        None,
        None,
        h.subagent_cache.clone(),
        None,
        None,
        None,
        h.app_data_dir.clone(),
        None,
        h.question_store.clone(),
        None,
        None,
        None,
        h.stub_loaded.clone(),
    )
    .await;
}

/// AC1:正常完成轮 Done 臂发 TurnUsage,事件字段(seq / run_id='' /
/// usage / tools / system / context_window)与 turn_trace 落值一致
/// —— emit 与 upsert 同点同值,一致性构造性成立,本测试锁该构造
/// 不被回归破坏(比如有人把 emit 挪到值被改写的路径之后)。
#[tokio::test]
async fn turn_usage_event_matches_db_row() {
    crate::memory::tokens::ensure_initialized().await;
    let h = make_harness().await;
    let usage = TokenUsage {
        input_tokens: 120,
        output_tokens: 45,
        cache_creation_input_tokens: 30,
        cache_read_input_tokens: 80,
        context_input_tokens: 230,
    };
    let mock = Arc::new(MockProvider::new(vec![done_response_with_usage(usage)]));
    let emitter = Arc::new(MockEmitter::new());
    run_loop_with_emitter(&h, mock, vec![user("hello")], emitter.clone()).await;

    let events = emitter.chat_events();
    let turn_usage: Vec<_> = events
        .iter()
        .filter(|p| matches!(&p.event, ChatEvent::TurnUsage { .. }))
        .collect();
    assert_eq!(
        turn_usage.len(),
        1,
        "normal completed turn must emit exactly one TurnUsage"
    );
    let ChatEvent::TurnUsage {
        seq,
        run_id,
        usage: ev_usage,
        tools_token,
        system_token,
        context_window: ev_window,
        ..
    } = &turn_usage[0].event
    else {
        unreachable!("filtered above");
    };
    assert_eq!(run_id, "", "main-loop rows use the '' sentinel");
    assert_eq!(ev_usage, &usage);
    assert!(
        tools_token.unwrap_or(0) > 0,
        "tools slice estimated (harness tools non-empty)"
    );
    assert!(system_token.unwrap_or(0) > 0, "system slice estimated");
    assert_eq!(*ev_window, WINDOW);

    // 与 DB 落值逐字段一致(AC1 的"同点同值"半边)。
    let rows = crate::db::trace::list_turn_traces(&h.db, &h.session_id)
        .await
        .expect("list turn traces");
    let row = rows
        .iter()
        .find(|r| r.seq == *seq)
        .expect("turn_trace row for the emitted seq");
    assert_eq!(row.run_id, "");
    let db_usage: TokenUsage =
        serde_json::from_str(row.token_usage_json.as_deref().unwrap_or("{}"))
            .expect("parse token_usage_json");
    assert_eq!(db_usage, *ev_usage);
    assert_eq!(row.tools_token, tools_token.map(|t| t as i64));
    assert_eq!(row.system_token, system_token.map(|t| t as i64));
    assert_eq!(
        row.context_window.map(|w| w as u32),
        Some(*ev_window),
        "context_window snapshot rides the same row"
    );
}

/// P2 边界:Done{usage: None}(provider 未报 usage —— 取消 / 网络断
/// 等形态)不发 TurnUsage,也不落 token 行 —— 前端该轮 cells 保持
/// "—",退化为事件存在前的行为(等下一次 loadHistory)。
#[tokio::test]
async fn done_without_usage_emits_no_turn_usage() {
    crate::memory::tokens::ensure_initialized().await;
    let h = make_harness().await;
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta {
            text: "partial".to_string(),
        }),
        Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: None,
        }),
    ])]));
    let emitter = Arc::new(MockEmitter::new());
    run_loop_with_emitter(&h, mock, vec![user("hello")], emitter.clone()).await;

    assert!(
        emitter
            .chat_events()
            .iter()
            .all(|p| { !matches!(&p.event, ChatEvent::TurnUsage { .. }) }),
        "usage=None Done must not emit TurnUsage"
    );
    let rows = crate::db::trace::list_turn_traces(&h.db, &h.session_id)
        .await
        .expect("list turn traces");
    assert!(
        rows.iter().all(|r| r.token_usage_json.is_none()),
        "usage=None Done must not write a token row either (gate symmetry)"
    );
}
