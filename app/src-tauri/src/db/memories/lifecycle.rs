//! Status machine: bump/promote/update_memory/update_status.
//!
//! Relocated verbatim from the pre-split `memories.rs`.

use chrono::Utc;
use sqlx::SqlitePool;

use super::crud::get_memory_by_id;
// `MemoryKind` / `MemoryScope` are used by the `test_helpers` submodule
// (cfg(test)); in non-test builds they appear unused, hence the allow.
#[allow(unused_imports)]
use super::types::{
    MemoryInsertError, MemoryKind, MemoryRow, MemoryScope, MemoryStatus, MemoryUpdateError,
};
use super::validation::validate_memory_text;

// ---------------------------------------------------------------------------
// bump_hit_count / update_status — P5 status-machine interfaces
// ---------------------------------------------------------------------------

/// Promotion thresholds for the P5 status machine (design D2).
///
/// - `CANDIDATE_TO_ACTIVE_AT` — a candidate memory is promoted to
///   `active` once its `hit_count` reaches this (i.e. it has been
///   recalled this many times). 2 = "recalled twice → it's not a
///   one-off".
/// - `ACTIVE_TO_VERIFIED_AT` — an active memory is promoted to
///   `verified` once `hit_count` reaches this AND `created_at` is at
///   least `ACTIVE_TO_VERIFIED_AGE_DAYS` days old. 5 + 3 days = "hit
///   repeatedly over a non-trivial window → high-confidence".
///
/// Verified is the gating tier for P5's soft-block (design §4) —
/// getting there is intentionally non-trivial so the LLM doesn't get
/// soft-blocked on transient or low-quality memories.
pub const CANDIDATE_TO_ACTIVE_AT: i64 = 2;
pub const ACTIVE_TO_VERIFIED_AT: i64 = 5;
pub const ACTIVE_TO_VERIFIED_AGE_DAYS: i64 = 3;

/// Increment `hit_count` and stamp `last_used_at` for a memory.
/// Called by the recall paths (`search_memories_fts` / P3's
/// `find_pitfalls_by_trigger` consumer) when a memory is surfaced
/// — P5's status machine reads `hit_count` to decide promotion
/// (candidate → active → verified).
///
/// **P5 auto-promotion (2026-06-29, 06-29-am-p5-quality)**: after
/// the UPDATE, the same function checks the row against the
/// [`CANDIDATE_TO_ACTIVE_AT`] / [`ACTIVE_TO_VERIFIED_AT`] thresholds
/// and calls [`update_status`] to transition it. This is done on the
/// **same pool** right after the UPDATE so SQLite's single-writer
/// serialisation covers the read-modify-write (design §5; avoids the
/// bump↔promote race a separate caller-driven step would introduce).
/// Promotion failures are best-effort: logged + swallowed (the bump
/// already succeeded; a missed promotion this turn will fire next
/// turn).
///
/// Best-effort: a `warn!` on failure (matches the project's
/// "audit/metadata writes are best-effort" pattern). The recall
/// return value is unaffected by a hit-count bump failure.
pub async fn bump_hit_count(pool: &SqlitePool, memory_id: &str) -> Result<(), sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        UPDATE autonomous_memories
        SET hit_count = hit_count + 1,
            last_used_at = ?,
            updated_at = ?
        WHERE memory_id = ?
        "#,
    )
    .bind(&now)
    .bind(&now)
    .bind(memory_id)
    .execute(pool)
    .await?;

    // P5 (2026-06-29): best-effort auto-promotion. The bump already
    // landed; a promotion failure here is non-fatal (next bump
    // re-checks). Done on the same pool so the read-back sees the
    // just-written hit_count (SQLite serialises writers; the
    // UPDATE above is committed before this SELECT runs).
    if let Err(e) = promote_if_eligible(pool, memory_id).await {
        tracing::warn!(
            memory_id = memory_id,
            error = %e,
            "bump_hit_count: promote_if_eligible failed (non-fatal)"
        );
    }
    Ok(())
}

