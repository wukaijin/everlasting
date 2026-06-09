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
//! - `providers` / `models` / `app_config` (PR1 of multi-model task):
//!   user-managed LLM provider catalog. `providers` holds the
//!   connection details (base_url + api_key per protocol); `models`
//!   binds a model name to a provider with capability hints
//!   (`supports_thinking`, `context_window`); `app_config` is a small
//!   key/value store for global settings (currently only
//!   `default_model_id`). `sessions.model_id` is a soft FK to
//!   `models.id` — kept nullable so legacy rows from the pre-PR1 era
//!   (`model TEXT` only) still load; the seed function backfills
//!   `model_id` from `default_model_id` on first run.
//!
//! Schema is created idempotently by [`run_migrations`], so re-running
//! the app or upgrading doesn't error out on existing tables. New
//! columns for `sessions` (step 3b-1: `project_id`, `current_cwd`; step 4:
//! `worktree_path`; PR1 of multi-model: `model_id`) are added via
//! non-destructive `ALTER TABLE ... ADD COLUMN` so the upgrade from
//! any prior step preserves every existing row.
//!
//! The Auto-default project (id = `__default__`) backstops legacy
//! sessions: any pre-3b-1 row gets `project_id = '__default__'`
//! during the migration, and the user can later reassign it via
//! sqlite or a future "Manage projects" panel. See
//! `docs/PROPOSAL-project-binding-and-top-tabs.md` §3.4.

use chrono::Utc;
use serde::{Deserialize, Serialize, Serializer};
use sqlx::{Row, SqlitePool};
use std::path::Path;
use uuid::Uuid;

use crate::llm::types::{MessageContent, Role};
use crate::projects::ProjectRow;
use crate::projects::DEFAULT_PROJECT_ID;

impl Serialize for WorktreeState {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Provider / Model row types (PR1 of multi-model task)
// ---------------------------------------------------------------------------

/// LLM provider protocol. Maps to the wire format the LLM client
/// speaks. PR1 ships `Anthropic` (Messages API) and `Openai` (Chat
/// Completions); future protocols (Ollama, Gemini, …) extend this
/// enum in step with `Provider` impls added under
/// `app/src-tauri/src/llm/provider/`.
///
/// The enum + methods are intentionally unused in PR1 (PR1 only
/// persists `protocol` as a TEXT column); the dispatch in PR2's
/// `Provider` impls will pick them up. `#[allow(dead_code)]` keeps
/// the lib build clean in the meantime.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderProtocol {
    Anthropic,
    Openai,
}

#[allow(dead_code)]
impl ProviderProtocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Openai => "openai",
        }
    }

    /// Lenient parse for DB values. Unknown values fall back to
    /// `Anthropic` so a future schema migration that adds a new
    /// protocol doesn't crash an older binary reading a newer DB.
    pub fn from_str_opt(s: &str) -> Self {
        match s {
            "openai" => Self::Openai,
            _ => Self::Anthropic,
        }
    }
}

/// A user-managed LLM provider entry. Multiple rows may share the
/// same `protocol` (e.g. "Anthropic 官方" + "wukaijin 转发" both
/// `protocol=anthropic`); the `display_name` is the user-facing
/// label that disambiguates them in the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRow {
    pub id: String,
    pub protocol: String,
    pub display_name: String,
    pub base_url: String,
    pub api_key: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A user-managed LLM model entry. Always bound to one
/// `ProviderRow` via `provider_id` (FK with `ON DELETE CASCADE`).
/// Optional fields (`max_tokens`, `thinking_effort`) override the
/// global env defaults; `None` means "fall back to global".
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRow {
    pub id: String,
    pub provider_id: String,
    pub model_name: String,
    pub display_name: String,
    pub max_tokens: Option<u32>,
    pub thinking_effort: Option<String>,
    pub supports_thinking: bool,
    pub context_window: u32,
    pub created_at: String,
    pub updated_at: String,
}

/// `ModelRow` denormalized with the parent provider's `display_name`
/// + `protocol`. The UI renders this view directly (model picker
/// groups models under their provider's display name) so the
/// frontend does not need a second IPC roundtrip to render the
/// dropdown.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelWithProvider {
    #[serde(flatten)]
    pub model: ModelRow,
    pub provider_display_name: String,
    pub provider_protocol: String,
}

// ---------------------------------------------------------------------------
// Row types (Serialize for Tauri IPC payload)
// ---------------------------------------------------------------------------

/// Possible worktree states for a session. The state machine is
/// tri-valued:
///
/// - `None` (DB value `"none"`): the session was never attached
///   to a worktree. `worktree_path` is NULL.
/// - `Active` (DB value `"active"`): a worktree is currently
///   bound to this session. `worktree_path` is non-NULL.
/// - `Detached` (DB value `"detached"`): a worktree WAS attached
///   at some point, but the user has since unbound it. The
///   directory + branch are preserved on disk and
///   `last_worktree_path` records the path that was unbound (for
///   the "上次 worktree" UI affordance — a detached session still
///   has a branch on disk, and the user may want to re-attach to
///   it).
///
/// Migration: a session that was created under step 4 (auto-create
/// flow, before the opt-in refactor) has `worktree_path IS NOT
/// NULL` but `worktree_state IS NULL`. `run_migrations` backfills
/// these to `"active"` so the UI behaves the same as a freshly
/// attached session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeState {
    None,
    Active,
    Detached,
}

