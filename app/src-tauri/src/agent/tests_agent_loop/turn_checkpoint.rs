#![cfg(test)]

//! RULE-PERSIST-001 (08-24-p1-turn-crash-recovery) WP2 集成锁:
//! drive.rs 检查点写点①②③ 的端到端行为 —— 正常多 turn 无 in_progress
//! 残留、cancel mid-stream 占位被终态覆盖、AC4 孤儿尾行恢复后下一
//! 请求上下文含合成 tool_result(pair atomicity,不 400)。
//!
//! 时间门本身(CHECKPOINT_INTERVAL=1s)不在此处等真时长 —— 门的纯
//! 逻辑单测在 `chat_loop/drive.rs::checkpoint_tests`;本文件依赖的
//! 覆盖路径是**占位行**(写点①,stream ready 即写,无时间门)被
//! 收尾落库(写点③)覆盖,与周期检查点是同一条 upsert → finalize
//! SQL 链路。

use std::sync::atomic::Ordering;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::tests_common::{
    chat_loop_deps, chat_loop_request, make_harness, parent_role, test_messages, MockEmitter,
    TestHarness,
};
use crate::agent::chat_loop::run_chat_loop;
use crate::db;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, ChatMessage, ContentBlock, MessageContent, Role};

/// File-local wrapper: production-style `run_chat_loop` invocation
/// (routes through the `tests_common` fixtures per RULE-ARGS-001;
/// one helper per file keeps each cluster file self-contained).
async fn run_loop(
    h: &TestHarness,
    mock: Arc<MockProvider>,
    emitter: Arc<MockEmitter>,
    messages: Vec<ChatMessage>,
    token: CancellationToken,
) {
    // production-style caller — skip_persist = false: turn
    // checkpoints are ACTIVE (persisted like production).
    run_chat_loop(
        chat_loop_request(
            vec![],
            mock,
            200_000,
            format!("rid-{}", uuid::Uuid::new_v4()),
            h.session_id.clone(),
            messages,
            emitter,
        ),
        {
            let mut deps = chat_loop_deps(&h);
            deps.token = token;
            deps
        },
        parent_role(&h),
    )
    .await;
}

/// 前端 rehydrate 同款:DB 行 → wire ChatMessage,**保留 content JSON
/// 的块结构**(manual_compaction 的 `reload_wire` 走 text 列是有损
/// 快写;本测试需要 tool_result 块原样回灌 —— 生产前端 rehydrate
/// 正是把 tool_result 块带回 wire 的,llm-contract §Tool-Result
/// invariants 第 4 条)。
fn rehydrate_wire(rows: &[db::MessageRow]) -> Vec<ChatMessage> {
    rows.iter()
        .map(|r| ChatMessage {
            role: if r.role == "assistant" {
                Role::Assistant
            } else {
                Role::User
            },
            content: serde_json::from_value(r.content.clone()).expect("content JSON rehydrate"),
            speaker: None,
            attachments: None,
        })
        .collect()
}

async fn count_in_progress(db: &sqlx::SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE status IS NOT NULL")
        .fetch_one(db)
        .await
        .unwrap()
}

async fn assistant_rows(db: &sqlx::SqlitePool, session_id: &str) -> Vec<db::MessageRow> {
    db::load_session(db, session_id)
        .await
        .expect("load_session")
        .expect("session exists")
        .messages
        .into_iter()
        .filter(|m| m.role == "assistant")
        .collect()
}

// ---------------------------------------------------------------------------
// AC1: 正常多 turn 对话 → 无 in_progress 残留,内容逐字节照旧
// ---------------------------------------------------------------------------

/// 两轮正常 turn(工具轮 + 纯文本收尾):检查点占位被终态覆盖,全表
/// status 无非 NULL 残留;助手行内容与无检查点时代的断言口径一致。
/// 时间门 1s 内流式即完 → 周期检查点(写点②)不触发,占位(写点①)
/// → finalize(写点③)覆盖链路被完整走到。首轮必须是 tool_use 轮
/// 才会续进第二 turn(纯 end_turn 首轮即出 loop —— basic.rs cancel
/// 测试同款脚本结构)。
#[tokio::test]
async fn agent_loop_normal_turns_leave_no_in_progress_residue() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: text + tool_use → tool executes → loop continues.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "first answer".into(),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_ck1".into(),
                name: "list_dir".into(),
                input: serde_json::json!({"path": "."}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: None,
            }),
        ]),
        // Turn 2: text answer ends the loop.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "final answer".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: None,
            }),
        ]),
    ]));

    run_loop(
        &h,
        mock.clone(),
        emitter,
        test_messages(),
        CancellationToken::new(),
    )
    .await;

    assert_eq!(mock.call_count(), 2);
    // 核心断言:零 in_progress / interrupted 残留。
    assert_eq!(
        count_in_progress(&h.db).await,
        0,
        "no status rows may survive a clean multi-turn run"
    );
    let assistants = assistant_rows(&h.db, &h.session_id).await;
    assert_eq!(assistants.len(), 2, "two assistant turns persisted");
    let texts: Vec<&str> = assistants.iter().map(|m| m.text.as_str()).collect();
    assert_eq!(texts, vec!["first answer", "final answer"]);
    for m in &assistants {
        assert!(m.status.is_none(), "terminal rows carry status NULL");
    }
    // 工具轮配对完整(Step B 无事可做也是本测试的隐含覆盖)。
    let loaded = db::load_session(&h.db, &h.session_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        loaded.messages.iter().any(|m| m.has_tool_results),
        "tool_result row persisted (pair intact)"
    );
}

