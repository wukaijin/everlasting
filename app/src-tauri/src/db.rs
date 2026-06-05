//! SQLite persistence for sessions, messages, and projects.
//!
//! Tables:
//! - `projects`: one row per registered directory (the user's "work
//!   environment"); sessions are scoped to a project.
//! - `sessions`: one row per conversation, scoped to a project; tracks
//!   title/timestamps/model and the current working directory the
//!   agent is in.
//! - `messages`: one row per message, `content` is JSON-serialized
//!   `Vec<ContentBlock>` so tool_use/tool_result/thinking round-trips
//!   losslessly.
//!
//! Schema is created idempotently by [`run_migrations`], so re-running
//! the app or upgrading doesn't error out on existing tables. New
//! columns for `sessions` (step 3b-1: `project_id`, `current_cwd`) are
//! added via non-destructive `ALTER TABLE ... ADD COLUMN` so the
//! upgrade from step 3a preserves every existing row.
//!
//! The Auto-default project (id = `__default__`) backstops legacy
//! sessions: any pre-3b-1 row gets `project_id = '__default__'`
//! during the migration, and the user can later reassign it via
//! sqlite or a future "Manage projects" panel. See
//! `docs/PROPOSAL-project-binding-and-top-tabs.md` §3.4.

use chrono::Utc;
use serde::Serialize;
use sqlx::{Row, SqlitePool};
use std::path::Path;
use uuid::Uuid;

use crate::llm::types::{MessageContent, Role};
use crate::projects::ProjectRow;
use crate::projects::DEFAULT_PROJECT_ID;

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
    pub project_id: String,
    pub current_cwd: String,
}

/// Summary used by `list_sessions` — includes a preview of the most recent
/// user message so the sidebar can show context without re-loading.
#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub updated_at: String,
    pub preview: String,
    pub project_id: String,
    pub current_cwd: String,
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

/// Open (or create) the SQLite file at `db_path` and return a connection
/// pool. `db_path` is typically `<app_data_dir>/everlasting.db`. Creates
/// the parent directory if missing.
///
/// **PRAGMA foreign_keys = ON** is set per-connection on the first
/// `execute` so the `messages` → `sessions` CASCADE actually fires.
pub async fn init_pool(db_path: &Path) -> Result<SqlitePool, sqlx::Error> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            sqlx::Error::Configuration(
                format!(
                    "failed to create db parent dir {}: {}",
                    parent.display(),
                    e
                )
                .into(),
            )
        })?;
    }

    let url = format!("sqlite://{}?mode=rwc", db_path.display());
    tracing::info!(db_path = %db_path.display(), "opening sqlite pool");
    let pool = SqlitePool::connect(&url).await?;

    // PRAGMA must be issued per-connection. sqlx's `connect` lazily
    // opens connections, so we set this once on every connection in
    // the pool by issuing a one-shot query. The pragma is idempotent
    // and a no-op on already-configured connections.
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await?;
    Ok(pool)
}