impl WorktreeState {
    pub fn as_str(&self) -> &'static str {
        match self {
            WorktreeState::None => "none",
            WorktreeState::Active => "active",
            WorktreeState::Detached => "detached",
        }
    }

    /// Lenient parse for DB values. Unknown values are treated as
    /// `None` so a future schema migration that adds a new state
    /// doesn't crash an older binary reading a newer DB.
    pub fn from_str_opt(s: &str) -> Self {
        match s {
            "active" => WorktreeState::Active,
            "detached" => WorktreeState::Detached,
            _ => WorktreeState::None,
        }
    }
}

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
    /// On-disk path to the session's git worktree. `None` for
    /// sessions that have never been attached (state `none`) or
    /// have been detached (state `detached` — see
    /// `last_worktree_path` for the historical path). Tools fall
    /// back to `current_cwd` when this is `None`.
    pub worktree_path: Option<String>,
    /// Current worktree state (see [`WorktreeState`]).
    pub worktree_state: WorktreeState,
    /// Path of the most recently detached worktree. `None` unless
    /// the session has been in `active` state at some point.
    /// Preserved across detach so the UI can show a "上次 worktree"
    /// chip that lets the user re-attach or inspect the branch.
    pub last_worktree_path: Option<String>,
    /// PR4 of multi-model: per-session model override. `None` when
    /// the session uses the global default model (the chat command's
    /// `resolve_chat_provider` falls back to `app_config.default_model_id`
    /// when this is NULL or the referenced model was deleted). This is a
    /// soft FK to `models.id` — no `REFERENCES` constraint so legacy rows
    /// and dangling references don't break INSERTs.
    pub model_id: Option<String>,
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
    /// Mirror of [`SessionRow::worktree_path`]. `None` for sessions
    /// in `none` or `detached` state.
    pub worktree_path: Option<String>,
    /// Mirror of [`SessionRow::worktree_state`].
    pub worktree_state: WorktreeState,
    /// Mirror of [`SessionRow::last_worktree_path`].
    pub last_worktree_path: Option<String>,
    /// PR4 of multi-model: per-session model override. `None` when the
    /// session uses the global default model. Soft FK to `models.id`.
    pub model_id: Option<String>,
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
    /// Optional structured metadata. `None` for chat history rows;
    /// `Some(json)` for system events injected by the worktree
    /// commands. Used by rehydrate to filter or specially render.
    pub metadata: Option<serde_json::Value>,
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

    // --- Step 4 ALTER: add worktree_path to sessions.
    //  Nullable (no DEFAULT) so pre-step-4 rows keep NULL and the
    //  Rust side falls back to `current_cwd` for them. New step 4
    //  rows always have a value (the create_session call returns
    //  an error before the INSERT if worktree creation fails). ---
    add_session_column_if_missing(pool, "worktree_path", "TEXT").await?;

    // --- Step 4 follow-up: opt-in worktree (auto-create → manual
    //  attach/detach/delete). Adds the tri-state `worktree_state`
    //  column (default 'none') and `last_worktree_path` for
    //  detached sessions.
    //
    //  Backfill: sessions that have `worktree_path IS NOT NULL`
    //  AND `worktree_state IS NULL` are pre-follow-up rows that
    //  were created under the old auto-create flow. They were
    //  effectively "active" at the time of creation, so we mark
    //  them as 'active' here. This matches the PR1 / PR2 spirit
    //  of the git-metadata backfill: idempotent, fire-and-forget,
    //  and run after the column add. ---
    add_session_column_if_missing(
        pool,
        "worktree_state",
        "TEXT NOT NULL DEFAULT 'none'",
    )
    .await?;
    add_session_column_if_missing(pool, "last_worktree_path", "TEXT").await?;
    sqlx::query(
        r#"
        UPDATE sessions
        SET worktree_state = 'active'
        WHERE worktree_path IS NOT NULL
          AND (worktree_state IS NULL OR worktree_state = '')
        "#,
    )
    .execute(pool)
    .await?;

    // --- PR2 ALTERs: add is_git_repo + git_branch to projects.
    //  `is_git_repo` already exists on freshly created tables (see
    //  CREATE TABLE above) so the idempotent probe is a no-op for
    //  greenfield DBs. Older pre-3b-1 databases may have a
    //  `projects` table without these columns; the probe + ALTER
    //  brings them up to date. ---
    add_project_column_if_missing(pool, "is_git_repo", "INTEGER NOT NULL DEFAULT 0").await?;
    add_project_column_if_missing(pool, "git_branch", "TEXT").await?;

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
            metadata         TEXT,
            UNIQUE(session_id, seq)
        )
        "#,
    )
    .execute(pool)
    .await?;
    // Step 4 follow-up: add `metadata` column for system events.
    // The CREATE TABLE above has the column for greenfield DBs;
    // the probe + ALTER backfills older databases. Nullable so
    // pre-existing rows keep NULL.
    add_messages_column_if_missing(pool, "metadata", "TEXT").await?;
    sqlx::query(
        r#"
        CREATE INDEX IF NOT EXISTS idx_messages_session_seq
        ON messages(session_id, seq)
        "#,
    )
    .execute(pool)
    .await?;

    // --- PR1 of multi-model task: providers / models / app_config.
    //
    //  The `providers` table is the user-managed catalog of LLM
    //  endpoints (Anthropic 官方, wukaijin 转发, OpenAI 官方, ...);
    //  multiple rows may share the same `protocol`. `models` binds
    //  model names to a provider with capability hints and per-row
    //  overrides for `max_tokens` / `thinking_effort`. `app_config`
    //  is a small key/value store; the only key written today is
    //  `default_model_id`, but the table is generic so future global
    //  settings don't need a new migration. ---
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS providers (
            id           TEXT PRIMARY KEY,
            protocol     TEXT NOT NULL,
            display_name TEXT NOT NULL,
            base_url     TEXT NOT NULL,
            api_key      TEXT NOT NULL DEFAULT '',
            created_at   TEXT NOT NULL,
            updated_at   TEXT NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS models (
            id                TEXT PRIMARY KEY,
            provider_id       TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
            model_name        TEXT NOT NULL,
            display_name      TEXT NOT NULL,
            max_tokens        INTEGER,
            thinking_effort   TEXT,
            supports_thinking INTEGER NOT NULL DEFAULT 0,
            context_window    INTEGER NOT NULL,
            created_at        TEXT NOT NULL,
            updated_at        TEXT NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
        CREATE INDEX IF NOT EXISTS idx_models_provider_id
        ON models(provider_id)
        "#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS app_config (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

    // --- PR1 of multi-model task: add `model_id` to sessions.
    //  Nullable FK to `models.id`. Pre-PR1 sessions have NULL; the
    //  seed function below backfills them with the default model.
    //  Kept as a soft FK (no FK constraint) so a future row with a
    //  dangling `model_id` (e.g. legacy dump) doesn't break INSERTs. ---
    add_session_column_if_missing(pool, "model_id", "TEXT").await?;
    sqlx::query(
        r#"
        CREATE INDEX IF NOT EXISTS idx_sessions_model_id
        ON sessions(model_id)
        "#,
    )
    .execute(pool)
    .await?;

    // --- PR1 of multi-model task: seed default providers + models
    //  if the catalog is empty. Idempotent: 0-row check skips the
    //  insert on subsequent boots. Backfills `sessions.model_id`
    //  for any row still NULL after the ALTER. ---
    seed_default_providers_and_models(pool).await?;

    // --- Auto-default project (backstop for legacy sessions) ---
    // Insert the backstop row *after* the ALTERs so any sessions
    // created in this same migration (none in normal flow) can FK
    // against it. For pre-3b-1 sessions, the ALTER DEFAULT
    // `'__default__'` already wires them up.
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        INSERT OR IGNORE INTO projects
            (id, name, path, is_git_repo, git_branch, is_legacy, created_at, updated_at, hidden, metadata)
        VALUES (?, ?, ?, 0, NULL, 1, ?, ?, 0, NULL)
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

/// Add a column to `projects` if it doesn't already exist. Mirrors
/// [`add_session_column_if_missing`].
async fn add_project_column_if_missing(
    pool: &SqlitePool,
    column: &str,
    decl: &str,
) -> Result<(), sqlx::Error> {
    let exists: i64 =
        sqlx::query("SELECT COUNT(*) FROM pragma_table_info('projects') WHERE name = ?")
            .bind(column)
            .fetch_one(pool)
            .await?
            .try_get(0)?;
    if exists == 0 {
        let stmt = format!("ALTER TABLE projects ADD COLUMN {} {}", column, decl);
        sqlx::query(&stmt).execute(pool).await?;
    }
    Ok(())
}

/// Add a column to `messages` if it doesn't already exist. Mirrors
/// [`add_session_column_if_missing`].
async fn add_messages_column_if_missing(
    pool: &SqlitePool,
    column: &str,
    decl: &str,
) -> Result<(), sqlx::Error> {
    let exists: i64 =
        sqlx::query("SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name = ?")
            .bind(column)
            .fetch_one(pool)
            .await?
            .try_get(0)?;
    if exists == 0 {
        let stmt = format!("ALTER TABLE messages ADD COLUMN {} {}", column, decl);
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
    git_branch: Option<String>,
) -> Result<ProjectRow, sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    let id = Uuid::new_v4().to_string();

    let res = sqlx::query(
        r#"
        INSERT INTO projects
            (id, name, path, is_git_repo, git_branch, is_legacy, created_at, updated_at, hidden, metadata)
        VALUES (?, ?, ?, ?, ?, 0, ?, ?, 0, NULL)
        "#,
    )
    .bind(&id)
    .bind(name)
    .bind(path)
    .bind(is_git_repo as i64)
    .bind(git_branch.as_deref())
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
            git_branch,
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
            SELECT id, name, path, is_git_repo, git_branch, is_legacy, created_at, updated_at, hidden, metadata
            FROM projects
            ORDER BY created_at ASC
            "#,
        )
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query(
            r#"
            SELECT id, name, path, is_git_repo, git_branch, is_legacy, created_at, updated_at, hidden, metadata
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
        SELECT id, name, path, is_git_repo, git_branch, is_legacy, created_at, updated_at, hidden, metadata
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
        SELECT id, name, path, is_git_repo, git_branch, is_legacy, created_at, updated_at, hidden, metadata
        FROM projects
        WHERE id = ?
        "#,
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await?;
    row.map(row_to_project).transpose()
}

/// Change a project's `path` (re-probing `is_git_repo` and
/// `git_branch` is the caller's responsibility — see
/// `projects::store::update_project_path`).
pub async fn update_project_path(
    pool: &SqlitePool,
    project_id: &str,
    new_path: &str,
    is_git_repo: bool,
    git_branch: Option<String>,
) -> Result<ProjectRow, sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    let res = sqlx::query(
        r#"
        UPDATE projects
        SET path = ?, is_git_repo = ?, git_branch = ?, updated_at = ?
        WHERE id = ?
        "#,
    )
    .bind(new_path)
    .bind(is_git_repo as i64)
    .bind(git_branch.as_deref())
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

/// List projects whose `is_git_repo` is `0` — i.e. projects that
/// were created before the PR2 migration (which adds
/// `is_git_repo` / `git_branch`) and have never been re-probed, or
/// projects whose original probe failed. Sorted by `created_at ASC`
/// for stable test ordering.
///
/// Hidden projects are excluded from the backfill: they're not shown
/// in the Tab bar (which is the surface that would expose the bug),
/// and a user who explicitly hid a project is signaling that they
/// don't want proactive work on it. If they unhide later, the chip
/// will still show "git" until the next `update_project_path` call,
/// but that case is rare and acceptable.
///
/// Used by the startup backfill task — see
/// `projects::store::batch_reprobe_git_metadata` and the spawn in
/// `lib.rs::AppState::load`.
pub async fn list_projects_with_stale_git_probe(
    pool: &SqlitePool,
) -> Result<Vec<ProjectRow>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT id, name, path, is_git_repo, git_branch, is_legacy, created_at, updated_at, hidden, metadata
        FROM projects
        WHERE is_git_repo = 0 AND hidden = 0
        ORDER BY created_at ASC
        "#,
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(row_to_project).collect()
}

/// Update a project's `is_git_repo` and `git_branch`. Used by the
/// startup batch backfill to write re-probed git metadata without
/// touching the other columns (name / path / hidden / etc.).
///
/// `git_branch` is `None` for non-git repos; the literal string
/// `"HEAD"` is allowed through for detached-HEAD repos.
pub async fn update_project_git_metadata(
    pool: &SqlitePool,
    project_id: &str,
    is_git_repo: bool,
    git_branch: Option<&str>,
) -> Result<(), sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        UPDATE projects
        SET is_git_repo = ?, git_branch = ?, updated_at = ?
        WHERE id = ?
        "#,
    )
    .bind(is_git_repo as i64)
    .bind(git_branch)
    .bind(&now)
    .bind(project_id)
    .execute(pool)
    .await?;
    Ok(())
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
        git_branch: r.try_get("git_branch")?,
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
///
/// `session_id` is supplied by the caller; the caller is responsible
/// for UUID uniqueness.
///
/// `worktree_path` is `None` for sessions in `WorktreeState::None`
/// (the new opt-in default — sessions no longer auto-create a
/// worktree; the user must call `attach_worktree` explicitly).
/// Sessions that have been migrated from the pre-follow-up auto-
/// create flow get the path on attach instead.
pub async fn create_session(
    pool: &SqlitePool,
    session_id: &str,
    project_id: &str,
    initial_cwd: &str,
    model: &str,
) -> Result<SessionRow, sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    let title = "新对话".to_string();

    sqlx::query(
        r#"
        INSERT INTO sessions
            (id, title, created_at, updated_at, model, metadata, project_id, current_cwd,
             worktree_path, worktree_state, last_worktree_path)
        VALUES (?, ?, ?, ?, ?, NULL, ?, ?, NULL, 'none', NULL)
        "#,
    )
    .bind(session_id)
    .bind(&title)
    .bind(&now)
    .bind(&now)
    .bind(model)
    .bind(project_id)
    .bind(initial_cwd)
    .execute(pool)
    .await?;

    Ok(SessionRow {
        id: session_id.to_string(),
        title,
        created_at: now.clone(),
        updated_at: now,
        model: model.to_string(),
        project_id: project_id.to_string(),
        current_cwd: initial_cwd.to_string(),
        worktree_path: None,
        worktree_state: WorktreeState::None,
        last_worktree_path: None,
        model_id: None,
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
               s.worktree_path, s.worktree_state, s.last_worktree_path,
               s.model_id,
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
            let state_str: String = r.try_get("worktree_state")?;
            Ok(SessionSummary {
                id: r.try_get("id")?,
                title: r.try_get("title")?,
                updated_at: r.try_get("updated_at")?,
                preview,
                project_id: r.try_get("project_id")?,
                current_cwd: r.try_get("current_cwd")?,
                worktree_path: r.try_get("worktree_path")?,
                worktree_state: WorktreeState::from_str_opt(&state_str),
                last_worktree_path: r.try_get("last_worktree_path")?,
                model_id: r.try_get("model_id")?,
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
        SELECT id, title, created_at, updated_at, model, project_id, current_cwd,
               worktree_path, worktree_state, last_worktree_path, model_id
        FROM sessions
        WHERE id = ?
        "#,
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await?;

    let session = match session_row {
        Some(r) => {
            let state_str: String = r.try_get("worktree_state")?;
            SessionRow {
                id: r.try_get("id")?,
                title: r.try_get("title")?,
                created_at: r.try_get("created_at")?,
                updated_at: r.try_get("updated_at")?,
                model: r.try_get("model")?,
                project_id: r.try_get("project_id")?,
                current_cwd: r.try_get("current_cwd")?,
                worktree_path: r.try_get("worktree_path")?,
                worktree_state: WorktreeState::from_str_opt(&state_str),
                last_worktree_path: r.try_get("last_worktree_path")?,
                model_id: r.try_get("model_id")?,
            }
        }
        None => return Ok(None),
    };

    let msg_rows = sqlx::query(
        r#"
        SELECT id, session_id, role, content, text, has_tool_calls, has_tool_results,
               created_at, seq, metadata
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
            // metadata column is JSON or NULL. Parse if present.
            let metadata: Option<serde_json::Value> = r
                .try_get::<Option<String>, _>("metadata")?
                .and_then(|s| serde_json::from_str(&s).ok());
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
                metadata,
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
// Session model assignment (PR4 of multi-model task)
// ---------------------------------------------------------------------------

/// Update the `model_id` soft FK on a session row. Used by the
/// frontend's per-session model dropdown (StatusBar) so the user can
/// switch models without changing the global default. The value is a
/// UUID string referencing `models.id`, or can be set to NULL by
/// passing an empty string (the resolve-default fallback in the chat
/// command's `resolve_chat_provider` handles NULL by using the global
/// default).
///
/// `updated_at` is bumped to the current time on every successful
/// write so the session list re-sorts correctly.
pub async fn update_session_model_id(
    pool: &SqlitePool,
    session_id: &str,
    model_id: &str,
) -> Result<(), sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    // Empty string → store NULL (session falls back to global default).
    let model_id_value: Option<&str> = if model_id.is_empty() {
        None
    } else {
        Some(model_id)
    };
    sqlx::query(
        r#"
        UPDATE sessions
           SET model_id = ?, updated_at = ?
         WHERE id = ?
        "#,
    )
    .bind(model_id_value)
    .bind(&now)
    .bind(session_id)
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Worktree state transitions (step 4 follow-up)
// ---------------------------------------------------------------------------

/// Set the session's `worktree_path`, `worktree_state`, and
/// (optionally) `last_worktree_path` in a single statement. Used
/// by the `attach_worktree` / `detach_worktree` / `delete_worktree`
/// Tauri commands to keep the three columns consistent. The
/// `last_worktree_path` is preserved across detach by passing the
/// old value through; the caller computes it from the row before
/// the transition.
pub async fn set_worktree_state(
    pool: &SqlitePool,
    session_id: &str,
    state: WorktreeState,
    worktree_path: Option<&str>,
    last_worktree_path: Option<&str>,
) -> Result<(), sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        UPDATE sessions
        SET worktree_state = ?,
            worktree_path = ?,
            last_worktree_path = ?,
            updated_at = ?
        WHERE id = ?
        "#,
    )
    .bind(state.as_str())
    .bind(worktree_path)
    .bind(last_worktree_path)
    .bind(&now)
    .bind(session_id)
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// System event injection (step 4 follow-up)
// ---------------------------------------------------------------------------

/// Append a synthetic user-role message to the session's history,
/// recording a worktree state change (attach / detach / delete).
/// The next LLM turn will see the message in its history, so the
/// model is aware of the worktree state transition before any
/// tool call goes out.
///
/// The stored `content` is a JSON array of one `text` block so the
/// rehydrate path picks it up correctly. The `text` column gets a
/// short plain-text summary for the sidebar preview. The
/// `metadata` column carries the structured event marker so future
/// migrations can filter these from the chat history.
pub async fn insert_system_event(
    pool: &SqlitePool,
    session_id: &str,
    text: &str,
    event_kind: &str,
) -> Result<(), sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    // Compute the next seq for this session. We do a separate
    // SELECT MAX to keep the query portable across SQLite versions
    // (no RETURNING in 3.35, no UPSERT-with-RETURNING before that).
    let next_seq: i64 = sqlx::query("SELECT COALESCE(MAX(seq), -1) + 1 FROM messages WHERE session_id = ?")
        .bind(session_id)
        .fetch_one(pool)
        .await?
        .try_get(0)?;
    let content_json = serde_json::json!([
        {
            "type": "text",
            "text": format!("[worktree event] {}", text),
        }
    ])
    .to_string();
    let metadata = serde_json::json!({
        "kind": "worktree_event",
        "event": event_kind,
    })
    .to_string();
    sqlx::query(
        r#"
        INSERT INTO messages
           (session_id, role, content, text, has_tool_calls, has_tool_results,
            created_at, seq, metadata)
        VALUES (?, 'user', ?, ?, 0, 0, ?, ?, ?)
        "#,
    )
    .bind(session_id)
    .bind(&content_json)
    .bind(text)
    .bind(&now)
    .bind(next_seq)
    .bind(&metadata)
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
// PR1 of multi-model task: providers / models / app_config CRUD
// ---------------------------------------------------------------------------

/// Insert a new provider. Returns the inserted row with server-set
/// fields (`id`, `created_at`, `updated_at`) populated. `protocol`
/// is taken as a `String` (not `ProviderProtocol`) so an unknown
/// future value from a newer DB can still be stored without
/// crashing the writer; the read path will fall back to
/// `ProviderProtocol::from_str_opt` for lenient parsing.
pub async fn create_provider(
    pool: &SqlitePool,
    protocol: &str,
    display_name: &str,
    base_url: &str,
    api_key: &str,
) -> Result<ProviderRow, sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
        INSERT INTO providers
            (id, protocol, display_name, base_url, api_key, created_at, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&id)
    .bind(protocol)
    .bind(display_name)
    .bind(base_url)
    .bind(api_key)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(ProviderRow {
        id,
        protocol: protocol.to_string(),
        display_name: display_name.to_string(),
        base_url: base_url.to_string(),
        api_key: api_key.to_string(),
        created_at: now.clone(),
        updated_at: now,
    })
}

/// List all providers, newest updated first.
pub async fn list_providers(pool: &SqlitePool) -> Result<Vec<ProviderRow>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT id, protocol, display_name, base_url, api_key, created_at, updated_at
        FROM providers
        ORDER BY updated_at DESC
        "#,
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(ProviderRow {
                id: r.try_get("id")?,
                protocol: r.try_get("protocol")?,
                display_name: r.try_get("display_name")?,
                base_url: r.try_get("base_url")?,
                api_key: r.try_get("api_key")?,
                created_at: r.try_get("created_at")?,
                updated_at: r.try_get("updated_at")?,
            })
        })
        .collect()
}

