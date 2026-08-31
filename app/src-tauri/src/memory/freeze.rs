//! D2 (08-31-cache-head-volatility): session-scoped freeze of the
//! 4 instruction files (User/Project × CLAUDE.md/AGENTS.md).
//!
//! ## Why
//!
//! The instruction blocks are injected at `messages[0..1]` — the
//! very head of every request. The OpenAI-compatible path
//! prefix-caches from byte 0 with no `cache_control` breakpoints,
//! so whenever a new chat request re-reads the files from disk
//! and they changed mid-session (e.g. the agent itself edited
//! AGENTS.md with `write_file`), the head forks and the whole
//! cached prefix is re-billed (live evidence: session d6728b3a
//! seq 437, cache_read=0 on a 281k-token request right after the
//! agent's own instruction-file edits).
//!
//! ## Contract
//!
//! Within one session, the FIRST request's loaded layers are
//! frozen: every later request of the same session reuses that
//! snapshot even if the disk files changed. Semantics: an
//! instruction edit is "for future sessions" (they are norms, not
//! runtime config) — this matches how the repo actually uses
//! them. A daemon restart drops the in-memory map, so the first
//! request after a restart re-reads once (equivalent to "the
//! session's first request") — accepted by design.
//!
//! ## Shape
//!
//! Process-level `OnceLock` singleton keyed by `session_id`, the
//! same precedent as `memory::digest::registry()` /
//! `agent::compaction::compaction_registry()`: the read site
//! (`chat_loop/init.rs`) cannot thread an extra handle through
//! the 72 `run_chat_loop` call sites, and `delete_session_inner`
//! is the single shared cleanup point (Tauri command + daemon
//! route). No cross-session leakage — entries are strictly keyed
//! by session id and cleared on session delete.
//!
//! The kill-switch const below reverts the freeze wholesale
//! (compile-time; flip + rebuild) back to the mtime-fenced
//! per-request re-read.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use tokio::sync::RwLock;

use crate::memory::loader::{load_for_session, MemoryCache};
use crate::memory::types::MemoryLayer;

/// Compile-time kill-switch (design D2 rollback point). `true` =
/// freeze instruction layers per session (default). `false` =
/// every request re-reads via the mtime fence (pre-D2 behavior,
/// byte-identical).
pub const INSTRUCTION_FREEZE_ENABLED: bool = true;

/// Session → frozen instruction layers (the 4-element canonical
/// `Vec<MemoryLayer>` `load_for_session` returns).
#[derive(Default)]
pub struct InstructionFreeze {
    inner: RwLock<HashMap<String, Arc<Vec<MemoryLayer>>>>,
}

static FREEZE: OnceLock<InstructionFreeze> = OnceLock::new();

/// Process-level singleton access point (same pattern as
/// `memory::digest::registry()`).
pub fn freeze_registry() -> &'static InstructionFreeze {
    FREEZE.get_or_init(InstructionFreeze::default)
}

impl InstructionFreeze {
    /// The session's frozen layers, if a previous request already
    /// froze them. `Arc`-shared so the (potentially 100s-of-KiB)
    /// layer bodies are cloned per request only when actually
    /// consumed downstream.
    pub async fn get(&self, session_id: &str) -> Option<Arc<Vec<MemoryLayer>>> {
        self.inner.read().await.get(session_id).cloned()
    }

    /// Freeze the session's layers (first request wins; later
    /// `set` calls are no-ops so a concurrent duplicate first
    /// request can't flip the snapshot mid-flight).
    pub async fn set(&self, session_id: &str, layers: Vec<MemoryLayer>) {
        let mut guard = self.inner.write().await;
        guard
            .entry(session_id.to_string())
            .or_insert(Arc::new(layers));
    }

    /// Drop the session's frozen layers (`delete_session_inner`
    /// wiring point — a reused session_id must not inherit the
    /// deleted session's snapshot).
    pub async fn clear(&self, session_id: &str) {
        self.inner.write().await.remove(session_id);
    }
}

/// `load_for_session` with the D2 session freeze in front. See
/// the module doc for the rationale. Exposed as the single read
/// site `chat_loop/init.rs` consumes.
pub async fn load_for_session_frozen(
    cache: &MemoryCache,
    session_id: &str,
    project_id: &str,
    project_path: &str,
) -> Vec<MemoryLayer> {
    load_frozen_impl(
        cache,
        session_id,
        project_id,
        project_path,
        INSTRUCTION_FREEZE_ENABLED,
    )
    .await
}

