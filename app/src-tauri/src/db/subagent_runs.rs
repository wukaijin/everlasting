//! B6 PR2 (2026-06-20): `subagent_runs` persistence.
//!
//! Stores per-dispatch records of worker subagents spawned via the
//! `dispatch_subagent` tool. The worker is driven by a nested
//! `run_chat_loop` call (B6 PR1, see `agent::chat_loop::run_subagent`)
//! and accumulates its `SubagentBufferSink` transcript + final
//! `TokenUsage` + final assistant text in memory. PR2 lifts those
//! three concerns into SQLite so:
//!
//! 1. PR3's frontend `ToolCallCard` expand UI can render the
//!    worker's transcript + summary without re-running the worker.
//! 2. A session reload (after app restart) still shows the
//!    worker's intermediate state — the in-memory sink is gone
//!    after a reload.
//! 3. Token-usage accounting is auditable per-run (`token_usage_json`
//!    on the `subagent_runs` row); the parent session's
//!    `sessions.input_tokens_total` carries the *aggregated* total
//!    (updated by `db::add_token_usage` at `chat_loop.rs:1031`
//!    as the worker runs — see RULE-A-015 + RULE-BackSubagent-002
//!    option i; `add_token_usage_streaming` is retained as the
//!    PR2 API surface but has no production callsite).
//!
//! ⚠️ **Production-only path**: production code paths MUST go
//! through `db::add_token_usage` (decoupled from `skip_persist`),
//! NOT through `add_token_usage_streaming`. The streaming helper
//! is a future-API surface for a worker↔parent session identity
//! split (see its doc for context).
//!
//! # Schema
//!
//! The migration is in `db/migrations.rs`; columns are:
//! - `id TEXT PRIMARY KEY` (UUID v4 nanoid).
//! - `parent_session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE`.
//! - `parent_request_id TEXT NOT NULL` (worker rid; not a FK).
//! - `subagent_name TEXT NOT NULL` (`researcher` / `general-purpose`).
//! - `status TEXT NOT NULL CHECK(status IN ('running','completed','cancelled','error'))`.
//! - `started_at TEXT NOT NULL`, `finished_at TEXT` (NULL while running).
//! - `token_usage_json TEXT` (JSON-encoded [`TokenUsage`]; NULL while running).
//! - `summary TEXT` (worker's `final_text` plain string; NULL while running).
//! - `transcript_json TEXT` (JSON-encoded `Vec<TranscriptEntry>`; NULL while running).
//! - `task TEXT` (2026-06-21 PR1, LLM's delegation prompt).
//! - `final_text TEXT` (2026-06-21 PR1, prefix-stripped worker reply).
//! - `turn_count INTEGER` (2026-06-22 RULE-FrontSubagent-004, actual
//!   completed turn iterations the worker executed before reaching
//!   terminal state; NULL on pre-PR2 rows — drawer degrades to
//!   wall-clock suffix for those legacy rows).
//! - `transcript_truncated INTEGER NOT NULL DEFAULT 0` (1 = over 4MB cap).
//! - `created_at TEXT NOT NULL DEFAULT (datetime('now'))`.
//!
//! Indexed on `(parent_session_id, started_at DESC)` and
//! `parent_request_id` for the PR3 list-by-session + per-request lookups.
//!
//! # Audit invariant
//!
//! **`subagent_runs` writes do NOT contaminate the parent session's
//! `session_audit_events`.** Worker ⑨ decisions (Tier 2 / Tier 3 /
//! Tier 4 collapse) are recorded in the transcript's
//! `TranscriptKind::PermissionAsk` entries, not in
//! `session_audit_events` (see `permissions::check` + the
//! `is_worker` collapse at `ask_path`). This module never calls
//! `record_audit_event`; the parent session's audit log remains a
//! pure record of the parent's own ⑨ decisions.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Row, SqlitePool};
use uuid::Uuid;

use crate::llm::types::TokenUsage;

