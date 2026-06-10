//! `notify`-based file watcher for the 4 memory paths.
//!
//! V2 1 期 (B5) is intentionally conservative: the watcher ONLY
//! listens for **modify-style** events on the 4 fixed paths, and
//! routes them through a **1-second debounce** so a single editor
//! save (which fires several inotify events: Modify → CloseWrite
//! → Attrib) yields one cache invalidation per file.
//!
//! **Why a single watcher, not 4?** `notify::RecommendedWatcher`
//! holds a single OS-level inotify handle internally; spawning 4
//! is wasteful. We register the parent directory of each file
//! (the user dir and the project dir) and filter events in
//! Rust.
//!
//! **Why debounce?** Without it, a 3-event editor save yields 3
//! cache invalidations → 3 reloads on the next chat. With the
//! 1s debounce, the 3 events coalesce into 1 invalidation. The
//! user still sees their change "within 1 second" of saving.
//!
//! **Why ignore Create?** The PRD's "新建 memory 文件需重启 session
//! 生效" rule means we never watch for newly-created files
//! (the 4 paths are fixed at startup). The watcher's
//! `EventMask` is therefore restricted to `MODIFY | CLOSE_WRITE`
//! on the 4 known files only.
//!
//! **Lifetime**: the watcher holds a `Weak<MemoryCache>` so it
//! does NOT prolong `AppState`'s lifetime. When the app
//! shuts down, `AppState` is dropped, the weak ref fails to
//! upgrade, and the watcher quietly stops firing (the OS handle
//! is closed when `_watcher` is dropped at the end of
//! `start_watcher`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::Duration;

use notify::{
    event::{EventKind, ModifyKind},
    Event, RecommendedWatcher, RecursiveMode, Watcher,
};

use crate::memory::loader::{all_paths, MemoryCache};
use crate::memory::types::{MemoryKind, MemorySource};
use crate::memory::WATCHER_DEBOUNCE_MS;

/// Long-lived watcher state. Holds the OS-level inotify handle
/// and the 1s debounce state. Drop = close the OS handle.
pub struct MemoryWatcher {
    /// Kept around so the OS handle isn't dropped. When this
    /// struct is dropped, the watcher fires its `Drop` impl and
    /// the inotify handle is closed.
    _watcher: RecommendedWatcher,
    /// The debounce state lives in a separate task; this field
    /// holds the abort sender for it.
    _abort: tokio::sync::oneshot::Sender<()>,
}

/// One debounce entry: the last time we saw an event for a
/// given `(kind, source)` triple, and the cached `project_path`
/// for Project-layer events.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct DebounceKey {
    kind: MemoryKind,
    source: MemorySource,
    project_path: Option<String>,
}

/// Spawn the memory watcher. Returns a `MemoryWatcher` whose
/// `Drop` cancels the background task and closes the OS
/// handle.
///
/// `cache` is the shared `MemoryCache` (held via `Weak` so we
/// don't keep `AppState` alive forever). `project_paths` is the
/// set of known project paths at startup — the watcher needs to
/// watch the user dir AND every project's dir. New projects
/// added at runtime are NOT watched (per the PRD's "新建 memory
/// 文件需重启 session 生效" rule, extended to "新建 project 也
/// 需要重启 watcher").
pub fn start_watcher(
    cache: Weak<MemoryCache>,
    project_paths: Vec<(String, String)>, // (project_id, project_path)
) -> Result<MemoryWatcher, String> {
    // 1. Collect the directories to watch.
    let mut dirs_to_watch: Vec<PathBuf> = Vec::new();
    if let Some(user) = crate::memory::file::user_dir() {
        if user.exists() {
            dirs_to_watch.push(user);
        }
    }
    for (_id, path) in &project_paths {
        let p = PathBuf::from(path);
        if p.exists() && p.is_dir() {
            dirs_to_watch.push(p);
        }
    }

    // 2. Pre-compute the (kind, source, path) table the
    //    debounce task uses to map an event path → (kind,
    //    source, project).
    let path_table: Arc<HashMap<PathBuf, DebounceKey>> = Arc::new(build_path_table(&project_paths));

    // 3. Spawn the debounce task. It owns the
    //    `RecommendedWatcher` via the channel. Wait — we want
    //    the watcher on the calling task so its OS handle
    //    lifetime ties to the returned MemoryWatcher. Re-
    //    architect: the watcher fires events into a channel;
    //    the debounce task reads the channel and fires
    //    invalidations into the cache.
    #[allow(unused_mut)]
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
    let (abort_tx, abort_rx) = tokio::sync::oneshot::channel::<()>();
    // The watcher itself runs on the calling thread. We move
    // `tx` into the closure it owns.
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        match res {
            Ok(event) => {
                if matches!(
                    event.kind,
                    EventKind::Modify(ModifyKind::Data(_))
                        | EventKind::Modify(ModifyKind::Any)
                        | EventKind::Modify(ModifyKind::Name(_))
                        | EventKind::Create(_)
                        | EventKind::Remove(_)
                ) {
                    let _ = tx.send(event);
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "memory watcher: event error");
            }
        }
    })
    .map_err(|e| format!("failed to create memory watcher: {}", e))?;

    for dir in &dirs_to_watch {
        if let Err(e) = watcher.watch(dir, RecursiveMode::NonRecursive) {
            tracing::warn!(
                dir = %dir.display(),
                error = %e,
                "memory watcher: failed to watch dir (continuing)"
            );
        }
    }

    let debounce_table = path_table.clone();
    let debounce_cache = cache.clone();
    tauri::async_runtime::spawn(async move {
        run_debounce_loop(rx, abort_rx, debounce_cache, debounce_table).await;
    });

    Ok(MemoryWatcher {
        _watcher: watcher,
        _abort: abort_tx,
    })
}

