//! Column-probe helpers: add a column if missing (PRAGMA table_info probe).
//!
//! Split out of `db/migrations.rs` (2026-08-08 batch3).

use sqlx::{Row, SqlitePool};

/// Add a column to `sessions` if it doesn't already exist. SQLite
/// doesn't have `ALTER TABLE ... ADD COLUMN IF NOT EXISTS` in3.35
/// reliably (and the underlying error code is `1` for "duplicate
/// column"), so we probe `PRAGMA table_info` first.
pub(crate) async fn add_session_column_if_missing(
    pool: &SqlitePool,
    column: &str,
    decl: &str,
) -> Result<(), sqlx::Error> {
    let exists: i64 =
        sqlx::query("SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = ?")
            .bind(column)
            .fetch_one(pool)
            .await?
            .try_get(0)?;
    if exists == 0 {
        let stmt = format!("ALTER TABLE sessions ADD COLUMN {} {}", column, decl);
        sqlx::query(&stmt).execute(pool).await?;
    }
    Ok(())
}

/// Add a column to `projects` if it doesn't already exist. Mirrors
/// [`add_session_column_if_missing`].
pub(crate) async fn add_project_column_if_missing(
    pool: &SqlitePool,
    column: &str,
    decl: &str,
) -> Result<(), sqlx::Error> {
    let exists: i64 =
        sqlx::query("SELECT COUNT(*) FROM pragma_table_info('projects') WHERE name = ?")
            .bind(column)
            .fetch_one(pool)
            .await?
            .try_get(0)?;
    if exists == 0 {
        let stmt = format!("ALTER TABLE projects ADD COLUMN {} {}", column, decl);
        sqlx::query(&stmt).execute(pool).await?;
    }
    Ok(())
}

/// Add a column to `providers` if it doesn't already exist. Mirrors
/// [`add_session_column_if_missing`]. Added for RULE-D-001 (`api_key_enc` /
/// `key_migrated_at`, 2026-06-24).
pub(crate) async fn add_provider_column_if_missing(
    pool: &SqlitePool,
    column: &str,
    decl: &str,
) -> Result<(), sqlx::Error> {
    let exists: i64 =
        sqlx::query("SELECT COUNT(*) FROM pragma_table_info('providers') WHERE name = ?")
            .bind(column)
            .fetch_one(pool)
            .await?
            .try_get(0)?;
    if exists == 0 {
        let stmt = format!("ALTER TABLE providers ADD COLUMN {} {}", column, decl);
        sqlx::query(&stmt).execute(pool).await?;
    }
    Ok(())
}

pub(crate) async fn add_messages_column_if_missing(
    pool: &SqlitePool,
    column: &str,
    decl: &str,
) -> Result<(), sqlx::Error> {
    let exists: i64 =
        sqlx::query("SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name = ?")
            .bind(column)
            .fetch_one(pool)
            .await?
            .try_get(0)?;
    if exists == 0 {
        let stmt = format!("ALTER TABLE messages ADD COLUMN {} {}", column, decl);
        sqlx::query(&stmt).execute(pool).await?;
    }
    Ok(())
}

/// Add a column to `models` if it doesn't already exist. Mirrors
/// [`add_session_column_if_missing`]. Added for B1 (image
/// multimodal, 2026-08-16) — the `supports_images` capability flag
/// that gates wire-layer image blocks (default 0 = degrade to text
/// placeholder, behavior-equivalent to the pre-B1 text-only channel).
pub(crate) async fn add_models_column_if_missing(
    pool: &SqlitePool,
    column: &str,
    decl: &str,
) -> Result<(), sqlx::Error> {
    let exists: i64 =
        sqlx::query("SELECT COUNT(*) FROM pragma_table_info('models') WHERE name = ?")
            .bind(column)
            .fetch_one(pool)
            .await?
            .try_get(0)?;
    if exists == 0 {
        let stmt = format!("ALTER TABLE models ADD COLUMN {} {}", column, decl);
        sqlx::query(&stmt).execute(pool).await?;
    }
    Ok(())
}

