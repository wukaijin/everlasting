//! SQLite persistence for sessions and messages.
//!
//! Two tables:
//! - `sessions`: one row per conversation, tracks title/timestamps/model
//! - `messages`: one row per message, `content` is JSON-serialized
//!   `Vec<ContentBlock>` so tool_use/tool_result round-trips losslessly.
//!
//! Schema is created idempotently by [`run_migrations`], so re-running the
//! app or upgrading doesn't error out on existing tables.

use chrono::Utc;
use serde::Serialize;
use sqlx::{Row, SqlitePool};
use std::path::Path;
use uuid::Uuid;

use crate::llm::types::{MessageContent, Role};

// ---------------------------------------------------------------------------
// Row types (Serialize for Tauri IPC payload)
// ---------------------------------------------------------------------------

/// A session as stored in the DB.
#[derive(Debug, Clone, Serialize)]
pub struct SessionRow {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub model: String,
}

/// Summary used by `list_sessions` — includes a preview of the most recent
/// user message so the sidebar can show context without re-loading.
#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub updated_at: String,
    pub preview: String,
}

/// A message as stored in the DB. `content` is JSON (`Vec<ContentBlock>`).
#[derive(Debug, Clone, Serialize)]
pub struct MessageRow {
    pub id: i64,
    pub session_id: String,
    pub role: String,
    pub content: serde_json::Value,
    pub text: String,
    pub has_tool_calls: bool,
    pub has_tool_results: bool,
    pub created_at: String,
    pub seq: i64,
}

/// Result of `load_session` — session meta + all messages ordered by `seq`.
#[derive(Debug, Clone, Serialize)]
pub struct LoadedSession {
    pub session: SessionRow,
    pub messages: Vec<MessageRow>,
}

// ---------------------------------------------------------------------------
// Pool + migrations
// ---------------------------------------------------------------------------

/// Open (or create) the SQLite file at `db_path` and return a connection pool.
///
/// `db_path` is typically `<app_data_dir>/everlasting.db`.
/// Creates the parent directory if missing.
pub async fn init_pool(db_path: &Path) -> Result<SqlitePool, sqlx::Error> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            sqlx::Error::Configuration(format!(
                "failed to create db parent dir {}: {}",
                parent.display(),
                e
            )
            .into())
        })?;
    }

    let url = format!("sqlite://{}?mode=rwc", db_path.display());
    tracing::info!(db_path = %db_path.display(), "opening sqlite pool");
    SqlitePool::connect(&url).await
}

/// Create the schema if it doesn't already exist. Idempotent.
pub async fn run_migrations(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS sessions (
            id          TEXT PRIMARY KEY,
            title       TEXT NOT NULL,
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL,
            model       TEXT NOT NULL,
            metadata    TEXT
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE INDEX IF NOT EXISTS idx_sessions_updated_at
        ON sessions(updated_at DESC)
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS messages (
            id               INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id       TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
            role             TEXT NOT NULL,
            content          TEXT NOT NULL,
            text             TEXT NOT NULL,
            has_tool_calls   INTEGER NOT NULL DEFAULT 0,
            has_tool_results INTEGER NOT NULL DEFAULT 0,
            created_at       TEXT NOT NULL,
            seq              INTEGER NOT NULL,
            UNIQUE(session_id, seq)
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE INDEX IF NOT EXISTS idx_messages_session_seq
        ON messages(session_id, seq)
        "#,
    )
    .execute(pool)
    .await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Session CRUD
// ---------------------------------------------------------------------------

/// Create a new empty session. Returns the new session's row.
pub async fn create_session(pool: &SqlitePool, model: &str) -> Result<SessionRow, sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    let id = Uuid::new_v4().to_string();
    let title = "新对话".to_string();

    sqlx::query(
        r#"INSERT INTO sessions (id, title, created_at, updated_at, model, metadata)
           VALUES (?, ?, ?, ?, ?, NULL)"#,
    )
    .bind(&id)
    .bind(&title)
    .bind(&now)
    .bind(&now)
    .bind(model)
    .execute(pool)
    .await?;

    Ok(SessionRow {
        id,
        title,
        created_at: now.clone(),
        updated_at: now,
        model: model.to_string(),
    })
}

/// List all sessions, newest updated first. Includes a preview of the most
/// recent user message in each session.
pub async fn list_sessions(pool: &SqlitePool) -> Result<Vec<SessionSummary>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT s.id, s.title, s.updated_at,
               COALESCE(
                   (SELECT text FROM messages m
                    WHERE m.session_id = s.id AND m.role = 'user'
                    ORDER BY m.seq DESC LIMIT 1),
                   ''
               ) AS preview
        FROM sessions s
        ORDER BY s.updated_at DESC
        "#,
    )
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|r| {
            let preview: String = r.try_get("preview")?;
            let preview = if preview.chars().count() > 80 {
                let truncated: String = preview.chars().take(80).collect();
                format!("{}…", truncated)
            } else {
                preview
            };
            Ok(SessionSummary {
                id: r.try_get("id")?,
                title: r.try_get("title")?,
                updated_at: r.try_get("updated_at")?,
                preview,
            })
        })
        .collect()
}

