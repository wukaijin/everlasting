#![cfg(test)]
// C3 摘要压缩 PR2 集成测试(08-18-llm-context-compaction,design §9)。
//
// 公共口径:
// - `context_window = 20_000`:触发线 0.85 → 17_000 token;保留区预算
//   clamp(15_000, 2_000, 25_000) → 15_000(下限);摘要后复查线 19_000。
//   历史构造为 ~24 条 pad(4000)(≈ 22-24k token)—— 超触发线、保留区
//   吃 15k 后仍有 7k+ 待压区。
// - 摘要调用是 MockProvider 的**旁路 send**(脚本顺序:摘要条目排在
//   对应主 turn 条目之前)。识别口径:摘要 send 的首条消息含
//   "CONTEXT CHECKPOINT COMPACTION"(主 turn 不会)。
// - 合成头随测试机环境的 user-scope memory/skill 层漂移(0..3,
//   开发机实测 P=3:User CLAUDE.md 头对 + skill listing)—— 所有
//   断言位置无关(算法本就不依赖摘要消息的绝对位置,评审 P1-1)。

use std::sync::Arc;

use super::tests_common::{
    chat_loop_deps, chat_loop_request, make_harness, parent_role, MockEmitter, TestHarness,
};
use crate::agent::chat_loop::run_chat_loop;
use crate::agent::compaction::{compaction_registry, COMPACTION_SUMMARY_KIND};
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, ChatMessage, MessageContent, Role, TokenUsage};

/// 测试窗口:trigger = 17_000,preservation = 15_000,postcheck = 19_000。
const WINDOW: u32 = 20_000;

/// 大 padding 文本(~4 chars/token)。
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

/// 超线历史:24 条 pad(6000) 交替 user/assistant(≈ 27k token,留足
/// 待压区:27k − 触发线 17k − 保留区 15k 后仍 > 8k)+ 尾部当前输入。
/// 含 tool 配对组(AC6 配对不变量扫描素材)。
fn overline_history() -> Vec<ChatMessage> {
    let mut msgs = Vec::new();
    for i in 0..24 {
        if i % 4 == 3 {
            // assistant(tool_use) + user(tool_result) 配对组。
            msgs.push(ChatMessage {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![crate::llm::types::ContentBlock::ToolUse {
                    id: format!("tu_hist_{i}"),
                    name: "list_dir".to_string(),
                    input: serde_json::json!({"path": "."}),
                }]),
                speaker: None,
                attachments: None,
            });
            msgs.push(ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![
                    crate::llm::types::ContentBlock::ToolResult {
                        tool_use_id: format!("tu_hist_{i}"),
                        content: pad(5_800),
                        is_error: false,
                        images: None,
                        resolved: None,
                    },
                ]),
                speaker: None,
                attachments: None,
            });
        } else if i % 2 == 0 {
            msgs.push(user(pad(6_000)));
        } else {
            msgs.push(assistant(pad(6_000)));
        }
    }
    msgs.push(user("current question"));
    msgs
}

/// 带唯一文本标记的超线历史(P1 回归测试用):每条 pad 行前缀
/// `HIST_{i}`,成员断言无歧义。tool 配对组的 text 列为空(to_text
/// 只聚合 Text 块),成员断言跳过空 text 行。
fn distinct_overline_history() -> Vec<ChatMessage> {
    let mut msgs = Vec::new();
    for i in 0..24 {
        if i % 4 == 3 {
            msgs.push(ChatMessage {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![crate::llm::types::ContentBlock::ToolUse {
                    id: format!("tu_p1_{i}"),
                    name: "list_dir".to_string(),
                    input: serde_json::json!({"path": "."}),
                }]),
                speaker: None,
                attachments: None,
            });
            msgs.push(ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![
                    crate::llm::types::ContentBlock::ToolResult {
                        tool_use_id: format!("tu_p1_{i}"),
                        content: format!("TR_{:02} {}", i, pad(5_800)),
                        is_error: false,
                        images: None,
                        resolved: None,
                    },
                ]),
                speaker: None,
                attachments: None,
            });
        } else if i % 2 == 0 {
            msgs.push(user(format!("HIST_{:02} {}", i, pad(6_000))));
        } else {
            msgs.push(assistant(format!("HIST_{:02} {}", i, pad(6_000))));
        }
    }
    msgs.push(user("current question"));
    msgs
}

/// 摘要调用的 MockResponse(旁路 completion 的成功形态)。
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

/// 主 turn 的文本终态响应。
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

/// 非 retryable 的摘要失败(Auth —— retry_open 单发即终,零退避延迟)。
fn summary_auth_failure() -> MockResponse {
    MockResponse::ErrThenEnd(crate::llm::error::LlmError::Auth("bad key".into()))
}

