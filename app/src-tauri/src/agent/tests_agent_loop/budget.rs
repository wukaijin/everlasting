#![cfg(test)]
// unified-context-budget WP1 集成测试(08-19-unified-context-budget,
// prd AC2 集成半边;单测半边在 `agent/budget.rs`)。
//
// 验证统一口径在 drive_turn 的端到端接线:messages 部件自身未达
// 0.85 触发线的历史,因 system+tools overhead 的补计而触发**机械**
// 压缩(旧 messages-only 口径不会触发 —— 该隔离对照由
// `budget.rs::tools_and_system_overhead_crosses_trigger_when_messages_under`
// 单测锁定,集成侧不再重复构造环境噪声隔离)。
//
// 环境无关性说明:合成头(user-scope memory/skill 层)随测试机漂移
// (0..3 条),断言只锚定我们可控的历史行 token 区间与"压缩确实发生"
// 的行为证据(主 turn send 骤减 + turn_trace.compaction_json 落值),
// 不对绝对总量做精确断言。

use std::sync::Arc;

use super::tests_common::{
    chat_loop_deps, chat_loop_request, make_harness, parent_role, MockEmitter, TestHarness,
};
use crate::agent::chat_loop::run_chat_loop;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, ChatMessage, MessageContent, Role, TokenUsage};

/// 测试窗口:trigger = 0.85 × 20_000 = 17_000,target = 10_000。
const WINDOW: u32 = 20_000;

fn pad(n_chars: usize) -> String {
    "the quick brown fox jumps over the lazy dog. "
        .repeat(n_chars / 45 + 1)
        .chars()
        .take(n_chars)
        .collect()
}

fn user(text: impl Into<String>) -> ChatMessage {
    ChatMessage {
        role: Role::User,
        content: MessageContent::Text(text.into()),
        speaker: None,
        attachments: None,
    }
}

fn assistant(text: impl Into<String>) -> ChatMessage {
    ChatMessage {
        role: Role::Assistant,
        content: MessageContent::Text(text.into()),
        speaker: None,
        attachments: None,
    }
}

fn end_turn_response(text: &str) -> MockResponse {
    MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta {
            text: text.to_string(),
        }),
        Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: Some(TokenUsage::default()),
        }),
    ])
}

/// 自校准历史:交替 user/assistant 对(每对 ≈ 500 tok,步长小于判定
/// 窗),直到历史行自身落进 [16_300, 16_850) token —— 距 17_000 触发
/// 线留 ≥150 判定余量、距 16_300 下限留足系统开销(≥150,任何环境的
/// 最小 system prompt 都覆盖)的补计空间。尾部当前输入不计入(seed 集
/// 外,init 会 persist)。
async fn under_line_history() -> (Vec<ChatMessage>, ChatMessage) {
    let mut rows: Vec<ChatMessage> = Vec::new();
    let mut tokens = 0u32;
    while tokens < 16_300 {
        rows.push(user(pad(1_000)));
        rows.push(assistant(pad(1_000)));
        tokens = crate::agent::context::estimate_messages_tokens(&rows).await;
    }
    assert!(
        (16_300..16_850).contains(&tokens),
        "self-calibrated history out of range: {}",
        tokens
    );
    (rows, user("current question"))
}

async fn seed_history(h: &TestHarness, rows: &[ChatMessage]) {
    for (i, m) in rows.iter().enumerate() {
        crate::db::persist_turn(
            &h.db,
            &h.session_id,
            m.role,
            &m.content,
            i as i64,
            None,
            None,
        )
        .await
        .expect("seed history row");
    }
}

/// 标准(非 worker / 非群聊)run_chat_loop 调用,对齐
/// compaction_summary.rs 的模板(harness 默认压缩关 → 机械路径直达,
/// 免摘要 mock 脚本)。
async fn run_loop(h: &TestHarness, provider: Arc<MockProvider>, messages: Vec<ChatMessage>) {
    run_loop_with_emitter(h, provider, messages, Arc::new(MockEmitter::new())).await;
}

/// `run_loop` 的 emitter 注入变体(预算闸门测试要检查 ChatEvent 流)。
async fn run_loop_with_emitter(
    h: &TestHarness,
    provider: Arc<MockProvider>,
    messages: Vec<ChatMessage>,
    emitter: Arc<MockEmitter>,
) {
    run_chat_loop(
        chat_loop_request(
            vec![],
            provider,
            WINDOW,
            "rid-budget-it".into(),
            h.session_id.clone(),
            messages,
            emitter,
        ),
        chat_loop_deps(&h),
        parent_role(&h),
    )
    .await;
}