/// Get a single provider by `id`. Returns `None` when the row
/// doesn't exist. Used by the `test_model` IPC to look up the
/// parent provider given a model id.
pub async fn get_provider(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<ProviderRow>, sqlx::Error> {
    let row = sqlx::query(
        r#"
        SELECT id, protocol, display_name, base_url, api_key, created_at, updated_at
        FROM providers
        WHERE id = ?
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    match row {
        None => Ok(None),
        Some(r) => Ok(Some(ProviderRow {
            id: r.try_get("id")?,
            protocol: r.try_get("protocol")?,
            display_name: r.try_get("display_name")?,
            base_url: r.try_get("base_url")?,
            api_key: r.try_get("api_key")?,
            created_at: r.try_get("created_at")?,
            updated_at: r.try_get("updated_at")?,
        })),
    }
}

/// Patch a provider by `id`. Returns `None` if the row doesn't
/// exist; otherwise returns the updated row. `updated_at` is bumped
/// to the current time on every successful update.
pub async fn update_provider(
    pool: &SqlitePool,
    id: &str,
    protocol: &str,
    display_name: &str,
    base_url: &str,
    api_key: &str,
) -> Result<Option<ProviderRow>, sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    let res = sqlx::query(
        r#"
        UPDATE providers
           SET protocol = ?, display_name = ?, base_url = ?,
               api_key = ?, updated_at = ?
         WHERE id = ?
        "#,
    )
    .bind(protocol)
    .bind(display_name)
    .bind(base_url)
    .bind(api_key)
    .bind(&now)
    .bind(id)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return Ok(None);
    }
    Ok(Some(ProviderRow {
        id: id.to_string(),
        protocol: protocol.to_string(),
        display_name: display_name.to_string(),
        base_url: base_url.to_string(),
        api_key: api_key.to_string(),
        created_at: String::new(), // not reloaded; callers that need it should re-fetch
        updated_at: now,
    }))
}

