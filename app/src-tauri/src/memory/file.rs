//! Memory file path resolution + reading.
//!
//! V2 1 期 hard-codes 4 fixed paths (2 layers × 2 sources):
//! - `~/.config/everlasting/CLAUDE.md`  (User layer)
//! - `~/.config/everlasting/AGENTS.md`  (User layer)
//! - `<project.path>/CLAUDE.md`          (Project layer)
//! - `<project.path>/AGENTS.md`          (Project layer)
//!
//! The User directory uses `dirs::config_dir()` so it follows the
//! platform convention (`~/.config/` on Linux,
//! `~/Library/Application Support/` on macOS,
//! `%APPDIR%` on Windows). On Linux (the project's primary dev
//! platform) this resolves to `~/.config/everlasting/`, matching
//! the PRD's locked-in choice (2026-06-10 grill decision #2).
//!
//! Project paths are the raw `projects.path` column from SQLite —
//! the agent loop has already validated the path through
//! `projects::boundary::assert_within_root` on session load, so we
//! trust the caller.

use std::path::{Path, PathBuf};

use crate::memory::types::{LayerStatus, MemoryKind, MemorySource, MemoryLayer};
use crate::memory::MAX_FILE_SIZE;

use super::tokens::count_tokens;

// Test-only: thread-local override for the user-layer
// directory. When `Some(p)`, `user_dir()` returns
// `Some(p.clone())` instead of `dirs::config_dir()`. When
// `None`, falls back to the real platform path. The previous
// value is returned so the guard can restore on drop.
#[cfg(test)]
thread_local! {
    static USER_DIR_OVERRIDE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub fn set_user_dir_for_test(path: Option<PathBuf>) -> Option<PathBuf> {
    USER_DIR_OVERRIDE.with(|cell| std::mem::replace(&mut *cell.borrow_mut(), path))
}

/// Resolve the User-layer directory (`~/.config/everlasting/`).
///
/// Returns `None` if `dirs::config_dir()` is unavailable (rare —
/// only on platforms where the XDG / equivalent env var is unset
/// and there's no fallback). The caller treats `None` as a
/// per-file "user dir unreachable" error.
///
/// We always append a hard-coded `everlasting` subdirectory rather
/// than dumping memory files into the user's bare config dir
/// (which would conflict with other tools' configs).
///
/// In test builds, the path is taken from the thread-local
/// override (see `set_user_dir_for_test`) so tests can run
/// hermetically without touching the developer's real
/// `~/.config/everlasting/`.
pub fn user_dir() -> Option<PathBuf> {
    #[cfg(test)]
    {
        if let Some(override_path) = USER_DIR_OVERRIDE.with(|cell| cell.borrow().clone()) {
            return Some(override_path);
        }
    }
    dirs::config_dir().map(|p| p.join("everlasting"))
}

/// Resolve the absolute path of a single memory file.
///
/// `project_path` is `None` for `MemoryKind::User` (the user layer
/// is global). For `Project`, the caller passes the project's
/// `path` column from the `projects` table.
pub fn resolve_path(
    kind: MemoryKind,
    source: MemorySource,
    project_path: Option<&str>,
) -> Option<PathBuf> {
    match kind {
        MemoryKind::User => Some(user_dir()?.join(source.filename())),
        MemoryKind::Project => {
            let p = project_path?;
            Some(PathBuf::from(p).join(source.filename()))
        }
        // Session / Runtime are V2 2 期. Return `None` to keep
        // the type signature forward-compat without inventing a
        // path that has no backing storage.
        MemoryKind::Session | MemoryKind::Runtime => None,
    }
}

/// Read a single memory file at a known path. **Prefer
/// [`load_layer`]** for the public path — it handles the
/// kind/source envelope correctly. `load_file` is a thin
/// convenience that returns a "shell" `MemoryLayer` with
/// `kind = User`, `source = Claude` placeholders. It exists
/// for direct-path callers in the test suite.
///
/// Failure modes are identical to `load_file_inner` (see
/// below). All errors are absorbed into `LayerStatus`.
#[allow(dead_code)]
pub async fn load_file(path: &Path) -> MemoryLayer {
    let (content, tokens, status) = load_file_inner(path).await;
    // The kind/source are placeholders — the caller should
    // use `load_layer` instead. We pick `User / Claude` as
    // the most common case.
    MemoryLayer {
        kind: MemoryKind::User,
        source: MemorySource::Claude,
        path: path.to_path_buf(),
        content,
        tokens,
        status,
    }
}

/// Internal helper that reads the file body and returns
/// `(content, tokens, status)`. Public callers go through
/// `load_layer` so the `MemoryLayer` envelope (with kind +
/// source) is constructed correctly.
async fn load_file_inner(path: &Path) -> (String, u32, LayerStatus) {
    // 1. Existence check (fast-path for the "file not there"
    //    case so we don't pay the metadata + read syscalls).
    if !path.exists() {
        return (String::new(), 0, LayerStatus::Missing);
    }

    // 2. Metadata: size cap.
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "memory: metadata failed");
            return (
                String::new(),
                0,
                LayerStatus::Error {
                    reason: format!("metadata failed: {}", e),
                },
            );
        }
    };
    if meta.len() > MAX_FILE_SIZE {
        tracing::warn!(
            path = %path.display(),
            size = meta.len(),
            max = MAX_FILE_SIZE,
            "memory: file exceeds size cap, skipping"
        );
        return (
            String::new(),
            0,
            LayerStatus::Error {
                reason: format!(
                    "file is {} bytes, exceeds {} byte cap",
                    meta.len(),
                    MAX_FILE_SIZE
                ),
            },
        );
    }

    // 3. Read body. `read_to_string` rejects non-UTF-8; we do
    //    NOT lossy-convert (silently mangling CJK would corrupt
    //    the prompt and confuse the model).
    let body = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
            tracing::warn!(path = %path.display(), "memory: file is not valid UTF-8");
            return (
                String::new(),
                0,
                LayerStatus::Error {
                    reason: "file is not valid UTF-8".to_string(),
                },
            );
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "memory: read failed");
            return (
                String::new(),
                0,
                LayerStatus::Error {
                    reason: format!("read failed: {}", e),
                },
            );
        }
    };

    // 4. Token count. Empty bodies are valid and yield 0.
    let tokens = count_tokens(&body).await;
    (body, tokens, LayerStatus::Loaded)
}