// ---------------------------------------------------------------------------
// Cancel mid-stream: 占位行被终态覆盖(写点① → 写点③),marker 仍在
// ---------------------------------------------------------------------------

/// 模拟崩溃面的对照路径:cancel mid-stream(HangingThenCancel,流
/// 永不产出)。占位行已在 stream ready 时写入;cancel 后收尾 persist
/// 覆盖同一 seq(status → NULL)且 CANCELLED_MARKER 可见。这锁定
/// "检查点不污染 cancel 语义"(AC1 对 cancel 路径的延伸)。首轮是
/// tool_use 轮(读工具 Tier 5 直放),loop 续进悬挂的第二轮;cancel
/// 侧信道在 call_count >= 2(第二轮 send 已观察到 = 占位已写)后触发
/// —— basic.rs `agent_loop_cancel_in_turn_2_kills_loop` 同款骨架。
#[tokio::test]
async fn agent_loop_cancel_midstream_placeholder_overwritten_by_final_persist() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: text + tool_use. Tool executes (read tool, Tier 5
        // default-allow), tool_result persists, loop re-enters turn 2.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "turn one".into(),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_ck_cancel".into(),
                name: "list_dir".into(),
                input: serde_json::json!({"path": "."}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: None,
            }),
        ]),
        // Turn 2: hangs forever; cancel side-channel fires once the
        // send is observed (placeholder is written by then).
        MockResponse::HangingThenCancel,
    ]));
    let call_handle = mock.call_count_handle();
    let cancel_token = CancellationToken::new();
    let cancel_for_task = cancel_token.clone();
    let cancel_handle = tokio::spawn(async move {
        loop {
            if call_handle.load(Ordering::SeqCst) >= 2 {
                cancel_for_task.cancel();
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    });

    run_loop(
        &h,
        mock.clone(),
        emitter.clone(),
        test_messages(),
        cancel_token,
    )
    .await;
    cancel_handle.await.unwrap();

    assert_eq!(mock.call_count(), 2);
    assert_eq!(emitter.cancel_done_count(), 1);
    assert_eq!(
        count_in_progress(&h.db).await,
        0,
        "cancelled turn's placeholder must be finalized (status NULL)"
    );
    // Turn 2 的 assistant 行存在 = 占位被终态覆盖(而非残留 / 被删)。
    let assistants = assistant_rows(&h.db, &h.session_id).await;
    assert_eq!(assistants.len(), 2, "turn 1 + cancelled turn 2 rows");
    assert!(assistants[0].text.contains("turn one"));
    assert!(
        assistants[1]
            .text
            .contains(crate::agent::helpers::CANCELLED_MARKER),
        "cancel marker survives the checkpoint overwrite, got: {}",
        assistants[1].text
    );
    for m in &assistants {
        assert!(m.status.is_none());
    }
}

// ---------------------------------------------------------------------------
// AC4: 孤儿尾行(W2 残留)→ recover → 下一请求上下文含合成 tool_result
// ---------------------------------------------------------------------------

/// 手工构造 W2 崩溃残留(user + assistant(tool_use) 无 tool_result),
/// 跑启动恢复 pass,然后发第二个请求:断言 **provider 实际收到的
/// messages** 含配对的 is_error tool_result —— 恢复后的 session 不再
/// 因孤儿 tool_use 400(pair atomicity,AC4 的全环断言)。
#[tokio::test]
async fn agent_loop_orphan_tail_recovered_and_paired_in_next_request_context() {
    let h = make_harness().await;

    // 构造崩溃残留:eq 视角 = 崩溃前一个正常请求已落 user(seq 0)+
    // assistant(tool_use)(seq 1),tool_result 因 daemon kill -9 丢失。
    let orphan_tool_use = MessageContent::Blocks(vec![ContentBlock::ToolUse {
        id: "toolu_orphan_ac4".to_string(),
        name: "read_file".to_string(),
        input: serde_json::json!({"path": "/tmp/hostname"}),
    }]);
    db::persist_turn(
        &h.db,
        &h.session_id,
        Role::User,
        &MessageContent::Text("read the hostname".into()),
        0,
        None,
        None,
    )
    .await
    .unwrap();
    db::persist_turn(
        &h.db,
        &h.session_id,
        Role::Assistant,
        &orphan_tool_use,
        1,
        None,
        None,
    )
    .await
    .unwrap();

    // 启动恢复 pass(生产挂在 state.rs,此处直调)。
    let report = db::recover_interrupted_messages(&h.db).await.unwrap();
    assert_eq!(report.orphan_repaired, 1);
    assert_eq!(report.total(), 1);

    // 第二个请求:daemon 重启后前端 rehydrate 恢复后的 DB 行(load_
    // session → wire,tool_result 块原样回灌)+ 新的用户输入 ——
    // 生产链路的下一请求正是这个形态。
    let rehydrated = rehydrate_wire(
        &db::load_session(&h.db, &h.session_id)
            .await
            .unwrap()
            .unwrap()
            .messages,
    );
    let mut wire = rehydrated;
    wire.extend(test_messages());
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta {
            text: "after recovery".into(),
        }),
        Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: None,
        }),
    ])]));
    run_loop(&h, mock.clone(), emitter, wire, CancellationToken::new()).await;
    assert_eq!(mock.call_count(), 1);

    // 核心断言:provider 看到的 messages 里,孤儿 tool_use 有了配对
    // 的 is_error tool_result(user-role)。sent_messages 是 per-turn
    // 快照(Vec<Vec<ChatMessage>>),按 turn → msg → block 三层走。
    let sent = mock.sent_messages();
    assert!(!sent.is_empty());
    let mut paired = false;
    for turn in &sent {
        for msg in turn {
            if let MessageContent::Blocks(blocks) = &msg.content {
                for b in blocks {
                    if let ContentBlock::ToolResult {
                        tool_use_id,
                        is_error,
                        ..
                    } = b
                    {
                        if tool_use_id == "toolu_orphan_ac4" {
                            assert!(*is_error, "synthetic recovery result is is_error");
                            assert_eq!(msg.role, Role::User);
                            paired = true;
                        }
                    }
                }
            }
        }
    }
    assert!(
        paired,
        "recovered synthetic tool_result must ride the next request context"
    );
    // 第二请求本身也干净收尾。
    assert_eq!(count_in_progress(&h.db).await, 0);
    let assistants = assistant_rows(&h.db, &h.session_id).await;
    // 崩溃残留的 tool_use 行(seq 1)+ 恢复后新答案行(seq 4)。
    assert_eq!(
        assistants.len(),
        2,
        "orphan assistant + post-recovery answer"
    );
    let last = assistants.last().unwrap();
    assert!(last.text.contains("after recovery"));
    assert!(last.status.is_none());
}

