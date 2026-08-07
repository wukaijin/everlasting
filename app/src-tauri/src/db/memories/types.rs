//! Shared types for the autonomous-memory storage layer.
//!
//! Relocated verbatim from the pre-split `memories.rs`. Enums + row
//! shapes + error enums + length constants live here so the crud /
//! validation / search / lifecycle submodules share one source.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

// Enums: MemoryScope / MemoryKind / MemoryStatus + MemoryRow + MemoryInput
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
///
/// `edited_by_user` (07-06, am-observability-panel D1) is a
/// provenance marker the management modal renders as a chip —
/// `true` means the user has manually edited title/content via
/// `update_memory` (R4), distinguishing from agent-written
/// memories (P2 `remember` tool / P4 `reflect_to_pitfall`).
/// Default `false` for all existing rows (the migration is
/// `BOOLEAN NOT NULL DEFAULT 0`).
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
    #[serde(default)]
    pub edited_by_user: bool,
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

// Length constants (referenced by MemoryInsertError / MemoryUpdateError
// #[error] strings + the validation helpers).
/// Maximum lengths — DB CHECK enforces the same values, but the
/// write safety net rejects early so the error message is
/// actionable (DB CHECK rejection is a generic "CHECK failed").
pub const MAX_TITLE_LEN: usize = 200;
pub const MAX_CONTENT_LEN: usize = 500;

// Error enums shared by crud (insert) + validation + lifecycle (update).
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

/// 07-06 (am-observability-panel D2): write-safety-net error for
/// [`update_memory`]. Re-uses the same validation as
/// [`MemoryInsertError`] (the safety net is a single source of
/// truth — both writers go through [`validate_memory_text`]) +
/// adds a `NotFound` variant when the target `memory_id` doesn't
/// exist (the frontend should surface a clean "row vanished"
/// error, not a generic DB error).
#[derive(Debug, thiserror::Error)]
pub enum MemoryUpdateError {
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
    #[error("memory {0} not found")]
    NotFound(String),
    #[error("DB error: {0}")]
    Db(#[from] sqlx::Error),
}