/// Testable core: `enabled` is the kill-switch value so both
/// branches stay coverable while the production wrapper pins the
/// const.
async fn load_frozen_impl(
    cache: &MemoryCache,
    session_id: &str,
    project_id: &str,
    project_path: &str,
    enabled: bool,
) -> Vec<MemoryLayer> {
    if enabled {
        if let Some(frozen) = freeze_registry().get(session_id).await {
            return (*frozen).clone();
        }
        let layers = load_for_session(cache, project_id, project_path).await;
        freeze_registry().set(session_id, layers.clone()).await;
        layers
    } else {
        // Kill-switch off: pre-D2 behavior, byte-identical
        // (mtime-fenced read-through every request).
        load_for_session(cache, project_id, project_path).await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::{LayerStatus, MemoryKind, MemorySource};

    /// Write a project CLAUDE.md with a distinct mtime so the
    /// loader's mtime fence sees each write as a change.
    fn write_project_claude(project_path: &std::path::Path, content: &str, stamp: u64) {
        let file = project_path.join("CLAUDE.md");
        std::fs::write(&file, content).unwrap();
        let f = std::fs::File::options().write(true).open(&file).unwrap();
        f.set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(stamp))
            .unwrap();
    }

    fn project_layer_content(layers: &[MemoryLayer]) -> String {
        layers
            .iter()
            .find(|l| {
                l.kind == MemoryKind::Project
                    && l.source == MemorySource::Claude
                    && matches!(l.status, LayerStatus::Loaded)
            })
            .map(|l| l.content.clone())
            .expect("project CLAUDE.md layer must be Loaded")
    }

    #[tokio::test]
    async fn frozen_layers_survive_disk_change_within_session() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path().to_string_lossy().to_string();
        write_project_claude(dir.path(), "FIRST-CONTENT", 1_000);

        let cache = MemoryCache::new();
        let sid = format!("freeze-{}", uuid::Uuid::new_v4());

        // First request: reads FIRST-CONTENT off disk, freezes it.
        let first = load_frozen_impl(&cache, &sid, "pid", &project_path, true).await;
        assert_eq!(project_layer_content(&first), "FIRST-CONTENT");

        // Mid-session disk change (agent edited the instruction file).
        write_project_claude(dir.path(), "SECOND-CONTENT", 2_000);

        // Second request of the SAME session: still FIRST-CONTENT
        // (frozen) — this is the seq-437 regression guard.
        let second = load_frozen_impl(&cache, &sid, "pid", &project_path, true).await;
        assert_eq!(
            project_layer_content(&second),
            "FIRST-CONTENT",
            "D2: same-session request must reuse the frozen (first) content"
        );

        // A DIFFERENT session sees the new content (no cross-session
        // leakage of the freeze).
        let other_sid = format!("freeze-{}", uuid::Uuid::new_v4());
        let other = load_frozen_impl(&cache, &other_sid, "pid", &project_path, true).await;
        assert_eq!(
            project_layer_content(&other),
            "SECOND-CONTENT",
            "a new session must read the current disk content"
        );
    }

    #[tokio::test]
    async fn kill_switch_off_reads_fresh_every_request() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path().to_string_lossy().to_string();
        write_project_claude(dir.path(), "BEFORE", 1_000);

        let cache = MemoryCache::new();
        let sid = format!("freeze-off-{}", uuid::Uuid::new_v4());

        let first = load_frozen_impl(&cache, &sid, "pid", &project_path, false).await;
        assert_eq!(project_layer_content(&first), "BEFORE");

        write_project_claude(dir.path(), "AFTER", 2_000);
        let second = load_frozen_impl(&cache, &sid, "pid", &project_path, false).await;
        assert_eq!(
            project_layer_content(&second),
            "AFTER",
            "kill-switch off: mtime fence must pick the disk change up (pre-D2 behavior)"
        );
    }

    #[tokio::test]
    async fn clear_drops_the_session_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path().to_string_lossy().to_string();
        write_project_claude(dir.path(), "ONE", 1_000);

        let cache = MemoryCache::new();
        let sid = format!("freeze-clear-{}", uuid::Uuid::new_v4());
        let _ = load_frozen_impl(&cache, &sid, "pid", &project_path, true).await;

        write_project_claude(dir.path(), "TWO", 2_000);
        freeze_registry().clear(&sid).await;

        // After clear, the session re-freezes from the CURRENT disk
        // state (delete_session_inner contract: a reused session_id
        // must not inherit the old snapshot).
        let reread = load_frozen_impl(&cache, &sid, "pid", &project_path, true).await;
        assert_eq!(project_layer_content(&reread), "TWO");
    }
}