/// Delete a provider by `id`. Cascades to its models (FK is
/// `ON DELETE CASCADE`). Returns whether a row was actually
/// removed.
pub async fn delete_provider(pool: &SqlitePool, id: &str) -> Result<bool, sqlx::Error> {
    let res = sqlx::query("DELETE FROM providers WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

/// Insert a new model. Returns the inserted row.
pub async fn create_model(
    pool: &SqlitePool,
    provider_id: &str,
    model_name: &str,
    display_name: &str,
    max_tokens: Option<u32>,
    thinking_effort: Option<&str>,
    supports_thinking: bool,
    context_window: u32,
) -> Result<ModelRow, sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
        INSERT INTO models
            (id, provider_id, model_name, display_name, max_tokens, thinking_effort,
             supports_thinking, context_window, created_at, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&id)
    .bind(provider_id)
    .bind(model_name)
    .bind(display_name)
    .bind(max_tokens)
    .bind(thinking_effort)
    .bind(supports_thinking as i32)
    .bind(context_window)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(ModelRow {
        id,
        provider_id: provider_id.to_string(),
        model_name: model_name.to_string(),
        display_name: display_name.to_string(),
        max_tokens,
        thinking_effort: thinking_effort.map(str::to_string),
        supports_thinking,
        context_window,
        created_at: now.clone(),
        updated_at: now,
    })
}

/// List all models joined with their parent provider's
/// `display_name` + `protocol` for UI rendering. Newest updated
/// first; within a model, sort is by `display_name` ascending.
pub async fn list_models(pool: &SqlitePool) -> Result<Vec<ModelWithProvider>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT m.id, m.provider_id, m.model_name, m.display_name,
               m.max_tokens, m.thinking_effort, m.supports_thinking,
               m.context_window, m.created_at, m.updated_at,
               p.display_name AS provider_display_name,
               p.protocol      AS provider_protocol
        FROM models m
        JOIN providers p ON p.id = m.provider_id
        ORDER BY m.updated_at DESC, m.display_name ASC
        "#,
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|r| {
            let supports_thinking_i: i32 = r.try_get("supports_thinking")?;
            Ok(ModelWithProvider {
                model: ModelRow {
                    id: r.try_get("id")?,
                    provider_id: r.try_get("provider_id")?,
                    model_name: r.try_get("model_name")?,
                    display_name: r.try_get("display_name")?,
                    max_tokens: r.try_get("max_tokens")?,
                    thinking_effort: r.try_get("thinking_effort")?,
                    supports_thinking: supports_thinking_i != 0,
                    context_window: r.try_get("context_window")?,
                    created_at: r.try_get("created_at")?,
                    updated_at: r.try_get("updated_at")?,
                },
                provider_display_name: r.try_get("provider_display_name")?,
                provider_protocol: r.try_get("provider_protocol")?,
            })
        })
        .collect()
}

