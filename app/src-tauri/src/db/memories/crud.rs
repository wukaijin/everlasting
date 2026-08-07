//! CRUD: insert / list / delete / get / count.
//!
//! Relocated verbatim from the pre-split `memories.rs`.

use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;

use super::types::{MemoryInput, MemoryInsertError, MemoryKind, MemoryRow, MemoryScope};
use super::validation::apply_safety_net;

// ---------------------------------------------------------------------------
// CRUD: insert / list / delete
// ---------------------------------------------------------------------------

/// Insert a new memory row. Applies the write safety net (§4) before
/// the INSERT: empty/over-length/sensitive-content/sensitive-path/
/// temporary-path are rejected with a typed error; `/home/<user>/`
/// is generalized to `~/`. The FTS5 sync trigger (migration PR1b)
/// keeps the FTS index in sync automatically — no manual FTS write.
///
/// `memory_id` is generated as UUID v7 (time-ordered, B-tree
/// friendly, RFC 9562). A UNIQUE collision returns `Err` (UUIDv7
/// collision probability is astronomically low; we do NOT upsert).
///
/// **scope/project_id interaction (H2)**:
/// - `scope=User` → `project_id` MUST be `None` (rejected otherwise;
///   a user-scope memory is global to the user, not project-bound).
/// - `scope=Project` → `project_id` MUST be `Some(_)` (rejected
///   otherwise; a project memory without a project is meaningless).
pub async fn insert_memory(
    pool: &SqlitePool,
    input: &MemoryInput,
) -> Result<MemoryRow, MemoryInsertError> {
    // scope/project_id interaction (H2).
    match (input.scope, &input.project_id) {
        (MemoryScope::User, Some(id)) => {
            return Err(MemoryInsertError::UserScopeHasProjectId(id.clone()));
        }
        (MemoryScope::Project, None) => {
            return Err(MemoryInsertError::ProjectScopeMissingId);
        }
        _ => {}
    }

    // Write safety net (§4).
    let (title, content) = apply_safety_net(input)?;

    let memory_id = Uuid::now_v7().to_string();
    let now = Utc::now().to_rfc3339();
    // `edited_by_user` defaults to 0 (false) for agent-written
    // memories; only `update_memory` flips it to 1 (the user-edit
    // trail per D1).
    sqlx::query(
        r#"
        INSERT INTO autonomous_memories
        (memory_id, scope, project_id, kind, status, title, content, tags,
         tool_name, command_pattern, path_globs, source_session_id, source_ref,
         confidence, hit_count, last_used_at, created_at, updated_at, demoted_reason,
         edited_by_user)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0.5, 0, NULL, ?, ?, NULL, 0)
        "#,
    )
    .bind(&memory_id)
    .bind(input.scope.as_str())
    .bind(&input.project_id)
    .bind(input.kind.as_str())
    .bind(input.status.as_str())
    .bind(&title)
    .bind(&content)
    .bind(&input.tags)
    .bind(&input.tool_name)
    .bind(&input.command_pattern)
    .bind(&input.path_globs)
    .bind(&input.source_session_id)
    .bind(&input.source_ref)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;

    // Read back to return the full row (auto-id + timestamps).
    // Defensive: a concurrent `delete_memory` between our INSERT and
    // this readback could make the row vanish (single-writer SQLite
    // makes this near-impossible, but the safety-net contract says
    // production code never `.unwrap()`s / `.expect()`s on a DB
    // result). Map a missing row to `sqlx::Error::RowNotFound`, which
    // `#[from]` lifts into `MemoryInsertError::Db` — the caller gets
    // a typed error instead of a panic. Mirrors the defensive no-op
    // pattern used by `record_message_resend_audit` /
    // `record_tool_duration`.
    let row = get_memory_by_id(pool, &memory_id)
        .await?
        .ok_or_else(|| sqlx::Error::RowNotFound)?;

    // P5 hygiene event trigger (design D4 / §6): every Nth insert in
    // this `(scope, kind)` bucket kicks a fire-and-forget
    // dedup-merge + age-out pass. The COUNT is cheap (small table);
    // the `spawn` keeps the insert path sync-fast. Best-effort — a
    // spawn failure just delays cleanup to the next tick or the
    // startup pass. `pool.clone()` is Arc-internal (cheap).
    //
    // `cfg!(test)` guard: the spawn is a fire-and-forget side effect
    // that would make insert-driven tests flaky (the async hygiene
    // task could dedup/delete rows the test then counts). The guard
    // keeps the code path compiled (so `count_memories_by_scope_kind`
    // stays reachable in test builds) but skips the spawn at runtime
    // under `cargo test`. Production builds run the trigger.
    if !cfg!(test) {
        const HYGIENE_TRIGGER_EVERY: i64 = 10;
        let bucket_count = count_memories_by_scope_kind(pool, input.scope, input.kind).await;
        if bucket_count > 0 && bucket_count % HYGIENE_TRIGGER_EVERY == 0 {
            tokio::spawn(crate::agent::memory_hygiene::run_hygiene_pass(pool.clone()));
        }
    }

    Ok(row)
}