/// 该 send 是否摘要旁路调用(首条消息是压缩 prompt)。
fn is_summary_send(msgs: &[ChatMessage]) -> bool {
    msgs.first().is_some_and(|m| {
        m.content
            .to_text()
            .contains("CONTEXT CHECKPOINT COMPACTION")
    })
}

/// 显式打开摘要压缩(harness 默认关,同 stub 先例)。
async fn enable_compaction(h: &TestHarness) {
    crate::db::config::set_config_value(&h.db, "llm_compaction_enabled", "true")
        .await
        .expect("set llm_compaction_enabled=true");
}

/// 历史拆成 (预落行, 尾部当前输入):`overline_history*` 的最后一条
/// 是当前输入(init 会 persist 它,不入预落集)。
fn split_history(history: &[ChatMessage]) -> (Vec<ChatMessage>, ChatMessage) {
    let mut rows = history.to_vec();
    let tail = rows.pop().expect("history non-empty");
    (rows, tail)
}

/// 预落历史到 DB(seq = 下标,镜像生产 reload;PR2.5 后 cutoff 精确
/// 计算依赖 wire ↔ DB 行序 1:1 —— 直灌 wire 而 DB 空的铺法会让对齐
/// 防御拒绝摘要,不再是合法测试前置)。
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

/// reload wire:DB 全行 → ChatMessage(text 列原文 + Text 形态,
/// 前端 rehydrate 管线同款回灌)。
fn reload_wire(loaded: &crate::db::LoadedSession) -> Vec<ChatMessage> {
    loaded
        .messages
        .iter()
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

/// 标准(非 worker / 非群聊)run_chat_loop 调用。fixture 缺省 +
/// `is_worker` 具名差异(RULE-ARGS-001)。
async fn run_loop(
    h: &TestHarness,
    provider: Arc<MockProvider>,
    rid: &str,
    messages: Vec<ChatMessage>,
    emitter: Arc<MockEmitter>,
    is_worker: bool,
    session_id: String,
) {
    run_chat_loop(
        chat_loop_request(
            vec![],
            provider,
            WINDOW,
            rid.into(),
            session_id,
            messages,
            emitter.clone(),
        ),
        chat_loop_deps(&h),
        {
            let mut role = parent_role(&h);
            // worker gate 测试:skip_persist = is_worker。
            role.skip_persist = is_worker;
            role.is_worker = Some(is_worker);
            role
        },
    )
    .await;
}

// ---------------------------------------------------------------------------
// 1) 超线触发摘要:第二次 send(主 turn)以摘要开头且骤减;
//    摘要调用次数 = 1;DB 摘要行落库;trace method=summary;
//    RULE-A-001 配对不变量扫描(AC1/AC5/AC6)。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn overline_history_compacts_via_summary() {
    let h = make_harness().await;
    enable_compaction(&h).await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        summary_response("SUMMARY_BODY_MARKER"),
        end_turn_response("final answer"),
    ]));
    let history = overline_history();
    let original_len = history.len();
    // 预落历史(生产 wire 镜像 DB reload;PR2.5 cutoff 依赖行序对齐)。
    let (seed_rows, tail) = split_history(&history);
    seed_history(&h, &seed_rows).await;
    let mut first_wire = seed_rows.clone();
    first_wire.push(tail);

    run_loop(
        &h,
        mock.clone(),
        "rid-sum-1",
        first_wire,
        emitter.clone(),
        false,
        h.session_id.clone(),
    )
    .await;

    let sends = mock.sent_messages();
    // 摘要调用恰好 1 次,且在主 turn 之前(脚本顺序)。
    let summary_sends: Vec<&Vec<ChatMessage>> =
        sends.iter().filter(|m| is_summary_send(m)).collect();
    assert_eq!(
        summary_sends.len(),
        1,
        "摘要调用次数 = 1(实际 {} sends 总 {})",
        summary_sends.len(),
        sends.len()
    );
    assert_eq!(mock.call_count(), 2, "1 摘要 + 1 主 turn");
    // 主 turn 请求含 [前缀+摘要] 回填消息且长度骤减。**位置无关断言**
    // (摘要在合成头之后,具体位置随 memory/skill 布局漂移 —— 位置
    // 独立性正是 P1-1 的教训;本测试 env 无 memory/skill,实际落在 0)。
    let main = &sends[1];
    let summary_idx = main
        .iter()
        .position(|m| {
            let t = m.content.to_text();
            t.contains(crate::agent::compaction::SUMMARY_CONTEXT_PREFIX)
                && t.contains("SUMMARY_BODY_MARKER")
        })
        .expect("主 turn 请求含 [前缀+摘要] 回填消息");
    assert!(
        summary_idx <= 3,
        "回填消息在合成头之后位置 ≤ 3(got {})",
        summary_idx
    );
    assert!(
        main.len() <= original_len - 8,
        "长度骤减(≥ 8 条被折叠进摘要):主 turn {} vs 原史 {}",
        main.len(),
        original_len
    );
    // 尾部当前输入逐字保留。
    assert_eq!(
        main.last().unwrap().content.to_text(),
        "current question",
        "当前输入逐字保留"
    );

    // RULE-A-001 配对不变量扫描(压缩后消息全量)。
    for i in 0..main.len() {
        let m = &main[i];
        if let MessageContent::Blocks(blocks) = &m.content {
            if blocks
                .iter()
                .any(|b| matches!(b, crate::llm::types::ContentBlock::ToolUse { .. }))
            {
                let next = main.get(i + 1);
                assert!(
                    matches!(next, Some(n) if n.role == Role::User && matches!(&n.content,
                        MessageContent::Blocks(bs) if bs.iter().any(|b| matches!(b,
                            crate::llm::types::ContentBlock::ToolResult { .. })))),
                    "压缩后 tool_use 必须紧跟 tool_result(idx={})",
                    i
                );
            }
        }
    }

    // DB:摘要行落库(kind + 纯摘要无前缀 + prior_summary_seq null)。
    let loaded = crate::db::load_session(&h.db, &h.session_id)
        .await
        .unwrap()
        .expect("session loaded");
    let summary_rows: Vec<_> = loaded
        .messages
        .iter()
        .filter(|r| {
            crate::agent::compaction::message_metadata_kind(r.metadata.as_ref())
                == Some(COMPACTION_SUMMARY_KIND)
        })
        .collect();
    assert_eq!(summary_rows.len(), 1, "恰好一行摘要");
    let row = summary_rows[0];
    assert_eq!(
        row.text, "SUMMARY_BODY_MARKER",
        "text 列 = 纯摘要(无前缀话术)"
    );
    assert_eq!(row.role, "user");
    let meta = row.metadata.as_ref().unwrap();
    assert_eq!(meta["trigger"], "auto");
    assert_eq!(meta["prior_summary_seq"], serde_json::Value::Null);
    assert_eq!(meta["summary_usage"]["output_tokens"], 500);
    // PR2.5 metadata 契约(design §2.1 修订):cutoff_seq = 待压区末行
    // 真实 seq(精确,非"摘要行 seq-1"近似 —— 那是当前输入行 seq,
    // 会让下一请求折叠吞掉保留区)。预落 30 行(24 轮循环,工具配对占
    // 2 行,seq 0..=29)+ 当前输入行 seq 30:精确 cutoff 必然 < 30,
    // 且 preserve_from_seq 严格 = cutoff + 1。行为级精确边界断言
    // (被压区行不在、保留区行在)由下方 P1 回归测试
    // `preserved_region_and_question_survive_...` 承担,这里钉
    // metadata 契约。
    let cutoff = meta["cutoff_seq"]
        .as_i64()
        .expect("cutoff_seq is an integer");
    assert!(
        (0..30).contains(&cutoff),
        "cutoff 落在预落历史行区间(旧 bug 写的是当前输入行 seq 30;got {})",
        cutoff
    );
    assert_eq!(meta["preserve_from_seq"], serde_json::json!(cutoff + 1));

    // trace 事件:method = summary。
    let events = emitter.chat_events();
    let compacted_events: Vec<&ChatEvent> = events
        .iter()
        .map(|p| &p.event)
        .filter(|e| matches!(e, ChatEvent::ContextCompacted { .. }))
        .collect();
    assert_eq!(compacted_events.len(), 1);
    match compacted_events[0] {
        ChatEvent::ContextCompacted { method, .. } => {
            assert_eq!(method, "summary");
        }
        _ => unreachable!(),
    }

    compaction_registry().clear(&h.session_id).await;
}