/// Create the schema if it doesn't already exist, then run the step
/// 3b-1 ALTERs that add `project_id` / `current_cwd` to `sessions`.
/// Idempotent — safe to call on every startup.
pub async fn run_migrations(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    // --- projects (new in 3b-1) ---
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS projects (
            id           TEXT PRIMARY KEY,
            name         TEXT NOT NULL,
            path         TEXT NOT NULL,
            is_git_repo  INTEGER NOT NULL DEFAULT 0,
            is_legacy    INTEGER NOT NULL DEFAULT 0,
            created_at   TEXT NOT NULL,
            updated_at   TEXT NOT NULL,
            hidden       INTEGER NOT NULL DEFAULT 0,
            metadata     TEXT
        )
        "#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
        CREATE UNIQUE INDEX IF NOT EXISTS idx_projects_path
        ON projects(path)
        "#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
        CREATE INDEX IF NOT EXISTS idx_projects_updated_at
        ON projects(updated_at DESC)
        "#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
        CREATE INDEX IF NOT EXISTS idx_projects_hidden
        ON projects(hidden, updated_at DESC)
        "#,
    )
    .execute(pool)
    .await?;

    // --- sessions (unchanged shape; existing dbs may not have the
    //  3b-1 columns yet, so we add them lazily below) ---
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

    // --- 3b-1 ALTERs: add project_id / current_cwd to sessions.
    //  We probe for column existence first so the migration is
    //  idempotent across a fresh DB and an upgraded DB. ---
    add_session_column_if_missing(pool, "current_cwd", "TEXT NOT NULL DEFAULT ''").await?;
    add_session_column_if_missing(
        pool,
        "project_id",
        &format!("TEXT NOT NULL DEFAULT '{}'", DEFAULT_PROJECT_ID),
    )
    .await?;
    sqlx::query(
        r#"
        CREATE INDEX IF NOT EXISTS idx_sessions_project_id
        ON sessions(project_id)
        "#,
    )
    .execute(pool)
    .await?;

    // --- messages ---
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

    // --- Auto-default project (backstop for legacy sessions) ---
    // Insert the backstop row *after* the ALTERs so any sessions
    // created in this same migration (none in normal flow) can FK
    // against it. For pre-3b-1 sessions, the ALTER DEFAULT
    // `'__default__'` already wires them up.
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        INSERT OR IGNORE INTO projects
            (id, name, path, is_git_repo, is_legacy, created_at, updated_at, hidden, metadata)
        VALUES (?, ?, ?, 0, 1, ?, ?, 0, NULL)
        "#,
    )
    .bind(DEFAULT_PROJECT_ID)
    .bind("Legacy / 未分类")
    // path is $HOME at the OS level; canonicalized here so the
    // "not a git repo" field is conservative. The user can later
    // reassign the legacy sessions to their real project.
    .bind(home_dir_or_dot())
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;

    // For any session whose `current_cwd` is still empty (the
    // pre-3b-1 default we just added), backfill with the backstop
    // project's path so the agent's first turn doesn't try to
    // execute with an empty cwd.
    sqlx::query(
        r#"
        UPDATE sessions
        SET current_cwd = (SELECT path FROM projects WHERE id = ?)
        WHERE current_cwd = '' OR current_cwd IS NULL
        "#,
    )
    .bind(DEFAULT_PROJECT_ID)
    .execute(pool)
    .await?;

    Ok(())
}

/// Add a column to `sessions` if it doesn't already exist. SQLite
/// doesn't have `ALTER TABLE ... ADD COLUMN IF NOT EXISTS` in 3.35
/// reliably (and the underlying error code is `1` for "duplicate
/// column"), so we probe `PRAGMA table_info` first.
async fn add_session_column_if_missing(
    pool: &SqlitePool,
    column: &str,
    decl: &str,
) -> Result<(), sqlx::Error> {
    let exists: i64 =
        sqlx::query("SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = ?")
            .bind(column)
            .fetch_one(pool)
            .await?
            .try_get(0)?;
    if exists == 0 {
        let stmt = format!("ALTER TABLE sessions ADD COLUMN {} {}", column, decl);
        sqlx::query(&stmt).execute(pool).await?;
    }
    Ok(())
}

/// `std::env::home_dir` was removed; this is the cross-platform
/// fallback. If the env vars are unset we fall back to "." so the
/// legacy row has *some* path (it'll be wrong, but the row will
/// exist; the user is expected to reassign or hide it).
fn home_dir_or_dot() -> String {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".to_string())
}

// ---------------------------------------------------------------------------
// Project CRUD
// ---------------------------------------------------------------------------

