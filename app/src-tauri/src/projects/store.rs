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