/// Add a column to `subagent_runs` if it doesn't already exist.
/// Mirrors [`add_session_column_if_missing`]. Added for the
/// 2026-06-21 subagent-drawer redesign PR1 (`task` + `final_text`).
pub(crate) async fn add_subagent_runs_column_if_missing(
    pool: &SqlitePool,
    column: &str,
    decl: &str,
) -> Result<(), sqlx::Error> {
    let exists: i64 =
        sqlx::query("SELECT COUNT(*) FROM pragma_table_info('subagent_runs') WHERE name = ?")
            .bind(column)
            .fetch_one(pool)
            .await?
            .try_get(0)?;
    if exists == 0 {
        let stmt = format!("ALTER TABLE subagent_runs ADD COLUMN {} {}", column, decl);
        sqlx::query(&stmt).execute(pool).await?;
    }
    Ok(())
}

/// Add a column to `autonomous_memories` if it doesn't already
/// exist. Mirrors [`add_session_column_if_missing`]. Added for
/// 07-06 (am-observability-panel) — the `edited_by_user`
/// provenance marker that the management modal uses to distinguish
/// agent-written memories from user-edited ones.
pub(crate) async fn add_autonomous_memories_column_if_missing(
    pool: &SqlitePool,
    column: &str,
    decl: &str,
) -> Result<(), sqlx::Error> {
    let exists: i64 =
        sqlx::query("SELECT COUNT(*) FROM pragma_table_info('autonomous_memories') WHERE name = ?")
            .bind(column)
            .fetch_one(pool)
            .await?
            .try_get(0)?;
    if exists == 0 {
        let stmt = format!(
            "ALTER TABLE autonomous_memories ADD COLUMN {} {}",
            column, decl
        );
        sqlx::query(&stmt).execute(pool).await?;
    }
    Ok(())
}

/// Add a column to `session_audit_events` if it doesn't already
/// exist. Mirrors [`add_session_column_if_missing`]. Added for E2
/// (harness trace pipeline, 2026-07-14) — the `turn_seq` column
/// that lets audit rows be grouped by turn for the trace viewer.
pub(crate) async fn add_session_audit_events_column_if_missing(
    pool: &SqlitePool,
    column: &str,
    decl: &str,
) -> Result<(), sqlx::Error> {
    let exists: i64 = sqlx::query(
        "SELECT COUNT(*) FROM pragma_table_info('session_audit_events') WHERE name = ?",
    )
    .bind(column)
    .fetch_one(pool)
    .await?
    .try_get(0)?;
    if exists == 0 {
        let stmt = format!(
            "ALTER TABLE session_audit_events ADD COLUMN {} {}",
            column, decl
        );
        sqlx::query(&stmt).execute(pool).await?;
    }
    Ok(())
}

/// Add a column to `turn_trace` if it doesn't already exist. Mirrors
/// [`add_session_column_if_missing`]. Added for C7 (tools[] token
/// governance, 2026-08-14) — the `tools_token` column that records
/// the per-turn estimated token cost of the serialized `tools[]`
/// array (a separately-measured dimension of context usage that is
/// NOT folded into the cache-rate; see `db::trace` + design §R1).
pub(crate) async fn add_turn_trace_column_if_missing(
    pool: &SqlitePool,
    column: &str,
    decl: &str,
) -> Result<(), sqlx::Error> {
    let exists: i64 =
        sqlx::query("SELECT COUNT(*) FROM pragma_table_info('turn_trace') WHERE name = ?")
            .bind(column)
            .fetch_one(pool)
            .await?
            .try_get(0)?;
    if exists == 0 {
        let stmt = format!("ALTER TABLE turn_trace ADD COLUMN {} {}", column, decl);
        sqlx::query(&stmt).execute(pool).await?;
    }
    Ok(())
}

/// Add a column to `scheduled_tasks` if it doesn't already exist.
/// Mirrors [`add_session_column_if_missing`]. Added for F2b
/// (schedule end conditions, 2026-08-28) — `run_count` / `max_runs` /
/// `ends_at`:次数上限与结束日期的判定与展示列。
pub(crate) async fn add_scheduled_tasks_column_if_missing(
    pool: &SqlitePool,
    column: &str,
    decl: &str,
) -> Result<(), sqlx::Error> {
    let exists: i64 =
        sqlx::query("SELECT COUNT(*) FROM pragma_table_info('scheduled_tasks') WHERE name = ?")
            .bind(column)
            .fetch_one(pool)
            .await?
            .try_get(0)?;
    if exists == 0 {
        let stmt = format!("ALTER TABLE scheduled_tasks ADD COLUMN {} {}", column, decl);
        sqlx::query(&stmt).execute(pool).await?;
    }
    Ok(())
}
