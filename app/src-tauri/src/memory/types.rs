//! Type definitions for the B5 memory loader.
//!
//! These types are split into two surfaces:
//!
//! - **Internal / cache layer** ([`MemoryLayer`], [`LayerStatus`]):
//!   the full in-memory representation, owned by `MemoryCache` and
//!   read on every chat request. `content` is the file's bytes
//!   (decoded as UTF-8) plus the token count.
//!
//! - **Wire / preview layer** ([`MemoryLayerInfo`]): the DTO sent
//!   to the frontend over Tauri IPC for the read-only Memory
//!   Preview panel. `content` is NOT included (the file may be
//!   large); the frontend calls `read_memory_content(path)` to
//!   fetch the body on demand.
//!
//! `MemoryKind` is a 4-variant enum even though V2 1 期 only
//! populates `User` and `Project`. The variants are forward-compat
//! slots — V2 2 期 will add Session (a `sessions.session_instructions`
//! table) and Runtime (a `memories` table with FTS5).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Which memory layer a file belongs to.
///
/// Ordered from outermost (lowest priority) to innermost (highest
/// priority) — the agent sees the LEAST specific context first and
/// the MOST specific last, so project instructions can override
/// user defaults without an explicit "override" mechanism.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryKind {
    /// `~/.claude/CLAUDE.md` (Claude Code interop) and
    /// `~/.config/everlasting/AGENTS.md` (Everlasting-native).
    /// The two files live in different directories — locked by
    /// 2026-06-26 user-claude-md-home-dir. Global across all
    /// projects.
    User,
    /// `<project.path>/CLAUDE.md` and `AGENTS.md`. Scoped to one
    /// project.
    Project,
    /// Per-session instructions. V2 2 期: stored in a new
    /// `sessions.session_instructions` SQLite column. Reserved —
    /// never populated in V2 1 期.
    #[allow(dead_code)]
    Session,
    /// Cross-session memories with FTS5 retrieval. V2 2 期: stored
    /// in a new `memories` table. Reserved — never populated in
    /// V2 1 期.
    #[allow(dead_code)]
    Runtime,
}

impl MemoryKind {
    /// Human-readable label for the LLM banner ("[User CLAUDE.md]"
    /// vs. "[Project AGENTS.md]"). Stable, used as a stable
    /// identifier in the prompt.
    pub fn label_prefix(self) -> &'static str {
        match self {
            MemoryKind::User => "User",
            MemoryKind::Project => "Project",
            MemoryKind::Session => "Session",
            MemoryKind::Runtime => "Runtime",
        }
    }
}

/// Distinguishes the two filenames per layer. The distinction is
/// purely conventional — both files are loaded identically — but
/// the LLM and the user benefit from the file's role in the
/// banner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySource {
    /// `CLAUDE.md` (the "main" memory slot — Claude Code
    /// convention).
    Claude,
    /// `AGENTS.md` (the "agent instructions" slot — adopted from
    /// Aider / Codex convention).
    Agents,
}

impl MemorySource {
    /// Bare filename, including the `.md` extension.
    pub fn filename(self) -> &'static str {
        match self {
            MemorySource::Claude => "CLAUDE.md",
            MemorySource::Agents => "AGENTS.md",
        }
    }

    /// Human-readable label for the LLM banner.
    pub fn label(self) -> &'static str {
        match self {
            MemorySource::Claude => "CLAUDE.md",
            MemorySource::Agents => "AGENTS.md",
        }
    }
}

/// Outcome of attempting to read a single memory file.
///
/// `Loaded` and `Missing` are the two user-visible "expected"
/// states. `Error` is reserved for I/O / encoding / size failures
/// that the loader could not silently absorb (though it does its
/// best — see `file::load_file`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "reason")]
pub enum LayerStatus {
    /// File read successfully; `content` and `tokens` are
    /// populated.
    Loaded,
    /// File does not exist at the resolved path. This is the
    /// expected state for a fresh install / a project that has
    /// never set up memory.
    Missing,
    /// File exists but the loader could not read it (permission,
    /// non-UTF-8, > 100 KiB, symlink loop). The `reason` field
    /// carries a short human-readable explanation.
    Error { reason: String },
}

/// One in-memory representation of a single memory file. Lives
/// inside [`crate::memory::MemoryCache`] and is cloned out on
/// every chat request to be formatted into the system prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryLayer {
    /// Which layer (User / Project / Session / Runtime).
    pub kind: MemoryKind,
    /// Which file in the layer (CLAUDE.md / AGENTS.md).
    pub source: MemorySource,
    /// Absolute path on disk. Always populated, even for `Missing`
    /// (so the frontend can show "create at /home/x/.../CLAUDE.md").
    pub path: PathBuf,
    /// File body, decoded as UTF-8. Empty string for
    /// `Missing` / `Error`.
    pub content: String,
    /// Estimated token count (cl100k_base). 0 for `Missing` /
    /// `Error`.
    pub tokens: u32,
    /// Load outcome.
    pub status: LayerStatus,
}

impl MemoryLayer {
    /// Stable label used in the LLM banner and the preview UI:
    /// `"[User CLAUDE.md]"`, `"[Project AGENTS.md]"`, etc.
    pub fn label(&self) -> String {
        format!("[{} {}]", self.kind.label_prefix(), self.source.label())
    }

    /// Render the LLM-facing section. Returns `None` for
    /// `Missing` / `Error` — the agent skips those layers silently
    /// (the banner's count is the only hint the LLM gets).
    pub fn render_prompt_section(&self) -> Option<String> {
        if !matches!(self.status, LayerStatus::Loaded) {
            return None;
        }
        Some(format!("{}\n{}", self.label(), self.content))
    }
}

/// Lightweight DTO for the frontend preview panel (no `content` —
/// fetched on demand via `read_memory_content`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryLayerInfo {
    pub kind: MemoryKind,
    pub source: MemorySource,
    pub path: PathBuf,
    pub tokens: u32,
    pub status: LayerStatus,
    /// File body length in characters. `0` for Missing / Error.
    /// Cheap to compute; gives the preview UI a "size" indicator
    /// without streaming the full content.
    pub char_count: usize,
}

impl From<&MemoryLayer> for MemoryLayerInfo {
    fn from(l: &MemoryLayer) -> Self {
        Self {
            kind: l.kind,
            source: l.source,
            path: l.path.clone(),
            tokens: l.tokens,
            status: l.status.clone(),
            char_count: l.content.chars().count(),
        }
    }
}
