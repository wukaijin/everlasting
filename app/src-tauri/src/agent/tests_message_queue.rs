#![cfg(test)]

//! F1 消息队列(2026-08-25)驱动器集成测试。
//!
//! 直接构造 [`QueueDriverDeps`] 驱动 `run_queue_driver`(与
//! `agent_loop_*` 测试直调 `run_chat_loop` 同构)。覆盖 implement.md
//! #9 的驱动器语义:单轮注入落库、多轮续轮(TurnContinuation + 单
//! Done 契约)、cancel 清队、错误终止保队。
//!
//! 已知覆盖缺口(记录于任务 review):chat_inner 路由临界区的
//! "忙 → Queued" 分支需要完整 `AppState`,测试 harness 不构造它 ——
//! 该分支由 PR4 live 冒烟(curl 打 REST 排队分支)覆盖。

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::tests_common::{make_harness, MockEmitter, TestHarness};
use crate::agent::chat::{run_queue_driver, QueueDriverDeps};
use crate::agent::message_queue::{self, SharedQueues};
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, ChatMessage, MessageContent, Role, TokenUsage};

fn user_msg(text: &str) -> ChatMessage {
    ChatMessage {
        role: Role::User,
        content: MessageContent::Text(text.to_string()),
        speaker: None,
        attachments: None,
    }
}

async fn preload(queues: &SharedQueues, session_id: &str, texts: &[&str]) {
    let mut map = queues.lock().await;
    let q = map.entry(session_id.to_string()).or_default();
    for t in texts {
        message_queue::push_with_origin(q, user_msg(t), 0, None).expect("preload under capacity");
    }
}

async fn persisted_user_texts(h: &TestHarness) -> Vec<String> {
    let loaded = crate::db::load_session(&h.db, &h.session_id)
        .await
        .expect("load_session")
        .expect("session row");
    loaded
        .messages
        .iter()
        .filter(|m| m.role == "user")
        .map(|m| m.text.clone())
        .collect()
}

fn done_count(emitter: &MockEmitter) -> usize {
    emitter
        .chat_events()
        .iter()
        .filter(|p| matches!(&p.event, ChatEvent::Done { .. }))
        .count()
}

/// 模拟"忙时用户连发":首次 send 调用完成后向队列追加一条。
fn spawn_late_enqueue(
    queues: SharedQueues,
    session_id: String,
    text: &'static str,
    handle: Arc<std::sync::atomic::AtomicUsize>,
) {
    tokio::spawn(async move {
        loop {
            if handle.load(Ordering::SeqCst) >= 1 {
                let mut map = queues.lock().await;
                let q = map.entry(session_id.clone()).or_default();
                let _ = message_queue::push_with_origin(q, user_msg(text), 0, None);
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    });
}

async fn run_driver(
    h: &TestHarness,
    mock: Arc<MockProvider>,
    emitter: Arc<MockEmitter>,
    queues: SharedQueues,
    token: CancellationToken,
) {
    let deps = QueueDriverDeps {
        tool_defs: vec![],
        provider: mock,
        context_window: 200_000,
        provider_id: None,
        rid: "rid-f1-driver".into(),
        session_id: h.session_id.clone(),
        inner_sink: emitter,
        db: h.db.clone(),
        cancellations: h.cancellations.clone(),
        session_active_request: h.session_active_request.clone(),
        read_guard: h.read_guard.clone(),
        memory_cache: h.memory_cache.clone(),
        skill_cache: h.skill_cache.clone(),
        permission_asks: h.permission_asks.clone(),
        token,
        resend_seq: None,
        forced_dispatch: None,
        background_shells: h.background_shells.clone(),
        worker_catalog: None,
        worker_event_sink: Arc::new(crate::agent::subagent::ThreadLocalSubagentSink),
        subagent_cache: h.subagent_cache.clone(),
        app_data_dir: h.app_data_dir.clone(),
        question_store: h.question_store.clone(),
        workflow_ctx: None,
        stub_loaded: h.stub_loaded.clone(),
        queues,
    };
    run_queue_driver(deps).await;
}

// --- tests ---------------------------------------------------------------

#[tokio::test]
async fn driver_single_round_injects_persists_and_reemits_one_done() {
    let h = make_harness().await;
    let queues: SharedQueues = Default::default();
    preload(&queues, &h.session_id, &["hello"]).await;
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Delta {
            text: "reply".into(),
        }),
        Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: Some(TokenUsage::default()),
        }),
    ])]));
    let emitter = Arc::new(MockEmitter::new());
    let token = CancellationToken::new();

    run_driver(&h, mock.clone(), emitter.clone(), queues.clone(), token).await;

    assert_eq!(mock.call_count(), 1, "single queued message = single round");
    // 注入内容 = reload 历史 + 队列尾条;provider 看到的请求尾条是排队消息。
    let sent = mock.sent_messages();
    let tail_text = match &sent[0].last().expect("non-empty request").content {
        MessageContent::Text(t) => t.clone(),
        _ => panic!("text expected"),
    };
    assert_eq!(tail_text, "hello");
    // 落库:user 排队项 + assistant 回复。
    let rows = persisted_user_texts(&h).await;
    assert_eq!(rows, vec!["hello"]);
    // 单 Done 契约:整请求只补发一次(round 内层的 Done 被吞)。
    assert_eq!(done_count(&emitter), 1);
    // 评审 Round 2 P0 回归锁:内层轮的常规事件(Delta)必须穿透
    // DriverSink 到达前端 sink —— 曾经 `_ => true` 分支不转发,
    // 流式在生产端完全不可见。
    let deltas = emitter
        .chat_events()
        .iter()
        .filter(|p| matches!(p.event, ChatEvent::Delta { .. }))
        .count();
    assert_eq!(deltas, 1, "inner-round deltas must reach the frontend sink");
    // round 0 不发 TurnContinuation。
    assert!(emitter
        .chat_events()
        .iter()
        .all(|p| !matches!(p.event, ChatEvent::TurnContinuation { .. })));
}

