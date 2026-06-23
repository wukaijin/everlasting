#![cfg(test)]

//! Permissions / audit / mode integration tests (split from `db/tests.rs` on 2026-06-23).
//!
//! Coverage:
//! - A2 + B7: tool permission grant/cascade + audit + mode
//! - C4: audit event round-trip + wire-shape (camelCase)
//! - Mode backfill on legacy rows

use sqlx::SqlitePool;
use uuid::Uuid;

use crate::projects::DEFAULT_PROJECT_ID;

use super::{
    migrations::run_migrations,
    permissions::{
        grant_tool_permission, has_tool_permission, list_audit_events, record_audit_event,
        update_session_mode,
    },
    sessions::{create_session, delete_session, list_sessions, load_session},
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
// A2 + B7 (2026-06-13): permission DB CRUD tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_session_mode_persists_and_round_trips() {
 // The migration backfill sets mode='edit' on legacy rows; the
 // `set_session_mode` IPC call must flip it to any of the 3
 // valid modes and survive a re-load.
 let pool = make_pool().await;
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();
 // Default after create_session = 'edit'.
 let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
 assert_eq!(loaded.session.mode, crate::db::Mode::Edit);

 update_session_mode(&pool, &s.id, crate::db::Mode::Plan).await.unwrap();
 let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
 assert_eq!(loaded.session.mode, crate::db::Mode::Plan);

 update_session_mode(&pool, &s.id, crate::db::Mode::Yolo).await.unwrap();
 let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
 assert_eq!(loaded.session.mode, crate::db::Mode::Yolo);
}

#[tokio::test]
async fn update_session_mode_on_missing_session_is_noop() {
 let pool = make_pool().await;
 // UPDATE with a non-matching id matches 0 rows; no error.
 update_session_mode(&pool, "nonexistent-session-id", crate::db::Mode::Plan)
 .await
 .unwrap();
}

#[tokio::test]
async fn list_sessions_includes_mode_field() {
 // The mode field on SessionSummary must round-trip through
 // the SELECT path so the sidebar / mode badge reads it
 // without a per-session IPC.
 let pool = make_pool().await;
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();
 update_session_mode(&pool, &s.id, crate::db::Mode::Yolo).await.unwrap();

 let list = list_sessions(&pool, DEFAULT_PROJECT_ID).await.unwrap();
 let found = list.iter().find(|x| x.id == s.id).expect("session in list");
 assert_eq!(found.mode, crate::db::Mode::Yolo);
}

#[tokio::test]
async fn grant_tool_permission_round_trip_and_has_check() {
 let pool = make_pool().await;
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();
 // Fresh session: no permissions yet.
 assert!(!has_tool_permission(&pool, &s.id, "shell").await.unwrap());
 assert!(!has_tool_permission(&pool, &s.id, "write_file").await.unwrap());

 grant_tool_permission(&pool, &s.id, "shell", "tool", None)
 .await
 .unwrap();
 assert!(has_tool_permission(&pool, &s.id, "shell").await.unwrap());
 // Different tool: still no permission.
 assert!(!has_tool_permission(&pool, &s.id, "write_file").await.unwrap());

 // Re-granting the same tool is a no-op (UPSERT semantics —
 // the `granted_at` is updated, but the row count stays 1).
 grant_tool_permission(&pool, &s.id, "shell", "tool", None)
 .await
 .unwrap();
 assert!(has_tool_permission(&pool, &s.id, "shell").await.unwrap());
}

#[tokio::test]
async fn grant_tool_permission_cascades_on_session_delete() {
 // ON DELETE CASCADE: deleting the session must clean up its
 // permission rows. PRAGMA foreign_keys = ON is set in
 // test_pool — without it the cascade silently no-ops.
 let pool = make_pool().await;
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();
 grant_tool_permission(&pool, &s.id, "shell", "tool", None)
 .await
 .unwrap();
 assert!(has_tool_permission(&pool, &s.id, "shell").await.unwrap());

 delete_session(&pool, &s.id).await.unwrap();
 // Session row is gone — the (sid, tool_name) lookup must
 // return false (the permission row was CASCADE-deleted).
 assert!(!has_tool_permission(&pool, &s.id, "shell").await.unwrap());
}