// ---------------------------------------------------------------------------
// 2) AC2(修订 2026-08-18):同 session 第二次请求零摘要调用(水位
//    替换生效),且 context = [摘要] + [seq > cutoff 的常规行] ——
//    断言语义按 cutoff 折叠改写(旧 `len() < 8` 固化的恰是"丢保留区"
//    的错误行为)。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn second_request_pays_no_summary_watermark_replacement() {
    let h = make_harness().await;
    enable_compaction(&h).await;

    // 预落历史(模拟前一个 session 的既有行)。
    let history = overline_history();
    let (seed_rows, current_input) = split_history(&history);
    seed_history(&h, &seed_rows).await;
    let mut first_wire = seed_rows.clone();
    first_wire.push(current_input);

    // 请求 1:压缩发生。
    let emitter1 = Arc::new(MockEmitter::new());
    let mock1 = Arc::new(MockProvider::new(vec![
        summary_response("WATERMARK_SUMMARY"),
        end_turn_response("done one"),
    ]));
    run_loop(
        &h,
        mock1.clone(),
        "rid-ac2-1",
        first_wire,
        emitter1,
        false,
        h.session_id.clone(),
    )
    .await;
    assert_eq!(mock1.call_count(), 2);
    assert_eq!(
        mock1
            .sent_messages()
            .iter()
            .filter(|m| is_summary_send(m))
            .count(),
        1,
        "第一请求付了一次摘要"
    );

    // 请求 2:wire = DB 全行(rehydrate 回灌:text 列 → Text)+ 新输入。
    let loaded = crate::db::load_session(&h.db, &h.session_id)
        .await
        .unwrap()
        .unwrap();
    let cutoff = loaded
        .messages
        .iter()
        .rev()
        .find(|r| {
            crate::agent::compaction::message_metadata_kind(r.metadata.as_ref())
                == Some(COMPACTION_SUMMARY_KIND)
        })
        .and_then(|r| r.metadata.as_ref())
        .and_then(|m| m.get("cutoff_seq"))
        .and_then(|v| v.as_i64())
        .expect("watermark row carries cutoff_seq");
    let mut second_wire = reload_wire(&loaded);
    second_wire.push(user("follow-up question"));

    let mock2 = Arc::new(MockProvider::new(vec![end_turn_response("done two")]));
    run_loop(
        &h,
        mock2.clone(),
        "rid-ac2-2",
        second_wire,
        Arc::new(MockEmitter::new()),
        false,
        h.session_id.clone(),
    )
    .await;

    // AC2:第二请求零摘要调用(唯一 send 是主 turn)。
    assert_eq!(mock2.call_count(), 1, "零摘要调用,只有主 turn");
    let sends = mock2.sent_messages();
    assert!(sends.iter().all(|m| !is_summary_send(m)));
    // 水位替换生效:主 turn 请求含折叠摘要消息(内容取 DB 行;位置
    // 无关断言 —— memory 合成头在它之前,测试 env P=0 实际在 0)。
    let main = &sends[0];
    let summary_positions: Vec<usize> = main
        .iter()
        .enumerate()
        .filter(|(_, m)| m.content.to_text().contains("WATERMARK_SUMMARY"))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        summary_positions.len(),
        1,
        "折叠摘要恰好一条(got {:?})",
        summary_positions
    );
    let folded_idx = summary_positions[0];
    assert!(
        folded_idx <= 3,
        "折叠摘要位于合成头之后(got {}): {:?}",
        folded_idx,
        main[folded_idx]
            .content
            .to_text()
            .chars()
            .take(60)
            .collect::<String>()
    );
    // AC2 修订语义:context = [合成头 0..3] + [摘要] + [DB 中 seq >
    // cutoff 的非摘要常规行] + [新输入]。保留区跨请求存活 → 不再是
    // 旧断言的 `len() < 8`(那固化了"按摘要行位置折叠、保留区被吞"
    // 的错误行为);被压区被折叠 → 远小于全量 wire。合成头长度随
    // 测试机环境的 memory/skill 层漂移(0..3),用区间断言。
    let expected_kept = loaded
        .messages
        .iter()
        .filter(|r| {
            r.seq > cutoff
                && crate::agent::compaction::message_metadata_kind(r.metadata.as_ref())
                    != Some(COMPACTION_SUMMARY_KIND)
        })
        .count();
    assert!(
        expected_kept > 0,
        "保留区非空(cutoff={} 行数={})",
        cutoff,
        loaded.messages.len()
    );
    let overhead = main.len() as i64 - expected_kept as i64 - 2;
    assert!(
        (0..=3).contains(&overhead),
        "context = [合成头≤3] + [摘要] + {expected_kept} 条存活行 + 新输入 \
         (main={} overhead={})",
        main.len(),
        overhead
    );
    assert!(
        main.len() < loaded.messages.len(),
        "被压区折叠:context({})远小于全量 wire({})",
        main.len(),
        loaded.messages.len()
    );
    assert_eq!(main.last().unwrap().content.to_text(), "follow-up question");
    // 请求 1 的提问与回答都在存活区(seq > cutoff)。
    let texts: Vec<String> = main.iter().map(|m| m.content.to_text()).collect();
    assert!(
        texts.iter().any(|t| t == "current question"),
        "请求 1 提问存活"
    );
    assert!(texts.iter().any(|t| t == "done one"), "请求 1 回答存活");

    compaction_registry().clear(&h.session_id).await;
}

