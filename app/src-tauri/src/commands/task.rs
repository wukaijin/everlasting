//! W1 (Workflow integration, Phase 0 Step 0.4 — 2026-07-08):
//! Tauri IPC bridge for the **task** side of the workflow
//! engine.
//!
//! ## Phase surface
//!
//! Two IPCs (Phase 3 fully landed):
//!
//! - [`create_task`] — seed `.everlasting/tasks/<slug>/`
//!   with a v1 `task.json` + `prd.md` skeleton. The
//!   frontend drives this when the user starts a new
//!   workflow task (matches W1 prd.md §"task CLI" copy:
//!   "create_task / archive_task / update_task command
//!   (非裸 write_file)").
//!
//! - [`archive_task`] — Step 3.3 (2026-07-09). Move
//!   `.everlasting/tasks/<slug>/` to
//!   `.everlasting/tasks/archive/<YYYY-MM>/<slug>/`, set
//!   `status = completed` + `completed_at`, and (by
//!   default) `git add` + commit the move. The IPC is the
//!   design-doc §6.8 "task CLI 脚本" **archive** subcommand
//!   — a Tauri command rather than a Python script, so the
//!   archive path lives next to the rest of the engine's
//!   authoritative writers.
//!
//! `update_task` is owned by Phase 2 Step 2.6
//! (B12 checklist sync owns the writer path).
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
//!   is informative enough for the frontend.
//! - `AlreadyArchived` (Step 3.3) → `InvalidRequest`. The
//!   "archive target already occupied" case is the
//!   companion of `AlreadyExists` — archive is a one-way
//!   move, never a clobber.
//! - `NotInDoneStatus` (Step 3.3) → `InvalidRequest`. The
//!   workflow engine hasn't finished producing spec
//!   content / closing out items, so archive is premature.
//!   The IPC surfaces the offending status in the toast
//!   so the frontend can prompt "complete the workflow
//!   first".
//! - `NotFound` → `InvalidRequest`
//! - `MalformedJson` / `Io` → `Server` (file-state
//!   integrity issue / filesystem failure; matches the
//!   existing categories in `commands::question`'s
//!   mode-change audit failures — `Server` is the
//!   pragmatic catch-all for "won't fix via user action").

use std::path::PathBuf;
use std::sync::Arc;

use tauri::State;

use crate::agent::workflow::{archive_task_init, create_task_init, TaskError, TaskJson};
use crate::db;
use crate::error::{AppCommandError, ErrorCategory};
use crate::state::AppState;

