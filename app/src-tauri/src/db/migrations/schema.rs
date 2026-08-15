//! Idempotent schema bootstrap — the full migration sequence.
//!
//! Split out of `db/migrations.rs` (2026-08-08 batch3).

use chrono::Utc;
use sqlx::SqlitePool;

use crate::projects::DEFAULT_PROJECT_ID;

use super::columns::{
    add_autonomous_memories_column_if_missing, add_messages_column_if_missing,
    add_project_column_if_missing, add_provider_column_if_missing,
    add_session_audit_events_column_if_missing, add_session_column_if_missing,
    add_subagent_runs_column_if_missing, add_turn_trace_column_if_missing,
};
use super::schema_helpers::{
    home_dir_or_dot, migrate_provider_api_keys_to_encrypted,
    widen_subagent_runs_status_check_for_incomplete,
};

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
    add_session_column_if_missing(pool, "worktree_state", "TEXT NOT NULL DEFAULT 'none'").await?;
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

    // --- 2026-06-26 (token-usage snapshot fix): per-session LAST-TURN
    // snapshot columns.
    //
    // Five nullable INTEGER columns. Nullable (no DEFAULT) so
    // pre-snapshot sessions keep NULL — the frontend renders NULL
    // as "—" (the "升级前未统计" fallback). The agent loop OVERWRITES
    // these on every LLM turn Done via `update_last_turn_usage`
    // (single `UPDATE col = ?`, NOT `col = col + ?` — the value is
    // a per-request snapshot, not a cumulative sum).
    //
    // Replaces the A4 cumulative-accumulator model. The four
    // `*_total` columns above are FROZEN — kept in the schema for
    // non-destructive migration but no longer written by production
    // code; they remain readable for legacy UIs / debt cleanup.
    //
    // Field semantics (mirror `llm::types::TokenUsage`):
    // - `last_context_input_tokens`: cross-provider-normalized total
    //   input (Anthropic: input+cc+cr; OpenAI: prompt_tokens). This
    //   is the field the frontend ChatInput hint uses as the "% of
    //   context_window" numerator.
    // - `last_input_tokens` / `last_output_tokens` /
    //   `last_cache_creation` / `last_cache_read`: the four
    //   provider-native breakdowns (the "tooltip" detail rows).
    add_session_column_if_missing(pool, "last_context_input_tokens", "INTEGER").await?;
    add_session_column_if_missing(pool, "last_input_tokens", "INTEGER").await?;
    add_session_column_if_missing(pool, "last_output_tokens", "INTEGER").await?;
    add_session_column_if_missing(pool, "last_cache_creation", "INTEGER").await?;
    add_session_column_if_missing(pool, "last_cache_read", "INTEGER").await?;

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

    // --- Group chat (07-29-group-chat, 2026-07-31): per-message
    // speaker identity. In a group_chat session multiple LLM
    // participants take turns; `speaker` records which participant
    // (by name) authored each assistant turn, so the UI can render
    // distinct bubbles and the next speaker's model can see who
    // said what. NULL for classic-chat messages (single agent) and
    // for user messages — the classic path is byte-identical to
    // pre-group-chat. Nullable, no DEFAULT (additive pattern
    // matching `thinking_ms`).
    add_messages_column_if_missing(pool, "speaker", "TEXT").await?;

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

    // --- Workflow integration (W1, 2026-07-08): per-session workflow opt-in.
    //
    // Boolean flag (INTEGER NOT NULL DEFAULT 0 per database-guidelines
    // boolean convention). 0 = workflow off (default, existing behavior
    // unchanged), 1 = workflow on (agent follows the active plugin's
    // state machine). Step 0.1 of task 07-08-workflow-integration.
    // Non-null + DEFAULT 0 so pre-existing rows survive (existing sessions
    // keep workflow off). Follows the color_tag / mode additive pattern.
    add_session_column_if_missing(pool, "workflow_enabled", "INTEGER NOT NULL DEFAULT 0").await?;

    // W1 (Workflow integration, 2026-07-08) Step 2.2: per-session
    // active workflow plugin name. TEXT NOT NULL DEFAULT 'dev'
    // so pre-Step-2.2 sessions resolve to the dev plugin on the
    // first workflow-enabled turn (no migration backfill needed —
    // the DEFAULT handles it). Mirrors the `workflow_enabled`
    // additive pattern: existing rows survive the upgrade.
    add_session_column_if_missing(pool, "plugin_name", "TEXT NOT NULL DEFAULT 'dev'").await?;

    // --- Group chat (07-29-group-chat, 2026-07-31): per-session
    // type discriminator. `chat` (default, existing single-LLM
    // behavior) vs `group_chat` (moderator-LLM-orchestrated multi-
    // LLM conversation). TEXT NOT NULL DEFAULT 'chat' so
    // pre-existing rows resolve to the classic single-agent path
    // (opt-in: only `group_chat` rows enter the group-chat
    // orchestration). Mirrors the `mode` / `plugin_name` additive
    // pattern — existing rows survive the upgrade with no backfill.
    add_session_column_if_missing(pool, "session_type", "TEXT NOT NULL DEFAULT 'chat'").await?;

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

    // --- L3b (2026-06-27): worktree_path column on subagent_runs.
    //
    // One new nullable TEXT column: the absolute path to the
    // worker's isolated git worktree (when isolation is active).
    // Written by `run_subagent` when it creates the worker worktree
    // via `git::worktree::create_worker`; cleared (set to NULL) when
    // the worker exits with no changes (the worktree is destroyed
    // immediately). When the worker exits WITH changes, the path is
    // preserved so a future PR3 `merge_worker` / `discard_worker`
    // tool can locate the branch + worktree for the merge/discard
    // decision (PR3 is out of scope for L3b PR1; the column is
    // forward-compatible so PR3 doesn't need a migration).
    //
    // - Nullable (no DEFAULT) — pre-L3b rows keep NULL (no worker
    //   worktree was ever created for them); non-isolated workers
    //   (researcher builtin, or dispatch `isolation: false`) also
    //   leave the column NULL.
    // - Not the branch name — the branch is derivable from the run
    //   id (`worker/<run_id>`) via `git::worktree::worker_branch_name`.
    //   Storing the path gives PR3 the on-disk location without a
    //   round-trip through the data_dir layout helpers.
    //
    // Idempotent: re-running on a pre-L3b DB brings it up to date;
    // re-running on a post-L3b DB is a no-op (the column exists).
    add_subagent_runs_column_if_missing(pool, "worktree_path", "TEXT").await?;

    // --- 2026-07-03 (task 07-03-subagent-per-agent-model-ui):
    // subagent_model_overrides table.
    //
    // 1 row per subagent name (PRIMARY KEY), carrying a `models.id`
    // UUID (soft FK; no constraint — see database-guidelines.md "Soft
    // FK pattern"). Globally scoped (no project_id) — matches the
    // builtin subagents' global nature and keeps the schema minimal.
    //
    // Why no FK: `models.id` may be deleted out from under a stale
    // override; the dispatch path's `resolve_worker_provider` already
    // handles catalog miss with `warn!` + parent fallback. The
    // Settings UI shows invalid overrides with a red "model 已删除"
    // badge so the user can fix it.
    //
    // Idempotent: `CREATE TABLE IF NOT EXISTS` is a no-op on
    // greenfield DBs and a no-op on post-migration DBs. No
    // `add_subagent_model_overrides_column_if_missing` needed (the
    // first release ships the full schema).
    sqlx::query(
        r#"
 CREATE TABLE IF NOT EXISTS subagent_model_overrides (
 agent_name TEXT NOT NULL PRIMARY KEY,
 model_id   TEXT NOT NULL,
 updated_at TEXT NOT NULL
 )
 "#,
    )
    .execute(pool)
    .await?;

    // --- 2026-07-03 (task 07-03-subagent-per-agent-model-ui, 可观测性段):
    // model_display column on subagent_runs.
    //
    // Nullable TEXT (no DEFAULT) so pre-C rows keep NULL — the frontend
    // reads `null` as "inherit parent" and either renders a "继承父级"
    // chip or hides the chip entirely (per AC14 / AC15). The column
    // carries the worker's *actual* model display, not the override /
    // frontmatter declaration (a catalog-miss downgrade writes NULL
    // even if the agent's override points at a real model; the
    // "实际用的" semantic is the load-bearing one for the UI).
    //
    // Nullable also lets legacy pre-C rows (and the post-L3b / pre-C
    // 1b-transition rows) load losslessly — the column is purely
    // additive.
    add_subagent_runs_column_if_missing(pool, "model_display", "TEXT").await?;

    // --- C1 (07-26-subagent-resume): messages_json + messages_truncated.
    //
    // `messages_json` stores the worker's accumulated `Vec<ChatMessage>`
    // serialized to JSON, so a later `dispatch_subagent` can resume the
    // conversation by replaying this history (instead of rebuilding
    // from scratch and re-reading the whole codebase). `messages_truncated`
    // mirrors `transcript_truncated`: when the serialized messages exceed
    // `MESSAGES_MAX_BYTES` (8 MiB — 2x the transcript threshold, see
    // `agent/subagent/truncate_summary.rs`), head+tail truncation kicks
    // in and the flag flips to 1; a truncated history is NOT safe to
    // resume from (the middle is missing), so resume falls back to a
    // fresh dispatch in that case (design §5).
    //
    // `messages_json` is NULLable (no DEFAULT): pre-C1 rows and any run
    // whose persistence failed keep NULL → `load_messages_by_run_id`
    // returns an empty Vec → resume falls back to fresh dispatch. This
    // matches the existing `transcript_json` NULLability posture.
    // `messages_truncated` is `INTEGER NOT NULL DEFAULT 0` so reads on
    // legacy rows resolve to "not truncated" without a NULL check
    // (mirrors `transcript_truncated`).
    //
    // Idempotent: re-running on a pre-C1 DB adds the columns; re-running
    // on a post-C1 DB is a no-op.
    add_subagent_runs_column_if_missing(pool, "messages_json", "TEXT").await?;
    add_subagent_runs_column_if_missing(pool, "messages_truncated", "INTEGER NOT NULL DEFAULT 0")
        .await?;

    // --- P1 (autonomous memory, 2026-06-29): storage layer for the
    // agent's self-produced, cross-session recalled experience memory.
    //
    // See `.trellis/tasks/06-29-am-p1-storage/prd.md` §1 for the full
    // schema rationale + spike-007 §5 for the design lineage. This
    // table is the foundation P2 (read/write closed loop via remember
    // tool + session-start recall injection) / P3 (pre-tool pitfall
    // recall) / P4 (event-driven reflection write) / P5 (status
    // machine + hygiene job) all build on.
    //
    // **FK decision (H1)**: `project_id` is a soft column — deliberately
    // NOT a `REFERENCES projects(id) ON DELETE CASCADE`. Memories are
    // durable experience: deleting a project should NOT wipe the
    // memories the agent learned while working in it (the project may
    // be restored, and the experience transfers to other projects via
    // the `scope='user'` layer). Orphan rows (project_id pointing at a
    // deleted project) are reclaimed by P5's hygiene job / an
    // independent sweep — NOT by CASCADE.
    //
    // **CHECK constraints (B1/2.2)**: SQLite has no `ALTER TABLE ...
    // ADD CONSTRAINT` — length/enum CHECKs MUST be defined at CREATE
    // TABLE time. The 4 CHECKs here are the DB-side guard; the Rust
    // enums (`MemoryKind` / `MemoryScope` / `MemoryStatus` in
    // `db/memories.rs`) are the application-side guard. Unknown enum
    // strings fail at INSERT; over-length title/content fail at INSERT.
    //
    // **trigger_key split (M1)**: spike-007 §5 originally proposed a
    // single JSON `trigger_key` column. External review split it into
    // 3 typed columns (`tool_name` / `command_pattern` / `path_globs`)
    // so the high-frequency pre-tool pitfall recall path
    // (`find_pitfalls_by_trigger`) can hit `idx_am_pitfall` via an
    // equality probe on `tool_name` — no `json_extract` (SQLite ≥3.38
    // dependency) and no LIKE-order-sensitivity on JSON text.
    //
    // **id vs memory_id (B2)**: `id INTEGER PRIMARY KEY AUTOINCREMENT`
    // is the internal rowid that FTS5's `content_rowid` requires (FTS5
    // external-content tables demand an integer rowid). `memory_id
    // TEXT UNIQUE` is the UUID v7 the rest of the system references
    // (UUID v7 is time-ordered → B-tree friendly, RFC 9562). The two
    // are split so FTS5 can keep its integer rowid contract while the
    // public API exposes a stable UUID.
    //
    // **forward-compat fields (H5)**: `confidence` / `hit_count` /
    // `last_used_at` / `demoted_reason` are P5 (status machine /
    // hygiene job) consumption. P1 only provides the storage + the
    // `bump_hit_count` / `update_status` interfaces; no production
    // code reads these fields yet. Defaults keep INSERTs from the
    // current write paths valid (P2 remember tool / P4 reflection).
    sqlx::query(
        r#"
 CREATE TABLE IF NOT EXISTS autonomous_memories (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    memory_id           TEXT    NOT NULL UNIQUE,
    scope               TEXT    NOT NULL,
    project_id          TEXT,
    kind                TEXT    NOT NULL,
    status              TEXT    NOT NULL,
    title               TEXT    NOT NULL,
    content             TEXT    NOT NULL,
    tags                TEXT    NOT NULL DEFAULT '[]',
    -- pitfall trigger key split into 3 typed columns (M1):
    -- pre-tool recall hits `idx_am_pitfall` via tool_name equality.
    tool_name           TEXT,
    command_pattern     TEXT,
    path_globs          TEXT,
    source_session_id   TEXT,
    source_ref          TEXT,
    -- forward-compat: P5 status machine / hygiene job consumption.
    confidence          REAL    NOT NULL DEFAULT 0.5,
    hit_count           INTEGER NOT NULL DEFAULT 0,
    last_used_at        TEXT,
    created_at          TEXT    NOT NULL,
    updated_at          TEXT    NOT NULL,
    demoted_reason      TEXT,
    CHECK(scope  IN ('user','project')),
    CHECK(kind   IN ('pitfall','preference','fact','decision')),
    CHECK(status IN ('candidate','active','verified','demoted')),
    CHECK(length(title)   <= 200),
    CHECK(length(content) <= 500)
 )
 "#,
    )
    .execute(pool)
    .await?;
    // session-start recall: FTS5 search + scope/project filter.
    sqlx::query(
        r#"
 CREATE INDEX IF NOT EXISTS idx_am_recall
 ON autonomous_memories(scope, project_id, status, kind)
 "#,
    )
    .execute(pool)
    .await?;
    // pitfall pre-tool recall: tool_name equality + status filter.
    // Partial index (WHERE tool_name IS NOT NULL) — non-pitfall kinds
    // never have tool_name set, so they're excluded from the index
    // entirely (smaller index, faster pitfall probe).
    sqlx::query(
        r#"
 CREATE INDEX IF NOT EXISTS idx_am_pitfall
 ON autonomous_memories(tool_name, status)
 WHERE tool_name IS NOT NULL
 "#,
    )
    .execute(pool)
    .await?;

    // --- P1 PR1b: FTS5 virtual table + sync triggers.
    //
    // **Open Q#1 verification (2026-06-29)**: FTS5 is compiled into the
    // system SQLite (3.53.0 on this dev box). Default tokenizer
    // `unicode61` does NOT tokenize CJK into searchable terms — both
    // ASCII terms embedded in CJK runs AND CJK terms themselves fail
    // to MATCH (verified empirically). The `trigram` tokenizer (SQLite
    // ≥3.34) tokenizes by 3-char sliding window, which handles CJK
    // AND substring search ("cargo" / "WSL" inside CJK text both
    // MATCH). Trade-off accepted: trigram requires ≥3 chars in the
    // query (2-char Chinese queries like "权限" won't MATCH); v1 is
    // precision-first and 2-char queries are uncommon for recall
    // (the title field still carries the bulk of search signal).
    //
    // External-content table pattern (`content='autonomous_memories'`)
    // keeps the FTS index in sync with the base table WITHOUT storing
    // a second copy of the text — FTS5 stores only the token index;
    // `content_rowid='id'` tells FTS5 to use the base table's integer
    // `id` as the rowid. The 3 triggers below are the standard FTS5
    // external-content sync dance: INSERT inserts into FTS, DELETE
    // inserts a special `'delete'` row (FTS5's idiom for "remove this
    // rowid's index entries"), UPDATE does a delete-then-insert pair.
    sqlx::query(
        r#"
 CREATE VIRTUAL TABLE IF NOT EXISTS autonomous_memories_fts USING fts5(
    title, content, tags,
    content='autonomous_memories', content_rowid='id',
    tokenize='trigram'
 )
 "#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
 CREATE TRIGGER IF NOT EXISTS am_fts_insert AFTER INSERT ON autonomous_memories BEGIN
    INSERT INTO autonomous_memories_fts(rowid, title, content, tags)
    VALUES (new.id, new.title, new.content, new.tags);
 END
 "#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
 CREATE TRIGGER IF NOT EXISTS am_fts_delete AFTER DELETE ON autonomous_memories BEGIN
    INSERT INTO autonomous_memories_fts(autonomous_memories_fts, rowid, title, content, tags)
    VALUES ('delete', old.id, old.title, old.content, old.tags);
 END
 "#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
 CREATE TRIGGER IF NOT EXISTS am_fts_update AFTER UPDATE ON autonomous_memories BEGIN
    INSERT INTO autonomous_memories_fts(autonomous_memories_fts, rowid, title, content, tags)
    VALUES ('delete', old.id, old.title, old.content, old.tags);
    INSERT INTO autonomous_memories_fts(rowid, title, content, tags)
    VALUES (new.id, new.title, new.content, new.tags);
 END
 "#,
    )
    .execute(pool)
    .await?;

    // --- 07-06 (am-observability-panel): `edited_by_user` provenance
    // marker. New column on `autonomous_memories` so the frontend
    // management modal (R5) can distinguish agent-written memories
    // from user-edited ones. `BOOLEAN NOT NULL DEFAULT 0` is
    // non-destructive (existing rows backfill to 0 = "not yet
    // user-edited"). The D1 design decision in
    // `.trellis/tasks/07-06-am-observability-panel/design.md` is
    // "新列,不复用 `source_ref`" — keeping the two concerns
    // (`source_ref` for P4 reflection provenance, `edited_by_user`
    // for the human-edit trail) cleanly separated.
    add_autonomous_memories_column_if_missing(pool, "edited_by_user", "BOOLEAN NOT NULL DEFAULT 0")
        .await?;

    // --- E2 (harness trace pipeline, 2026-07-14): v7 migration.
    //
    // Two additive schema changes for the per-turn harness trace
    // viewer (ROADMAP E2). Both are non-destructive: existing rows
    // survive, NULL / absent values are meaningful ("no trace data
    // for this turn").
    //
    // 1. New table `turn_trace`: one row per (session_id, seq) pair,
    //    accumulating trace dimensions via UPSERT as signals arrive
    //    at different write points during a turn (C3 compaction /
    //    C2 loop hint / workflow breadcrumb / per-turn token usage).
    //    UNIQUE(session_id, seq) is the UPSERT anchor — each write
    //    updates only its column, leaving the others untouched.
    //    ON DELETE CASCADE → deleting a session cleans up its trace
    //    rows (requires PRAGMA foreign_keys = ON, set by init_pool).
    //
    // 2. `session_audit_events.turn_seq INTEGER` — nullable column
    //    so audit rows can be grouped by turn. NULL for historical
    //    rows (pre-v7) and for IPC-handler audit writes that have no
    //    turn-loop context (commands/question.rs resolve_* etc.).
    //    The agent loop passes `Some(seq)` from inside the turn loop;
    //    the `record_audit_event` signature gains a `turn_seq` param.
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS turn_trace (
            id                INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id        TEXT NOT NULL,
            seq               INTEGER NOT NULL,
            token_usage_json  TEXT,
            compaction_json   TEXT,
            loop_hint_json    TEXT,
            breadcrumb_json   TEXT,
            -- C7 (2026-08-14): per-turn estimated token cost of the
            -- serialized `tools[]` array (cl100k estimate of the
            -- post-filter ToolDef JSON). A separately-measured slice
            -- of context that is already inside context_input_tokens
            -- but never counted alone before — surfaced so the trace
            -- viewer can show tools[]'s window share. Nullable: NULL
            -- for rows written before this column existed, and for
            -- turns where the estimate was skipped (worker path).
            tools_token       INTEGER,
            -- memory-block-governance WP1 (2026-08-15): cl100k
            -- estimate of the memory instruction blocks actually
            -- injected this request (banner + wrappers + layer
            -- bodies; digest-mode bodies when WP2 lands). Same
            -- separately-measured-slice semantics as tools_token:
            -- already inside context_input_tokens, surfaced so the
            -- trace viewer can show memory's window share. NULL for
            -- pre-column rows and worker turns (worker injection
            -- lives in subagent/prompt.rs, out of scope here —
            -- design §3.5a).
            memory_token      INTEGER,
            created_at        TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE,
            UNIQUE(session_id, seq)
        )
        "#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
        CREATE INDEX IF NOT EXISTS idx_turn_trace_session_seq
        ON turn_trace(session_id, seq)
        "#,
    )
    .execute(pool)
    .await?;
    add_session_audit_events_column_if_missing(pool, "turn_seq", "INTEGER").await?;
    // C7 (2026-08-14): `turn_trace.tools_token` — backfills the new
    // per-turn tools[] token-estimate column on existing turn_trace
    // tables. No-op for greenfield DBs (the CREATE TABLE above
    // already declares it). Idempotent via the PRAGMA probe.
    add_turn_trace_column_if_missing(pool, "tools_token", "INTEGER").await?;
    // memory-block-governance WP1 (2026-08-15):
    // `turn_trace.memory_token` — same idempotent backfill pattern
    // as tools_token above; no-op for greenfield DBs (declared in
    // the CREATE TABLE above).
    add_turn_trace_column_if_missing(pool, "memory_token", "INTEGER").await?;

    // --- PR1 of multi-model task: seed default providers + models
    // if the catalog is empty. Idempotent:0-row check skips the
    // insert on subsequent boots. Backfills `sessions.model_id`
    // for any row still NULL after the ALTER. ---
    super::super::config::seed_default_providers_and_models(pool).await?;

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

    // --- RULE-D-001 (P1 API key 加密, 2026-06-24).
    //
    // provider api_key 不再明文存 DB. 两列:
    // - `api_key_enc TEXT NOT NULL DEFAULT ''`: base64(VERSION||nonce||ct||tag),
    //   AES-256-GCM + HKDF(machine-id) 派生 master key, AAD = provider id.
    //   空串 = 未设置 key.
    // - `key_migrated_at TEXT`: 迁移完成哨兵(RFC3339). NULL = 未迁移(仍需
    //   从旧 `api_key` 明文列迁移).
    //
    // 旧 `api_key TEXT` 列保留(SQLite < 3.35 无 DROP COLUMN, 留空列无成本),
    // 迁移后 UPDATE 为 ''. 见 `migrate_provider_api_keys_to_encrypted`.
    add_provider_column_if_missing(pool, "api_key_enc", "TEXT NOT NULL DEFAULT ''").await?;
    add_provider_column_if_missing(pool, "key_migrated_at", "TEXT").await?;
    migrate_provider_api_keys_to_encrypted(pool).await?;

    Ok(())
}