/// Load a session and all its messages. Returns None if session doesn't exist.
pub async fn load_session(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Option<LoadedSession>, sqlx::Error> {
    let session_row = sqlx::query("SELECT id, title, created_at, updated_at, model FROM sessions WHERE id = ?")
        .bind(session_id)
        .fetch_optional(pool)
        .await?;

    let session = match session_row {
        Some(r) => SessionRow {
            id: r.try_get("id")?,
            title: r.try_get("title")?,
            created_at: r.try_get("created_at")?,
            updated_at: r.try_get("updated_at")?,
            model: r.try_get("model")?,
        },
        None => return Ok(None),
    };

    let msg_rows = sqlx::query(
        r#"SELECT id, session_id, role, content, text, has_tool_calls, has_tool_results, created_at, seq
           FROM messages WHERE session_id = ? ORDER BY seq ASC"#,
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;

    let messages = msg_rows
        .into_iter()
        .map(|r| {
            let content_str: String = r.try_get("content")?;
            let content: serde_json::Value = serde_json::from_str(&content_str)
                .map_err(|e| sqlx::Error::Decode(format!("bad message content JSON: {}", e).into()))?;
            Ok(MessageRow {
                id: r.try_get("id")?,
                session_id: r.try_get("session_id")?,
                role: r.try_get("role")?,
                content,
                text: r.try_get("text")?,
                has_tool_calls: r.try_get::<i64, _>("has_tool_calls")? != 0,
                has_tool_results: r.try_get::<i64, _>("has_tool_results")? != 0,
                created_at: r.try_get("created_at")?,
                seq: r.try_get("seq")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;

    Ok(Some(LoadedSession { session, messages }))
}

/// Delete a session and (via CASCADE) all its messages.
pub async fn delete_session(pool: &SqlitePool, session_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sessions WHERE id = ?")
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Bump the session's `updated_at` to now. Called at the end of a turn.
pub async fn touch_session(pool: &SqlitePool, session_id: &str) -> Result<(), sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    sqlx::query("UPDATE sessions SET updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Message persistence
// ---------------------------------------------------------------------------

/// Persist one message (assistant turn or tool_result turn). The `seq` is
/// caller-managed and must be strictly increasing within a session.
///
/// If the message is a user message and the session title is still the
/// default "新对话", auto-generate a title from the message text.
pub async fn persist_turn(
    pool: &SqlitePool,
    session_id: &str,
    role: Role,
    content: &MessageContent,
    seq: i64,
) -> Result<(), sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    let role_str = match role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    let content_json = serde_json::to_string(content)
        .map_err(|e| sqlx::Error::Encode(format!("serialize content: {}", e).into()))?;
    let text = content.to_text();
    let has_tool_calls = matches!(content, MessageContent::Blocks(b)
        if b.iter().any(|x| matches!(x, crate::llm::types::ContentBlock::ToolUse { .. })));
    let has_tool_results = matches!(content, MessageContent::Blocks(b)
        if b.iter().any(|x| matches!(x, crate::llm::types::ContentBlock::ToolResult { .. })));

    sqlx::query(
        r#"INSERT INTO messages
           (session_id, role, content, text, has_tool_calls, has_tool_results, created_at, seq)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(session_id)
    .bind(role_str)
    .bind(&content_json)
    .bind(&text)
    .bind(has_tool_calls as i64)
    .bind(has_tool_results as i64)
    .bind(&now)
    .bind(seq)
    .execute(pool)
    .await?;

    // Auto-title from first user message.
    if matches!(role, Role::User) {
        sqlx::query(
            r#"UPDATE sessions
               SET title = CASE
                   WHEN title = '新对话' AND ? != '' THEN substr(?, 1, 50)
                   ELSE title
               END
               WHERE id = ?"#,
        )
        .bind(&text)
        .bind(&text)
        .bind(session_id)
        .execute(pool)
        .await?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::ContentBlock;

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        run_migrations(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn migrations_are_idempotent() {
        let pool = test_pool().await;
        // Running twice should not error.
        run_migrations(&pool).await.unwrap();
    }

    #[tokio::test]
    async fn create_and_list_session() {
        let pool = test_pool().await;
        let s1 = create_session(&pool, "GLM-4.7").await.unwrap();
        let s2 = create_session(&pool, "GLM-4.7").await.unwrap();

        let list = list_sessions(&pool).await.unwrap();
        assert_eq!(list.len(), 2);
        // Most recent first (s2 was created after s1).
        assert_eq!(list[0].id, s2.id);
        assert_eq!(list[1].id, s1.id);
        assert_eq!(list[0].title, "新对话");
    }

    #[tokio::test]
    async fn load_session_returns_none_for_missing() {
        let pool = test_pool().await;
        let result = load_session(&pool, "nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn persist_and_load_messages() {
        let pool = test_pool().await;
        let session = create_session(&pool, "GLM-4.7").await.unwrap();

        // Persist a user message and an assistant turn with tool_use.
        let user_msg = MessageContent::Text("read the file".to_string());
        persist_turn(&pool, &session.id, Role::User, &user_msg, 0)
            .await
            .unwrap();

        let assistant_blocks = vec![
            ContentBlock::Text { text: "OK reading".to_string() },
            ContentBlock::ToolUse {
                id: "toolu_abc".to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({"path": "/etc/hostname"}),
            },
        ];
        let assistant_msg = MessageContent::Blocks(assistant_blocks);
        persist_turn(&pool, &session.id, Role::Assistant, &assistant_msg, 1)
            .await
            .unwrap();

        // Load and verify.
        let loaded = load_session(&pool, &session.id).await.unwrap().unwrap();
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.messages[0].seq, 0);
        assert_eq!(loaded.messages[0].text, "read the file");
        assert_eq!(loaded.messages[1].seq, 1);
        assert!(loaded.messages[1].has_tool_calls);
        assert!(!loaded.messages[1].has_tool_results);

        // Verify the ContentBlock[] round-trips losslessly.
        let blocks: Vec<ContentBlock> = serde_json::from_value(loaded.messages[1].content.clone()).unwrap();
        assert_eq!(blocks.len(), 2);
        assert!(matches!(&blocks[1], ContentBlock::ToolUse { name, .. } if name == "read_file"));
    }

    #[tokio::test]
    async fn first_user_message_auto_titles_session() {
        let pool = test_pool().await;
        let session = create_session(&pool, "GLM-4.7").await.unwrap();

        let msg = MessageContent::Text("帮我读一下 /etc/hostname".to_string());
        persist_turn(&pool, &session.id, Role::User, &msg, 0)
            .await
            .unwrap();

        let updated = load_session(&pool, &session.id).await.unwrap().unwrap();
        assert_eq!(updated.session.title, "帮我读一下 /etc/hostname");
    }

    #[tokio::test]
    async fn second_user_message_does_not_overwrite_title() {
        let pool = test_pool().await;
        let session = create_session(&pool, "GLM-4.7").await.unwrap();

        persist_turn(&pool, &session.id, Role::User, &MessageContent::Text("first".into()), 0)
            .await
            .unwrap();
        persist_turn(&pool, &session.id, Role::User, &MessageContent::Text("second".into()), 1)
            .await
            .unwrap();

        let loaded = load_session(&pool, &session.id).await.unwrap().unwrap();
        assert_eq!(loaded.session.title, "first");
    }

    #[tokio::test]
    async fn delete_session_cascades_messages() {
        let pool = test_pool().await;
        let session = create_session(&pool, "GLM-4.7").await.unwrap();
        persist_turn(
            &pool,
            &session.id,
            Role::User,
            &MessageContent::Text("hi".into()),
            0,
        )
        .await
        .unwrap();

        delete_session(&pool, &session.id).await.unwrap();
        assert!(load_session(&pool, &session.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_sessions_preview_truncates_at_80_chars() {
        let pool = test_pool().await;
        let session = create_session(&pool, "GLM-4.7").await.unwrap();
        let long = "a".repeat(120);
        persist_turn(&pool, &session.id, Role::User, &MessageContent::Text(long), 0)
            .await
            .unwrap();

        let list = list_sessions(&pool).await.unwrap();
        // 80 a's + ellipsis.
        assert!(list[0].preview.starts_with("a".repeat(80).as_str()));
        assert!(list[0].preview.ends_with('…'));
    }

    #[tokio::test]
    async fn touch_session_updates_timestamp() {
        let pool = test_pool().await;
        let session = create_session(&pool, "GLM-4.7").await.unwrap();
        let original = session.updated_at.clone();
        // Sleep a tiny moment to ensure the timestamp differs.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        touch_session(&pool, &session.id).await.unwrap();
        let reloaded = load_session(&pool, &session.id).await.unwrap().unwrap();
        assert_ne!(reloaded.session.updated_at, original);
    }
}