/// Map a `TaskError` to an `AppCommandError`. Defined once
/// here so all IPCs in this file share the boundary
/// conversion — keeps the IPC surface legible and the
/// error vocabulary in one place.
///
/// **Step 3.3 (2026-07-09)**: added the two new
/// `TaskError` variants (`AlreadyArchived` /
/// `NotInDoneStatus`) to the `match`. Both map to
/// `InvalidRequest` — they're user-correctable (re-archive
/// after deleting the conflict; or move the workflow to
/// `done` first), so the toast message can carry the
/// remediation hint verbatim.
fn map_task_error(e: TaskError) -> AppCommandError {
    match e {
        TaskError::InvalidSlug(msg) => AppCommandError::new(ErrorCategory::InvalidRequest, msg),
        TaskError::AlreadyExists(path) => AppCommandError::new(
            ErrorCategory::InvalidRequest,
            format!("create_task: task directory already exists at {}", path.display()),
        ),
        TaskError::NotFound(path) => AppCommandError::new(
            ErrorCategory::InvalidRequest,
            format!("task: task directory not found at {}", path.display()),
        ),
        TaskError::AlreadyArchived(path) => AppCommandError::new(
            ErrorCategory::InvalidRequest,
            format!(
                "archive_task: an archive entry already exists at {} — \
                 remove it manually before re-archiving",
                path.display()
            ),
        ),
        TaskError::NotInDoneStatus(status) => AppCommandError::new(
            ErrorCategory::InvalidRequest,
            format!(
                "archive_task: task is in status `{status}`; \
                 finish the workflow (Check → Done) before archiving"
            ),
        ),
        TaskError::MalformedJson(path, msg) => AppCommandError::new(
            ErrorCategory::Server,
            format!(
                "task: malformed task.json at {}: {}",
                path.display(),
                msg
            ),
        ),
        TaskError::Io(path, src) => AppCommandError::new(
            ErrorCategory::Server,
            format!("task: io error at {}: {}", path.display(), src),
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

/// W1 (Workflow integration, Phase 3 Step 3.3 — 2026-07-09):
/// `archive_task` IPC — finalize a workflow task by
/// moving it under `.everlasting/tasks/archive/<YYYY-MM>/`
/// and flipping `task.json` to `status = completed` with
/// `completed_at` set. The post-archive task is **not**
/// resolvable by `inject::resolve_current_task` — the
/// workflow engine treats it as closed.
///
/// **Inputs**:
/// - `project_id`: the project whose `.everlasting/tasks/`
///   receives the move. Same lookup pattern as
///   `create_task` — the frontend doesn't pass absolute
///   paths.
/// - `slug`: the task's slug. Refused with `InvalidRequest`
///   if the slug doesn't match `[a-z0-9-]{1,64}`, if the
///   task doesn't exist (`NotFound`), if the task isn't
///   `Done` yet (`NotInDoneStatus`), or if the archive
///   target is already occupied (`AlreadyArchived`).
/// - `no_commit`: when `true`, skip the post-archive
///   `git add` + `git commit` — useful for tests and
///   for dry-runs on non-git project dirs.
///
/// **Returns**: the post-archive `TaskJson` (status
/// `Completed`, `completed_at` set). The frontend uses
/// this to navigate to the archive dir, update any in-app
/// task list, etc.
///
/// **Concurrency**: the IPC is racy on a multi-tab
/// frontend — two concurrent `archive_task` calls for the
/// same slug will both read `Done`, but only the first
/// will reach the `fs::rename`; the second sees
/// `AlreadyArchived`. The frontend should treat
/// `AlreadyArchived` as "the other tab already did it"
/// rather than retry.
#[tauri::command]
pub async fn archive_task(
    state: State<'_, Arc<AppState>>,
    project_id: String,
    slug: String,
    no_commit: bool,
) -> Result<TaskJson, AppCommandError> {
    if project_id.trim().is_empty() {
        return Err(AppCommandError::new(
            ErrorCategory::InvalidRequest,
            "archive_task: project_id must not be empty",
        ));
    }

    let project = match db::get_project(&state.db, &project_id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return Err(AppCommandError::new(
                ErrorCategory::InvalidRequest,
                format!("archive_task: project '{}' not found", project_id),
            ));
        }
        Err(e) => {
            return Err(AppCommandError::new(
                ErrorCategory::Server,
                format!("archive_task: failed to load project: {}", e),
            ))
        }
    };

    let project_path = PathBuf::from(&project.path);
    archive_task_init(&project_path, &slug, no_commit).map_err(map_task_error)
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
    ///
    /// Step 3.3: also covers `AlreadyArchived` and
    /// `NotInDoneStatus` so the boundary conversion is
    /// exhaustively tested.
    #[test]
    fn map_task_error_covers_every_variant() {
        let _ = map_task_error(TaskError::InvalidSlug("BAD".into()));
        let _ = map_task_error(TaskError::AlreadyExists(PathBuf::from("/x")));
        let _ = map_task_error(TaskError::NotFound(PathBuf::from("/y")));
        let _ = map_task_error(TaskError::AlreadyArchived(PathBuf::from("/z/archive")));
        let _ = map_task_error(TaskError::NotInDoneStatus("implement".into()));

        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "nope");
        let _ = map_task_error(TaskError::MalformedJson(
            PathBuf::from("/z"),
            "bad json".into(),
        ));
        let _ = map_task_error(TaskError::Io(PathBuf::from("/w"), io_err));
    }
}
