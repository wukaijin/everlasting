//! P1 (autonomous memory, 2026-06-29): storage layer for the
//! agent's self-produced, cross-session recalled experience memory.
//!
//! See `.trellis/tasks/06-29-am-p1-storage/prd.md` for the full
//! spec + spike-007 §5 for the design lineage. This module is the
//! data-layer foundation P2 (read/write closed loop) / P3 (pre-tool
//! pitfall recall) / P4 (event-driven reflection write) / P5
//! (status machine + hygiene job) build on.
//!
//! # Dead-code policy
//!
//! `#![allow(dead_code)]` is set at the module level. This is a
//! **storage底座 (foundation) task** — P1 lands the table + CRUD +
//! write safety net + unit tests, but **no production caller wires
//! any function yet**. P2 (remember tool + recall injection) is the
//! first production consumer; P3 / P4 / P5 follow. Every `pub` item
//! in this module (3 enums + 2 structs + 7 CRUD fns + 2 error enums
//! + the safety-net helpers) is forward-compat storage with zero
//! current callers — ~25 dead-code warnings would fire otherwise.
//!
//! **Deviation from the `subagent_runs.rs` precedent** (which uses
//! per-item `#[allow(dead_code)]`): that module's PR2 landed the
//! table + CRUD **alongside** production callers in B6 PR1's
//! `dispatch.rs`, so only ~8 items (UI-read shapes, a `get_run`
//! helper, an unused flag) lacked callers and got per-item allows.
//! `memories.rs` is a pure-foundation task — **every** public
//! symbol is unused until P2, so per-item allows would be noisy
//! (~25 annotations) without adding precision over the module form.
//! The trade-off accepted here: the module-level allow could mask a
//! future drift (typo'd helper, refactor-orphaned fn), but P2 is
//! the immediate next task and will surface any orphan when it
//! becomes the first caller. When P2 lands, **replace this
//! module-level allow with per-item allows** on whatever P2 still
//! doesn't consume (mirroring `subagent_runs.rs`).

#![allow(dead_code)]

//!
//! # Module shape
//!
//! Mirrors `db::subagent_runs` (same era, same patterns):
//! - Rust enums (`MemoryKind` / `MemoryScope` / `MemoryStatus`)
//!   with `as_str` + lenient `from_str_opt` lockstep with the
//!   DB-side CHECK constraint (PRD B1/2.2 — DB CHECK + Rust enum
//!   double-guard).
//! - `MemoryRow` — `sqlx::FromRow` read shape, camelCased on the
//!   wire (matches every other `db::*Row` crossing the IPC boundary).
//! - `MemoryInput` — write shape (insert parameter bundle).
//! - CRUD functions: `insert_memory` / `list_memories` /
//!   `delete_memory` / `search_memories_fts` /
//!   `find_pitfalls_by_trigger` / `bump_hit_count` / `update_status`.
//!
//! # Audit / forward-compat
//!
//! `confidence` / `hit_count` / `last_used_at` / `demoted_reason`
//! are P5 status-machine fields. P1 stores them + provides the
//! `bump_hit_count` / `update_status` interfaces; no production
//! reader consumes them yet (P5 will).

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Enums — lockstep with the DB-side CHECK constraint
// ---------------------------------------------------------------------------

/// Memory layer / visibility. Matches the `scope` column's CHECK
/// `IN ('user','project')`. `global` is a forward-compat variant
/// deferred to v2 (per spike-007 §8 out-of-scope); it's NOT in the
/// CHECK constraint, so inserting `MemoryScope::Global` would
/// fail at the DB level today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryScope {
    User,
    Project,
}

impl MemoryScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
        }
    }

    /// Lenient parse from a DB string. Unknown values fall back to
    /// `User` — a future binary may add `global` and an older
    /// binary reading a newer DB should default to the broadest
    /// visible scope rather than crash.
    #[allow(dead_code)] // exposed for future UI reads
    pub fn from_str_opt(s: &str) -> Self {
        match s {
            "project" => Self::Project,
            _ => Self::User,
        }
    }
}

/// Memory content category. Matches the `kind` column's CHECK
/// `IN ('pitfall','preference','fact','decision')`.
///
/// - `Pitfall`: a known trip-up (e.g. "WSL cargo test fails on
///   gdk-pixbuf") — written by both the `remember` tool AND the
///   P4 event-driven reflection (consecutive-tool-failure path).
///   Carries a structured trigger key (`tool_name` +
///   `command_pattern` + `path_globs`).
/// - `Preference`: a user-stated or agent-inferred taste ("the
///   user prefers absolute paths").
/// - `Fact`: a piece of project / environment knowledge ("the
///   DB lives at app_data_dir").
/// - `Decision`: an architectural / design choice ("self-built
///   SSE parser, no eventsource-stream crate").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryKind {
    Pitfall,
    Preference,
    Fact,
    Decision,
}

impl MemoryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pitfall => "pitfall",
            Self::Preference => "preference",
            Self::Fact => "fact",
            Self::Decision => "decision",
        }
    }

    /// Lenient parse — unknown strings fall back to `Fact` (the
    /// most neutral category; a forward-compat `kind` added in a
    /// future binary shouldn't crash an older binary reading a
    /// newer DB).
    #[allow(dead_code)]
    pub fn from_str_opt(s: &str) -> Self {
        match s {
            "pitfall" => Self::Pitfall,
            "preference" => Self::Preference,
            "decision" => Self::Decision,
            _ => Self::Fact,
        }
    }
}

