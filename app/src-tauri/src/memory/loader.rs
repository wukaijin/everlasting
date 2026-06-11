/// MemoryCache + the public `load_for_session` entry point.
///
/// The cache is structured as two halves:
/// - **User layer** (1 set of 2 files, `CLAUDE.md` + `AGENTS.md`):
///   global across all projects. Read once on first access, then
///   cached; invalidated by `invalidate_user()` (called by the
///   watcher when the user edits one of the user-layer files).
/// - **Project layer** (1 set of 2 files per project): keyed by
///   `project_id`. Cached on first access; invalidated by
///   `invalidate_project(project_id)` (called by the watcher AND
///   by `delete_session` / `delete_project`).
///
/// The cache holds **`Option<[MemoryLayer; 2]>`** rather than
/// `[MemoryLayer; 2]` because:
/// 1. We want a "not yet loaded" state separate from "loaded
///    but missing" (the latter is encoded in each layer's
///    `status`).
/// 2. The watcher's invalidation handler can simply set the slot
///    to `None`; the next `load_for_session` call re-reads.
///
/// **Concurrency**: the cache uses `tokio::sync::RwLock` so
/// concurrent chat requests on different projects can read
/// their respective slots in parallel; the writer path
/// (invalidation) takes the write lock only for the duration of
/// the slot swap.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::llm::types::{CacheControl, ContentBlock};
use crate::memory::file::{load_layer, resolve_path, user_dir};
use crate::memory::types::{LayerStatus, MemoryKind, MemoryLayer, MemorySource};

/// A single project's two-file memory slot: `[CLAUDE.md, AGENTS.md]`.
///
/// `None` means "not yet loaded" — the loader must re-read on
/// the next request. `Some([...])` is the cached pair; either
/// element may be `Loaded` / `Missing` / `Error` per
/// [`LayerStatus`].
pub type ProjectSlot = [Option<MemoryLayer>; 2];

/// User-layer two-file slot.
pub type UserSlot = [Option<MemoryLayer>; 2];

/// Convert `(kind, source)` to the slot index (0 = Claude, 1 =
/// Agents). Centralised so the cache, loader, and watcher all
/// agree on the mapping.
fn slot_index(source: MemorySource) -> usize {
    match source {
        MemorySource::Claude => 0,
        MemorySource::Agents => 1,
    }
}

/// Process-wide memory cache. Lives inside `AppState`. The
/// `notify` watcher holds a `Weak<MemoryCache>` so dropping the
/// state (on app shutdown) cleanly severs the watcher callback
/// without a deadlock.
pub struct MemoryCache {
    user: RwLock<UserSlot>,
    project: RwLock<HashMap<String, ProjectSlot>>,
}

impl MemoryCache {
    /// Empty cache. Both slots start as `None` (not yet loaded);
    /// the first chat request will populate them.
    pub fn new() -> Self {
        Self {
            user: RwLock::new([None, None]),
            project: RwLock::new(HashMap::new()),
        }
    }

    /// Wrap in `Arc` for storage in `AppState` and for the
    /// watcher to share. Convenience: `Arc::new(MemoryCache::new())`.
    pub fn arc() -> Arc<Self> {
        Arc::new(Self::new())
    }

    /// Invalidate the entire user-layer cache. The next
    /// `load_for_session` call will re-read both user files.
    /// Cheap — just sets both slots to `None`.
    #[allow(dead_code)]
    pub async fn invalidate_user(&self) {
        let mut guard = self.user.write().await;
        guard[0] = None;
        guard[1] = None;
    }

    /// Invalidate a single user-layer slot (e.g. the watcher
    /// saw a write to `CLAUDE.md`; `AGENTS.md` is unchanged and
    /// its cached `Loaded` / `Missing` is still valid).
    pub async fn invalidate_user_slot(&self, source: MemorySource) {
        let mut guard = self.user.write().await;
        guard[slot_index(source)] = None;
    }

    /// Invalidate an entire project's cache (both files). Called
    /// by `delete_session` and `delete_project` and by the
    /// watcher for any write under a project's directory.
    pub async fn invalidate_project(&self, project_id: &str) {
        let mut guard = self.project.write().await;
        guard.remove(project_id);
    }

    /// Invalidate a single project-layer slot.
    pub async fn invalidate_project_slot(
        &self,
        project_id: &str,
        source: MemorySource,
    ) {
        let mut guard = self.project.write().await;
        if let Some(slot) = guard.get_mut(project_id) {
            slot[slot_index(source)] = None;
        }
    }

    /// Read-only peek at a user-layer slot. Returns `None` if
    /// the slot has never been populated; otherwise returns the
    /// cached layer (which may itself be `Missing` / `Error`).
    pub async fn peek_user(&self, source: MemorySource) -> Option<MemoryLayer> {
        let guard = self.user.read().await;
        guard[slot_index(source)].clone()
    }

