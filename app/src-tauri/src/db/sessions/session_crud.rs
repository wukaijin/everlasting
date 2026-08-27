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

use crate::llm::types::TokenUsage;

use super::super::types::{LoadedSession, MessageRow, SessionRow, SessionSummary, WorktreeState};

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
#[allow(clippy::too_many_arguments)]
pub async fn create_session(
    pool: &SqlitePool,
    session_id: &str,
    project_id: &str,
    initial_cwd: &str,
    model: &str,
    model_id: Option<&str>,
    // Group chat (07-29-group-chat, Phase 4 Step 3): `None` /
    // `Some("chat")` for classic chat (default; matches the
    // column DEFAULT 'chat'). `Some("group_chat")` for the
    // multi-LLM session type. Backed by the `sessions.session_type`
    // column (Phase 1 migration, nullable in tests / never
    // unset in production).
    session_type: Option<&str>,
    // Group chat (07-29-group-chat, Phase 4 Step 3): JSON-encoded
    // metadata blob. `None` for classic chat. `Some(json)` for
    // group-chat sessions (currently `{participants: [{...}]}`
    // per the `GroupChatConfig` model). Backed by the
    // `sessions.metadata` JSON column (Phase 1 migration, column
    // already exists for legacy subagent use cases — this is the
    // first primary consumer).
    metadata: Option<&str>,
) -> Result<SessionRow, sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    let title = "新对话".to_string();
    // Phase 4 default: 'chat' (matches column DEFAULT). The
    // group-chat caller passes Some("group_chat"). Stored as a
    // typed enum on the in-memory row (`SessionType::Chat`),
    // but the SQL column is plain TEXT for forward compatibility.
    let session_type = session_type.unwrap_or("chat");
    let session_type_typed = crate::db::SessionType::from_str_opt(session_type);

    // The `mode` slot below MUST stay 'edit' (mirroring the struct
    // field). 2026-08-18 (5df29977 问题4): b999803 wrote the legacy
    // 'chat' here — confused with session_type's DEFAULT 'chat' —
    // so new sessions' DB rows disagreed with the returned
    // `Mode::Edit` struct. The every-init `chat→edit` scrub
    // migration masked it: only sessions created after the last
    // process start kept the bad value (2 rows in that incident).
    sqlx::query(
        r#"
 INSERT INTO sessions
 (id, title, created_at, updated_at, model, metadata, project_id, current_cwd,
 worktree_path, worktree_state, last_worktree_path, model_id, color_tag, mode, workflow_enabled, plugin_name, session_type)
 VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL, 'none', NULL, ?, NULL, 'edit', 0, 'dev', ?)
 "#,
    )
    .bind(session_id)
    .bind(&title)
    .bind(&now)
    .bind(&now)
    .bind(model)
    .bind(metadata)
    .bind(project_id)
    .bind(initial_cwd)
    .bind(model_id)
    .bind(session_type)
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
        workflow_enabled: false,
        // Step 2.2: default plugin is `dev`. The migration
        // column also has DEFAULT 'dev', so the bare INSERT
        // above is consistent with the SELECT-without-bind
        // fallback; we set the field explicitly here so the
        // returned struct matches the row verbatim (no
        // round-trip race for callers that read the return
        // value before any SELECT).
        plugin_name: "dev".to_string(),
        // Group chat (07-29-group-chat, Phase 4 Step 3):
        // reflect the `session_type` arg into the typed
        // struct so the caller sees the row verbatim (no
        // round-trip race; same convention as the previous
        // hard-coded `::Chat` default).
        session_type: session_type_typed,
        metadata: metadata.and_then(|s| serde_json::from_str(s).ok()),
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
 s.color_tag, s.mode, s.workflow_enabled, s.plugin_name,
 s.session_type, s.metadata,
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
            let color_tag: Option<i32> = r.try_get("color_tag")?;
            let mode_str: String = r.try_get("mode")?;
            // Group chat (07-29-group-chat): parse session_type +
            // metadata the same defensive way as the full SessionRow
            // load below. metadata is JSON-or-NULL; a malformed value
            // is swallowed (→ None) so it can never break the sidebar.
            let session_type_str: String = r.try_get("session_type")?;
            let metadata: Option<serde_json::Value> = r
                .try_get::<Option<String>, _>("metadata")?
                .and_then(|s| serde_json::from_str(&s).ok());
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
                workflow_enabled: r.try_get::<i64, _>("workflow_enabled")? != 0,
                plugin_name: r.try_get("plugin_name")?,
                session_type: crate::db::SessionType::from_str_opt(&session_type_str),
                metadata,
                // Runtime state, not DB state — enriched by
                // `list_sessions_inner` from `session_active_request`.
                busy: false,
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
 color_tag, mode, workflow_enabled, plugin_name,
 session_type, metadata
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
            // Group chat (07-29-group-chat): parse session_type
            // (defensive fallback to Chat) + metadata (JSON-or-NULL,
            // malformed swallowed). Mirrors the list_sessions map.
            let session_type_str: String = r.try_get("session_type")?;
            let metadata: Option<serde_json::Value> = r
                .try_get::<Option<String>, _>("metadata")?
                .and_then(|s| serde_json::from_str(&s).ok());
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
                workflow_enabled: r.try_get::<i64, _>("workflow_enabled")? != 0,
                plugin_name: r.try_get("plugin_name")?,
                session_type: crate::db::SessionType::from_str_opt(&session_type_str),
                metadata,
            }
        }
        None => return Ok(None),
    };

    let msg_rows = sqlx::query(
        r#"
 SELECT id, session_id, role, content, text, has_tool_calls, has_tool_results,
 created_at, seq, metadata, ttfb_ms, gen_ms, total_ms, thinking_ms,
 speaker, status
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
                // `update_message_thinking` IPC at stream end.
                thinking_ms: r.try_get("thinking_ms")?,
                // Group chat (07-29-group-chat, Phase 4 TODO-B): the
                // originating speaker for this message. `None` for
                // classic chat / subagent / review messages (no
                // behavior change vs. pre-Phase 4). For group-chat
                // sessions, set to "moderator" or participant.name by
                // the per-turn `current_speaker` parameter (see
                // `chat_loop.rs:run_chat_loop` + `persist_turn`). The
                // frontend renders this as a chip + accent color.
                speaker: r.try_get("speaker")?,
                // RULE-PERSIST-001 (08-24-p1-turn-crash-recovery):
                // 终态行 NULL;崩溃恢复过的行 'interrupted'。流式
                // 进行中的检查点行 'in_progress' 只在"读取与活跃
                // 流并发"(daemon 存活时 reload 页面)可见 —— WP3
                // 据此把流式占位替换为检查点内容;跨进程重启后
                // 该形态不可见(启动恢复 pass 先于任何 chat 跑)。
                status: r.try_get("status")?,
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

/// W1 (Workflow integration, Step 0.2 — 2026-07-08):
/// per-session workflow opt-in toggle. `true` = the agent
/// follows the active plugin's state machine; `false` =
/// default behavior (the existing chat loop, unchanged).
///
/// Mirrors `set_session_color`'s contract: a single column
/// flip, no audit row. Workflow *toggling* is a UI preference
/// (akin to color tag); the audit-grade events for the
/// workflow machinery itself — `workflow_toggled`,
/// `state_transition`, `spec_distilled`, etc. — land in
/// Phase 3 alongside `set_task_state`'s Rust fixed hook
/// (Step 3.1). The DB column is `INTEGER NOT NULL DEFAULT 0`
/// (see `db::migrations::run_migrations`); we bind the
/// boolean as `i64 1/0` to match the column type and the
/// `try_get` readers in `list_sessions` /
/// `load_session`.
///
/// Returns `Ok(())` even when `session_id` matches no row —
/// `sqlx::query::execute` reports `rows_affected == 0` but
/// does NOT raise an error, so an unknown id is a silent
/// no-op rather than a surface-level failure. Mirrors
/// `set_session_color`'s lenient contract.
pub async fn set_session_workflow_enabled(
    pool: &SqlitePool,
    session_id: &str,
    enabled: bool,
) -> Result<(), sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    let value: i64 = if enabled { 1 } else { 0 };
    sqlx::query(
        r#"
 UPDATE sessions SET workflow_enabled = ?, updated_at = ? WHERE id = ?
 "#,
    )
    .bind(value)
    .bind(&now)
    .bind(session_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// W1 (Workflow integration, Step 2.2 — 2026-07-08):
/// per-session workflow plugin name. The frontend's
/// `PluginSelect.vue` chip writes this on click; the
/// engine's `build_workflow_ctx` reads it on every IPC
/// entry to call `load_workflow(plugin_name, project_path)`
/// — so the breadcrumb reflects whatever plugin is
/// currently selected by the next turn.
///
/// **No validation here** — the column accepts any non-empty
/// string. The loader (`load_workflow`) is responsible for
/// validating the on-disk JSON shape; if `plugin_name`
/// doesn't match a real plugin dir, the loader falls back
/// to `default_workflow()` (which is itself `dev` per
/// `WorkflowDef::name`), so a stale name never breaks the
/// engine — it just gets the dev workflow until the user
/// picks a real one.
///
/// **Naming rules**: ASCII snake_case, English (per W1 AC
/// §非功能). Empty string is rejected at the IPC layer
/// (see `commands::sessions::set_session_plugin_name`).
///
/// Returns `Ok(())` even when `session_id` matches no row —
/// mirrors `set_session_workflow_enabled`'s lenient contract
/// (unknown id is a silent no-op rather than a surfaced
/// error).
pub async fn set_session_plugin_name(
    pool: &SqlitePool,
    session_id: &str,
    plugin_name: &str,
) -> Result<(), sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
 UPDATE sessions SET plugin_name = ?, updated_at = ? WHERE id = ?
 "#,
    )
    .bind(plugin_name)
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
    let next_seq: i64 =
        sqlx::query("SELECT COALESCE(MAX(seq), -1) +1 FROM messages WHERE session_id = ?")
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

/// C3 摘要压缩 PR2(08-18-llm-context-compaction,design §4.3):
/// 插入一行 LLM 压缩摘要(`role='user'` + `metadata.kind =
/// compaction_summary`),仿 [`insert_system_event`] 的落库形态。
///
/// **与 insert_system_event 的两处关键差异**:
///
/// 1. **seq 吃传入游标,返回推进值**(复核 P1 级):绝不走独立
///    `MAX(seq)+1` —— 活跃 loop 内 loop 自己持有内存 seq 游标,独立
///    MAX+1 会与 loop 的下一次 persist 撞 `(session_id, seq)` 主键。
///    调用方(drive.rs C3 块)把 loop 当前 `seq` 传入,摘要行落在
///    `seq` 处(即当前 turn 持久化行之前,水位链天然有序),返回
///    `seq + 1` 作为后续 persist 的新游标。
/// 2. **content 与 text 两列同值写纯摘要**(PR1 check 固化契约):
///    前端 rehydrate 管线对 text-only user 行回发 `text` 列原文
///    (水位替换的对齐锚点,见 `agent::compaction` 模块文档),
///    折叠消息从 `content` 列重建 —— 两列分叉会让 in-context 摘要
///    与对齐/前端展示所用文本漂移。`insert_system_event` 先例本身
///    两列不同值(content 带 "[worktree event] " 前缀),**别照抄**。
///    回填前缀话术只在 in-context 构建时拼接,绝不落库(评审 P1-2)。
///
/// `metadata` 由调用方组装(design §2.1 字段:tokens_before/after、
/// trigger、model、prior_summary_seq、summary_usage 等),本函数只
/// 负责 kind 之外原样透传 —— kind 由调用方写入(与常量
/// `crate::agent::compaction::COMPACTION_SUMMARY_KIND` 对齐)。
///
/// handoff(08-18-handoff-mechanism)复用本函数落接力行:`kind =
/// handoff_summary` + `summary_text` 为 prefix+摘要 自包含落库(与
/// compaction_summary"前缀不落库"契约的有意分歧,见
/// `crate::agent::compaction::HANDOFF_SUMMARY_KIND` 文档)。
pub async fn insert_compaction_summary(
    pool: &SqlitePool,
    session_id: &str,
    summary_text: &str,
    seq: i64,
    metadata: &serde_json::Value,
) -> Result<i64, sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    // content = 单 Text 块 JSON(insert_system_event 同款形态,
    // rehydrate 路径可直接解析);text = 同值纯摘要。
    let content_json = serde_json::json!([{ "type": "text", "text": summary_text }]).to_string();
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
    .bind(summary_text)
    .bind(&now)
    .bind(seq)
    .bind(metadata.to_string())
    .execute(pool)
    .await?;
    // 推进后的游标:摘要行占了 `seq`,后续 persist 从 seq+1 起。
    Ok(seq + 1)
}

/// 覆写 `sessions.metadata`(整块 JSON 写入)。handoff
/// (08-18-handoff-mechanism)parent 侧 `handoff_children` 合并写入的
/// 落点;调用方负责读-改-写合并语义(handoff 用户驱动低频,并发
/// clobber 风险接受 —— task design §3.8;需硬化时换 SQLite
/// `json_set` 原子合并)。
pub async fn set_session_metadata(
    pool: &SqlitePool,
    session_id: &str,
    metadata: &serde_json::Value,
) -> Result<(), sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
 UPDATE sessions
 SET metadata = ?, updated_at = ?
 WHERE id = ?
 "#,
    )
    .bind(metadata.to_string())
    .bind(&now)
    .bind(session_id)
    .execute(pool)
    .await?;
    Ok(())
}
