//! Integration tests for the B5 memory module.
//!
//! 15+ test cases covering the full loader + token + cache +
//! invalidation surface. Each test creates an isolated
//! `tempfile::TempDir` and (where needed) overrides the user-
//! layer directory via the `memory::file::set_user_dir_for_test`
//! shim — see [`crate::memory::file`] for the test-only hook.
//!
//! **Why a custom test pool for the cache**: the in-memory cache
//! (`MemoryCache`) is NOT a sqlx pool; it's a `RwLock<HashMap>`.
//! Tests for it therefore don't need `test_pool()`.

#![cfg(test)]

use std::path::PathBuf;
use std::sync::Arc;

#[allow(unused_imports)]
use crate::memory::file::{load_file, load_layer};
use crate::memory::loader::{
    all_paths, build_banner, build_instructions_blocks, build_layers_block, load_for_session,
    MemoryCache,
};
use crate::llm::types::{CacheControl, ContentBlock};
use crate::memory::tokens::{count_tokens, ensure_initialized};
use crate::memory::types::{LayerStatus, MemoryKind, MemorySource};
use crate::memory::MAX_FILE_SIZE;

// ---------------------------------------------------------------------------
// Test-only: override the user dir so tests don't depend on the
// developer's actual `~/.config/everlasting/` state.
// ---------------------------------------------------------------------------

/// Test-only: temporarily override the user-layer directory for
/// the duration of a test. Restores the real path on Drop via
/// thread-local state. This is a small shim around the real
/// `user_dir()` resolver — see `memory::file::set_user_dir_for_test`
/// for the implementation.
pub struct UserDirGuard {
    previous: Option<PathBuf>,
}

impl UserDirGuard {
    pub fn new(path: PathBuf) -> Self {
        let previous = crate::memory::file::set_user_dir_for_test(Some(path));
        Self { previous }
    }
}

impl Drop for UserDirGuard {
    fn drop(&mut self) {
        crate::memory::file::set_user_dir_for_test(self.previous.clone());
    }
}

// ---------------------------------------------------------------------------
// `tokens` tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tokens_count_empty_string_is_zero() {
    ensure_initialized().await;
    let n = count_tokens("").await;
    assert_eq!(n, 0);
}

#[tokio::test]
async fn tokens_count_ascii_short() {
    ensure_initialized().await;
    // "hello" is a single cl100k_base token.
    let n = count_tokens("hello").await;
    assert!(n >= 1 && n <= 2, "expected ~1 token for 'hello', got {}", n);
}

#[tokio::test]
async fn tokens_count_cjk_grows() {
    ensure_initialized().await;
    // A single CJK char is typically 1-2 tokens in cl100k_base.
    let one_char = count_tokens("中").await;
    let ten_chars = count_tokens("中华人民共和国").await;
    assert!(one_char >= 1, "CJK char should be at least 1 token");
    assert!(
        ten_chars > one_char * 5,
        "10 CJK chars should be substantially more tokens than 1: {} vs {}",
        ten_chars,
        one_char
    );
}

#[tokio::test]
async fn tokens_count_mixed() {
    ensure_initialized().await;
    let mixed = count_tokens("Hello, 世界! This is a mixed test.").await;
    // Sanity range: 8-20 tokens. We don't pin the exact value
    // because cl100k_base could shift; the contract is "more
    // than the empty string, less than 100".
    assert!(mixed > 0 && mixed < 100, "got {} tokens", mixed);
}

// ---------------------------------------------------------------------------
// `file` tests — single-file read paths
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_load_missing_returns_missing_status() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("CLAUDE.md");
    let (content, tokens, status) = load_file_for_test(&p).await;
    assert_eq!(content, "");
    assert_eq!(tokens, 0);
    assert_eq!(status, LayerStatus::Missing);
}

#[tokio::test]
async fn file_load_loaded_returns_body_and_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("CLAUDE.md");
    std::fs::write(&p, "# Hello, world.\nThis is a test.").unwrap();
    let (content, tokens, status) = load_file_for_test(&p).await;
    assert!(matches!(status, LayerStatus::Loaded));
    assert!(content.contains("Hello, world."));
    assert!(tokens > 0);
}