#[tokio::test]
async fn driver_continuation_round_emits_turn_continuation_and_fifo_order() {
    let h = make_harness().await;
    let queues: SharedQueues = Default::default();
    preload(&queues, &h.session_id, &["first"]).await;
    let script = vec![
        MockResponse::Events(vec![
            Ok(ChatEvent::Delta { text: "one".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        MockResponse::Events(vec![
            Ok(ChatEvent::Delta { text: "two".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ];
    let mock = Arc::new(MockProvider::new(script));
    let emitter = Arc::new(MockEmitter::new());
    let token = CancellationToken::new();

    // 模拟忙时连发:第一次 send 完成后入队 "second"。
    spawn_late_enqueue(
        queues.clone(),
        h.session_id.clone(),
        "second",
        mock.call_count_handle(),
    );

    run_driver(&h, mock.clone(), emitter.clone(), queues.clone(), token).await;

    assert_eq!(
        mock.call_count(),
        2,
        "queued second message must open a continuation round"
    );
    // TurnContinuation 恰发一次,count=1(round1 注入一条)。
    let conts: Vec<usize> = emitter
        .chat_events()
        .iter()
        .filter_map(|p| match &p.event {
            ChatEvent::TurnContinuation { count } => Some(*count),
            _ => None,
        })
        .collect();
    assert_eq!(conts, vec![1]);
    // FIFO 落库顺序:first → one → second → two。
    let all = crate::db::load_session(&h.db, &h.session_id)
        .await
        .expect("load_session")
        .expect("session row");
    let pairs: Vec<(String, String)> = all
        .messages
        .iter()
        .map(|m| (m.role.clone(), m.text.clone()))
        .collect();
    assert_eq!(
        pairs,
        vec![
            ("user".to_string(), "first".to_string()),
            ("assistant".to_string(), "one".to_string()),
            ("user".to_string(), "second".to_string()),
            ("assistant".to_string(), "two".to_string()),
        ]
    );
    // 单 Done 契约:两轮内层各吞一次,真结束只补发一次。
    assert_eq!(done_count(&emitter), 1);
    // 续轮流式可见性(P0 回归):两轮的 Delta 都要到达前端 sink。
    let delta_texts: Vec<String> = emitter
        .chat_events()
        .iter()
        .filter_map(|p| match &p.event {
            ChatEvent::Delta { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(delta_texts, vec!["one", "two"]);
}

#[tokio::test]
async fn driver_cancel_mid_round_clears_remaining_queue() {
    let h = make_harness().await;
    let queues: SharedQueues = Default::default();
    // 注意语义:round 开始时 drain_all 取走**当时**的全部队列项,
    // 已注入的无法撤回。cancel 保护的是"之后才入队"的滞留项 ——
    // 故只预载 m1,m2 由 watcher 在 round-0 进行中入队。
    preload(&queues, &h.session_id, &["m1"]).await;
    let token = CancellationToken::new();
    let mock = Arc::new(MockProvider::new(vec![MockResponse::HangingThenCancel]));
    let emitter = Arc::new(MockEmitter::new());

    spawn_late_enqueue(
        queues.clone(),
        h.session_id.clone(),
        "m2",
        mock.call_count_handle(),
    );

    // 挂起流中触发 cancel(HangingThenCancel 契约:token 取消才破环)。
    let cancel_token = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(80)).await;
        cancel_token.cancel();
    });

    run_driver(&h, mock.clone(), emitter.clone(), queues.clone(), token).await;

    // R7:取消 → 清空队列,m2 永不注入。
    let rest = message_queue::list_session(&queues, &h.session_id).await;
    assert!(rest.is_empty(), "cancel must clear the queue");
    let users = persisted_user_texts(&h).await;
    assert!(
        !users.contains(&"m2".to_string()),
        "cancelled remainder must not be injected"
    );
    // Done(cancelled) 补发一次(非 error 路径,包装器吞掉后真结束补发)。
    let cancelled_dones = emitter
        .chat_events()
        .iter()
        .filter(|p| {
            matches!(&p.event, ChatEvent::Done { stop_reason, .. }
                if stop_reason.as_deref() == Some("cancelled"))
        })
        .count();
    assert_eq!(cancelled_dones, 1);
}

#[tokio::test]
async fn driver_error_retains_queue_and_suppresses_done() {
    use crate::llm::error::LlmError;
    let h = make_harness().await;
    let queues: SharedQueues = Default::default();
    preload(&queues, &h.session_id, &["boom-target"]).await;
    let script = vec![MockResponse::ErrThenEnd(LlmError::Server {
        status: 503,
        message: "service unavailable".into(),
        retry_after: None,
    })];
    let mock = Arc::new(MockProvider::new(script));
    let emitter = Arc::new(MockEmitter::new());
    let token = CancellationToken::new();

    // 错误前用户的连发(模拟 design §4 "错误终止时已在队列的消息")。
    spawn_late_enqueue(
        queues.clone(),
        h.session_id.clone(),
        "retained",
        mock.call_count_handle(),
    );

    run_driver(&h, mock.clone(), emitter.clone(), queues.clone(), token).await;

    // 错误终止:队列保留(design §4),不续轮。
    let rest = message_queue::list_session(&queues, &h.session_id).await;
    assert_eq!(rest.len(), 1, "error must retain the queue");
    // Error 穿透包装器;Done 被抑制(error 路径不补发)。
    assert_eq!(emitter.error_event_count(), 1);
    assert_eq!(done_count(&emitter), 0);
}

// ---------------------------------------------------------------------------
// F2 定时任务 origin 载体链(2026-08-28, `08-28-f2-scheduled-tasks`
// design §4.1):QueuedMessage.origin → 驱动器 drained.last() →
// ChatLoopRequest.origin → init.rs persist 门控 + metadata 信封
// `scheduled` 键。
// ---------------------------------------------------------------------------

fn sched_origin(task_id: &str) -> crate::scheduler::TaskOrigin {
    crate::scheduler::TaskOrigin::Scheduled {
        task_id: task_id.to_string(),
        task_name: "定时早报".to_string(),
        fired_at: 1_720_000_000_000,
    }
}

async fn scheduled_audit_payloads(h: &TestHarness) -> Vec<serde_json::Value> {
    let rows = sqlx::query(
        "SELECT payload_json FROM session_audit_events \
         WHERE session_id = ? AND kind = 'scheduled_task_fired' ORDER BY id ASC",
    )
    .bind(&h.session_id)
    .fetch_all(&h.db)
    .await
    .expect("audit query");
    rows.iter()
        .filter_map(|r| {
            use sqlx::Row;
            let payload: String = r.try_get(0).ok()?;
            serde_json::from_str(&payload).ok()
        })
        .collect()
}

/// AC8「实 fire 断言 metadata.scheduled 三键」的链路级覆盖:调度器注入
/// 的队列条目(origin Some)经驱动器消费后,该 user 行的
/// `messages.metadata.scheduled` 必须携带 task_id / task_name / fired_at
/// 三键;且纯 origin 轮**不**发 FileInjections 事件(persist 门控放宽
/// 只作用于 metadata 写,事件触发条件不变)。
#[tokio::test]
async fn scheduled_origin_persists_scheduled_metadata() {
    let h = make_harness().await;
    let queues: SharedQueues = Default::default();
    {
        let mut map = queues.lock().await;
        let q = map.entry(h.session_id.clone()).or_default();
        message_queue::push_with_origin(
            q,
            user_msg("scheduled body"),
            0,
            Some(sched_origin("task-77")),
        )
        .expect("preload under capacity");
    }
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Delta {
            text: "reply".into(),
        }),
        Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: Some(TokenUsage::default()),
        }),
    ])]));
    let emitter = Arc::new(MockEmitter::new());

    run_driver(
        &h,
        mock.clone(),
        emitter.clone(),
        queues.clone(),
        CancellationToken::new(),
    )
    .await;

    // user 行已落库,metadata.scheduled 三键齐全。
    let loaded = crate::db::load_session(&h.db, &h.session_id)
        .await
        .expect("load_session")
        .expect("session row");
    let user_row = loaded
        .messages
        .iter()
        .find(|m| m.role == "user" && m.text == "scheduled body")
        .expect("injected scheduled user row");
    let meta = user_row
        .metadata
        .as_ref()
        .expect("origin-only turn must write messages.metadata");
    let sched = &meta["scheduled"];
    assert_eq!(sched["kind"], "scheduled");
    assert_eq!(sched["task_id"], "task-77");
    assert_eq!(sched["task_name"], "定时早报");
    assert_eq!(sched["fired_at"], 1_720_000_000_000i64);

    // 纯 origin 轮无 injections/attachments → 无 FileInjections 事件。
    assert!(
        emitter
            .chat_events()
            .iter()
            .all(|p| !matches!(p.event, ChatEvent::FileInjections { .. })),
        "persist-gate widening must not emit an empty FileInjections payload"
    );

    // 对照组语义(既有路径零影响)由既有测试锁定:本文件其余用例的
    // user 行(pre-F1/F2 注入手动消息)metadata 均为 None。
    let manual_rows: Vec<&crate::db::MessageRow> = loaded
        .messages
        .iter()
        .filter(|m| m.role == "user" && m.text != "scheduled body")
        .collect();
    assert!(
        manual_rows.iter().all(|m| m.metadata.is_none()),
        "non-scheduler user rows must stay metadata-free"
    );
}

