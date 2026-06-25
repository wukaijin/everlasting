//! B2 @文件补全 — Tauri command surface.
//!
//! Thin IPC over [`crate::files::walk_files`]. The frontend's
//! `<TriggerMenu>` calls `list_files` when the user types `@` to
//! populate the file-completion panel (root-relative forward-slash
//! paths). The walk is synchronous + git2-based, so it runs on
//! `spawn_blocking` to avoid blocking the async runtime on std::fs +
//! libgit2.
//!
//! `max_depth = None` returns the full bounded walk (legacy default,
//! still used for the `@/`-prefixed full-project search). `Some(n)`
//! caps depth to `n` layers under the project root — the default `@`
//! trigger sends `Some(3)` so the panel opens instantly even on huge
//! repos.

use std::path::PathBuf;
use std::sync::Arc;

use tauri::State;

use crate::state::AppState;

/// List files under the current project root as root-relative
/// forward-slash paths, for the `@`-mention completion panel.
///
/// * `project_id` selects the project; the project's `path` is the walk
///   root.
/// * `max_depth = None` walks with the default cap
///   ([`crate::files::MAX_DEPTH`]); `Some(n)` caps at `n` layers under
///   the root (use a small value for the default `@` trigger).
///
/// Returns an empty vec when there is no project / the project has no
/// path / the walk fails — the frontend renders an empty panel
/// ("无匹配文件") rather than surfacing an error.
#[tauri::command]
pub async fn list_files(
    state: State<'_, Arc<AppState>>,
    project_id: Option<String>,
    max_depth: Option<u32>,
) -> Result<Vec<String>, String> {
    let project_path = match project_id {
        Some(pid) => crate::db::get_project(&state.db, &pid)
            .await
            .map_err(|e| format!("list_files: get_project failed: {}", e))?
            .map(|p| p.path),
        None => None,
    };
    let Some(path) = project_path else {
        return Ok(Vec::new());
    };
    let root = PathBuf::from(path);
    let depth = max_depth.map(|d| d as usize);
    // std::fs + git2 are blocking → offload onto the blocking pool.
    let paths = tokio::task::spawn_blocking(move || match depth {
        Some(d) => crate::files::walk_files_with_depth(&root, d),
        None => crate::files::walk_files(&root),
    })
    .await
    .map_err(|e| format!("list_files: walk join failed: {}", e))?;
    Ok(paths)
}

/// List files under an arbitrary absolute `root` for the `@/`-prefixed
/// system-root mention panel. Returns root-relative forward-slash
/// paths (so a file at `/etc/hosts` comes back as `etc/hosts`).
///
/// `root` MUST be absolute and MUST live under `/`. Non-`/` roots
/// are rejected — `@/foo` is reserved for the filesystem root view;
/// project-relative paths go through the project-aware `list_files`.
/// The walk uses the wider [`crate::files::SYSTEM_EXCLUDE`] set so
/// `/proc`, `/sys`, `/dev`, etc. never get visited (would either
/// hang on virtual fs or pollute the picker with device nodes).
///
/// `max_depth = None` uses [`crate::files::MAX_DEPTH`]; `Some(n)` caps
/// the walk at `n` layers. We strongly recommend `Some(<= 4)` —
/// `/usr/share/*` alone has tens of thousands of files.
#[tauri::command]
pub async fn list_files_at(
    root: String,
    max_depth: Option<u32>,
) -> Result<Vec<String>, String> {
    let root_path = PathBuf::from(&root);
    if !root_path.is_absolute() {
        return Err(format!(
            "list_files_at: root must be absolute, got {:?}",
            root_path
        ));
    }
    // Only the literal filesystem root is allowed. We don't accept
    // arbitrary subdirs of `/` here — that would let a chat turn
    // walk `/home/...` or other users' homes. If the need arises,
    // add a separate `list_files_under` with explicit boundary checks.
    if root_path != PathBuf::from("/") {
        return Err(format!(
            "list_files_at: root must be `/`, got {:?} (use list_files for project paths)",
            root_path
        ));
    }
    let depth = max_depth.map(|d| d as usize);
    let paths = tokio::task::spawn_blocking(move || match depth {
        Some(d) => crate::files::walk_system(&root_path, d),
        None => crate::files::walk_system(&root_path, crate::files::MAX_DEPTH),
    })
    .await
    .map_err(|e| format!("list_files_at: walk join failed: {}", e))?;
    Ok(paths)
}