#[tokio::test]
async fn record_audit_event_inserts_and_cascades_on_delete() {
 let pool = make_pool().await;
 let s = create_session(&pool, &Uuid::new_v4().to_string(), DEFAULT_PROJECT_ID, "/tmp", "GLM-4.7", None)
 .await
 .unwrap();
 record_audit_event(
 &pool,
 &s.id,
 "tool_allowed",
 Some(r#"{"tool_name":"shell","reason":null}"#),
 )
 .await
 .unwrap();
 record_audit_event(&pool, &s.id, "mode_changed", Some(r#"{"new_mode":"yolo"}"#))
 .await
 .unwrap();
 // Verify the rows are present by SELECTing directly.
 let count: i64 = sqlx::query_scalar(
 "SELECT COUNT(*) FROM session_audit_events WHERE session_id = ?",
 )
 .bind(&s.id)
 .fetch_one(&pool)
 .await
 .unwrap();
 assert_eq!(count, 2);

 // Cascade on session delete.
 delete_session(&pool, &s.id).await.unwrap();
 let count: i64 = sqlx::query_scalar(
 "SELECT COUNT(*) FROM session_audit_events WHERE session_id = ?",
 )
 .bind(&s.id)
 .fetch_one(&pool)
 .await
 .unwrap();
 assert_eq!(count, 0);
}

/// C4 PR1 (2026-06-14): the new `tool_executed` audit kind writes
/// through `record_audit_event` with the C4 payload shape
/// (`tool_name` / `tool_input` / `duration_ms` / `exit_code`) and
/// round-trips through `list_audit_events` so the audit-log UI
/// can read it back. The `kind` is a plain string on the wire (the
/// DB column is TEXT) so the new variant requires no migration;
/// this test locks the round-trip + the payload parse path the
/// frontend will rely on.
///
/// **Order-independence note**: `session_audit_events.ts` is
/// `datetime('now')` (1-second resolution). Two inserts inside the
/// same wall-clock second share the same `ts`, so
/// `ORDER BY ts DESC` is non-deterministic for ties. The test
/// therefore finds each row by its `tool_name` instead of
/// assuming `rows[0]` is the shell row.
#[tokio::test]
async fn tool_executed_audit_round_trips_via_list_audit_events() {
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

    // Case 1: shell tool with a real exit code.
    let payload_shell = serde_json::json!({
        "tool_name": "shell",
        "tool_input": {"command": "cargo build"},
        "duration_ms": 1234_u64,
        "exit_code": 0_i32,
    })
    .to_string();
    record_audit_event(&pool, &s.id, "tool_executed", Some(&payload_shell))
        .await
        .unwrap();

    // Case 2: read_file tool with no exit code (Option::None on
    // the agent-loop side serializes as JSON null).
    let payload_read = serde_json::json!({
        "tool_name": "read_file",
        "tool_input": {"path": "/tmp/foo.rs"},
        "duration_ms": 12_u64,
        "exit_code": serde_json::Value::Null,
    })
    .to_string();
    record_audit_event(&pool, &s.id, "tool_executed", Some(&payload_read))
        .await
        .unwrap();

    // Round-trip: list_audit_events returns both rows.
    let rows = list_audit_events(&pool, &s.id).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].kind, "tool_executed");
    assert_eq!(rows[1].kind, "tool_executed");

    // Find each payload by tool_name (the `ts` ties make the row
    // order non-deterministic).
    let mut shell_payload: Option<serde_json::Value> = None;
    let mut read_payload: Option<serde_json::Value> = None;
    for r in &rows {
        let p: serde_json::Value =
            serde_json::from_str(r.payload_json.as_deref().unwrap()).unwrap();
        match p["tool_name"].as_str() {
            Some("shell") => shell_payload = Some(p),
            Some("read_file") => read_payload = Some(p),
            _ => {}
        }
    }

    let p_shell = shell_payload.expect("shell payload must be present");
    assert_eq!(p_shell["duration_ms"], 1234);
    assert_eq!(p_shell["exit_code"], 0);
    assert!(
        !p_shell["exit_code"].is_null(),
        "exit_code must NOT be null for shell"
    );

    let p_read = read_payload.expect("read_file payload must be present");
    assert_eq!(p_read["duration_ms"], 12);
    assert!(
        p_read["exit_code"].is_null(),
        "exit_code must be null for read_file"
    );
}

/// C4 PR1: list_audit_events on an empty session returns an empty
/// Vec (NOT an error). The audit-log UI renders its "暂无审计事件"
/// placeholder against this shape; an error would surface as a
/// toast instead.
#[tokio::test]
async fn list_audit_events_empty_session_returns_empty_vec() {
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
    let rows = list_audit_events(&pool, &s.id).await.unwrap();
    assert!(rows.is_empty());
}

/// C4 PR1: list_audit_events tolerates a NULL payload_json. Older
/// code paths (or future ones) may write rows without a payload;
/// the read side must surface them as `payload_json: None` instead
/// of crashing. The audit-log UI's "payload 为 null/malformed 时不
/// 崩" AC leans on this.
#[tokio::test]
async fn list_audit_events_tolerates_null_payload() {
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
    record_audit_event(&pool, &s.id, "tool_executed", None)
        .await
        .unwrap();
    let rows = list_audit_events(&pool, &s.id).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, "tool_executed");
    assert!(rows[0].payload_json.is_none());
}