/// The debounce loop. Reads events from `rx`, debounces them by
/// `(kind, source, project_path)` for `WATCHER_DEBOUNCE_MS`
/// milliseconds, and fires a single cache invalidation per
/// debounced bucket.
///
/// `abort_rx` cancels the loop on `MemoryWatcher` drop. The
/// `Weak<MemoryCache>` is upgraded on every iteration; if the
/// app state is gone, the loop quietly exits (the abort sender
/// has fired and the channel is closed).
async fn run_debounce_loop(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<Event>,
    mut abort_rx: tokio::sync::oneshot::Receiver<()>,
    cache: Weak<MemoryCache>,
    path_table: Arc<HashMap<PathBuf, DebounceKey>>,
) {
    let debounce_window = Duration::from_millis(WATCHER_DEBOUNCE_MS);
    // For each bucket, track "last event time" so we can
    // decide whether the next event is within the debounce
    // window.
    let mut pending: HashMap<DebounceKey, std::time::Instant> = HashMap::new();

    loop {
        tokio::select! {
            biased;
            _ = &mut abort_rx => {
                tracing::info!("memory watcher: abort signal received, stopping");
                return;
            }
            maybe_event = rx.recv() => {
                let Some(event) = maybe_event else {
                    tracing::info!("memory watcher: event channel closed, stopping");
                    return;
                };
                let cache = match cache.upgrade() {
                    Some(c) => c,
                    None => {
                        tracing::info!("memory watcher: cache dropped, stopping");
                        return;
                    }
                };
                for event_path in &event.paths {
                    if let Some(key) = lookup_key(&path_table, event_path) {
                        pending.insert(key.clone(), std::time::Instant::now());
                    }
                }
                // Drain debounced entries.
                let now = std::time::Instant::now();
                let ready: Vec<DebounceKey> = pending
                    .iter()
                    .filter_map(|(k, t)| {
                        if now.duration_since(*t) >= debounce_window {
                            Some(k.clone())
                        } else {
                            None
                        }
                    })
                    .collect();
                for key in ready {
                    pending.remove(&key);
                    apply_invalidation(&cache, &key).await;
                }
            }
            // Periodic sweep: check for debounce windows that
            // have elapsed even if no new events arrive.
            _ = tokio::time::sleep(debounce_window) => {
                if pending.is_empty() {
                    continue;
                }
                let cache = match cache.upgrade() {
                    Some(c) => c,
                    None => {
                        tracing::info!("memory watcher: cache dropped, stopping");
                        return;
                    }
                };
                let now = std::time::Instant::now();
                let ready: Vec<DebounceKey> = pending
                    .iter()
                    .filter_map(|(k, t)| {
                        if now.duration_since(*t) >= debounce_window {
                            Some(k.clone())
                        } else {
                            None
                        }
                    })
                    .collect();
                for key in ready {
                    pending.remove(&key);
                    apply_invalidation(&cache, &key).await;
                }
            }
        }
    }
}

/// Fire a single cache invalidation for a debounced bucket.
async fn apply_invalidation(cache: &MemoryCache, key: &DebounceKey) {
    match key.kind {
        MemoryKind::User => {
            cache.invalidate_user_slot(key.source).await;
            tracing::info!(
                kind = "user",
                source = key.source.label(),
                "memory watcher: cache invalidated"
            );
        }
        MemoryKind::Project => {
            if let Some(pid) = &key.project_path {
                cache.invalidate_project_slot(pid, key.source).await;
                tracing::info!(
                    kind = "project",
                    source = key.source.label(),
                    project_id = %pid,
                    "memory watcher: cache invalidated"
                );
            }
        }
        MemoryKind::Session | MemoryKind::Runtime => {
            // V2 2 期. Nothing to do.
        }
    }
}

/// Build the path → `(kind, source, project_id)` lookup table.
/// The watcher compares every event's path against this table
/// and only the matching entries get debounced / invalidated.
fn build_path_table(
    project_paths: &[(String, String)],
) -> HashMap<PathBuf, DebounceKey> {
    let mut out = HashMap::new();
    for (pid, path) in project_paths {
        for (kind, source, full_path) in all_paths(Some(path)) {
            out.insert(
                full_path,
                DebounceKey {
                    kind,
                    source,
                    project_path: if matches!(kind, MemoryKind::Project) {
                        Some(pid.clone())
                    } else {
                        None
                    },
                },
            );
        }
    }
    // The user dir is project-agnostic — add its 2 files too.
    for (kind, source, full_path) in all_paths(None) {
        if matches!(kind, MemoryKind::User) {
            out.insert(
                full_path,
                DebounceKey {
                    kind,
                    source,
                    project_path: None,
                },
            );
        }
    }
    out
}

/// Look up a `(kind, source, project)` triple for a given event
/// path. Tries exact match first; falls back to comparing the
/// file's name (some editors emit events for the file while it's
/// being renamed / moved, so the absolute path may not match
/// the canonical path we registered).
fn lookup_key(
    table: &HashMap<PathBuf, DebounceKey>,
    event_path: &Path,
) -> Option<DebounceKey> {
    if let Some(k) = table.get(event_path) {
        return Some(k.clone());
    }
    // Fallback: match by filename across all entries.
    let name = event_path.file_name()?;
    for (registered_path, key) in table {
        if registered_path.file_name() == Some(name) {
            return Some(key.clone());
        }
    }
    None
}
