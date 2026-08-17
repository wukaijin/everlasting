//! D2 (cross-session search, 2026-08-17): full-text search over
//! `messages.text` across all sessions, backed by the `messages_fts`
//! FTS5 index created in `migrations/schema.rs`.
//!
//! This is the **shared query layer** for ROADMAP D2's two drivers:
//! ① the user-facing search modal (this task) and ② the agent-driven
//! `search_history` tool (follow-up). Both call [`search_messages`];
//! they differ only in presentation, not in SQL.
//!
//! Dispatch contract (design §2):
//! - ≥3 unicode chars → FTS5 `MATCH` (phrase-escaped) + `bm25` rank.
//!   The trigram tokenizer cannot match shorter queries (verified
//!   empirically for the memories FTS — see schema.rs notes), so:
//! - <3 chars → `LIKE '%q%'` fallback (recency-ordered). Personal-DB
//!   scale makes the full scan acceptable; this is what keeps
//!   2-char Chinese queries (e.g. "权限") searchable.
//! - Title hits (`sessions.title LIKE`) ride along on both paths so
//!   one IPC round-trip returns both kinds, discriminated by `kind`.
//!
//! Snippets are cut in Rust (not FTS5 `snippet()`) so both dispatch
//! paths share one wire contract; the frontend locates the query
//! inside the snippet itself (`toLowerCase().indexOf`) for
//! highlighting — no cross-language index arithmetic on the wire.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::db::memories::escape_fts5;

/// Which surface a search hit came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchHitKind {
    /// Matched the session's title (no message-level fields).
    Title,
    /// Matched a message body (`messages.text`).
    Content,
}

/// One search result row. Message-level fields are `None` for
/// [`SearchHitKind::Title`] hits; session-level fields are always
/// set (wire snake_case per BACKLOG §5.2 — no serde rename).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageSearchHit {
    pub kind: SearchHitKind,
    pub session_id: String,
    pub session_title: String,
    pub project_id: String,
    pub project_name: Option<String>,
    pub updated_at: String,
    // Content-hit-only fields (None for Title hits).
    pub seq: Option<i64>,
    pub role: Option<String>,
    pub speaker: Option<String>,
    /// ~±100-char window around the first match, char-boundary safe.
    pub snippet: Option<String>,
}

/// Upper bound for `limit` — one modal page of grouped results.
const MAX_LIMIT: u32 = 200;
const DEFAULT_LIMIT: u32 = 50;

/// Trigram tokenizer minimum — shorter queries cannot MATCH.
const FTS_MIN_CHARS: usize = 3;

pub async fn search_messages(
    pool: &SqlitePool,
    query: &str,
    project_id: Option<&str>,
    limit: Option<u32>,
) -> Result<Vec<MessageSearchHit>, sqlx::Error> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as i64;

    let mut hits = title_hits(pool, q, project_id, limit).await?;
    hits.extend(content_hits(pool, q, project_id, limit).await?);
    Ok(hits)
}

/// `sessions.title LIKE` on both dispatch paths. Titles are short
/// and sessions are few — a LIKE scan is trivial and keeps title
/// matching working for sub-3-char queries where FTS can't help.
/// Ordered by `updated_at DESC` (most recent project activity
/// first); the frontend regroups anyway.
async fn title_hits(
    pool: &SqlitePool,
    q: &str,
    project_id: Option<&str>,
    limit: i64,
) -> Result<Vec<MessageSearchHit>, sqlx::Error> {
    let pattern = format!("%{}%", escape_like(q));
    let sql = r#"
        SELECT s.id, s.title, s.project_id, p.name AS project_name, s.updated_at
        FROM sessions s
        LEFT JOIN projects p ON p.id = s.project_id
        WHERE s.title LIKE ? ESCAPE '\'
    "#;
    let sql = match project_id {
        Some(_) => format!("{sql} AND s.project_id = ? ORDER BY s.updated_at DESC LIMIT ?"),
        None => format!("{sql} ORDER BY s.updated_at DESC LIMIT ?"),
    };
    let mut query =
        sqlx::query_as::<_, (String, String, String, Option<String>, String)>(&sql).bind(&pattern);
    if let Some(pid) = project_id {
        query = query.bind(pid);
    }
    let rows = query.bind(limit).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(
            |(session_id, session_title, project_id, project_name, updated_at)| MessageSearchHit {
                kind: SearchHitKind::Title,
                session_id,
                session_title,
                project_id,
                project_name,
                updated_at,
                seq: None,
                role: None,
                speaker: None,
                snippet: None,
            },
        )
        .collect())
}