    /// Read-only peek at a project-layer slot.
    pub async fn peek_project(
        &self,
        project_id: &str,
        source: MemorySource,
    ) -> Option<MemoryLayer> {
        let guard = self.project.read().await;
        guard
            .get(project_id)
            .and_then(|slot| slot[slot_index(source)].clone())
    }
}

impl Default for MemoryCache {
    fn default() -> Self {
        Self::new()
    }
}

/// The 4 fixed file paths the cache is responsible for. Used by
/// the watcher to register the right inotify watches and by the
/// Tauri command `read_memory_content` to resolve a path back
/// to a `(kind, source)` tuple for the preview UI.
///
/// `project_root` is the project's `path` column; pass `None`
/// for the user layer. Returns 4 `(kind, source, absolute_path)`
/// triples in canonical order: User CLAUDE → User AGENTS →
/// Project CLAUDE → Project AGENTS.
pub fn all_paths(project_root: Option<&str>) -> Vec<(MemoryKind, MemorySource, PathBuf)> {
    let mut out = Vec::with_capacity(4);
    if let Some(user) = user_dir() {
        out.push((MemoryKind::User, MemorySource::Claude, user.join(MemorySource::Claude.filename())));
        out.push((MemoryKind::User, MemorySource::Agents, user.join(MemorySource::Agents.filename())));
    }
    if let Some(root) = project_root {
        let p = PathBuf::from(root);
        out.push((MemoryKind::Project, MemorySource::Claude, p.join(MemorySource::Claude.filename())));
        out.push((MemoryKind::Project, MemorySource::Agents, p.join(MemorySource::Agents.filename())));
    }
    out
}

/// Public entry point. Loads (or returns cached) memory layers
/// for a given project, in canonical order:
///   1. User CLAUDE.md
///   2. User AGENTS.md
///   3. Project CLAUDE.md
///   4. Project AGENTS.md
///
/// Read-through: cache misses trigger `load_layer`; cache hits
/// return the stored value. Always returns a 4-element `Vec` —
/// the agent loop relies on the index to format the LLM banner
/// consistently.
///
/// `cache` is the shared `Arc<MemoryCache>`; `project_id` is the
/// session's `projects.id`; `project_path` is the matching
/// `projects.path`.
pub async fn load_for_session(
    cache: &MemoryCache,
    project_id: &str,
    project_path: &str,
) -> Vec<MemoryLayer> {
    let mut out = Vec::with_capacity(4);
    // 1+2. User layer.
    for source in [MemorySource::Claude, MemorySource::Agents] {
        let layer = read_or_load_user(cache, source).await;
        out.push(layer);
    }
    // 3+4. Project layer.
    for source in [MemorySource::Claude, MemorySource::Agents] {
        let layer = read_or_load_project(cache, project_id, project_path, source).await;
        out.push(layer);
    }
    out
}

/// User-layer read-through.
async fn read_or_load_user(cache: &MemoryCache, source: MemorySource) -> MemoryLayer {
    if let Some(cached) = cache.peek_user(source).await {
        return cached;
    }
    let layer = load_layer(MemoryKind::User, source, None).await;
    let mut guard = cache.user.write().await;
    guard[slot_index(source)] = Some(layer.clone());
    layer
}

/// Project-layer read-through.
async fn read_or_load_project(
    cache: &MemoryCache,
    project_id: &str,
    project_path: &str,
    source: MemorySource,
) -> MemoryLayer {
    if let Some(cached) = cache.peek_project(project_id, source).await {
        return cached;
    }
    let layer = load_layer(MemoryKind::Project, source, Some(project_path)).await;
    let mut guard = cache.project.write().await;
    let entry = guard
        .entry(project_id.to_string())
        .or_insert([None, None]);
    entry[slot_index(source)] = Some(layer.clone());
    layer
}

/// Build the LLM-facing banner string. Always non-empty when at
/// least one layer is `Loaded`; the agent loop calls this once
/// per turn to format the system prompt header.
///
/// Format:
/// ```text
/// <system>已加载 N 个 memory: [User CLAUDE.md] (X tokens) / [Project AGENTS.md] (Y tokens)</system>
/// ```
///
/// Returns the empty string when NO layer is `Loaded` (so the
/// caller can skip the banner entirely — a fresh install with
/// no memory files produces no banner noise).
pub fn build_banner(layers: &[MemoryLayer]) -> String {
    let loaded: Vec<&MemoryLayer> = layers
        .iter()
        .filter(|l| matches!(l.status, LayerStatus::Loaded))
        .collect();
    if loaded.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = loaded
        .iter()
        .map(|l| format!("{} ({} tokens)", l.label(), l.tokens))
        .collect();
    format!(
        "<system>已加载 {} 个 memory: {}</system>",
        loaded.len(),
        parts.join(" / ")
    )
}