// ---------------------------------------------------------------------------
// SubagentStatusDb — DB-side enum for the `status` CHECK column
// ---------------------------------------------------------------------------

/// The terminal status a worker subagent exited with. Mirrors
/// `agent::subagent::SubagentStatus` (the agent-side enum used to
/// format the dispatch_subagent tool_result) but is a separate
/// type because the DB layer is a different crate boundary. The
/// two enums are kept in lockstep via [`SubagentStatusDb::from_agent`]
/// + `as_str` so a future drift (renaming `Error` to `Failed`,
/// etc.) is caught at the `as_str()` boundary, not by a silent
/// enum mismatch.
///
/// 2026-06-21 (R2): added `Incomplete` for the `max_turns` soft-
/// terminal path. The `widen_subagent_runs_status_check_for_incomplete`
/// migration (db/migrations.rs) widens the `status` column's
/// CHECK constraint to include `'incomplete'`. The two
/// enums (`agent::subagent::SubagentStatus` and
/// `db::subagent_runs::SubagentStatusDb`) must stay in lockstep —
/// a new variant here requires a corresponding variant in the
/// agent-side enum + a string mapping in both `as_str` and
/// `from_str_opt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubagentStatusDb {
    Running,
    Completed,
    Cancelled,
    Error,
    Incomplete,
}

impl SubagentStatusDb {
    /// Wire form for the `status` column. CHECK-constrained to
    /// `('running','completed','cancelled','error','incomplete')`
    /// — keep this in lockstep with the migration's CHECK
    /// expression.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Error => "error",
            Self::Incomplete => "incomplete",
        }
    }

    /// Lenient parse from a DB string. Returns `Self::Running` for
    /// unknown values — a future binary may add new variants and an
    /// older binary reading a newer DB should default to the
    /// "in-flight" semantic (which is the safe fallback — the row
    /// will be re-classified when the worker exits).
    #[allow(dead_code)] // exposed for future PR3 list UI / C4 audit reads
    pub fn from_str_opt(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "completed" => Self::Completed,
            "cancelled" => Self::Cancelled,
            "error" => Self::Error,
            "incomplete" => Self::Incomplete,
            _ => Self::Running,
        }
    }
}

// ---------------------------------------------------------------------------
// SubagentRunRow — read shape (SELECT * FROM subagent_runs)
// ---------------------------------------------------------------------------