/// Memory lifecycle status — the quality funnel (spike-007 §3).
/// Matches the `status` column's CHECK
/// `IN ('candidate','active','verified','demoted')`.
///
/// Transitions (state machine; P1 provides the interface, P5 wires
/// the auto-promotion rules):
/// ```text
///   candidate ──(hit / user review)──► active ──(multi-hit)──► verified
///                                                                    │
///                                                          (aging)   │
///                                                              ▼      ▼
///                                                           demoted ◄──
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryStatus {
    Candidate,
    Active,
    Verified,
    Demoted,
}

impl MemoryStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Active => "active",
            Self::Verified => "verified",
            Self::Demoted => "demoted",
        }
    }

    /// Lenient parse — unknown strings fall back to `Candidate`
    /// (the safest "untrusted" status; a forward-compat `status`
    /// added in a future binary shouldn't crash an older binary).
    #[allow(dead_code)]
    pub fn from_str_opt(s: &str) -> Self {
        match s {
            "active" => Self::Active,
            "verified" => Self::Verified,
            "demoted" => Self::Demoted,
            _ => Self::Candidate,
        }
    }
}

// ---------------------------------------------------------------------------
// MemoryRow — read shape (SELECT * FROM autonomous_memories)
// ---------------------------------------------------------------------------

/// Row shape for SELECTs against `autonomous_memories`. Camel-cased
/// on the wire (matches every other `db::*Row` crossing the IPC
/// boundary). `pitfall` trigger-key columns (`tool_name` /
/// `command_pattern` / `path_globs`) are `Option` — non-pitfall
/// kinds leave them NULL.
///
/// `tags` / `path_globs` are stored as JSON TEXT in the DB; the
/// wire exposes them as the raw JSON string (P2's frontend parses
/// them). The CRUD layer round-trips them verbatim — no schema
/// validation beyond "valid JSON" (P1 scope).
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRow {
    pub id: i64,
    pub memory_id: String,
    pub scope: String,
    pub project_id: Option<String>,
    pub kind: String,
    pub status: String,
    pub title: String,
    pub content: String,
    pub tags: String,
    pub tool_name: Option<String>,
    pub command_pattern: Option<String>,
    pub path_globs: Option<String>,
    pub source_session_id: Option<String>,
    pub source_ref: Option<String>,
    pub confidence: f64,
    pub hit_count: i64,
    pub last_used_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub demoted_reason: Option<String>,
}

// ---------------------------------------------------------------------------
// MemoryInput — write shape (insert parameter bundle)
// ---------------------------------------------------------------------------

/// Insert parameter bundle for [`insert_memory`]. Carries every
/// caller-supplied field; the function fills `memory_id` (UUID v7),
/// `created_at` / `updated_at` (RFC 3339), and the P5 forward-compat
/// defaults (`confidence=0.5`, `hit_count=0`, `last_used_at=NULL`,
/// `demoted_reason=NULL`).
///
/// `tags` and `path_globs` are JSON-encoded `Vec<String>` strings;
/// pass `"[]"` / `None` for empty. The caller is responsible for
/// JSON validity (the DB column is plain TEXT — no schema check).
///
/// `scope=Project` requires `project_id = Some(_)` — enforced by
/// [`insert_memory`] (H2 scope/project_id interaction).
#[derive(Debug, Clone)]
pub struct MemoryInput {
    pub scope: MemoryScope,
    pub project_id: Option<String>,
    pub kind: MemoryKind,
    pub status: MemoryStatus,
    pub title: String,
    pub content: String,
    pub tags: String,
    pub tool_name: Option<String>,
    pub command_pattern: Option<String>,
    pub path_globs: Option<String>,
    pub source_session_id: Option<String>,
    pub source_ref: Option<String>,
}

// ---------------------------------------------------------------------------
// Write safety net — applied before INSERT in `insert_memory`
// ---------------------------------------------------------------------------

/// Sensitive-content regex. Match → reject the insert + warn.
/// Absorbed from spike-005 §4. Anchored case-insensitive;
/// `token=` is the query-param form (catches `Authorization: Bearer`
/// URL leaks), `bearer` catches the header form.
///
/// `OnceLock` would be marginally faster, but `regex::Regex::new`
/// is cheap (~µs) and only runs once per insert — the simplicity
/// of a `const &str` pattern + per-call compile wins for P1.
const SENSITIVE_PATTERN: &str = r"(?i)(api[_-]?key|secret|password|token=|bearer)";

/// Path-segment deny-list. Any path in `content` / `title` /
/// `command_pattern` / `path_globs` whose components include one
/// of these is rejected outright (the agent tried to memorize a
/// secret location). The deny-list is matched on path-component
/// equality (split on `/`), so `/home/user/.ssh/foo` matches but
/// `/home/user/.sshd-config` does NOT (false-positive avoidance).
const SENSITIVE_PATH_COMPONENTS: &[&str] = &[
    ".ssh",
    ".aws",
    ".gnupg",
    "credentials",
    "id_rsa",
];

/// Temporary-path deny-list. These paths are ephemeral (process-
/// scoped, not durable across reboots) so a memory referencing
/// them is almost certainly useless — reject.
const TEMP_PATH_PREFIXES: &[&str] = &["/tmp/", "/var/log/"];

