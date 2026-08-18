//! 手动 /compact 集成测试(08-18-manual-compact-command,design §6)。
//!
//! 公共口径:直调 `run_manual_compaction`(不经 `compact_session_inner`
//! 的 gate 链 —— 群聊/in-flight/config 开关在 daemon route 冒烟与
//! commands 层覆盖);MockProvider 单条摘要脚本;历史经 `persist_turn`
//! 预落 DB(空闲期 wire↔DB 1:1 的等价前提)。窗口 20_000:保留区预算
//! clamp 下限 15_000,24 条 pad(6000)(≈ 22-24k token)保证待压区
//! 非空;手动路径无 0.85 触发线,"低于触发线也能压"是 R1 的验收点。

use std::sync::Arc;

use super::tests_common::{make_harness, TestHarness};
use crate::agent::compaction::{
    apply_compaction_watermark, compaction_registry, latest_summary_anchor, run_manual_compaction,
    ManualCompactionError, COMPACTION_SUMMARY_KIND,
};
use crate::db::MessageRow;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, ChatMessage, MessageContent, Role, TokenUsage};

/// 与 compaction_summary.rs 同款窗口(触发线概念在手动路径不存在,
/// 保留区预算 = 15_000 下限)。
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

/// 摘要旁路 completion 的成功脚本(compaction_summary.rs 同款形态)。
fn summary_response(body: &str) -> MockResponse {
    MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta {
            text: body.to_string(),
        }),
        Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: Some(TokenUsage {
                input_tokens: 9_100,
                output_tokens: 500,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                context_input_tokens: 9_100,
            }),
        }),
    ])
}

/// 非 retryable 失败(Auth —— retry_open 单发即终,零退避延迟)。
fn summary_auth_failure() -> MockResponse {
    MockResponse::ErrThenEnd(crate::llm::error::LlmError::Auth("bad key".into()))
}

/// 交替 user/assistant 的 pad 历史(≈ 每条 1.5k token)。
fn padded_history(n: usize) -> Vec<ChatMessage> {
    (0..n)
        .map(|i| {
            if i % 2 == 0 {
                user(format!("HIST_{:02} {}", i, pad(6_000)))
            } else {
                assistant(format!("HIST_{:02} {}", i, pad(6_000)))
            }
        })
        .collect()
}

async fn seed(h: &TestHarness, rows: &[ChatMessage], seq_base: i64) {
    for (i, m) in rows.iter().enumerate() {
        crate::db::persist_turn(
            &h.db,
            &h.session_id,
            m.role,
            &m.content,
            seq_base + i as i64,
            None,
            None,
        )
        .await
        .expect("seed history row");
    }
}

async fn load_rows(h: &TestHarness) -> Vec<MessageRow> {
    crate::db::load_session(&h.db, &h.session_id)
        .await
        .expect("load_session")
        .expect("session exists")
        .messages
}

/// reload wire(前端 rehydrate 同款:text 列原文回灌)。
fn reload_wire(rows: &[MessageRow]) -> Vec<ChatMessage> {
    rows.iter()
        .map(|r| ChatMessage {
            role: if r.role == "assistant" {
                Role::Assistant
            } else {
                Role::User
            },
            content: MessageContent::Text(r.text.clone()),
            speaker: None,
            attachments: None,
        })
        .collect()
}

fn kind_of(row: &MessageRow) -> Option<&str> {
    row.metadata
        .as_ref()
        .and_then(|m| m.get("kind"))
        .and_then(|v| v.as_str())
}