/// C4 PR1 check-phase follow-up (2026-06-14): lock the wire shape
/// of `AuditEventRow` to **camelCase** — matches every other
/// `db::*Row` that crosses the IPC boundary (SessionRow,
/// SessionSummary, ProviderRow, ModelRow, … all carry
/// `#[serde(rename_all = \"camelCase\")]`). The frontend's TS
/// interface reads `sessionId` / `payloadJson`, not snake_case —
/// if a future refactor drops the `rename_all` attribute, the
/// frontend gets `undefined` for every field. This regression test
/// fails in that case.
///
/// Locks spec `.trellis/spec/backend/database-guidelines.md`:
/// "All Serialize structs that cross the IPC boundary have
///  #[serde(rename_all = \"camelCase\")]"
#[tokio::test]
async fn audit_event_row_serializes_to_camel_case_wire_shape() {
    use crate::db::permissions::AuditEventRow;
    let row = AuditEventRow {
        id: 42,
        session_id: "sess-abc".to_string(),
        ts: "2026-06-14T10:00:00Z".to_string(),
        kind: "tool_executed".to_string(),
        payload_json: Some("{\"tool_name\":\"shell\"}".to_string()),
    };
    let v: serde_json::Value = serde_json::to_value(&row).unwrap();
    let obj = v.as_object().expect("row must serialize to JSON object");

    // camelCase keys must be present.
    assert!(
        obj.contains_key("sessionId"),
        "wire shape must use `sessionId` (camelCase), got keys: {:?}",
        obj.keys().collect::<Vec<_>>()
    );
    assert!(
        obj.contains_key("payloadJson"),
        "wire shape must use `payloadJson` (camelCase), got keys: {:?}",
        obj.keys().collect::<Vec<_>>()
    );

    // snake_case keys must NOT be present (would mean `rename_all`
    // was dropped).
    assert!(
        !obj.contains_key("session_id"),
        "wire shape must NOT leak snake_case `session_id`"
    );
    assert!(
        !obj.contains_key("payload_json"),
        "wire shape must NOT leak snake_case `payload_json`"
    );

    // Round-trip the value to confirm the non-renamed fields are intact.
    assert_eq!(obj.get("id").and_then(|v| v.as_i64()), Some(42));
    assert_eq!(obj.get("kind").and_then(|v| v.as_str()), Some("tool_executed"));
}

#[tokio::test]
async fn mode_backfill_legacy_null_to_edit() {
 // Simulate a pre-A2 session with `mode IS NULL` (column was
 // added but the backfill hasn't run yet). Mirrors what a
 // real upgrade path looks like between the ALTER and the
 // UPDATE in `run_migrations`.
 let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
 sqlx::query("PRAGMA foreign_keys = ON")
 .execute(&pool)
 .await
 .unwrap();
 // Minimal pre-A2 schema: sessions row without `mode`.
 sqlx::query(
 r#"
 CREATE TABLE sessions (
 id TEXT PRIMARY KEY, title TEXT NOT NULL,
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
 model TEXT NOT NULL, project_id TEXT NOT NULL DEFAULT '__default__',
 current_cwd TEXT NOT NULL DEFAULT ''
 )
 "#,
 )
 .execute(&pool)
 .await
 .unwrap();
 sqlx::query(
 r#"
 CREATE TABLE projects (
 id TEXT PRIMARY KEY, name TEXT NOT NULL, path TEXT NOT NULL,
 is_git_repo INTEGER NOT NULL DEFAULT 0, is_legacy INTEGER NOT NULL DEFAULT 0,
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
 hidden INTEGER NOT NULL DEFAULT 0, metadata TEXT
 )
 "#,
 )
 .execute(&pool)
 .await
 .unwrap();
 sqlx::query(
 "INSERT INTO projects (id, name, path, is_legacy, created_at, updated_at, hidden) \
 VALUES ('__default__', 'legacy', '/tmp', 1, '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 0)",
 )
 .execute(&pool)
 .await
 .unwrap();
 sqlx::query(
 "INSERT INTO sessions (id, title, created_at, updated_at, model) \
 VALUES ('legacy-1', 't', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 'GLM-4.7')",
 )
 .execute(&pool)
 .await
 .unwrap();
 // Run the full migration (adds the `mode` column + backfills).
 run_migrations(&pool).await.unwrap();
 // Verify the backfill set mode='edit' on the legacy row.
 let mode: Option<String> = sqlx::query_scalar("SELECT mode FROM sessions WHERE id = 'legacy-1'")
 .fetch_one(&pool)
 .await
 .unwrap();
 assert_eq!(mode.as_deref(), Some("edit"));
}