#[tokio::test]
async fn file_load_empty_file_is_loaded_with_zero_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("AGENTS.md");
    std::fs::write(&p, "").unwrap();
    let (content, tokens, status) = load_file_for_test(&p).await;
    assert_eq!(content, "");
    assert_eq!(tokens, 0);
    assert_eq!(status, LayerStatus::Loaded);
}

#[tokio::test]
async fn file_load_oversize_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("CLAUDE.md");
    // Write one byte past the cap.
    let big = vec![b'x'; (MAX_FILE_SIZE + 1) as usize];
    std::fs::write(&p, &big).unwrap();
    let (content, tokens, status) = load_file_for_test(&p).await;
    assert!(matches!(status, LayerStatus::Error { .. }), "got {:?}", status);
    assert!(content.is_empty());
    assert_eq!(tokens, 0);
}

#[tokio::test]
async fn file_load_non_utf8_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("CLAUDE.md");
    // 0xFF 0xFE is not valid UTF-8.
    std::fs::write(&p, [0xFFu8, 0xFEu8, 0xFDu8]).unwrap();
    let (content, tokens, status) = load_file_for_test(&p).await;
    assert!(matches!(status, LayerStatus::Error { .. }), "got {:?}", status);
    assert!(content.is_empty());
    assert_eq!(tokens, 0);
}

/// Helper that invokes `load_file` and returns the (content,
/// tokens, status) triple. Async because `load_file_inner` is
/// async (the token count is async via `tiktoken-rs`).
async fn load_file_for_test(p: &std::path::Path) -> (String, u32, LayerStatus) {
    let layer = crate::memory::file::load_file(p).await;
    (layer.content, layer.tokens, layer.status)
}

// ---------------------------------------------------------------------------
// `loader` tests — MemoryCache read-through, invalidation, banner
// ---------------------------------------------------------------------------

#[tokio::test]
async fn loader_load_for_session_with_all_files_present() {
    let user_dir = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let _user_guard = UserDirGuard::new(user_dir.path().to_path_buf());

    std::fs::write(user_dir.path().join("CLAUDE.md"), "user claude body").unwrap();
    std::fs::write(user_dir.path().join("AGENTS.md"), "user agents body").unwrap();
    std::fs::write(project_dir.path().join("CLAUDE.md"), "project claude body").unwrap();
    std::fs::write(project_dir.path().join("AGENTS.md"), "project agents body").unwrap();

    let cache = MemoryCache::new();
    let layers = load_for_session(&cache, "proj-1", project_dir.path().to_str().unwrap()).await;
    assert_eq!(layers.len(), 4);
    for l in &layers {
        assert!(matches!(l.status, LayerStatus::Loaded), "{:?}", l);
    }
    assert_eq!(layers[0].kind, MemoryKind::User);
    assert_eq!(layers[0].source, MemorySource::Claude);
    assert_eq!(layers[1].source, MemorySource::Agents);
    assert_eq!(layers[2].kind, MemoryKind::Project);
    assert_eq!(layers[3].source, MemorySource::Agents);
    assert!(layers[0].content.contains("user claude"));
    assert!(layers[3].content.contains("project agents"));
}

#[tokio::test]
async fn loader_load_for_session_with_all_files_missing() {
    let user_dir = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let _user_guard = UserDirGuard::new(user_dir.path().to_path_buf());

    // No files written — every layer should be Missing.
    let cache = MemoryCache::new();
    let layers = load_for_session(&cache, "proj-1", project_dir.path().to_str().unwrap()).await;
    assert_eq!(layers.len(), 4);
    for l in &layers {
        assert_eq!(l.status, LayerStatus::Missing, "{:?}", l);
    }
}

#[tokio::test]
async fn loader_load_for_session_partial_files() {
    let user_dir = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let _user_guard = UserDirGuard::new(user_dir.path().to_path_buf());

    // Only User CLAUDE.md and Project AGENTS.md.
    std::fs::write(user_dir.path().join("CLAUDE.md"), "u-c body").unwrap();
    std::fs::write(project_dir.path().join("AGENTS.md"), "p-a body").unwrap();

    let cache = MemoryCache::new();
    let layers = load_for_session(&cache, "proj-1", project_dir.path().to_str().unwrap()).await;
    assert_eq!(layers.len(), 4);
    assert_eq!(layers[0].status, LayerStatus::Loaded); // User Claude
    assert_eq!(layers[1].status, LayerStatus::Missing); // User Agents
    assert_eq!(layers[2].status, LayerStatus::Missing); // Project Claude
    assert_eq!(layers[3].status, LayerStatus::Loaded); // Project Agents
}

