#![cfg(test)]

//! subagent_runs-domain integration tests (split from `db/tests.rs` on 2026-06-23).
//!
//! Coverage:
//! - B6 PR2: insert_run / update_run_finished / cascade / list /
//!   token_usage_streaming / list_runs_summary_by_session
//! - B6 redesign PR1: task + final_text columns
//! - R2 (2026-06-21): incomplete status + widen migration idempotency
//! - 2026-06-22 (RULE-FrontSubagent-004): turn_count column

use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::llm::types::TokenUsage;
use crate::projects::DEFAULT_PROJECT_ID;

use super::{
    migrations::run_migrations,
    sessions::{create_session, delete_session},
    subagent_runs::{
        get_run, insert_run, list_runs_by_session,
        list_runs_summary_by_session, update_run_finished, SubagentStatusDb,
    },
};

async fn test_pool() -> SqlitePool {
 let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
 // Mirror what `init_pool` does.
 sqlx::query("PRAGMA foreign_keys = ON")
 .execute(&pool)
 .await
 .unwrap();
 run_migrations(&pool).await.unwrap();
 pool
}

async fn make_pool() -> SqlitePool {
 test_pool().await // alias for readability inside this section
}
// ---------------------------------------------------------------------------
// B6 PR2: subagent_runs tests
// ---------------------------------------------------------------------------

/// Insert returns a unique id and the row lands in `running`
/// state with `finished_at` NULL, the empty `TokenUsage` JSON
/// default, the empty transcript `[]`, and `transcript_truncated=0`.
#[tokio::test]
async fn subagent_runs_insert_creates_running_row() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    let id = insert_run(&pool, &s.id, "rid-test", "researcher", None)
        .await
        .unwrap();
    let row = get_run(&pool, &id).await.unwrap().expect("row exists");
    assert_eq!(row.id, id);
    assert_eq!(row.parent_session_id, s.id);
    assert_eq!(row.parent_request_id, "rid-test");
    assert_eq!(row.subagent_name, "researcher");
    assert_eq!(row.status, "running");
    assert!(row.finished_at.is_none(), "running → finished_at=NULL");
    assert_eq!(
        row.transcript_truncated, 0,
        "fresh row → transcript_truncated=0"
    );
    assert_eq!(
        row.transcript_json.as_deref(),
        Some("[]"),
        "fresh row → transcript_json=[]"
    );
    assert!(
        row.token_usage_json.is_some(),
        "fresh row → token_usage_json seeded"
    );
    assert!(row.summary.is_none(), "running → summary=NULL");
    assert!(row.task.is_none(), "task=None at insert → column NULL");
    assert!(row.final_text.is_none(), "running → final_text=NULL");
    assert!(!row.started_at.is_empty());
    assert!(!row.created_at.is_empty());
}

/// `update_run_finished` flips `status` to the terminal value,
/// sets `finished_at`, populates `summary` + `token_usage_json`
/// + `transcript_json`, and sets `transcript_truncated` to the
/// caller's choice.
#[tokio::test]
async fn subagent_runs_update_finished_sets_status_and_fields() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    let id = insert_run(&pool, &s.id, "rid-test", "general-purpose", None)
        .await
        .unwrap();
    let usage = TokenUsage {
        input_tokens: 1234,
        output_tokens: 567,
        cache_creation_input_tokens: 10,
        cache_read_input_tokens: 20,
        context_input_tokens: 1264,
    };
    let transcript = vec![crate::agent::subagent::TranscriptEntry {
        kind: crate::agent::subagent::TranscriptKind::ChatEvent,
        payload_json: serde_json::json!({"hello": "world"}),
    }];
    update_run_finished(
        &pool,
        &id,
        SubagentStatusDb::Completed,
        "2026-06-20T00:00:00+00:00",
        "found 3 files",
        "found 3 files",
        &usage,
        &transcript,
        false,
        None,
    )
    .await
    .unwrap();
    let row = get_run(&pool, &id).await.unwrap().expect("row exists");
    assert_eq!(row.status, "completed");
    assert_eq!(row.finished_at.as_deref(), Some("2026-06-20T00:00:00+00:00"));
    assert_eq!(row.summary.as_deref(), Some("found 3 files"));
    assert_eq!(
        row.final_text.as_deref(),
        Some("found 3 files"),
        "final_text column reflects the same final assistant text"
    );
    assert_eq!(row.transcript_truncated, 0);
    let parsed_usage: TokenUsage =
        serde_json::from_str(row.token_usage_json.as_deref().unwrap()).unwrap();
    assert_eq!(parsed_usage.input_tokens, 1234);
    assert_eq!(parsed_usage.output_tokens, 567);
    let parsed_transcript: Vec<crate::agent::subagent::TranscriptEntry> =
        serde_json::from_str(row.transcript_json.as_deref().unwrap()).unwrap();
    assert_eq!(parsed_transcript.len(), 1);
}