/// Row shape for SELECTs against `subagent_runs`. Camel-cased
/// on the wire to match every other `db::*Row` type that crosses
/// the IPC boundary (Tauri 2 default behavior; verified by
/// `backend/database-guidelines.md` "When you add a new user-managed
/// catalog" checklist).
///
/// `allow(dead_code)` is set at the type level because PR2's
/// production wire-up uses `insert_run` + `update_run_finished`
/// for the `subagent_runs` writes; per-turn token-usage fold
/// goes through `db::add_token_usage` at `chat_loop.rs:1031`
/// (decoupled from `skip_persist` in PR2a per RULE-A-015 — the
/// worker reuses `parent_session_id`, so the parent's
/// `add_token_usage` call naturally accumulates the worker's
/// per-turn usage). `add_token_usage_streaming` is retained as
/// the PR2 API surface for a future worker↔parent session
/// identity split, exercised by
/// `db/tests.rs::add_token_usage_streaming_accumulates_in_parent`.
/// The row struct is materialized by `get_run` +
/// `list_runs_by_session` which are themselves PR3-API surface
/// (frontend expand UI + C4 audit read). The `db/tests.rs`
/// integration tests exercise the row constructor via the
/// `FromRow` derive.
///
/// 2026-06-21 (subagent drawer redesign PR1): two new nullable
/// TEXT columns — `task` (the LLM's delegation prompt written at
/// dispatch time) and `final_text` (the worker's terminal
/// assistant text with the `[status: ...]\n` prefix stripped,
/// written at worker exit). Both NULL for pre-PR1 rows; the
/// frontend drawer falls back to `summary` when `final_text` is
/// NULL (legacy compat) and renders `task` as the prompt card
/// header.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SubagentRunRow {
    pub id: String,
    pub parent_session_id: String,
    pub parent_request_id: String,
    pub subagent_name: String,
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub token_usage_json: Option<String>,
    pub summary: Option<String>,
    /// B6 redesign PR1 (2026-06-21): worker's final assistant text
    /// with `[status: ...]\n` prefix stripped. The `status` column
    /// carries the prefix independently — this field is the pure
    /// summary text, suitable for direct display in the drawer's
    /// Reply segment. `serde(rename_all = "camelCase")` projects
    /// to `finalText` on the wire.
    pub final_text: Option<String>,
    /// B6 redesign PR1 (2026-06-21): the LLM's delegation prompt
    /// at dispatch time (`input.task` of `dispatch_subagent`). The
    /// drawer's prompt card renders this verbatim (with a length
    /// truncation). NULL if the row pre-dates PR1 (legacy row).
    pub task: Option<String>,
    /// 2026-06-22 (RULE-FrontSubagent-004): the actual number of
    /// completed LLM turn iterations the worker executed before
    /// reaching its terminal state. Counted by `SubagentBufferSink`
    /// (one increment per real per-turn `Done` event — synthetic
    /// `cancelled` / `max_turns` terminals do NOT increment; the
    /// counter is always the real turn count at exit). NULL on
    /// pre-PR2 rows (column added by `add_subagent_runs_column_if_missing`);
    /// the drawer's `statusDisplay` degrades to the wall-clock
    /// "stopped at X.Xs" / "incomplete at X.Xs" suffix when NULL.
    /// `serde(rename_all = "camelCase")` projects to `turnCount`
    /// on the wire.
    pub turn_count: Option<i64>,
    pub transcript_json: Option<String>,
    pub transcript_truncated: i64,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// SubagentRunSummary — projected read shape for list endpoints
// ---------------------------------------------------------------------------