/// Fetch a single row by `memory_id` (the UUID, not the auto-id).
/// Used internally by `insert_memory` to read back the full row;
/// exposed for P2's future "fetch single memory" IPC.
#[allow(dead_code)]
pub async fn get_memory_by_id(
    pool: &SqlitePool,
    memory_id: &str,
) -> Result<Option<MemoryRow>, sqlx::Error> {
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
    Ok(row)
}

/// List memories, optionally filtered by scope and/or project_id.
/// Used by P2's frontend memory browser (the MemoryPreview list).
///
/// **scope/project_id interaction (H2)** — same semantics as
/// `search_memories_fts`:
/// - `(Some(User), _)` → only user-scope rows (project_id ignored).
/// - `(Some(Project), None)` → Err (project query needs an id).
/// - `(Some(Project), Some(id))` → only that project's rows.
/// - `(None, _)` → all rows (both scopes); project_id is ignored.
///
/// Ordered by `created_at DESC` (newest first) — matches the UI
/// convention for list endpoints.
pub async fn list_memories(
    pool: &SqlitePool,
    scope: Option<MemoryScope>,
    project_id: Option<&str>,
) -> Result<Vec<MemoryRow>, MemoryInsertError> {
    // Validate scope/project_id interaction up-front (mirrors
    // search_memories_fts). User scope ignores project_id; Project
    // scope requires project_id.
    if let Some(MemoryScope::Project) = scope {
        if project_id.is_none() {
            return Err(MemoryInsertError::ProjectScopeMissingId);
        }
    }

    let rows = match scope {
        Some(MemoryScope::User) => {
            sqlx::query_as::<_, MemoryRow>(
                r#"
                SELECT id, memory_id, scope, project_id, kind, status, title, content,
                       tags, tool_name, command_pattern, path_globs, source_session_id,
                       source_ref, confidence, hit_count, last_used_at, created_at,
                       updated_at, demoted_reason, edited_by_user
                FROM autonomous_memories
                WHERE scope = 'user'
                ORDER BY created_at DESC
                "#,
            )
            .fetch_all(pool)
            .await?
        }
        Some(MemoryScope::Project) => {
            sqlx::query_as::<_, MemoryRow>(
                r#"
                SELECT id, memory_id, scope, project_id, kind, status, title, content,
                       tags, tool_name, command_pattern, path_globs, source_session_id,
                       source_ref, confidence, hit_count, last_used_at, created_at,
                       updated_at, demoted_reason, edited_by_user
                FROM autonomous_memories
                WHERE scope = 'project' AND project_id = ?
                ORDER BY created_at DESC
                "#,
            )
            .bind(project_id)
            .fetch_all(pool)
            .await?
        }
        None => {
            sqlx::query_as::<_, MemoryRow>(
                r#"
                SELECT id, memory_id, scope, project_id, kind, status, title, content,
                       tags, tool_name, command_pattern, path_globs, source_session_id,
                       source_ref, confidence, hit_count, last_used_at, created_at,
                       updated_at, demoted_reason, edited_by_user
                FROM autonomous_memories
                ORDER BY created_at DESC
                "#,
            )
            .fetch_all(pool)
            .await?
        }
    };
    Ok(rows)
}

/// Delete a memory by `memory_id`. The FTS5 sync trigger
/// (`am_fts_delete`) removes the row's FTS index entries
/// automatically. Returns the number of rows deleted (0 if the
/// memory_id didn't exist — caller decides whether that's an error).
pub async fn delete_memory(pool: &SqlitePool, memory_id: &str) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("DELETE FROM autonomous_memories WHERE memory_id = ?")
        .bind(memory_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

/// Count memories attributable to a session via `source_session_id`.
/// Used by P2's `remember` tool frequency control (spike-005 §4.3
/// "same session ≤ 50" rule). The count covers ALL statuses (a
/// demoted row still occupies a slot — pruning is a separate concern).
///
/// Best-effort + cheap: one `COUNT(*) WHERE source_session_id = ?`
/// (no dedicated index — the table is small; full scan is
/// microseconds). Returns 0 on any error (frequency control is a
/// soft guard — a DB hiccup shouldn't block a legitimate write;
/// the worst case is one extra row over the cap, which the next
/// hygiene job / manual delete fixes).
pub async fn count_memories_for_session(pool: &SqlitePool, session_id: &str) -> i64 {
    let count: Option<i64> =
        sqlx::query_scalar("SELECT COUNT(*) FROM autonomous_memories WHERE source_session_id = ?")
            .bind(session_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    count.unwrap_or(0)
}

/// P5 hygiene trigger helper: count rows in a `(scope, kind)` bucket.
/// Used by [`insert_memory`] to fire a fire-and-forget hygiene pass
/// every Nth insert per bucket (design D4 / §6) — amortising the
/// dedup + age-out cost across writes instead of polling on an
/// interval. Same best-effort + cheap shape as
/// [`count_memories_for_session`] (returns 0 on error; the trigger is
/// a soft guard, never blocks a write).
pub async fn count_memories_by_scope_kind(
    pool: &SqlitePool,
    scope: MemoryScope,
    kind: MemoryKind,
) -> i64 {
    let count: Option<i64> =
        sqlx::query_scalar("SELECT COUNT(*) FROM autonomous_memories WHERE scope = ? AND kind = ?")
            .bind(scope.as_str())
            .bind(kind.as_str())
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    count.unwrap_or(0)
}