/// Maximum lengths — DB CHECK enforces the same values, but the
/// write safety net rejects early so the error message is
/// actionable (DB CHECK rejection is a generic "CHECK failed").
pub const MAX_TITLE_LEN: usize = 200;
pub const MAX_CONTENT_LEN: usize = 500;

/// Write-safety-net rejection error. Each variant carries enough
/// context for `tracing::warn!` and the caller's IPC error string.
#[derive(Debug, thiserror::Error)]
pub enum MemoryInsertError {
    #[error("title is empty")]
    EmptyTitle,
    #[error("content is empty")]
    EmptyContent,
    #[error("title length {0} exceeds {MAX_TITLE_LEN}")]
    TitleTooLong(usize),
    #[error("content length {0} exceeds {MAX_CONTENT_LEN}")]
    ContentTooLong(usize),
    #[error("content matches sensitive pattern (api_key/secret/password/token/bearer)")]
    SensitiveContent,
    #[error("content references sensitive path component: {0}")]
    SensitivePath(String),
    #[error("content references temporary path: {0}")]
    TemporaryPath(String),
    #[error("scope=Project requires project_id; got None")]
    ProjectScopeMissingId,
    #[error("scope=User must not carry project_id; got {0}")]
    UserScopeHasProjectId(String),
    #[error("DB error: {0}")]
    Db(#[from] sqlx::Error),
}

/// Generalize a `/home/<user>/...` absolute path to `~/...` so the
/// stored memory doesn't leak the local username. Only applies to
/// the `content` / `title` fields (the user-visible experience text);
/// `source_session_id` / `source_ref` are opaque ids that don't
/// carry filesystem paths.
///
/// Conservative: matches `/home/<non-empty-segment>/` and replaces
/// the prefix with `~/`. Doesn't touch `/root/` (root's home is
/// already non-identifying for a single-user dev box). Windows
/// `C:\Users\<user>\` is out of scope (WSL-first design).
fn generalize_home_path(text: &str) -> String {
    // Walk the string and replace each `/home/<seg>/` occurrence.
    // Simple scan (no regex) — the input is ≤500 chars so the
    // quadratic worst case is irrelevant.
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if text[i..].starts_with("/home/") {
            // Find the end of the username segment.
            let after_home = i + "/home/".len();
            if let Some(slash) = text[after_home..].find('/') {
                let seg_end = after_home + slash;
                let username = &text[after_home..seg_end];
                if !username.is_empty() && !username.contains('\\') {
                    out.push_str("~/");
                    i = seg_end + 1;
                    continue;
                }
            }
        }
        // Default: copy one char (preserves UTF-8 boundary).
        let ch = text[i..].chars().next().expect("non-empty slice");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Detect a sensitive path component in any path-like field of the
/// insert. Returns the first offending component (for the error
/// message) or `None` if clean.
fn find_sensitive_path(text: &str) -> Option<&'static str> {
    for component in text.split(|c| c == '/' || c == '\\') {
        for deny in SENSITIVE_PATH_COMPONENTS {
            if component == *deny {
                return Some(deny);
            }
        }
    }
    None
}

/// Detect a temporary-path reference. Returns the matched prefix.
fn find_temporary_path(text: &str) -> Option<&'static str> {
    TEMP_PATH_PREFIXES
        .iter()
        .copied()
        .find(|p| text.contains(p))
}

/// Apply the write safety net to the caller-supplied fields.
/// Returns `Ok((generalized_title, generalized_content))` on
/// success, or the first rejection encountered. Path generalization
/// (`/home/<user>/` → `~/`) is applied to `title` + `content` on
/// the success path so the stored memory is username-agnostic.
///
/// `tags` / `path_globs` / `command_pattern` are NOT generalized
/// (they're structured fields the caller controls; path_globs is
/// a glob the recall path matches against, so generalizing it would
/// break the match). They ARE checked for sensitive-path
/// components (`/home/user/.ssh` in path_globs is still rejected).
fn apply_safety_net(
    input: &MemoryInput,
) -> Result<(String, String), MemoryInsertError> {
    // 1. Empty-value rejection (B1/2.2).
    let title_trimmed = input.title.trim();
    if title_trimmed.is_empty() {
        return Err(MemoryInsertError::EmptyTitle);
    }
    let content_trimmed = input.content.trim();
    if content_trimmed.is_empty() {
        return Err(MemoryInsertError::EmptyContent);
    }

    // 2. Length caps (B1) — DB CHECK is the backstop; reject early
    //    so the error message is actionable.
    if input.title.chars().count() > MAX_TITLE_LEN {
        return Err(MemoryInsertError::TitleTooLong(
            input.title.chars().count(),
        ));
    }
    if input.content.chars().count() > MAX_CONTENT_LEN {
        return Err(MemoryInsertError::ContentTooLong(
            input.content.chars().count(),
        ));
    }

    // 3. Sensitive-content regex (spike-005 §4). Anchored on
    //    title + content (the free-form text the LLM produces).
    let sensitive_re =
        regex::Regex::new(SENSITIVE_PATTERN).expect("sensitive pattern compiles");
    if sensitive_re.is_match(&input.title) || sensitive_re.is_match(&input.content) {
        return Err(MemoryInsertError::SensitiveContent);
    }

    // 4. Sensitive-path deny-list (2.3). Check every path-like
    //    field; reject on the first hit.
    for field in [
        &input.title,
        &input.content,
        input.command_pattern.as_deref().unwrap_or(""),
        input.path_globs.as_deref().unwrap_or(""),
    ] {
        if let Some(deny) = find_sensitive_path(field) {
            return Err(MemoryInsertError::SensitivePath(deny.to_string()));
        }
    }

    // 5. Temporary-path deny-list.
    for field in [
        &input.title,
        &input.content,
        input.command_pattern.as_deref().unwrap_or(""),
        input.path_globs.as_deref().unwrap_or(""),
    ] {
        if let Some(prefix) = find_temporary_path(field) {
            return Err(MemoryInsertError::TemporaryPath(prefix.to_string()));
        }
    }

    // 6. Path generalization (`/home/<user>/` → `~/`). Applied
    //    AFTER the deny-list checks (a path under `/home/<user>/.ssh`
    //    is rejected by step 4 before reaching here).
    let title = generalize_home_path(&input.title);
    let content = generalize_home_path(&input.content);

    Ok((title, content))
}