/// `transcript_truncated=true` is reflected in the column read.
#[tokio::test]
async fn subagent_runs_update_finished_records_truncated_flag() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    let id = insert_run(&pool, &s.id, "rid-test", "researcher", None)
        .await
        .unwrap();
    let empty = vec![];
    update_run_finished(
        &pool,
        &id,
        SubagentStatusDb::Error,
        "2026-06-20T00:00:00+00:00",
        "",
        "",
        &TokenUsage::default(),
        &empty,
        true,
        None,
    )
    .await
    .unwrap();
    let row = get_run(&pool, &id).await.unwrap().expect("row exists");
    assert_eq!(row.status, "error");
    assert_eq!(row.transcript_truncated, 1);
}

/// `ON DELETE CASCADE`: deleting the parent `sessions` row drops
/// every `subagent_runs` row that references it.
#[tokio::test]
async fn subagent_runs_cascade_delete_with_parent_session() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    let id1 = insert_run(&pool, &s.id, "rid-1", "researcher", None)
        .await
        .unwrap();
    let id2 = insert_run(&pool, &s.id, "rid-2", "general-purpose", None)
        .await
        .unwrap();
    // Sanity: both rows are there.
    assert!(get_run(&pool, &id1).await.unwrap().is_some());
    assert!(get_run(&pool, &id2).await.unwrap().is_some());

    delete_session(&pool, &s.id).await.unwrap();
    // CASCADE: both rows gone.
    assert!(get_run(&pool, &id1).await.unwrap().is_none());
    assert!(get_run(&pool, &id2).await.unwrap().is_none());
}

/// `list_runs_by_session` returns all runs for the parent
/// session, sorted by `started_at DESC` (newest first).
#[tokio::test]
async fn subagent_runs_list_by_session_orders_by_started_desc() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    let id1 = insert_run(&pool, &s.id, "rid-1", "researcher", None)
        .await
        .unwrap();
    // Tiny sleep so `started_at` advances between inserts.
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let id2 = insert_run(&pool, &s.id, "rid-2", "general-purpose", None)
        .await
        .unwrap();
    let rows = list_runs_by_session(&pool, &s.id).await.unwrap();
    assert_eq!(rows.len(), 2);
    // Newest first → id2 (later insert) before id1.
    assert_eq!(rows[0].id, id2);
    assert_eq!(rows[1].id, id1);
}

/// B6 PR3a (2026-06-20): `list_runs_summary_by_session` returns
/// the projected `SubagentRunSummary` (no transcript column) for
/// the parent session. Verifies:
/// 1. Newest-first ordering (same as `list_runs_by_session`).
/// 2. The typed `SubagentStatusDb::Completed` enum variant is
///    decoded (NOT the raw wire string) — the frontend renders
///    the status badge from the enum without an extra parse.
/// 3. Summary field carries the worker's final_text verbatim.
#[tokio::test]
async fn subagent_runs_list_runs_summary_by_session_projects_typed_enum() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    // Insert + complete 1 run with a populated transcript + summary.
    let id = insert_run(&pool, &s.id, "rid-summary", "researcher", None)
        .await
        .unwrap();
    let usage = TokenUsage {
        input_tokens: 10,
        output_tokens: 5,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
        context_input_tokens: 10,
    };
    let transcript = vec![crate::agent::subagent::TranscriptEntry {
        kind: crate::agent::subagent::TranscriptKind::ChatEvent,
        payload_json: serde_json::json!({"text": "hello"}),
    }];
    update_run_finished(
        &pool,
        &id,
        SubagentStatusDb::Completed,
        "2026-06-20T00:00:00+00:00",
        "summary text",
        "summary text",
        &usage,
        &transcript,
        false,
        None,
    )
    .await
    .unwrap();

    let summaries = list_runs_summary_by_session(&pool, &s.id)
        .await
        .expect("list_runs_summary_by_session");
    assert_eq!(summaries.len(), 1);
    let sum = &summaries[0];
    assert_eq!(sum.id, id);
    assert_eq!(sum.subagent_name, "researcher");
    assert_eq!(
        sum.status,
        SubagentStatusDb::Completed,
        "status must be decoded to the typed enum (not the wire string)"
    );
    assert_eq!(sum.summary.as_deref(), Some("summary text"));
    assert_eq!(
        sum.final_text.as_deref(),
        Some("summary text"),
        "final_text projected into summary"
    );
    assert!(
        sum.task.is_none(),
        "task=None at insert → column NULL, projected as None"
    );
    assert_eq!(sum.token_usage_json.as_deref(), Some(serde_json::to_string(&usage).unwrap().as_str()));
}