/// B6 PR3a (2026-06-20): list-endpoint projection of
/// `SubagentRunRow`. Excludes `transcript_json` +
/// `transcript_truncated` so a "list all runs for this session"
/// IPC stays cheap (transcript can be 4 MiB per run; multi-run
/// lists would balloon the payload). PR3's frontend
/// `subagentRuns.ts` calls `list_subagent_runs_by_session` to
/// populate the per-session run badge; the transcript is fetched
/// on-demand via `get_subagent_run(run_id)` when the user opens
/// the drawer.
///
/// The shape mirrors the columns a UI list needs (id +
/// identity + status + timing + token usage + summary preview)
/// without the heavy transcript column. `status` is the typed
/// `SubagentStatusDb` enum (not the wire string) so the
/// frontend can render the status badge without an extra parse
/// step — the IPC layer's `Serialize` derive projects the enum
/// to lowercase automatically.
///
/// 2026-06-21 (B6 redesign PR1): the projection also includes
/// `task` + `final_text` so the drawer's list view can render
/// the prompt + Reply segment without a separate per-run IPC
/// (the heavy transcript is still excluded — list IPC stays
/// KB-scale, not MB-scale).
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SubagentRunSummary {
    pub id: String,
    pub parent_session_id: String,
    pub parent_request_id: String,
    pub subagent_name: String,
    /// Typed enum — serializes lowercase (`running` / `completed` /
    /// `cancelled` / `error`) matching the DB column's CHECK
    /// constraint.
    pub status: SubagentStatusDb,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub token_usage_json: Option<String>,
    pub summary: Option<String>,
    /// B6 redesign PR1 (2026-06-21): final assistant text (with
    /// `[status: ...]\n` prefix stripped).
    pub final_text: Option<String>,
    /// B6 redesign PR1 (2026-06-21): LLM's delegation prompt.
    pub task: Option<String>,
    /// 2026-06-22 (RULE-FrontSubagent-004): actual completed turn
    /// count the worker executed before reaching terminal state.
    /// NULL on pre-PR2 rows. Cheap single-i64 column so it's
    /// included in the summary projection (no transcript bloat);
    /// the card / drawer can both read it without a second IPC.
    pub turn_count: Option<i64>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Insert a new `running` row for a freshly-spawned worker. Returns
/// the new row's id. `started_at` is `datetime('now')` (RFC 3339).
/// All other optional columns are seeded with sensible defaults
/// (empty `TokenUsage` JSON, empty transcript `[]`, `transcript_truncated=0`)
/// so a future `SELECT * FROM subagent_runs WHERE status='running'`
/// always sees well-formed rows.
///
/// Called from `agent::chat_loop::run_subagent` immediately before
/// the nested `run_chat_loop` call. The returned id is the
/// `worker_run_id` that PR3's expand UI fetches with [`get_run`].
///
/// `task` is the LLM's delegation prompt (`input.task` of
/// `dispatch_subagent`). Written inline so the prompt is on the
/// row the moment `insert_run` returns — this matters for the
/// drawer: the user can open the drawer mid-worker (before
/// `update_run_finished` fires) and see the prompt card. `None`
/// for callers without a prompt (e.g. tests); pre-PR1 callers
/// pass `None` and the column stays NULL (legacy compat).
pub async fn insert_run(
    pool: &SqlitePool,
    parent_session_id: &str,
    parent_request_id: &str,
    subagent_name: &str,
    task: Option<&str>,
) -> Result<String, sqlx::Error> {
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let empty_usage = serde_json::to_string(&TokenUsage::default())
        .unwrap_or_else(|_| "{\"input_tokens\":0,\"output_tokens\":0,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0}".to_string());
    sqlx::query(
        r#"
        INSERT INTO subagent_runs
        (id, parent_session_id, parent_request_id, subagent_name,
         status, started_at, finished_at, token_usage_json, summary,
         task, final_text, transcript_json, transcript_truncated, created_at)
        VALUES (?, ?, ?, ?, 'running', ?, NULL, ?, NULL, ?, NULL, '[]', 0, ?)
        "#,
    )
    .bind(&id)
    .bind(parent_session_id)
    .bind(parent_request_id)
    .bind(subagent_name)
    .bind(&now)
    .bind(&empty_usage)
    .bind(task)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(id)
}

/// Update a `running` row to its terminal state. Called from
/// `run_subagent` after the nested `run_chat_loop` returns. The
/// `transcript` Vec is the `SubagentBufferSink::transcript_snapshot()`
/// (already capped by `truncate_transcript_for_persistence` so it
/// fits in 4MB; the `truncated` flag carries the cap signal so
/// PR3's UI can show a "transcript truncated" badge).
///
/// The `summary` is the worker's `final_text` plain string
/// (NO status prefix — the `status` column is the source of truth
/// for the prefix; this matches the PRD's `summary 字段语义` decision
/// "final_text 纯文本,status 字段独立"). The `summary` field is the
/// **legacy wire field** — pre-PR1 callers wrote it, post-PR1
/// callers continue to write it for backward compat.
///
/// `final_text` is the same content as `summary` after
/// `format_dispatch_result`'s `[status: ...]\n` prefix is
/// stripped (PR1, 2026-06-21). The two fields intentionally
/// carry the same string for completed runs; for cancelled
/// runs `final_text` is `worker_text + "\n\n[CANCELLED_MARKER]"`
/// (with the prefix stripped from the wrapper), and for error
/// runs it's the error message. The drawer reads `final_text`
/// as the Reply segment; legacy rows without `final_text` fall
/// back to `summary` (the drawer's defensive read order).
///
/// `token_usage` is the worker's final `TokenUsage` sum (across all
/// the worker's turns). Serialized as JSON for the
/// `token_usage_json` column.
///
/// `transcript` is serialized as JSON; on serialization failure
/// (extremely unlikely for `Vec<TranscriptEntry>` — its inner
/// `payload_json` is already a `serde_json::Value` so the wire
/// shape round-trips), the function falls back to `"[]"` so a
/// truncation never loses the entire transcript row.
///
/// 2026-06-22 (RULE-FrontSubagent-004): `turn_count` is the actual
/// number of completed LLM turn iterations the worker executed
/// before reaching its terminal state. Counted by
/// `SubagentBufferSink` (one increment per real per-turn `Done`;
/// synthetic `cancelled` / `max_turns` terminals do NOT increment).
/// Pass `None` for pre-PR2 callers or when the count is unknown —
/// the column stays NULL and the drawer degrades to wall-clock.
/// Production `run_subagent` always passes `Some(turns)`.
#[allow(clippy::too_many_arguments)]
pub async fn update_run_finished(
    pool: &SqlitePool,
    id: &str,
    status: SubagentStatusDb,
    finished_at: &str,
    summary: &str,
    final_text: &str,
    token_usage: &TokenUsage,
    transcript: &[crate::agent::subagent::TranscriptEntry],
    transcript_truncated: bool,
    turn_count: Option<i64>,
) -> Result<(), sqlx::Error> {
    let token_usage_json =
        serde_json::to_string(token_usage).unwrap_or_else(|_| String::from("{}"));
    let transcript_json = serde_json::to_string(transcript).unwrap_or_else(|_| "[]".to_string());
    sqlx::query(
        r#"
        UPDATE subagent_runs
        SET status = ?,
            finished_at = ?,
            summary = ?,
            final_text = ?,
            token_usage_json = ?,
            transcript_json = ?,
            transcript_truncated = ?,
            turn_count = ?
        WHERE id = ?
        "#,
    )
    .bind(status.as_str())
    .bind(finished_at)
    .bind(summary)
    .bind(final_text)
    .bind(&token_usage_json)
    .bind(&transcript_json)
    .bind(transcript_truncated as i64)
    .bind(turn_count)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Look up a single `subagent_runs` row by id. Returns `None` for
/// unknown ids. Used by PR3's frontend `ToolCallCard` expand IPC
/// (a future `get_subagent_run` Tauri command) and by C4's audit
/// log read path.
///
/// `allow(dead_code)` because PR2's production wire-up does not
/// call this directly (no current IPC consumes it); the
/// `db/tests.rs` integration test will exercise the read path
/// to lock the schema.
///
/// 2026-06-21 (B6 redesign PR1): SELECT + row mapping expanded
/// with `task` + `final_text` (the two new nullable TEXT
/// columns). Pre-PR1 rows have NULL for both; the row mapping
/// uses `try_get` so older databases still load (defensive
/// against pre-PR1 deployments — the migration adds the
/// columns but a snapshot taken mid-migration could in
/// principle race; `try_get` on a NULL TEXT returns `Ok(None)`
/// either way).
#[allow(dead_code)]
pub async fn get_run(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<SubagentRunRow>, sqlx::Error> {
    let row = sqlx::query(
        r#"
        SELECT id, parent_session_id, parent_request_id, subagent_name,
               status, started_at, finished_at, token_usage_json, summary,
               final_text, task, turn_count, transcript_json, transcript_truncated,
               created_at
        FROM subagent_runs
        WHERE id = ?
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    match row {
        None => Ok(None),
        Some(r) => Ok(Some(SubagentRunRow {
            id: r.try_get("id")?,
            parent_session_id: r.try_get("parent_session_id")?,
            parent_request_id: r.try_get("parent_request_id")?,
            subagent_name: r.try_get("subagent_name")?,
            status: r.try_get("status")?,
            started_at: r.try_get("started_at")?,
            finished_at: r.try_get("finished_at")?,
            token_usage_json: r.try_get("token_usage_json")?,
            summary: r.try_get("summary")?,
            final_text: r.try_get("final_text")?,
            task: r.try_get("task")?,
            turn_count: r.try_get("turn_count")?,
            transcript_json: r.try_get("transcript_json")?,
            transcript_truncated: r.try_get("transcript_truncated")?,
            created_at: r.try_get("created_at")?,
        })),
    }
}

/// List all `subagent_runs` for `parent_session_id`, newest first.
/// Used by PR3's session-detail UI to render every worker
/// dispatched by the parent. The
/// `idx_subagent_runs_session_started(parent_session_id,
/// started_at DESC)` index covers this query.
///
/// `allow(dead_code)` because PR2's production wire-up does not
/// call this directly (no current IPC consumes it); the
/// `db/tests.rs` integration test will exercise the read path
/// to lock the schema.
///
/// 2026-06-21 (B6 redesign PR1): SELECT + row mapping expanded
/// with `task` + `final_text` (same rationale as `get_run`).
#[allow(dead_code)]
pub async fn list_runs_by_session(
    pool: &SqlitePool,
    parent_session_id: &str,
) -> Result<Vec<SubagentRunRow>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT id, parent_session_id, parent_request_id, subagent_name,
               status, started_at, finished_at, token_usage_json, summary,
               final_text, task, turn_count, transcript_json, transcript_truncated,
               created_at
        FROM subagent_runs
        WHERE parent_session_id = ?
        ORDER BY started_at DESC
        "#,
    )
    .bind(parent_session_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(SubagentRunRow {
                id: r.try_get("id")?,
                parent_session_id: r.try_get("parent_session_id")?,
                parent_request_id: r.try_get("parent_request_id")?,
                subagent_name: r.try_get("subagent_name")?,
                status: r.try_get("status")?,
                started_at: r.try_get("started_at")?,
                finished_at: r.try_get("finished_at")?,
                token_usage_json: r.try_get("token_usage_json")?,
                summary: r.try_get("summary")?,
                final_text: r.try_get("final_text")?,
                task: r.try_get("task")?,
                turn_count: r.try_get("turn_count")?,
                transcript_json: r.try_get("transcript_json")?,
                transcript_truncated: r.try_get("transcript_truncated")?,
                created_at: r.try_get("created_at")?,
            })
        })
        .collect()
}

/// B6 PR3a (2026-06-20): list endpoint projection. Returns the
/// same set of rows as [`list_runs_by_session`] but as
/// [`SubagentRunSummary`] (no `transcript_json` +
/// `transcript_truncated` columns). Used by the PR3 frontend
/// `list_subagent_runs_by_session` IPC; the full row is fetched
/// on demand via `get_subagent_run(run_id)` when the user opens
/// the drawer.
///
/// The status column is decoded into the typed
/// [`SubagentStatusDb`] enum (NOT the wire string) so the
/// frontend's `SubagentDrawer.vue` status badge can read the
/// enum directly — `serde` re-projects it to lowercase on the
/// wire. This is the same "lenient parse for forward-compat"
/// pattern [`SubagentStatusDb::from_str_opt`] uses; an unknown
/// status string falls back to `Running` (matches the column's
/// safe-default for forward-compat strings).
///
/// The query SELECTs only the 9 projected columns, so a multi-
/// run session returns a payload sized in KB (not MB) — the
/// 4 MiB-cap'd transcript stays on the per-run detail path.
#[allow(dead_code)]
pub async fn list_runs_summary_by_session(
    pool: &SqlitePool,
    parent_session_id: &str,
) -> Result<Vec<SubagentRunSummary>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT id, parent_session_id, parent_request_id, subagent_name,
               status, started_at, finished_at, token_usage_json, summary,
               final_text, task, turn_count
        FROM subagent_runs
        WHERE parent_session_id = ?
        ORDER BY started_at DESC
        "#,
    )
    .bind(parent_session_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|r| {
            let status_str: String = r.try_get("status")?;
            Ok(SubagentRunSummary {
                id: r.try_get("id")?,
                parent_session_id: r.try_get("parent_session_id")?,
                parent_request_id: r.try_get("parent_request_id")?,
                subagent_name: r.try_get("subagent_name")?,
                status: SubagentStatusDb::from_str_opt(&status_str),
                started_at: r.try_get("started_at")?,
                finished_at: r.try_get("finished_at")?,
                token_usage_json: r.try_get("token_usage_json")?,
                summary: r.try_get("summary")?,
                final_text: r.try_get("final_text")?,
                task: r.try_get("task")?,
                turn_count: r.try_get("turn_count")?,
            })
        })
        .collect()
}

