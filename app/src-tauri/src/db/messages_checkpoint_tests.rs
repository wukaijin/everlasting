#![cfg(test)]

//! RULE-PERSIST-001 (08-24-p1-turn-crash-recovery) WP1 单元测试:
//! turn 流式检查点三函数(upsert / finalize / delete)+ 启动恢复 pass
//! (Step A in_progress 残留 / Step B 尾部孤儿 tool_use)。
//!
//! **file-backed 建池**(init_pool + tempdir),不用其余 db 测试的
//! `sqlite::memory:` 惯例 —— 恢复 pass 是生产启动路径的真实写链路,
//! 对齐 backup.rs 的教训(sqlx :memory: 池存在静默 no-op 坑,
//! database-guidelines §备份 Scenario);file-backed 才是 daemon
//! 真实形态(WAL + busy_timeout + FK pragma)。

use sqlx::{Row, SqlitePool};

use crate::db::sessions::messages::{
    delete_in_progress_turn, finalize_turn_persist, recover_interrupted_messages,
    upsert_in_progress_turn, RecoveryReport,
};
use crate::llm::types::{ContentBlock, MessageContent, Role};
use crate::projects::DEFAULT_PROJECT_ID;

/// File-backed pool with the production config (`init_pool`: WAL +
/// busy_timeout + FK pragmas) + full migrations, rooted at
/// `<data_dir>/everlasting.db` just like the daemon. The TempDir guard
/// is returned alongside — caller must keep it alive for the pool's
/// lifetime.
async fn file_backed_pool() -> (SqlitePool, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::db::migrations::init_pool(&dir.path().join("everlasting.db"))
        .await
        .unwrap();
    crate::db::migrations::run_migrations(&pool).await.unwrap();
    (pool, dir)
}

async fn make_session(pool: &SqlitePool) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    crate::db::sessions::create_session(
        pool,
        &id,
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    id
}

fn text_block(s: &str) -> ContentBlock {
    ContentBlock::Text {
        text: s.to_string(),
        cache_control: None,
    }
}

/// Fetch `(status, text, content)` for the row at `(session_id, seq)`.
/// Panics when the row doesn't exist (test invariant).
async fn row_at(pool: &SqlitePool, session_id: &str, seq: i64) -> (Option<String>, String, String) {
    let row =
        sqlx::query("SELECT status, text, content FROM messages WHERE session_id = ? AND seq = ?")
            .bind(session_id)
            .bind(seq)
            .fetch_one(pool)
            .await
            .expect("row exists");
    (
        row.try_get("status").unwrap(),
        row.try_get("text").unwrap(),
        row.try_get("content").unwrap(),
    )
}

async fn count_status_not_null(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE status IS NOT NULL")
        .fetch_one(pool)
        .await
        .unwrap()
}

// =====================================================================
// 写点三函数
// =====================================================================

