//! N19（2026-09-29, task `09-29-n19-mcode-semantics-impl`）：执行阶段取消的
//! tool_use/tool_result 配对完整性 —— `finalize_turn` 取消臂差集补齐的
//! 直测层。与 drive.rs send 阶段取消的全量 synthetic（error_path.rs 已覆盖）
//! 共同保证不变量：任意取消后，DB 尾部 assistant(tool_use×N) 的下一条 user
//! 消息恒含 N 个 tool_result（真 + synthetic 差集）。
//!
//! 直呼 `finalize_turn`（而非整跑 run_chat_loop）：本件测的是 finalize 层
//! 的差集逻辑本身，dispatch/drive 层由既有 `agent_loop_cancel_*` 族覆盖；
//! 直呼避免为「构造 serial 批中途取消」搭建 provider 时序脚本。

use std::sync::Arc;

use super::tests_common::{
    chat_loop_deps, chat_loop_request, make_harness, parent_role, test_messages, MockEmitter,
};
use crate::agent::chat_loop::{finalize_turn, FinalizeFrame};
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatMessage, ContentBlock, MessageContent, Role};

fn tool_call(id: &str, name: &str) -> (String, String, serde_json::Value) {
    (id.to_string(), name.to_string(), serde_json::json!({}))
}

/// A real (non-synthetic) tool result block — what dispatch produces
/// for a tool that ran to completion before the cancel landed.
fn real_result(id: &str) -> ContentBlock {
    ContentBlock::ToolResult {
        tool_use_id: id.to_string(),
        content: "real tool output".to_string(),
        is_error: false,
        images: None,
        resolved: None,
    }
}

/// (tool_use_id, is_error) pairs of every ToolResult block in a
/// Blocks message, in block order.
fn tool_result_pairs(msg: &ChatMessage) -> Vec<(String, bool)> {
    match &msg.content {
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolResult {
                    tool_use_id,
                    is_error,
                    ..
                } => Some((tool_use_id.clone(), *is_error)),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// 主用例（AC4）：5 个 tool_use，2 个真结果后取消 —— 落库消息必含
/// 5 个 tool_result（2 真 + 3 synthetic is_error），loop_hint Text 仍在
/// 末尾（wire 顺序约束：tool×N → user(text)，OpenAI tool 消息紧邻性）。
/// 修复前该消息只带 2 个真结果，尾部 3 个 tool_use 悬空，靠 wire 层
/// 每请求 orphan 注入兜底（该 warn 出现即回归信号）。
#[tokio::test]
async fn finalize_cancel_appends_synthetic_difference_set() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    // finalize_turn 不调 provider；空脚本即可。
    let mock = Arc::new(MockProvider::new(vec![MockResponse::HangingThenCancel]));
    let request = chat_loop_request(
        vec![],
        mock,
        200_000,
        "rid-pairing".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
    );
    let deps = chat_loop_deps(&h);
    let role = parent_role(&h);

    let tool_calls = vec![
        tool_call("toolu_1", "list_dir"),
        tool_call("toolu_2", "read_file"),
        tool_call("toolu_3", "grep"),
        tool_call("toolu_4", "shell"),
        tool_call("toolu_5", "edit_file"),
    ];
    // 前两个跑完出了真结果，第三个执行中取消 break —— 后三个无 result。
    let result_blocks = vec![real_result("toolu_1"), real_result("toolu_2")];
    let mut messages = test_messages();
    let loop_hint = Some("loop hint text".to_string());
    let last_cwd: Option<std::path::PathBuf> = None;

    let outcome = finalize_turn(
        &request,
        &deps,
        &role,
        FinalizeFrame {
            result_blocks,
            loop_hint: &loop_hint,
            cancelled: true,
            seq: 7,
            messages: &mut messages,
            last_cwd: &last_cwd,
            tool_calls: &tool_calls,
        },
    )
    .await;
    assert!(
        outcome.is_err(),
        "cancel path returns Err(()) so the hub stops (terminal Done already emitted)"
    );

    // 内存 transcript：恰好追加一条 user 消息，5 个 tool_result。
    assert_eq!(messages.len(), 2, "one tool_result message appended");
    let tail = messages.last().unwrap();
    assert_eq!(tail.role, Role::User);
    let pairs = tool_result_pairs(tail);
    assert_eq!(
        pairs.len(),
        5,
        "tool_result count must equal tool_use count, got {pairs:?}"
    );
    for (id, is_err) in &pairs {
        match id.as_str() {
            "toolu_1" | "toolu_2" => assert!(!is_err, "real results keep is_error=false"),
            "toolu_3" | "toolu_4" | "toolu_5" => {
                assert!(is_err, "difference set must be synthetic is_error")
            }
            other => panic!("unexpected tool_use_id {other}"),
        }
    }
    // 顺序：真×2 → synthetic×3 → hint Text（hint 必须是最后一个 block）。
    match &tail.content {
        MessageContent::Blocks(blocks) => {
            assert_eq!(blocks.len(), 6);
            assert!(
                matches!(blocks.last(), Some(ContentBlock::Text { .. })),
                "loop_hint Text must stay terminal, got {:?}",
                blocks.last()
            );
        }
        _ => panic!("expected Blocks content"),
    }

    // 终态事件：恰好一个 Done{cancelled}（取消臂的固定终态）。
    assert_eq!(emitter.cancel_done_count(), 1);

    // DB 行：与上方的 assistant(tool_use×5) 组成无孤儿配对（AC4 的落库面）。
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT role, content FROM messages WHERE session_id = ? ORDER BY seq")
            .bind(&h.session_id)
            .fetch_all(&h.db)
            .await
            .expect("fetch messages");
    assert_eq!(
        rows.len(),
        1,
        "only the cancelled tool_result turn persisted"
    );
    let content: MessageContent =
        serde_json::from_str(&rows[0].1).expect("persisted content deserializes");
    let persisted = ChatMessage {
        role: Role::User,
        content,
        speaker: None,
        attachments: None,
    };
    assert_eq!(tool_result_pairs(&persisted).len(), 5);

    let assistant_turn = ChatMessage {
        role: Role::Assistant,
        content: MessageContent::Blocks(
            tool_calls
                .iter()
                .map(|(id, name, input)| ContentBlock::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                })
                .collect(),
        ),
        speaker: None,
        attachments: None,
    };
    let orphans = crate::llm::provider::wire::orphan_tool_use_ids(&[assistant_turn, persisted]);
    assert!(
        orphans.is_empty(),
        "cancelled turn must persist a fully-paired tail, orphans={orphans:?}"
    );
}

