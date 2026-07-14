//! E2 (harness trace pipeline, 2026-07-14) — `turn_trace` table CRUD.
//!
//! One row per `(session_id, seq)` pair. Each trace dimension
//! (token_usage / compaction / loop_hint / breadcrumb) is written
//! by a separate UPSERT that touches only its column, leaving the
//! others untouched. The `UNIQUE(session_id, seq)` constraint is
//! the UPSERT anchor.
//!
//! All functions return `Result<T, sqlx::Error>` (no logging) so
//! the caller decides how to surface the error — the trace
//! pipeline (`agent::trace.rs`) wraps each call in `warn!` and
//! swallows, matching the `record_*_audit` best-effort contract.

use serde::Serialize;
use sqlx::{Row, SqlitePool};

use crate::llm::types::TokenUsage;

// ---------------------------------------------------------------------------
// TurnTraceRow — IPC payload for list_turn_traces
// ---------------------------------------------------------------------------

/// One row of the `turn_trace` table. Returned by `list_turn_traces`
/// and serialized across the IPC boundary as camelCase (per
/// database-guidelines.md, matching `AuditEventRow`).
///
/// Each `*_json` field is the raw JSON text stored in the column;
/// the frontend parses it per dimension. `None` means that dimension
/// was never written for this turn (e.g. a turn with no C3
/// compaction has `compaction_json = None`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnTraceRow {
    pub id: i64,
    pub session_id: String,
    pub seq: i64,
    pub token_usage_json: Option<String>,
    pub compaction_json: Option<String>,
    pub loop_hint_json: Option<String>,
    pub breadcrumb_json: Option<String>,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// UPSERT helpers — one per trace dimension
// ---------------------------------------------------------------------------

/// Upsert the `token_usage_json` column for `(session_id, seq)`.
/// Called from the agent loop's `Done { usage }` arm, right after
/// `update_last_turn_usage` (inside the `!skip_persist` gate so
/// worker turns don't pollute the parent's trace rows).
///
/// The JSON payload is the serialized `TokenUsage` (5-field shape
/// mirroring `ChatEvent::Done.usage`).
pub async fn upsert_turn_trace_token(
    pool: &SqlitePool,
    session_id: &str,
    seq: i64,
    usage: &TokenUsage,
) -> Result<(), sqlx::Error> {
    let json = serde_json::to_string(usage).unwrap_or_else(|_| "{}".to_string());
    sqlx::query(
        r#"
        INSERT INTO turn_trace (session_id, seq, token_usage_json)
        VALUES (?, ?, ?)
        ON CONFLICT(session_id, seq)
        DO UPDATE SET token_usage_json = excluded.token_usage_json
        "#,
    )
    .bind(session_id)
    .bind(seq)
    .bind(&json)
    .execute(pool)
    .await?;
    Ok(())
}

/// Upsert the `compaction_json` column for `(session_id, seq)`.
/// Called from `agent::trace::record_compaction` at the C3
/// compaction write point (both normal and `StillOver` branches).
///
/// The JSON payload shape: `{"tokens_before", "tokens_after",
/// "dropped_count", "degradation"}`.
pub async fn upsert_turn_trace_compaction(
    pool: &SqlitePool,
    session_id: &str,
    seq: i64,
    payload: &serde_json::Value,
) -> Result<(), sqlx::Error> {
    let json = payload.to_string();
    sqlx::query(
        r#"
        INSERT INTO turn_trace (session_id, seq, compaction_json)
        VALUES (?, ?, ?)
        ON CONFLICT(session_id, seq)
        DO UPDATE SET compaction_json = excluded.compaction_json
        "#,
    )
    .bind(session_id)
    .bind(seq)
    .bind(&json)
    .execute(pool)
    .await?;
    Ok(())
}

/// Upsert the `loop_hint_json` column for `(session_id, seq)`.
/// Called from `agent::trace::record_loop_hint` at the C2 soft-hint
/// write point (1-2 consecutive hits, before the ≥3 intervention).
///
/// The JSON payload shape: `{"hit_count", "verdict_kind"}`.
pub async fn upsert_turn_trace_loop_hint(
    pool: &SqlitePool,
    session_id: &str,
    seq: i64,
    payload: &serde_json::Value,
) -> Result<(), sqlx::Error> {
    let json = payload.to_string();
    sqlx::query(
        r#"
        INSERT INTO turn_trace (session_id, seq, loop_hint_json)
        VALUES (?, ?, ?)
        ON CONFLICT(session_id, seq)
        DO UPDATE SET loop_hint_json = excluded.loop_hint_json
        "#,
    )
    .bind(session_id)
    .bind(seq)
    .bind(&json)
    .execute(pool)
    .await?;
    Ok(())
}