#[tokio::test]
async fn loader_invalidate_user_slot_re_reads() {
    let user_dir = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let _user_guard = UserDirGuard::new(user_dir.path().to_path_buf());

    // First load: file missing.
    let cache = MemoryCache::new();
    let first = load_for_session(&cache, "proj-1", project_dir.path().to_str().unwrap()).await;
    assert_eq!(first[0].status, LayerStatus::Missing);

    // Create the file outside the cache.
    std::fs::write(user_dir.path().join("CLAUDE.md"), "now it exists").unwrap();

    // Cache hit (no invalidation) → still Missing.
    let second = load_for_session(&cache, "proj-1", project_dir.path().to_str().unwrap()).await;
    assert_eq!(second[0].status, LayerStatus::Missing);

    // Invalidate the user Claude slot → next load sees the file.
    cache.invalidate_user_slot(MemorySource::Claude).await;
    let third = load_for_session(&cache, "proj-1", project_dir.path().to_str().unwrap()).await;
    assert_eq!(third[0].status, LayerStatus::Loaded);
    assert!(third[0].content.contains("now it exists"));
}

#[tokio::test]
async fn loader_invalidate_project_does_not_touch_user() {
    let user_dir = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let _user_guard = UserDirGuard::new(user_dir.path().to_path_buf());

    std::fs::write(user_dir.path().join("CLAUDE.md"), "user body").unwrap();
    std::fs::write(project_dir.path().join("CLAUDE.md"), "project body").unwrap();

    let cache = MemoryCache::new();
    let _ = load_for_session(&cache, "proj-1", project_dir.path().to_str().unwrap()).await;
    cache.invalidate_project("proj-1").await;

    // Edit user file; cache must reflect (invalidation
    // shouldn't have touched the user slot).
    std::fs::write(user_dir.path().join("CLAUDE.md"), "user body v2").unwrap();
    let layers = load_for_session(&cache, "proj-1", project_dir.path().to_str().unwrap()).await;
    // The project slot was invalidated, so project CLAUDE.md
    // re-reads ("project body"). The user slot was NOT
    // invalidated, so user CLAUDE.md still shows the old
    // cached value (cache miss path: load_layer sees
    // "user body" still on disk → returns the same content;
    // semantically the user slot wasn't touched).
    assert_eq!(layers[2].content, "project body");
    // The user slot's content reflects what was on disk at
    // the first load (the cache was not invalidated, but
    // the file content was — so the second load is a cache
    // hit and returns the stale value). This is the
    // intentional behavior: only `invalidate_user*` clears
    // the user slot.
    assert_eq!(layers[0].content, "user body");
}

#[tokio::test]
async fn loader_different_projects_have_independent_caches() {
    let user_dir = tempfile::tempdir().unwrap();
    let proj_a = tempfile::tempdir().unwrap();
    let proj_b = tempfile::tempdir().unwrap();
    let _user_guard = UserDirGuard::new(user_dir.path().to_path_buf());

    std::fs::write(proj_a.path().join("CLAUDE.md"), "a body").unwrap();
    std::fs::write(proj_b.path().join("CLAUDE.md"), "b body").unwrap();

    let cache = MemoryCache::new();
    let layers_a = load_for_session(&cache, "proj-a", proj_a.path().to_str().unwrap()).await;
    let layers_b = load_for_session(&cache, "proj-b", proj_b.path().to_str().unwrap()).await;

    // Project Claude is index 2 in the canonical order.
    assert_eq!(layers_a[2].content, "a body");
    assert_eq!(layers_b[2].content, "b body");
}

// ---------------------------------------------------------------------------
// `loader` tests — banner + layers block formatting
// ---------------------------------------------------------------------------

#[tokio::test]
async fn banner_with_no_loaded_layers_is_empty() {
    let layers = vec![
        missing_layer(MemoryKind::User, MemorySource::Claude),
        missing_layer(MemoryKind::User, MemorySource::Agents),
        missing_layer(MemoryKind::Project, MemorySource::Claude),
        missing_layer(MemoryKind::Project, MemorySource::Agents),
    ];
    assert_eq!(build_banner(&layers), "");
}