/// lost 审计(design §4.3-5):驱动器 cancel break 清队前,滞留的带
/// origin 条目补 `lost` 审计(best-effort;已注入消费的条目不审计)。
#[tokio::test]
async fn driver_cancel_audits_lost_for_scheduled_entries() {
    let h = make_harness().await;
    let queues: SharedQueues = Default::default();
    // m1(round-0 即被 drain 注入消费,不算 lost)+ m2(cancel 时滞留)。
    preload(&queues, &h.session_id, &["m1"]).await;
    let token = CancellationToken::new();
    let mock = Arc::new(MockProvider::new(vec![MockResponse::HangingThenCancel]));
    let emitter = Arc::new(MockEmitter::new());

    // 确定性时序(免固定 sleep 竞态):HangingThenCancel 让 round-0
    // 挂在 pending 流上 —— 同一个 watcher 观测到 send 已发起
    // (call_count>=1)后先入队 m2(滞留项),**再** cancel。cancel 时
    // m2 必然已在队列。
    let queues2 = queues.clone();
    let sid = h.session_id.clone();
    let handle = mock.call_count_handle();
    let cancel_token = token.clone();
    tokio::spawn(async move {
        loop {
            if handle.load(Ordering::SeqCst) >= 1 {
                let mut map = queues2.lock().await;
                let q = map.entry(sid.clone()).or_default();
                let _ = message_queue::push_with_origin(
                    q,
                    user_msg("m2"),
                    0,
                    Some(sched_origin("task-lost-1")),
                );
                drop(map);
                cancel_token.cancel();
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    });

    run_driver(&h, mock.clone(), emitter.clone(), queues.clone(), token).await;

    // 队列已清空,m2 永不注入。
    assert!(message_queue::list_session(&queues, &h.session_id)
        .await
        .is_empty());
    let users = persisted_user_texts(&h).await;
    assert!(!users.contains(&"m2".to_string()));

    // 滞留的 origin 条目被补 lost 审计,task_id/name 随载荷。
    let payloads = scheduled_audit_payloads(&h).await;
    assert_eq!(payloads.len(), 1, "only the stranded origin entry");
    assert_eq!(payloads[0]["action"], "lost");
    assert_eq!(payloads[0]["task_id"], "task-lost-1");
    assert_eq!(payloads[0]["task_name"], "定时早报");
}

/// **多 drain 钉现状**(AC8 / prd R4,DEBT §RULE-QUEUE-001 根治前防漂移):
/// scheduled 条目 + 手动条目同轮 drain 时,驱动器把**全部** drained 条目
/// 喂给 LLM(sent 尾条 = 手动条目),但 persist 点只写**尾条** user 行
/// —— 非尾条 scheduled prompt 无 DB 行、「定时」metadata 随失。这是
/// **现状行为**的显式断言,不是期望行为;根治(持久化全部非尾条)时
/// 本测试**应当**被改写。
#[tokio::test]
async fn multi_drain_pins_current_tail_only_persist_rule_queue_001() {
    let h = make_harness().await;
    let queues: SharedQueues = Default::default();
    {
        let mut map = queues.lock().await;
        let q = map.entry(h.session_id.clone()).or_default();
        // 调度条目在队列头,手动条目尾随(= 本轮被持久化的尾条)。
        // 同 tick 串行化(design §9)之外的残余窗口:二者仍可同轮 drain。
        message_queue::push_with_origin(
            q,
            user_msg("scheduled body"),
            0,
            Some(sched_origin("task-pin")),
        )
        .expect("under capacity");
        message_queue::push_with_origin(q, user_msg("manual body"), 1, None)
            .expect("under capacity");
    }
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Delta {
            text: "reply".into(),
        }),
        Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: Some(TokenUsage::default()),
        }),
    ])]));
    let emitter = Arc::new(MockEmitter::new());

    run_driver(
        &h,
        mock.clone(),
        emitter.clone(),
        queues.clone(),
        CancellationToken::new(),
    )
    .await;

    assert_eq!(mock.call_count(), 1, "single round drains the whole queue");
    // LLM 单次看到全部 drained 条目:FIFO 尾两条 = scheduled → manual。
    let sent = mock.sent_messages();
    let texts: Vec<String> = sent[0]
        .iter()
        .rev()
        .take(2)
        .rev()
        .map(|m| match &m.content {
            MessageContent::Text(t) => t.clone(),
            _ => panic!("text expected"),
        })
        .collect();
    assert_eq!(texts, vec!["scheduled body", "manual body"]);

    // 现状(钉):DB 只落尾条 user 行;非尾条 scheduled prompt 无行,
    // 其 origin 亦无处安放(metadata 只写在被持久化的尾条上)。
    let rows = persisted_user_texts(&h).await;
    assert_eq!(rows, vec!["manual body"], "only the tail entry persists");
    let loaded = crate::db::load_session(&h.db, &h.session_id)
        .await
        .expect("load_session")
        .expect("session row");
    let manual_row = loaded
        .messages
        .iter()
        .find(|m| m.role == "user" && m.text == "manual body")
        .expect("tail row persisted");
    assert!(
        manual_row.metadata.is_none(),
        "tail (manual, origin-less) row carries no metadata"
    );
}