// ---------------------------------------------------------------------------
// 2.5) P1 回归(核心,PR2.5 修订的行为锚点):请求 N 触发摘要后,
//      请求 N+1 的 context = [摘要(头部)] + [seq > cutoff 的常规行]。
//      断言:保留区消息、请求 N 的用户提问行(seq 30)、请求 N 的
//      assistant 回答逐字在场;被压区行(seq <= cutoff)出局;且
//      metadata.cutoff_seq 是被压区末行的真实 seq(精确边界,非近似
//      —— 旧实现写的"摘要行 seq-1" = 当前输入行 seq 30,折叠点会
//      吞掉整个保留区与本请求提问;末行可能是空 text 的工具配对,
//      文本成员断言无法直接观察,故由"边界行在 DB + 缺席最大非空
//      seq ≤ cutoff"钉住,见测试尾部)。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn preserved_region_and_question_survive_across_requests() {
    let h = make_harness().await;
    enable_compaction(&h).await;

    // 请求 N:预落 30 行(24 轮循环,工具配对占 2 行,seq 0..=29)
    // + 提问(init 落在 seq 30)。
    let history = distinct_overline_history();
    let (seed_rows, tail) = split_history(&history);
    seed_history(&h, &seed_rows).await;
    let mut first_wire = seed_rows.clone();
    first_wire.push(tail);

    let mock1 = Arc::new(MockProvider::new(vec![
        summary_response("P1_SUMMARY_BODY"),
        end_turn_response("request N answer"),
    ]));
    run_loop(
        &h,
        mock1.clone(),
        "rid-p1-1",
        first_wire,
        Arc::new(MockEmitter::new()),
        false,
        h.session_id.clone(),
    )
    .await;
    assert_eq!(mock1.call_count(), 2, "请求 N:1 摘要 + 1 主 turn");

    // DB:摘要行落库,携带精确 cutoff_seq + preserve_from_seq。
    let loaded = crate::db::load_session(&h.db, &h.session_id)
        .await
        .unwrap()
        .unwrap();
    let summary_row = loaded
        .messages
        .iter()
        .rev()
        .find(|r| {
            crate::agent::compaction::message_metadata_kind(r.metadata.as_ref())
                == Some(COMPACTION_SUMMARY_KIND)
        })
        .expect("摘要行已落库");
    let meta = summary_row.metadata.as_ref().unwrap();
    let cutoff = meta["cutoff_seq"].as_i64().expect("cutoff_seq 为整数");
    assert!(
        (0..24).contains(&cutoff),
        "cutoff 是被压区末行(预落历史 seq 0..=23)的真实 seq;旧 bug 写的恰是 \
         当前输入行 seq 24(got {})",
        cutoff
    );
    assert_eq!(meta["preserve_from_seq"], serde_json::json!(cutoff + 1));

    // 请求 N+1:reload wire(DB 全行,含摘要行)+ follow-up。
    let mut second_wire = reload_wire(&loaded);
    second_wire.push(user("follow-up question"));
    let mock2 = Arc::new(MockProvider::new(vec![end_turn_response(
        "request N+1 answer",
    )]));
    run_loop(
        &h,
        mock2.clone(),
        "rid-p1-2",
        second_wire,
        Arc::new(MockEmitter::new()),
        false,
        h.session_id.clone(),
    )
    .await;
    assert_eq!(mock2.call_count(), 1, "AC2:第二请求零摘要调用");
    let main = &mock2.sent_messages()[0];
    let texts: Vec<String> = main.iter().map(|m| m.content.to_text()).collect();

    // 头部摘要恰好一条(位置无关断言;测试 env 合成头 P=0 → 实际 0)。
    let summary_positions: Vec<usize> = main
        .iter()
        .enumerate()
        .filter(|(_, m)| m.content.to_text() == "P1_SUMMARY_BODY")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(summary_positions.len(), 1, "摘要恰好一条(kind 过滤防重复)");
    assert!(summary_positions[0] <= 3, "摘要位于合成头之后");

    // P1 核心:请求 N 的用户提问行 + assistant 回答 + 新输入在场。
    assert!(
        texts.iter().any(|t| t == "current question"),
        "请求 N 的用户提问行必须跨请求存活(P1 缺陷正是它被丢)"
    );
    assert!(
        texts.iter().any(|t| t == "request N answer"),
        "请求 N 的 assistant 回答必须存活"
    );
    assert_eq!(main.last().unwrap().content.to_text(), "follow-up question");

    // 精确边界(逐行):seq <= cutoff 的历史行出局,seq > cutoff 的
    // 非摘要行逐字在场;cutoff 恰为出局行的最大 seq。空 text 行
    // (tool_use/tool_result)跳过 —— to_text 只聚合 Text 块。
    let mut max_absent: Option<i64> = None;
    for r in &loaded.messages {
        if crate::agent::compaction::message_metadata_kind(r.metadata.as_ref())
            == Some(COMPACTION_SUMMARY_KIND)
        {
            continue;
        }
        if r.text.is_empty() {
            continue;
        }
        if r.seq <= cutoff {
            assert!(
                !texts.iter().any(|t| t == &r.text),
                "被压区行 seq={} 不得出席: {:?}",
                r.seq,
                &r.text[..r.text.len().min(40)]
            );
            max_absent = Some(max_absent.map_or(r.seq, |m: i64| m.max(r.seq)));
        } else {
            assert!(
                texts.iter().any(|t| t == &r.text),
                "保留区行 seq={} 必须逐字在场: {:?}",
                r.seq,
                &r.text[..r.text.len().min(40)]
            );
        }
    }
    // cutoff 精确性钉住(修订 2026-08-18):cutoff 必须是被压区**末行**
    // 的真实 seq。文本成员断言只看得到非空 text 行 —— 被压区末组
    // 可能是工具配对(text 列皆空),此时"缺席非空行最大 seq"(12)天然
    // 小于 cutoff(14),不能要求二者相等。正确不变量:①边界行(seq ==
    // cutoff)确在 DB 且非摘要行;②缺席非空行最大 seq ≤ cutoff(折叠
    // 没有越过边界);③seq > cutoff 的非空行全部在场(上方循环已断言)。
    assert!(
        loaded.messages.iter().any(|r| {
            r.seq == cutoff
                && crate::agent::compaction::message_metadata_kind(r.metadata.as_ref())
                    != Some(COMPACTION_SUMMARY_KIND)
        }),
        "cutoff 边界行(seq {cutoff})必须在 DB 存在且非摘要行"
    );
    assert!(
        max_absent.is_some_and(|m| m <= cutoff),
        "缺席非空文本行最大 seq 不得超过 cutoff:max_absent={max_absent:?} cutoff={cutoff}"
    );
    let folded = loaded.messages.iter().filter(|r| r.seq <= cutoff).count();
    assert!(folded >= 8, "被压区非平凡(≥ 8 行被折叠;got {})", folded);

    compaction_registry().clear(&h.session_id).await;
}

