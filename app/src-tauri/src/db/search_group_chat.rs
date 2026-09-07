//! GCE M4b (discussion-library search, 2026-09-07): **field-level**
//! browse + keyword search over historical group-chat discussion
//! **sessions** (one hit = one session / one conclusion document),
//! for the GUI "讨论库" panel. Message-level search is already covered
//! by `messages_fts` (`db/search.rs`) — this module deliberately does
//! NOT touch that path.
//!
//! Index shape decision (design §3/§4, Q4): **no new table, no FTS
//! triggers**. The document IS the `sessions` row — `session_type =
//! 'group_chat'` selects the field-level set, and every searchable
//! field is either a plain column (`title`, `discussion_summary`,
//! `stop_reason`) or parsed from the `metadata` JSON blob
//! (`participants[].name`, `scheduled_task_name`). Session counts are
//! 1-2 orders of magnitude below `messages`, so a `LIKE` scan is
//! millisecond-level; the FTS5 external-content template in
//! `database-guidelines.md` is the documented upgrade path if the
//! session count ever crosses ~10^3.
//!
//! The session row itself stays the source of truth for writers —
//! creation / finalize (`finalize_group_chat_lifecycle`) / delete are
//! untouched; a fresh session created after a browse simply appears on
//! the next query.
//!
//! LIKE matching contract (mirrors `db/search.rs`): every user-supplied
//! keyword is `%q%`-wrapped and escaped via [`escape_like`] so a query
//! containing `%` / `_` / `\` matches literally (wildcard literalism).
//! Participants + task_name are matched through `json_each` /
//! `json_extract` (defensive `json_valid` guard — malformed metadata
//! must not raise the whole query, same red line as the audit-log
//! keyset pattern in `database-guidelines.md`).

use serde::{Deserialize, Serialize};
use sqlx::Row;
use sqlx::SqlitePool;

use crate::db::search::escape_like;

/// One historical group-chat session hit — a single "场" of a
/// discussion. Wire snake_case (sessions-domain convention, no serde
/// rename). `task_name` / `participants` are parsed from
/// `sessions.metadata`; both are `None`/empty for sessions whose
/// discussion never persisted a config (defensive — the classic chat
/// rows are excluded by `session_type` anyway).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupChatSessionHit {
    pub session_id: String,
    pub project_id: String,
    pub title: String,
    /// `metadata.scheduled_task_name` — present only for
    /// scheduler-fired discussions (定时场). `None` for ad-hoc /
    /// GUI-created group chats.
    pub task_name: Option<String>,
    /// `metadata.participants[].name`, joined in metadata order.
    pub participants: Vec<String>,
    /// Terminal orchestration reason (`group_chat_end` / `max_rounds`
    /// / `cancelled` / `error` / `preempted` / `interrupted`); `None`
    /// while a discussion has never terminally finalized.
    pub stop_reason: Option<String>,
    /// The moderator's `end_discussion({summary})` conclusion
    /// (first-class column, GC7). `None` until the discussion ended
    /// via `end_discussion` — mid-flight sessions stay searchable by
    /// title / task / participants (AC5).
    pub discussion_summary: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Field-level filter knobs shared by the browse + search queries.
#[derive(Debug, Clone, Default)]
pub struct GroupChatSessionFilters {
    /// Restrict to one project.
    pub project_id: Option<String>,
    /// Restrict to a single terminal stop_reason (exact match).
    pub stop_reason: Option<String>,
}

/// Browse every historical group-chat session, newest-activity first.
/// Empty keyword list mode — the DiscussionLibrary panel shows this
/// list on open (R2).
pub async fn list_group_chat_sessions(
    pool: &SqlitePool,
    filters: &GroupChatSessionFilters,
) -> Result<Vec<GroupChatSessionHit>, sqlx::Error> {
    fetch_group_chat_hits(pool, filters, None).await
}

/// Keyword search over group-chat sessions. A non-empty query matches
/// when it hits **any** of title / discussion_summary /
/// scheduled_task_name / any participant name (R2/AC2). Empty /
/// whitespace query degrades to [`list_group_chat_sessions`] (browse).
pub async fn search_group_chat_discussions(
    pool: &SqlitePool,
    query: &str,
    filters: &GroupChatSessionFilters,
) -> Result<Vec<GroupChatSessionHit>, sqlx::Error> {
    let q = query.trim();
    if q.is_empty() {
        return list_group_chat_sessions(pool, filters).await;
    }
    fetch_group_chat_hits(pool, filters, Some(q)).await
}