/// AC2 集成半边:历史行自身 < 0.85 触发线,统一口径(补 system+tools
/// overhead)≥ 触发线 → 机械压缩发生。行为证据三件:
/// ① 主 turn send 的消息数较输入骤减(丢组生效);
/// ② turn_trace.compaction_json 落值(record_compaction 跑过);
/// ③ turn 正常完成(未 StillOver 中止 —— 环境合成头 + 尾部保护
///    之下 target 10k 可达)。
#[tokio::test]
async fn tools_and_system_squeeze_triggers_mechanical_compaction() {
    crate::memory::tokens::ensure_initialized().await;
    let h = make_harness().await;
    let (seed_rows, tail) = under_line_history().await;
    let history_tokens = crate::agent::context::estimate_messages_tokens(&seed_rows).await;
    let trigger = crate::agent::context::trigger_threshold(WINDOW);
    assert!(
        (history_tokens as u64) < (trigger as u64),
        "fixture premise: history rows under the trigger line ({} < {})",
        history_tokens,
        trigger
    );
    seed_history(&h, &seed_rows).await;
    let mut wire = seed_rows.clone();
    wire.push(tail);

    let mock = Arc::new(MockProvider::new(vec![end_turn_response("done")]));
    let input_len = wire.len();
    run_loop(&h, mock.clone(), wire).await;

    // ③ turn 完成:主 turn send 至少发生一次。
    let sends = mock.sent_messages();
    assert!(!sends.is_empty(), "main turn must reach provider.send");
    // ① 丢组生效:发送消息数远小于输入(保护头/尾 + 环境合成头之外
    // 的中段被机械丢弃)。
    let sent_len = sends[0].len();
    assert!(
        sent_len < input_len,
        "mechanical compaction must drop middle groups: sent {} of {}",
        sent_len,
        input_len
    );
    // ② trace 落值:compaction_json 存在(dropped_count > 0 路径)。
    let traces = crate::db::trace::list_turn_traces(&h.db, &h.session_id)
        .await
        .expect("list turn traces");
    assert!(
        traces.iter().any(|t| t.compaction_json.is_some()),
        "record_compaction must have persisted a compaction observation"
    );
}

/// AC6 集成半边:常态请求(未超 0.95 预算线)在硬卡默认开(fail-open)
/// 之下零干扰 —— 无 BudgetTrim 事件、无 context_budget_trim 审计行、
/// turn 正常 Done。硬卡臂级行为的确定性覆盖在 `agent::budget::tests`
/// (enforce_budget 单测;全 loop 逼出臂级行为会与环境合成头耦合,
/// 由 C3 StillOver 抢先中止,见任务 prd Review Resolution)。
#[tokio::test]
async fn budget_gate_default_on_does_not_fire_on_normal_request() {
    crate::memory::tokens::ensure_initialized().await;
    let h = make_harness().await;
    // budget gate 开关缺省 = 开(fail-open),不 set 即默认开。
    let mock = Arc::new(MockProvider::new(vec![end_turn_response("done")]));
    let emitter = Arc::new(MockEmitter::new());
    let wire = vec![
        user("hello"),
        assistant("hi there"),
        user("normal question"),
    ];
    run_loop_with_emitter(&h, mock.clone(), wire, emitter.clone()).await;

    // turn 正常完成。
    assert_eq!(mock.call_count(), 1);
    let events = emitter.chat_events();
    assert!(
        events
            .iter()
            .any(|p| matches!(&p.event, ChatEvent::Done { .. })),
        "常态请求必须照常 Done"
    );
    // 无 BudgetTrim 事件(未裁剪)。
    assert!(
        events
            .iter()
            .all(|p| !matches!(&p.event, ChatEvent::BudgetTrim { .. })),
        "未超线不得发 BudgetTrim"
    );
    // 无 context_budget_trim 审计行。
    let audit_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM session_audit_events WHERE session_id = ? AND kind = 'context_budget_trim'",
    )
    .bind(&h.session_id)
    .fetch_one(&h.db)
    .await
    .unwrap();
    assert_eq!(audit_rows, 0, "未裁剪不得落 context_budget_trim 审计");
}