// ---------------------------------------------------------------------------
// 3) AC3(同 loop 半边):同一 loop run 内二次压缩 —— 第二次摘要的
//    prior-summary 来自循环内 SummaryAnchor(增量合并)。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn same_loop_second_compaction_uses_inloop_anchor() {
    let h = make_harness().await;
    enable_compaction(&h).await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // turn 1 摘要。
        summary_response("FIRST_SUMMARY_BODY"),
        // turn 1 主响应:大文本 + tool_use(触发 turn 2)。
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: pad(16_000) }),
            Ok(ChatEvent::ToolCall {
                id: "tu_same_loop".into(),
                name: "list_dir".into(),
                input: serde_json::json!({"path": "."}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // turn 2 摘要(增量:prior 来自循环内 anchor)。
        summary_response("SECOND_SUMMARY_BODY"),
        // turn 2 主响应:终态。
        end_turn_response("all done"),
    ]));
    let history = overline_history();
    // 预落历史(cutoff 精确计算依赖 wire ↔ DB 行序对齐;同 loop 二次
    // 压缩走 prior=Some 的过滤后缀路径,同样要求对齐)。
    let (seed_rows, tail) = split_history(&history);
    seed_history(&h, &seed_rows).await;
    let mut first_wire = seed_rows.clone();
    first_wire.push(tail);

    run_loop(
        &h,
        mock.clone(),
        "rid-incr",
        first_wire,
        emitter.clone(),
        false,
        h.session_id.clone(),
    )
    .await;

    let sends = mock.sent_messages();
    if std::env::var("P1_DBG").is_ok() {
        for (i, s) in sends.iter().enumerate() {
            eprintln!(
                "DBG send[{}] len={} first={:?} last={:?}",
                i,
                s.len(),
                s.first()
                    .map(|m| m.content.to_text())
                    .unwrap_or_default()
                    .chars()
                    .take(50)
                    .collect::<String>(),
                s.last()
                    .map(|m| m.content.to_text())
                    .unwrap_or_default()
                    .chars()
                    .take(50)
                    .collect::<String>(),
            );
        }
    }
    assert_eq!(mock.call_count(), 4, "2 摘要 + 2 主 turn");
    let summary_sends: Vec<&Vec<ChatMessage>> =
        sends.iter().filter(|m| is_summary_send(m)).collect();
    assert_eq!(summary_sends.len(), 2, "同 loop 内两次压缩");

    // 第二次摘要的 prompt:含 <prior-summary> 且内容是**第一份摘要的
    // 纯正文**(循环内 anchor,非位置猜测);正文只出现一次(anchor
    // 消息不进 transcript,不重复喂)。
    let second_prompt = summary_sends[1][0].content.to_text();
    assert!(
        second_prompt.contains("<prior-summary>\nFIRST_SUMMARY_BODY\n</prior-summary>"),
        "prior 块注入第一份摘要纯正文"
    );
    assert_eq!(
        second_prompt.matches("FIRST_SUMMARY_BODY").count(),
        1,
        "anchor 不进 transcript(不重复喂)"
    );
    // 第一次摘要的 prompt 没有 prior 块。
    assert!(!summary_sends[0][0]
        .content
        .to_text()
        .contains("<prior-summary>"));

    // DB:两行摘要,第二行 prior_summary_seq 指向第一行。
    let loaded = crate::db::load_session(&h.db, &h.session_id)
        .await
        .unwrap()
        .unwrap();
    let summary_rows: Vec<_> = loaded
        .messages
        .iter()
        .filter(|r| {
            crate::agent::compaction::message_metadata_kind(r.metadata.as_ref())
                == Some(COMPACTION_SUMMARY_KIND)
        })
        .collect();
    assert_eq!(summary_rows.len(), 2);
    let (first, second) = (summary_rows[0], summary_rows[1]);
    assert_eq!(first.text, "FIRST_SUMMARY_BODY");
    assert_eq!(second.text, "SECOND_SUMMARY_BODY");
    assert_eq!(
        second.metadata.as_ref().unwrap()["prior_summary_seq"],
        serde_json::json!(first.seq),
        "第二行 prior_summary_seq 指向第一行"
    );

    compaction_registry().clear(&h.session_id).await;
}

