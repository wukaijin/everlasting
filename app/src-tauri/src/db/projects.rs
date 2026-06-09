//! Project CRUD.
//!
//! Every row in the `projects` table maps to one "work environment"
//! directory the user has registered. Sessions are scoped to a
//! project. The `is_legacy` flag flags the auto-default backstop
//! row inserted by [`crate::db::migrations::run_migrations`] so the
//! UI can render it with a special "Legacy / 未分类" badge.

use chrono::Utc;
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::projects::ProjectRow;

/// Insert a new project row. Returns the inserted row.
pub async fn create_project(
 pool: &SqlitePool,
 name: &str,
 path: &str,
 is_git_repo: bool,
 git_branch: Option<String>,
) -> Result<ProjectRow, sqlx::Error> {
 let now = Utc::now().to_rfc3339();
 let id = Uuid::new_v4().to_string();

 let res = sqlx::query(
 r#"
 INSERT INTO projects
 (id, name, path, is_git_repo, git_branch, is_legacy, created_at, updated_at, hidden, metadata)
 VALUES (?, ?, ?, ?, ?,0, ?, ?,0, NULL)
 "#,
 )
 .bind(&id)
 .bind(name)
 .bind(path)
 .bind(is_git_repo as i64)
 .bind(git_branch.as_deref())
 .bind(&now)
 .bind(&now)
 .execute(pool)
 .await;

 match res {
 Ok(_) => Ok(ProjectRow {
 id,
 name: name.to_string(),
 path: path.to_string(),
 is_git_repo,
 git_branch,
 is_legacy: false,
 created_at: now.clone(),
 updated_at: now,
 hidden: false,
 metadata: None,
 }),
 Err(sqlx::Error::Database(db)) if db.is_unique_violation() => Err(sqlx::Error::Protocol(
 format!("a project with path '{}' already exists", path),
 )),
 Err(e) => Err(e),
 }
}

/// List projects. `include_hidden=false` returns only visible tabs
/// (the default for the main Tab bar); `include_hidden=true` is used
/// by the empty-state "recently hidden" list. Sorted by `created_at`
/// ASC so the Tab bar reads chronologically (oldest = leftmost).
pub async fn list_projects(
 pool: &SqlitePool,
 include_hidden: bool,
) -> Result<Vec<ProjectRow>, sqlx::Error> {
 let rows = if include_hidden {
 sqlx::query(
 r#"
 SELECT id, name, path, is_git_repo, git_branch, is_legacy, created_at, updated_at, hidden, metadata
 FROM projects
 ORDER BY created_at ASC
 "#,
 )
 .fetch_all(pool)
 .await?
 } else {
 sqlx::query(
 r#"
 SELECT id, name, path, is_git_repo, git_branch, is_legacy, created_at, updated_at, hidden, metadata
 FROM projects
 WHERE hidden = 0
 ORDER BY created_at ASC
 "#,
 )
 .fetch_all(pool)
 .await?
 };

 rows.into_iter().map(row_to_project).collect()
}

/// List hidden projects for the empty-state "recently hidden" panel,
/// sorted by `updated_at DESC` (most-recently-hidden first).
pub async fn list_hidden_projects(pool: &SqlitePool) -> Result<Vec<ProjectRow>, sqlx::Error> {
 let rows = sqlx::query(
 r#"
 SELECT id, name, path, is_git_repo, git_branch, is_legacy, created_at, updated_at, hidden, metadata
 FROM projects
 WHERE hidden = 1
 ORDER BY updated_at DESC
 "#,
 )
 .fetch_all(pool)
 .await?;
 rows.into_iter().map(row_to_project).collect()
}

/// Get a single project by id.
pub async fn get_project(
 pool: &SqlitePool,
 project_id: &str,
) -> Result<Option<ProjectRow>, sqlx::Error> {
 let row = sqlx::query(
 r#"
 SELECT id, name, path, is_git_repo, git_branch, is_legacy, created_at, updated_at, hidden, metadata
 FROM projects
 WHERE id = ?
 "#,
 )
 .bind(project_id)
 .fetch_optional(pool)
 .await?;
 row.map(row_to_project).transpose()
}

/// Change a project's `path` (re-probing `is_git_repo` and
/// `git_branch` is the caller's responsibility — see
/// `projects::store::update_project_path`).
pub async fn update_project_path(
 pool: &SqlitePool,
 project_id: &str,
 new_path: &str,
 is_git_repo: bool,
 git_branch: Option<String>,
) -> Result<ProjectRow, sqlx::Error> {
 let now = Utc::now().to_rfc3339();
 let res = sqlx::query(
 r#"
 UPDATE projects
 SET path = ?, is_git_repo = ?, git_branch = ?, updated_at = ?
 WHERE id = ?
 "#,
 )
 .bind(new_path)
 .bind(is_git_repo as i64)
 .bind(git_branch.as_deref())
 .bind(&now)
 .bind(project_id)
 .execute(pool)
 .await;
 match res {
 Ok(r) if r.rows_affected() == 0 => Err(sqlx::Error::RowNotFound),
 Ok(_) => get_project(pool, project_id)
 .await
 .and_then(|opt| opt.ok_or(sqlx::Error::RowNotFound)),
 Err(sqlx::Error::Database(db)) if db.is_unique_violation() => Err(sqlx::Error::Protocol(
 format!("a project with path '{}' already exists", new_path),
 )),
 Err(e) => Err(e),
 }
}

