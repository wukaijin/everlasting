//! Task archive subsystem: move a completed task into the archive tree.
//!
//! Split out of `agent/workflow/task.rs` (2026-08-08 batch3).

use std::fs;
use std::io;
use std::path::Path;

use chrono::Utc;

use super::io::read_task;
use super::paths::{task_dir, validate_slug};
use super::types::{TaskError, TaskJson, TaskResult, TaskStatus};

/// Path component used for the archive sub-tree under
/// `.everlasting/tasks/`. Re-exported (not just `const`)
/// because the IPC layer (`commands::task::archive_task`)
/// and tests both reference it; centralizing here keeps
/// the archive layout single-sourced.
pub const PROJ_NS_TASKS_ARCHIVE_DIR: &str = "archive";

/// `archive_task_init` body — moves
/// `<project>/.everlasting/tasks/<slug>/` →
/// `<project>/.everlasting/tasks/archive/<YYYY-MM>/<slug>/`
/// and flips `task.json` to `status = Completed` with a
/// `completed_at` timestamp. Called by the Tauri IPC
/// (`commands::task::archive_task`).
///
/// **Preconditions** (returns `Err` on failure):
/// - `slug` validates as `[a-z0-9-]{1,64}` (`InvalidSlug`).
/// - `<slug>/task.json` exists and parses (`NotFound` /
///   `MalformedJson`).
/// - `status == Done` (`NotInDoneStatus`). Archive must
///   follow the workflow's `in_progress → done` hook (Step 3.1)
///   so the spec-distillation hint + items close-out have
///   already happened; archiving a planning / in_progress
///   task would orphan in-flight work.
/// - archive target dir does **not** already exist
///   (`AlreadyArchived`). This makes the call idempotent
///   against retries: a partial-write + retry won't
///   clobber the prior archive.
///
/// **What it does**:
/// 1. Read current `task.json` (must be `Done`).
/// 2. Compute `dst = <project>/.everlasting/tasks/archive/<YYYY-MM>/<slug>/`.
/// 3. `mkdir -p` the archive parent.
/// 4. `fs::rename(src, dst)` — same-filesystem rename is
///    atomic; the `mkdir -p` step above guarantees
///    `dst`'s parent exists on the same FS as `src`.
/// 5. Re-open `dst/task.json`, set `status = Completed`
///    + `completed_at = now()` + `updated_at = now()`,
///    atomic-rename write (via `write_task`).
/// 6. (Default) `git add` + `git commit` the archive tree
///    so the move is captured in version history. Caller
///    can opt out via `no_commit = true` for dry-run /
///    offline scenarios.
///
/// **Returns** the archived `TaskJson` (status
/// `Completed`, `completed_at` set) so the IPC layer
/// can surface the post-archive state to the frontend.
///
/// **Step 3.3 design note**: archive is intentionally a
/// **move**, not a copy. The pre-archive dir is gone
/// after a successful archive — `resolve_current_task`
/// will not pick the task up again because (a) the live
/// tree no longer has the slug, and (b) the archive tree
/// has `status = Completed` which `inject.rs` also skips.
pub fn archive_task_init(project_path: &Path, slug: &str, no_commit: bool) -> TaskResult<TaskJson> {
    use std::fmt::Write;

    validate_slug(slug)?;

    let src_dir = task_dir(project_path, slug);

    // 1. Read current task.json (must be Done).
    let mut task = read_task(project_path, slug)?;

    if task.status != TaskStatus::Done {
        return Err(TaskError::NotInDoneStatus(task.status.as_str().to_string()));
    }

    // 2. Compute destination under archive/<YYYY-MM>/<slug>/.
    let now_dt = Utc::now();
    let ym = now_dt.format("%Y-%m").to_string();
    let dst_dir = src_dir
        .parent()
        .ok_or_else(|| TaskError::Io(src_dir.clone(), io::Error::other("task dir has no parent")))?
        .join(PROJ_NS_TASKS_ARCHIVE_DIR)
        .join(&ym)
        .join(slug);

    if dst_dir.exists() {
        return Err(TaskError::AlreadyArchived(dst_dir));
    }

    // 3. mkdir -p the archive parent (so `fs::rename`
    //    succeeds on the same FS).
    if let Some(parent) = dst_dir.parent() {
        fs::create_dir_all(parent).map_err(|e| TaskError::Io(parent.to_path_buf(), e))?;
    }

    // 4. Move src → dst (atomic on same FS).
    fs::rename(&src_dir, &dst_dir).map_err(|e| TaskError::Io(src_dir.clone(), e))?;

    // 5. Re-write task.json in dst with Completed +
    //    completed_at + updated_at. We bypass `write_task`
    //    (which would re-check the parent dir layout)
    //    and write directly via atomic tmp+rename against
    //    the new path.
    let now_ts = now_dt.to_rfc3339();
    task.status = TaskStatus::Completed;
    task.updated_at = now_ts.clone();
    task.completed_at = Some(now_ts);

    let dst_json = dst_dir.join("task.json");
    let dst_tmp = dst_dir.join("task.json.tmp");
    let bytes = serde_json::to_vec_pretty(&task)
        .map_err(|e| TaskError::MalformedJson(dst_json.clone(), e.to_string()))?;
    fs::write(&dst_tmp, &bytes).map_err(|e| TaskError::Io(dst_tmp.clone(), e))?;
    fs::rename(&dst_tmp, &dst_json).map_err(|e| TaskError::Io(dst_json.clone(), e))?;

    tracing::info!(
        slug = %slug,
        src = %src_dir.display(),
        dst = %dst_dir.display(),
        "archive_task_init: task moved to archive tree"
    );

    // 6. (Default) git add + commit the archive tree.
    //    Use a shell-out to `git` rather than wiring
    //    libgit2 here — the commit is the project's repo,
    //    not a session worktree, and the project repo's
    //    HEAD branch identity varies per developer
    //    (main / master / feature). libgit2 would need
    //    an equivalent of `git -C <repo> commit -m ...`
    //    with the right user identity pre-configured; the
    //    spawn route inherits the developer's existing
    //    identity + branch config without ceremony. The
    //    IPC test layer skips this branch (passes
    //    `no_commit = true`).
    if !no_commit {
        let archive_rel = format!(
            ".everlasting/tasks/{dir}/{slug}",
            dir = PROJ_NS_TASKS_ARCHIVE_DIR,
            slug = slug,
        );
        if let Err(e) = git_add_path(project_path, &archive_rel) {
            tracing::warn!(
                slug = %slug,
                error = %e,
                "archive_task_init: git add failed; archive still on disk, \
                 commit skipped. The user can `git add` + commit manually."
            );
        } else {
            // Sanity: if `git add` succeeded but the
            // path is already tracked (e.g. archive
            // happened twice and the second time the
            // archive dir already exists as a stale
            // entry), `git commit` is a no-op. That's
            // acceptable — the move is the primary
            // effect; the commit is a convenience.
            let mut msg = String::from("chore(task): archive ");
            let _ = write!(&mut msg, "{}", slug);
            if let Err(e) = git_commit(project_path, &msg) {
                tracing::warn!(
                    slug = %slug,
                    error = %e,
                    "archive_task_init: git commit failed; archive on disk, \
                     commit skipped. The user can commit manually."
                );
            } else {
                tracing::info!(
                    slug = %slug,
                    "archive_task_init: git committed archive tree"
                );
            }
        }
    }

    Ok(task)
}

/// Helper: spawn `git -C <repo> add <path>`. Returns
/// `Ok(())` if `git` exits 0 OR the path is already
/// tracked (idempotent — re-running with no diff is a
/// no-op for `git add`). Errors (binary missing /
/// non-zero exit / non-git dir) are surfaced as
/// `Err(String)` so the caller can decide whether to
/// log + continue or bubble up.
pub(crate) fn git_add_path(repo: &Path, rel_path: &str) -> Result<(), String> {
    use std::process::Command;
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["add", "--", rel_path])
        .output()
        .map_err(|e| format!("spawn git add failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git add exited {:?}: stderr={}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

/// Helper: spawn `git -C <repo> commit -m <msg>`.
/// Returns `Ok(())` on success. The "nothing to commit"
/// case is treated as `Err("nothing to commit")` so the
/// caller logs it but doesn't escalate — archive's
/// primary effect (the move) already happened.
pub(crate) fn git_commit(repo: &Path, msg: &str) -> Result<(), String> {
    use std::process::Command;
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["commit", "--quiet", "-m", msg])
        .output()
        .map_err(|e| format!("spawn git commit failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git commit exited {:?}: stderr={}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}
