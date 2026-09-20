#![cfg(test)]

//! N2 turn-boundary checkpoint DB 层测试(2026-09-20, task
//! 09-20-n2-checkpoint-revert):`turn_checkpoints` CRUD + 约束语义。
//!
//! - upsert(INSERT OR REPLACE)幂等:同 (session_id, seq) 重写整行,
//!   行数不涨(design §2.6);
//! - latest / list / has_baseline 的读法;
//! - FK CASCADE:删 session 级联清行(AC6 的 DB 半;伞 ref 半见
//!   `agent/tests_agent_loop/checkpoint.rs`);
//! - D3 交互(AC11,OQ-B 裁定):`edit_user_message` 级联删尾在同一
//!   事务内跟随删 `seq > message_seq` 的 checkpoint 行 —— 删尾重跑
//!   经 seq 回落复用不会撞出孤儿行/静默顶行。
//!
//! **test_pool**(`sqlite::memory:` + FK pragma + 全量迁移)足够:本
//! 文件没有 VACUUM INTO 类静默 no-op 坑(教训见 database-guidelines
//! §备份 Scenario);FK CASCADE 依赖池级 pragma,test_support 已带。

use sqlx::SqlitePool;

use crate::db::checkpoint::{
    has_checkpoint_baseline, latest_checkpoint, list_checkpoints, upsert_checkpoint, CheckpointRow,
};
use crate::db::test_support::test_pool;
use crate::llm::types::{MessageContent, Role};
use crate::projects::DEFAULT_PROJECT_ID;

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
    .expect("create_session");
    id
}

async fn row(pool: &SqlitePool, sid: &str, seq: i64, commit: &str) {
    upsert_checkpoint(pool, sid, seq, &format!("tree-{commit}"), commit)
        .await
        .expect("upsert_checkpoint");
}

#[tokio::test]
async fn upsert_is_replace_idempotent_on_same_seq() {
    let pool = test_pool().await;
    let sid = make_session(&pool).await;

    row(&pool, &sid, 0, "c0").await;
    // 同 seq 重试:REPLACE 整行重写(commit_sha 变),行数恒 1。
    row(&pool, &sid, 0, "c0-retry").await;
    // 推进 seq:正常链 append。
    row(&pool, &sid, 1, "c1").await;

    let rows = list_checkpoints(&pool, &sid).await.unwrap();
    assert_eq!(rows.len(), 2, "同 seq REPLACE 不涨行");
    assert_eq!(rows[0].commit_sha, "c0-retry");
    assert_eq!(rows[1].commit_sha, "c1");
    assert_eq!(rows[1].tree_sha, "tree-c1");
    // created_at = unix ms(scheduled_tasks 同风格 INTEGER 毫秒)。
    assert!(rows[0].created_at > 1_600_000_000_000);
}

#[tokio::test]
async fn latest_and_has_baseline_read_the_chain() {
    let pool = test_pool().await;
    let sid = make_session(&pool).await;

    assert!(!has_checkpoint_baseline(&pool, &sid).await.unwrap());
    assert!(latest_checkpoint(&pool, &sid).await.unwrap().is_none());

    row(&pool, &sid, 3, "c3").await;
    row(&pool, &sid, 1, "c1").await;

    assert!(has_checkpoint_baseline(&pool, &sid).await.unwrap());
    let latest = latest_checkpoint(&pool, &sid).await.unwrap().unwrap();
    assert_eq!(latest.seq, 3, "latest = 最高 seq 行");
    assert_eq!(latest.commit_sha, "c3");

    // 空 session 与已删 session 一样读空。
    assert!(!has_checkpoint_baseline(&pool, "no-such-session")
        .await
        .unwrap());
}

#[tokio::test]
async fn session_delete_cascades_checkpoint_rows() {
    // AC6 的 DB 半:伞 ref 由 delete_session_inner 清(集成测试在
    // agent 侧),DB 行靠 FK CASCADE —— 依赖 test_pool 的 FK pragma。
    let pool = test_pool().await;
    let sid = make_session(&pool).await;
    row(&pool, &sid, 0, "c0").await;
    row(&pool, &sid, 1, "c1").await;

    crate::db::delete_session(&pool, &sid).await.unwrap();

    let rows = list_checkpoints(&pool, &sid).await.unwrap();
    assert!(rows.is_empty(), "FK CASCADE 必须清 turn_checkpoints 行");
}

// ---------------------------------------------------------------------------
// D3 交互(AC11,OQ-B 裁定):edit_user_message 事务内级联删 checkpoint 行
// ---------------------------------------------------------------------------

async fn persist_message(pool: &SqlitePool, sid: &str, seq: i64, text: &str, role: Role) {
    crate::db::persist_turn(
        pool,
        sid,
        role,
        &MessageContent::Text(text.to_string()),
        seq,
        None,
        None,
    )
    .await
    .expect("persist_turn");
}

#[tokio::test]
async fn edit_user_message_cascade_deletes_checkpoint_tail_ac11() {
    let pool = test_pool().await;
    let sid = make_session(&pool).await;

    // 会话形状:user(0) → assistant(1) → user(2) → assistant(3)。
    persist_message(&pool, &sid, 0, "first prompt", Role::User).await;
    persist_message(&pool, &sid, 1, "answer one", Role::Assistant).await;
    persist_message(&pool, &sid, 2, "second prompt", Role::User).await;
    persist_message(&pool, &sid, 3, "answer two", Role::Assistant).await;

    // checkpoint 链:基线(seq 0)+ 两个写轮(seq 1 / seq 3)。
    row(&pool, &sid, 0, "c0").await;
    row(&pool, &sid, 1, "c1").await;
    row(&pool, &sid, 3, "c3").await;

    // D3:编辑第二条 user 消息(seq 2)→ messages seq>2 与
    // turn_checkpoints seq>2 同事务级联删;基线与 seq1 行幸存。
    crate::db::edit_user_message(
        &pool,
        &sid,
        2,
        &MessageContent::Text("second prompt (edited)".to_string()),
    )
    .await
    .expect("edit_user_message");

    let rows = list_checkpoints(&pool, &sid).await.unwrap();
    let seqs: Vec<i64> = rows.iter().map(|r: &CheckpointRow| r.seq).collect();
    assert_eq!(seqs, vec![0, 1], "被删轮区间的 checkpoint 行必须同步消失");
    assert_eq!(rows.last().unwrap().commit_sha, "c1");

    // 重跑经 seq 回落复用:新 user 行落 seq 3 —— REPLACE 语义下即使
    // 残留孤儿行也只会被顶掉,但级联删后此处是干净的 append。
    persist_message(&pool, &sid, 3, "second prompt (resent)", Role::User).await;
    row(&pool, &sid, 3, "c3-new").await;
    let rows = list_checkpoints(&pool, &sid).await.unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[2].commit_sha, "c3-new", "无 PK 撞静默零行");
}

#[tokio::test]
async fn edit_user_message_noop_keeps_checkpoints() {
    // no-op 快路径(内容未变)不写任何状态 —— checkpoint 行也不动。
    let pool = test_pool().await;
    let sid = make_session(&pool).await;
    persist_message(&pool, &sid, 0, "prompt", Role::User).await;
    row(&pool, &sid, 0, "c0").await;

    crate::db::edit_user_message(&pool, &sid, 0, &MessageContent::Text("prompt".to_string()))
        .await
        .expect("edit_user_message noop");

    let rows = list_checkpoints(&pool, &sid).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].commit_sha, "c0");
}