// ---------------------------------------------------------------------------
// AC2 等效(检查点行在崩溃后以 in_progress 形态可见): DB 层构造
// ---------------------------------------------------------------------------

/// 用 harness 的真实池手工落一条 in_progress 检查点行(等同"流式中
/// 崩溃"的 DB 终态,AC2 的分解形态 —— 进程内崩溃模拟成本过高,见
/// implement.md WP2 第三条),再跑恢复,断言行含检查点内容 +
/// status 流转(AC3)。覆盖"agent harness 池 × 恢复 pass"的组合。
#[tokio::test]
async fn agent_loop_harness_pool_recovery_marks_interrupted() {
    let h = make_harness().await;
    db::upsert_in_progress_turn(
        &h.db,
        &h.session_id,
        0,
        &[ContentBlock::Text {
            text: "checkpointed partial".into(),
            cache_control: None,
        }],
        None,
    )
    .await
    .unwrap();

    let report = db::recover_interrupted_messages(&h.db).await.unwrap();
    assert_eq!(report.interrupted, 1);

    let loaded = db::load_session(&h.db, &h.session_id)
        .await
        .unwrap()
        .unwrap();
    let row = loaded
        .messages
        .iter()
        .find(|m| m.seq == 0)
        .expect("checkpoint row survived as interrupted");
    assert_eq!(row.status.as_deref(), Some("interrupted"));
    assert!(row.text.contains("checkpointed partial"));
    assert!(row.text.contains(crate::agent::helpers::INTERRUPTED_MARKER));
    assert_eq!(
        count_in_progress(&h.db).await,
        1,
        "exactly the interrupted row"
    );
}