/// Check a memory's `(status, hit_count, created_at)` against the P5
/// promotion thresholds and transition it if eligible (design §5 +
/// D2). Reads back the post-bump values from the DB (so it sees the
/// just-incremented `hit_count`), then calls [`update_status`] for
/// the legal transition. No-op if no threshold is crossed or the
/// current status isn't promotion-eligible (e.g. already `verified`
/// or `demoted`).
///
/// Thresholds:
/// - `candidate` + `hit_count >= CANDIDATE_TO_ACTIVE_AT` → `active`.
/// - `active` + `hit_count >= ACTIVE_TO_VERIFIED_AT`
///   + age (`created_at` → now) `>= ACTIVE_TO_VERIFIED_AGE_DAYS` →
///   `verified`.
///
/// Illegal transitions are caught by `update_status`'s state matrix
/// (e.g. `demoted` rows are never re-promoted by this function —
/// re-promotion is the hygiene job's job). `NotFound` is benign (row
/// deleted between bump + promote) — returns `Ok(())`.
pub async fn promote_if_eligible(
    pool: &SqlitePool,
    memory_id: &str,
) -> Result<(), StatusTransitionError> {
    // Read back the post-bump row.
    let row = sqlx::query_as::<_, MemoryRow>(
        r#"
        SELECT id, memory_id, scope, project_id, kind, status, title, content,
               tags, tool_name, command_pattern, path_globs, source_session_id,
               source_ref, confidence, hit_count, last_used_at, created_at,
               updated_at, demoted_reason, edited_by_user
        FROM autonomous_memories
        WHERE memory_id = ?
        "#,
    )
    .bind(memory_id)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        // Row vanished (concurrent delete). Benign.
        return Ok(());
    };

    let current = MemoryStatus::from_str_opt(&row.status);
    let target = match current {
        MemoryStatus::Candidate if row.hit_count >= CANDIDATE_TO_ACTIVE_AT => MemoryStatus::Active,
        MemoryStatus::Active if row.hit_count >= ACTIVE_TO_VERIFIED_AT => {
            // Age gate: created_at must be ≥ N days old.
            let Ok(created) = chrono::DateTime::parse_from_rfc3339(&row.created_at) else {
                return Ok(()); // unparseable timestamp → skip promotion
            };
            let age_days = (Utc::now() - created.with_timezone(&Utc)).num_days();
            if age_days >= ACTIVE_TO_VERIFIED_AGE_DAYS {
                MemoryStatus::Verified
            } else {
                // Hit-count met but age gate not yet — stay active.
                return Ok(());
            }
        }
        // Candidate below threshold / active below verified threshold
        // / already verified / demoted → no auto-promotion.
        _ => return Ok(()),
    };

    // Transition. Illegal (e.g. somehow already at target) is a
    // benign no-op via `update_status`'s identity transition.
    update_status(pool, memory_id, target, None).await
}