/// B6 PR3a (2026-06-20): `list_runs_summary_by_session` returns
/// an empty `Vec` (NOT an error) for a session with no runs.
#[tokio::test]
async fn subagent_runs_list_runs_summary_by_session_empty() {
    let pool = make_pool().await;
    let summaries = list_runs_summary_by_session(&pool, "nonexistent-session-id")
        .await
        .expect("empty list, no error");
    assert!(summaries.is_empty());
}

// ---------------------------------------------------------------------------
// B6 redesign PR1 (2026-06-21): task + final_text columns
// ---------------------------------------------------------------------------

/// `insert_run` with `task = Some(...)` writes the task verbatim
/// into the `task` column. The drawer reads this as the prompt
/// card header — the prompt must land on the row the moment
/// `insert_run` returns so the user can open the drawer mid-
/// worker (before `update_run_finished` fires).
#[tokio::test]
async fn subagent_runs_insert_writes_task_column() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    let task_text = "find all files that mention dispatch_subagent";
    let id = insert_run(&pool, &s.id, "rid-task", "researcher", Some(task_text))
        .await
        .unwrap();
    let row = get_run(&pool, &id).await.unwrap().expect("row exists");
    assert_eq!(row.task.as_deref(), Some(task_text));
    // task is non-NULL but the worker hasn't run yet → summary +
    // final_text are still NULL.
    assert!(row.summary.is_none());
    assert!(row.final_text.is_none());
    assert!(row.finished_at.is_none(), "still running");
}

/// `insert_run` with `task = None` leaves the column NULL. Mirrors
/// the legacy pre-PR1 behavior — pre-existing test callers pass
/// `None` so the migration remains backward-compatible.
#[tokio::test]
async fn subagent_runs_insert_with_none_task_leaves_column_null() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    let id = insert_run(&pool, &s.id, "rid-no-task", "researcher", None)
        .await
        .unwrap();
    let row = get_run(&pool, &id).await.unwrap().expect("row exists");
    assert!(row.task.is_none());
}

/// `update_run_finished` with `final_text` writes the column
/// verbatim — the caller is responsible for pre-stripping the
/// `[status: ...]\n` prefix (via
/// `crate::agent::subagent::format_final_text`). This test
/// exercises the storage-layer contract: the column carries
/// whatever string the caller passes.
#[tokio::test]
async fn subagent_runs_update_finished_writes_final_text_column() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    let id = insert_run(&pool, &s.id, "rid-ft", "general-purpose", Some("do the thing"))
        .await
        .unwrap();
    // Caller passes the prefix-stripped final_text (per the
    // run_subagent contract: format_final_text is invoked at the
    // call site, not inside update_run_finished).
    let stripped = "the worker finished and reported X";
    update_run_finished(
        &pool,
        &id,
        SubagentStatusDb::Completed,
        "2026-06-21T12:00:00+00:00",
        stripped, // summary (legacy wire field)
        stripped, // final_text (drawer Reply segment)
        &TokenUsage::default(),
        &[],
        false,
        None,
    )
    .await
    .unwrap();
    let row = get_run(&pool, &id).await.unwrap().expect("row exists");
    assert_eq!(row.summary.as_deref(), Some(stripped));
    assert_eq!(row.final_text.as_deref(), Some(stripped));
    // task (written at insert) is preserved through the update.
    assert_eq!(row.task.as_deref(), Some("do the thing"));
    // final_text is independent of summary at the column level —
    // a future PR could store different shapes per column (e.g.
    // status-prefixed summary for the wire, prefix-stripped
    // final_text for the UI).
}