/// Upsert the `breadcrumb_json` column for `(session_id, seq)`.
/// Called from `agent::trace::record_breadcrumb` at the workflow
/// breadcrumb injection write point.
///
/// The JSON payload shape: `{"task_slug", "status",
/// "breadcrumb_text"}`.
pub async fn upsert_turn_trace_breadcrumb(
    pool: &SqlitePool,
    session_id: &str,
    seq: i64,
    payload: &serde_json::Value,
) -> Result<(), sqlx::Error> {
    let json = payload.to_string();
    sqlx::query(
        r#"
        INSERT INTO turn_trace (session_id, seq, breadcrumb_json)
        VALUES (?, ?, ?)
        ON CONFLICT(session_id, seq)
        DO UPDATE SET breadcrumb_json = excluded.breadcrumb_json
        "#,
    )
    .bind(session_id)
    .bind(seq)
    .bind(&json)
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Read + clear
// ---------------------------------------------------------------------------

/// Read all turn_trace rows for `session_id`, ordered by `seq ASC`
/// (chronological). Wired to the `list_turn_traces` Tauri command
/// for the trace viewer's 回看 mode. Empty / missing session
/// returns an empty `Vec` (NOT an error).
pub async fn list_turn_traces(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Vec<TurnTraceRow>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT id, session_id, seq, token_usage_json, compaction_json,
               loop_hint_json, breadcrumb_json, created_at
        FROM turn_trace
        WHERE session_id = ?
        ORDER BY seq ASC
        "#,
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(TurnTraceRow {
                id: r.try_get("id")?,
                session_id: r.try_get("session_id")?,
                seq: r.try_get("seq")?,
                token_usage_json: r.try_get("token_usage_json")?,
                compaction_json: r.try_get("compaction_json")?,
                loop_hint_json: r.try_get("loop_hint_json")?,
                breadcrumb_json: r.try_get("breadcrumb_json")?,
                created_at: r.try_get("created_at")?,
            })
        })
        .collect()
}

