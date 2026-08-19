//! handoff 跨 session 接力集成测试(08-18-handoff-mechanism,design §6)。
//!
//! 公共口径(对齐 manual_compaction.rs):直调 `generate_handoff_summary`
//! + `persist_handoff_child`(gate 链在 daemon route 冒烟覆盖);历史经
//! `persist_turn` 预落 DB;MockProvider 摘要脚本**必须含必含段**(Work
//! State / Next Step —— D4 校验对 mock 产出同样生效)。

use std::sync::Arc;

use super::tests_common::{make_harness, TestHarness};
use crate::agent::compaction::{
    apply_compaction_watermark, compaction_registry, generate_handoff_summary,
    latest_summary_anchor, run_manual_compaction, HandoffGenerationError, WatermarkResult,
    COMPACTION_SUMMARY_KIND, HANDOFF_SUMMARY_KIND, SUMMARY_CONTEXT_PREFIX,
};
use crate::commands::sessions::persist_handoff_child;
use crate::db::MessageRow;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, ChatMessage, MessageContent, Role, TokenUsage};

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

/// 含全部必含段的合法摘要脚本(Work State / Next Step 标题 + 非空正文;
/// 注意 Next Step 标题后必须有正文行 —— 校验器契约)。
fn valid_summary(tag: &str) -> String {
    format!(
        "1. Primary Request and Intent — {tag}\n\
         2. Key Technical Concepts — {tag}\n\
         3. Files and Code Sections — src/main.rs\n\
         4. Errors and Fixes — none\n\
         5. Work State — Completed: setup; In progress: wiring\n\
         6. Optional Next Step — run the test suite next ({tag})\n\
         Continue with the failing test, then commit."
    )
}

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

// ---------------------------------------------------------------------------
// validate_handoff_summary 单元矩阵
// ---------------------------------------------------------------------------

#[test]
fn validate_accepts_template_shaped_summary() {
    assert!(crate::agent::compaction::validate_handoff_summary(&valid_summary("X")).is_empty());
}

#[test]
fn validate_flags_missing_sections() {
    let no_next = "1. Primary Request\n5. Work State — done things\n- item";
    let missing = crate::agent::compaction::validate_handoff_summary(no_next);
    assert_eq!(missing, vec!["Next Step"]);

    let no_work = "1. Primary Request\n6. Optional Next Step — do it\n- action item";
    let missing = crate::agent::compaction::validate_handoff_summary(no_work);
    assert_eq!(missing, vec!["Work State"]);

    let missing = crate::agent::compaction::validate_handoff_summary("garbage");
    assert_eq!(missing, vec!["Work State", "Next Step"]);
}

#[test]
fn validate_tolerates_header_decorations_and_case() {
    // 井号 / 编号 / "Optional" 前缀 / 大小写差异都按子串识别。
    let text = "## 5. Work state — all green\nbody line\n### 6. optional next step\nbody";
    assert!(crate::agent::compaction::validate_handoff_summary(text).is_empty());
}

#[test]
fn validate_flags_header_without_any_body() {
    // "Work State" 标题后到文末无任何非空行 → 缺段。
    let text = "5. Work State";
    assert_eq!(
        crate::agent::compaction::validate_handoff_summary(text),
        vec!["Work State", "Next Step"]
    );
}

// ---------------------------------------------------------------------------
// generate_handoff_summary
// ---------------------------------------------------------------------------

