//! Tauri command surface for the B5 memory preview.
//!
//! Three commands, registered in [`crate::lib::run`]:
//!
//! - [`read_memory_layers`] — returns a `Vec<MemoryLayerInfo>`
//!   (the lightweight DTO) for the current session's project
//!   (User layer + Project layer). The frontend renders the
//!   preview panel from this list; the file body is fetched on
//!   demand via [`read_memory_content`].
//! - [`read_memory_content`] — returns the raw UTF-8 body of a
//!   single memory file at the given path. The frontend
//!   passes back a path it got from `read_memory_layers`.
//! - [`open_memory_in_editor`] — spawns the user's `$EDITOR`
//!   (or falls back to `xdg-open` on Linux / `open` on macOS /
//!   `cmd /c start` on Windows) with the memory file's path.
//!   Best-effort: failures are logged, not surfaced to the
//!   IPC.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use tauri::State;

use crate::db;
use crate::db::memories::list_memories as db_list_memories;
use crate::memory::loader::{
    all_paths, load_for_session, resolve_one, MemoryCache,
};
use crate::memory::types::MemoryLayerInfo;
use crate::state::AppState;

/// Read the per-session memory layer summary (User + Project
/// layers). The frontend calls this on memory-preview panel
/// mount. Freshness is handled by the mtime fence in
/// `load_for_session` — every call re-checks each file's `mtime`.
#[tauri::command]
pub async fn read_memory_layers(
    state: State<'_, Arc<AppState>>,
    project_id: String,
) -> Result<Vec<MemoryLayerInfo>, String> {
    let project = match db::get_project(&state.db, &project_id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return Err(format!("read_memory_layers: project '{}' not found", project_id));
        }
        Err(e) => return Err(format!("read_memory_layers: failed to load project: {}", e)),
    };

    let layers = load_for_session(&state.memory_cache, &project_id, &project.path).await;
    Ok(layers.iter().map(MemoryLayerInfo::from).collect())
}

/// Read the body of a single memory file. `path` must be one of
/// the 4 fixed file paths the loader knows about (i.e. it was
/// returned by `read_memory_layers`). Arbitrary paths are
/// rejected — this is a security boundary: the IPC must not
/// leak arbitrary file content to the frontend.
///
/// For the user layer, `project_id` is irrelevant; for the
/// project layer, the resolved path must match the project's
/// `CLAUDE.md` / `AGENTS.md`. We resolve via `resolve_one`
/// which uses the project path as the source of truth.
#[tauri::command]
pub async fn read_memory_content(
    state: State<'_, Arc<AppState>>,
    project_id: String,
    path: String,
) -> Result<String, String> {
    let project = match db::get_project(&state.db, &project_id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return Err(format!("read_memory_content: project '{}' not found", project_id));
        }
        Err(e) => return Err(format!("read_memory_content: failed to load project: {}", e)),
    };

    let requested = PathBuf::from(&path);
    // Match the requested path against the 4 known paths.
    let known = all_paths(Some(&project.path));
    let canonical = requested
        .canonicalize()
        .unwrap_or_else(|_| requested.clone());
    let known_match = known
        .iter()
        .find(|(_, _, k)| k == &canonical || k == &requested);

    let Some((kind, source, _)) = known_match else {
        return Err(format!(
            "read_memory_content: path '{}' is not a known memory file",
            path
        ));
    };

    let resolved = match resolve_one(*kind, *source, Some(&project.path)) {
        Some(p) => p,
        None => return Err("read_memory_content: cannot resolve path".to_string()),
    };

    std::fs::read_to_string(&resolved).map_err(|e| {
        format!(
            "read_memory_content: failed to read {}: {}",
            resolved.display(),
            e
        )
    })
}

/// Spawn the user's editor to edit a memory file. The
/// resolution chain is:
/// 1. `$VISUAL` (preferred — vim, emacs, vscode, etc.)
/// 2. `$EDITOR` (fallback)
/// 3. `xdg-open` on Linux / `open` on macOS / `cmd /c start` on
///    Windows (last-ditch: the OS picks the default app)
///
/// The command is best-effort: we don't wait for the editor
/// to exit (`Command::spawn` not `Command::output`).
///
/// As with `read_memory_content`, the path must match one of
/// the 4 known memory files. We refuse to open arbitrary
/// paths.
#[tauri::command]
pub async fn open_memory_in_editor(
    state: State<'_, Arc<AppState>>,
    project_id: String,
    path: String,
) -> Result<(), String> {
    let project = match db::get_project(&state.db, &project_id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return Err(format!(
                "open_memory_in_editor: project '{}' not found",
                project_id
            ));
        }
        Err(e) => {
            return Err(format!(
                "open_memory_in_editor: failed to load project: {}",
                e
            ));
        }
    };

    let requested = PathBuf::from(&path);
    let known = all_paths(Some(&project.path));
    let canonical = requested
        .canonicalize()
        .unwrap_or_else(|_| requested.clone());
    let known_match = known
        .iter()
        .find(|(_, _, k)| k == &canonical || k == &requested);
    let Some((kind, source, _)) = known_match else {
        return Err(format!(
            "open_memory_in_editor: path '{}' is not a known memory file",
            path
        ));
    };

    let resolved = match resolve_one(*kind, *source, Some(&project.path)) {
        Some(p) => p,
        None => return Err("open_memory_in_editor: cannot resolve path".to_string()),
    };

    // Try $VISUAL → $EDITOR → OS default.
    let editor = std::env::var("VISUAL")
        .ok()
        .or_else(|| std::env::var("EDITOR").ok());

    let result: Result<std::process::Child, String> = if let Some(editor) = editor {
        // $EDITOR is a single command (possibly with args
        // like "code --wait"). Split on whitespace for the
        // simple case; full shell-quoting is out of scope
        // for V2 1 期.
        let mut parts = editor.split_whitespace();
        let cmd = match parts.next() {
            Some(c) => c,
            None => return Err("open_memory_in_editor: empty $EDITOR".to_string()),
        };
        let args: Vec<&str> = parts.collect();
        Command::new(cmd)
            .args(&args)
            .arg(&resolved)
            .spawn()
            .map_err(|e| {
                format!(
                    "open_memory_in_editor: failed to spawn editor '{}': {}",
                    editor, e
                )
            })
    } else {
        // No $EDITOR — use the OS default opener as a fallback.
        fallback_open(&resolved)
            .map_err(|e| format!("open_memory_in_editor: fallback open failed: {}", e))
    };

    match result {
        Ok(_) => Ok(()),
        Err(e) => {
            tracing::warn!(error = %e, path = %resolved.display(), "open_memory_in_editor failed");
            Err(e)
        }
    }
}