/// 占位 → 周期 upsert:同一 (session_id, seq) 只有一行,内容反映
/// 最后一次 upsert,status 保持 'in_progress'。
#[tokio::test]
async fn upsert_in_progress_is_idempotent_and_overwrites() {
    let (pool, _dir) = file_backed_pool().await;
    let sid = make_session(&pool).await;

    // 写点①:空占位。
    upsert_in_progress_turn(&pool, &sid, 1, &[], None)
        .await
        .unwrap();
    let (status, text, content) = row_at(&pool, &sid, 1).await;
    assert_eq!(status.as_deref(), Some("in_progress"));
    assert_eq!(text, "");
    assert_eq!(content, "[]");

    // 写点②:周期检查点覆盖(同 seq)。
    let blocks = vec![text_block("partial answer")];
    upsert_in_progress_turn(&pool, &sid, 1, &blocks, Some("alice"))
        .await
        .unwrap();
    let (status, text, content) = row_at(&pool, &sid, 1).await;
    assert_eq!(status.as_deref(), Some("in_progress"));
    assert_eq!(text, "partial answer");
    assert!(content.contains("partial answer"), "got: {}", content);
    // 单行 —— upsert 没有产生第二行。
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE session_id = ?")
        .bind(&sid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
    // speaker 随检查点透传(群聊场景)。
    let speaker: Option<String> =
        sqlx::query_scalar("SELECT speaker FROM messages WHERE session_id = ? AND seq = 1")
            .bind(&sid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(speaker.as_deref(), Some("alice"));
}

/// 收尾 finalize:覆盖检查点行、status 清回 NULL(终态)、latency
/// 落列 —— 状态机的 in_progress → NULL 流转。
#[tokio::test]
async fn finalize_overwrites_checkpoint_and_clears_status() {
    let (pool, _dir) = file_backed_pool().await;
    let sid = make_session(&pool).await;

    upsert_in_progress_turn(&pool, &sid, 1, &[text_block("checkpoint text")], None)
        .await
        .unwrap();
    let latency = crate::db::sessions::messages::MessageLatency {
        ttfb_ms: Some(120),
        gen_ms: Some(340),
        total_ms: Some(460),
        thinking_ms: None,
    };
    finalize_turn_persist(
        &pool,
        &sid,
        Role::Assistant,
        &MessageContent::Blocks(vec![text_block("final text")]),
        1,
        Some(&latency),
        None,
    )
    .await
    .unwrap();

    let (status, text, _) = row_at(&pool, &sid, 1).await;
    assert_eq!(status, None, "finalize must clear status to NULL");
    assert_eq!(text, "final text");
    let ttfb: Option<i64> =
        sqlx::query_scalar("SELECT ttfb_ms FROM messages WHERE session_id = ? AND seq = 1")
            .bind(&sid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(ttfb, Some(120));
    assert_eq!(count_status_not_null(&pool).await, 0);
}

/// finalize 对不存在的 seq 也直接 INSERT(无检查点时的正常路径,
/// 语义 == persist_turn)。
#[tokio::test]
async fn finalize_without_checkpoint_inserts() {
    let (pool, _dir) = file_backed_pool().await;
    let sid = make_session(&pool).await;
    finalize_turn_persist(
        &pool,
        &sid,
        Role::Assistant,
        &MessageContent::Blocks(vec![text_block("direct")]),
        2,
        None,
        None,
    )
    .await
    .unwrap();
    let (status, text, _) = row_at(&pool, &sid, 2).await;
    assert_eq!(status, None);
    assert_eq!(text, "direct");
}

/// delete 的 status 守卫:只吃自己的 in_progress 占位;对终态行
/// (status NULL)是 no-op,返回 0。
#[tokio::test]
async fn delete_only_touches_in_progress_rows() {
    let (pool, _dir) = file_backed_pool().await;
    let sid = make_session(&pool).await;

    // 终态行(persist_turn 裸 INSERT,status NULL)。
    crate::db::sessions::persist_turn(
        &pool,
        &sid,
        Role::User,
        &MessageContent::Text("question".to_string()),
        0,
        None,
        None,
    )
    .await
    .unwrap();
    let deleted = delete_in_progress_turn(&pool, &sid, 0).await.unwrap();
    assert_eq!(deleted, 0, "terminal row must be a no-op");
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE session_id = ?")
        .bind(&sid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1, "terminal row untouched");

    // in_progress 占位 → 删除。
    upsert_in_progress_turn(&pool, &sid, 1, &[], None)
        .await
        .unwrap();
    let deleted = delete_in_progress_turn(&pool, &sid, 1).await.unwrap();
    assert_eq!(deleted, 1);
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE session_id = ?")
        .bind(&sid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1, "only the placeholder is gone");
}

// =====================================================================
// 恢复 pass:Step A(in_progress 残留)
// =====================================================================

/// AC3 前半:空占位(零内容,或仅空文本块)→ 删除,不残留空行。
#[tokio::test]
async fn recover_deletes_empty_placeholder() {
    let (pool, _dir) = file_backed_pool().await;
    let sid = make_session(&pool).await;

    // 完全空占位。
    upsert_in_progress_turn(&pool, &sid, 1, &[], None)
        .await
        .unwrap();
    // 仅一个空 Text 块的占位(也算"空")。
    upsert_in_progress_turn(&pool, &sid, 2, &[text_block("")], None)
        .await
        .unwrap();

    let report = recover_interrupted_messages(&pool).await.unwrap();
    assert_eq!(
        report,
        RecoveryReport {
            interrupted: 0,
            deleted: 2,
            orphan_repaired: 0
        }
    );
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE session_id = ?")
        .bind(&sid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0, "empty placeholders deleted");
}

/// AC3 主体:有内容的检查点行 → 追加 INTERRUPTED_MARKER 独立 Text
/// 块(`\n\n` 前缀,对齐 drive.rs cancel/error marker 约定)+ status
/// ='interrupted'。
#[tokio::test]
async fn recover_marks_content_row_interrupted() {
    let (pool, _dir) = file_backed_pool().await;
    let sid = make_session(&pool).await;

    let blocks = vec![text_block("partial generation before crash")];
    upsert_in_progress_turn(&pool, &sid, 1, &blocks, None)
        .await
        .unwrap();
    let report = recover_interrupted_messages(&pool).await.unwrap();
    assert_eq!(report.interrupted, 1);
    assert_eq!(report.deleted, 0);

    let (status, text, content) = row_at(&pool, &sid, 1).await;
    assert_eq!(status.as_deref(), Some("interrupted"));
    assert!(text.contains("partial generation before crash"));
    assert!(
        text.contains(crate::agent::helpers::INTERRUPTED_MARKER),
        "marker visible in denormalized text, got: {}",
        text
    );
    // marker 是独立 Text 块,带 \n\n 前缀(有前文时)。
    let parsed: Vec<ContentBlock> = serde_json::from_str(&content).unwrap();
    let last = parsed.last().expect("marker block appended");
    match last {
        ContentBlock::Text { text, .. } => {
            assert_eq!(
                *text,
                format!("\n\n{}", crate::agent::helpers::INTERRUPTED_MARKER)
            );
        }
        other => panic!("last block must be the marker Text block, got: {:?}", other),
    }
    // 恢复幂等:再跑一遍无 in_progress 残留可处理,行不再变化。
    let report2 = recover_interrupted_messages(&pool).await.unwrap();
    assert_eq!(report2.total(), 0);
    let (status2, _, _) = row_at(&pool, &sid, 1).await;
    assert_eq!(status2.as_deref(), Some("interrupted"));
}

/// 有 thinking 无 text 的检查点行:marker 无 `\n\n` 前缀(无可见
/// 前文,对齐 drive.rs 空 turn marker 分支)。
#[tokio::test]
async fn recover_marker_without_prefix_when_no_visible_text() {
    let (pool, _dir) = file_backed_pool().await;
    let sid = make_session(&pool).await;
    let blocks = vec![ContentBlock::Thinking {
        thinking: "reasoning".to_string(),
        signature: "sig".to_string(),
    }];
    upsert_in_progress_turn(&pool, &sid, 1, &blocks, None)
        .await
        .unwrap();
    let report = recover_interrupted_messages(&pool).await.unwrap();
    assert_eq!(report.interrupted, 1);
    let (_, text, content) = row_at(&pool, &sid, 1).await;
    // text 列不含 thinking 文本(项目不变量:to_text 排除 thinking)。
    assert!(!text.contains("reasoning"));
    assert_eq!(
        text,
        crate::agent::helpers::INTERRUPTED_MARKER,
        "bare marker when no preceding visible text"
    );
    let parsed: Vec<ContentBlock> = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed.len(), 2, "thinking block preserved + marker");
}

// =====================================================================
// 恢复 pass:Step B(尾部孤儿 tool_use,W2 窗口)
// =====================================================================

/// AC4:assistant(tool_use) 尾行无配对 tool_result → 恢复追加合成
/// is_error tool_result user 行(seq+1),pair atomicity 修复。
#[tokio::test]
async fn recover_repairs_orphan_tool_use_tail() {
    let (pool, _dir) = file_backed_pool().await;
    let sid = make_session(&pool).await;

    crate::db::sessions::persist_turn(
        &pool,
        &sid,
        Role::User,
        &MessageContent::Text("run a tool".to_string()),
        0,
        None,
        None,
    )
    .await
    .unwrap();
    // W2 残留:assistant(tool_use) 已落库,tool_result 因崩溃丢失。
    let assistant_content = MessageContent::Blocks(vec![
        text_block("let me check"),
        ContentBlock::ToolUse {
            id: "toolu_orphan".to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({"path": "/tmp/x"}),
        },
    ]);
    crate::db::sessions::persist_turn(
        &pool,
        &sid,
        Role::Assistant,
        &assistant_content,
        1,
        None,
        None,
    )
    .await
    .unwrap();

    // R2.4 断言锚:先把 updated_at 钉到哨兵值,恢复后必须被 touch。
    sqlx::query("UPDATE sessions SET updated_at = '2000-01-01T00:00:00Z' WHERE id = ?")
        .bind(&sid)
        .execute(&pool)
        .await
        .unwrap();

    let report = recover_interrupted_messages(&pool).await.unwrap();
    assert_eq!(report.orphan_repaired, 1);
    assert_eq!(report.interrupted, 0);
    assert_eq!(report.deleted, 0);

    let (status, _, content) = row_at(&pool, &sid, 2).await;
    assert_eq!(status, None, "synthetic row is terminal");
    let parsed: Vec<ContentBlock> = serde_json::from_str(&content).unwrap();
    match parsed.as_slice() {
        [ContentBlock::ToolResult {
            tool_use_id,
            is_error,
            ..
        }] => {
            assert_eq!(tool_use_id, "toolu_orphan");
            assert!(*is_error, "synthetic result must be is_error");
        }
        other => panic!(
            "expected single synthetic tool_result block, got: {:?}",
            other
        ),
    }
    let role: String =
        sqlx::query_scalar("SELECT role FROM messages WHERE session_id = ? AND seq = 2")
            .bind(&sid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(role, "user", "tool_result blocks live in user-role rows");
    let has_tool_results: i64 = sqlx::query_scalar(
        "SELECT has_tool_results FROM messages WHERE session_id = ? AND seq = 2",
    )
    .bind(&sid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(has_tool_results, 1);

    // R2.4:受影响 session 的 updated_at 被 touch(≠ 哨兵值)。
    let updated: String = sqlx::query_scalar("SELECT updated_at FROM sessions WHERE id = ?")
        .bind(&sid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_ne!(updated, "2000-01-01T00:00:00Z");

    // 幂等:修复后的尾行是 user(tool_result),不再触发 Step B。
    let report2 = recover_interrupted_messages(&pool).await.unwrap();
    assert_eq!(report2.total(), 0);
}

/// 多 tool_use 同行 → 每个都得到配对 tool_result;多 session 各自
/// 修复。
#[tokio::test]
async fn recover_repairs_multiple_sessions_and_multiple_tool_uses() {
    let (pool, _dir) = file_backed_pool().await;
    let sid_a = make_session(&pool).await;
    let sid_b = make_session(&pool).await;
    // 另一个健康 session(user + assistant 纯文本尾)—— 不该被动。
    let sid_clean = make_session(&pool).await;

    let two_tools = MessageContent::Blocks(vec![
        ContentBlock::ToolUse {
            id: "t1".to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({}),
        },
        ContentBlock::ToolUse {
            id: "t2".to_string(),
            name: "list_dir".to_string(),
            input: serde_json::json!({}),
        },
    ]);
    crate::db::sessions::persist_turn(
        &pool,
        &sid_a,
        Role::User,
        &MessageContent::Text("q".into()),
        0,
        None,
        None,
    )
    .await
    .unwrap();
    crate::db::sessions::persist_turn(&pool, &sid_a, Role::Assistant, &two_tools, 1, None, None)
        .await
        .unwrap();
    crate::db::sessions::persist_turn(
        &pool,
        &sid_b,
        Role::User,
        &MessageContent::Text("q".into()),
        0,
        None,
        None,
    )
    .await
    .unwrap();
    crate::db::sessions::persist_turn(
        &pool,
        &sid_b,
        Role::Assistant,
        &MessageContent::Blocks(vec![ContentBlock::ToolUse {
            id: "t3".to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({}),
        }]),
        1,
        None,
        None,
    )
    .await
    .unwrap();
    crate::db::sessions::persist_turn(
        &pool,
        &sid_clean,
        Role::User,
        &MessageContent::Text("hi".into()),
        0,
        None,
        None,
    )
    .await
    .unwrap();
    crate::db::sessions::persist_turn(
        &pool,
        &sid_clean,
        Role::Assistant,
        &MessageContent::Text("answer".into()),
        1,
        None,
        None,
    )
    .await
    .unwrap();

    let report = recover_interrupted_messages(&pool).await.unwrap();
    assert_eq!(report.orphan_repaired, 2, "one repair per orphan session");

    // sid_a 的合成行覆盖两个 tool_use id。
    let (_, _, content) = row_at(&pool, &sid_a, 2).await;
    let parsed: Vec<ContentBlock> = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed.len(), 2);
    let ids: Vec<&str> = parsed
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(ids, vec!["t1", "t2"], "one tool_result per tool_use");

    // 健康 session 无新增行。
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE session_id = ?")
        .bind(&sid_clean)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 2);
}

/// 健康 pair(assistant(tool_use) + user(tool_result))不触发 Step B;
/// 纯文本 assistant 尾行也不触发。
#[tokio::test]
async fn recover_ignores_healthy_tails() {
    let (pool, _dir) = file_backed_pool().await;
    let sid = make_session(&pool).await;
    crate::db::sessions::persist_turn(
        &pool,
        &sid,
        Role::User,
        &MessageContent::Text("q".into()),
        0,
        None,
        None,
    )
    .await
    .unwrap();
    crate::db::sessions::persist_turn(
        &pool,
        &sid,
        Role::Assistant,
        &MessageContent::Blocks(vec![ContentBlock::ToolUse {
            id: "t_ok".to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({}),
        }]),
        1,
        None,
        None,
    )
    .await
    .unwrap();
    crate::db::sessions::persist_turn(
        &pool,
        &sid,
        Role::User,
        &MessageContent::Blocks(vec![ContentBlock::ToolResult {
            tool_use_id: "t_ok".to_string(),
            content: "result".to_string(),
            is_error: false,
            images: None,
            resolved: None,
        }]),
        2,
        None,
        None,
    )
    .await
    .unwrap();

    let report = recover_interrupted_messages(&pool).await.unwrap();
    assert_eq!(report.total(), 0, "healthy pair is a no-op");
}

// =====================================================================
// 恢复 pass:复合 + AC6(干净 DB no-op)
// =====================================================================

/// Step A 标成 interrupted 的行若本身含 tool_use(崩溃发生在模型已
/// emit tool_use 后的流式尾部),Step B(后跑)同样要修复 —— 恢复
/// 后的 session 既能看到中断 marker,也不会 400。
#[tokio::test]
async fn recover_interrupted_row_with_tool_use_gets_both_steps() {
    let (pool, _dir) = file_backed_pool().await;
    let sid = make_session(&pool).await;
    crate::db::sessions::persist_turn(
        &pool,
        &sid,
        Role::User,
        &MessageContent::Text("q".into()),
        0,
        None,
        None,
    )
    .await
    .unwrap();
    let blocks = vec![
        text_block("partial"),
        ContentBlock::ToolUse {
            id: "toolu_mid".to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({}),
        },
    ];
    upsert_in_progress_turn(&pool, &sid, 1, &blocks, None)
        .await
        .unwrap();

    let report = recover_interrupted_messages(&pool).await.unwrap();
    assert_eq!(report.interrupted, 1);
    assert_eq!(report.orphan_repaired, 1);

    let (status, text, _) = row_at(&pool, &sid, 1).await;
    assert_eq!(status.as_deref(), Some("interrupted"));
    assert!(text.contains(crate::agent::helpers::INTERRUPTED_MARKER));
    // seq 2 = 合成 tool_result(user-role,终态)。
    let (status2, _, content2) = row_at(&pool, &sid, 2).await;
    assert_eq!(status2, None);
    assert!(content2.contains("toolu_mid"));
    assert_eq!(
        count_status_not_null(&pool).await,
        1,
        "only the interrupted row"
    );
}

/// AC6:干净 DB(有 session、有正常行)→ no-op 不报错;全新空库
/// 同样 no-op。RecoveryReport 计数全零。
#[tokio::test]
async fn recover_on_clean_db_is_noop() {
    let (pool, _dir) = file_backed_pool().await;
    let sid = make_session(&pool).await;
    crate::db::sessions::persist_turn(
        &pool,
        &sid,
        Role::User,
        &MessageContent::Text("q".into()),
        0,
        None,
        None,
    )
    .await
    .unwrap();
    crate::db::sessions::persist_turn(
        &pool,
        &sid,
        Role::Assistant,
        &MessageContent::Text("a".into()),
        1,
        None,
        None,
    )
    .await
    .unwrap();
    let report = recover_interrupted_messages(&pool).await.unwrap();
    assert_eq!(report, RecoveryReport::default());
    assert_eq!(report.total(), 0);
}
