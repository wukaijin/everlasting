//! Project data-access layer. Thin wrapper over `crate::db` that
//! adds the higher-level "create / hide / unhide / rename / repath"
//! operations; actual SQL stays in `db.rs` to keep all DDL / DML in
//! one place.
//!
//! This module is intentionally tiny — it is a façade, not a new
//! persistence layer. The `db` module owns sqlx; this module owns
//! the orchestration (e.g. "re-probe is_git_repo + git_branch on
//! path change", "default name = basename(path)").

use std::path::Path;

use crate::db;
use crate::projects::types::ProjectRow;

use super::detector::{current_branch_sync, is_git_repo_sync};

/// Create a new project, probing for `is_git_repo` and `git_branch`
/// (the latter is only meaningful when the directory is a git repo).
///
/// Steps:
/// 1. Verify `path` exists and is a directory.
/// 2. Probe `is_git_repo` synchronously (cheap, <1s).
/// 3. If the directory is a git repo, probe `git_branch` (also
///    cheap; the `current_branch_sync` helper short-circuits to
///    `None` when `is_git_repo` is `false`).
/// 4. Insert the row, defaulting `name` to `basename(path)`.
pub async fn create_project(pool: &sqlx::SqlitePool, path: &str) -> Result<ProjectRow, String> {
    let p = Path::new(path);
    if !p.exists() {
        return Err(format!("path '{}' does not exist", path));
    }
    if !p.is_dir() {
        return Err(format!("path '{}' is not a directory", path));
    }

    let is_git = is_git_repo_sync(p);
    // `current_branch_sync` already short-circuits on non-git repos,
    // but doing the explicit guard keeps the intent clear and saves a
    // redundant `git -C <path> rev-parse --show-toplevel` invocation.
    let git_branch = if is_git {
        current_branch_sync(p)
    } else {
        None
    };
    let name = ProjectRow::default_name_from_path(path);
    db::create_project(pool, &name, path, is_git, git_branch)
        .await
        .map_err(|e| format!("create_project failed: {}", e))
}

/// Change a project's `path`. The new path must exist, must be a
/// directory, and no project may be using it already. `is_git_repo`
/// and `git_branch` are re-probed.
pub async fn update_project_path(
    pool: &sqlx::SqlitePool,
    project_id: &str,
    new_path: &str,
) -> Result<ProjectRow, String> {
    let p = Path::new(new_path);
    if !p.exists() {
        return Err(format!("path '{}' does not exist", new_path));
    }
    if !p.is_dir() {
        return Err(format!("path '{}' is not a directory", new_path));
    }
    let is_git = is_git_repo_sync(p);
    let git_branch = if is_git {
        current_branch_sync(p)
    } else {
        None
    };
    db::update_project_path(pool, project_id, new_path, is_git, git_branch)
        .await
        .map_err(|e| format!("update_project_path failed: {}", e))
}

pub async fn update_project_name(
    pool: &sqlx::SqlitePool,
    project_id: &str,
    new_name: &str,
) -> Result<ProjectRow, String> {
    let trimmed = new_name.trim();
    if trimmed.is_empty() {
        return Err("name must not be empty".to_string());
    }
    db::update_project_name(pool, project_id, trimmed)
        .await
        .map_err(|e| format!("update_project_name failed: {}", e))
}

pub async fn hide_project(pool: &sqlx::SqlitePool, project_id: &str) -> Result<(), String> {
    db::hide_project(pool, project_id)
        .await
        .map_err(|e| format!("hide_project failed: {}", e))
}

pub async fn unhide_project(pool: &sqlx::SqlitePool, project_id: &str) -> Result<(), String> {
    db::unhide_project(pool, project_id)
        .await
        .map_err(|e| format!("unhide_project failed: {}", e))
}

