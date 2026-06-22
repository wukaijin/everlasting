//! Pool init + schema migrations.
//!
//! `init_pool` opens (or creates) the SQLite file at `db_path` and
//! returns a `sqlx::SqlitePool`. `run_migrations` is idempotent and
//! safe to call on every startup; it ensures the schema is at the
//! current shape regardless of whether the DB is fresh or upgraded
//! from an older version. The per-table column-probe helpers
//! (`add_session_column_if_missing` etc.) handle the non-destructive
//! ALTER step for any new column added in a later release.

use std::path::Path;

use chrono::Utc;
use sqlx::{Row, SqlitePool};

use crate::projects::DEFAULT_PROJECT_ID;

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
///3b-1 ALTERs that add `project_id` / `current_cwd` to `sessions`.
/// Idempotent — safe to call on every startup.
pub async fn run_migrations(pool: &SqlitePool) -> Result<(), sqlx::Error> {
 // --- projects (new in3b-1) ---
 sqlx::query(
 r#"
 CREATE TABLE IF NOT EXISTS projects (
 id TEXT PRIMARY KEY,
 name TEXT NOT NULL,
 path TEXT NOT NULL,
        is_git_repo INTEGER NOT NULL DEFAULT 0,
 is_legacy INTEGER NOT NULL DEFAULT 0,
 created_at TEXT NOT NULL,
 updated_at TEXT NOT NULL,
 hidden INTEGER NOT NULL DEFAULT 0,
 metadata TEXT
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
 //3b-1 columns yet, so we add them lazily below) ---
 sqlx::query(
 r#"
 CREATE TABLE IF NOT EXISTS sessions (
 id TEXT PRIMARY KEY,
 title TEXT NOT NULL,
 created_at TEXT NOT NULL,
 updated_at TEXT NOT NULL,
 model TEXT NOT NULL,
 metadata TEXT
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

 // ---3b-1 ALTERs: add project_id / current_cwd to sessions.
 // We probe for column existence first so the migration is
 // idempotent across a fresh DB and an upgraded DB. ---
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

 // --- Step4 ALTER: add worktree_path to sessions.
 // Nullable (no DEFAULT) so pre-step4 rows keep NULL and the
 // Rust side falls back to `current_cwd` for them. New step4
 // rows always have a value (the create_session call returns
 // an error before the INSERT if worktree creation fails). ---
 add_session_column_if_missing(pool, "worktree_path", "TEXT").await?;

 // --- Step4 follow-up: opt-in worktree (auto-create → manual
 // attach/detach/delete). Adds the tri-state `worktree_state`
 // column (default 'none') and `last_worktree_path` for
 // detached sessions.
 //
 // Backfill: sessions that have `worktree_path IS NOT NULL`
 // AND `worktree_state IS NULL` are pre-follow-up rows that
 // were created under the old auto-create flow. They were
 // effectively "active" at the time of creation, so we mark
 // them as 'active' here. This matches the PR1 / PR2 spirit
 // of the git-metadata backfill: idempotent, fire-and-forget,
 // and run after the column add. ---
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
 // `is_git_repo` already exists on freshly created tables (see
 // CREATE TABLE above) so the idempotent probe is a no-op for
 // greenfield DBs. Older pre-3b-1 databases may have a
 // `projects` table without these columns; the probe + ALTER
 // brings them up to date. ---
 add_project_column_if_missing(pool, "is_git_repo", "INTEGER NOT NULL DEFAULT 0").await?;
 add_project_column_if_missing(pool, "git_branch", "TEXT").await?;

 // --- messages ---
 sqlx::query(
 r#"
 CREATE TABLE IF NOT EXISTS messages (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
 role TEXT NOT NULL,
 content TEXT NOT NULL,
 text TEXT NOT NULL,
 has_tool_calls INTEGER NOT NULL DEFAULT 0,
 has_tool_results INTEGER NOT NULL DEFAULT 0,
 created_at TEXT NOT NULL,
 seq INTEGER NOT NULL,
 metadata TEXT,
 UNIQUE(session_id, seq)
 )
 "#,
 )
 .execute(pool)
 .await?;
 // Step4 follow-up: add `metadata` column for system events.
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
 // The `providers` table is the user-managed catalog of LLM
 // endpoints (Anthropic官方, 第三方Anthropic-compat, OpenAI官方, ...);
 // multiple rows may share the same `protocol`. `models` binds
 // model names to a provider with capability hints and per-row
 // overrides for `max_tokens` / `thinking_effort`. `app_config`
 // is a small key/value store; the only key written today is
 // `default_model_id`, but the table is generic so future global
 // settings don't need a new migration. ---
 sqlx::query(
 r#"
 CREATE TABLE IF NOT EXISTS providers (
 id TEXT PRIMARY KEY,
 protocol TEXT NOT NULL,
 display_name TEXT NOT NULL,
 base_url TEXT NOT NULL,
 api_key TEXT NOT NULL DEFAULT '',
 created_at TEXT NOT NULL,
 updated_at TEXT NOT NULL
 )
 "#,
 )
 .execute(pool)
 .await?;
 sqlx::query(
 r#"
 CREATE TABLE IF NOT EXISTS models (
 id TEXT PRIMARY KEY,
 provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
 model_name TEXT NOT NULL,
 display_name TEXT NOT NULL,
 max_tokens INTEGER,
 thinking_effort TEXT,
 supports_thinking INTEGER NOT NULL DEFAULT 0,
 context_window INTEGER NOT NULL,
 created_at TEXT NOT NULL,
 updated_at TEXT NOT NULL
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
 key TEXT PRIMARY KEY,
 value TEXT NOT NULL
 )
 "#,
 )
 .execute(pool)
 .await?;

 // --- PR1 of multi-model task: add `model_id` to sessions.
 // Nullable FK to `models.id`. Pre-PR1 sessions have NULL; the
 // seed function below backfills them with the default model.
 // Kept as a soft FK (no FK constraint) so a future row with a
 // dangling `model_id` (e.g. legacy dump) doesn't break INSERTs. ---
 add_session_column_if_missing(pool, "model_id", "TEXT").await?;
 sqlx::query(
 r#"
 CREATE INDEX IF NOT EXISTS idx_sessions_model_id
 ON sessions(model_id)
 "#,
 )
 .execute(pool)
 .await?;

 // --- A4 (Token Usage Tracking): per-session token totals.
 //
 // Four nullable INTEGER columns. Nullable (no DEFAULT) so
 // pre-A4 sessions keep NULL — the frontend renders NULL as
 // "—" (the "升级前未统计" tooltip path). The agent loop
 // accumulates via `UPDATE col = col + ?` on every LLM turn
 // Done (see `db::sessions::add_token_usage`); a single
 // session can record N turns, the column is the cumulative
 // sum.
 //
 // Field semantics (mirror `llm::types::TokenUsage`):
 // - `input_tokens_total`: sum of per-turn `input_tokens`
 //   (Anthropic: inclusive of cache_creation + cache_read;
 //    this is the "current context usage" the ChatInput hint
 //    displays as percentage of `models.context_window`).
 // - `output_tokens_total`: sum of per-turn `output_tokens`
 //   (the response, not the context).
 // - `cache_creation_total`: sum of
 //   `cache_creation_input_tokens` (Anthropic only; OpenAI
 //   reports 0 here today).
 // - `cache_read_total`: sum of `cache_read_input_tokens`
 //   (Anthropic + OpenAI's `cached_tokens`).
 add_session_column_if_missing(pool, "input_tokens_total", "INTEGER").await?;
 add_session_column_if_missing(pool, "output_tokens_total", "INTEGER").await?;
 add_session_column_if_missing(pool, "cache_creation_total", "INTEGER").await?;
 add_session_column_if_missing(pool, "cache_read_total", "INTEGER").await?;

 // --- D1 (Session Rename + Color Tag): per-session color mark.
 // Nullable INTEGER, 0-7 = palette index, NULL = no mark.
 add_session_column_if_missing(pool, "color_tag", "INTEGER").await?;

 // --- F5 (LLM Latency Tracking): per-message latency breakdown.
 //
 // Three nullable INTEGER columns on `messages`. Nullable (no
 // DEFAULT) so pre-F5 rows keep NULL — the UI renders NULL as
 // "—" with the "升级前未统计" tooltip (mirrors the A4 chat-input
 // hint UX). The frontend `streamController` measures the three
 // values via `Date.now()` deltas around the `start` / first
 // `delta` / `done` events of each chat invocation, then issues
 // a new IPC (`update_message_latency`) at stream end to persist
 // them. Tool-call duration follows the same in-memory pattern
 // but lives in the `messages.content` JSON, not as a column —
 // see `db::sessions::record_tool_duration`.
 //
 // Field semantics (mirror the frontend `LatencyInfo`):
 // - `ttfb_ms`: time-to-first-byte (send → first `delta` event)
 // - `gen_ms`:  generation time (first `delta` → `done`)
 // - `total_ms`: end-to-end (`send` → `done`)
 // - `tool duration` lives inside the `tool_result` content block
 //   (per R2 / PRD decision 1) and is patched via the
 //   `record_tool_duration` IPC. Zero schema change for that.
 add_messages_column_if_missing(pool, "ttfb_ms", "INTEGER").await?;
 add_messages_column_if_missing(pool, "gen_ms", "INTEGER").await?;
 add_messages_column_if_missing(pool, "total_ms", "INTEGER").await?;

 // --- F5 follow-up: thinking-phase timing.
 //
 // One nullable INTEGER column on `messages`. The frontend
 // `streamController` measures the thinking-phase wall-clock
 // (first `thinking_delta` → first non-thinking boundary:
 // text `delta`, `tool:call` IPC, `done`, or `error`) and
 // issues a new IPC (`update_message_thinking`) at stream
 // end to persist it. NULL for messages that never entered
 // the thinking phase — the UI renders NULL as "—" in the
 // ThinkingBlock header. Schema-aligned with the three
 // latency columns above: nullable INTEGER, no DEFAULT, no
 // non-null upgrade path (pre-F5-follow-up rows stay NULL
 // forever, which is the correct semantic — there's no
 // retroactive way to measure how long a past turn spent
 // thinking).
 add_messages_column_if_missing(pool, "thinking_ms", "INTEGER").await?;

 // --- A2 + B7 (Permission system + per-session Mode, 2026-06-13).
 //
 // Per-session Mode binding (`sessions.mode TEXT`), persistent
 // 3 档 mode: `edit` / `plan` / `yolo`. Nullable (no
 // DEFAULT) so pre-A2 sessions keep NULL; the backfill below
 // writes `'edit'` for any NULL row. Pattern mirrors the
 // worktree_state / model_id migrations — additive, idempotent.
 //
 // Two new tables: `session_tool_permissions` (per-session
 // "always allow" set, indexed by tool_name + match_kind) and
 // `session_audit_events` (the audit log; one row per
 // decision path hit). Both use `ON DELETE CASCADE` so
 // deleting a session cleans up its permission grants and
 // audit trail — requires `PRAGMA foreign_keys = ON` which
 // `init_pool` sets on first connection (see line 46).
 //
 // 2026-06-13 3 档化: drop Review, rename Chat→Edit (ADR in
 // IMPLEMENTATION.md §4). The `'chat'` / `'review'` backfill
 // below the v5 migration rewrites historical rows; both
 // UPDATE statements are idempotent (re-running on already-
 // migrated rows is a no-op).
 add_session_column_if_missing(pool, "mode", "TEXT").await?;
 sqlx::query(
 r#"
 UPDATE sessions SET mode = 'edit' WHERE mode IS NULL
 "#,
 )
 .execute(pool)
 .await?;

 sqlx::query(
 r#"
 CREATE TABLE IF NOT EXISTS session_tool_permissions (
 session_id TEXT NOT NULL,
 tool_name TEXT NOT NULL,
 match_kind TEXT NOT NULL CHECK (match_kind IN ('tool','prefix','path')),
 match_value TEXT,
 granted_at TEXT NOT NULL DEFAULT (datetime('now')),
 PRIMARY KEY (session_id, tool_name, match_kind, match_value),
 FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
 )
 "#,
 )
 .execute(pool)
 .await?;
 sqlx::query(
 r#"
 CREATE INDEX IF NOT EXISTS idx_session_tool_permissions_session
 ON session_tool_permissions(session_id, tool_name)
 "#,
 )
 .execute(pool)
 .await?;

 sqlx::query(
 r#"
 CREATE TABLE IF NOT EXISTS session_audit_events (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 session_id TEXT NOT NULL,
 ts TEXT NOT NULL DEFAULT (datetime('now')),
 kind TEXT NOT NULL,
 payload_json TEXT,
 FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
 )
 "#,
 )
 .execute(pool)
 .await?;
 sqlx::query(
 r#"
 CREATE INDEX IF NOT EXISTS idx_session_audit_events_session_ts
 ON session_audit_events(session_id, ts DESC)
 "#,
 )
 .execute(pool)
 .await?;

 // --- 2026-06-13 v6: Mode 3 档化 backfill
 // (rename Chat→Edit, drop Review→Plan). Idempotent: re-running on
 // a fully-migrated DB is a no-op (the LHS values no longer exist).
 // `review` → `plan` (R1 in 06-13 grill-with-docs decision: keep the
 // "read-only" behavior, which Plan implements). `chat` → `edit`
 // (Chat variant renamed to Edit, behavior unchanged). ---
 sqlx::query(
 r#"
 UPDATE sessions SET mode = 'plan' WHERE mode = 'review'
 "#,
 )
 .execute(pool)
 .await?;
 sqlx::query(
 r#"
 UPDATE sessions SET mode = 'edit' WHERE mode = 'chat'
 "#,
 )
 .execute(pool)
 .await?;

 // --- B6 PR2 (2026-06-20): subagent_runs persistence.
 //
 // Worker subagents (`dispatch_subagent` tool) accumulate their
 // chat-events / tool calls / tool results in a `SubagentBufferSink`
 // transcript. PR2 persists that transcript to `subagent_runs` so:
 // (1) PR3's ToolCallCard expand UI can render what the worker
 // did, (2) a session reload after a parent restart still shows
 // the worker's intermediate state, (3) token-usage aggregation
 // is auditable per-run.
 //
 // Schema design (follows `session_audit_events` precedent —
 // `parent_session_id` FK CASCADE, indexed ts DESC, RFC 3339
 // timestamps):
 // - `id` is a nanoid (UUID v4 form, matches the rest of the DB)
 // - `parent_session_id` FK CASCADE → `sessions(id)`; deleting a
 //   session cleans up all its worker subagent_runs in one shot
 //   (the CASCADE requires `PRAGMA foreign_keys = ON` which
 //   `init_pool` sets on first connection).
 // - `parent_request_id` = the worker rid (the
 //   `"{parent_rid}-sub-{tool_use_id}"` string the agent loop
 //   builds at `chat_loop.rs:1989`). NOT a FK — `cancellations`
 //   is in-memory, not durable.
 // - `status` is a CHECK-constrained TEXT column with 4 values
 //   (`running` / `completed` / `cancelled` / `error`); INSERT
 //   always sets `running`, UPDATE on worker exit sets the
 //   terminal value. `running` rows are the "in-flight" set a
 //   future PR could surface as "5 workers active" badges.
 // - `started_at` is set on INSERT; `finished_at` is NULL
 //   while running, set on UPDATE.
 // - `token_usage_json` is a JSON-encoded `TokenUsage`
 //   (`{ input, output, cache_creation, cache_read }`). NULL
 //   while running; non-NULL after the worker exits.
 // - `summary` is the worker's `final_text` plain string
 //   (NO status prefix — the `status` column carries that
 //   separately, so PR3's UI can render the prefix without
 //   parsing the summary). NULL while running.
 // - `transcript_json` is the serialized
 //   `Vec<TranscriptEntry>` from `SubagentBufferSink`. NULL
 //   while running; non-NULL on UPDATE. Capped at 4MB by
 //   `truncate_transcript_for_persistence` (see
 //   `agent/subagent.rs`); the `transcript_truncated=1` flag
 //   signals truncation so PR3 can render a "show full" affordance
 //   to fetch the full text from elsewhere (or document the cap).
 // --- 2026-06-21 (subagent incomplete status): widen the
 // `subagent_runs.status` CHECK constraint to include
 // `'incomplete'`. The pre-existing constraint was set in B6 PR2
 // (`'running','completed','cancelled','error'`). This task adds
 // a 5th variant for the max_turns soft-terminal path
 // (worker hit its 200-turn budget without cleanly finishing).
 //
 // SQLite cannot ALTER a CHECK constraint in place — the
 // `widen_subagent_runs_status_check_for_incomplete` helper
 // uses the table-rebuild pattern (rename, create new, copy,
 // drop, re-index) gated on a probe of `sqlite_master.sql` for
 // the literal `'incomplete'` so the migration is idempotent
 // (a re-run on a dev DB that already has the widened
 // constraint is a no-op).
 widen_subagent_runs_status_check_for_incomplete(pool).await?;
 sqlx::query(
 r#"
 CREATE INDEX IF NOT EXISTS idx_subagent_runs_session_started
 ON subagent_runs(parent_session_id, started_at DESC)
 "#,
 )
 .execute(pool)
 .await?;
 sqlx::query(
 r#"
 CREATE INDEX IF NOT EXISTS idx_subagent_runs_request
 ON subagent_runs(parent_request_id)
 "#,
 )
 .execute(pool)
 .await?;

 // --- 2026-06-21 (subagent drawer redesign PR1): task + final_text.
 //
 // Two new TEXT columns on `subagent_runs`. Both nullable (no
 // DEFAULT) so pre-PR1 rows keep NULL — the UI renders NULL as
 // "—" with the same "升级前未统计" tooltip pattern used elsewhere
 // (mirrors A4 chat-input hint UX; mirrors F5 latency NULL
 // handling).
 //
 // - `task` is the LLM's delegation prompt as supplied to
 //   `dispatch_subagent(input.task)`. Written once at
 //   `run_subagent` dispatch time (best-effort warn+continue on
 //   DB failure). NULL if `insert_run` itself failed (the row
 //   won't exist at all in that case).
 // - `final_text` is the worker's terminal assistant text with
 //   the `[status: ...]\n` prefix **stripped** — `status` is the
 //   source of truth for the prefix (per the existing `summary`
 //   field contract; `subagent_runs-schema.md` §3 "`update_run_finished`
 //   行为"). The PRD splits `summary` (kept for backward compat
 //   + the "summary" wire field) from `final_text` (the
 //   drawer's `finalText` consumer-facing field).
 //
 // The split lets the PR2 frontend wire `final_text` → drawer
 // Reply segment while keeping `summary` as the legacy wire
 // field unchanged. Existing rows (`status='completed'` from
 // pre-PR1) keep `final_text=NULL`; PR3's drawer reads
 // `final_text` first and falls back to `summary` for legacy
 // rows. Future maintenance can backfill if needed.
 //
 // Idempotent: re-running on a pre-PR1 DB brings it up to date;
 // re-running on a post-PR1 DB is a no-op (the column exists).
 add_subagent_runs_column_if_missing(pool, "task", "TEXT").await?;
 add_subagent_runs_column_if_missing(pool, "final_text", "TEXT").await?;

 // --- 2026-06-22 (RULE-FrontSubagent-004): turn_count column.
 //
 // One new nullable INTEGER column on `subagent_runs`: the actual
 // number of completed LLM turn iterations the worker executed
 // before reaching its terminal state (completed / cancelled /
 // error / incomplete). NULL on pre-PR2 rows (the column didn't
 // exist); the drawer degrades to the wall-clock suffix for those
 // legacy rows (AC: "stopped at X.Xs" for NULL turn_count).
 //
 // - Nullable (no DEFAULT) — pre-PR2 rows keep NULL and the UI
 //   falls back to wall-clock. The production chat.rs / run_subagent
 //   path writes `Some(turns)` on every post-PR2 terminal UPDATE.
 // - `INTEGER` matches the project's convention for numeric
 //   columns (sqlx derives `i64` on read; the Row struct maps it
 //   to `Option<i64>`). NOT a boolean; NOT a TEXT enum.
 // - Not the SUBAGENT_MAX_TURNS=200 constant — that's the budget
 //   ceiling; `turn_count` is how many turns were actually
 //   executed (which may be < 200 on clean completion / cancel /
 //   error, or == 200 on the incomplete soft-cap exit).
 //
 // Idempotent: re-running on a pre-PR2 DB brings it up to date;
 // re-running on a post-PR2 DB is a no-op (the column exists).
 add_subagent_runs_column_if_missing(pool, "turn_count", "INTEGER").await?;

 // --- PR1 of multi-model task: seed default providers + models
 // if the catalog is empty. Idempotent:0-row check skips the
 // insert on subsequent boots. Backfills `sessions.model_id`
 // for any row still NULL after the ALTER. ---
 super::config::seed_default_providers_and_models(pool).await?;

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
 VALUES (?, ?, ?,0, NULL,1, ?, ?,0, NULL)
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
/// doesn't have `ALTER TABLE ... ADD COLUMN IF NOT EXISTS` in3.35
/// reliably (and the underlying error code is `1` for "duplicate
/// column"), so we probe `PRAGMA table_info` first.
pub(crate) async fn add_session_column_if_missing(
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
pub(crate) async fn add_project_column_if_missing(
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
pub(crate) async fn add_messages_column_if_missing(
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

/// Add a column to `subagent_runs` if it doesn't already exist.
/// Mirrors [`add_session_column_if_missing`]. Added for the
/// 2026-06-21 subagent-drawer redesign PR1 (`task` + `final_text`).
pub(crate) async fn add_subagent_runs_column_if_missing(
 pool: &SqlitePool,
 column: &str,
 decl: &str,
) -> Result<(), sqlx::Error> {
 let exists: i64 =
 sqlx::query("SELECT COUNT(*) FROM pragma_table_info('subagent_runs') WHERE name = ?")
 .bind(column)
 .fetch_one(pool)
 .await?
 .try_get(0)?;
 if exists == 0 {
 let stmt = format!("ALTER TABLE subagent_runs ADD COLUMN {} {}", column, decl);
 sqlx::query(&stmt).execute(pool).await?;
 }
 Ok(())
}

/// Widen the `subagent_runs.status` CHECK constraint to include
/// `'incomplete'` (the 2026-06-21 max_turns soft-terminal variant).
/// SQLite has no `ALTER TABLE ... DROP CONSTRAINT` /
/// `ALTER TABLE ... ADD CONSTRAINT` for CHECK expressions, so the
/// only reliable way to widen the constraint is the 12-step
/// table-rebuild pattern:
///
/// 1. Rename `subagent_runs` → `subagent_runs_old`.
/// 2. `CREATE TABLE subagent_runs (...)` with the wider CHECK.
/// 3. `INSERT INTO subagent_runs SELECT * FROM subagent_runs_old`.
/// 4. `DROP TABLE subagent_runs_old`.
/// 5. Re-create the two indexes (`idx_subagent_runs_session_started` +
///    `idx_subagent_runs_request`) — they were not transferred by
///    the rebuild because they were attached to `_old`.
///
/// **Idempotency**: the function probes `sqlite_master.sql` for
/// the literal `'incomplete'` in the `subagent_runs` CREATE
/// statement. If it's already there, the function returns
/// `Ok(())` without rebuilding. A re-run on a dev DB that
/// already has the widened constraint is therefore a no-op.
///
/// **FK safety**: this function does NOT toggle
/// `PRAGMA foreign_keys`. The standard 12-step pattern requires
/// `PRAGMA foreign_keys=OFF` because the rebuild temporarily
/// creates a window where FK references could fire (e.g. if
/// some other table referenced `subagent_runs`). The
/// `subagent_runs` table has NO outgoing FK references (the
/// only FK is `parent_session_id REFERENCES sessions(id)` on
/// the column itself, which keeps pointing at the same
/// `sessions` rows throughout the rebuild) and NO incoming FK
/// references (no other table references `subagent_runs`).
/// Toggling `PRAGMA foreign_keys=OFF` is therefore unnecessary,
/// and skipping it avoids polluting the per-connection pragma
/// state of the test pool (which uses multiple connections —
/// setting the pragma on one connection doesn't propagate to
/// the others, and the test pool's `PRAGMA foreign_keys=ON` is
/// per-connection, so a toggle on one connection can leave
/// other connections in an inconsistent state across
/// concurrently-running tests).
pub(crate) async fn widen_subagent_runs_status_check_for_incomplete(
 pool: &SqlitePool,
) -> Result<(), sqlx::Error> {
 // Probe the live CREATE statement for the `incomplete` literal.
 // A re-run on a dev DB that already has the widened CHECK sees
 // the literal and short-circuits.
 let sql_row: Option<String> = sqlx::query_scalar(
 "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'subagent_runs'",
 )
 .fetch_optional(pool)
 .await?;
 let already_widened = sql_row
 .as_deref()
 .map(|s| s.contains("'incomplete'"))
 .unwrap_or(false);
 if already_widened {
 return Ok(());
 }

 // Probe failed (table missing entirely or constraint narrow).
 // Rebuild via the table-rebuild dance. Both are no-ops when the
 // condition doesn't apply.
 // Step 1: rename existing.
 sqlx::query("ALTER TABLE subagent_runs RENAME TO subagent_runs_old")
 .execute(pool)
 .await
 .ok(); // benign if the table doesn't exist
 // Step 2: create the widened table.
 sqlx::query(
 r#"
 CREATE TABLE subagent_runs (
 id TEXT PRIMARY KEY,
 parent_session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
 parent_request_id TEXT NOT NULL,
 subagent_name TEXT NOT NULL,
 status TEXT NOT NULL CHECK(status IN ('running','completed','cancelled','error','incomplete')),
 started_at TEXT NOT NULL,
 finished_at TEXT,
 token_usage_json TEXT,
 summary TEXT,
 transcript_json TEXT,
 transcript_truncated INTEGER NOT NULL DEFAULT 0,
 created_at TEXT NOT NULL DEFAULT (datetime('now'))
 )
 "#,
 )
 .execute(pool)
 .await?;
 // Step 3: copy rows from the old table (if it exists). The
 // SELECT * works because the new table has the same column
 // set, only the CHECK constraint differs.
 sqlx::query(
 r#"
 INSERT INTO subagent_runs
 SELECT * FROM subagent_runs_old
 "#,
 )
 .execute(pool)
 .await
 .ok(); // benign if the old table didn't exist
 // Step 4: drop the old table.
 sqlx::query("DROP TABLE subagent_runs_old")
 .execute(pool)
 .await
 .ok();
 // Step 5: re-create the two indexes.
 sqlx::query(
 r#"
 CREATE INDEX IF NOT EXISTS idx_subagent_runs_session_started
 ON subagent_runs(parent_session_id, started_at DESC)
 "#,
 )
 .execute(pool)
 .await?;
 sqlx::query(
 r#"
 CREATE INDEX IF NOT EXISTS idx_subagent_runs_request
 ON subagent_runs(parent_request_id)
 "#,
 )
 .execute(pool)
 .await?;
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