// ---------------------------------------------------------------------------
// 4) 摘要失败注入 → fallback 机械丢组,turn 正常 Done;失败计数 +1
//    (AC4 前半;method=mechanical)。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn summary_failure_falls_back_to_mechanical() {
    let h = make_harness().await;
    enable_compaction(&h).await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        summary_auth_failure(),
        end_turn_response("mechanical path answer"),
    ]));
    let history = overline_history();
    let original_len = history.len();
    // 预落历史(生产 wire 镜像;摘要尝试须先通过 cutoff 对齐才会
    // 发起 LLM 调用,auth 失败注入才有意义)。
    let (seed_rows, tail) = split_history(&history);
    seed_history(&h, &seed_rows).await;
    let mut first_wire = seed_rows.clone();
    first_wire.push(tail);

    run_loop(
        &h,
        mock.clone(),
        "rid-fallback",
        first_wire,
        emitter.clone(),
        false,
        h.session_id.clone(),
    )
    .await;

    assert_eq!(mock.call_count(), 2, "1 失败摘要 + 1 主 turn");
    // turn 正常完成。
    let dones: Vec<_> = emitter
        .chat_events()
        .into_iter()
        .filter_map(|p| match p.event {
            ChatEvent::Done { stop_reason, .. } => stop_reason,
            _ => None,
        })
        .collect();
    assert_eq!(dones, vec!["end_turn".to_string()]);

    // 主 turn 走机械丢组:消息骤减(机械保头 2 + 尾)。
    let sends = mock.sent_messages();
    let main = &sends[1];
    assert!(!is_summary_send(main));
    assert!(
        main.len() < original_len,
        "机械丢组生效:主 turn {} vs 原史 {}",
        main.len(),
        original_len
    );
    assert_eq!(main.last().unwrap().content.to_text(), "current question");

    // trace method = mechanical。
    let methods: Vec<String> = emitter
        .chat_events()
        .into_iter()
        .filter_map(|p| match p.event {
            ChatEvent::ContextCompacted { method, .. } => Some(method),
            _ => None,
        })
        .collect();
    assert_eq!(methods, vec!["mechanical".to_string()]);

    // 失败计数 +1(未熔断)。
    assert_eq!(compaction_registry().failures(&h.session_id).await, 1);
    assert!(!compaction_registry().is_tripped(&h.session_id).await);

    compaction_registry().clear(&h.session_id).await;
}

