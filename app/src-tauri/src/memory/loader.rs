/// MemoryCache + the public `load_for_session` entry point.
///
/// The cache is structured as two halves:
/// - **User layer** (1 set of 2 files, `CLAUDE.md` + `AGENTS.md`):
///   global across all projects. Read once on first access, then
///   cached.
/// - **Project layer** (1 set of 2 files per project): keyed by
///   `project_id`. Cached on first access.
///
/// Each slot holds `Option<CachedLayer>` where `CachedLayer` pairs
/// the loaded `MemoryLayer` with the file's `mtime` at load time.
/// `None` means "not yet loaded"; the read path stats the current
/// `mtime` and reloads when it differs (RULE-C-001 fence) — so
/// freshness is decided at read time, with no background watcher.
///
/// **Concurrency**: the cache uses `tokio::sync::RwLock` so
/// concurrent chat requests on different projects can read
/// their respective slots in parallel; the writer path (a cache
/// miss / mtime change) takes the write lock only for the
/// duration of the slot swap.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use tokio::sync::RwLock;

use crate::llm::types::{CacheControl, ContentBlock};
use crate::memory::file::{load_layer, resolve_path, user_dir};
use crate::memory::types::{LayerStatus, MemoryKind, MemoryLayer, MemorySource};

/// A cached memory layer paired with the file's `mtime` at load
/// time. The read-through fence stats the current `mtime` and
/// compares: equal ⇒ cache hit (return `layer`), different ⇒ the
/// file changed (or appeared / vanished) ⇒ reload.
///
/// `mtime = None` covers both "file absent" and "filesystem
/// refuses `modified()`" (rare); a file that later appears flips
/// to `Some` and trips the inequality. This makes the read path
/// the authority on freshness — no dependence on a background
/// watcher (RULE-C-001 fence, 2026-06-15).
#[derive(Clone)]
pub(crate) struct CachedLayer {
    layer: MemoryLayer,
    mtime: Option<SystemTime>,
}

/// A single project's two-file memory slot: `[CLAUDE.md, AGENTS.md]`.
///
/// `None` means "not yet loaded" — the loader must re-read on
/// the next request. `Some([...])` is the cached pair; either
/// element may be `Loaded` / `Missing` / `Error` per
/// [`LayerStatus`].
pub type ProjectSlot = [Option<CachedLayer>; 2];

/// User-layer two-file slot.
pub type UserSlot = [Option<CachedLayer>; 2];

/// Convert `(kind, source)` to the slot index (0 = Claude, 1 =
/// Agents). Centralised so the cache and loader agree on the
/// mapping.
fn slot_index(source: MemorySource) -> usize {
    match source {
        MemorySource::Claude => 0,
        MemorySource::Agents => 1,
    }
}

/// Stat a memory file's `modified` time for the read-through
/// fence. Returns `None` when the path is unresolvable, the file
/// is absent, or the filesystem refuses `modified()` — all
/// treated as "no usable mtime" (see [`CachedLayer`]).
///
/// Uses `tokio::fs::metadata` (async) so we never block the
/// runtime on a syscall (contrast RULE-E-004's glob lesson).
async fn file_mtime(path: Option<&std::path::Path>) -> Option<SystemTime> {
    let path = path?;
    tokio::fs::metadata(path)
        .await
        .ok()
        .and_then(|m| m.modified().ok())
}

/// Process-wide memory cache. Lives inside `AppState`. Read-
/// through with an mtime fence (no background watcher).
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

    /// Wrap in `Arc` for storage in `AppState`. Convenience:
    /// `Arc::new(MemoryCache::new())`.
    pub fn arc() -> Arc<Self> {
        Arc::new(Self::new())
    }

    /// Read-only peek at a user-layer slot. Returns `None` if
    /// the slot has never been populated; otherwise returns the
    /// cached layer (which may itself be `Missing` / `Error`).
    ///
    /// Note: this peeks the **cache**, not the file — it does not
    /// apply the mtime fence. The read path goes through
    /// `read_or_load_user` instead. Kept as an introspection API
    /// for tests / diagnostics.
    #[allow(dead_code)]
    pub async fn peek_user(&self, source: MemorySource) -> Option<MemoryLayer> {
        let guard = self.user.read().await;
        guard[slot_index(source)].as_ref().map(|c| c.layer.clone())
    }

    /// Read-only peek at a project-layer slot.
    #[allow(dead_code)]
    pub async fn peek_project(
        &self,
        project_id: &str,
        source: MemorySource,
    ) -> Option<MemoryLayer> {
        let guard = self.project.read().await;
        guard
            .get(project_id)
            .and_then(|slot| slot[slot_index(source)].as_ref().map(|c| c.layer.clone()))
    }
}

impl Default for MemoryCache {
    fn default() -> Self {
        Self::new()
    }
}

/// The 4 fixed file paths the cache is responsible for. Used by
/// `load_for_session`'s mtime fence to stat each file and by the
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

/// User-layer read-through with an mtime fence (RULE-C-001).
///
/// Stats the file's current `mtime`; if it matches the cached
/// slot's `mtime`, return the cached layer (hit). Otherwise
/// reload from disk and store the fresh layer + mtime. This
/// makes the read path the authority on freshness — a file
/// saved between cache population and the next read is always
/// reflected, with no dependence on a background watcher.
async fn read_or_load_user(cache: &MemoryCache, source: MemorySource) -> MemoryLayer {
    let path = resolve_path(MemoryKind::User, source, None);
    let mtime = file_mtime(path.as_deref()).await;
    {
        let guard = cache.user.read().await;
        if let Some(cached) = &guard[slot_index(source)] {
            if cached.mtime == mtime {
                return cached.layer.clone();
            }
        }
    }
    let layer = load_layer(MemoryKind::User, source, None).await;
    let mut guard = cache.user.write().await;
    guard[slot_index(source)] = Some(CachedLayer {
        layer: layer.clone(),
        mtime,
    });
    layer
}

/// Project-layer read-through with an mtime fence (RULE-C-001).
/// See [`read_or_load_user`] for the fence rationale.
async fn read_or_load_project(
    cache: &MemoryCache,
    project_id: &str,
    project_path: &str,
    source: MemorySource,
) -> MemoryLayer {
    let path = resolve_path(MemoryKind::Project, source, Some(project_path));
    let mtime = file_mtime(path.as_deref()).await;
    {
        let guard = cache.project.read().await;
        if let Some(slot) = guard.get(project_id) {
            if let Some(cached) = &slot[slot_index(source)] {
                if cached.mtime == mtime {
                    return cached.layer.clone();
                }
            }
        }
    }
    let layer = load_layer(MemoryKind::Project, source, Some(project_path)).await;
    let mut guard = cache.project.write().await;
    let entry = guard
        .entry(project_id.to_string())
        .or_insert([None, None]);
    entry[slot_index(source)] = Some(CachedLayer {
        layer: layer.clone(),
        mtime,
    });
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
/// `crate::commands::memory`).
#[allow(dead_code)]
pub fn resolve_one(
    kind: MemoryKind,
    source: MemorySource,
    project_path: Option<&str>,
) -> Option<PathBuf> {
    resolve_path(kind, source, project_path)
}
