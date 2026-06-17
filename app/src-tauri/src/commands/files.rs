//! B2 @文件补全 — Tauri command surface.
//!
//! Thin IPC over [`crate::files::walk_files`]. The frontend's
//! `<TriggerMenu>` calls `list_files` when the user types `@` to
//! populate the file-completion panel (root-relative forward-slash
//! paths). The walk is synchronous + git2-based, so it runs on
//! `spawn_blocking` to avoid blocking the async runtime on std::fs +
//! libgit2.

use std::path::PathBuf;
use std::sync::Arc;

use tauri::State;

use crate::state::AppState;

/// List files under the current project root as root-relative
/// forward-slash paths, for the `@`-mention completion panel.
///
/// `project_id` selects the project; the project's `path` is the walk
/// root. Returns an empty vec when there is no project / the project
/// has no path / the walk fails — the frontend renders an empty panel
/// ("无匹配文件") rather than surfacing an error.
#[tauri::command]
pub async fn list_files(
    state: State<'_, Arc<AppState>>,
    project_id: Option<String>,
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
    // std::fs + git2 are blocking → offload onto the blocking pool.
    let paths = tokio::task::spawn_blocking(move || crate::files::walk_files(&root))
        .await
        .map_err(|e| format!("list_files: walk join failed: {}", e))?;
    Ok(paths)
}
