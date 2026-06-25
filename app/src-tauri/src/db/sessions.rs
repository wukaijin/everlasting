//! Session CRUD + worktree-state transitions + message persistence.
//!
//! Each session is one conversation scoped to a project. The
//! `current_cwd` column tracks the directory the agent is operating
//! in; tools fall back to it when `worktree_path` is `None`. The
//! `worktree_state` tri-valued enum tracks whether the session has a
//! live worktree bound (`Active`), previously had one (`Detached`),
//! or never did (`None`).

use chrono::Utc;
use sqlx::{Row, SqlitePool};

use crate::llm::types::{ContentBlock, MessageContent, Role, TokenUsage};

use super::types::{LoadedSession, MessageRow, SessionRow, SessionSummary, WorktreeState};

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
 model_id: Option<&str>,
) -> Result<SessionRow, sqlx::Error> {
 let now = Utc::now().to_rfc3339();
 let title = "新对话".to_string();

 sqlx::query(
 r#"
 INSERT INTO sessions
 (id, title, created_at, updated_at, model, metadata, project_id, current_cwd,
 worktree_path, worktree_state, last_worktree_path, model_id, color_tag, mode)
 VALUES (?, ?, ?, ?, ?, NULL, ?, ?, NULL, 'none', NULL, ?, NULL, 'chat')
 "#,
 )
 .bind(session_id)
 .bind(&title)
 .bind(&now)
 .bind(&now)
 .bind(model)
 .bind(project_id)
 .bind(initial_cwd)
 .bind(model_id)
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
 model_id: model_id.map(|s| s.to_string()),
 input_tokens_total: None,
 output_tokens_total: None,
 cache_creation_total: None,
 cache_read_total: None,
 last_context_input_tokens: None,
 last_input_tokens: None,
 last_output_tokens: None,
 last_cache_creation: None,
 last_cache_read: None,
 color_tag: None,
 mode: crate::db::Mode::Edit,
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
 s.input_tokens_total, s.output_tokens_total,
 s.cache_creation_total, s.cache_read_total,
 s.last_context_input_tokens, s.last_input_tokens,
 s.last_output_tokens, s.last_cache_creation, s.last_cache_read,
 s.color_tag, s.mode,
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
 let preview = if preview.chars().count() >80 {
 let truncated: String = preview.chars().take(80).collect();
 format!("{}…", truncated)
 } else {
 preview
 };
 let state_str: String = r.try_get("worktree_state")?;
 let color_tag: Option<i32> = r.try_get("color_tag")?;
 let mode_str: String = r.try_get("mode")?;
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
 input_tokens_total: r.try_get("input_tokens_total")?,
 output_tokens_total: r.try_get("output_tokens_total")?,
 cache_creation_total: r.try_get("cache_creation_total")?,
 cache_read_total: r.try_get("cache_read_total")?,
 last_context_input_tokens: r.try_get("last_context_input_tokens")?,
 last_input_tokens: r.try_get("last_input_tokens")?,
 last_output_tokens: r.try_get("last_output_tokens")?,
 last_cache_creation: r.try_get("last_cache_creation")?,
 last_cache_read: r.try_get("last_cache_read")?,
 color_tag,
 mode: crate::db::Mode::from_str_opt(&mode_str),
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
 worktree_path, worktree_state, last_worktree_path, model_id,
 input_tokens_total, output_tokens_total,
 cache_creation_total, cache_read_total,
 last_context_input_tokens, last_input_tokens,
 last_output_tokens, last_cache_creation, last_cache_read,
 color_tag, mode
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
 let mode_str: String = r.try_get("mode")?;
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
 input_tokens_total: r.try_get("input_tokens_total")?,
 output_tokens_total: r.try_get("output_tokens_total")?,
 cache_creation_total: r.try_get("cache_creation_total")?,
 cache_read_total: r.try_get("cache_read_total")?,
 last_context_input_tokens: r.try_get("last_context_input_tokens")?,
 last_input_tokens: r.try_get("last_input_tokens")?,
 last_output_tokens: r.try_get("last_output_tokens")?,
 last_cache_creation: r.try_get("last_cache_creation")?,
 last_cache_read: r.try_get("last_cache_read")?,
 color_tag: r.try_get("color_tag")?,
 mode: crate::db::Mode::from_str_opt(&mode_str),
 }
 }
 None => return Ok(None),
 };

 let msg_rows = sqlx::query(
 r#"
 SELECT id, session_id, role, content, text, has_tool_calls, has_tool_results,
 created_at, seq, metadata, ttfb_ms, gen_ms, total_ms, thinking_ms
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
 // F5: per-message latency breakdown. All three nullable
 // for pre-F5 rows; the frontend `update_message_latency` IPC
 // sets them at stream done.
 ttfb_ms: r.try_get("ttfb_ms")?,
 gen_ms: r.try_get("gen_ms")?,
 total_ms: r.try_get("total_ms")?,
 // F5 follow-up: thinking-phase wall-clock. `None` for
 // messages that never entered the thinking phase AND
 // for pre-F5-follow-up rows. Set by the
 // `update_message_thinking` IPC at stream done.
 thinking_ms: r.try_get("thinking_ms")?,
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

/// Delete all messages for a session, keeping the session row itself.
///
/// B3 `/clear`: clears the conversation but preserves session metadata
/// (title / color / mode / model / project / timestamps). Audit events
/// (`session_audit_events`) are session-scoped and intentionally kept —
/// they record what the agent *did*, not the live message buffer.
pub async fn delete_messages_by_session(
 pool: &SqlitePool,
 session_id: &str,
) -> Result<(), sqlx::Error> {
 sqlx::query("DELETE FROM messages WHERE session_id = ?")
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
/// "turn结束一次性写").
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
// 2026-06-26 (token-usage snapshot fix): per-session LAST-TURN snapshot
// ---------------------------------------------------------------------------

/// OVERWRITE the per-session `last_*` snapshot columns with this
/// turn's [`TokenUsage`]. Replaces the A4 cumulative accumulator
/// `add_token_usage` (which was `col = COALESCE(col, 0) + ?` per
/// turn). Snapshot semantics: the value reflects the LLM's LAST
/// request, not the running session total — the frontend ChatInput
/// hint renders this as "X · Y% / context_window" so the user sees
/// the live context pressure (matching Anthropic's statusline
/// convention; same shape as `sanztheo/claude-code-statusline`).
///
/// Worker isolation (2026-06-26 reversal of RULE-A-015/PR2a): the
/// agent loop's caller gates this call behind `if !skip_persist`
/// again. The worker path reuses the parent's `session_id`, so
/// leaving the gate off (per PR2a) would let every worker turn
/// OVERWRITE the parent's snapshot with worker numbers — the
/// parent UI would oscillate between parent-turn and worker-turn
/// values, and on a multi-worker dispatch the last-writer-wins
/// outcome would be arbitrary. Worker token usage stays isolated
/// in `subagent_runs.token_usage_json` (written at worker exit by
/// `dispatch.rs`).
///
/// Silent no-op on a missing `session_id` (matches the legacy
/// `add_token_usage` contract — `UPDATE` matches 0 rows, no error).
pub async fn update_last_turn_usage(
    pool: &SqlitePool,
    session_id: &str,
    usage: &TokenUsage,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE sessions
        SET last_context_input_tokens = ?,
            last_input_tokens = ?,
            last_output_tokens = ?,
            last_cache_creation = ?,
            last_cache_read = ?,
            updated_at = ?
        WHERE id = ?
        "#,
    )
    .bind(usage.context_input_tokens as i64)
    .bind(usage.input_tokens as i64)
    .bind(usage.output_tokens as i64)
    .bind(usage.cache_creation_input_tokens as i64)
    .bind(usage.cache_read_input_tokens as i64)
    .bind(Utc::now().to_rfc3339())
    .bind(session_id)
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Worktree state transitions (step4 follow-up)
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
// D1: Session rename + color tag
// ---------------------------------------------------------------------------

/// Rename a session. Truncates to 80 chars on the server side.
pub async fn rename_session(
 pool: &SqlitePool,
 session_id: &str,
 new_title: &str,
) -> Result<(), sqlx::Error> {
 let now = Utc::now().to_rfc3339();
 let truncated: String = new_title.chars().take(80).collect();
 sqlx::query(
 r#"
 UPDATE sessions SET title = ?, updated_at = ? WHERE id = ?
 "#,
 )
 .bind(&truncated)
 .bind(&now)
 .bind(session_id)
 .execute(pool)
 .await?;
 Ok(())
}

/// Set (or clear) a session's color tag. `None` or out-of-range clears the
/// mark. Valid range: 0–7.
pub async fn set_session_color(
 pool: &SqlitePool,
 session_id: &str,
 color_tag: Option<i32>,
) -> Result<(), sqlx::Error> {
 let now = Utc::now().to_rfc3339();
 let tag = color_tag.filter(|&t| (0..=7).contains(&t));
 sqlx::query(
 r#"
 UPDATE sessions SET color_tag = ?, updated_at = ? WHERE id = ?
 "#,
 )
 .bind(tag)
 .bind(&now)
 .bind(session_id)
 .execute(pool)
 .await?;
 Ok(())
}

// ---------------------------------------------------------------------------
// System event injection (step4 follow-up)
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
 // (no RETURNING in3.35, no UPSERT-with-RETURNING before that).
 let next_seq: i64 = sqlx::query("SELECT COALESCE(MAX(seq), -1) +1 FROM messages WHERE session_id = ?")
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
 VALUES (?, 'user', ?, ?,0,0, ?, ?, ?)
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
///
/// F5 (LLM Latency Tracking): the optional `latency` carries the
/// three millisecond values (ttfb / gen / total) measured by the
/// frontend's `Date.now()` deltas around the `start` / first
/// `delta` / `done` events. The values are NULL when the caller
/// has not measured them (e.g. `tool_result` rows; the tool
/// result is emitted as a user-role row by the agent loop and
/// the latency is per assistant turn, not per tool). Pre-F5
/// callers can pass `None` and the columns stay NULL.
pub async fn persist_turn(
 pool: &SqlitePool,
 session_id: &str,
 role: Role,
 content: &MessageContent,
 seq: i64,
 latency: Option<&MessageLatency>,
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
 if b.iter().any(|x| matches!(x, ContentBlock::ToolUse { .. })));
 let has_tool_results = matches!(content, MessageContent::Blocks(b)
 if b.iter().any(|x| matches!(x, ContentBlock::ToolResult { .. })));

 sqlx::query(
 r#"
 INSERT INTO messages
 (session_id, role, content, text, has_tool_calls, has_tool_results, created_at, seq, ttfb_ms, gen_ms, total_ms, thinking_ms)
 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
 .bind(latency.and_then(|l| l.ttfb_ms))
 .bind(latency.and_then(|l| l.gen_ms))
 .bind(latency.and_then(|l| l.total_ms))
 // F5 follow-up: thinking-phase duration. Persisted
 // alongside the three latency columns in the same
 // INSERT — both go in at the moment the agent loop
 // calls `persist_turn` for the assistant row, which
 // is also the row the frontend will fire
 // `update_message_latency` / `update_message_thinking`
 // against (those IPCs are the patch-after-the-fact
 // path for rows persisted BEFORE the per-message
 // telemetry was wired through the agent loop).
 .bind(latency.and_then(|l| l.thinking_ms))
 .execute(pool)
 .await?;

 // Auto-title from first user message.
 if matches!(role, Role::User) {
 sqlx::query(
 r#"
 UPDATE sessions
 SET title = CASE
 WHEN title = '新对话' AND ? != '' THEN substr(?,1,50)
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
// F5: per-message latency + per-tool-result duration persistence
// ---------------------------------------------------------------------------

/// Three-field latency breakdown measured by the frontend around
/// the SSE event boundaries of one chat invocation. All three
/// fields are optional because the cancel / error paths may
/// only know the total (no `delta` was ever received → no
/// `ttfb_ms`).
///
/// Field semantics (mirrored in `.trellis/spec/backend/llm-contract.md`
/// "Scenario: Latency Tracking" §2):
/// - `ttfb_ms`: send → first `delta` event (time-to-first-byte)
/// - `gen_ms`:  first `delta` → `done` (active generation)
/// - `total_ms`: send → `done` (end-to-end; always set when
///   `total_ms.is_some()`)
/// - `thinking_ms`: F5 follow-up — first `thinking_delta` →
///   first non-thinking boundary (text `delta`, `tool:call`,
///   `done`, or `error`). `None` when the message never
///   entered the thinking phase. Drives the
///   "Thought for X.Xs" header in ThinkingBlock.vue.
#[derive(Debug, Clone, Copy, Default)]
pub struct MessageLatency {
 pub ttfb_ms: Option<i64>,
 pub gen_ms: Option<i64>,
 pub total_ms: Option<i64>,
 pub thinking_ms: Option<i64>,
}

/// Update the latency + thinking-time columns on an
/// already-persisted message row. Called from the frontend's
/// `streamController.handleChatEvent("done")` after the four
/// `Date.now()` deltas resolve (TTFB / gen / total +
/// thinking). Updates the assistant row's four columns in
/// one SQL statement; a no-op if the message id is unknown
/// (defensive — the controller could in principle race the
/// agent loop's `persist_turn` if the user cancels mid-stream
/// and the cancel cleanup path persists the partial turn at
/// a later time).
///
/// The `id` is the SQLite `messages.id` (auto-incrementing). The
/// controller tracks this via the `seq` on the assistant message;
/// the IPC layer looks up the id by `(session_id, seq)` and passes
/// it here. See `find_message_id_by_seq` for the helper.
pub async fn update_message_latency(
 pool: &SqlitePool,
 message_id: i64,
 latency: &MessageLatency,
) -> Result<(), sqlx::Error> {
 sqlx::query(
 r#"
 UPDATE messages
 SET ttfb_ms = ?, gen_ms = ?, total_ms = ?, thinking_ms = ?
 WHERE id = ?
 "#,
 )
 .bind(latency.ttfb_ms)
 .bind(latency.gen_ms)
 .bind(latency.total_ms)
 .bind(latency.thinking_ms)
 .bind(message_id)
 .execute(pool)
 .await?;
 Ok(())
}

/// Find a session message's auto-incrementing row id by its
/// caller-managed `seq`. Used by the F5 `update_message_latency`
/// IPC: the frontend tracks the seq of the assistant placeholder
/// (it appears in `messages.content` as a JSON-serialized
/// `Vec<ContentBlock>`), but doesn't know the SQLite id at
/// stream end (the row was inserted by the agent loop's
/// `persist_turn`, not by the frontend). This helper bridges
/// the two.
pub async fn find_message_id_by_seq(
 pool: &SqlitePool,
 session_id: &str,
 seq: i64,
) -> Result<Option<i64>, sqlx::Error> {
 let row: Option<(i64,)> = sqlx::query_as(
 "SELECT id FROM messages WHERE session_id = ? AND seq = ?",
 )
 .bind(session_id)
 .bind(seq)
 .fetch_optional(pool)
 .await?;
 Ok(row.map(|(id,)| id))
}

/// B2 PR3: write the per-user-turn `@relpath` injection
/// manifest to `messages.metadata`. Called from the agent loop
/// after `inject_at_tokens` returns the manifest — the
/// `persist_turn` call earlier in the same turn already wrote
/// the row with `metadata: None`, so this is a patch on top of
/// the just-inserted row.
///
/// The function is a single `UPDATE` keyed by
/// `(session_id, seq)`. The frontend rehydrate path reads
/// `metadata` back via `MessageRow.metadata` (see
/// `db::types.rs::MessageRow`) and parses it into the
/// `ChatMessage.injections` array. Bumps no `updated_at` —
/// the message is immutable from the moment it's inserted.
pub async fn update_message_metadata(
 pool: &SqlitePool,
 session_id: &str,
 seq: i64,
 metadata: &serde_json::Value,
) -> Result<(), sqlx::Error> {
 let meta_str = serde_json::to_string(metadata)
 .map_err(|e| sqlx::Error::Encode(format!("serialize metadata: {}", e).into()))?;
 sqlx::query(
 r#"
 UPDATE messages
 SET metadata = ?
 WHERE session_id = ? AND seq = ?
 "#,
 )
 .bind(&meta_str)
 .bind(session_id)
 .bind(seq)
 .execute(pool)
 .await?;
 Ok(())
}

/// Patch the `duration_ms` field onto a `tool_result` content
/// block embedded in `messages.content` JSON, keyed by
/// `tool_use_id`. Per PRD ADR-lite decision 1, the per-tool
/// duration is embedded in the `tool_result` block rather
/// than a column — zero schema change for the tool side.
///
/// The function reads the matching message row, walks the
/// `content` JSON array, finds the `tool_result` block with
/// the matching `tool_use_id`, and writes
/// `{"duration_ms": <n>}` into the block. Other blocks and
/// the rest of the message row are untouched. A missing
/// `tool_use_id` is a no-op (the controller could in principle
/// fire `tool:result` for a tool_use that hasn't been persisted
/// yet, e.g. if the agent loop bails out before `persist_turn`
/// runs — we don't want to surface that as an error).
///
/// Both user-role rows that carry `tool_result` blocks
/// (the post-tool-execution turn the agent loop persists)
/// AND assistant-role rows that were repaired by the
/// 2013-orphan fix are supported: the search walks every
/// `tool_result` block in the row's content array, so a
/// durationMs patch lands on whichever row holds the
/// matching block.
pub async fn record_tool_duration(
 pool: &SqlitePool,
 session_id: &str,
 tool_use_id: &str,
 duration_ms: i64,
) -> Result<bool, sqlx::Error> {
 // Load every message row in the session that has tool_results,
 // patch the matching block in memory, and UPDATE the row if
 // the patch landed. SQLite's `json_patch` is also an option
 // (no Rust-side parsing), but loading + writing in Rust keeps
 // the patch logic readable and gives a free `did we actually
 // find a block` boolean for the IPC return value.
 let rows = sqlx::query(
 r#"
 SELECT id, content FROM messages
 WHERE session_id = ? AND has_tool_results =1
 ORDER BY seq ASC
 "#,
 )
 .bind(session_id)
 .fetch_all(pool)
 .await?;

 for row in rows {
 let id: i64 = row.try_get("id")?;
 let content_str: String = row.try_get("content")?;
 let mut value: serde_json::Value = match serde_json::from_str(&content_str) {
 Ok(v) => v,
 Err(_) => continue, // corrupt content — skip silently
 };
 let Some(blocks) = value.as_array_mut() else {
 continue;
 };
 let mut patched = false;
 for block in blocks.iter_mut() {
 let Some(obj) = block.as_object_mut() else {
 continue;
 };
 let is_tool_result = obj.get("type").and_then(|v| v.as_str()) == Some("tool_result");
 if !is_tool_result {
 continue;
 }
 let matches = obj.get("tool_use_id").and_then(|v| v.as_str()) == Some(tool_use_id);
 if !matches {
 continue;
 }
 obj.insert(
 "duration_ms".to_string(),
 serde_json::Value::Number(duration_ms.into()),
 );
 patched = true;
 }
 if !patched {
 continue;
 }
 let new_content = serde_json::to_string(&value).map_err(|e| {
 sqlx::Error::Encode(format!("re-serialize content: {}", e).into())
 })?;
 sqlx::query("UPDATE messages SET content = ? WHERE id = ?")
 .bind(&new_content)
 .bind(id)
 .execute(pool)
 .await?;
 return Ok(true);
 }
 Ok(false)
}

// ---------------------------------------------------------------------------
// D3 (session 内消息编辑/重发, PR1 2026-06-17):
// edit_user_message — in-place content patch + cascade-delete tail + audit
// ---------------------------------------------------------------------------

/// Edit a single user message in place: replace its `content` / `text`
/// with the new value, stamp `messages.metadata` with `edited_at` and
/// `original_content`, cascade-delete every strictly later message
/// (so the next resend starts from a clean slate — the assistant
/// tool_use chain on row N+1+ no longer references the old prompt),
/// and append an `edit_message` audit row.
///
/// All three operations (UPDATE message + DELETE tail + INSERT audit)
/// run inside a single `sqlx::Transaction` so a partial failure cannot
/// leave the DB in a split-brain state (e.g. content updated but tail
/// not deleted → assistant turn still references the old prompt).
/// Matches the `emit_persist_failure` single-rollback invariant the
/// agent loop uses for its own persist sites (RULE-A-003, 2026-06-15).
///
/// ### No-op fast path
///
/// If the new `content` serializes to the same JSON as the current
/// row's `content`, the function is a no-op: it returns `Ok(())`
/// without writing any state. The caller (the `edit_user_message`
/// Tauri command) sees success and the audit log gets no row —
/// this avoids spurious audit entries on save-without-change clicks.
///
/// ### `original_content` semantics
///
/// `original_content` is the JSON-serialized value of the row BEFORE
/// this edit. The first edit on a row writes `original_content` from
/// the previously-stored `content`; subsequent edits (re-edit of an
/// already-edited row) do NOT overwrite `original_content` — it
/// always points at the original (pre-any-edit) value. This gives a
/// future "undo edit" affordance a stable restore target.
///
/// ### `edited_at` semantics
///
/// `edited_at` is the RFC3339 timestamp of the latest edit on this
/// row. It is overwritten on every edit (so the UI can show "last
/// edited at X"). NULL for never-edited rows.
///
/// ### Cascade delete scope
///
/// `DELETE FROM messages WHERE session_id = ? AND seq > ?` removes
/// every strictly-later message in the session — assistant turns,
/// tool_result turns, the synthetic tool_result orphan-repair rows,
/// etc. The `messages` table has no FKs to other tables (just an
/// index on `(session_id, seq)`), so a single DELETE is enough — no
/// other table holds a reference to a `messages.id`. Audit events
/// (`session_audit_events`) are NOT touched: they record what the
/// agent DID, not the live message buffer, so they survive the
/// cascade delete (mirrors `delete_messages_by_session` semantics in
/// `B3 /clear`, `sessions.rs:265-274`).
///
/// ### Atomicity
///
/// A single `sqlx::Transaction` wraps the entire flow. If any of
/// the three SQL calls fails, the transaction is dropped (sqlx
/// auto-rollback on Drop) and the function returns the underlying
/// `sqlx::Error`. The caller wraps the error in
/// `emit_persist_failure`-style error handling.
///
/// ### Permission
///
/// This function does NOT consult the ⑨ 关 permission layer. Edit is
/// a user-initiated direct IPC call, not an LLM tool invocation; the
/// industry consensus (Cursor / Cline / Cody / OpenHands / OpenCode;
/// see `.trellis/tasks/06-17-d3-message-edit-resend/research/industry-edit-resend.md`)
/// is to bypass the modal entirely. The audit log captures every
/// edit so the user can review changes later.
///
/// ### Args
///
/// - `session_id` — the session containing the row to edit. The
///   cascade delete is scoped to this session.
/// - `message_seq` — the caller-managed `seq` of the user message to
///   edit. Resolved to the auto-incrementing `id` via
///   `find_message_id_by_seq`. Returns `Ok(())` silently if the
///   pair is unknown (defensive — the frontend's view can race the
///   agent loop's persist on a mid-stream edit/cancel).
/// - `new_content` — the new `MessageContent` to write. The
///   `text` column is denormalized from `MessageContent::to_text()`
///   (excludes thinking text per the project invariant).
pub async fn edit_user_message(
 pool: &SqlitePool,
 session_id: &str,
 message_seq: i64,
 new_content: &MessageContent,
) -> Result<(), sqlx::Error> {
 // 1. Resolve (session_id, seq) → message_id. The same helper
 // F5 uses for the latency IPC; the seq is unique per session
 // by the UNIQUE(session_id, seq) constraint. Returns Ok(())
 // silently on unknown pair to mirror `update_message_latency`'s
 // defensive no-op contract.
 let message_id = match find_message_id_by_seq(pool, session_id, message_seq).await? {
 Some(id) => id,
 None => return Ok(()),
 };

 // 2. Read the current `content` for the no-op check + for
 // the `original_content` backup. We need it inside the
 // transaction (concurrent edit/cancel races) so a later
 // writer doesn't sneak in between the read and the UPDATE.
 let mut tx = pool.begin().await?;

 let current_content_str: Option<String> = sqlx::query_scalar(
 "SELECT content FROM messages WHERE id = ? AND session_id = ?",
 )
 .bind(message_id)
 .bind(session_id)
 .fetch_optional(&mut *tx)
 .await?;
 let current_content_str = match current_content_str {
 Some(s) => s,
 // The row vanished between the find_message_id_by_seq and
 // the SELECT (e.g. concurrent cascade delete). No-op.
 None => {
 tx.rollback().await?;
 return Ok(());
 }
 };

 // 3. No-op fast path: if the new content serializes to the
 // same JSON as the current row, return without writing. This
 // keeps the audit log clean on save-without-change clicks and
 // avoids spurious `edited_at` bumps.
 let new_content_json = serde_json::to_string(new_content)
 .map_err(|e| sqlx::Error::Encode(format!("serialize content: {}", e).into()))?;
 if new_content_json == current_content_str {
 tx.rollback().await?;
 return Ok(());
 }

 // 4. Read the current metadata (if any) to decide whether to
 // seed `original_content` from the pre-edit value (first edit
 // only — subsequent edits preserve the original). We use
 // SQLite's `json_extract` so the parse stays on the SQL side.
 let existing_edited_at: Option<String> = sqlx::query_scalar(
 r#"
 SELECT json_extract(metadata, '$.edited_at')
 FROM messages WHERE id = ?
 "#,
 )
 .bind(message_id)
 .fetch_one(&mut *tx)
 .await?;
 let already_edited = existing_edited_at.is_some();

 // 5. Build the new metadata JSON. `edited_at` is always
 // overwritten (latest edit timestamp); `original_content` is
 // seeded on the FIRST edit only — subsequent edits preserve
 // the original so a future "undo edit" affordance can restore
 // the pre-any-edit text.
 //
 // SQLite's `json_patch` (RFC 7396) merges the patch into the
 // existing metadata object. When the existing metadata is
 // `NULL` (no prior metadata), `json_patch` returns the patch
 // object directly — no extra branch needed.
 let now = Utc::now().to_rfc3339();
 let metadata_patch = if already_edited {
 serde_json::json!({ "edited_at": &now }).to_string()
 } else {
 // First edit: parse the current content as JSON (it's the
 // serialized `MessageContent`). If parsing fails, fall back
 // to the string form so the backup is never lossy.
 let original_content_value = serde_json::from_str(&current_content_str)
 .unwrap_or_else(|_| serde_json::Value::String(current_content_str.clone()));
 serde_json::json!({
 "edited_at": &now,
 "original_content": original_content_value,
 })
 .to_string()
 };
 let new_metadata_json: String = sqlx::query_scalar(
 r#"
 SELECT json_patch(COALESCE(metadata, '{}'), ?)
 FROM messages WHERE id = ?
 "#,
 )
 .bind(&metadata_patch)
 .bind(message_id)
 .fetch_one(&mut *tx)
 .await?;

 let new_text = new_content.to_text();
 sqlx::query(
 r#"
 UPDATE messages
 SET content = ?, text = ?, metadata = ?
 WHERE id = ? AND session_id = ?
 "#,
 )
 .bind(&new_content_json)
 .bind(&new_text)
 .bind(&new_metadata_json)
 .bind(message_id)
 .bind(session_id)
 .execute(&mut *tx)
 .await?;

 // 6. Cascade-delete every strictly-later message in this
 // session. This wipes the (now-stale) assistant turn, the
 // tool_result turns, the orphan-repair rows — everything that
 // chained off the old user prompt. The next resend starts
 // from a clean slate.
 //
 // Single-table FK story: `messages` has no outgoing FKs to
 // other tables (only an index on `(session_id, seq)`), so the
 // DELETE doesn't cascade anywhere. Audit events
 // (`session_audit_events`) are session-scoped and intentionally
 // kept — they record what the agent DID, not the live
 // message buffer.
 sqlx::query(
 "DELETE FROM messages WHERE session_id = ? AND seq > ?",
 )
 .bind(session_id)
 .bind(message_seq)
 .execute(&mut *tx)
 .await?;

 // 7. Audit row. Single INSERT into `session_audit_events`
 // with kind `edit_message` (mirrors the
 // `AuditKind::EditMessage` enum string in
 // `agent::permissions::AuditKind::as_str`). The chat command
 // path uses the string literal directly so the cross-module
 // call graph stays tight (same pattern as
 // `set_session_mode`'s `mode_changed` audit).
 let audit_payload = serde_json::json!({
 "message_seq": message_seq,
 "new_text_preview": new_text.chars().take(80).collect::<String>(),
 "edited_at": &now,
 })
 .to_string();
 sqlx::query(
 r#"
 INSERT INTO session_audit_events
 (session_id, ts, kind, payload_json)
 VALUES (?, datetime('now'), 'edit_message', ?)
 "#,
 )
 .bind(session_id)
 .bind(&audit_payload)
 .execute(&mut *tx)
 .await?;

 // 8. Commit. Any error in steps 2-7 leaves the transaction
 // uncommitted and sqlx drops it (auto-rollback on Drop),
 // giving the caller a clean `sqlx::Error` to wrap in
 // `emit_persist_failure`.
 tx.commit().await?;
 Ok(())
}