// ---------------------------------------------------------------------------
// 5) 连续 3 次失败熔断:第 4 次请求跳过摘要直达机械(AC4 后半)。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn three_consecutive_failures_trip_breaker() {
    let h = make_harness().await;
    enable_compaction(&h).await;

    // 预落一次历史;后续请求用 reload wire(生产同款:wire 镜像 DB
    // 行序 —— cutoff 对齐前提,且失败路径不落摘要行,DB 不再增长
    // 摘要行,重放同款 wire 会破坏对齐)。
    let history = overline_history();
    let (seed_rows, tail) = split_history(&history);
    seed_history(&h, &seed_rows).await;
    let mut first_wire = seed_rows.clone();
    first_wire.push(tail);

    // 3 次请求,每次摘要失败 → 机械兜底 → 正常完成。
    let mut wire = first_wire;
    for i in 0..3 {
        let mock = Arc::new(MockProvider::new(vec![
            summary_auth_failure(),
            end_turn_response("mech"),
        ]));
        run_loop(
            &h,
            mock.clone(),
            &format!("rid-breaker-{i}"),
            wire,
            Arc::new(MockEmitter::new()),
            false,
            h.session_id.clone(),
        )
        .await;
        assert_eq!(mock.call_count(), 2, "请求 {i}:失败摘要 + 主 turn");
        assert_eq!(
            compaction_registry().failures(&h.session_id).await,
            i as u8 + 1
        );
        // 下一次请求的 wire = 当前 DB 全行 + 新输入(reload)。
        let loaded = crate::db::load_session(&h.db, &h.session_id)
            .await
            .unwrap()
            .unwrap();
        wire = reload_wire(&loaded);
        wire.push(user("current question"));
    }
    assert!(compaction_registry().is_tripped(&h.session_id).await);

    // 第 4 次请求:零摘要调用(脚本只有主 turn —— 若仍尝试摘要,
    // 摘要会吃掉主 turn 脚本,主 turn 将报 script exhausted)。
    let mock4 = Arc::new(MockProvider::new(vec![end_turn_response("breaker open")]));
    run_loop(
        &h,
        mock4.clone(),
        "rid-breaker-3",
        wire,
        Arc::new(MockEmitter::new()),
        false,
        h.session_id.clone(),
    )
    .await;
    assert_eq!(mock4.call_count(), 1, "熔断后只有主 turn send");
    assert!(
        mock4.sent_messages().iter().all(|m| !is_summary_send(m)),
        "无摘要调用"
    );

    compaction_registry().clear(&h.session_id).await;
}