/// Escape a user-supplied query string for safe FTS5 MATCH.
///
/// Wraps the query in double quotes (FTS5 phrase-match syntax) and
/// doubles any embedded double quotes per the FTS5 string-literal
/// rule. This neutralizes FTS5 operators (`AND` / `OR` / `NOT` /
/// `NEAR` / `*` / `^`) — a query like `cargo AND test` is treated
/// as a single phrase, not a boolean expression.
///
/// **Tradeoff (H3)**: phrase match requires the tokens to appear
/// contiguously AND in the given order. `"WSL cargo"` won't match
/// content reading "cargo ... WSL" (different order). v1 accepts
/// this (precision-first); v2 can switch to per-token escaping +
/// OR-join for recall-first semantics. See prd §4 H3 tradeoff
/// note.
fn escape_fts5(q: &str) -> String {
    format!("\"{}\"", q.replace('"', "\"\""))
}

// ---------------------------------------------------------------------------
// CRUD: insert / list / delete
// ---------------------------------------------------------------------------

/// Insert a new memory row. Applies the write safety net (§4) before
/// the INSERT: empty/over-length/sensitive-content/sensitive-path/
/// temporary-path are rejected with a typed error; `/home/<user>/`
/// is generalized to `~/`. The FTS5 sync trigger (migration PR1b)
/// keeps the FTS index in sync automatically — no manual FTS write.
///
/// `memory_id` is generated as UUID v7 (time-ordered, B-tree
/// friendly, RFC 9562). A UNIQUE collision returns `Err` (UUIDv7
/// collision probability is astronomically low; we do NOT upsert).
///
/// **scope/project_id interaction (H2)**:
/// - `scope=User` → `project_id` MUST be `None` (rejected otherwise;
///   a user-scope memory is global to the user, not project-bound).
/// - `scope=Project` → `project_id` MUST be `Some(_)` (rejected
///   otherwise; a project memory without a project is meaningless).
pub async fn insert_memory(
    pool: &SqlitePool,
    input: &MemoryInput,
) -> Result<MemoryRow, MemoryInsertError> {
    // scope/project_id interaction (H2).
    match (input.scope, &input.project_id) {
        (MemoryScope::User, Some(id)) => {
            return Err(MemoryInsertError::UserScopeHasProjectId(id.clone()));
        }
        (MemoryScope::Project, None) => {
            return Err(MemoryInsertError::ProjectScopeMissingId);
        }
        _ => {}
    }

    // Write safety net (§4).
    let (title, content) = apply_safety_net(input)?;

    let memory_id = Uuid::now_v7().to_string();
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        INSERT INTO autonomous_memories
        (memory_id, scope, project_id, kind, status, title, content, tags,
         tool_name, command_pattern, path_globs, source_session_id, source_ref,
         confidence, hit_count, last_used_at, created_at, updated_at, demoted_reason)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0.5, 0, NULL, ?, ?, NULL)
        "#,
    )
    .bind(&memory_id)
    .bind(input.scope.as_str())
    .bind(&input.project_id)
    .bind(input.kind.as_str())
    .bind(input.status.as_str())
    .bind(&title)
    .bind(&content)
    .bind(&input.tags)
    .bind(&input.tool_name)
    .bind(&input.command_pattern)
    .bind(&input.path_globs)
    .bind(&input.source_session_id)
    .bind(&input.source_ref)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;

    // Read back to return the full row (auto-id + timestamps).
    // Defensive: a concurrent `delete_memory` between our INSERT and
    // this readback could make the row vanish (single-writer SQLite
    // makes this near-impossible, but the safety-net contract says
    // production code never `.unwrap()`s / `.expect()`s on a DB
    // result). Map a missing row to `sqlx::Error::RowNotFound`, which
    // `#[from]` lifts into `MemoryInsertError::Db` — the caller gets
    // a typed error instead of a panic. Mirrors the defensive no-op
    // pattern used by `record_message_resend_audit` /
    // `record_tool_duration`.
    let row = get_memory_by_id(pool, &memory_id)
        .await?
        .ok_or_else(|| sqlx::Error::RowNotFound)?;
    Ok(row)
}