/// Insert a new project row. Returns the inserted row.
pub async fn create_project(
    pool: &SqlitePool,
    name: &str,
    path: &str,
    is_git_repo: bool,
) -> Result<ProjectRow, sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    let id = Uuid::new_v4().to_string();

    let res = sqlx::query(
        r#"
        INSERT INTO projects
            (id, name, path, is_git_repo, is_legacy, created_at, updated_at, hidden, metadata)
        VALUES (?, ?, ?, ?, 0, ?, ?, 0, NULL)
        "#,
    )
    .bind(&id)
    .bind(name)
    .bind(path)
    .bind(is_git_repo as i64)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await;

    match res {
        Ok(_) => Ok(ProjectRow {
            id,
            name: name.to_string(),
            path: path.to_string(),
            is_git_repo,
            is_legacy: false,
            created_at: now.clone(),
            updated_at: now,
            hidden: false,
            metadata: None,
        }),
        Err(sqlx::Error::Database(db)) if db.is_unique_violation() => Err(sqlx::Error::Protocol(
            format!("a project with path '{}' already exists", path),
        )),
        Err(e) => Err(e),
    }
}

/// List projects. `include_hidden=false` returns only visible tabs
/// (the default for the main Tab bar); `include_hidden=true` is used
/// by the empty-state "recently hidden" list. Sorted by `created_at`
/// ASC so the Tab bar reads chronologically (oldest = leftmost).
pub async fn list_projects(
    pool: &SqlitePool,
    include_hidden: bool,
) -> Result<Vec<ProjectRow>, sqlx::Error> {
    let rows = if include_hidden {
        sqlx::query(
            r#"
            SELECT id, name, path, is_git_repo, is_legacy, created_at, updated_at, hidden, metadata
            FROM projects
            ORDER BY created_at ASC
            "#,
        )
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query(
            r#"
            SELECT id, name, path, is_git_repo, is_legacy, created_at, updated_at, hidden, metadata
            FROM projects
            WHERE hidden = 0
            ORDER BY created_at ASC
            "#,
        )
        .fetch_all(pool)
        .await?
    };

    rows.into_iter().map(row_to_project).collect()
}

/// List hidden projects for the empty-state "recently hidden" panel,
/// sorted by `updated_at DESC` (most-recently-hidden first).
pub async fn list_hidden_projects(pool: &SqlitePool) -> Result<Vec<ProjectRow>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT id, name, path, is_git_repo, is_legacy, created_at, updated_at, hidden, metadata
        FROM projects
        WHERE hidden = 1
        ORDER BY updated_at DESC
        "#,
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(row_to_project).collect()
}

/// Get a single project by id.
pub async fn get_project(
    pool: &SqlitePool,
    project_id: &str,
) -> Result<Option<ProjectRow>, sqlx::Error> {
    let row = sqlx::query(
        r#"
        SELECT id, name, path, is_git_repo, is_legacy, created_at, updated_at, hidden, metadata
        FROM projects
        WHERE id = ?
        "#,
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await?;
    row.map(row_to_project).transpose()
}

/// Change a project's `path` (re-probing is_git_repo is the caller's
/// responsibility — see `projects::store::update_project_path`).
pub async fn update_project_path(
    pool: &SqlitePool,
    project_id: &str,
    new_path: &str,
    is_git_repo: bool,
) -> Result<ProjectRow, sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    let res = sqlx::query(
        r#"
        UPDATE projects
        SET path = ?, is_git_repo = ?, updated_at = ?
        WHERE id = ?
        "#,
    )
    .bind(new_path)
    .bind(is_git_repo as i64)
    .bind(&now)
    .bind(project_id)
    .execute(pool)
    .await;
    match res {
        Ok(r) if r.rows_affected() == 0 => Err(sqlx::Error::RowNotFound),
        Ok(_) => get_project(pool, project_id)
            .await
            .and_then(|opt| opt.ok_or(sqlx::Error::RowNotFound)),
        Err(sqlx::Error::Database(db)) if db.is_unique_violation() => Err(sqlx::Error::Protocol(
            format!("a project with path '{}' already exists", new_path),
        )),
        Err(e) => Err(e),
    }
}