/// List projects whose `is_git_repo` is `0` — i.e. projects that
/// were created before the PR2 migration (which adds
/// `is_git_repo` / `git_branch`) and have never been re-probed, or
/// projects whose original probe failed. Sorted by `created_at ASC`
/// for stable test ordering.
///
/// Hidden projects are excluded from the backfill: they're not shown
/// in the Tab bar (which is the surface that would expose the bug),
/// and a user who explicitly hid a project is signaling that they
/// don't want proactive work on it. If they unhide later, the chip
/// will still show "git" until the next `update_project_path` call,
/// but that case is rare and acceptable.
///
/// Used by the startup backfill task — see
/// `projects::store::batch_reprobe_git_metadata` and the spawn in
/// `lib.rs::AppState::load`.
pub async fn list_projects_with_stale_git_probe(
 pool: &SqlitePool,
) -> Result<Vec<ProjectRow>, sqlx::Error> {
 let rows = sqlx::query(
 r#"
 SELECT id, name, path, is_git_repo, git_branch, is_legacy, created_at, updated_at, hidden, metadata
 FROM projects
 WHERE is_git_repo = 0 AND hidden = 0
 ORDER BY created_at ASC
 "#,
 )
 .fetch_all(pool)
 .await?;
 rows.into_iter().map(row_to_project).collect()
}

/// Update a project's `is_git_repo` and `git_branch`. Used by the
/// startup batch backfill to write re-probed git metadata without
/// touching the other columns (name / path / hidden / etc.).
///
/// `git_branch` is `None` for non-git repos; the literal string
/// `"HEAD"` is allowed through for detached-HEAD repos.
pub async fn update_project_git_metadata(
 pool: &SqlitePool,
 project_id: &str,
 is_git_repo: bool,
 git_branch: Option<&str>,
) -> Result<(), sqlx::Error> {
 let now = Utc::now().to_rfc3339();
 sqlx::query(
 r#"
 UPDATE projects
 SET is_git_repo = ?, git_branch = ?, updated_at = ?
 WHERE id = ?
 "#,
 )
 .bind(is_git_repo as i64)
 .bind(git_branch)
 .bind(&now)
 .bind(project_id)
 .execute(pool)
 .await?;
 Ok(())
}

/// Change a project's `name`.
pub async fn update_project_name(
 pool: &SqlitePool,
 project_id: &str,
 new_name: &str,
) -> Result<ProjectRow, sqlx::Error> {
 let now = Utc::now().to_rfc3339();
 let res = sqlx::query(
 r#"
 UPDATE projects
 SET name = ?, updated_at = ?
 WHERE id = ?
 "#,
 )
 .bind(new_name)
 .bind(&now)
 .bind(project_id)
 .execute(pool)
 .await;
 match res {
 Ok(r) if r.rows_affected() == 0 => Err(sqlx::Error::RowNotFound),
 Ok(_) => get_project(pool, project_id)
 .await
 .and_then(|opt| opt.ok_or(sqlx::Error::RowNotFound)),
 Err(e) => Err(e),
 }
}

/// Hide a project (× close-tab). Data is preserved. Hidden projects
/// do not show in the Tab bar but remain available via
/// [`list_hidden_projects`].
pub async fn hide_project(pool: &SqlitePool, project_id: &str) -> Result<(), sqlx::Error> {
 let now = Utc::now().to_rfc3339();
 sqlx::query("UPDATE projects SET hidden = 1, updated_at = ? WHERE id = ?")
 .bind(&now)
 .bind(project_id)
 .execute(pool)
 .await?;
 Ok(())
}

/// Reverse a [`hide_project`].
pub async fn unhide_project(pool: &SqlitePool, project_id: &str) -> Result<(), sqlx::Error> {
 let now = Utc::now().to_rfc3339();
 sqlx::query("UPDATE projects SET hidden = 0, updated_at = ? WHERE id = ?")
 .bind(&now)
 .bind(project_id)
 .execute(pool)
 .await?;
 Ok(())
}

pub(crate) fn row_to_project(r: sqlx::sqlite::SqliteRow) -> Result<ProjectRow, sqlx::Error> {
 Ok(ProjectRow {
 id: r.try_get("id")?,
 name: r.try_get("name")?,
 path: r.try_get("path")?,
 is_git_repo: r.try_get::<i64, _>("is_git_repo")? != 0,
 git_branch: r.try_get("git_branch")?,
 is_legacy: r.try_get::<i64, _>("is_legacy")? != 0,
 created_at: r.try_get("created_at")?,
 updated_at: r.try_get("updated_at")?,
 hidden: r.try_get::<i64, _>("hidden")? != 0,
 metadata: r.try_get("metadata")?,
 })
}
