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
use crate::sandbox::policy::ProjectSandboxPolicy;

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
            sandbox_policy: ProjectSandboxPolicy::ReadWrite.as_str().to_string(),
            sandbox_net: None,
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
 SELECT id, name, path, is_git_repo, git_branch, is_legacy, created_at, updated_at, hidden, metadata, sandbox_policy, sandbox_net
 FROM projects
 ORDER BY created_at ASC
 "#,
        )
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query(
            r#"
 SELECT id, name, path, is_git_repo, git_branch, is_legacy, created_at, updated_at, hidden, metadata, sandbox_policy, sandbox_net
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
 SELECT id, name, path, is_git_repo, git_branch, is_legacy, created_at, updated_at, hidden, metadata, sandbox_policy, sandbox_net
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
 SELECT id, name, path, is_git_repo, git_branch, is_legacy, created_at, updated_at, hidden, metadata, sandbox_policy, sandbox_net
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
 SELECT id, name, path, is_git_repo, git_branch, is_legacy, created_at, updated_at, hidden, metadata, sandbox_policy, sandbox_net
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

/// P3c (design §2): set a project's sandbox policy tier. The value
/// is validated by the command layer whitelist (and by the DB CHECK
/// constraint as a backstop); this function stores it verbatim.
pub async fn set_project_sandbox_policy(
    pool: &SqlitePool,
    project_id: &str,
    policy: &str,
) -> Result<ProjectRow, sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    let res = sqlx::query("UPDATE projects SET sandbox_policy = ?, updated_at = ? WHERE id = ?")
        .bind(policy)
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
        sandbox_policy: r.try_get("sandbox_policy")?,
        sandbox_net: r.try_get("sandbox_net")?,
    })
}

// ---------------------------------------------------------------------------
// 09-21-sandbox-net-bindonly: net snapshot / proposal persistence (R4)
// ---------------------------------------------------------------------------

/// One confirmed bind-port snapshot row (operator-authorized).
#[derive(Debug, Clone, serde::Serialize)]
pub struct NetSnapshotRow {
    pub project_id: String,
    /// Canonicalized worktree absolute path — the authorization key
    /// (a branch switch / re-checkout produces a different key).
    pub worktree_key: String,
    /// Comma-separated u16 list (parse via `BindSet::parse_ports`).
    pub ports: String,
    pub confirmed_by: String,
    /// Unix ms.
    pub confirmed_at: i64,
}

/// One port proposal (LLM/manifest suggestion — never effective
/// until an operator confirms it into a snapshot).
#[derive(Debug, Clone, serde::Serialize)]
pub struct NetProposalRow {
    pub project_id: String,
    pub worktree_key: String,
    pub ports: String,
    /// Where the suggestion came from (`llm` / `manifest` / …).
    pub source: String,
    /// `pending` / `confirmed` / `rejected`.
    pub status: String,
    pub proposed_at: i64,
}

/// Set the NET tier column verbatim (`block` / `bind_only:<ports>`;
/// validated by the command layer — no `allow_all` write surface
/// exists this task).
pub async fn set_project_sandbox_net(
    pool: &SqlitePool,
    project_id: &str,
    net: Option<&str>,
) -> Result<ProjectRow, sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    let res = sqlx::query("UPDATE projects SET sandbox_net = ?, updated_at = ? WHERE id = ?")
        .bind(net)
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

/// Upsert the confirmed snapshot for (project, worktree) — the
/// authorization truth for the BindOnly tier.
pub async fn upsert_net_snapshot(
    pool: &SqlitePool,
    project_id: &str,
    worktree_key: &str,
    ports: &str,
    confirmed_by: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO project_net_snapshots (project_id, worktree_key, ports, confirmed_by, confirmed_at) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT (project_id, worktree_key) \
         DO UPDATE SET ports = excluded.ports, confirmed_by = excluded.confirmed_by, confirmed_at = excluded.confirmed_at",
    )
    .bind(project_id)
    .bind(worktree_key)
    .bind(ports)
    .bind(confirmed_by)
    .bind(chrono::Utc::now().timestamp_millis())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_net_snapshots(
    pool: &SqlitePool,
    project_id: &str,
) -> Result<Vec<NetSnapshotRow>, sqlx::Error> {
    let rows: Vec<(String, String, String, String, i64)> = sqlx::query_as(
        "SELECT project_id, worktree_key, ports, confirmed_by, confirmed_at \
         FROM project_net_snapshots WHERE project_id = ? ORDER BY confirmed_at DESC",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(project_id, worktree_key, ports, confirmed_by, confirmed_at)| NetSnapshotRow {
                project_id,
                worktree_key,
                ports,
                confirmed_by,
                confirmed_at,
            },
        )
        .collect())
}

/// Upsert a proposal (pending) — the latest suggestion per worktree
/// wins (REPLACE on the PK).
pub async fn upsert_net_proposal(
    pool: &SqlitePool,
    project_id: &str,
    worktree_key: &str,
    ports: &str,
    source: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO project_net_proposals (project_id, worktree_key, ports, source, status, proposed_at) \
         VALUES (?, ?, ?, ?, 'pending', ?) \
         ON CONFLICT (project_id, worktree_key) \
         DO UPDATE SET ports = excluded.ports, source = excluded.source, status = 'pending', proposed_at = excluded.proposed_at",
    )
    .bind(project_id)
    .bind(worktree_key)
    .bind(ports)
    .bind(source)
    .bind(chrono::Utc::now().timestamp_millis())
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark the latest proposal for a worktree confirmed/rejected (row
/// kept for UI echo).
pub async fn set_net_proposal_status(
    pool: &SqlitePool,
    project_id: &str,
    worktree_key: &str,
    status: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE project_net_proposals SET status = ? WHERE project_id = ? AND worktree_key = ?",
    )
    .bind(status)
    .bind(project_id)
    .bind(worktree_key)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_net_proposals(
    pool: &SqlitePool,
    project_id: &str,
) -> Result<Vec<NetProposalRow>, sqlx::Error> {
    let rows: Vec<(String, String, String, String, String, i64)> = sqlx::query_as(
        "SELECT project_id, worktree_key, ports, source, status, proposed_at \
         FROM project_net_proposals WHERE project_id = ? ORDER BY proposed_at DESC",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(project_id, worktree_key, ports, source, status, proposed_at)| NetProposalRow {
                project_id,
                worktree_key,
                ports,
                source,
                status,
                proposed_at,
            },
        )
        .collect())
}