/// 低阈值(远低于 0.85×window)也成功:摘要行落库(trigger=manual、
/// cutoff 精确指向真实行、seq=MAX+1),下一请求水位替换 Applied 且
/// **保留区跨请求存活**(manual 版的 PR2 check P1 回归守卫),熔断清零。
#[tokio::test(flavor = "multi_thread")]
async fn manual_compact_succeeds_below_trigger_line_and_writes_watermark() {
    let h = make_harness().await;
    compaction_registry().clear(&h.session_id).await;
    seed(&h, &padded_history(24), 0).await;
    let rows = load_rows(&h).await;
    let max_seq = rows.iter().map(|r| r.seq).max().unwrap();
    let last_row_text = rows.last().unwrap().text.clone();

    let provider = Arc::new(MockProvider::new(vec![summary_response("MANUAL V1")]));
    let outcome =
        run_manual_compaction(&h.db, &h.session_id, provider.clone(), WINDOW, None, &rows)
            .await
            .expect("manual compaction succeeds");
    assert_eq!(provider.call_count(), 1, "exactly one summary send");
    assert!(outcome.tokens_after < outcome.tokens_before);

    // 摘要行契约:trigger=manual、focus null、seq=MAX+1、cutoff 指向
    // 真实存在的行(精确值,非 seq-1 近似可指向不存在的行)。
    let rows2 = load_rows(&h).await;
    let summary_row = rows2
        .iter()
        .rev()
        .find(|r| kind_of(r) == Some(COMPACTION_SUMMARY_KIND))
        .expect("summary row persisted");
    assert_eq!(summary_row.seq, max_seq + 1, "insert seq = MAX(seq)+1");
    assert_eq!(summary_row.text, "MANUAL V1", "pure summary body");
    let meta = summary_row.metadata.as_ref().unwrap();
    assert_eq!(meta["trigger"], "manual");
    assert!(meta["focus"].is_null());
    assert!(meta["prior_summary_seq"].is_null());
    let cutoff = meta["cutoff_seq"].as_i64().unwrap();
    assert!(
        rows.iter().any(|r| r.seq == cutoff),
        "cutoff points at a real row ({cutoff})"
    );
    assert_eq!(rows2.len(), rows.len() + 1, "append-only, no deletion");

    // 水位替换(下一请求视角):Applied + anchor 指向新行 + 折叠尾部
    // 含最后一条历史行(保留区存活)。
    let (folded, anchor) = match apply_compaction_watermark(reload_wire(&rows2), &rows2) {
        crate::agent::compaction::WatermarkResult::Applied { messages, anchor } => {
            (messages, anchor)
        }
        other => panic!("watermark must apply after manual compaction: {other:?}"),
    };
    assert_eq!(anchor.seq, summary_row.seq);
    assert_eq!(
        folded.last().unwrap().content.to_text(),
        last_row_text,
        "preserved region survives the fold"
    );
    assert_eq!(
        compaction_registry().failures(&h.session_id).await,
        0,
        "success clears the breaker"
    );
}

/// focus 参数注入:prompt 含定向指令块,metadata 落 focus 原文。
#[tokio::test(flavor = "multi_thread")]
async fn manual_compact_injects_focus_into_prompt() {
    let h = make_harness().await;
    seed(&h, &padded_history(24), 0).await;
    let rows = load_rows(&h).await;

    let provider = Arc::new(MockProvider::new(vec![summary_response("FOCUSED")]));
    run_manual_compaction(
        &h.db,
        &h.session_id,
        provider.clone(),
        WINDOW,
        Some("聚焦 API 变更"),
        &rows,
    )
    .await
    .expect("manual compaction succeeds");

    let sent = provider.sent_messages();
    let prompt = sent[0][0].content.to_text();
    assert!(
        prompt.contains("FOCUS INSTRUCTIONS FROM THE USER: 聚焦 API 变更"),
        "focus block injected: {}",
        &prompt[..prompt.len().min(120)]
    );

    let rows2 = load_rows(&h).await;
    let summary_row = rows2
        .iter()
        .rev()
        .find(|r| kind_of(r) == Some(COMPACTION_SUMMARY_KIND))
        .unwrap();
    assert_eq!(
        summary_row.metadata.as_ref().unwrap()["focus"],
        "聚焦 API 变更"
    );
}