/// 全量覆盖 + 接力落库端到端(AC1/AC3/AC4):摘要 prompt 含**最后一行**
/// 历史(与 /compact 的保留区差异);子 session 首条 = prefix+摘要、
/// kind=handoff_summary、seq=1;双向 metadata 关联;parent 行数不变。
#[tokio::test(flavor = "multi_thread")]
async fn handoff_generates_full_coverage_and_persists_child_session() {
    let h = make_harness().await;
    compaction_registry().clear(&h.session_id).await;
    seed(&h, &padded_history(24), 0).await;
    let rows = load_rows(&h).await;
    let parent_before = rows.len();
    let parent_row = crate::db::load_session(&h.db, &h.session_id)
        .await
        .unwrap()
        .unwrap()
        .session;
    let last_marker = "HIST_23";

    let provider = Arc::new(MockProvider::new(vec![summary_response(&valid_summary(
        "V1",
    ))]));
    let gen = generate_handoff_summary(&h.session_id, provider.clone(), WINDOW, None, &rows)
        .await
        .expect("handoff generation succeeds");
    assert_eq!(provider.call_count(), 1);
    assert!(!gen.from_prior_fast_path);

    // 全量覆盖(评审 ① 回归锚):prompt transcript 含最后一行历史 ——
    // 若漏构造 anchor 占位,被跳的会是 prior 后最旧一行;这里无 prior,
    // 该断言主要锁"尾部不被保留区排除"。
    let prompt = provider.sent_messages()[0][0].content.to_text();
    assert!(
        prompt.contains(last_marker),
        "last history row in transcript"
    );

    let outcome = persist_handoff_child(&h.db, &parent_row, &gen, None)
        .await
        .expect("persist child");

    // 子 session:标题 + 首行契约(prefix 自包含、seq=1)。
    let child = crate::db::load_session(&h.db, &outcome.new_session_id)
        .await
        .unwrap()
        .expect("child session exists");
    assert_eq!(child.session.title, format!("接力: {}", parent_row.title));
    assert_eq!(child.messages.len(), 1);
    let row = &child.messages[0];
    assert_eq!(row.seq, 1);
    assert_eq!(kind_of(row), Some(HANDOFF_SUMMARY_KIND));
    assert!(
        row.text.starts_with(SUMMARY_CONTEXT_PREFIX),
        "prefix persisted"
    );
    assert!(row.text.contains("Work State"), "summary body present");
    // content 列 = 单 Text 块 JSON(两列同载完整 prefix+摘要)。
    assert!(row.content.is_array(), "content is JSON blocks");
    assert_eq!(row.content[0]["type"], "text");
    assert_eq!(row.content[0]["text"], row.text);
    let meta = row.metadata.as_ref().unwrap();
    assert_eq!(meta["parent_session_id"], h.session_id);
    assert_eq!(meta["trigger"], "handoff");

    // 双向 metadata(AC3)。
    assert_eq!(
        child.session.metadata.as_ref().unwrap()["handoff"]["parent_session_id"],
        h.session_id
    );
    let parent_after = crate::db::load_session(&h.db, &h.session_id)
        .await
        .unwrap()
        .unwrap();
    let children = parent_after.session.metadata.as_ref().unwrap()["handoff_children"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0], serde_json::json!(outcome.new_session_id));

    // 原会话完好(AC4)。
    assert_eq!(parent_after.messages.len(), parent_before);

    // 水位语义:handoff 行不参与锚点检测(NoWatermark),wire 中就是
    // 普通 user 行(自包含 prefix)。
    let child_wire = reload_wire(&child.messages);
    match apply_compaction_watermark(child_wire.clone(), &child.messages) {
        WatermarkResult::Miss {
            reason: crate::agent::compaction::MissReason::NoWatermark,
            ..
        } => {}
        other => panic!("handoff row must not act as a watermark anchor: {other:?}"),
    }
    assert!(child_wire[0]
        .content
        .to_text()
        .starts_with(SUMMARY_CONTEXT_PREFIX));
}

/// anchor 占位契约(评审 ①):已有水位时,水位后**第一条**常规行必须
/// 进 transcript(prompt 含其标记),prior 内容经 <prior-summary> 块进场。
#[tokio::test(flavor = "multi_thread")]
async fn handoff_with_prior_anchor_keeps_first_post_cutoff_row_in_transcript() {
    let h = make_harness().await;
    seed(&h, &padded_history(24), 0).await;
    let rows1 = load_rows(&h).await;

    // 先手动压缩建水位(V1)。
    let p1 = Arc::new(MockProvider::new(vec![summary_response(&valid_summary(
        "V1",
    ))]));
    run_manual_compaction(&h.db, &h.session_id, p1, WINDOW, None, &rows1)
        .await
        .expect("seed watermark via manual compaction");

    // 水位后追加 6 行;第一行标记 HIST_FIRST_AFTER_CUTOFF。
    let rows2 = load_rows(&h).await;
    let seq_base = rows2.iter().map(|r| r.seq).max().unwrap() + 1;
    let mut tail: Vec<ChatMessage> = vec![user("HIST_FIRST_AFTER_CUTOFF marker content")];
    tail.extend(padded_history(4));
    seed(&h, &tail, seq_base).await;

    let rows3 = load_rows(&h).await;
    let provider = Arc::new(MockProvider::new(vec![summary_response(&valid_summary(
        "V2",
    ))]));
    generate_handoff_summary(&h.session_id, provider.clone(), WINDOW, None, &rows3)
        .await
        .expect("handoff with prior");

    let prompt = provider.sent_messages()[0][0].content.to_text();
    assert!(prompt.contains("<prior-summary>"), "prior block injected");
    assert!(
        prompt.contains("HIST_FIRST_AFTER_CUTOFF"),
        "first post-cutoff row must NOT be skipped as anchor placeholder"
    );
}