/// Cancelled run: `final_text` carries the worker's partial text
/// plus the `[已停止]` marker (the format `format_final_text`
/// produces for `Cancelled` + non-empty worker_text). The status
/// column carries `cancelled` independently.
#[tokio::test]
async fn subagent_runs_update_finished_cancelled_status_and_marker() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    let id = insert_run(&pool, &s.id, "rid-cancel", "researcher", None)
        .await
        .unwrap();
    // Mirror what run_subagent sends for a cancelled run with
    // partial worker text.
    let final_text = format!(
        "partial analysis\n\n{}",
        crate::agent::helpers::CANCELLED_MARKER
    );
    update_run_finished(
        &pool,
        &id,
        SubagentStatusDb::Cancelled,
        "2026-06-21T12:00:00+00:00",
        &final_text,
        &final_text,
        &TokenUsage::default(),
        &[],
        false,
        None,
    )
    .await
    .unwrap();
    let row = get_run(&pool, &id).await.unwrap().expect("row exists");
    assert_eq!(row.status, "cancelled");
    assert!(row.finished_at.is_some());
    assert_eq!(row.final_text.as_deref(), Some(final_text.as_str()));
    assert!(row
        .final_text
        .as_deref()
        .unwrap()
        .contains(crate::agent::helpers::CANCELLED_MARKER));
}

/// Error run: `final_text` carries the error message verbatim
/// (the `format_final_text` shape for `Error`). The `status`
/// column carries `error` independently — the drawer renders
/// the status badge from the column.
#[tokio::test]
async fn subagent_runs_update_finished_error_status_and_text() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    let id = insert_run(&pool, &s.id, "rid-error", "general-purpose", None)
        .await
        .unwrap();
    update_run_finished(
        &pool,
        &id,
        SubagentStatusDb::Error,
        "2026-06-21T12:00:00+00:00",
        "LLM stream errored",
        "LLM stream errored",
        &TokenUsage::default(),
        &[],
        false,
        None,
    )
    .await
    .unwrap();
    let row = get_run(&pool, &id).await.unwrap().expect("row exists");
    assert_eq!(row.status, "error");
    assert_eq!(row.final_text.as_deref(), Some("LLM stream errored"));
}

/// `list_runs_by_session` returns rows with `task` + `final_text`
/// populated (no column is dropped on the list path — the
/// projected shape still carries the new fields).
#[tokio::test]
async fn subagent_runs_list_returns_task_and_final_text() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    let id = insert_run(&pool, &s.id, "rid-list", "researcher", Some("prompt here"))
        .await
        .unwrap();
    update_run_finished(
        &pool,
        &id,
        SubagentStatusDb::Completed,
        "2026-06-21T12:00:00+00:00",
        "found 5 files",
        "found 5 files",
        &TokenUsage::default(),
        &[],
        false,
        None,
    )
    .await
    .unwrap();
    let rows = list_runs_by_session(&pool, &s.id).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].task.as_deref(), Some("prompt here"));
    assert_eq!(rows[0].final_text.as_deref(), Some("found 5 files"));
}

