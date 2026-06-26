//! B5 Memory — User / Project two-layer loader (V2 first-tier, 2026-06-10).
//!
//! Loads Markdown memory files (CLAUDE.md / AGENTS.md) from two fixed
//! layers — User (`~/.claude/CLAUDE.md` + `~/.config/everlasting/AGENTS.md`,
//! split locked 2026-06-26 user-claude-md-home-dir) and Project
//! (`<project.path>/`) — and injects their content into the LLM
//! system prompt at the ⑤a context-construction stage (per
//! `docs/ARCHITECTURE.md` §2.2 step ⑤a).
//!
//! **V2 1 期 (this implementation)**: 2 layers (User + Project).
//! Session / Runtime memory are explicitly out of scope and reserved
//! for V2 2 期 (per `b5-memory-user-project-2layer/prd.md` "Out of
//! Scope" section). The `MemoryKind` enum carries `Session` and
//! `Runtime` variants as forward-compat placeholders only — they are
//! never populated in the loader output today.
//!
//! **Failure tolerance**: every file is loaded in isolation. A
//! missing file, a permission error, a non-UTF-8 read, a 100KB+ file,
//! or a symlink loop all yield `LayerStatus::Missing` / `Error` with
//! a `tracing::warn!` log — and the other layers are unaffected.
//! Memory is a "premium" context feature; one missing file must
//! never break a chat session.
//!
//! **Freshness (RULE-C-001 fence, 2026-06-15)**: there is NO
//! background watcher. Every `load_for_session` stats each of the
//! 4 files' `mtime` and reloads the slot when it changed since the
//! last load — the read path is the authority on freshness, so a
//! file saved between reads is always reflected on the next read
//! (no debounce window, no dropped-watcher hazard). The cache lives
//! in `AppState::memory_cache`; reads are non-blocking through
//! `tokio::sync::RwLock`.
//!
//! Files in this module:
//! - [`types`] — `MemoryKind`, `MemorySource`, `LayerStatus`,
//!   `MemoryLayer`, `MemoryLayerInfo` (the wire / preview types).
//! - [`file`] — 4 fixed path resolution (`user_claude_dir` / `user_dir` / `project_path`).
//! - [`tokens`] — `count_tokens` (cl100k_base via `tiktoken-rs`).
//! - [`loader`] — `MemoryCache` + `load_for_session` (mtime-fenced
//!   read-through).
//! - [`commands`] — Tauri command surface (3 commands, lives in
//!   `crate::commands::memory`).
//! - [`tests`] — `#[cfg(test)]` integration tests (≥15 cases).

pub mod file;
pub mod loader;
pub mod tokens;
pub mod types;

#[cfg(test)]
mod tests;

pub use loader::MemoryCache;
#[allow(unused_imports)]
pub use types::{LayerStatus, MemoryKind, MemoryLayer, MemoryLayerInfo, MemorySource};

/// Hard cap on a single memory file's size (100 KiB). Above this
/// the file is rejected with `LayerStatus::Error` (and a warn!).
/// Rationale: 4 files * 100KB = 400KB worst case ≈ 100K tokens,
/// which is the entire context window of a 200K model. A single
/// file larger than 100KB is almost certainly a content-store
/// accidentally committed to a memory slot, not a real CLAUDE.md.
pub const MAX_FILE_SIZE: u64 = 100 * 1024;
