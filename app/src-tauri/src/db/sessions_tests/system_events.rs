#![cfg(test)]

use uuid::Uuid;

use crate::llm::types::{ContentBlock, MessageContent, Role};
use crate::projects::DEFAULT_PROJECT_ID;

use super::sessions::{create_session, insert_system_event, load_session, persist_turn};
use super::test_pool;

#[tokio::test]
async fn insert_system_event_appends_to_history() {
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
    persist_turn(
        &pool,
        &s.id,
        Role::User,
        &MessageContent::Text("hi".into()),
        0,
        None,
        None,
    )
    .await
    .unwrap();
    insert_system_event(
        &pool,
        &s.id,
        "worktree attached: /data/wt on branch session/abc",
        "attached",
    )
    .await
    .unwrap();
    let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
    assert_eq!(loaded.messages.len(), 2);
    let evt = &loaded.messages[1];
    assert_eq!(evt.role, "user");
    assert_eq!(evt.seq, 1);
    let meta = evt.metadata.as_ref().expect("metadata present");
    assert_eq!(meta["kind"], "worktree_event");
    assert_eq!(meta["event"], "attached");
    let blocks: Vec<ContentBlock> = serde_json::from_value(evt.content.clone()).unwrap();
    assert_eq!(blocks.len(), 1);
    if let ContentBlock::Text { text, .. } = &blocks[0] {
        assert!(text.contains("[worktree event]"));
        assert!(text.contains("/data/wt"));
    } else {
        panic!("expected text block");
    }
}

#[tokio::test]
async fn insert_system_event_seq_increments() {
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
    insert_system_event(&pool, &s.id, "first", "attached")
        .await
        .unwrap();
    insert_system_event(&pool, &s.id, "second", "detached")
        .await
        .unwrap();
    let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
    assert_eq!(loaded.messages.len(), 2);
    assert_eq!(loaded.messages[0].seq, 0);
    assert_eq!(loaded.messages[1].seq, 1);
}