// ---------------------------------------------------------------------------
// 6) gate:worker(skip_persist)不触发摘要路径(AC8)。机械丢组照常。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn worker_session_skips_summary_path() {
    let h = make_harness().await;
    enable_compaction(&h).await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![end_turn_response("worker answer")]));
    let history = overline_history();
    let original_len = history.len();

    run_loop(
        &h,
        mock.clone(),
        "rid-worker",
        history,
        emitter.clone(),
        true, // is_worker → skip_persist + effective_is_worker
        h.session_id.clone(),
    )
    .await;

    assert_eq!(mock.call_count(), 1, "worker 零摘要调用");
    let sends = mock.sent_messages();
    assert!(sends.iter().all(|m| !is_summary_send(m)));
    // 机械丢组照常(C3 对 worker 不豁免)。
    let main = &sends[0];
    assert!(
        main.len() < original_len,
        "机械丢组仍生效:主 turn {} vs 原史 {}",
        main.len(),
        original_len
    );
    // 无摘要行落库。
    let loaded = crate::db::load_session(&h.db, &h.session_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        loaded.messages.iter().all(|r| {
            crate::agent::compaction::message_metadata_kind(r.metadata.as_ref())
                != Some(COMPACTION_SUMMARY_KIND)
        }),
        "worker 不落摘要行"
    );

    compaction_registry().clear(&h.session_id).await;
}

// ---------------------------------------------------------------------------
// 7) gate:群聊(session_type=GroupChat 的 session 行)不触发摘要路径
//    (AC8;gate 判 session_type,非 speaker 参数 —— 评审 P3)。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn group_chat_session_skips_summary_path() {
    let h = make_harness().await;
    enable_compaction(&h).await;
    // 建 GroupChat 类型 session 行(gate 判定对象)。
    let gc_sid = uuid::Uuid::new_v4().to_string();
    crate::db::create_session(
        &h.db,
        &gc_sid,
        &h.project_id,
        h.project_path.to_str().unwrap(),
        "mock-model",
        None,
        Some("group_chat"),
        None,
    )
    .await
    .unwrap();

    let mock = Arc::new(MockProvider::new(vec![end_turn_response("gc answer")]));
    let history = overline_history();
    let original_len = history.len();

    run_loop(
        &h,
        mock.clone(),
        "rid-gc",
        history,
        Arc::new(MockEmitter::new()),
        false,
        gc_sid.clone(),
    )
    .await;

    assert_eq!(mock.call_count(), 1, "群聊零摘要调用");
    let sends = mock.sent_messages();
    assert!(sends.iter().all(|m| !is_summary_send(m)));
    // 机械丢组照常。
    let main = &sends[0];
    assert!(
        main.len() < original_len,
        "机械丢组仍生效:主 turn {} vs 原史 {}",
        main.len(),
        original_len
    );
    // 无摘要行落库。
    let loaded = crate::db::load_session(&h.db, &gc_sid)
        .await
        .unwrap()
        .unwrap();
    assert!(
        loaded.messages.iter().all(|r| {
            crate::agent::compaction::message_metadata_kind(r.metadata.as_ref())
                != Some(COMPACTION_SUMMARY_KIND)
        }),
        "群聊不落摘要行"
    );

    compaction_registry().clear(&gc_sid).await;
}
