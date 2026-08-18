#![cfg(test)]
// C3 摘要压缩 PR2(08-18-llm-context-compaction)——
// `insert_compaction_summary` 的 seq 游标契约 + 列契约(design §4.3 /
// 复核 P1):
// 1. 插入行的 seq == 传入游标(不走独立 MAX(seq)+1),返回游标+1;
// 2. content 与 text 两列同值存纯摘要(水位替换的对齐锚点在 text 列,
//    PR1 check 固化);
// 3. metadata 原样落库(kind = compaction_summary 可被
//    `apply_compaction_watermark` 命中)。

use uuid::Uuid;

use crate::agent::compaction::COMPACTION_SUMMARY_KIND;
use crate::llm::types::{ContentBlock, MessageContent, Role};
use crate::projects::DEFAULT_PROJECT_ID;

use super::sessions::{create_session, insert_compaction_summary, load_session, persist_turn};
use super::test_pool;

fn summary_metadata() -> serde_json::Value {
    serde_json::json!({
        "kind": COMPACTION_SUMMARY_KIND,
        "cutoff_seq": 3,
        "tokens_before": 171_000,
        "tokens_after": 52_300,
        "trigger": "auto",
        "model": "Mock",
        "prior_summary_seq": null,
        "summary_usage": {"input_tokens": 91_000, "output_tokens": 5_200,
                          "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0,
                          "context_input_tokens": 91_000},
    })
}

/// seq 游标推进:摘要行落在传入 seq 上,返回 seq+1;loop 后续 persist
/// 用返回值不撞主键。
#[tokio::test]
async fn insert_compaction_summary_advances_seq_cursor() {
    let pool = test_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    // loop 已 persist 了 seq 0(user)与 seq 1(assistant),游标在 2。
    persist_turn(
        &pool,
        &s.id,
        Role::User,
        &MessageContent::Text("q1".into()),
        0,
        None,
        None,
    )
    .await
    .unwrap();
    persist_turn(
        &pool,
        &s.id,
        Role::Assistant,
        &MessageContent::Text("a1".into()),
        1,
        None,
        None,
    )
    .await
    .unwrap();

    let advanced = insert_compaction_summary(&pool, &s.id, "SUMMARY_BODY", 2, &summary_metadata())
        .await
        .unwrap();
    assert_eq!(advanced, 3, "返回推进后的游标(seq + 1)");

    // loop 用返回的游标继续 persist —— 不撞 (session_id, seq) 主键。
    persist_turn(
        &pool,
        &s.id,
        Role::Assistant,
        &MessageContent::Text("after-summary turn".into()),
        advanced,
        None,
        None,
    )
    .await
    .unwrap();

    let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
    assert_eq!(loaded.messages.len(), 4);
    assert_eq!(loaded.messages[2].seq, 2, "摘要行落在传入游标上");
    assert_eq!(loaded.messages[3].seq, 3, "后续 turn 排在摘要行之后");
}

/// 列契约:role=user、content 单 Text 块、**text 列同值纯摘要**
/// (rehydrate 回发 text 列原文 —— 水位替换对齐锚点);metadata.kind
/// 可被水位算法识别。
#[tokio::test]
async fn insert_compaction_summary_column_contract() {
    let pool = test_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    insert_compaction_summary(&pool, &s.id, "PURE SUMMARY TEXT", 0, &summary_metadata())
        .await
        .unwrap();

    let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
    assert_eq!(loaded.messages.len(), 1);
    let row = &loaded.messages[0];
    assert_eq!(row.role, "user");
    assert_eq!(row.seq, 0);
    // 两列同值。
    assert_eq!(row.text, "PURE SUMMARY TEXT");
    let blocks: Vec<ContentBlock> = serde_json::from_value(row.content.clone()).unwrap();
    assert_eq!(blocks.len(), 1);
    match &blocks[0] {
        ContentBlock::Text { text, .. } => {
            assert_eq!(
                text, "PURE SUMMARY TEXT",
                "content 块文本 = 纯摘要,无前缀话术"
            );
        }
        other => panic!("expected text block, got {:?}", other),
    }
    // metadata 原样落库,kind 可被 PR1 的水位算法命中。
    let meta = row.metadata.as_ref().expect("metadata present");
    assert_eq!(meta["kind"], COMPACTION_SUMMARY_KIND);
    assert_eq!(meta["tokens_before"], 171_000);
    assert_eq!(meta["summary_usage"]["input_tokens"], 91_000);
    // 与 insert_system_event 先例的关键差异:两列不分叉。
    assert_ne!(row.text, "", "text 列非空(水位对齐锚点在 text 列)");
}