/// 幂等/门控回归（AC4 后半）：无取消时差集逻辑不触发 —— 即使故意留一个
/// 未配对 tool_use（正常路径 dispatch 恒配对，此构造只为钉死「synthetic
/// 只在取消臂追加」的门），消息也只含真结果，行为与改动前逐字节一致。
#[tokio::test]
async fn finalize_no_cancel_appends_nothing() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![MockResponse::HangingThenCancel]));
    let request = chat_loop_request(
        vec![],
        mock,
        200_000,
        "rid-pairing-nocancel".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
    );
    let deps = chat_loop_deps(&h);
    let role = parent_role(&h);

    let tool_calls = vec![
        tool_call("toolu_1", "list_dir"),
        tool_call("toolu_2", "read_file"),
    ];
    let result_blocks = vec![real_result("toolu_1")];
    let mut messages = test_messages();
    let loop_hint = Some("hint".to_string());
    let last_cwd: Option<std::path::PathBuf> = None;

    let outcome = finalize_turn(
        &request,
        &deps,
        &role,
        FinalizeFrame {
            result_blocks,
            loop_hint: &loop_hint,
            cancelled: false,
            seq: 3,
            messages: &mut messages,
            last_cwd: &last_cwd,
            tool_calls: &tool_calls,
        },
    )
    .await;
    assert!(outcome.is_ok(), "normal path returns Ok(())");

    let tail = messages.last().unwrap();
    let pairs = tool_result_pairs(tail);
    assert_eq!(pairs.len(), 1, "no synthetic on the no-cancel path");
    assert_eq!(pairs[0].0, "toolu_1");
    assert!(!pairs[0].1);
    match &tail.content {
        MessageContent::Blocks(blocks) => {
            assert!(matches!(blocks.last(), Some(ContentBlock::Text { .. })));
        }
        _ => panic!("expected Blocks content"),
    }
}

/// worker 模式（skip_persist，B6 PR1b）：差集补齐同样作用于内存
/// transcript（messages.push），但不落库 —— worker 的执行记录是
/// SubagentBufferSink transcript，DB 不出该行。
#[tokio::test]
async fn finalize_cancel_worker_mode_pairs_transcript_without_db_row() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![MockResponse::HangingThenCancel]));
    let request = chat_loop_request(
        vec![],
        mock,
        200_000,
        "rid-pairing-worker".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
    );
    let deps = chat_loop_deps(&h);
    let mut role = parent_role(&h);
    role.skip_persist = true;

    let tool_calls = vec![
        tool_call("toolu_1", "list_dir"),
        tool_call("toolu_2", "grep"),
        tool_call("toolu_3", "glob"),
    ];
    let result_blocks = vec![real_result("toolu_1")];
    let mut messages = test_messages();
    let loop_hint: Option<String> = None;
    let last_cwd: Option<std::path::PathBuf> = None;

    let outcome = finalize_turn(
        &request,
        &deps,
        &role,
        FinalizeFrame {
            result_blocks,
            loop_hint: &loop_hint,
            cancelled: true,
            seq: 5,
            messages: &mut messages,
            last_cwd: &last_cwd,
            tool_calls: &tool_calls,
        },
    )
    .await;
    assert!(outcome.is_err());

    // transcript 配对完整（worker 的执行记录面）。
    let tail = messages.last().unwrap();
    let pairs = tool_result_pairs(tail);
    assert_eq!(pairs.len(), 3, "worker transcript pairs every tool_use");
    assert!(!pairs[0].1, "toolu_1 real");
    assert!(pairs[1].1 && pairs[2].1, "difference set synthetic");

    // DB 无行（skip_persist 语义不变）。
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE session_id = ?")
        .bind(&h.session_id)
        .fetch_one(&h.db)
        .await
        .expect("count messages");
    assert_eq!(count, 0, "worker mode must not persist the cancelled turn");
}
