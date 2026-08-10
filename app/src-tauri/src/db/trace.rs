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
// Group-chat cache rates (08-10-group-chat-cache-rate) — read side
// ---------------------------------------------------------------------------

/// One speaker's latest-turn cache-usage numbers for the group-chat
/// cache-rate display. `cache_read` / `context_input` are the raw
/// `cache_read_input_tokens` / `context_input_tokens` of that
/// speaker's MOST RECENT assistant turn that carried token usage
/// (max-seq semantics — single-turn, not aggregated). The frontend
/// computes the percentage (`cache_read / context_input`) and
/// renders "—" for `context_input = 0` (legacy rows that predate
/// the 2026-06-26 `context_input_tokens` field — see `COALESCE`
/// below). Serialized snake_case on the wire, matching the
/// frontend `SpeakerCacheUsage` contract in design.md.
#[derive(Debug, Clone, Serialize)]
pub struct SpeakerCacheUsage {
    pub speaker: String,
    pub cache_read: u32,
    pub context_input: u32,
}

/// Per-speaker latest-turn cache-usage read for group-chat sessions.
///
/// Derived data — zero new storage: `turn_trace(session_id, seq,
/// token_usage_json)` joined to `messages(session_id, seq, role,
/// speaker)`. Semantics (all pre-existing facts, this query only
/// projects them):
///   - assistant message rows carry `seq ==` the turn's seq
///     (`drive.rs` pushes with the current seq), so the join does
///     not skew.
///   - group-chat seq is globally contiguous (`init.rs` starts each
///     call at DB `max(seq)+1`), so each speaker's `max(seq)`
///     assistant row IS their latest turn.
///   - `role = 'assistant'` filters out rewrite products (user
///     rows that carry a speaker).
///   - `m.speaker IS NOT NULL` excludes classic chat / worker rows
///     (their speaker is NULL).
///   - `t.token_usage_json IS NOT NULL` skips turns with no usage
///     (cancel / error / trace rows written for other dimensions).
///   - retries overwrite `turn_trace` by `(session_id, seq)`
///     (`upsert_turn_trace_token`), so the last usage wins.
///
/// `COALESCE(json_extract(...), 0)`: a legacy 4-field usage JSON
/// (pre-2026-06-26, no `context_input_tokens`) is returned as
/// `context_input = 0` instead of dropping the row — the frontend
/// decides the display ("—" via `cacheRatePercent`), per design.
pub async fn list_speaker_cache_usage(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Vec<SpeakerCacheUsage>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT m.speaker,
               COALESCE(json_extract(t.token_usage_json, '$.cache_read_input_tokens'), 0) AS cache_read,
               COALESCE(json_extract(t.token_usage_json, '$.context_input_tokens'), 0)   AS context_input
        FROM messages m
        JOIN turn_trace t ON t.session_id = m.session_id AND t.seq = m.seq
        WHERE m.session_id = ?
          AND m.role = 'assistant'
          AND m.speaker IS NOT NULL
          AND t.token_usage_json IS NOT NULL
          AND m.seq = (
              SELECT MAX(m2.seq) FROM messages m2
              WHERE m2.session_id = m.session_id
                AND m2.speaker = m.speaker
                AND m2.role = 'assistant'
          )
        "#,
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(SpeakerCacheUsage {
                speaker: r.try_get("speaker")?,
                cache_read: r.try_get("cache_read")?,
                context_input: r.try_get("context_input")?,
            })
        })
        .collect()
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
    use crate::db::sessions::{create_session, persist_turn};
    use crate::llm::types::{MessageContent, Role, TokenUsage};
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
            None,
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

    // -------------------------------------------------------------------
    // list_speaker_cache_usage (08-10-group-chat-cache-rate)
    // -------------------------------------------------------------------

    /// Raw `token_usage_json` INSERT helper for the cache-rate tests
    /// — `upsert_turn_trace_token` always writes the full 5-field
    /// shape, but the NULL (usage-less turn) and legacy 4-field
    /// (missing `context_input_tokens`) cases need raw SQL.
    async fn insert_trace_token_json(pool: &SqlitePool, sid: &str, seq: i64, json: Option<&str>) {
        match json {
            Some(j) => {
                sqlx::query(
                    r#"
                    INSERT INTO turn_trace (session_id, seq, token_usage_json)
                    VALUES (?, ?, ?)
                    ON CONFLICT(session_id, seq)
                    DO UPDATE SET token_usage_json = excluded.token_usage_json
                    "#,
                )
                .bind(sid)
                .bind(seq)
                .bind(j)
                .execute(pool)
                .await
                .unwrap();
            }
            None => {
                sqlx::query(
                    r#"
                    INSERT INTO turn_trace (session_id, seq)
                    VALUES (?, ?)
                    ON CONFLICT(session_id, seq)
                    DO UPDATE SET token_usage_json = NULL
                    "#,
                )
                .bind(sid)
                .bind(seq)
                .execute(pool)
                .await
                .unwrap();
            }
        }
    }

    /// Group-chat fixture: one session whose messages + turn_trace
    /// rows exercise every filter of `list_speaker_cache_usage`:
    ///
    /// | seq | role      | speaker    | trace JSON                     | expected in result |
    /// |-----|-----------|------------|--------------------------------|--------------------|
    /// | 0   | user      | (none)     | —                              | no (role filter)   |
    /// | 1   | assistant | moderator  | full, cache_read=100, ctx=1000 | no (not latest)    |
    /// | 2   | assistant | Alice      | full, cache_read=20, ctx=200   | yes                |
    /// | 3   | assistant | moderator  | full, cache_read=50, ctx=500   | yes (latest)       |
    /// | 4   | assistant | Bob        | (no turn_trace row at all)     | no (JOIN miss)     |
    /// | 5   | assistant | Bob        | token_usage_json = NULL        | no (NULL usage)    |
    /// | 6   | assistant | Bob        | legacy 4-field, cache_read=60  | yes (ctx=0)        |
    /// | 7   | assistant | (none)     | full, cache_read=999, ctx=9999 | no (speaker NULL)  |
    /// | 8   | assistant | Carol      | token_usage_json = NULL        | no (only turn, no usage) |
    /// | 9   | assistant | Dave       | full, cache_read=70, ctx=700   | no (not latest)    |
    /// | 10  | assistant | Dave       | token_usage_json = NULL        | no (latest turn has no usage — NO fallback to seq 9) |
    ///
    /// The max-seq subquery considers every assistant turn of the
    /// speaker (design.md SQL verbatim), THEN the
    /// `token_usage_json IS NOT NULL` filter applies — so a speaker
    /// whose most recent assistant turn lacks usage is absent from
    /// the result (frontend renders "—"), even when an older turn
    /// carried usage. Dave locks in that exact behavior: his seq 9
    /// turn HAS usage, but his latest assistant turn (seq 10) does
    /// not, so he is absent — the query does NOT fall back to the
    /// older usage-bearing turn. (Carol, whose only turn has no
    /// usage, would be absent under either interpretation; Dave's
    /// two-turn shape is what distinguishes the design SQL from a
    /// "max-seq over usage-bearing turns" variant.)
    async fn seed_cache_usage_fixture(pool: &SqlitePool, sid: &str) {
        let prompt = MessageContent::Text("prompt".to_string());
        persist_turn(pool, sid, Role::User, &prompt, 0, None, None)
            .await
            .unwrap();
        for (seq, speaker) in [
            (1i64, "moderator"),
            (2, "Alice"),
            (3, "moderator"),
            (4, "Bob"),
            (5, "Bob"),
            (6, "Bob"),
            (7, "nobody"),
            (8, "Carol"),
            (9, "Dave"),
            (10, "Dave"),
        ] {
            let content = MessageContent::Text(format!("turn {}", seq));
            let speaker_opt = if speaker == "nobody" {
                None
            } else {
                Some(speaker)
            };
            persist_turn(pool, sid, Role::Assistant, &content, seq, None, speaker_opt)
                .await
                .unwrap();
        }
        insert_trace_token_json(
            pool,
            sid,
            1,
            Some(r#"{"input_tokens":100,"output_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":100,"context_input_tokens":1000}"#),
        )
        .await;
        insert_trace_token_json(
            pool,
            sid,
            2,
            Some(r#"{"input_tokens":200,"output_tokens":20,"cache_creation_input_tokens":0,"cache_read_input_tokens":20,"context_input_tokens":200}"#),
        )
        .await;
        insert_trace_token_json(
            pool,
            sid,
            3,
            Some(r#"{"input_tokens":500,"output_tokens":50,"cache_creation_input_tokens":0,"cache_read_input_tokens":50,"context_input_tokens":500}"#),
        )
        .await;
        // seq 4: no turn_trace row at all (turn had no usage write).
        // seq 5: usage-less turn (cancel / error path) — skipped.
        insert_trace_token_json(pool, sid, 5, None).await;
        // seq 6: legacy 4-field JSON — `context_input_tokens` absent.
        insert_trace_token_json(
            pool,
            sid,
            6,
            Some(r#"{"input_tokens":90,"output_tokens":9,"cache_creation_input_tokens":0,"cache_read_input_tokens":60}"#),
        )
        .await;
        insert_trace_token_json(
            pool,
            sid,
            7,
            Some(r#"{"input_tokens":999,"output_tokens":99,"cache_creation_input_tokens":0,"cache_read_input_tokens":999,"context_input_tokens":9999}"#),
        )
        .await;
        // seq 8: Carol's only assistant turn has NULL usage → she is
        // absent from the result ("—" placeholder on the frontend).
        insert_trace_token_json(pool, sid, 8, None).await;
        // seq 9: Dave's older turn HAS usage (cache_read=70,
        // ctx=700) — seq 10 below shadows it. If the query fell
        // back to the latest USAGE-bearing turn, Dave would appear
        // with 70/700; the design SQL (max-seq over ALL assistant
        // turns, then usage filter) must drop him instead.
        insert_trace_token_json(
            pool,
            sid,
            9,
            Some(r#"{"input_tokens":700,"output_tokens":70,"cache_creation_input_tokens":0,"cache_read_input_tokens":70,"context_input_tokens":700}"#),
        )
        .await;
        // seq 10: Dave's latest assistant turn has NULL usage (cancel
        // / error path) → he must be ABSENT, not reverted to seq 9.
        insert_trace_token_json(pool, sid, 10, None).await;
    }

    fn find_by_speaker<'a>(rows: &'a [SpeakerCacheUsage], speaker: &str) -> &'a SpeakerCacheUsage {
        rows.iter()
            .find(|r| r.speaker == speaker)
            .unwrap_or_else(|| panic!("missing speaker {} in {:?}", speaker, rows))
    }

    #[tokio::test]
    async fn list_speaker_cache_usage_returns_latest_usage_turn_per_speaker() {
        let pool = test_pool().await;
        let sid = seed_session(&pool).await;
        seed_cache_usage_fixture(&pool, &sid).await;

        let rows = list_speaker_cache_usage(&pool, &sid).await.unwrap();

        // Exactly 3 speakers qualify: moderator / Alice / Bob.
        assert_eq!(rows.len(), 3, "rows: {:?}", rows);

        // Moderator: max-seq assistant turn is seq 3 (seq 1 is older).
        let moderator_usage = find_by_speaker(&rows, "moderator");
        assert_eq!(moderator_usage.cache_read, 50);
        assert_eq!(moderator_usage.context_input, 500);

        // Alice: single usage turn at seq 2 — returned as-is.
        let alice = find_by_speaker(&rows, "Alice");
        assert_eq!(alice.cache_read, 20);
        assert_eq!(alice.context_input, 200);

        // Bob: seq 4 has no trace row and seq 5 has NULL usage —
        // both skipped; his latest usable turn is the legacy
        // 4-field JSON at seq 6 → returned with context_input = 0
        // (frontend decides the "—" display).
        let bob = find_by_speaker(&rows, "Bob");
        assert_eq!(bob.cache_read, 60);
        assert_eq!(bob.context_input, 0);

        // Carol's only assistant turn (seq 8) has NULL usage → the
        // design SQL's max-seq + usage filter drops her entirely
        // (frontend renders "—").
        assert!(
            rows.iter().all(|r| r.speaker != "Carol"),
            "speaker whose latest turn has no usage must be absent: {:?}",
            rows
        );

        // Dave's older turn (seq 9) HAS usage but his latest
        // assistant turn (seq 10) has none → he is absent too: the
        // max-seq is over ALL assistant turns, with the usage filter
        // applied AFTER — no fallback to the earlier usage-bearing
        // turn (this is the design SQL's core semantic; a
        // "max-seq over usage-bearing turns" variant would have
        // returned Dave with 70/700).
        assert!(
            rows.iter().all(|r| r.speaker != "Dave"),
            "latest turn has no usage → NO fallback to older usage turn: {:?}",
            rows
        );
    }

    #[tokio::test]
    async fn list_speaker_cache_usage_excludes_user_and_speaker_null_rows() {
        let pool = test_pool().await;
        let sid = seed_session(&pool).await;
        seed_cache_usage_fixture(&pool, &sid).await;

        let rows = list_speaker_cache_usage(&pool, &sid).await.unwrap();

        // The user row (seq 0) and the speaker-NULL assistant row
        // (seq 7, written with a full usage JSON) must not appear.
        assert!(
            rows.iter().all(|r| r.speaker != "nobody"),
            "speaker=NULL row must be excluded: {:?}",
            rows
        );
        assert!(rows.iter().all(|r| r.speaker != ""), "rows: {:?}", rows);
    }

    #[tokio::test]
    async fn list_speaker_cache_usage_empty_for_missing_session_or_cleared_trace() {
        let pool = test_pool().await;
        // Missing session → empty vec (NOT an error).
        let rows = list_speaker_cache_usage(&pool, "nonexistent-session")
            .await
            .unwrap();
        assert!(rows.is_empty());

        // AC4: `clear_session_trace` wipes turn_trace → the group
        // chat cache-rate area renders its empty state.
        let sid = seed_session(&pool).await;
        seed_cache_usage_fixture(&pool, &sid).await;
        assert_eq!(
            list_speaker_cache_usage(&pool, &sid).await.unwrap().len(),
            3
        );
        clear_session_trace(&pool, &sid).await.unwrap();
        assert!(list_speaker_cache_usage(&pool, &sid)
            .await
            .unwrap()
            .is_empty());
    }
}