/// Change a project's `name`.
pub async fn update_project_name(
    pool: &SqlitePool,
    project_id: &str,
    new_name: &str,
) -> Result<ProjectRow, sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    let res = sqlx::query(
        r#"
        UPDATE projects
        SET name = ?, updated_at = ?
        WHERE id = ?
        "#,
    )
    .bind(new_name)
    .bind(&now)
    .bind(project_id)
    .execute(pool)
    .await;
    match res {
        Ok(r) if r.rows_affected() == 0 => Err(sqlx::Error::RowNotFound),
        Ok(_) => get_project(pool, project_id)
            .await
            .and_then(|opt| opt.ok_or(sqlx::Error::RowNotFound)),
        Err(e) => Err(e),
    }
}

/// Hide a project (× close-tab). Data is preserved. Hidden projects
/// do not show in the Tab bar but remain available via
/// [`list_hidden_projects`].
pub async fn hide_project(pool: &SqlitePool, project_id: &str) -> Result<(), sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    sqlx::query("UPDATE projects SET hidden = 1, updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(project_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Reverse a [`hide_project`].
pub async fn unhide_project(pool: &SqlitePool, project_id: &str) -> Result<(), sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    sqlx::query("UPDATE projects SET hidden = 0, updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(project_id)
        .execute(pool)
        .await?;
    Ok(())
}

fn row_to_project(r: sqlx::sqlite::SqliteRow) -> Result<ProjectRow, sqlx::Error> {
    Ok(ProjectRow {
        id: r.try_get("id")?,
        name: r.try_get("name")?,
        path: r.try_get("path")?,
        is_git_repo: r.try_get::<i64, _>("is_git_repo")? != 0,
        is_legacy: r.try_get::<i64, _>("is_legacy")? != 0,
        created_at: r.try_get("created_at")?,
        updated_at: r.try_get("updated_at")?,
        hidden: r.try_get::<i64, _>("hidden")? != 0,
        metadata: r.try_get("metadata")?,
    })
}

// ---------------------------------------------------------------------------
// Session CRUD
// ---------------------------------------------------------------------------

/// Create a new empty session under `project_id` with the given
/// initial working directory. Returns the new session's row.
pub async fn create_session(
    pool: &SqlitePool,
    project_id: &str,
    initial_cwd: &str,
    model: &str,
) -> Result<SessionRow, sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    let id = Uuid::new_v4().to_string();
    let title = "新对话".to_string();

    sqlx::query(
        r#"
        INSERT INTO sessions
            (id, title, created_at, updated_at, model, metadata, project_id, current_cwd)
        VALUES (?, ?, ?, ?, ?, NULL, ?, ?)
        "#,
    )
    .bind(&id)
    .bind(&title)
    .bind(&now)
    .bind(&now)
    .bind(model)
    .bind(project_id)
    .bind(initial_cwd)
    .execute(pool)
    .await?;

    Ok(SessionRow {
        id,
        title,
        created_at: now.clone(),
        updated_at: now,
        model: model.to_string(),
        project_id: project_id.to_string(),
        current_cwd: initial_cwd.to_string(),
    })
}

/// List all sessions belonging to `project_id`, newest updated first.
/// Includes a preview of the most recent user message in each session.
pub async fn list_sessions(
    pool: &SqlitePool,
    project_id: &str,
) -> Result<Vec<SessionSummary>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT s.id, s.title, s.updated_at, s.project_id, s.current_cwd,
               COALESCE(
                   (SELECT text FROM messages m
                    WHERE m.session_id = s.id AND m.role = 'user'
                    ORDER BY m.seq DESC LIMIT 1),
                   ''
               ) AS preview
        FROM sessions s
        WHERE s.project_id = ?
        ORDER BY s.updated_at DESC
        "#,
    )
    .bind(project_id)
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
                project_id: r.try_get("project_id")?,
                current_cwd: r.try_get("current_cwd")?,
            })
        })
        .collect()
}