/// Shared row query for both surfaces. `keyword = None` → plain
/// browse; `Some` → add the LIKE OR-block across all four searchable
/// fields. Filters are interpolated as bound `?`s in declaration
/// order (project_id → stop_reason → the 4 keyword patterns).
async fn fetch_group_chat_hits(
    pool: &SqlitePool,
    filters: &GroupChatSessionFilters,
    keyword: Option<&str>,
) -> Result<Vec<GroupChatSessionHit>, sqlx::Error> {
    let mut sql = String::from(
        r#"
        SELECT s.id, s.project_id, s.title, s.metadata, s.stop_reason,
               s.discussion_summary, s.created_at, s.updated_at
        FROM sessions s
        WHERE s.session_type = 'group_chat'
        "#,
    );
    let mut binds: Vec<String> = Vec::new();
    if let Some(pid) = &filters.project_id {
        sql.push_str(" AND s.project_id = ?");
        binds.push(pid.clone());
    }
    if let Some(reason) = &filters.stop_reason {
        sql.push_str(" AND s.stop_reason = ?");
        binds.push(reason.clone());
    }
    if let Some(q) = keyword {
        // `%`-wrap + escape: `%`/`_`/`\` in the query match literally
        // (same contract as db/search.rs's LIKE fallback).
        let pattern = format!("%{}%", escape_like(q));
        sql.push_str(
            r#"
            AND (
                s.title LIKE ? ESCAPE '\'
                OR COALESCE(s.discussion_summary, '') LIKE ? ESCAPE '\'
                OR (
                    json_valid(s.metadata)
                    AND COALESCE(
                        json_extract(s.metadata, '$.scheduled_task_name'), ''
                    ) LIKE ? ESCAPE '\'
                )
                OR (
                    json_valid(s.metadata)
                    AND EXISTS (
                        SELECT 1
                        FROM json_each(s.metadata, '$.participants') AS p
                        WHERE json_extract(p.value, '$.name') LIKE ? ESCAPE '\'
                    )
                )
            )
            "#,
        );
        for _ in 0..4 {
            binds.push(pattern.clone());
        }
    }
    sql.push_str(" ORDER BY s.updated_at DESC");

    let mut query = sqlx::query(&sql);
    for b in &binds {
        query = query.bind(b);
    }
    let rows = query.fetch_all(pool).await?;

    let mut hits = Vec::with_capacity(rows.len());
    for r in rows {
        // metadata is TEXT-or-NULL (JSON string); parsed defensively
        // below — a corrupt value must degrade to no task/participants,
        // not fail the whole browse.
        let metadata: Option<String> = r.try_get("metadata")?;
        let (task_name, participants) = parse_session_metadata(metadata.as_deref());
        hits.push(GroupChatSessionHit {
            session_id: r.try_get("id")?,
            project_id: r.try_get("project_id")?,
            title: r.try_get("title")?,
            task_name,
            participants,
            stop_reason: r.try_get("stop_reason")?,
            discussion_summary: r.try_get("discussion_summary")?,
            created_at: r.try_get("created_at")?,
            updated_at: r.try_get("updated_at")?,
        });
    }
    Ok(hits)
}

/// Read the searchable field-level keys out of `sessions.metadata`.
/// Keys mirror what `group_chat_loop` / the scheduler fire writes
/// (`participants[].name`, `scheduled_task_name`); bad / absent JSON
/// degrades to `(None, vec![])` — a session without a saved config
/// stays visible (browse) but matches no task/participant keyword.
fn parse_session_metadata(metadata: Option<&str>) -> (Option<String>, Vec<String>) {
    let Some(raw) = metadata else {
        return (None, Vec::new());
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return (None, Vec::new());
    };
    let task_name = v
        .get("scheduled_task_name")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string());
    let participants = v
        .get("participants")
        .and_then(|p| p.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| {
                    p.get("name")
                        .and_then(|n| n.as_str())
                        .map(|s| s.to_string())
                })
                .collect()
        })
        .unwrap_or_default();
    (task_name, participants)
}
