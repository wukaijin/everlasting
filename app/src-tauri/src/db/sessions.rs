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
 worktree_path, worktree_state, last_worktree_path, model_id)
 VALUES (?, ?, ?, ?, ?, NULL, ?, ?, NULL, 'none', NULL, ?)
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
 cache_creation_total, cache_read_total
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
 input_tokens_total: r.try_get("input_tokens_total")?,
 output_tokens_total: r.try_get("output_tokens_total")?,
 cache_creation_total: r.try_get("cache_creation_total")?,
 cache_read_total: r.try_get("cache_read_total")?,
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
// A4: per-session token usage accumulation
// ---------------------------------------------------------------------------

/// Accumulate one turn's [`TokenUsage`] into the session's per-column
/// totals. Single SQL UPDATE, additive on the existing column
/// values; a session that has N LLM turns ends up with the
/// column-wise sum.
///
/// All four totals are updated in one statement so the row stays
/// consistent (a partial write — input but not output, etc — would
/// be a subtle bug visible as "input climbed but output didn't").
/// NULL columns are treated as 0 by SQLite's `+` operator, so a
/// pre-A4 session's first turn starts the counters from 0
/// (subsequent UI loads show the running total, not "—").
///
/// The chat command calls this once per `ChatEvent::Done` with
/// `usage: Some(t)`. Cancel / error / network drop paths pass
/// `usage: None`; the chat command skips the call entirely in
/// that case (no `add_token_usage(_, _)` invocation). See
/// `agent::chat::chat` for the call site.
pub async fn add_token_usage(
 pool: &SqlitePool,
 session_id: &str,
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
 if b.iter().any(|x| matches!(x, ContentBlock::ToolUse { .. })));
 let has_tool_results = matches!(content, MessageContent::Blocks(b)
 if b.iter().any(|x| matches!(x, ContentBlock::ToolResult { .. })));

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
