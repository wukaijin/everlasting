//! Turn-boundary checkpoint rows (`turn_checkpoints` table) — the DB
//! half of the N2 snapshot chain (task `09-20-n2-checkpoint-revert`).
//!
//! One row per turn-end snapshot: `seq` = the turn's final assistant
//! row `messages.seq` (baseline row sits on the FIRST user row's seq,
//! usually 0), `tree_sha` / `commit_sha` address the dangling git
//! objects created by `git::checkpoint` (this module never touches
//! git — the wiring layer in `agent/checkpoint.rs` owns the repo
//! handle and the zero-touch contract lives there).
//!
//! Write semantics are pinned by the design (§2.6): INSERT OR REPLACE
//! — a same-`(session_id, seq)` retry is idempotent; a key collision
//! is a signal, not an error path (D3 edit-resend reuses seqs after
//! its cascade delete). Session deletion cleans rows via the FK
//! CASCADE (the umbrella ref cleanup is a separate concern, wired in
//! `commands::sessions::delete_session_inner`).

use sqlx::SqlitePool;

/// One `turn_checkpoints` row. `tree_sha` / `commit_sha` are hex oids
/// (git2 `Oid::to_string` form — lowercase 40-hex for SHA-1 repos).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointRow {
    pub session_id: String,
    pub seq: i64,
    pub tree_sha: String,
    pub commit_sha: String,
    /// Unix milliseconds (scheduled_tasks-style INTEGER ms column).
    pub created_at: i64,
}

/// Insert or replace the checkpoint row for `(session_id, seq)`.
/// REPLACE (not ON CONFLICT DO UPDATE) per design §2.6: the whole row
/// is rewritten on re-snapshot of the same seq, `created_at` included
/// (the row's identity is "the snapshot taken for this turn", and a
/// retry IS a new snapshot).
pub async fn upsert_checkpoint(
    pool: &SqlitePool,
    session_id: &str,
    seq: i64,
    tree_sha: &str,
    commit_sha: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
 INSERT OR REPLACE INTO turn_checkpoints
 (session_id, seq, tree_sha, commit_sha, created_at)
 VALUES (?, ?, ?, ?, ?)
 "#,
    )
    .bind(session_id)
    .bind(seq)
    .bind(tree_sha)
    .bind(commit_sha)
    .bind(chrono::Utc::now().timestamp_millis())
    .execute(pool)
    .await?;
    Ok(())
}

/// The latest (highest-seq) checkpoint row, or `None` when the session
/// has no chain yet. The dedupe / chain-parent source for the wiring
/// layer and the "skip a signal-less turn" gate (design §4).
pub async fn latest_checkpoint(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Option<CheckpointRow>, sqlx::Error> {
    sqlx::query_as(
        r#"
 SELECT session_id, seq, tree_sha, commit_sha, created_at
 FROM turn_checkpoints WHERE session_id = ?
 ORDER BY seq DESC LIMIT 1
 "#,
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await
    .map(|opt| {
        opt.map(
            |(session_id, seq, tree_sha, commit_sha, created_at)| CheckpointRow {
                session_id,
                seq,
                tree_sha,
                commit_sha,
                created_at,
            },
        )
    })
}

/// All checkpoint rows for a session, oldest first. The PR2 read
/// surface's source (`prev_seq` derives from this ordering).
#[allow(dead_code)] // PR2 list_turn_checkpoints 命令消费(AC 排期同 git/checkpoint.rs 的按项 allow)
pub async fn list_checkpoints(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Vec<CheckpointRow>, sqlx::Error> {
    sqlx::query_as(
        r#"
 SELECT session_id, seq, tree_sha, commit_sha, created_at
 FROM turn_checkpoints WHERE session_id = ?
 ORDER BY seq ASC
 "#,
    )
    .bind(session_id)
    .fetch_all(pool)
    .await
    .map(|rows| {
        rows.into_iter()
            .map(
                |(session_id, seq, tree_sha, commit_sha, created_at)| CheckpointRow {
                    session_id,
                    seq,
                    tree_sha,
                    commit_sha,
                    created_at,
                },
            )
            .collect()
    })
}

/// Whether the session has a baseline (any checkpoint row). The
/// loop-entry hook probes this before building the baseline; `true`
/// means a signal-less turn-end can skip entirely (zero scans, zero
/// rows — design §4).
pub async fn has_checkpoint_baseline(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<bool, sqlx::Error> {
    let (exists,): (i64,) =
        sqlx::query_as("SELECT EXISTS(SELECT 1 FROM turn_checkpoints WHERE session_id = ?)")
            .bind(session_id)
            .fetch_one(pool)
            .await?;
    Ok(exists != 0)
}