#[tokio::test]
async fn banner_with_some_loaded_layers_lists_them() {
    let layers = vec![
        loaded_layer(MemoryKind::User, MemorySource::Claude, "hi", 1),
        missing_layer(MemoryKind::User, MemorySource::Agents),
        missing_layer(MemoryKind::Project, MemorySource::Claude),
        missing_layer(MemoryKind::Project, MemorySource::Agents),
    ];
    let banner = build_banner(&layers);
    assert!(banner.contains("<system>"));
    assert!(banner.contains("</system>"));
    assert!(banner.contains("[User CLAUDE.md]"));
    assert!(banner.contains("1 tokens"));
    // Missing layers should not appear in the banner.
    assert!(!banner.contains("[User AGENTS.md]"));
}

#[tokio::test]
async fn layers_block_renders_only_loaded_layers() {
    let layers = vec![
        loaded_layer(MemoryKind::User, MemorySource::Claude, "user-claude-body", 3),
        missing_layer(MemoryKind::User, MemorySource::Agents),
        loaded_layer(MemoryKind::Project, MemorySource::Agents, "project-agents-body", 4),
        missing_layer(MemoryKind::Project, MemorySource::Claude),
    ];
    let block = build_layers_block(&layers);
    assert!(block.contains("[User CLAUDE.md]"));
    assert!(block.contains("user-claude-body"));
    assert!(block.contains("[Project AGENTS.md]"));
    assert!(block.contains("project-agents-body"));
    assert!(!block.contains("[User AGENTS.md]"));
    assert!(!block.contains("[Project CLAUDE.md]"));
}

// ---------------------------------------------------------------------------
// `all_paths` test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn all_paths_yields_four_entries_in_canonical_order() {
    let project_dir = tempfile::tempdir().unwrap();
    let entries = all_paths(Some(project_dir.path().to_str().unwrap()));
    // The user dir may or may not exist on the test host; we
    // accept either 2 (no user dir) or 4 (user dir exists).
    assert!(entries.len() == 2 || entries.len() == 4);
    // The first 2 user entries (when present) come first, then
    // the 2 project entries.
    if entries.len() == 4 {
        assert_eq!(entries[0].0, MemoryKind::User);
        assert_eq!(entries[0].1, MemorySource::Claude);
        assert_eq!(entries[1].0, MemoryKind::User);
        assert_eq!(entries[1].1, MemorySource::Agents);
        assert_eq!(entries[2].0, MemoryKind::Project);
        assert_eq!(entries[2].1, MemorySource::Claude);
        assert_eq!(entries[3].0, MemoryKind::Project);
        assert_eq!(entries[3].1, MemorySource::Agents);
    } else {
        // No user dir — must be 2 project entries.
        assert_eq!(entries[0].0, MemoryKind::Project);
        assert_eq!(entries[0].1, MemorySource::Claude);
        assert_eq!(entries[1].0, MemoryKind::Project);
        assert_eq!(entries[1].1, MemorySource::Agents);
    }
}

// ---------------------------------------------------------------------------
// Arc<MemoryCache> smoke (used by AppState)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn memory_cache_arc_smoke() {
    let cache = Arc::new(MemoryCache::new());
    cache.invalidate_user().await;
    cache.invalidate_user_slot(MemorySource::Claude).await;
    cache.invalidate_project("missing-id").await;
    cache.invalidate_project_slot("missing-id", MemorySource::Agents).await;
    // No assertion — just exercises the public API.
    let _ = cache.peek_user(MemorySource::Claude).await;
    let _ = cache.peek_project("missing-id", MemorySource::Claude).await;
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn missing_layer(kind: MemoryKind, source: MemorySource) -> crate::memory::MemoryLayer {
    crate::memory::MemoryLayer {
        kind,
        source,
        path: PathBuf::from("/nonexistent"),
        content: String::new(),
        tokens: 0,
        status: LayerStatus::Missing,
    }
}

