#![cfg(test)]

use uuid::Uuid;

use crate::llm::types::{ContentBlock, MessageContent, Role};
use crate::projects::DEFAULT_PROJECT_ID;

use super::projects::create_project;
use super::sessions::{
    create_session, delete_messages_by_session, delete_session, list_sessions, load_session,
    persist_turn,
};
use super::test_pool;

#[tokio::test]
async fn create_session_scopes_to_project() {
    let pool = test_pool().await;
    let p = create_project(
        &pool,
        "p",
        "/tmp/everlasting_test_session_proj",
        false,
        None,
    )
    .await
    .unwrap();

    let s1 = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        &p.id,
        "/tmp/foo",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let s2 = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        &p.id,
        "/tmp/bar",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(s1.project_id, p.id);
    assert_eq!(s1.current_cwd, "/tmp/foo");
    assert_eq!(s2.current_cwd, "/tmp/bar");

    let list = list_sessions(&pool, &p.id).await.unwrap();
    assert_eq!(list.len(), 2);
    // Cross-project isolation: legacy project's sessions are not
    // in this list.
    let legacy = list_sessions(&pool, DEFAULT_PROJECT_ID).await.unwrap();
    assert_eq!(legacy.len(), 0);
}

#[tokio::test]
async fn load_session_returns_none_for_missing() {
    let pool = test_pool().await;
    let result = load_session(&pool, "nonexistent").await.unwrap();
    assert!(result.is_none());
}

/// 2026-08-18 (5df29977 问题4) regression: the INSERT must persist
/// `mode='edit'`, agreeing with the returned struct's `Mode::Edit`.
/// b999803 accidentally wrote the legacy `'chat'` into the mode slot
/// (confused with session_type's DEFAULT 'chat'); the every-init
/// `chat→edit` scrub migration masked the disagreement, so only
/// sessions created after the last process start kept the bad value.
#[tokio::test]
async fn create_session_persists_edit_mode() {
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
    assert_eq!(s.mode, crate::db::Mode::Edit, "struct field is Edit");
    // The persisted row must agree — pre-fix the struct said Edit
    // while the row held the legacy 'chat'.
    let row: (String,) = sqlx::query_as("SELECT mode FROM sessions WHERE id = ?")
        .bind(&s.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.0, "edit", "INSERT must persist edit, not legacy chat");
}

#[tokio::test]
async fn persist_and_load_messages() {
    let pool = test_pool().await;
    let session = create_session(
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

    let user_msg = MessageContent::Text("read the file".to_string());
    persist_turn(&pool, &session.id, Role::User, &user_msg, 0, None, None)
        .await
        .unwrap();

    let assistant_blocks = vec![
        ContentBlock::Text {
            text: "OK reading".to_string(),
            cache_control: None,
        },
        ContentBlock::ToolUse {
            id: "toolu_abc".to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({"path": "/etc/hostname"}),
        },
    ];
    let assistant_msg = MessageContent::Blocks(assistant_blocks);
    persist_turn(
        &pool,
        &session.id,
        Role::Assistant,
        &assistant_msg,
        1,
        None,
        None,
    )
    .await
    .unwrap();

    let loaded = load_session(&pool, &session.id).await.unwrap().unwrap();
    assert_eq!(loaded.messages.len(), 2);
    assert_eq!(loaded.messages[0].seq, 0);
    assert_eq!(loaded.messages[0].text, "read the file");
    assert_eq!(loaded.messages[1].seq, 1);
    assert!(loaded.messages[1].has_tool_calls);
    assert!(!loaded.messages[1].has_tool_results);

    let blocks: Vec<ContentBlock> =
        serde_json::from_value(loaded.messages[1].content.clone()).unwrap();
    assert_eq!(blocks.len(), 2);
    assert!(matches!(&blocks[1], ContentBlock::ToolUse { name, .. } if name == "read_file"));
}

#[tokio::test]
async fn first_user_message_auto_titles_session() {
    let pool = test_pool().await;
    let session = create_session(
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

    let msg = MessageContent::Text("帮我读一下 /etc/hostname".to_string());
    persist_turn(&pool, &session.id, Role::User, &msg, 0, None, None)
        .await
        .unwrap();

    let updated = load_session(&pool, &session.id).await.unwrap().unwrap();
    assert_eq!(updated.session.title, "帮我读一下 /etc/hostname");
}

#[tokio::test]
async fn second_user_message_does_not_overwrite_title() {
    let pool = test_pool().await;
    let session = create_session(
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
        &session.id,
        Role::User,
        &MessageContent::Text("first".into()),
        0,
        None,
        None,
    )
    .await
    .unwrap();
    persist_turn(
        &pool,
        &session.id,
        Role::User,
        &MessageContent::Text("second".into()),
        1,
        None,
        None,
    )
    .await
    .unwrap();

    let loaded = load_session(&pool, &session.id).await.unwrap().unwrap();
    assert_eq!(loaded.session.title, "first");
}

#[tokio::test]
async fn delete_session_cascades_messages() {
    let pool = test_pool().await;
    let session = create_session(
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
        &session.id,
        Role::User,
        &MessageContent::Text("hi".into()),
        0,
        None,
        None,
    )
    .await
    .unwrap();

    delete_session(&pool, &session.id).await.unwrap();
    assert!(load_session(&pool, &session.id).await.unwrap().is_none());
}

#[tokio::test]
async fn delete_messages_by_session_keeps_session_drops_messages() {
    let pool = test_pool().await;
    let session = create_session(
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
        &session.id,
        Role::User,
        &MessageContent::Text("hi".into()),
        0,
        None,
        None,
    )
    .await
    .unwrap();

    // Sanity: the message was persisted.
    let before = load_session(&pool, &session.id).await.unwrap().unwrap();
    assert_eq!(before.messages.len(), 1);

    // B3 /clear: messages gone, session row + metadata survive.
    delete_messages_by_session(&pool, &session.id)
        .await
        .unwrap();
    let after = load_session(&pool, &session.id).await.unwrap().unwrap();
    assert!(after.messages.is_empty(), "messages should be cleared");
    assert_eq!(
        after.session.id, session.id,
        "session row must survive /clear"
    );
    assert_eq!(
        after.session.title, before.session.title,
        "metadata preserved"
    );
}

#[tokio::test]
async fn list_sessions_preview_truncates_at_80_chars() {
    let pool = test_pool().await;
    let session = create_session(
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
    let long = "a".repeat(120);
    persist_turn(
        &pool,
        &session.id,
        Role::User,
        &MessageContent::Text(long),
        0,
        None,
        None,
    )
    .await
    .unwrap();

    let list = list_sessions(&pool, DEFAULT_PROJECT_ID).await.unwrap();
    assert!(list[0].preview.starts_with("a".repeat(80).as_str()));
    assert!(list[0].preview.ends_with('…'));
}