/// Fetch a single row by `memory_id` (the UUID, not the auto-id).
/// Used internally by `insert_memory` to read back the full row;
/// exposed for P2's future "fetch single memory" IPC.
#[allow(dead_code)]
pub async fn get_memory_by_id(
    pool: &SqlitePool,
    memory_id: &str,
) -> Result<Option<MemoryRow>, sqlx::Error> {
    let row = sqlx::query_as::<_, MemoryRow>(
        r#"
        SELECT id, memory_id, scope, project_id, kind, status, title, content,
               tags, tool_name, command_pattern, path_globs, source_session_id,
               source_ref, confidence, hit_count, last_used_at, created_at,
               updated_at, demoted_reason
        FROM autonomous_memories
        WHERE memory_id = ?
        "#,
    )
    .bind(memory_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// List memories, optionally filtered by scope and/or project_id.
/// Used by P2's frontend memory browser (the MemoryPreview list).
///
/// **scope/project_id interaction (H2)** — same semantics as
/// `search_memories_fts`:
/// - `(Some(User), _)` → only user-scope rows (project_id ignored).
/// - `(Some(Project), None)` → Err (project query needs an id).
/// - `(Some(Project), Some(id))` → only that project's rows.
/// - `(None, _)` → all rows (both scopes); project_id is ignored.
///
/// Ordered by `created_at DESC` (newest first) — matches the UI
/// convention for list endpoints.
pub async fn list_memories(
    pool: &SqlitePool,
    scope: Option<MemoryScope>,
    project_id: Option<&str>,
) -> Result<Vec<MemoryRow>, MemoryInsertError> {
    // Validate scope/project_id interaction up-front (mirrors
    // search_memories_fts). User scope ignores project_id; Project
    // scope requires project_id.
    if let Some(MemoryScope::Project) = scope {
        if project_id.is_none() {
            return Err(MemoryInsertError::ProjectScopeMissingId);
        }
    }

    let rows = match scope {
        Some(MemoryScope::User) => {
            sqlx::query_as::<_, MemoryRow>(
                r#"
                SELECT id, memory_id, scope, project_id, kind, status, title, content,
                       tags, tool_name, command_pattern, path_globs, source_session_id,
                       source_ref, confidence, hit_count, last_used_at, created_at,
                       updated_at, demoted_reason
                FROM autonomous_memories
                WHERE scope = 'user'
                ORDER BY created_at DESC
                "#,
            )
            .fetch_all(pool)
            .await?
        }
        Some(MemoryScope::Project) => {
            sqlx::query_as::<_, MemoryRow>(
                r#"
                SELECT id, memory_id, scope, project_id, kind, status, title, content,
                       tags, tool_name, command_pattern, path_globs, source_session_id,
                       source_ref, confidence, hit_count, last_used_at, created_at,
                       updated_at, demoted_reason
                FROM autonomous_memories
                WHERE scope = 'project' AND project_id = ?
                ORDER BY created_at DESC
                "#,
            )
            .bind(project_id)
            .fetch_all(pool)
            .await?
        }
        None => {
            sqlx::query_as::<_, MemoryRow>(
                r#"
                SELECT id, memory_id, scope, project_id, kind, status, title, content,
                       tags, tool_name, command_pattern, path_globs, source_session_id,
                       source_ref, confidence, hit_count, last_used_at, created_at,
                       updated_at, demoted_reason
                FROM autonomous_memories
                ORDER BY created_at DESC
                "#,
            )
            .fetch_all(pool)
            .await?
        }
    };
    Ok(rows)
}

/// Delete a memory by `memory_id`. The FTS5 sync trigger
/// (`am_fts_delete`) removes the row's FTS index entries
/// automatically. Returns the number of rows deleted (0 if the
/// memory_id didn't exist — caller decides whether that's an error).
pub async fn delete_memory(
    pool: &SqlitePool,
    memory_id: &str,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("DELETE FROM autonomous_memories WHERE memory_id = ?")
        .bind(memory_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

// ---------------------------------------------------------------------------
// search_memories_fts — FTS5 bm25 search with scope semantics
// ---------------------------------------------------------------------------

/// Search memories via FTS5 `MATCH` + `bm25` ranking. The query is
/// escaped via [`escape_fts5`] (phrase match; H3 tradeoff accepted
/// for v1).
///
/// **scope/project_id interaction (H2)**:
/// - `scope = Some(User)` → `WHERE scope='user'` (project_id
///   ignored — a user-scope memory is global to the user).
/// - `scope = Some(Project)` + `project_id = None` → **Err**
///   (a project query without a project id is meaningless).
/// - `scope = Some(Project)` + `project_id = Some(id)` →
///   `WHERE scope='project' AND project_id=?`.
/// - `scope = None` → search both layers:
///   `WHERE scope='user' OR (scope='project' AND project_id=?)`.
///   In this case `project_id` MUST be `Some` (the project branch
///   of the OR needs it) — returns Err otherwise.
///
/// Only `status IN ('active','verified')` rows are returned
/// (candidate / demoted rows aren't surfaced to recall — they
/// haven't earned promotion or have been demoted).
///
/// `limit` caps the result count (P2's session-start recall uses
/// a small top-k; the caller decides).
pub async fn search_memories_fts(
    pool: &SqlitePool,
    project_id: Option<&str>,
    scope: Option<MemoryScope>,
    query: &str,
    limit: i64,
) -> Result<Vec<MemoryRow>, MemoryInsertError> {
    // Empty / whitespace query → empty result (FTS5 MATCH on an
    // empty phrase is a syntax error; short-circuit instead).
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }

    let escaped = escape_fts5(query);

    // Build the scope filter per H2. Three branches:
    // (a) User scope — ignore project_id.
    // (b) Project scope — require project_id.
    // (c) None — search both; project_id required for the project
    //     branch of the OR.
    let (sql, bind_project_id) = match scope {
        Some(MemoryScope::User) => (
            // (a)
            r#"
            SELECT m.id, m.memory_id, m.scope, m.project_id, m.kind, m.status,
                   m.title, m.content, m.tags, m.tool_name, m.command_pattern,
                   m.path_globs, m.source_session_id, m.source_ref, m.confidence,
                   m.hit_count, m.last_used_at, m.created_at, m.updated_at,
                   m.demoted_reason
            FROM autonomous_memories_fts f
            JOIN autonomous_memories m ON m.id = f.rowid
            WHERE autonomous_memories_fts MATCH ?
              AND m.scope = 'user'
              AND m.status IN ('active','verified')
            ORDER BY bm25(autonomous_memories_fts)
            LIMIT ?
            "#,
            false,
        ),
        Some(MemoryScope::Project) => {
            if project_id.is_none() {
                return Err(MemoryInsertError::ProjectScopeMissingId);
            }
            (
                // (b)
                r#"
                SELECT m.id, m.memory_id, m.scope, m.project_id, m.kind, m.status,
                       m.title, m.content, m.tags, m.tool_name, m.command_pattern,
                       m.path_globs, m.source_session_id, m.source_ref, m.confidence,
                       m.hit_count, m.last_used_at, m.created_at, m.updated_at,
                       m.demoted_reason
                FROM autonomous_memories_fts f
                JOIN autonomous_memories m ON m.id = f.rowid
                WHERE autonomous_memories_fts MATCH ?
                  AND m.scope = 'project'
                  AND m.project_id = ?
                  AND m.status IN ('active','verified')
                ORDER BY bm25(autonomous_memories_fts)
                LIMIT ?
                "#,
                true,
            )
        }
        None => {
            // (c) — search both layers; project_id required.
            if project_id.is_none() {
                return Err(MemoryInsertError::ProjectScopeMissingId);
            }
            (
                r#"
                SELECT m.id, m.memory_id, m.scope, m.project_id, m.kind, m.status,
                       m.title, m.content, m.tags, m.tool_name, m.command_pattern,
                       m.path_globs, m.source_session_id, m.source_ref, m.confidence,
                       m.hit_count, m.last_used_at, m.created_at, m.updated_at,
                       m.demoted_reason
                FROM autonomous_memories_fts f
                JOIN autonomous_memories m ON m.id = f.rowid
                WHERE autonomous_memories_fts MATCH ?
                  AND (m.scope = 'user'
                       OR (m.scope = 'project' AND m.project_id = ?))
                  AND m.status IN ('active','verified')
                ORDER BY bm25(autonomous_memories_fts)
                LIMIT ?
                "#,
                true,
            )
        }
    };

    let mut q = sqlx::query_as::<_, MemoryRow>(sql).bind(&escaped);
    if bind_project_id {
        q = q.bind(project_id);
    }
    q = q.bind(limit);
    let rows = q.fetch_all(pool).await?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// find_pitfalls_by_trigger — pre-tool recall (P3 consumer)
// ---------------------------------------------------------------------------

/// Find pitfall memories matching the current tool invocation. Used
/// by P3's pre-tool recall hook (the `permissions/check.rs` Tier 1
/// Hooks site). The probe is `tool_name` exact-match (indexed by
/// `idx_am_pitfall`); `command_pattern` is an optional secondary
/// substring filter.
///
/// **path_globs semantics (M2)**:
/// - If a pitfall's `path_globs` is `NULL` → the pitfall is
///   path-agnostic (fires for ANY path; e.g. "always pass
///   `--offline` to cargo").
/// - If `path_globs` is `Some(globs)` AND `path` is `Some(p)` →
///   the pitfall fires only if `p` matches at least one glob in
///   the JSON array.
/// - If `path_globs` is `Some(globs)` AND `path` is `None` → the
///   pitfall does NOT fire (the caller didn't supply a path, so
///   we can't confirm the glob match; precision-first).
///
/// Only `status IN ('active','verified')` rows are returned (a
/// `candidate` pitfall hasn't earned recall surface yet).
pub async fn find_pitfalls_by_trigger(
    pool: &SqlitePool,
    tool_name: &str,
    command_pattern: Option<&str>,
    path: Option<&str>,
) -> Result<Vec<MemoryRow>, sqlx::Error> {
    // First: the indexed tool_name equality probe (idx_am_pitfall).
    let candidates: Vec<MemoryRow> = sqlx::query_as::<_, MemoryRow>(
        r#"
        SELECT id, memory_id, scope, project_id, kind, status, title, content,
               tags, tool_name, command_pattern, path_globs, source_session_id,
               source_ref, confidence, hit_count, last_used_at, created_at,
               updated_at, demoted_reason
        FROM autonomous_memories
        WHERE tool_name = ?
          AND kind = 'pitfall'
          AND status IN ('active','verified')
        "#,
    )
    .bind(tool_name)
    .fetch_all(pool)
    .await?;

    // Second: in-memory filtering for command_pattern + path_globs.
    // The candidate set is small (one tool_name's worth — typically
    // single digits), so the post-filter is cheaper than a complex
    // SQL expression and avoids SQLite glob's lack of JSON-array
    // iteration support.
    let mut out = Vec::with_capacity(candidates.len());
    for mem in candidates {
        // command_pattern substring match (P3 will refine the rule).
        if let Some(cp) = command_pattern {
            if let Some(mem_cp) = &mem.command_pattern {
                if !cp.contains(mem_cp.as_str()) {
                    continue;
                }
            }
        }
        // path_globs match (M2).
        if let Some(globs_json) = &mem.path_globs {
            match path {
                Some(p) => {
                    // Parse the JSON array; if it fails or is empty,
                    // treat as "no match" (precision-first).
                    let globs: Vec<String> =
                        serde_json::from_str(globs_json).unwrap_or_default();
                    let matched = globs
                        .iter()
                        .any(|g| glob_matches_path(g, p));
                    if !matched {
                        continue;
                    }
                }
                None => {
                    // path_globs is set but caller supplied no path —
                    // can't confirm; skip (precision-first).
                    continue;
                }
            }
        }
        // NULL path_globs → path-agnostic → always fires (no filter).
        out.push(mem);
    }
    Ok(out)
}

/// Simple glob matcher for `path_globs`. Supports `*` (any sequence
/// not crossing `/`) and `?` (one char). The glob set is supplied by
/// the writer (P2 remember tool / P4 reflection); this function is
/// the read-side matcher.
///
/// **Dialect note**: this is the `session_tool_permissions`-style
/// glob, NOT native SQLite GLOB. Verified empirically against SQLite
/// 3.53.0 at check time: native `'a/b' GLOB 'a*'` returns 1 (SQLite
/// GLOB's `*` DOES cross `/`). This matcher instead treats `*` as
/// segment-scoped (matches `app/src-tauri/Cargo.toml` but NOT
/// `app/src-tauri/src/lib.rs`), matching the
/// `session_tool_permissions.path` glob contract that
/// spike-007's re-grill explicitly standardized on (no `**`
/// recursion). The doc comment previously claimed "SQLite GLOB
/// semantics" — that was inaccurate and is corrected here.
///
/// **Char-level vs byte-level caveat**: `?` matches a single **byte**
/// here, not a single char. SQLite GLOB uses `sqlite3Utf8Read` and
/// is char-level (a CJK char is one match unit). For ASCII paths
/// (the dominant case) the two are equivalent; a CJK glob with `?`
/// (e.g. `中?` to match `中文`) would NOT match here. `*` is
/// unaffected (matching UTF-8 bytes within a segment == matching
/// chars within a segment). Accepted as low-priority for P1 (CJK
/// path globs with `?` are vanishingly rare); revisit if P3/P4
/// surface the case.
fn glob_matches_path(glob: &str, path: &str) -> bool {
    // Convert the glob to a regex-free byte-by-byte match. `*` →
    // any non-`/` run; `?` → any single byte (see char-level caveat
    // in the doc comment above).
    let glob_b: &[u8] = glob.as_bytes();
    let path_b: &[u8] = path.as_bytes();
    glob_match_inner(glob_b, path_b)
}

/// Recursive glob matcher (`session_tool_permissions`-style glob, NOT
/// native SQLite GLOB — see [`glob_matches_path`] doc). `*` matches
/// zero or more chars that are NOT `/`; `?` matches any single byte
/// (including `/`). All other chars are literal.
fn glob_match_inner(glob: &[u8], path: &[u8]) -> bool {
    let (mut gi, mut pi) = (0, 0);
    let mut star_gi: Option<usize> = None;
    let mut star_pi = 0;
    while pi < path.len() {
        if gi < glob.len() {
            match glob[gi] {
                b'?' => {
                    gi += 1;
                    pi += 1;
                    continue;
                }
                b'*' => {
                    // `*` doesn't cross `/` — remember position, try
                    // to consume zero chars first; if the next path
                    // char is `/`, the star stops matching.
                    if path[pi] == b'/' {
                        // Star can't cross `/`; advance past the star.
                        gi += 1;
                        continue;
                    }
                    star_gi = Some(gi);
                    star_pi = pi;
                    gi += 1;
                    continue;
                }
                c if c == path[pi] => {
                    gi += 1;
                    pi += 1;
                    continue;
                }
                _ => {}
            }
        }
        // Mismatch — backtrack to the last `*` and consume one more
        // char from the path (if possible).
        if let Some(sg) = star_gi {
            gi = sg + 1;
            star_pi += 1;
            if path[star_pi - 1] == b'/' {
                // Star can't cross `/`; no more backtracking.
                return false;
            }
            pi = star_pi;
        } else {
            return false;
        }
    }
    // Consume trailing `*`s in the glob.
    while gi < glob.len() && glob[gi] == b'*' {
        gi += 1;
    }
    gi == glob.len()
}

// ---------------------------------------------------------------------------
// bump_hit_count / update_status — P5 status-machine interfaces
// ---------------------------------------------------------------------------

/// Increment `hit_count` and stamp `last_used_at` for a memory.
/// Called by the recall paths (`search_memories_fts` / P3's
/// `find_pitfalls_by_trigger` consumer) when a memory is surfaced
/// — P5's status machine reads `hit_count` to decide promotion
/// (candidate → active → verified).
///
/// Best-effort: a `warn!` on failure (matches the project's
/// "audit/metadata writes are best-effort" pattern). The recall
/// return value is unaffected by a hit-count bump failure.
pub async fn bump_hit_count(
    pool: &SqlitePool,
    memory_id: &str,
) -> Result<(), sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        UPDATE autonomous_memories
        SET hit_count = hit_count + 1,
            last_used_at = ?,
            updated_at = ?
        WHERE memory_id = ?
        "#,
    )
    .bind(&now)
    .bind(&now)
    .bind(memory_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Transition a memory to a new status, wrapped in a transaction.
/// Reads the current status inside the transaction, validates the
/// transition is legal per the state machine (spike-007 §3), then
/// writes the new status + optional `demoted_reason` (set when
/// transitioning TO `demoted`; cleared otherwise).
///
/// Legal transitions:
/// ```text
///   candidate → active | verified | demoted
///   active    → verified | demoted
///   verified  → demoted
///   demoted   → active   (re-promotion via P5 hygiene job)
/// ```
/// All other transitions return `Err(StatusTransitionIllegal)`.
///
/// P1 provides the interface; P5 wires the auto-promotion rules.
/// The transaction ensures a concurrent `bump_hit_count` can't
/// race the status read (SQLite serializes writers under the
/// default rollback-journal mode).
#[derive(Debug, thiserror::Error)]
pub enum StatusTransitionError {
    #[error("memory {0} not found")]
    NotFound(String),
    #[error("illegal transition: {from} -> {to}")]
    Illegal {
        from: &'static str,
        to: &'static str,
    },
    #[error("DB error: {0}")]
    Db(#[from] sqlx::Error),
}

pub async fn update_status(
    pool: &SqlitePool,
    memory_id: &str,
    new_status: MemoryStatus,
    demoted_reason: Option<&str>,
) -> Result<(), StatusTransitionError> {
    let mut tx = pool.begin().await?;

    // Read current status inside the transaction.
    let current_str: Option<String> = sqlx::query_scalar(
        "SELECT status FROM autonomous_memories WHERE memory_id = ?",
    )
    .bind(memory_id)
    .fetch_optional(&mut *tx)
    .await?;
    let current_str = current_str.ok_or_else(|| StatusTransitionError::NotFound(memory_id.to_string()))?;
    let current = MemoryStatus::from_str_opt(&current_str);

    // Validate the transition.
    let legal = match (current, new_status) {
        // Identity is always legal (idempotent re-promotion).
        (a, b) if a == b => true,
        (MemoryStatus::Candidate, MemoryStatus::Active) => true,
        (MemoryStatus::Candidate, MemoryStatus::Verified) => true,
        (MemoryStatus::Candidate, MemoryStatus::Demoted) => true,
        (MemoryStatus::Active, MemoryStatus::Verified) => true,
        (MemoryStatus::Active, MemoryStatus::Demoted) => true,
        (MemoryStatus::Verified, MemoryStatus::Demoted) => true,
        (MemoryStatus::Demoted, MemoryStatus::Active) => true,
        _ => false,
    };
    if !legal {
        return Err(StatusTransitionError::Illegal {
            from: current.as_str(),
            to: new_status.as_str(),
        });
    }

    let now = Utc::now().to_rfc3339();
    // demoted_reason: set when transitioning TO demoted (and a
    // reason was supplied); clear when transitioning AWAY from
    // demoted (re-promotion). For non-demoted transitions where the
    // caller passed a reason, we ignore it (the column is for the
    // demoted state only).
    let reason_to_write: Option<&str> = if new_status == MemoryStatus::Demoted {
        demoted_reason
    } else {
        None
    };

    sqlx::query(
        r#"
        UPDATE autonomous_memories
        SET status = ?,
            demoted_reason = ?,
            updated_at = ?
        WHERE memory_id = ?
        "#,
    )
    .bind(new_status.as_str())
    .bind(reason_to_write)
    .bind(&now)
    .bind(memory_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers (test-only)
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::*;

    /// Direct row builder for tests that need to bypass the write
    /// safety net (e.g. to insert a memory with sensitive content
    /// to verify the FTS trigger or to test the trigger directly).
    /// Production code MUST use [`insert_memory`].
    #[allow(dead_code)]
    pub async fn insert_raw(
        pool: &SqlitePool,
        memory_id: &str,
        scope: MemoryScope,
        project_id: Option<&str>,
        kind: MemoryKind,
        status: MemoryStatus,
        title: &str,
        content: &str,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO autonomous_memories
            (memory_id, scope, project_id, kind, status, title, content, tags,
             created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, '[]', ?, ?)
            "#,
        )
        .bind(memory_id)
        .bind(scope.as_str())
        .bind(project_id)
        .bind(kind.as_str())
        .bind(status.as_str())
        .bind(title)
        .bind(content)
        .bind(&now)
        .bind(&now)
        .execute(pool)
        .await?;
        Ok(())
    }
}

// Allow tests in this file to reach `apply_safety_net` / `escape_fts5`
// / `glob_matches_path` for unit testing. The functions are already
// `pub(crate)`-visible via the module; this is just a reminder.