/// D4 校验重试成功路径:首份缺 Next Step → 纠正重试 → 第二份合法。
#[tokio::test(flavor = "multi_thread")]
async fn handoff_retries_with_correction_when_section_missing() {
    let h = make_harness().await;
    compaction_registry().clear(&h.session_id).await;
    seed(&h, &padded_history(24), 0).await;
    let rows = load_rows(&h).await;

    let bad = "1. Primary Request — x\n5. Work State — in progress\n- item";
    let provider = Arc::new(MockProvider::new(vec![
        summary_response(bad),
        summary_response(&valid_summary("FIXED")),
    ]));
    let gen = generate_handoff_summary(&h.session_id, provider.clone(), WINDOW, None, &rows)
        .await
        .expect("retry succeeds");
    assert_eq!(provider.call_count(), 2, "one correction retry");
    let retry_prompt = provider.sent_messages()[1][0].content.to_text();
    assert!(
        retry_prompt.contains("CORRECTION") && retry_prompt.contains("Next Step"),
        "correction block names the missing section"
    );
    assert!(gen.summary_text.contains("FIXED"));
    assert_eq!(
        compaction_registry().failures(&h.session_id).await,
        0,
        "final success clears the breaker"
    );
}

/// D4 恒缺路径:两次都缺段 → SummaryMissingSections,熔断 +1,零副作用
/// (无新 session 行;parent 行数不变)。
#[tokio::test(flavor = "multi_thread")]
async fn handoff_missing_sections_after_retry_is_error_with_zero_side_effect() {
    let h = make_harness().await;
    compaction_registry().clear(&h.session_id).await;
    seed(&h, &padded_history(24), 0).await;
    let rows = load_rows(&h).await;
    let sessions_before = crate::db::sessions::list_sessions(&h.db, &h.project_id)
        .await
        .unwrap()
        .len();

    let bad = "1. Primary Request — x\n5. Work State — only this\n- one item";
    let provider = Arc::new(MockProvider::new(vec![
        summary_response(bad),
        summary_response(bad),
    ]));
    let err = generate_handoff_summary(&h.session_id, provider.clone(), WINDOW, None, &rows)
        .await
        .expect_err("missing sections must fail");
    match err {
        HandoffGenerationError::SummaryMissingSections(sections) => {
            assert_eq!(sections, vec!["Next Step"]);
        }
        other => panic!("expected SummaryMissingSections, got {other:?}"),
    }
    assert_eq!(provider.call_count(), 2);
    assert_eq!(
        compaction_registry().failures(&h.session_id).await,
        1,
        "final failure recorded once (intermediate retry not counted)"
    );
    assert_eq!(
        load_rows(&h).await.len(),
        rows.len(),
        "parent rows untouched"
    );
    assert_eq!(
        crate::db::sessions::list_sessions(&h.db, &h.project_id)
            .await
            .unwrap()
            .len(),
        sessions_before,
        "no child session created"
    );
}

/// 快路径:水位后无新常规行 → prior 摘要原样接力,零 LLM 调用。
/// 构造方式:直接插一行 cutoff = 末行 seq 的水位摘要(manual 压缩总会
/// 留保留区,正常流程到不了这个态;该路径为 D3 自愈等边界的防御短路)。
#[tokio::test(flavor = "multi_thread")]
async fn handoff_fast_path_reuses_prior_summary_without_llm() {
    let h = make_harness().await;
    seed(&h, &padded_history(4), 0).await;
    let rows1 = load_rows(&h).await;
    let v1 = valid_summary("FASTPATH");
    let max_seq = rows1.iter().map(|r| r.seq).max().unwrap();
    crate::db::insert_compaction_summary(
        &h.db,
        &h.session_id,
        &v1,
        max_seq + 1,
        &serde_json::json!({
            "kind": COMPACTION_SUMMARY_KIND,
            "cutoff_seq": max_seq,
        }),
    )
    .await
    .expect("seed full-coverage anchor");

    let rows2 = load_rows(&h).await;
    let provider = Arc::new(MockProvider::new(vec![]));
    let gen = generate_handoff_summary(&h.session_id, provider.clone(), WINDOW, None, &rows2)
        .await
        .expect("fast path");
    assert_eq!(provider.call_count(), 0, "zero LLM calls");
    assert!(gen.from_prior_fast_path);
    assert_eq!(gen.summary_text, v1, "prior content relayed verbatim");
}