/// Best-effort OS-level "open with default app" command. We
/// never block on this — the spawn is fire-and-forget.
#[cfg(target_os = "linux")]
fn fallback_open(path: &std::path::Path) -> std::io::Result<std::process::Child> {
    Command::new("xdg-open").arg(path).spawn()
}

#[cfg(target_os = "macos")]
fn fallback_open(path: &std::path::Path) -> std::io::Result<std::process::Child> {
    Command::new("open").arg(path).spawn()
}

#[cfg(target_os = "windows")]
fn fallback_open(path: &std::path::Path) -> std::io::Result<std::process::Child> {
    Command::new("cmd").args(["/c", "start", "", &path.to_string_lossy()]).spawn()
}

// Re-export for unit tests.
#[allow(dead_code)]
pub(crate) fn _ensure_used_for_test(_c: &MemoryCache) {}

// ---------------------------------------------------------------------------
// P2 (2026-06-29): runtime autonomous-memory CRUD commands.
//
// `list_memories` / `delete_memory` back the MemoryPreview panel's
// "runtime memories" section. The list is project-isolated: a
// project-scoped memory in proj-A is NEVER surfaced when the panel
// is open for proj-B. User-scope memories are global (they surface
// for every project's panel — that's the "user" layer's contract).
//
// Permission: these are user-driven direct IPCs (the panel is the
// UI), NOT LLM tool invocations. The ⑨ 关 permission layer does
// NOT apply (same precedent as the B5 memory-file commands above
// + the D3 edit/resend commands). The project-isolation check
// below is the security boundary.
// ---------------------------------------------------------------------------

/// List runtime memories (P2 autonomous memories) visible to the
/// given project. Returns user-scope memories (global) + the
/// project's own project-scope memories, newest first. Used by
/// the MemoryPreview panel's "runtime memories" list.
///
/// Project isolation: a project-scope memory in another project
/// is NEVER returned. The user-scope memories are global by
/// design (cross-project experience).
#[tauri::command]
pub async fn list_autonomous_memories(
    state: State<'_, Arc<AppState>>,
    project_id: String,
) -> Result<Vec<crate::db::memories::MemoryRow>, String> {
    // Verify the project exists (defensive — the IPC is
    // user-driven from the panel, but a stale project_id should
    // surface a clean error rather than an empty list).
    match db::get_project(&state.db, &project_id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return Err(format!(
                "list_autonomous_memories: project '{}' not found",
                project_id
            ));
        }
        Err(e) => {
            return Err(format!(
                "list_autonomous_memories: failed to load project: {}",
                e
            ));
        }
    }
    // scope=None → both layers. The DB layer ignores project_id
    // for user-scope rows (they're global) and filters project-
    // scope rows by the supplied id. This is exactly the
    // project-isolation contract.
    match db_list_memories(&state.db, None, Some(&project_id)).await {
        Ok(rows) => Ok(rows),
        Err(e) => Err(format!(
            "list_autonomous_memories: query failed: {}",
            e
        )),
    }
}

/// Delete a runtime memory by its `memory_id` UUID. Best-effort
/// idempotent: deleting an already-deleted memory returns Ok(0).
///
/// **Project isolation**: the command does NOT take a project_id
/// — the `memory_id` is globally unique (UUID v7), so there's no
/// cross-project leak risk from the delete itself. The
/// MemoryPreview panel only displays memories already filtered
/// to the current project (via `list_autonomous_memories`), so
/// the user can only see + click delete on memories they're
/// allowed to manage.
#[tauri::command]
pub async fn delete_autonomous_memory(
    state: State<'_, Arc<AppState>>,
    memory_id: String,
) -> Result<u64, String> {
    match crate::db::memories::delete_memory(&state.db, &memory_id).await {
        Ok(n) => Ok(n),
        Err(e) => Err(format!(
            "delete_autonomous_memory: delete failed: {}",
            e
        )),
    }
}
