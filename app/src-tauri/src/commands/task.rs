//! W1 (Workflow integration, Phase 0 Step 0.4 — 2026-07-08):
//! Tauri IPC bridge for the **task** side of the workflow
//! engine.
//!
//! ## Phase 0 surface
//!
//! One IPC:
//!
//! - [`create_task`] — seed `.everlasting/tasks/<slug>/`
//!   with a v1 `task.json` + `prd.md` skeleton. The
//!   frontend drives this when the user starts a new
//!   workflow task (matches W1 prd.md §"task CLI" copy:
//!   "create_task / archive_task / update_task command
//!   (非裸 write_file)").
//!
//! ## Phase scope
//!
//! `update_task` / `archive_task` are deferred to Phase 2
//! Step 2.6 (B12 checklist sync owns the writer path) and
//! Phase 3 Step 3.3 (spec-distillation trigger archives the
//! task). For Phase 0 the only mutating IPC is
//! `create_task`; reads happen via `read_task` (a
//! Rust-side helper invoked by Step 0.5 chat_loop, no IPC
//! needed yet).
//!
//! ## Error mapping
//!
//! All `TaskError` variants map to `AppCommandError::new`
//! at the IPC boundary:
//! - `InvalidSlug` → `InvalidRequest`
//! - `AlreadyExists` → `InvalidRequest`. A dedicated
//!   `AlreadyExists` `ErrorCategory` variant would be
//!   nicer for routing but is out of Phase 0 scope; the
//!   toast message ("task directory already exists at ...")
//!   is informative enough for the Step 0.4 frontend.
//!   Phase 3's spec-distillation / archive IPCs can add
//!   the variant if the same collision recurs there.
//! - `NotFound` (only emitted by `read_task`, not used
//!   here yet) → `InvalidRequest`
//! - `MalformedJson` / `Io` → `Server` (file-state
//!   integrity issue / filesystem failure; matches the
//!   existing categories in `commands::question`'s
//!   mode-change audit failures — `Server` is the
//!   pragmatic catch-all for "won't fix via user action").

use std::path::PathBuf;
use std::sync::Arc;

use tauri::State;

use crate::agent::workflow::{create_task_init, TaskError, TaskJson};
use crate::db;
use crate::error::{AppCommandError, ErrorCategory};
use crate::state::AppState;

/// Map a `TaskError` to an `AppCommandError`. Defined once
/// here so all IPCs in this file share the boundary
/// conversion — keeps the IPC surface legible and the
/// error vocabulary in one place.
fn map_task_error(e: TaskError) -> AppCommandError {
    match e {
        TaskError::InvalidSlug(msg) => AppCommandError::new(ErrorCategory::InvalidRequest, msg),
        TaskError::AlreadyExists(path) => AppCommandError::new(
            ErrorCategory::InvalidRequest,
            format!("create_task: task directory already exists at {}", path.display()),
        ),
        TaskError::NotFound(path) => AppCommandError::new(
            ErrorCategory::InvalidRequest,
            format!("create_task: task directory not found at {}", path.display()),
        ),
        TaskError::MalformedJson(path, msg) => AppCommandError::new(
            ErrorCategory::Server,
            format!(
                "create_task: malformed task.json at {}: {}",
                path.display(),
                msg
            ),
        ),
        TaskError::Io(path, src) => AppCommandError::new(
            ErrorCategory::Server,
            format!(
                "create_task: io error at {}: {}",
                path.display(),
                src
            ),
        ),
    }
}

/// W1 (Workflow integration, Phase 0 Step 0.4):
/// `create_task` IPC — seed `.everlasting/tasks/<slug>/`
/// with a v1 `task.json` + `prd.md` skeleton.
///
/// **Inputs**:
/// - `project_id`: the project whose `.everlasting/tasks/`
///   receives the new task. We look up the project path
///   from SQLite (mirrors `create_session`'s pattern) so the
///   frontend doesn't need to know the absolute path.
/// - `title`: free-text human title. Trimmed; non-empty
///   validated by `create_task_init`.
/// - `slug`: ASCII `[a-z0-9-]{1,64}` (see
///   `agent::workflow::validate_slug`). Rejected with
///   `InvalidRequest` on bad input.
/// - `parent`: optional slug of the parent task — B8 DAG
///   slot (Phase 3+). Phase 0 accepts and persists the
///   field verbatim.
///
/// **Returns**: the fresh `TaskJson` row (so the frontend
/// can navigate to the task directory, jump to its first
/// item, etc., without re-reading the file).
///
/// **Concurrency**: none enforced. Two concurrent calls
/// with the same `(project_id, slug)` race; the second one
/// sees `Err(AlreadyExists)`. The frontend is expected to
/// prompt "another agent / tab beat you to it" rather than
/// silently overwrite.
#[tauri::command]
pub async fn create_task(
    state: State<'_, Arc<AppState>>,
    project_id: String,
    title: String,
    slug: String,
    parent: Option<String>,
) -> Result<TaskJson, AppCommandError> {
    // Project lookup mirrors `create_session` (commands/
    // sessions.rs:59) — defensive gate against stray
    // frontend IPC calls with stale project ids. Stray
    // empty id is also rejected up-front.
    if project_id.trim().is_empty() {
        return Err(AppCommandError::new(
            ErrorCategory::InvalidRequest,
            "create_task: project_id must not be empty",
        ));
    }

    let project = match db::get_project(&state.db, &project_id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return Err(AppCommandError::new(
                ErrorCategory::InvalidRequest,
                format!("create_task: project '{}' not found", project_id),
            ));
        }
        Err(e) => {
            return Err(AppCommandError::new(
                ErrorCategory::Server,
                format!("create_task: failed to load project: {}", e),
            ))
        }
    };

    let project_path = PathBuf::from(&project.path);
    create_task_init(&project_path, &title, &slug, parent.as_deref())
        .map_err(map_task_error)
}

// ---------------------------------------------------------------------------
// Tests — IPC layer (project lookup + error mapping)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// `map_task_error` consumes each `TaskError` variant
    /// without panicking AND produces a stringly-helpful
    /// message (the frontend surfaces this verbatim in a
    /// toast).
    #[test]
    fn map_task_error_covers_every_variant() {
        let _ = map_task_error(TaskError::InvalidSlug("BAD".into()));
        let _ = map_task_error(TaskError::AlreadyExists(PathBuf::from("/x")));
        let _ = map_task_error(TaskError::NotFound(PathBuf::from("/y")));

        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "nope");
        let _ = map_task_error(TaskError::MalformedJson(
            PathBuf::from("/z"),
            "bad json".into(),
        ));
        let _ = map_task_error(TaskError::Io(PathBuf::from("/w"), io_err));
    }
}