/// Get a single model row by `id` (no provider join). Returns
/// `None` when the row doesn't exist. Used by the `test_model`
/// IPC, which then looks up the parent provider separately.
pub async fn get_model(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<ModelRow>, sqlx::Error> {
    let row = sqlx::query(
        r#"
        SELECT id, provider_id, model_name, display_name,
               max_tokens, thinking_effort, supports_thinking,
               context_window, created_at, updated_at
        FROM models
        WHERE id = ?
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    match row {
        None => Ok(None),
        Some(r) => {
            let supports_thinking_i: i32 = r.try_get("supports_thinking")?;
            Ok(Some(ModelRow {
                id: r.try_get("id")?,
                provider_id: r.try_get("provider_id")?,
                model_name: r.try_get("model_name")?,
                display_name: r.try_get("display_name")?,
                max_tokens: r.try_get("max_tokens")?,
                thinking_effort: r.try_get("thinking_effort")?,
                supports_thinking: supports_thinking_i != 0,
                context_window: r.try_get("context_window")?,
                created_at: r.try_get("created_at")?,
                updated_at: r.try_get("updated_at")?,
            }))
        }
    }
}

/// Patch a model by `id`. Returns `None` if the row doesn't exist.
pub async fn update_model(
    pool: &SqlitePool,
    id: &str,
    provider_id: &str,
    model_name: &str,
    display_name: &str,
    max_tokens: Option<u32>,
    thinking_effort: Option<&str>,
    supports_thinking: bool,
    context_window: u32,
) -> Result<Option<ModelRow>, sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    let res = sqlx::query(
        r#"
        UPDATE models
           SET provider_id = ?, model_name = ?, display_name = ?,
               max_tokens = ?, thinking_effort = ?,
               supports_thinking = ?, context_window = ?, updated_at = ?
         WHERE id = ?
        "#,
    )
    .bind(provider_id)
    .bind(model_name)
    .bind(display_name)
    .bind(max_tokens)
    .bind(thinking_effort)
    .bind(supports_thinking as i32)
    .bind(context_window)
    .bind(&now)
    .bind(id)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return Ok(None);
    }
    Ok(Some(ModelRow {
        id: id.to_string(),
        provider_id: provider_id.to_string(),
        model_name: model_name.to_string(),
        display_name: display_name.to_string(),
        max_tokens,
        thinking_effort: thinking_effort.map(str::to_string),
        supports_thinking,
        context_window,
        created_at: String::new(),
        updated_at: now,
    }))
}