/// Load a session and all its messages. Returns `None` if the session
/// doesn't exist.
pub async fn load_session(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Option<LoadedSession>, sqlx::Error> {
    let session_row = sqlx::query(
        r#"
        SELECT id, title, created_at, updated_at, model, project_id, current_cwd
        FROM sessions
        WHERE id = ?
        "#,
    )
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
            project_id: r.try_get("project_id")?,
            current_cwd: r.try_get("current_cwd")?,
        },
        None => return Ok(None),
    };

    let msg_rows = sqlx::query(
        r#"
        SELECT id, session_id, role, content, text, has_tool_calls, has_tool_results, created_at, seq
        FROM messages
        WHERE session_id = ?
        ORDER BY seq ASC
        "#,
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;

    let messages = msg_rows
        .into_iter()
        .map(|r| {
            let content_str: String = r.try_get("content")?;
            let content: serde_json::Value = serde_json::from_str(&content_str).map_err(|e| {
                sqlx::Error::Decode(format!("bad message content JSON: {}", e).into())
            })?;
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

/// Delete a session. Messages are removed via FK CASCADE — but we
/// also issue an explicit `DELETE FROM messages` so the behavior is
/// correct on databases where `PRAGMA foreign_keys` was not set when
/// the row was created.
pub async fn delete_session(pool: &SqlitePool, session_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM messages WHERE session_id = ?")
        .bind(session_id)
        .execute(pool)
        .await?;
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

/// Persist the new `current_cwd` for a session. Called by the agent
/// loop at the **end of a turn** (not after every shell tool call —
/// see `docs/PROPOSAL-project-binding-and-top-tabs.md` §4.4 / §11
/// "turn 结束一次性写").
pub async fn update_session_cwd(
    pool: &SqlitePool,
    session_id: &str,
    new_cwd: &str,
) -> Result<(), sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        UPDATE sessions
        SET current_cwd = ?, updated_at = ?
        WHERE id = ?
        "#,
    )
    .bind(new_cwd)
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
        r#"
        INSERT INTO messages
           (session_id, role, content, text, has_tool_calls, has_tool_results, created_at, seq)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#,
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
            r#"
            UPDATE sessions
            SET title = CASE
                WHEN title = '新对话' AND ? != '' THEN substr(?, 1, 50)
                ELSE title
            END
            WHERE id = ?
            "#,
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
        // Mirror what `init_pool` does.
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
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
    async fn default_project_is_seeded() {
        let pool = test_pool().await;
        let projects = list_projects(&pool, true).await.unwrap();
        let backstop = projects
            .iter()
            .find(|p| p.id == DEFAULT_PROJECT_ID)
            .expect("default project should be seeded");
        assert!(backstop.is_legacy);
        assert_eq!(backstop.name, "Legacy / 未分类");
        assert!(!backstop.hidden);
    }

    #[tokio::test]
    async fn create_and_list_project() {
        let pool = test_pool().await;
        let dir = std::env::temp_dir().join("everlasting_test_create_proj");
        let _ = std::fs::create_dir_all(&dir);
        let path_str = dir.to_str().unwrap();

        let p1 = create_project(&pool, "a", path_str, false).await.unwrap();
        // Duplicate path → unique violation → Err.
        let dup = create_project(&pool, "b", path_str, false).await;
        assert!(dup.is_err(), "duplicate path should fail");

        let p2 = create_project(&pool, "c", "/tmp/everlasting_test_other", true)
            .await
            .unwrap();
        let list = list_projects(&pool, false).await.unwrap();
        let ids: Vec<&str> = list.iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&p1.id.as_str()));
        assert!(ids.contains(&p2.id.as_str()));
        assert!(ids.contains(&DEFAULT_PROJECT_ID));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn hide_and_unhide_project() {
        let pool = test_pool().await;
        let p = create_project(&pool, "x", "/tmp/everlasting_test_hide", false)
            .await
            .unwrap();

        hide_project(&pool, &p.id).await.unwrap();
        let visible = list_projects(&pool, false).await.unwrap();
        assert!(!visible.iter().any(|q| q.id == p.id));
        let hidden = list_hidden_projects(&pool).await.unwrap();
        assert!(hidden.iter().any(|q| q.id == p.id));

        unhide_project(&pool, &p.id).await.unwrap();
        let visible = list_projects(&pool, false).await.unwrap();
        assert!(visible.iter().any(|q| q.id == p.id));
    }

    #[tokio::test]
    async fn update_project_name_works() {
        let pool = test_pool().await;
        let p = create_project(&pool, "old", "/tmp/everlasting_test_rename", false)
            .await
            .unwrap();
        let p2 = update_project_name(&pool, &p.id, "new").await.unwrap();
        assert_eq!(p2.name, "new");
        // updated_at should have advanced.
        assert_ne!(p2.updated_at, p.updated_at);
    }

    #[tokio::test]
    async fn update_project_path_reprobes_git_flag() {
        let pool = test_pool().await;
        let p = create_project(&pool, "p", "/tmp/everlasting_test_repath", false)
            .await
            .unwrap();
        assert!(!p.is_git_repo);

        let p2 = update_project_path(&pool, &p.id, "/tmp/everlasting_test_repath2", true)
            .await
            .unwrap();
        assert!(p2.is_git_repo);
        assert_eq!(p2.path, "/tmp/everlasting_test_repath2");
    }

    #[tokio::test]
    async fn create_session_scopes_to_project() {
        let pool = test_pool().await;
        let p = create_project(&pool, "p", "/tmp/everlasting_test_session_proj", false)
            .await
            .unwrap();

        let s1 = create_session(&pool, &p.id, "/tmp/foo", "GLM-4.7")
            .await
            .unwrap();
        let s2 = create_session(&pool, &p.id, "/tmp/bar", "GLM-4.7")
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

    #[tokio::test]
    async fn persist_and_load_messages() {
        let pool = test_pool().await;
        let session = create_session(&pool, DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7")
            .await
            .unwrap();

        let user_msg = MessageContent::Text("read the file".to_string());
        persist_turn(&pool, &session.id, Role::User, &user_msg, 0)
            .await
            .unwrap();

        let assistant_blocks = vec![
            ContentBlock::Text {
                text: "OK reading".to_string(),
            },
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
        let session = create_session(&pool, DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7")
            .await
            .unwrap();

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
        let session = create_session(&pool, DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7")
            .await
            .unwrap();

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
        let session = create_session(&pool, DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7")
            .await
            .unwrap();
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
        let session = create_session(&pool, DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7")
            .await
            .unwrap();
        let long = "a".repeat(120);
        persist_turn(&pool, &session.id, Role::User, &MessageContent::Text(long), 0)
            .await
            .unwrap();

        let list = list_sessions(&pool, DEFAULT_PROJECT_ID).await.unwrap();
        assert!(list[0].preview.starts_with("a".repeat(80).as_str()));
        assert!(list[0].preview.ends_with('…'));
    }

    #[tokio::test]
    async fn touch_session_updates_timestamp() {
        let pool = test_pool().await;
        let session = create_session(&pool, DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7")
            .await
            .unwrap();
        let original = session.updated_at.clone();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        touch_session(&pool, &session.id).await.unwrap();
        let reloaded = load_session(&pool, &session.id).await.unwrap().unwrap();
        assert_ne!(reloaded.session.updated_at, original);
    }

    #[tokio::test]
    async fn update_session_cwd_persists() {
        let pool = test_pool().await;
        let session = create_session(&pool, DEFAULT_PROJECT_ID, "/tmp/start", "GLM-4.7")
            .await
            .unwrap();
        assert_eq!(session.current_cwd, "/tmp/start");

        update_session_cwd(&pool, &session.id, "/tmp/end").await.unwrap();
        let reloaded = load_session(&pool, &session.id).await.unwrap().unwrap();
        assert_eq!(reloaded.session.current_cwd, "/tmp/end");
    }
}