/// Re-probe `is_git_repo` + `git_branch` for every project whose
/// `is_git_repo` column is `0` (i.e. pre-PR2 rows that were created
/// before the migration added these columns, or rows whose original
/// probe failed). Used by the startup backfill task spawned in
/// `lib.rs::AppState::load`.
///
/// Each project is probed in the calling task; `is_git_repo_sync` +
/// `current_branch_sync` each shell out to `git` and typically
/// complete in <50ms. For ~10 projects the total wall-clock is
/// well under the 1s budget noted in the task spec.
///
/// The function is **idempotent**: rows where the probe result
/// matches the stored state (both `is_git_repo` and `git_branch`)
/// are skipped, so re-running on every startup does not bump
/// `updated_at` for projects that haven't changed.
///
/// Returns the number of rows that were actually written. The
/// caller (the spawn closure in `lib.rs`) is responsible for
/// emitting the `projects:refreshed` Tauri event so the frontend
/// can refresh its in-memory list when the count is non-zero.
pub async fn batch_reprobe_git_metadata(
    pool: &sqlx::SqlitePool,
) -> Result<usize, String> {
    let stale = db::list_projects_with_stale_git_probe(pool)
        .await
        .map_err(|e| format!("list stale projects failed: {}", e))?;
    let total = stale.len();
    let mut updated = 0usize;
    for p in stale {
        let path = std::path::Path::new(&p.path);
        // `is_git_repo_sync` returns `false` for paths that no
        // longer exist on disk (or for paths the user has since
        // moved/renamed) — that's the conservative answer and
        // matches what `create_project` / `update_project_path`
        // would have done at the time the project was registered.
        let is_git = is_git_repo_sync(path);
        let git_branch = if is_git {
            current_branch_sync(path)
        } else {
            None
        };
        // Skip the write if the probe result matches the stored
        // value. This keeps `updated_at` stable for projects that
        // genuinely are non-git (e.g. a home-directory project that
        // was never a repo) so the Tab bar's "recently updated"
        // ordering doesn't churn on every startup.
        let stored_branch = p.git_branch.as_deref();
        if p.is_git_repo == is_git && stored_branch == git_branch.as_deref() {
            continue;
        }
        if let Err(e) = db::update_project_git_metadata(
            pool,
            &p.id,
            is_git,
            git_branch.as_deref(),
        )
        .await
        {
            tracing::warn!(
                project_id = %p.id,
                path = %p.path,
                error = %e,
                "git metadata backfill update failed for project; will retry on next startup"
            );
            continue;
        }
        updated += 1;
        tracing::debug!(
            project_id = %p.id,
            path = %p.path,
            is_git,
            git_branch = ?git_branch,
            "git metadata backfilled for project"
        );
    }
    tracing::info!(total, updated, "git metadata backfill complete");
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use std::process::Command;

    /// Mirrors `db::tests::test_pool` — kept private to this module
    /// so the test fixture doesn't leak into a shared test helper.
    async fn test_pool() -> sqlx::SqlitePool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        db::run_migrations(&pool).await.unwrap();
        pool
    }

    /// `git init` is a real shell-out; if the host has no `git` we
    /// skip rather than fail (mirrors `detector::tests`).
    fn try_init_git(dir: &Path) -> bool {
        let status = Command::new("git")
            .arg("init")
            .arg("--quiet")
            .arg(dir)
            .status();
        matches!(status, Ok(s) if s.success())
    }

    fn try_config_identity(dir: &Path) {
        let _ = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["config", "user.email", "test@example.com"])
            .status();
        let _ = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["config", "user.name", "test"])
            .status();
    }

    fn try_commit_initial(dir: &Path) {
        std::fs::write(dir.join("README.md"), "x").unwrap();
        let _ = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["add", "."])
            .status();
        let _ = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["commit", "--quiet", "-m", "init"])
            .status();
    }

    /// End-to-end: a project row that was inserted with
    /// `is_git_repo=0, git_branch=NULL` (the pre-PR2 state) gets
    /// probed on startup and the row is updated to reflect the
    /// real git status of the directory on disk.
    #[tokio::test]
    async fn batch_reprobe_backfills_git_repo_metadata() {
        let dir = tempfile::tempdir().expect("tempdir");
        if !try_init_git(dir.path()) {
            eprintln!("git not available; skipping test");
            return;
        }
        try_config_identity(dir.path());
        try_commit_initial(dir.path());

        let pool = test_pool().await;
        // Insert a stale row: the path IS a git repo, but the row
        // was created before PR2 (so it carries the
        // `is_git_repo=0, git_branch=NULL` defaults from the
        // ALTER TABLE migration).
        let p = db::create_project(
            &pool,
            "stale",
            dir.path().to_str().unwrap(),
            false,
            None,
        )
        .await
        .unwrap();
        assert!(!p.is_git_repo);
        assert!(p.git_branch.is_none());

        // Run the backfill. Exactly one row should be written.
        let updated = batch_reprobe_git_metadata(&pool).await.unwrap();
        assert_eq!(updated, 1, "exactly one stale row should be updated");

        let reloaded = db::get_project(&pool, &p.id).await.unwrap().unwrap();
        assert!(reloaded.is_git_repo, "backfill should have flipped is_git_repo to true");
        // The branch name depends on the host's `init.defaultBranch`
        // (main / master / etc.); we just assert it's *some* real
        // branch (not the literal "HEAD" and not empty).
        let branch = reloaded.git_branch.expect("git_branch should be set");
        assert!(!branch.is_empty() && branch != "HEAD", "got {:?}", branch);
    }

    /// A non-git project must be left at `is_git_repo=0,
    /// git_branch=NULL` — the backfill probes but does not flip the
    /// row to `is_git_repo=true`. This guards against a regression
    /// where the function incorrectly "fills in" non-git projects.
    #[tokio::test]
    async fn batch_reprobe_skips_non_git_projects() {
        let dir = tempfile::tempdir().expect("tempdir");
        // No `git init` — `is_git_repo_sync` will return false.

        let pool = test_pool().await;
        let p = db::create_project(
            &pool,
            "notgit",
            dir.path().to_str().unwrap(),
            false,
            None,
        )
        .await
        .unwrap();

        // Run the backfill. The probe returns `false, None`, which
        // matches the stored `(false, None)`, so the row is skipped
        // and the function returns `0` (no rows updated).
        let updated = batch_reprobe_git_metadata(&pool).await.unwrap();
        assert_eq!(updated, 0, "non-git project should not be updated");

        let reloaded = db::get_project(&pool, &p.id).await.unwrap().unwrap();
        assert!(!reloaded.is_git_repo);
        assert!(reloaded.git_branch.is_none());
    }

    /// Idempotency: running the backfill a second time after a
    /// successful first run must report `0` updates (no rows
    /// changed → nothing to write → no churn on `updated_at`).
    #[tokio::test]
    async fn batch_reprobe_is_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        if !try_init_git(dir.path()) {
            eprintln!("git not available; skipping test");
            return;
        }
        try_config_identity(dir.path());
        try_commit_initial(dir.path());

        let pool = test_pool().await;
        let p = db::create_project(
            &pool,
            "idempotent",
            dir.path().to_str().unwrap(),
            false,
            None,
        )
        .await
        .unwrap();
        let original_updated_at = p.updated_at.clone();

        let first = batch_reprobe_git_metadata(&pool).await.unwrap();
        assert_eq!(first, 1);
        let after_first = db::get_project(&pool, &p.id).await.unwrap().unwrap();
        assert_ne!(after_first.updated_at, original_updated_at);

        // Second run: probe still returns the same result, so the
        // function must report 0 and leave `updated_at` untouched.
        let second = batch_reprobe_git_metadata(&pool).await.unwrap();
        assert_eq!(second, 0, "idempotent re-run must not write");
        let after_second = db::get_project(&pool, &p.id).await.unwrap().unwrap();
        assert_eq!(after_second.updated_at, after_first.updated_at);
    }
}