/// Delete a model by `id`. Returns whether a row was actually
/// removed. Sessions that referenced this model keep the dangling
/// `model_id` (it's a soft FK with no `ON DELETE` clause); the
/// reader path is responsible for the fallback (PR2 wires the
/// resolve-default fallback in the agent loop).
pub async fn delete_model(pool: &SqlitePool, id: &str) -> Result<bool, sqlx::Error> {
    let res = sqlx::query("DELETE FROM models WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

// ---------------------------------------------------------------------------
// PR1 of multi-model task: app_config key/value helpers
// ---------------------------------------------------------------------------

/// Read a value from `app_config` by key. Returns `None` if the key
/// is not present. Kept as a generic key/value getter so future
/// global settings don't need a new IPC.
pub async fn get_config_value(pool: &SqlitePool, key: &str) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query("SELECT value FROM app_config WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    row.map(|r| r.try_get("value")).transpose()
}

/// Write a value to `app_config`, inserting or replacing the row.
pub async fn set_config_value(pool: &SqlitePool, key: &str, value: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO app_config (key, value) VALUES (?, ?)
        ON CONFLICT(key) DO UPDATE SET value = excluded.value
        "#,
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// PR1 of multi-model task: seed
// ---------------------------------------------------------------------------

/// Seed the default `providers` + `models` + `default_model_id` if
/// the `providers` table is empty. Idempotent: when at least one
/// provider already exists, the function is a no-op (preserves any
/// user edits). When run, it inserts:
///
/// - `Anthropic 官方` provider, base URL `https://api.anthropic.com`,
///   empty api_key
/// - `OpenAI 官方` provider, base URL `https://api.openai.com/v1`,
///   empty api_key
/// - `claude-sonnet-4-5` model bound to Anthropic, `supports_thinking=true`,
///   `context_window=200_000`
/// - `claude-opus-4-7` model bound to Anthropic, `supports_thinking=true`,
///   `context_window=200_000`
/// - `gpt-4o` model bound to OpenAI, `supports_thinking=false`,
///   `context_window=128_000`
/// - `gpt-4.1` model bound to OpenAI, `supports_thinking=false`,
///   `context_window=1_000_000`
/// - `default_model_id` -> `claude-sonnet-4-5`
///
/// After the catalog is in place, backfills `sessions.model_id` for
/// any row still NULL or empty (legacy sessions from the pre-PR1
/// era) with the default model id.
pub async fn seed_default_providers_and_models(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let count: i64 = sqlx::query("SELECT COUNT(*) FROM providers")
        .fetch_one(pool)
        .await?
        .try_get(0)?;
    if count > 0 {
        return Ok(());
    }
    let now = Utc::now().to_rfc3339();

    // --- providers ---
    let anthropic_id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
        INSERT INTO providers
            (id, protocol, display_name, base_url, api_key, created_at, updated_at)
        VALUES (?, 'anthropic', 'Anthropic 官方', 'https://api.anthropic.com', '', ?, ?)
        "#,
    )
    .bind(&anthropic_id)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;
    let openai_id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
        INSERT INTO providers
            (id, protocol, display_name, base_url, api_key, created_at, updated_at)
        VALUES (?, 'openai', 'OpenAI 官方', 'https://api.openai.com/v1', '', ?, ?)
        "#,
    )
    .bind(&openai_id)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;

    // --- models ---
    let sonnet_id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
        INSERT INTO models
            (id, provider_id, model_name, display_name, max_tokens, thinking_effort,
             supports_thinking, context_window, created_at, updated_at)
        VALUES (?, ?, 'claude-sonnet-4-5', 'Claude Sonnet 4.5',
                NULL, NULL, 1, 200000, ?, ?)
        "#,
    )
    .bind(&sonnet_id)
    .bind(&anthropic_id)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;
    let opus_id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
        INSERT INTO models
            (id, provider_id, model_name, display_name, max_tokens, thinking_effort,
             supports_thinking, context_window, created_at, updated_at)
        VALUES (?, ?, 'claude-opus-4-7', 'Claude Opus 4.7',
                NULL, NULL, 1, 200000, ?, ?)
        "#,
    )
    .bind(&opus_id)
    .bind(&anthropic_id)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;
    let gpt4o_id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
        INSERT INTO models
            (id, provider_id, model_name, display_name, max_tokens, thinking_effort,
             supports_thinking, context_window, created_at, updated_at)
        VALUES (?, ?, 'gpt-4o', 'GPT-4o',
                NULL, NULL, 0, 128000, ?, ?)
        "#,
    )
    .bind(&gpt4o_id)
    .bind(&openai_id)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;
    let gpt41_id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"
        INSERT INTO models
            (id, provider_id, model_name, display_name, max_tokens, thinking_effort,
             supports_thinking, context_window, created_at, updated_at)
        VALUES (?, ?, 'gpt-4.1', 'GPT-4.1',
                NULL, NULL, 0, 1000000, ?, ?)
        "#,
    )
    .bind(&gpt41_id)
    .bind(&openai_id)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;

    // --- default model ---
    set_config_value(pool, "default_model_id", &sonnet_id).await?;

    // --- backfill sessions.model_id with the default ---
    sqlx::query("UPDATE sessions SET model_id = ? WHERE model_id IS NULL OR model_id = ''")
        .bind(&sonnet_id)
        .execute(pool)
        .await?;

    Ok(())
}



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

        let p1 = create_project(&pool, "a", path_str, false, None).await.unwrap();
        // Duplicate path → unique violation → Err.
        let dup = create_project(&pool, "b", path_str, false, None).await;
        assert!(dup.is_err(), "duplicate path should fail");

        let p2 = create_project(&pool, "c", "/tmp/everlasting_test_other", true, None)
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
        let p = create_project(&pool, "x", "/tmp/everlasting_test_hide", false, None)
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
        let p = create_project(&pool, "old", "/tmp/everlasting_test_rename", false, None)
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
        let p = create_project(&pool, "p", "/tmp/everlasting_test_repath", false, None)
            .await
            .unwrap();
        assert!(!p.is_git_repo);
        assert!(p.git_branch.is_none());

        let p2 =
            update_project_path(&pool, &p.id, "/tmp/everlasting_test_repath2", true, None)
                .await
                .unwrap();
        assert!(p2.is_git_repo);
        assert_eq!(p2.path, "/tmp/everlasting_test_repath2");
    }

    #[tokio::test]
    async fn list_projects_with_stale_git_probe_filters_correctly() {
        // Pre-PR2 row: should appear.
        let pool = test_pool().await;
        let p_stale =
            create_project(&pool, "stale", "/tmp/everlasting_test_stale", false, None)
                .await
                .unwrap();
        assert!(!p_stale.is_git_repo);

        // Already-probed row: should NOT appear.
        let p_fresh = create_project(
            &pool,
            "fresh",
            "/tmp/everlasting_test_fresh",
            true,
            Some("main".to_string()),
        )
        .await
        .unwrap();
        assert!(p_fresh.is_git_repo);

        // Hidden stale row: should NOT appear (we skip hidden
        // projects — see the function's docstring).
        let p_hidden =
            create_project(&pool, "hidden", "/tmp/everlasting_test_hidden", false, None)
                .await
                .unwrap();
        hide_project(&pool, &p_hidden.id).await.unwrap();

        let stale = list_projects_with_stale_git_probe(&pool).await.unwrap();
        let ids: Vec<&str> = stale.iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&p_stale.id.as_str()));
        assert!(!ids.contains(&p_fresh.id.as_str()));
        assert!(!ids.contains(&p_hidden.id.as_str()));
    }

    #[tokio::test]
    async fn update_project_git_metadata_round_trip() {
        let pool = test_pool().await;
        // Start from a non-git row, write git metadata, reload, verify.
        let p = create_project(&pool, "p", "/tmp/everlasting_test_metaupd", false, None)
            .await
            .unwrap();
        assert!(!p.is_git_repo);

        update_project_git_metadata(&pool, &p.id, true, Some("feature/x"))
            .await
            .unwrap();
        let reloaded = get_project(&pool, &p.id).await.unwrap().unwrap();
        assert!(reloaded.is_git_repo);
        assert_eq!(reloaded.git_branch.as_deref(), Some("feature/x"));

        // Setting `git_branch = None` (e.g. for a non-git repo) is
        // distinct from "empty string".
        update_project_git_metadata(&pool, &p.id, false, None).await.unwrap();
        let reloaded = get_project(&pool, &p.id).await.unwrap().unwrap();
        assert!(!reloaded.is_git_repo);
        assert!(reloaded.git_branch.is_none());
    }

    #[tokio::test]
    async fn create_project_persists_git_branch() {
        let pool = test_pool().await;
        // Branch string survives a round-trip through the DB; the
        // detached-HEAD literal "HEAD" is also accepted.
        let p = create_project(
            &pool,
            "branchy",
            "/tmp/everlasting_test_branch",
            true,
            Some("feature/pr2".to_string()),
        )
        .await
        .unwrap();
        assert_eq!(p.git_branch.as_deref(), Some("feature/pr2"));

        let reloaded = get_project(&pool, &p.id).await.unwrap().unwrap();
        assert_eq!(reloaded.git_branch.as_deref(), Some("feature/pr2"));

        let detached = create_project(
            &pool,
            "detached",
            "/tmp/everlasting_test_detached",
            true,
            Some("HEAD".to_string()),
        )
        .await
        .unwrap();
        assert_eq!(detached.git_branch.as_deref(), Some("HEAD"));
    }

    #[tokio::test]
    async fn update_project_path_reprobes_git_branch() {
        let pool = test_pool().await;
        let p = create_project(
            &pool,
            "rebranch",
            "/tmp/everlasting_test_rebranch",
            true,
            Some("main".to_string()),
        )
        .await
        .unwrap();
        assert_eq!(p.git_branch.as_deref(), Some("main"));

        // Re-probe with a different branch.
        let p2 = update_project_path(
            &pool,
            &p.id,
            "/tmp/everlasting_test_rebranch2",
            true,
            Some("develop".to_string()),
        )
        .await
        .unwrap();
        assert_eq!(p2.git_branch.as_deref(), Some("develop"));

        // Re-probe to a non-git path → branch cleared.
        let p3 = update_project_path(
            &pool,
            &p.id,
            "/tmp/everlasting_test_rebranch3",
            false,
            None,
        )
        .await
        .unwrap();
        assert!(!p3.is_git_repo);
        assert!(p3.git_branch.is_none());
    }

    #[tokio::test]
    async fn create_session_scopes_to_project() {
        let pool = test_pool().await;
        let p = create_project(&pool, "p", "/tmp/everlasting_test_session_proj", false, None)
            .await
            .unwrap();

        let s1 = create_session(&pool, &Uuid::new_v4().to_string(), &p.id, "/tmp/foo", "GLM-4.7")
            .await
            .unwrap();
        let s2 = create_session(&pool, &Uuid::new_v4().to_string(), &p.id, "/tmp/bar", "GLM-4.7")
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
        let session = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7")
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
        let session = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7")
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
        let session = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7")
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
        let session = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7")
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
        let session = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7")
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
        let session = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7")
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
        let session = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp/start", "GLM-4.7")
            .await
            .unwrap();
        assert_eq!(session.current_cwd, "/tmp/start");

        update_session_cwd(&pool, &session.id, "/tmp/end").await.unwrap();
        let reloaded = load_session(&pool, &session.id).await.unwrap().unwrap();
        assert_eq!(reloaded.session.current_cwd, "/tmp/end");
    }

    // -----------------------------------------------------------------------
    // Step 4 follow-up: worktree state transition + system event tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn new_session_defaults_to_none_state() {
        let pool = test_pool().await;
        let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7")
            .await
            .unwrap();
        assert_eq!(s.worktree_state, WorktreeState::None);
        assert!(s.worktree_path.is_none());
        assert!(s.last_worktree_path.is_none());

        let reloaded = load_session(&pool, &s.id).await.unwrap().unwrap();
        assert_eq!(reloaded.session.worktree_state, WorktreeState::None);
        assert!(reloaded.session.worktree_path.is_none());
    }

    #[tokio::test]
    async fn worktree_state_setter_round_trip() {
        let pool = test_pool().await;
        let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7")
            .await
            .unwrap();
        // Attach.
        set_worktree_state(&pool, &s.id, WorktreeState::Active, Some("/data/wt"), None)
            .await
            .unwrap();
        let r = load_session(&pool, &s.id).await.unwrap().unwrap();
        assert_eq!(r.session.worktree_state, WorktreeState::Active);
        assert_eq!(r.session.worktree_path.as_deref(), Some("/data/wt"));
        // Detach: clear worktree_path, preserve via last_worktree_path.
        set_worktree_state(
            &pool,
            &s.id,
            WorktreeState::Detached,
            None,
            Some("/data/wt"),
        )
        .await
        .unwrap();
        let r = load_session(&pool, &s.id).await.unwrap().unwrap();
        assert_eq!(r.session.worktree_state, WorktreeState::Detached);
        assert!(r.session.worktree_path.is_none());
        assert_eq!(r.session.last_worktree_path.as_deref(), Some("/data/wt"));
        // Delete: both clear.
        set_worktree_state(&pool, &s.id, WorktreeState::None, None, None)
            .await
            .unwrap();
        let r = load_session(&pool, &s.id).await.unwrap().unwrap();
        assert_eq!(r.session.worktree_state, WorktreeState::None);
        assert!(r.session.worktree_path.is_none());
        assert!(r.session.last_worktree_path.is_none());
    }

    #[tokio::test]
    async fn worktree_state_unknown_string_defaults_to_none() {
        // Defensive: a future schema migration may add a new state;
        // older binaries must not crash reading unknown values.
        assert_eq!(WorktreeState::from_str_opt(""), WorktreeState::None);
        assert_eq!(WorktreeState::from_str_opt("nope"), WorktreeState::None);
        assert_eq!(WorktreeState::from_str_opt("active"), WorktreeState::Active);
        assert_eq!(WorktreeState::from_str_opt("detached"), WorktreeState::Detached);
    }

    #[tokio::test]
    async fn worktree_state_backfill_legacy_active() {
        // Simulate a row that existed before the follow-up migration:
        // worktree_path set, worktree_state '' (the column exists
        // with DEFAULT 'none' but the row was inserted before the
        // backfill ran).
        let pool = test_pool().await;
        let sid = Uuid::new_v4().to_string();
        sqlx::query(
            r#"
            INSERT INTO sessions
                (id, title, created_at, updated_at, model, project_id, current_cwd,
                 worktree_path, worktree_state)
            VALUES (?, 'legacy', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z',
                    'GLM-4.7', ?, '/tmp', '/data/legacy_wt', '')
            "#,
        )
        .bind(&sid)
        .bind(DEFAULT_PROJECT_ID)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE sessions SET worktree_state = 'active' WHERE worktree_path IS NOT NULL AND (worktree_state IS NULL OR worktree_state = '')"
        )
        .execute(&pool)
        .await
        .unwrap();
        let reloaded = load_session(&pool, &sid).await.unwrap().unwrap();
        assert_eq!(reloaded.session.worktree_state, WorktreeState::Active);
        assert_eq!(reloaded.session.worktree_path.as_deref(), Some("/data/legacy_wt"));
    }

    #[tokio::test]
    async fn insert_system_event_appends_to_history() {
        let pool = test_pool().await;
        let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7")
            .await
            .unwrap();
        persist_turn(
            &pool,
            &s.id,
            Role::User,
            &MessageContent::Text("hi".into()),
            0,
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
        if let ContentBlock::Text { text } = &blocks[0] {
            assert!(text.contains("[worktree event]"));
            assert!(text.contains("/data/wt"));
        } else {
            panic!("expected text block");
        }
    }

    #[tokio::test]
    async fn insert_system_event_seq_increments() {
        let pool = test_pool().await;
        let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7")
            .await
            .unwrap();
        insert_system_event(&pool, &s.id, "first", "attached").await.unwrap();
        insert_system_event(&pool, &s.id, "second", "detached").await.unwrap();
        let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.messages[0].seq, 0);
        assert_eq!(loaded.messages[1].seq, 1);
    }

    // -----------------------------------------------------------------------
    // PR1 of multi-model task: providers / models / app_config tests
    //
    // Each CRUD function gets a happy path + a forced-error / edge-case
    // test. The "create_session" / "sessions.model_id" interactions are
    // covered separately in the seed test.
    // -----------------------------------------------------------------------

    async fn make_pool() -> SqlitePool {
        test_pool().await // alias for readability inside this section
    }

    #[tokio::test]
    async fn create_provider_then_list_returns_it() {
        let pool = make_pool().await;
        // `test_pool` already ran `run_migrations`, which seeded
        // 2 providers. Add one more and assert it appears in the list
        // (without asserting total count, since the seed counts
        // aren't the test's concern).
        let before = list_providers(&pool).await.unwrap().len();
        let p = create_provider(&pool, "anthropic", "Test provider", "https://api.anthropic.com", "sk-test")
            .await
            .unwrap();
        assert_eq!(p.protocol, "anthropic");
        assert_eq!(p.display_name, "Test provider");
        assert!(!p.id.is_empty());
        let list = list_providers(&pool).await.unwrap();
        assert_eq!(list.len(), before + 1);
        assert!(list.iter().any(|row| row.id == p.id));
    }

    #[tokio::test]
    async fn update_provider_on_missing_id_returns_none() {
        let pool = make_pool().await;
        let res = update_provider(
            &pool,
            "00000000-0000-0000-0000-000000000000",
            "openai",
            "ghost",
            "https://example.com",
            "sk-ghost",
        )
        .await
        .unwrap();
        assert!(res.is_none());
    }

    #[tokio::test]
    async fn delete_provider_cascades_to_models() {
        let pool = make_pool().await;
        let p = create_provider(&pool, "openai", "OpenAI 官方 (test)", "https://api.openai.com/v1", "")
            .await
            .unwrap();
        let m = create_model(
            &pool,
            &p.id,
            "gpt-4o-test",
            "GPT-4o (test)",
            None,
            None,
            false,
            128_000,
        )
        .await
        .unwrap();
        assert!(list_models(&pool).await.unwrap().iter().any(|mwp| mwp.model.id == m.id));
        assert!(delete_provider(&pool, &p.id).await.unwrap());
        // Cascade FK should have removed the model.
        assert!(!list_models(&pool).await.unwrap().iter().any(|mwp| mwp.model.id == m.id));
        assert!(!delete_model(&pool, &m.id).await.unwrap());
    }

    #[tokio::test]
    async fn create_model_then_list_joins_provider_fields() {
        let pool = make_pool().await;
        let p = create_provider(&pool, "anthropic", "Anthropic 官方 (test)", "https://api.anthropic.com", "")
            .await
            .unwrap();
        let m = create_model(
            &pool,
            &p.id,
            "claude-sonnet-4-5-test",
            "Claude Sonnet 4.5 (test)",
            Some(8192),
            Some("high"),
            true,
            200_000,
        )
        .await
        .unwrap();
        let list = list_models(&pool).await.unwrap();
        let mwp = list
            .iter()
            .find(|x| x.model.id == m.id)
            .expect("test model in list");
        assert_eq!(mwp.model.model_name, "claude-sonnet-4-5-test");
        assert_eq!(mwp.model.max_tokens, Some(8192));
        assert_eq!(mwp.model.thinking_effort.as_deref(), Some("high"));
        assert!(mwp.model.supports_thinking);
        assert_eq!(mwp.model.context_window, 200_000);
        assert_eq!(mwp.provider_display_name, "Anthropic 官方 (test)");
        assert_eq!(mwp.provider_protocol, "anthropic");
    }

    #[tokio::test]
    async fn update_model_on_missing_id_returns_none() {
        let pool = make_pool().await;
        let res = update_model(
            &pool,
            "00000000-0000-0000-0000-000000000000",
            "p",
            "gpt-4o",
            "GPT-4o",
            None,
            None,
            false,
            128_000,
        )
        .await
        .unwrap();
        assert!(res.is_none());
    }

    #[tokio::test]
    async fn delete_model_on_missing_id_returns_false() {
        let pool = make_pool().await;
        let res = delete_model(&pool, "00000000-0000-0000-0000-000000000000")
            .await
            .unwrap();
        assert!(!res);
    }

    #[tokio::test]
    async fn default_model_is_set_by_seed() {
        // The seed function runs as part of run_migrations; we
        // assert the contract that `default_model_id` is set AND
        // points at a real model row.
        let pool = make_pool().await;
        let id = get_config_value(&pool, "default_model_id").await.unwrap();
        let id = id.expect("default_model_id set by seed");
        let list = list_models(&pool).await.unwrap();
        assert!(list.iter().any(|mwp| mwp.model.id == id));
    }

    #[tokio::test]
    async fn set_then_get_config_value_round_trips() {
        let pool = make_pool().await;
        // `default_model_id` is already set by the seed; we use
        // a custom key to avoid clobbering.
        set_config_value(&pool, "test_key", "abc-123").await.unwrap();
        let res = get_config_value(&pool, "test_key").await.unwrap();
        assert_eq!(res.as_deref(), Some("abc-123"));
        // Overwrite.
        set_config_value(&pool, "test_key", "xyz-789").await.unwrap();
        let res = get_config_value(&pool, "test_key").await.unwrap();
        assert_eq!(res.as_deref(), Some("xyz-789"));
    }

    #[tokio::test]
    async fn seed_is_idempotent_and_inserts_defaults() {
        let pool = make_pool().await;
        // First call is a no-op because run_migrations already invoked
        // the seed; call again to prove idempotency (no duplicate
        // rows).
        let before_p = list_providers(&pool).await.unwrap().len();
        let before_m = list_models(&pool).await.unwrap().len();
        seed_default_providers_and_models(&pool).await.unwrap();
        assert_eq!(list_providers(&pool).await.unwrap().len(), before_p);
        assert_eq!(list_models(&pool).await.unwrap().len(), before_m);
    }

    #[tokio::test]
    async fn seed_backfills_sessions_model_id() {
        // Build a fresh DB that mirrors a pre-PR1 state: only the
        // pre-PR1 tables exist (projects / sessions / messages),
        // no providers/models/app_config yet. Insert a legacy
        // sessions row with `model_id IS NULL`, then call
        // `seed_default_providers_and_models` and assert the
        // backfill query sets `model_id` on the legacy row.
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE projects (
                id TEXT PRIMARY KEY, name TEXT NOT NULL, path TEXT NOT NULL,
                is_git_repo INTEGER NOT NULL DEFAULT 0, is_legacy INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
                hidden INTEGER NOT NULL DEFAULT 0, metadata TEXT
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE sessions (
                id TEXT PRIMARY KEY, title TEXT NOT NULL,
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
                model TEXT NOT NULL, metadata TEXT,
                project_id TEXT NOT NULL DEFAULT '__default__',
                current_cwd TEXT NOT NULL DEFAULT '',
                worktree_path TEXT,
                worktree_state TEXT NOT NULL DEFAULT 'none',
                last_worktree_path TEXT,
                model_id TEXT
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE providers (
                id TEXT PRIMARY KEY, protocol TEXT NOT NULL,
                display_name TEXT NOT NULL, base_url TEXT NOT NULL,
                api_key TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE models (
                id TEXT PRIMARY KEY, provider_id TEXT NOT NULL,
                model_name TEXT NOT NULL, display_name TEXT NOT NULL,
                max_tokens INTEGER, thinking_effort TEXT,
                supports_thinking INTEGER NOT NULL DEFAULT 0,
                context_window INTEGER NOT NULL,
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE app_config (
                key TEXT PRIMARY KEY, value TEXT NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO sessions (id, title, created_at, updated_at, model) \
             VALUES ('s1', 't', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 'claude-sonnet-4-5')"
        )
        .execute(&pool)
        .await
        .unwrap();
        // Now call the seed directly; it inserts providers/models
        // + sets default_model_id, then backfills sessions.model_id.
        seed_default_providers_and_models(&pool).await.unwrap();
        let row: String = sqlx::query("SELECT model_id FROM sessions WHERE id = 's1'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .try_get("model_id")
            .unwrap();
        assert!(!row.is_empty(), "model_id should be backfilled");
        // The default model id should match the backfilled value.
        let default_id = get_config_value(&pool, "default_model_id").await.unwrap();
        assert_eq!(row, default_id.expect("default set"));
    }

    #[tokio::test]
    async fn delete_provider_cascade_does_not_touch_unrelated_models() {
        let pool = make_pool().await;
        let p1 = create_provider(&pool, "anthropic", "P1 (cascade test)", "https://a.example.com", "")
            .await
            .unwrap();
        let p2 = create_provider(&pool, "openai", "P2 (cascade test)", "https://b.example.com", "")
            .await
            .unwrap();
        let m1 = create_model(&pool, &p1.id, "m1-cascade-test", "M1", None, None, false, 100_000)
            .await
            .unwrap();
        let m2 = create_model(&pool, &p2.id, "m2-cascade-test", "M2", None, None, false, 100_000)
            .await
            .unwrap();
        let list = list_models(&pool).await.unwrap();
        assert!(list.iter().any(|mwp| mwp.model.id == m1.id));
        assert!(list.iter().any(|mwp| mwp.model.id == m2.id));
        delete_provider(&pool, &p1.id).await.unwrap();
        let remaining = list_models(&pool).await.unwrap();
        assert!(!remaining.iter().any(|mwp| mwp.model.id == m1.id));
        assert!(remaining.iter().any(|mwp| mwp.model.id == m2.id));
    }

    // -----------------------------------------------------------------------
    // PR4 of multi-model task: update_session_model_id + load_session
    // model_id field tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn update_session_model_id_sets_and_clears() {
        let pool = make_pool().await;
        let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7")
            .await
            .unwrap();
        // New session: model_id is NULL (falls back to global default).
        let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
        assert!(loaded.session.model_id.is_none());

        // Set to a specific model.
        let p = create_provider(&pool, "anthropic", "Test (model_id)", "https://api.anthropic.com", "sk-test")
            .await
            .unwrap();
        let m = create_model(&pool, &p.id, "test-model", "Test Model", None, None, false, 100_000)
            .await
            .unwrap();
        update_session_model_id(&pool, &s.id, &m.id).await.unwrap();
        let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
        assert_eq!(loaded.session.model_id.as_deref(), Some(m.id.as_str()));

        // Clear by passing empty string.
        update_session_model_id(&pool, &s.id, "").await.unwrap();
        let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
        assert!(loaded.session.model_id.is_none());
    }

    #[tokio::test]
    async fn update_session_model_id_on_missing_session_is_noop() {
        let pool = make_pool().await;
        // Should not error — the UPDATE simply matches 0 rows.
        update_session_model_id(&pool, "nonexistent-session-id", "some-model-id")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn load_session_includes_model_id() {
        let pool = make_pool().await;
        let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7")
            .await
            .unwrap();
        // Directly set model_id in the DB to verify the SELECT picks it up.
        let p = create_provider(&pool, "anthropic", "Test (model_id select)", "https://api.anthropic.com", "sk-test")
            .await
            .unwrap();
        let m = create_model(&pool, &p.id, "select-test-model", "Select Test Model", None, None, false, 100_000)
            .await
            .unwrap();
        sqlx::query("UPDATE sessions SET model_id = ? WHERE id = ?")
            .bind(&m.id)
            .bind(&s.id)
            .execute(&pool)
            .await
            .unwrap();
        let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
        assert_eq!(loaded.session.model_id.as_deref(), Some(m.id.as_str()));
    }
}