/// Delete all `turn_trace` rows for `session_id`. Wired to the
/// `clear_session_trace` Tauri command. The `ON DELETE CASCADE`
/// on the `session_id` FK also fires this automatically when a
/// session is deleted, so this command is for the manual "清理"
/// button only.
pub async fn clear_session_trace(pool: &SqlitePool, session_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM turn_trace WHERE session_id = ?")
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations::run_migrations;
    use crate::db::sessions::create_session;
    use crate::llm::types::TokenUsage;
    use sqlx::SqlitePool;
    use uuid::Uuid;

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        run_migrations(&pool).await.unwrap();
        pool
    }

    async fn seed_session(pool: &SqlitePool) -> String {
        let row = create_session(
            pool,
            &Uuid::new_v4().to_string(),
            crate::projects::DEFAULT_PROJECT_ID,
            "/tmp",
            "GLM-4.7",
            None,
        )
        .await
        .unwrap();
        row.id
    }

    #[tokio::test]
    async fn upsert_accumulates_columns_across_writes() {
        // Multiple upserts to the same (session_id, seq) should merge
        // into a single row, each touching only its column.
        let pool = test_pool().await;
        let sid = seed_session(&pool).await;

        // Write token_usage for seq=1.
        let usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: 10,
            cache_read_input_tokens: 20,
            context_input_tokens: 130,
        };
        upsert_turn_trace_token(&pool, &sid, 1, &usage)
            .await
            .unwrap();

        // Write compaction for seq=1.
        let compaction = serde_json::json!({
            "tokens_before": 5000,
            "tokens_after": 3000,
            "dropped_count": 5,
            "degradation": "none"
        });
        upsert_turn_trace_compaction(&pool, &sid, 1, &compaction)
            .await
            .unwrap();

        // Write loop_hint for seq=1.
        let hint = serde_json::json!({"hit_count": 2, "verdict_kind": "soft"});
        upsert_turn_trace_loop_hint(&pool, &sid, 1, &hint)
            .await
            .unwrap();

        // Write breadcrumb for seq=1.
        let bc = serde_json::json!({
            "task_slug": "my-task",
            "status": "in_progress",
            "breadcrumb_text": "<workflow-task-meta>..."
        });
        upsert_turn_trace_breadcrumb(&pool, &sid, 1, &bc)
            .await
            .unwrap();

        // Verify: exactly 1 row, all 4 columns populated.
        let rows = list_turn_traces(&pool, &sid).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].seq, 1);
        assert!(rows[0].token_usage_json.is_some());
        assert!(rows[0].compaction_json.is_some());
        assert!(rows[0].loop_hint_json.is_some());
        assert!(rows[0].breadcrumb_json.is_some());

        // Verify the JSON content round-trips.
        let token_json: serde_json::Value =
            serde_json::from_str(rows[0].token_usage_json.as_deref().unwrap()).unwrap();
        assert_eq!(token_json["input_tokens"], 100);
        assert_eq!(token_json["context_input_tokens"], 130);

        let compaction_json: serde_json::Value =
            serde_json::from_str(rows[0].compaction_json.as_deref().unwrap()).unwrap();
        assert_eq!(compaction_json["dropped_count"], 5);
    }

    #[tokio::test]
    async fn list_turn_traces_returns_rows_ordered_by_seq_asc() {
        let pool = test_pool().await;
        let sid = seed_session(&pool).await;

        // Write out of order (seq=3, 1, 2).
        for seq in [3i64, 1, 2] {
            let usage = TokenUsage::default();
            upsert_turn_trace_token(&pool, &sid, seq, &usage)
                .await
                .unwrap();
        }

        let rows = list_turn_traces(&pool, &sid).await.unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].seq, 1);
        assert_eq!(rows[1].seq, 2);
        assert_eq!(rows[2].seq, 3);
    }

    #[tokio::test]
    async fn clear_session_trace_deletes_all_rows() {
        let pool = test_pool().await;
        let sid = seed_session(&pool).await;

        let usage = TokenUsage::default();
        upsert_turn_trace_token(&pool, &sid, 1, &usage)
            .await
            .unwrap();
        upsert_turn_trace_token(&pool, &sid, 2, &usage)
            .await
            .unwrap();

        assert_eq!(list_turn_traces(&pool, &sid).await.unwrap().len(), 2);

        clear_session_trace(&pool, &sid).await.unwrap();

        assert_eq!(list_turn_traces(&pool, &sid).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn upsert_overwrites_same_column_on_conflict() {
        // A second write to the same column should overwrite, not
        // duplicate the row.
        let pool = test_pool().await;
        let sid = seed_session(&pool).await;

        let usage1 = TokenUsage {
            input_tokens: 100,
            ..Default::default()
        };
        upsert_turn_trace_token(&pool, &sid, 1, &usage1)
            .await
            .unwrap();

        let usage2 = TokenUsage {
            input_tokens: 200,
            ..Default::default()
        };
        upsert_turn_trace_token(&pool, &sid, 1, &usage2)
            .await
            .unwrap();

        let rows = list_turn_traces(&pool, &sid).await.unwrap();
        assert_eq!(
            rows.len(),
            1,
            "second upsert must not create a duplicate row"
        );
        let json: serde_json::Value =
            serde_json::from_str(rows[0].token_usage_json.as_deref().unwrap()).unwrap();
        assert_eq!(
            json["input_tokens"], 200,
            "second write must overwrite the first"
        );
    }

    #[tokio::test]
    async fn list_turn_traces_empty_for_missing_session() {
        let pool = test_pool().await;
        let rows = list_turn_traces(&pool, "nonexistent-session")
            .await
            .unwrap();
        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn turn_trace_cascades_on_session_delete() {
        // ON DELETE CASCADE: deleting the session should clean up
        // turn_trace rows automatically.
        let pool = test_pool().await;
        let sid = seed_session(&pool).await;

        let usage = TokenUsage::default();
        upsert_turn_trace_token(&pool, &sid, 1, &usage)
            .await
            .unwrap();
        assert_eq!(list_turn_traces(&pool, &sid).await.unwrap().len(), 1);

        crate::db::delete_session(&pool, &sid).await.unwrap();

        assert_eq!(list_turn_traces(&pool, &sid).await.unwrap().len(), 0);
    }
}