/// 快路径退化:prior 摘要缺段 → 走 LLM 补段(D4 对快路径同样成立)。
#[tokio::test(flavor = "multi_thread")]
async fn handoff_fast_path_falls_back_to_llm_when_prior_missing_sections() {
    let h = make_harness().await;
    seed(&h, &padded_history(24), 0).await;
    let rows1 = load_rows(&h).await;
    let p1 = Arc::new(MockProvider::new(vec![summary_response(
        "only work state, no next step",
    )]));
    run_manual_compaction(&h.db, &h.session_id, p1, WINDOW, None, &rows1)
        .await
        .expect("seed watermark (its content lacks sections)");

    let rows2 = load_rows(&h).await;
    let provider = Arc::new(MockProvider::new(vec![summary_response(&valid_summary(
        "REGEN",
    ))]));
    let gen = generate_handoff_summary(&h.session_id, provider.clone(), WINDOW, None, &rows2)
        .await
        .expect("LLM fallback");
    assert_eq!(provider.call_count(), 1);
    assert!(!gen.from_prior_fast_path);
    assert!(gen.summary_text.contains("REGEN"));
}

/// 空会话拒绝:无常规行且无水位 → NothingToHandoff,零 LLM、零熔断。
#[tokio::test(flavor = "multi_thread")]
async fn handoff_rejects_empty_session() {
    let h = make_harness().await;
    compaction_registry().clear(&h.session_id).await;
    let rows = load_rows(&h).await;
    assert!(rows.is_empty());

    let provider = Arc::new(MockProvider::new(vec![summary_response("X")]));
    let err = generate_handoff_summary(&h.session_id, provider.clone(), WINDOW, None, &rows)
        .await
        .expect_err("empty session");
    assert!(matches!(err, HandoffGenerationError::NothingToHandoff));
    assert_eq!(provider.call_count(), 0);
    assert_eq!(compaction_registry().failures(&h.session_id).await, 0);
}

/// 摘要流失败:SummaryFailed + 熔断 +1(与 manual 共享信号)。
#[tokio::test(flavor = "multi_thread")]
async fn handoff_summary_failure_counts_breaker() {
    let h = make_harness().await;
    compaction_registry().clear(&h.session_id).await;
    seed(&h, &padded_history(24), 0).await;
    let rows = load_rows(&h).await;

    let provider = Arc::new(MockProvider::new(vec![summary_auth_failure()]));
    let err = generate_handoff_summary(&h.session_id, provider.clone(), WINDOW, None, &rows)
        .await
        .expect_err("auth failure");
    assert!(matches!(err, HandoffGenerationError::SummaryFailed(_)));
    assert_eq!(compaction_registry().failures(&h.session_id).await, 1);
}

/// focus 注入:prompt 含定向块(与 manual 同一管道)。
#[tokio::test(flavor = "multi_thread")]
async fn handoff_injects_focus_into_prompt() {
    let h = make_harness().await;
    seed(&h, &padded_history(24), 0).await;
    let rows = load_rows(&h).await;

    let provider = Arc::new(MockProvider::new(vec![summary_response(&valid_summary(
        "F",
    ))]));
    generate_handoff_summary(
        &h.session_id,
        provider.clone(),
        WINDOW,
        Some("聚焦 API 变更"),
        &rows,
    )
    .await
    .expect("focus handoff");
    let prompt = provider.sent_messages()[0][0].content.to_text();
    assert!(prompt.contains("FOCUS INSTRUCTIONS FROM THE USER: 聚焦 API 变更"));
}