/// Build the cacheable content blocks for the synthetic
/// "instructions" user message injected at the head of every
/// agent-loop invocation.
///
/// B5 refactor (2026-06-11): the previous design concatenated
/// the 4 instruction files into the `system_prompt` and sent
/// the entire string on every turn of the 20-turn loop (≈ 8 MB
/// of input tokens per session with all 4 files at 100 KiB).
/// This function produces a block-shaped payload instead:
///
/// - The first block is the banner from [`build_banner`] and
///   carries `cache_control: Some(CacheControl::Ephemeral)`.
///   Anthropic reads this as a cache breakpoint: everything
///   before it (on subsequent turns within the 5-min TTL)
///   becomes eligible for a cache hit, billed at 0.1× the input
///   rate. The banner is always the first block, so the
///   breakpoint is at a stable position relative to the
///   instructions content that follows.
/// - Subsequent blocks (one per loaded layer, in canonical
///   order: User CLAUDE → User AGENTS → Project CLAUDE →
///   Project AGENTS) carry the file body. AGENTS.md is wrapped
///   in `<primary instructions>...</primary>` because it is
///   written specifically for Everlasting; CLAUDE.md is wrapped
///   in `<reference>...</reference>` because it is the
///   Claude-Code interop file (see review §3 Q4). Neither
///   carries `cache_control` — only the banner block is the
///   cache marker, per Anthropic's "last cache_control block is
///   the breakpoint" rule.
///
/// `Missing` / `Error` layers are skipped (the agent silently
/// absorbs them).
///
/// Returns an empty `Vec` when NO layer is `Loaded` — the
/// caller skips the synthetic message entirely on a fresh
/// install.
pub fn build_instructions_blocks(layers: &[MemoryLayer]) -> Vec<ContentBlock> {
    let loaded: Vec<&MemoryLayer> = layers
        .iter()
        .filter(|l| matches!(l.status, LayerStatus::Loaded))
        .collect();
    if loaded.is_empty() {
        return Vec::new();
    }

    let mut out: Vec<ContentBlock> = Vec::with_capacity(loaded.len() + 1);

    // Block 0: banner + cache_control: ephemeral. This is the
    // cache breakpoint on subsequent turns.
    out.push(ContentBlock::Text {
        text: build_banner(layers),
        cache_control: Some(CacheControl::Ephemeral),
    });

    // Blocks 1..N: per-layer file body, with AGENTS.md / CLAUDE.md
    // priority wrapping per the B5 review §3 Q4 decision.
    for layer in loaded {
        let section = match layer.render_prompt_section() {
            Some(s) => s,
            None => continue, // defensive; the `Loaded` filter above already excludes this
        };
        let text = match layer.source {
            MemorySource::Agents => {
                format!("<primary instructions>\n{}\n</primary instructions>", section)
            }
            MemorySource::Claude => {
                format!("<reference>\n{}\n</reference>", section)
            }
        };
        out.push(ContentBlock::Text {
            text,
            cache_control: None,
        });
    }

    out
}

/// Resolve a `path` argument from the frontend (Tauri command
/// `read_memory_content(path)`) back to the `(kind, source,
/// project_path)` triple that the cache uses. We accept any path
/// that matches one of the 4 fixed files; the frontend sends
/// the canonical path it got from `read_memory_layers`.
///
/// Returns `None` if `path` does not match any of the 4 files —
/// the caller treats this as an invalid argument and returns
/// an `Err` to the IPC.
#[allow(dead_code)] // exposed for the Tauri command in PR2; the IPC surface uses it next
pub fn resolve_known_path(
    path: &std::path::Path,
    project_path: &str,
) -> Option<(MemoryKind, MemorySource, Option<String>)> {
    let canon = path.canonicalize().ok().unwrap_or_else(|| path.to_path_buf());
    for (kind, source, candidate) in all_paths(Some(project_path)) {
        if candidate == canon || candidate == path {
            return Some((
                kind,
                source,
                if matches!(kind, MemoryKind::Project) {
                    Some(project_path.to_string())
                } else {
                    None
                },
            ));
        }
    }
    None
}

/// Variant of `resolve_known_path` that does not require a
/// project path (used for the global user-layer read). Returns
/// `Some((MemoryKind, MemorySource, None))` if the path matches
/// a user-layer file.
#[allow(dead_code)]
pub fn resolve_user_known_path(
    path: &std::path::Path,
) -> Option<(MemoryKind, MemorySource)> {
    let canon = path.canonicalize().ok().unwrap_or_else(|| path.to_path_buf());
    for (kind, source, candidate) in all_paths(None) {
        if candidate == canon || candidate == path {
            if matches!(kind, MemoryKind::User) {
                return Some((kind, source));
            }
        }
    }
    None
}

/// Re-exported for the Tauri command surface (defined in
/// `crate::commands::memory`). Avoids the watcher module
/// re-importing `resolve_path` directly.
#[allow(dead_code)]
pub fn resolve_one(
    kind: MemoryKind,
    source: MemorySource,
    project_path: Option<&str>,
) -> Option<PathBuf> {
    resolve_path(kind, source, project_path)
}