/// 07-06 (am-observability-panel R4 + A3): update an existing
/// memory's `title` + `content`. Reuses [`validate_memory_text`] —
/// the single source of truth for the write safety net (500-char
/// cap + sensitive-content regex + sensitive-path deny-list +
/// temporary-path deny-list + path generalization).
///
/// Side effects:
/// - Sets `edited_by_user = 1` (the provenance marker — D1 design
///   decision; this is the ONLY write that flips the column).
/// - Bumps `updated_at` to `now` (the row's edit timestamp; P5
///   state-machine reads `updated_at` for hygiene-job ordering).
///
/// `tool_name` / `command_pattern` / `path_globs` are NOT editable
/// in R4 (PRD scope) — they're the pitfall's trigger key and are
/// set at insert time. The frontend can re-insert + delete to
/// re-set them in a future PR if needed.
///
/// Returns:
/// - `Ok(MemoryRow)` — the post-update row (with the new
///   `edited_by_user = 1` and `updated_at`).
/// - `Err(MemoryUpdateError::NotFound)` — `memory_id` doesn't
///   exist (frontend should surface a clean "row vanished" error).
/// - `Err(MemoryUpdateError::EmptyTitle/EmptyContent/...)` —
///   safety-net rejection (see [`validate_memory_text`]).
pub async fn update_memory(
    pool: &SqlitePool,
    memory_id: &str,
    title: &str,
    content: &str,
) -> Result<MemoryRow, MemoryUpdateError> {
    // Step 1: run the write safety net (rejects empty / over-length /
    // sensitive / temp-path; generalizes /home/<user>/ to ~/). Same
    // helper as `insert_memory` — single source of truth. The
    // mapping from `MemoryInsertError` to `MemoryUpdateError` is
    // manual (rather than a `From` impl) so the
    // `MemoryInsertError::ProjectScopeMissingId` /
    // `UserScopeHasProjectId` variants are NOT reachable here (the
    // R4 edit path only takes `title` + `content`; the
    // `scope` / `project_id` columns are immutable in this
    // command).
    let (safe_title, safe_content) =
        validate_memory_text(title, content, None, None).map_err(|e| match e {
            MemoryInsertError::EmptyTitle => MemoryUpdateError::EmptyTitle,
            MemoryInsertError::EmptyContent => MemoryUpdateError::EmptyContent,
            MemoryInsertError::TitleTooLong(n) => MemoryUpdateError::TitleTooLong(n),
            MemoryInsertError::ContentTooLong(n) => MemoryUpdateError::ContentTooLong(n),
            MemoryInsertError::SensitiveContent => MemoryUpdateError::SensitiveContent,
            MemoryInsertError::SensitivePath(p) => MemoryUpdateError::SensitivePath(p),
            MemoryInsertError::TemporaryPath(p) => MemoryUpdateError::TemporaryPath(p),
            // ProjectScope / UserScope variants are unreachable
            // here (R4 doesn't take a scope); fold into a DB error
            // for safety.
            other @ MemoryInsertError::ProjectScopeMissingId
            | other @ MemoryInsertError::UserScopeHasProjectId(_) => {
                tracing::error!(
                    error = %other,
                    "update_memory: unexpected scope/project_id safety-net variant"
                );
                MemoryUpdateError::Db(sqlx::Error::Protocol(format!(
                    "unexpected scope variant: {}",
                    other
                )))
            }
            MemoryInsertError::Db(e) => MemoryUpdateError::Db(e),
        })?;

    // Step 2: update the row. `edited_by_user = 1` is the ONLY
    // signature of a user-initiated edit (vs the agent's `remember`
    // tool write or P4 reflection write, both of which leave
    // `edited_by_user` at the default 0). `updated_at` is bumped to
    // the moment of edit.
    let now = Utc::now().to_rfc3339();
    let result = sqlx::query(
        r#"
        UPDATE autonomous_memories
        SET title = ?,
            content = ?,
            edited_by_user = 1,
            updated_at = ?
        WHERE memory_id = ?
        "#,
    )
    .bind(&safe_title)
    .bind(&safe_content)
    .bind(&now)
    .bind(memory_id)
    .execute(pool)
    .await?;

    // Step 3: map a 0-row UPDATE to `NotFound` (defensive — the
    // caller should not pass an unknown `memory_id`, but a race
    // with `delete_autonomous_memory` could land here).
    if result.rows_affected() == 0 {
        return Err(MemoryUpdateError::NotFound(memory_id.to_string()));
    }

    // Step 4: read back the post-update row so the caller (and
    // the IPC) gets the new `updated_at` / `edited_by_user`
    // stamped in one round trip. Falls through the `NotFound`
    // branch above if the row vanished between UPDATE and
    // SELECT (single-writer SQLite makes this near-impossible,
    // but the safety-net contract says production code never
    // `.unwrap()`s).
    get_memory_by_id(pool, memory_id)
        .await?
        .ok_or_else(|| MemoryUpdateError::NotFound(memory_id.to_string()))
}