/// Test-only: re-export of the private `load_file_inner` for
/// the test suite. Returns `(content, tokens, status)` for
/// ergonomic test assertions.
#[cfg(test)]
#[allow(dead_code)]
pub async fn load_file_inner_for_test(path: &Path) -> (String, u32, LayerStatus) {
    load_file_inner(path).await
}

/// Convenience: build a `MemoryLayer` envelope for the given
/// kind/source/project_path. Wraps `load_file_inner` with the
/// path resolution.
pub async fn load_layer(
    kind: MemoryKind,
    source: MemorySource,
    project_path: Option<&str>,
) -> MemoryLayer {
    let path = match resolve_path(kind, source, project_path) {
        Some(p) => p,
        // User dir unresolvable (no `config_dir()`), or
        // Session/Runtime variant.
        None => {
            let reason = if matches!(kind, MemoryKind::User) {
                "user config dir is not resolvable on this platform"
            } else {
                "session / runtime memory is not implemented in V2 1 期"
            };
            tracing::warn!(kind = ?kind, "memory: cannot resolve path");
            return MemoryLayer {
                kind,
                source,
                path: PathBuf::new(),
                content: String::new(),
                tokens: 0,
                status: LayerStatus::Error {
                    reason: reason.to_string(),
                },
            };
        }
    };

    let (content, tokens, status) = load_file_inner(&path).await;
    MemoryLayer {
        kind,
        source,
        path,
        content,
        tokens,
        status,
    }
}