/// Migration idempotency: re-running the migration on a pre-PR1
/// DB brings it up to date; re-running on a post-PR1 DB is a
/// no-op. This is the regression guard for the
/// `add_subagent_runs_column_if_missing` helper (analogous to
/// the existing `add_session_column_if_missing` smoke tests).
#[tokio::test]
async fn subagent_runs_migration_is_idempotent_on_pr1_columns() {
    let pool = make_pool().await;
    // First run (above via `make_pool`) already added `task` +
    // `final_text`. Re-run the migration — must NOT error.
    crate::db::migrations::run_migrations(&pool)
        .await
        .expect("migration re-run is idempotent");
    // Columns are still there.
    let exists_task: i64 =
        sqlx::query("SELECT COUNT(*) FROM pragma_table_info('subagent_runs') WHERE name = 'task'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
    let exists_final: i64 = sqlx::query(
        "SELECT COUNT(*) FROM pragma_table_info('subagent_runs') WHERE name = 'final_text'",
    )
    .fetch_one(&pool)
    .await
    .unwrap()
    .try_get(0)
    .unwrap();
    assert_eq!(exists_task, 1, "task column present");
    assert_eq!(exists_final, 1, "final_text column present");
}

/// 2026-06-22 (RULE-FrontSubagent-004) migration idempotency: the
/// `turn_count` column is added by `add_subagent_runs_column_if_missing`
/// and a re-run is a no-op. Mirrors the PR1 idempotency test on
/// `task` / `final_text`.
#[tokio::test]
async fn subagent_runs_migration_adds_turn_count_column_idempotently() {
    let pool = make_pool().await;
    // First run (via make_pool) already added `turn_count`. Re-run
    // the migration — must NOT error.
    crate::db::migrations::run_migrations(&pool)
        .await
        .expect("migration re-run is idempotent on turn_count");
    let exists: i64 =
        sqlx::query("SELECT COUNT(*) FROM pragma_table_info('subagent_runs') WHERE name = 'turn_count'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
    assert_eq!(exists, 1, "turn_count column present after re-run");
}

/// 2026-06-22 (RULE-FrontSubagent-004) round-trip: `update_run_finished`
/// with `turn_count: Some(N)` writes the value to the `turn_count`
/// column; `get_run` + `list_runs_by_session` +
/// `list_runs_summary_by_session` all read it back unchanged. Also
/// covers the legacy `None` path: a row whose `update_run_finished`
/// passes `None` keeps NULL turn_count (the drawer degrades to the
/// wall-clock suffix for those rows).
#[tokio::test]
async fn subagent_runs_update_finished_round_trips_turn_count() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();

    // Row 1: realistic cancelled run, 7 turns executed.
    let id_cancelled = insert_run(&pool, &s.id, "rid-turn-cancel", "general-purpose", None)
        .await
        .unwrap();
    update_run_finished(
        &pool,
        &id_cancelled,
        SubagentStatusDb::Cancelled,
        "2026-06-22T10:00:00+00:00",
        "partial",
        "partial",
        &TokenUsage::default(),
        &[],
        false,
        Some(7),
    )
    .await
    .unwrap();

    // Row 2: legacy caller passes `None` (pre-PR2 callers or when
    // the count is unknown). Column stays NULL.
    let id_legacy = insert_run(&pool, &s.id, "rid-turn-legacy", "researcher", None)
        .await
        .unwrap();
    update_run_finished(
        &pool,
        &id_legacy,
        SubagentStatusDb::Completed,
        "2026-06-22T10:05:00+00:00",
        "done",
        "done",
        &TokenUsage::default(),
        &[],
        false,
        None,
    )
    .await
    .unwrap();

    // get_run: round-trips both Some and None.
    let row_cancelled = get_run(&pool, &id_cancelled)
        .await
        .unwrap()
        .expect("cancelled row exists");
    assert_eq!(row_cancelled.turn_count, Some(7));
    let row_legacy = get_run(&pool, &id_legacy)
        .await
        .unwrap()
        .expect("legacy row exists");
    assert_eq!(row_legacy.turn_count, None, "legacy row keeps NULL");

    // list_runs_by_session: full row projection carries turn_count.
    let rows = list_runs_by_session(&pool, &s.id).await.unwrap();
    assert_eq!(rows.len(), 2);
    // Newest-first ordering: cancelled (10:00) < legacy (10:05)? No —
    // DESC by started_at, so the SECOND insert (later started_at)
    // comes first. Both started ~now; order between the two is
    // determined by `started_at` which is set at insert_run. Find
    // each by id to be robust against ordering.
    let listed_cancelled = rows
        .iter()
        .find(|r| r.id == id_cancelled)
        .expect("cancelled row in list");
    assert_eq!(listed_cancelled.turn_count, Some(7));
    let listed_legacy = rows
        .iter()
        .find(|r| r.id == id_legacy)
        .expect("legacy row in list");
    assert_eq!(listed_legacy.turn_count, None);

    // list_runs_summary_by_session: summary projection carries
    // turn_count too (single-i64 column is cheap; included so the
    // card / drawer can both read it).
    let summaries = list_runs_summary_by_session(&pool, &s.id)
        .await
        .unwrap();
    assert_eq!(summaries.len(), 2);
    let summary_cancelled = summaries
        .iter()
        .find(|r| r.id == id_cancelled)
        .expect("cancelled summary");
    assert_eq!(summary_cancelled.turn_count, Some(7));
    let summary_legacy = summaries
        .iter()
        .find(|r| r.id == id_legacy)
        .expect("legacy summary");
    assert_eq!(summary_legacy.turn_count, None);
}

/// R2 (2026-06-21) regression: the `subagent_runs.status` column
/// must accept `'incomplete'` (the 5th variant added by the
/// `widen_subagent_runs_status_check_for_incomplete` migration).
/// The CHECK constraint on the table reads
/// `('running','completed','cancelled','error','incomplete')` after
/// the migration runs. A pre-R2 DB has the 4-variant CHECK; the
/// migration rebuilds the table with the wider CHECK. This test
/// exercises the post-migration shape: a row is INSERTed with
/// `status='running'`, then UPDATEd to `status='incomplete'` via
/// `update_run_finished`. The CHECK must accept the value, the
/// row must round-trip through `get_run` with the right `status`
/// string, and `SubagentStatusDb::from_str_opt("incomplete")`
/// must return `SubagentStatusDb::Incomplete` (lenient parse
/// lockstep with the wire form).
#[tokio::test]
async fn subagent_runs_incomplete_status_round_trips() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    let id = insert_run(&pool, &s.id, "rid-incomplete", "general-purpose", None)
        .await
        .unwrap();
    // Mirror what `run_subagent` sends for an `Incomplete` run.
    // The `final_text` carries the worker's partial text + the
    // `INCOMPLETE_MARKER` (the `format_final_text(Incomplete, _)` shape).
    let partial = "did 100 of 200 turns";
    let final_text = format!(
        "{}\n\n{}",
        partial,
        crate::agent::helpers::INCOMPLETE_MARKER
    );
    // Realistic cumulative usage (R3 fix path: the synthetic
    // terminal `Done{max_turns, usage: last_usage}` arrives at
    // the sink, the guard skips the push, and the cumulative
    // reflects 100 turns of actual usage — not all-zero).
    let cumulative = crate::llm::types::TokenUsage {
        input_tokens: 12_345,
        output_tokens: 678,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
        context_input_tokens: 12_345,
    };
    update_run_finished(
        &pool,
        &id,
        SubagentStatusDb::Incomplete,
        "2026-06-21T13:00:00+00:00",
        &final_text,
        &final_text,
        &cumulative,
        &[],
        false,
        None,
    )
    .await
    .expect("update_run_finished must accept the new Incomplete variant");
    // Round-trip: read the row back.
    let row = get_run(&pool, &id).await.unwrap().expect("row exists");
    assert_eq!(
        row.status, "incomplete",
        "status column carries 'incomplete' (CHECK widened by migration)"
    );
    assert_eq!(row.final_text.as_deref(), Some(final_text.as_str()));
    // Cumulative usage is round-tripped verbatim (R3 fix).
    let parsed_usage: TokenUsage =
        serde_json::from_str(row.token_usage_json.as_deref().unwrap()).unwrap();
    assert_eq!(parsed_usage.input_tokens, 12_345);
    assert_eq!(parsed_usage.output_tokens, 678);
    // Lenient parse: "incomplete" → Incomplete (forward-compat
    // invariant for old binaries reading new DBs).
    assert_eq!(
        SubagentStatusDb::from_str_opt("incomplete"),
        SubagentStatusDb::Incomplete
    );
    // `as_str` lockstep: the enum → string mapping is
    // `Incomplete → "incomplete"`, matching the wire form.
    assert_eq!(SubagentStatusDb::Incomplete.as_str(), "incomplete");
}

/// R2 migration idempotency: the
/// `widen_subagent_runs_status_check_for_incomplete` migration
/// must be a no-op on a second run. The probe-based guard
/// (`sqlite_master.sql` contains `'incomplete'`) short-circuits
/// the table-rebuild path on a re-run, so the function returns
/// `Ok(())` and the existing rows are untouched. This is the
/// regression guard for the migration's idempotency claim in
/// the `widen_subagent_runs_status_check_for_incomplete`
/// docstring.
#[tokio::test]
async fn subagent_runs_widen_incomplete_migration_is_idempotent() {
    let pool = make_pool().await;
    // First run already widened the CHECK (via `make_pool`'s
    // `run_migrations` call). Re-run the migration directly.
    crate::db::migrations::run_migrations(&pool)
        .await
        .expect("re-run is idempotent");
    // Add a row to confirm the table still works.
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
    )
    .await
    .unwrap();
    let id = insert_run(&pool, &s.id, "rid-re", "researcher", None)
        .await
        .unwrap();
    assert!(
        get_run(&pool, &id).await.unwrap().is_some(),
        "rows survive the idempotent re-run"
    );
}