/// Transition a memory to a new status, wrapped in a transaction.
/// Reads the current status inside the transaction, validates the
/// transition is legal per the state machine (spike-007 §3), then
/// writes the new status + optional `demoted_reason` (set when
/// transitioning TO `demoted`; cleared otherwise).
///
/// Legal transitions:
/// ```text
///   candidate → active | verified | demoted
///   active    → verified | demoted
///   verified  → demoted
///   demoted   → active   (re-promotion via P5 hygiene job)
/// ```
/// All other transitions return `Err(StatusTransitionIllegal)`.
///
/// P1 provides the interface; P5 wires the auto-promotion rules.
/// The transaction ensures a concurrent `bump_hit_count` can't
/// race the status read (SQLite serializes writers under the
/// default rollback-journal mode).
#[derive(Debug, thiserror::Error)]
pub enum StatusTransitionError {
    #[error("memory {0} not found")]
    NotFound(String),
    #[error("illegal transition: {from} -> {to}")]
    Illegal {
        from: &'static str,
        to: &'static str,
    },
    #[error("DB error: {0}")]
    Db(#[from] sqlx::Error),
}

pub async fn update_status(
    pool: &SqlitePool,
    memory_id: &str,
    new_status: MemoryStatus,
    demoted_reason: Option<&str>,
) -> Result<(), StatusTransitionError> {
    let mut tx = pool.begin().await?;

    // Read current status inside the transaction.
    let current_str: Option<String> =
        sqlx::query_scalar("SELECT status FROM autonomous_memories WHERE memory_id = ?")
            .bind(memory_id)
            .fetch_optional(&mut *tx)
            .await?;
    let current_str =
        current_str.ok_or_else(|| StatusTransitionError::NotFound(memory_id.to_string()))?;
    let current = MemoryStatus::from_str_opt(&current_str);

    // Validate the transition.
    let legal = match (current, new_status) {
        // Identity is always legal (idempotent re-promotion).
        (a, b) if a == b => true,
        (MemoryStatus::Candidate, MemoryStatus::Active) => true,
        (MemoryStatus::Candidate, MemoryStatus::Verified) => true,
        (MemoryStatus::Candidate, MemoryStatus::Demoted) => true,
        (MemoryStatus::Active, MemoryStatus::Verified) => true,
        (MemoryStatus::Active, MemoryStatus::Demoted) => true,
        (MemoryStatus::Verified, MemoryStatus::Demoted) => true,
        (MemoryStatus::Demoted, MemoryStatus::Active) => true,
        _ => false,
    };
    if !legal {
        return Err(StatusTransitionError::Illegal {
            from: current.as_str(),
            to: new_status.as_str(),
        });
    }

    let now = Utc::now().to_rfc3339();
    // demoted_reason: set when transitioning TO demoted (and a
    // reason was supplied); clear when transitioning AWAY from
    // demoted (re-promotion). For non-demoted transitions where the
    // caller passed a reason, we ignore it (the column is for the
    // demoted state only).
    let reason_to_write: Option<&str> = if new_status == MemoryStatus::Demoted {
        demoted_reason
    } else {
        None
    };

    sqlx::query(
        r#"
        UPDATE autonomous_memories
        SET status = ?,
            demoted_reason = ?,
            updated_at = ?
        WHERE memory_id = ?
        "#,
    )
    .bind(new_status.as_str())
    .bind(reason_to_write)
    .bind(&now)
    .bind(memory_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers (test-only)
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::*;

    /// Direct row builder for tests that need to bypass the write
    /// safety net (e.g. to insert a memory with sensitive content
    /// to verify the FTS trigger or to test the trigger directly).
    /// Production code MUST use [`insert_memory`].
    #[allow(dead_code)]
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_raw(
        pool: &SqlitePool,
        memory_id: &str,
        scope: MemoryScope,
        project_id: Option<&str>,
        kind: MemoryKind,
        status: MemoryStatus,
        title: &str,
        content: &str,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO autonomous_memories
            (memory_id, scope, project_id, kind, status, title, content, tags,
             created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, '[]', ?, ?)
            "#,
        )
        .bind(memory_id)
        .bind(scope.as_str())
        .bind(project_id)
        .bind(kind.as_str())
        .bind(status.as_str())
        .bind(title)
        .bind(content)
        .bind(&now)
        .bind(&now)
        .execute(pool)
        .await?;
        Ok(())
    }
}