/// Add a worker's per-turn `TokenUsage` to the **parent session's**
/// `sessions.input_tokens_total` / `output_tokens_total` /
/// `cache_creation_total` / `cache_read_total` columns. Called by
/// the worker's `SubagentBufferSink` after each worker turn (so the
/// parent's UI sees the worker burning tokens in real time).
///
/// The worker's intermediate `messages` rows do NOT land in the
/// DB (worker path uses `skip_persist=true` — see
/// `agent::chat_loop::run_chat_loop`'s 20th parameter), so
/// `add_token_usage` inside `run_chat_loop` is gated by
/// `!skip_persist` and never fires for the worker. The worker's
/// per-turn `TokenUsage` therefore has to be folded into the
/// parent **from outside** the agent loop — that's this function's
/// job. The fold target is the parent session's id, NOT the
/// worker's session id (worker reuses the parent session id, so
/// the destination column is unambiguous).
///
/// `parent_session_id` MUST be a valid `sessions.id`; the helper
/// silently no-ops on missing ids (matching `add_token_usage`'s
/// contract — see `db/sessions.rs:374-399`).
///
/// This is the **streaming** accumulator (vs the `update_run_finished`
/// terminal-write to `subagent_runs.token_usage_json`, which
/// captures the worker's final cumulative total at exit time). The
/// two are independent: the streaming updates are for the parent
/// UI's live token counter, the terminal write is for the audit
/// trail.
///
/// **Implementation note (B6 PR2 production path)**: the current
/// `run_subagent` implementation reuses `db::add_token_usage` (the
/// generic helper) because the worker reuses `parent_session_id`
/// as its `session_id` — the per-turn `add_token_usage` call at
/// `chat_loop.rs:907` now runs unconditionally (decoupled from
/// `skip_persist` in this PR) and naturally folds the worker's
/// per-turn usage into the parent's running total. This function
/// is **retained** as the public PR2 API surface (the PRD §"db
/// module" lists it) and is exercised by `db/tests.rs`'s
/// `add_token_usage_streaming_accumulates_in_parent` integration
/// test. A future PR that splits worker ↔ parent session identity
/// (e.g. daemon-ized workers with their own session row) will
/// switch `run_subagent` to call this function instead of the
/// generic `add_token_usage`.
#[allow(dead_code)]
pub async fn add_token_usage_streaming(
    pool: &SqlitePool,
    parent_session_id: &str,
    usage: &TokenUsage,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE sessions
        SET input_tokens_total = COALESCE(input_tokens_total, 0) + ?,
            output_tokens_total = COALESCE(output_tokens_total, 0) + ?,
            cache_creation_total = COALESCE(cache_creation_total, 0) + ?,
            cache_read_total = COALESCE(cache_read_total, 0) + ?,
            updated_at = ?
        WHERE id = ?
        "#,
    )
    .bind(usage.input_tokens)
    .bind(usage.output_tokens)
    .bind(usage.cache_creation_input_tokens)
    .bind(usage.cache_read_input_tokens)
    .bind(Utc::now().to_rfc3339())
    .bind(parent_session_id)
    .execute(pool)
    .await?;
    Ok(())
}