async fn content_hits(
    pool: &SqlitePool,
    q: &str,
    project_id: Option<&str>,
    limit: i64,
) -> Result<Vec<MessageSearchHit>, sqlx::Error> {
    let use_fts = q.chars().count() >= FTS_MIN_CHARS;
    let project_clause = match project_id {
        Some(_) => " AND s.project_id = ?",
        None => "",
    };

    type Row = (
        String,         // session_id
        i64,            // seq
        String,         // role
        Option<String>, // speaker
        String,         // text
        String,         // session_title
        String,         // project_id
        Option<String>, // project_name
        String,         // updated_at
    );
    let rows: Vec<Row> = if use_fts {
        let matched = escape_fts5(q);
        let sql = format!(
            r#"
            SELECT m.session_id, m.seq, m.role, m.speaker, m.text,
                   s.title, s.project_id, p.name, s.updated_at
            FROM messages_fts f
            JOIN messages m ON m.id = f.rowid
            JOIN sessions s ON s.id = m.session_id
            LEFT JOIN projects p ON p.id = s.project_id
            WHERE messages_fts MATCH ?{project_clause}
            ORDER BY bm25(messages_fts)
            LIMIT ?
            "#
        );
        let mut query = sqlx::query_as(&sql).bind(&matched);
        if let Some(pid) = project_id {
            query = query.bind(pid);
        }
        query.bind(limit).fetch_all(pool).await?
    } else {
        let pattern = format!("%{}%", escape_like(q));
        let sql = format!(
            r#"
            SELECT m.session_id, m.seq, m.role, m.speaker, m.text,
                   s.title, s.project_id, p.name, s.updated_at
            FROM messages m
            JOIN sessions s ON s.id = m.session_id
            LEFT JOIN projects p ON p.id = s.project_id
            WHERE m.text LIKE ? ESCAPE '\'{project_clause}
            ORDER BY m.id DESC
            LIMIT ?
            "#
        );
        let mut query = sqlx::query_as(&sql).bind(&pattern);
        if let Some(pid) = project_id {
            query = query.bind(pid);
        }
        query.bind(limit).fetch_all(pool).await?
    };

    Ok(rows
        .into_iter()
        .map(
            |(
                session_id,
                seq,
                role,
                speaker,
                text,
                session_title,
                project_id,
                project_name,
                updated_at,
            )| {
                MessageSearchHit {
                    kind: SearchHitKind::Content,
                    session_id,
                    session_title,
                    project_id,
                    project_name,
                    updated_at,
                    seq: Some(seq),
                    role: Some(role),
                    speaker,
                    snippet: Some(cut_snippet(&text, q)),
                }
            },
        )
        .collect())
}

/// Escape `%`, `_`, `\` for a `LIKE … ESCAPE '\'` pattern.
fn escape_like(q: &str) -> String {
    let mut out = String::with_capacity(q.len());
    for c in q.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Cut a ~±100-char window around the first case-insensitive
/// occurrence of `q` (or the string head when it isn't found — the
/// FTS path may match with different casing). Char-boundary safe by
/// construction (`char_indices`).
fn cut_snippet(text: &str, q: &str) -> String {
    const BEFORE: usize = 48;
    const AFTER: usize = 96;
    let haystack = text.to_lowercase();
    let needle = q.to_lowercase();
    let start = match haystack.find(&needle) {
        Some(byte_idx) => text
            .char_indices()
            .map(|(i, _)| i)
            .find(|&i| i >= byte_idx)
            .unwrap_or(0),
        None => 0,
    };
    let window_start = text
        .char_indices()
        .map(|(i, _)| i)
        .find(|&i| i >= start.saturating_sub(BEFORE))
        .unwrap_or(0);
    let end = text
        .char_indices()
        .map(|(i, _)| i)
        .find(|&i| i >= start + needle.len() + AFTER)
        .unwrap_or(text.len());
    let mut snippet: String = text[window_start..end].to_string();
    if window_start > 0 {
        snippet.insert(0, '…');
    }
    if end < text.len() {
        snippet.push('…');
    }
    snippet
}