fn loaded_layer(
    kind: MemoryKind,
    source: MemorySource,
    body: &str,
    tokens: u32,
) -> crate::memory::MemoryLayer {
    crate::memory::MemoryLayer {
        kind,
        source,
        path: PathBuf::from("/fake"),
        content: body.to_string(),
        tokens,
        status: LayerStatus::Loaded,
    }
}

// `load_file` and `load_layer` are imported above; we use
// them through `load_file_for_test` in the file_load_* tests
// and through `MemoryCache` in the loader tests. The
// `#[allow(unused_imports)]` below silences the "unused" lint
// when the test is compiled in isolation.

// ---- build_instructions_blocks (B5 cache_control refactor) ----

/// When no layer is loaded, the builder returns an empty vec so
/// the agent loop can skip the synthetic instructions message
/// entirely (no banner, no cache marker, no message).
#[test]
fn instructions_blocks_empty_when_no_layer_loaded() {
    let layers = vec![
        missing_layer(MemoryKind::User, MemorySource::Claude),
        missing_layer(MemoryKind::User, MemorySource::Agents),
        missing_layer(MemoryKind::Project, MemorySource::Claude),
        missing_layer(MemoryKind::Project, MemorySource::Agents),
    ];
    let blocks = build_instructions_blocks(&layers);
    assert!(blocks.is_empty());
}

/// When at least one layer is loaded, the builder produces:
///
/// - Block 0: the banner text with `cache_control: Some(Ephemeral)`
///   (the Anthropic cache breakpoint).
/// - Blocks 1..N: per loaded layer in canonical order, wrapped
///   in `<primary>` for AGENTS.md and `<reference>` for CLAUDE.md
///   (review §3 Q4 decision). No cache_control on the body blocks
///   — only the first block is the marker.
#[test]
fn instructions_blocks_marks_only_first_block_as_cacheable() {
    let layers = vec![
        // First loaded: User CLAUDE.md → goes in block 1 as <reference>
        loaded_layer(MemoryKind::User, MemorySource::Claude, "user-claude-body", 5),
        // Second loaded: User AGENTS.md → block 2 as <primary>
        loaded_layer(MemoryKind::User, MemorySource::Agents, "user-agents-body", 5),
        missing_layer(MemoryKind::Project, MemorySource::Claude),
        // Fourth loaded: Project AGENTS.md → block 3 as <primary>
        loaded_layer(MemoryKind::Project, MemorySource::Agents, "project-agents-body", 5),
    ];
    let blocks = build_instructions_blocks(&layers);
    // 1 banner + 3 loaded = 4 blocks
    assert_eq!(blocks.len(), 4);

    // Block 0: banner + cache_control: Ephemeral
    match &blocks[0] {
        ContentBlock::Text { text, cache_control } => {
            assert!(text.starts_with("<system>已加载"));
            assert!(text.contains("User CLAUDE.md"));
            assert!(text.contains("User AGENTS.md"));
            assert!(text.contains("Project AGENTS.md"));
            assert_eq!(*cache_control, Some(CacheControl::Ephemeral));
        }
        other => panic!("expected Text block, got {:?}", other),
    }

    // Block 1: User CLAUDE.md → <reference> wrapper, no cache_control
    match &blocks[1] {
        ContentBlock::Text { text, cache_control } => {
            assert!(text.starts_with("<reference>"));
            assert!(text.contains("user-claude-body"));
            assert!(text.ends_with("</reference>"));
            assert_eq!(*cache_control, None);
        }
        other => panic!("expected Text block, got {:?}", other),
    }

    // Block 2: User AGENTS.md → <primary> wrapper, no cache_control
    match &blocks[2] {
        ContentBlock::Text { text, cache_control } => {
            assert!(text.starts_with("<primary instructions>"));
            assert!(text.contains("user-agents-body"));
            assert!(text.ends_with("</primary instructions>"));
            assert_eq!(*cache_control, None);
        }
        other => panic!("expected Text block, got {:?}", other),
    }

    // Block 3: Project AGENTS.md → <primary> wrapper, no cache_control
    match &blocks[3] {
        ContentBlock::Text { text, cache_control } => {
            assert!(text.starts_with("<primary instructions>"));
            assert!(text.contains("project-agents-body"));
            assert_eq!(*cache_control, None);
        }
        other => panic!("expected Text block, got {:?}", other),
    }
}