/// 已有水位 → 增量合并:prompt 注 <prior-summary>(V1 内容),新行
/// prior_summary_seq 指旧行;旧摘要行保留(DB 无损);水位推进到 V2。
#[tokio::test(flavor = "multi_thread")]
async fn manual_compact_merges_with_existing_watermark() {
    let h = make_harness().await;
    seed(&h, &padded_history(24), 0).await;

    // 第一次手动压缩 → V1 水位。
    let rows1 = load_rows(&h).await;
    let p1 = Arc::new(MockProvider::new(vec![summary_response("V1 SUMMARY")]));
    run_manual_compaction(&h.db, &h.session_id, p1, WINDOW, None, &rows1)
        .await
        .expect("first manual compaction");
    let rows2 = load_rows(&h).await;
    let v1_row = rows2
        .iter()
        .rev()
        .find(|r| kind_of(r) == Some(COMPACTION_SUMMARY_KIND))
        .unwrap()
        .clone();

    // 水位之后再追加新历史(seq 续在 MAX+1 之后)。
    let seq_base = rows2.iter().map(|r| r.seq).max().unwrap() + 1;
    seed(&h, &padded_history(6), seq_base).await;

    // 第二次手动压缩 → V2(增量合并)。
    let rows3 = load_rows(&h).await;
    let p2 = Arc::new(MockProvider::new(vec![summary_response("V2 SUMMARY")]));
    run_manual_compaction(&h.db, &h.session_id, p2.clone(), WINDOW, None, &rows3)
        .await
        .expect("second manual compaction");

    let prompt = p2.sent_messages()[0][0].content.to_text();
    assert!(prompt.contains("<prior-summary>"), "prior block injected");
    assert!(prompt.contains("V1 SUMMARY"), "prior content fed");

    let rows4 = load_rows(&h).await;
    let v2_row = rows4
        .iter()
        .rev()
        .find(|r| kind_of(r) == Some(COMPACTION_SUMMARY_KIND))
        .unwrap();
    assert_eq!(v2_row.seq, v1_row.seq + 7, "seq = MAX+1 across the append");
    assert_eq!(
        v2_row.metadata.as_ref().unwrap()["prior_summary_seq"],
        v1_row.seq,
        "incremental merge chains to the prior row"
    );
    assert!(
        rows4.iter().any(|r| r.seq == v1_row.seq),
        "prior summary row kept (append-only)"
    );

    // 水位推进:latest_summary_anchor 现指向 V2。
    let anchor = latest_summary_anchor(&rows4).expect("anchor from rows");
    assert_eq!(anchor.content, "V2 SUMMARY");
    assert_eq!(anchor.seq, v2_row.seq);
}

/// 摘要失败:零 DB 写入(行数不变、无摘要行),熔断计数 +1。
#[tokio::test(flavor = "multi_thread")]
async fn manual_compact_failure_writes_nothing_and_counts_breaker() {
    let h = make_harness().await;
    compaction_registry().clear(&h.session_id).await;
    seed(&h, &padded_history(24), 0).await;
    let rows = load_rows(&h).await;

    let provider = Arc::new(MockProvider::new(vec![summary_auth_failure()]));
    let err = run_manual_compaction(&h.db, &h.session_id, provider.clone(), WINDOW, None, &rows)
        .await
        .expect_err("auth failure must fail the compaction");
    assert!(
        matches!(err, ManualCompactionError::SummaryFailed(_)),
        "got {err:?}"
    );
    assert_eq!(provider.call_count(), 1, "non-retryable: single attempt");

    let rows2 = load_rows(&h).await;
    assert_eq!(rows2.len(), rows.len(), "zero DB writes on failure");
    assert!(
        rows2
            .iter()
            .all(|r| kind_of(r) != Some(COMPACTION_SUMMARY_KIND)),
        "no summary row"
    );
    assert_eq!(
        compaction_registry().failures(&h.session_id).await,
        1,
        "failure recorded (shared signal with the auto path)"
    );
}

/// 空待压区:历史几乎全进保留区 → NothingToCompress,零 LLM 调用,
/// 不计熔断。
#[tokio::test(flavor = "multi_thread")]
async fn manual_compact_rejects_when_nothing_to_compress() {
    let h = make_harness().await;
    compaction_registry().clear(&h.session_id).await;
    seed(&h, &[user("hi there")], 0).await;
    let rows = load_rows(&h).await;

    let provider = Arc::new(MockProvider::new(vec![summary_response("X")]));
    let err = run_manual_compaction(&h.db, &h.session_id, provider.clone(), WINDOW, None, &rows)
        .await
        .expect_err("nothing compressible");
    assert!(
        matches!(err, ManualCompactionError::NothingToCompress),
        "got {err:?}"
    );
    assert_eq!(provider.call_count(), 0, "no LLM call");
    assert_eq!(
        compaction_registry().failures(&h.session_id).await,
        0,
        "no-op does not touch the breaker"
    );
}

/// 熔断 tripped 不拦手动入口(prd D6),成功后解熔断。
#[tokio::test(flavor = "multi_thread")]
async fn manual_compact_bypasses_tripped_breaker_and_untrips_on_success() {
    let h = make_harness().await;
    for _ in 0..3 {
        compaction_registry().record_failure(&h.session_id).await;
    }
    assert!(
        compaction_registry().is_tripped(&h.session_id).await,
        "precondition: tripped"
    );

    seed(&h, &padded_history(24), 0).await;
    let rows = load_rows(&h).await;
    let provider = Arc::new(MockProvider::new(vec![summary_response("RECOVER")]));
    run_manual_compaction(&h.db, &h.session_id, provider, WINDOW, None, &rows)
        .await
        .expect("manual entry bypasses the breaker");
    assert!(
        !compaction_registry().is_tripped(&h.session_id).await,
        "success un-trips"
    );
}