/// 继承断言:parent 带 worktree/workflow/plugin/mode 偏离默认时,子
/// session 逐项继承;parent 在默认态时子全默认(跳过无谓 UPDATE)。
#[tokio::test(flavor = "multi_thread")]
async fn handoff_child_inherits_parent_fields() {
    let h = make_harness().await;
    seed(&h, &padded_history(24), 0).await;
    let rows = load_rows(&h).await;
    let mut parent = crate::db::load_session(&h.db, &h.session_id)
        .await
        .unwrap()
        .unwrap()
        .session;
    crate::db::sessions::set_worktree_state(
        &h.db,
        &parent.id,
        crate::db::WorktreeState::Active,
        Some("/tmp/wt-handoff"),
        None,
    )
    .await
    .unwrap();
    crate::db::sessions::set_session_workflow_enabled(&h.db, &parent.id, true)
        .await
        .unwrap();
    crate::db::sessions::set_session_plugin_name(&h.db, &parent.id, "review")
        .await
        .unwrap();
    crate::commands::question::set_session_mode_internal(&h.db, &parent.id, crate::db::Mode::Plan)
        .await
        .unwrap();
    parent = crate::db::load_session(&h.db, &parent.id)
        .await
        .unwrap()
        .unwrap()
        .session;

    let provider = Arc::new(MockProvider::new(vec![summary_response(&valid_summary(
        "INH",
    ))]));
    let gen = generate_handoff_summary(&h.session_id, provider, WINDOW, None, &rows)
        .await
        .unwrap();
    let outcome = persist_handoff_child(&h.db, &parent, &gen, Some("聚焦 X"))
        .await
        .unwrap();

    let child = crate::db::load_session(&h.db, &outcome.new_session_id)
        .await
        .unwrap()
        .unwrap()
        .session;
    assert_eq!(child.project_id, parent.project_id);
    assert_eq!(child.current_cwd, parent.current_cwd);
    assert_eq!(child.model, parent.model);
    assert_eq!(child.model_id, parent.model_id);
    assert_eq!(child.worktree_state, crate::db::WorktreeState::Active);
    assert_eq!(child.worktree_path.as_deref(), Some("/tmp/wt-handoff"));
    assert!(child.workflow_enabled);
    assert_eq!(child.plugin_name, "review");
    assert_eq!(child.mode, crate::db::Mode::Plan);
    assert_eq!(
        child.session_type,
        crate::db::SessionType::Chat,
        "handoff child is a classic chat session"
    );
    assert_eq!(
        child.metadata.as_ref().unwrap()["handoff"]["focus"],
        "聚焦 X"
    );
}

/// 多次接力:parent 的 handoff_children 追加且不 clobber 已有 metadata
/// 键(既有 handoff 块共存)。
#[tokio::test(flavor = "multi_thread")]
async fn handoff_children_list_appends_across_relay() {
    let h = make_harness().await;
    seed(&h, &padded_history(24), 0).await;
    let rows = load_rows(&h).await;
    let parent = crate::db::load_session(&h.db, &h.session_id)
        .await
        .unwrap()
        .unwrap()
        .session;

    let p = Arc::new(MockProvider::new(vec![
        summary_response(&valid_summary("R1")),
        summary_response(&valid_summary("R2")),
    ]));
    let gen1 = generate_handoff_summary(&h.session_id, p.clone(), WINDOW, None, &rows)
        .await
        .unwrap();
    let out1 = persist_handoff_child(&h.db, &parent, &gen1, None)
        .await
        .unwrap();
    let gen2 = generate_handoff_summary(&h.session_id, p, WINDOW, None, &rows)
        .await
        .unwrap();
    let out2 = persist_handoff_child(&h.db, &parent, &gen2, None)
        .await
        .unwrap();

    let parent_after = crate::db::load_session(&h.db, &h.session_id)
        .await
        .unwrap()
        .unwrap()
        .session;
    let meta = parent_after.metadata.as_ref().unwrap();
    let children = meta["handoff_children"].as_array().unwrap();
    assert_eq!(children.len(), 2);
    assert_eq!(children[0], serde_json::json!(out1.new_session_id));
    assert_eq!(children[1], serde_json::json!(out2.new_session_id));
}

/// `SummaryAnchor` 仍只认 compaction_summary:子 session 行集只含
/// handoff 行时 anchor 为 None(handoff 行不被误当水位,契约回归锚)。
#[tokio::test(flavor = "multi_thread")]
async fn latest_anchor_ignores_handoff_rows() {
    let h = make_harness().await;
    seed(&h, &padded_history(24), 0).await;
    let rows = load_rows(&h).await;
    let parent = crate::db::load_session(&h.db, &h.session_id)
        .await
        .unwrap()
        .unwrap()
        .session;

    let provider = Arc::new(MockProvider::new(vec![summary_response(&valid_summary(
        "A",
    ))]));
    let gen = generate_handoff_summary(&h.session_id, provider, WINDOW, None, &rows)
        .await
        .unwrap();
    let out = persist_handoff_child(&h.db, &parent, &gen, None)
        .await
        .unwrap();

    let child_rows = crate::db::load_session(&h.db, &out.new_session_id)
        .await
        .unwrap()
        .unwrap()
        .messages;
    assert_eq!(child_rows.len(), 1);
    assert_eq!(kind_of(&child_rows[0]), Some(HANDOFF_SUMMARY_KIND));
    assert!(
        latest_summary_anchor(&child_rows).is_none(),
        "handoff rows never act as anchors"
    );
    assert!(
        rows.iter()
            .all(|r| kind_of(r) != Some(COMPACTION_SUMMARY_KIND)),
        "parent has no compaction watermark either"
    );
}
