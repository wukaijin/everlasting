//! B5 Memory — User / Project two-layer loader (V2 first-tier, 2026-06-10).
//!
//! Loads Markdown memory files (CLAUDE.md / AGENTS.md) from two fixed
//! layers — User (`~/.config/everlasting/`) and Project
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
//! **Hot-reload**: a `notify::RecommendedWatcher` watches the 4
//! fixed paths. Events are coalesced through a 1s debounce so a
//! single editor save (which fires several inotify events) yields a
//! single cache invalidation. The cache lives in
//! `AppState::memory_cache`; reads are non-blocking through
//! `tokio::sync::RwLock`.
//!
//! Files in this module:
//! - [`types`] — `MemoryKind`, `MemorySource`, `LayerStatus`,
//!   `MemoryLayer`, `MemoryLayerInfo` (the wire / preview types).
//! - [`file`] — 4 fixed path resolution (`user_dir` / `project_path`).
//! - [`tokens`] — `count_tokens` (cl100k_base via `tiktoken-rs`).
//! - [`loader`] — `MemoryCache` + `load_for_session` + `invalidate_*`.
//! - [`watcher`] — `notify` integration + 1s debounce.
//! - [`commands`] — Tauri command surface (3 commands, lives in
//!   `crate::commands::memory`).
//! - [`tests`] — `#[cfg(test)]` integration tests (≥15 cases).

pub mod file;
pub mod loader;
pub mod tokens;
pub mod types;
pub mod watcher;

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

/// 1-second debounce window for `notify` events. Editor save emits
/// multiple inotify events (Modify + CloseWrite + sometimes
/// Create); coalescing them into a single cache invalidation keeps
/// the watcher quiet.
pub const WATCHER_DEBOUNCE_MS: u64 = 1000;
