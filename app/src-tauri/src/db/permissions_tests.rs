#![cfg(test)]

//! Permissions / audit / mode integration tests (split from `db/tests.rs` on 2026-06-23).
//!
//! Coverage:
//! - A2 + B7: tool permission grant/cascade + audit + mode
//! - C4: audit event round-trip + wire-shape (camelCase)
//! - Mode backfill on legacy rows

use super::test_support::test_pool;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::projects::DEFAULT_PROJECT_ID;

use super::{
    migrations::run_migrations,
    permissions::{
        grant_tool_permission, has_tool_permission, list_audit_events, list_audit_events_page,
        list_tool_permissions, record_audit_event, revoke_tool_permission, update_session_mode,
        AuditEventPageQuery, AUDIT_PAGE_DEFAULT_LIMIT, AUDIT_PAGE_MAX_LIMIT,
    },
    sessions::{create_session, delete_session, list_sessions, load_session},
};

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
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    // Default after create_session = 'edit'.
    let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
    assert_eq!(loaded.session.mode, crate::db::Mode::Edit);

    update_session_mode(&pool, &s.id, crate::db::Mode::Plan)
        .await
        .unwrap();
    let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
    assert_eq!(loaded.session.mode, crate::db::Mode::Plan);

    update_session_mode(&pool, &s.id, crate::db::Mode::Yolo)
        .await
        .unwrap();
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
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    update_session_mode(&pool, &s.id, crate::db::Mode::Yolo)
        .await
        .unwrap();

    let list = list_sessions(&pool, DEFAULT_PROJECT_ID).await.unwrap();
    let found = list.iter().find(|x| x.id == s.id).expect("session in list");
    assert_eq!(found.mode, crate::db::Mode::Yolo);
}

#[tokio::test]
async fn grant_tool_permission_round_trip_and_has_check() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    // Fresh session: no permissions yet.
    assert!(!has_tool_permission(&pool, &s.id, "shell").await.unwrap());
    assert!(!has_tool_permission(&pool, &s.id, "write_file")
        .await
        .unwrap());

    grant_tool_permission(&pool, &s.id, "shell", "tool", None)
        .await
        .unwrap();
    assert!(has_tool_permission(&pool, &s.id, "shell").await.unwrap());
    // Different tool: still no permission.
    assert!(!has_tool_permission(&pool, &s.id, "write_file")
        .await
        .unwrap());

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
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
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
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    record_audit_event(
        &pool,
        &s.id,
        "tool_allowed",
        Some(r#"{"tool_name":"shell","reason":null}"#),
        None,
    )
    .await
    .unwrap();
    record_audit_event(
        &pool,
        &s.id,
        "mode_changed",
        Some(r#"{"new_mode":"yolo"}"#),
        None,
    )
    .await
    .unwrap();
    // Verify the rows are present by SELECTing directly.
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM session_audit_events WHERE session_id = ?")
            .bind(&s.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 2);

    // Cascade on session delete.
    delete_session(&pool, &s.id).await.unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM session_audit_events WHERE session_id = ?")
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
        None,
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
    record_audit_event(&pool, &s.id, "tool_executed", Some(&payload_shell), None)
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
    record_audit_event(&pool, &s.id, "tool_executed", Some(&payload_read), None)
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
        None,
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
        None,
        None,
    )
    .await
    .unwrap();
    record_audit_event(&pool, &s.id, "tool_executed", None, None)
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
        turn_seq: Some(7),
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
    assert_eq!(
        obj.get("kind").and_then(|v| v.as_str()),
        Some("tool_executed")
    );

    // E2 (2026-07-14): turn_seq must serialize as camelCase `turnSeq`.
    assert!(
        obj.contains_key("turnSeq"),
        "wire shape must use `turnSeq` (camelCase), got keys: {:?}",
        obj.keys().collect::<Vec<_>>()
    );
    assert_eq!(obj.get("turnSeq").and_then(|v| v.as_i64()), Some(7));
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
    let mode: Option<String> =
        sqlx::query_scalar("SELECT mode FROM sessions WHERE id = 'legacy-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(mode.as_deref(), Some("edit"));
}

// ---------------------------------------------------------------------------
// Task 07-01 (permission-grant list management UI): list + revoke
// ---------------------------------------------------------------------------

/// `list_tool_permissions` on a fresh session returns an empty Vec
/// (NOT an error) — the management modal renders its empty-state
/// placeholder against this shape.
#[tokio::test]
async fn list_tool_permissions_empty_session_returns_empty_vec() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let rows = list_tool_permissions(&pool, &s.id).await.unwrap();
    assert!(rows.is_empty());
}

/// The three live match_kind dimensions (tool / prefix / path) all
/// round-trip through `list_tool_permissions` with their match_value
/// intact (NULL for tool, glob for path, token for prefix), so the
/// UI can render each row distinctly.
#[tokio::test]
async fn list_tool_permissions_returns_all_three_match_kinds() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    // tool kind (match_value = NULL) — e.g. web_fetch.
    grant_tool_permission(&pool, &s.id, "web_fetch", "tool", None)
        .await
        .unwrap();
    // path kind — e.g. read_file on a glob.
    grant_tool_permission(&pool, &s.id, "read_file", "path", Some("src/*"))
        .await
        .unwrap();
    // prefix kind — e.g. shell on a command prefix.
    grant_tool_permission(&pool, &s.id, "shell", "prefix", Some("cargo"))
        .await
        .unwrap();

    let rows = list_tool_permissions(&pool, &s.id).await.unwrap();
    assert_eq!(rows.len(), 3);

    // Find each by (tool_name, match_kind) — order is not guaranteed
    // to match insert order when granted_at ties (same second).
    let find = |kind: &str, tool: &str| {
        rows.iter()
            .find(|r| r.match_kind == kind && r.tool_name == tool)
            .unwrap_or_else(|| panic!("missing {kind} {tool} row"))
    };
    assert!(
        find("tool", "web_fetch").match_value.is_none(),
        "tool kind match_value must be NULL"
    );
    assert_eq!(
        find("path", "read_file").match_value.as_deref(),
        Some("src/*")
    );
    assert_eq!(
        find("prefix", "shell").match_value.as_deref(),
        Some("cargo")
    );
}

/// **design D2 — the NULL match_value pitfall**: revoking a
/// `match_kind='tool'` row (match_value IS NULL) must actually
/// delete it. A naive `WHERE match_value = ?` bound to NULL would
/// silently match 0 rows (revoke looks successful but the grant
/// survives). This test fails if the NULL branch is ever dropped.
#[tokio::test]
async fn revoke_tool_permission_null_value_tool_kind() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    grant_tool_permission(&pool, &s.id, "web_fetch", "tool", None)
        .await
        .unwrap();
    assert!(has_tool_permission(&pool, &s.id, "web_fetch")
        .await
        .unwrap());

    revoke_tool_permission(&pool, &s.id, "web_fetch", "tool", None)
        .await
        .unwrap();
    assert!(
        !has_tool_permission(&pool, &s.id, "web_fetch")
            .await
            .unwrap(),
        "revoking a tool-kind (NULL match_value) grant must actually delete it (design D2)"
    );
    let rows = list_tool_permissions(&pool, &s.id).await.unwrap();
    assert!(rows.is_empty());
}

/// Revoking one PK row must NOT touch sibling grants on the same
/// `tool_name` under a different `match_kind`/`match_value`. This
/// is the whole reason revoke is per-PK, not per-tool — e.g.
/// revoking one read_file path glob must keep the others.
#[tokio::test]
async fn revoke_tool_permission_preserves_sibling_grants() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    grant_tool_permission(&pool, &s.id, "read_file", "path", Some("src/*"))
        .await
        .unwrap();
    grant_tool_permission(&pool, &s.id, "read_file", "path", Some("docs/*"))
        .await
        .unwrap();
    grant_tool_permission(&pool, &s.id, "shell", "prefix", Some("cargo"))
        .await
        .unwrap();

    // Revoke just the src/* path row.
    revoke_tool_permission(&pool, &s.id, "read_file", "path", Some("src/*"))
        .await
        .unwrap();

    let rows = list_tool_permissions(&pool, &s.id).await.unwrap();
    assert_eq!(rows.len(), 2, "only the src/* row must be deleted");
    assert!(
        !rows
            .iter()
            .any(|r| r.tool_name == "read_file" && r.match_value.as_deref() == Some("src/*")),
        "the revoked src/* row must be gone"
    );
    assert!(
        rows.iter()
            .any(|r| r.tool_name == "read_file" && r.match_value.as_deref() == Some("docs/*")),
        "the sibling docs/* row must survive"
    );
    assert!(
        rows.iter()
            .any(|r| r.tool_name == "shell" && r.match_value.as_deref() == Some("cargo")),
        "the unrelated shell prefix row must survive"
    );
}

/// Revoking a grant on session A must not affect session B's grants
/// (the DELETE is scoped by session_id).
#[tokio::test]
async fn revoke_tool_permission_is_session_scoped() {
    let pool = make_pool().await;
    let a = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let b = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    grant_tool_permission(&pool, &a.id, "web_fetch", "tool", None)
        .await
        .unwrap();
    grant_tool_permission(&pool, &b.id, "web_fetch", "tool", None)
        .await
        .unwrap();

    revoke_tool_permission(&pool, &a.id, "web_fetch", "tool", None)
        .await
        .unwrap();
    assert!(!has_tool_permission(&pool, &a.id, "web_fetch")
        .await
        .unwrap());
    assert!(
        has_tool_permission(&pool, &b.id, "web_fetch")
            .await
            .unwrap(),
        "session B's grant must survive revoking session A's"
    );
}

/// E2 (2026-07-14): verify that `record_audit_event` with
/// `turn_seq = Some(seq)` persists the value, and `list_audit_events`
/// returns it. Also verifies `None` round-trips (IPC handlers without
/// turn context).
#[tokio::test]
async fn record_audit_event_persists_turn_seq() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    // Write with Some(turn_seq) — simulates an in-turn-loop audit.
    record_audit_event(
        &pool,
        &s.id,
        "tool_executed",
        Some(r#"{"tool_name":"shell"}"#),
        Some(5),
    )
    .await
    .unwrap();

    // Write with None — simulates an IPC-handler audit.
    record_audit_event(
        &pool,
        &s.id,
        "mode_changed",
        Some(r#"{"new_mode":"yolo"}"#),
        None,
    )
    .await
    .unwrap();

    let rows = list_audit_events(&pool, &s.id).await.unwrap();
    assert_eq!(rows.len(), 2);

    // Rows are ordered ts DESC — both inserted in the same second,
    // so order by rowid (insertion order) as a stable tiebreaker.
    // The tool_executed row was inserted first.
    let tool_row = rows.iter().find(|r| r.kind == "tool_executed").unwrap();
    assert_eq!(tool_row.turn_seq, Some(5));

    let mode_row = rows.iter().find(|r| r.kind == "mode_changed").unwrap();
    assert_eq!(mode_row.turn_seq, None);
}

// ---------------------------------------------------------------------------
// RULE-PERM-001 (2026-08-30): keyset-paginated audit read
// (`list_audit_events_page`) — AC1 ordering/cursor/limit, AC2 filters,
// AC3 counts.
// ---------------------------------------------------------------------------

/// Seed one audit row with an **explicit** `ts`. `record_audit_event`
/// stamps `datetime('now')` (1s resolution), which can't pin
/// same-second vs cross-second determinism for the ordering/cursor
/// tests — these go through raw INSERT instead (same column set the
/// production writer uses; `turn_seq` NULL).
async fn seed_audit_row(
    pool: &SqlitePool,
    session_id: &str,
    ts: &str,
    kind: &str,
    payload_json: Option<&str>,
) -> i64 {
    let rec = sqlx::query(
        r#"
        INSERT INTO session_audit_events (session_id, ts, kind, payload_json, turn_seq)
        VALUES (?, ?, ?, ?, NULL)
        "#,
    )
    .bind(session_id)
    .bind(ts)
    .bind(kind)
    .bind(payload_json)
    .execute(pool)
    .await
    .unwrap();
    rec.last_insert_rowid()
}

/// AC1: `ORDER BY ts DESC, id DESC` — same-second rows must tie-break
/// by id (newest id first), and a strictly-newer `ts` must sort ahead
/// of the whole older second. This is the SQL-side guarantee the
/// frontend's old `sortEvents` second key relied on (R4); the paged
/// read makes it authoritative so the UI can stop re-sorting.
#[tokio::test]
async fn audit_page_orders_ts_desc_id_desc_tie_break() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    // Four rows in the same wall-clock second (the norm: one turn can
    // fire several tool audits), inserted in ascending id order, plus
    // one newer-second row inserted *first* (id 1) to prove ts
    // dominates id.
    let new_ts = seed_audit_row(&pool, &s.id, "2026-08-30 10:00:01", "mode_changed", None).await;
    let id1 = seed_audit_row(&pool, &s.id, "2026-08-30 10:00:00", "tool_allowed", None).await;
    let id2 = seed_audit_row(&pool, &s.id, "2026-08-30 10:00:00", "tool_allowed", None).await;
    let id3 = seed_audit_row(&pool, &s.id, "2026-08-30 10:00:00", "tool_denied", None).await;
    let id4 = seed_audit_row(&pool, &s.id, "2026-08-30 10:00:00", "tool_denied", None).await;

    let page = list_audit_events_page(&pool, &s.id, AuditEventPageQuery::default())
        .await
        .unwrap();
    let ids: Vec<i64> = page.events.iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![new_ts, id4, id3, id2, id1]);
}

/// AC1 (R5): the keyset cursor is stable when a newer row is appended
/// mid-pagination. Page 1 → INSERT a newer row → cursor-fetch page 2:
/// the combined sequence must have **no duplicates and no gaps**.
///
/// This is exactly the scenario OFFSET pagination fails: after the
/// append the newest row occupies offset slot 0, so `LIMIT 5 OFFSET 5`
/// returns `6,5,4,3,2` — re-delivering id 6 (duplicate) while id 1
/// falls off the end (gap). The keyset cursor anchors on
/// `(ts, id)` of the last-seen row, so appended newer rows never
/// shift earlier pages.
#[tokio::test]
async fn audit_page_keyset_stable_when_new_row_appended_mid_pagination() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    // Ten rows, ts strictly increasing (so the DESC order is
    // unambiguous) and ids ascending in lockstep: id N ↔ 10:00:0N.
    for i in 1..=10_i64 {
        let ts = format!("2026-08-30 10:00:{:02}", i); // 10:00:01 .. 10:00:10
        seed_audit_row(&pool, &s.id, &ts, "tool_allowed", None).await;
    }

    // Page 1 (limit 5) → newest five: ids [10, 9, 8, 7, 6].
    let page1 = list_audit_events_page(
        &pool,
        &s.id,
        AuditEventPageQuery {
            limit: Some(5),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let page1_ids: Vec<i64> = page1.events.iter().map(|r| r.id).collect();
    assert_eq!(page1_ids, vec![10, 9, 8, 7, 6]);

    // Mid-pagination append: a newer audit row lands while the user
    // is reading page 1 (agent running, modal open).
    seed_audit_row(&pool, &s.id, "2026-08-30 10:00:11", "tool_denied", None).await;

    // Cursor = last row of page 1 (id 6's (ts, id)); fetch page 2.
    let anchor = page1.events.last().unwrap();
    let page2 = list_audit_events_page(
        &pool,
        &s.id,
        AuditEventPageQuery {
            limit: Some(5),
            before_ts: Some(anchor.ts.clone()),
            before_id: Some(anchor.id),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // Full walk = the original ten ids, newest first — no 11, no
    // duplicate 6, no missing 1 (the OFFSET failure mode above).
    let mut all_ids = page1_ids.clone();
    all_ids.extend(page2.events.iter().map(|r| r.id));
    assert_eq!(all_ids, vec![10, 9, 8, 7, 6, 5, 4, 3, 2, 1]);
    // The appended row is visible to a fresh first page, not retro-
    // injected into the in-flight walk.
    assert_eq!(page2.events.len(), 5);
}

/// AC1 + design C3: `limit` default = 100, values above 500 clamp to
/// the cap, and values below 1 clamp to 1 (SQLite would read a
/// negative LIMIT as *unlimited* — the clamp is what keeps the cap
/// honest against a hostile/negative request).
#[tokio::test]
async fn audit_page_respects_limit_and_caps() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    // 600 rows with distinct ascending ts: enough to observe both the
    // default (100) and the cap (500) in one seed.
    for i in 0..600_i64 {
        let minute = i / 60;
        let sec = i % 60;
        let ts = format!(
            "2026-08-30 {:02}:{:02}:{:02}",
            10 + minute / 60,
            minute % 60,
            sec
        );
        seed_audit_row(&pool, &s.id, &ts, "tool_allowed", None).await;
    }

    // Default: exactly AUDIT_PAGE_DEFAULT_LIMIT rows of the 600.
    let page = list_audit_events_page(&pool, &s.id, AuditEventPageQuery::default())
        .await
        .unwrap();
    assert_eq!(page.events.len(), AUDIT_PAGE_DEFAULT_LIMIT as usize);
    assert_eq!(page.matched, 600);

    // Explicit small limit is honored verbatim.
    let page = list_audit_events_page(
        &pool,
        &s.id,
        AuditEventPageQuery {
            limit: Some(7),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(page.events.len(), 7);

    // Over-cap request clamps to AUDIT_PAGE_MAX_LIMIT (not 600).
    let page = list_audit_events_page(
        &pool,
        &s.id,
        AuditEventPageQuery {
            limit: Some(10_000),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(page.events.len(), AUDIT_PAGE_MAX_LIMIT as usize);

    // Sub-1 request (incl. the LIMIT -1 = unlimited footgun) clamps to 1.
    for bad in [Some(0), Some(-3)] {
        let page = list_audit_events_page(
            &pool,
            &s.id,
            AuditEventPageQuery {
                limit: bad,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(page.events.len(), 1, "limit {bad:?} must clamp to 1");
    }
}

/// AC2: the `kind` filter is pushed to SQL and returns exactly the
/// subset — every returned row carries the requested kind, `matched`
/// counts the subset, and `total_all`/`total_critical` stay unfiltered.
#[tokio::test]
async fn audit_page_kind_filter_matches_subset() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    seed_audit_row(&pool, &s.id, "2026-08-30 10:00:01", "tool_allowed", None).await;
    seed_audit_row(&pool, &s.id, "2026-08-30 10:00:02", "tool_allowed", None).await;
    seed_audit_row(&pool, &s.id, "2026-08-30 10:00:03", "tool_denied", None).await;
    seed_audit_row(&pool, &s.id, "2026-08-30 10:00:04", "mode_changed", None).await;

    let page = list_audit_events_page(
        &pool,
        &s.id,
        AuditEventPageQuery {
            kind: Some("tool_allowed".to_string()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(page.events.len(), 2);
    assert!(page.events.iter().all(|r| r.kind == "tool_allowed"));
    assert_eq!(page.matched, 2, "matched counts the filtered subset");
    assert_eq!(page.total_all, 4, "total_all ignores the kind filter");
    assert_eq!(page.total_critical, 0, "no critical payloads seeded");

    // A kind with no rows: empty page, zero matched, totals intact.
    let page = list_audit_events_page(
        &pool,
        &s.id,
        AuditEventPageQuery {
            kind: Some("no_such_kind".to_string()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(page.events.is_empty());
    assert_eq!(page.matched, 0);
    assert_eq!(page.total_all, 4);
}

/// AC2 + R7: the critical filter's four payload states. Only JSON
/// payloads carrying `$.critical = 1` count as critical; `false`,
/// NULL payload, and **malformed JSON** must all classify as
/// non-critical WITHOUT erroring (the `json_valid` guard precedes
/// `json_extract`; the client-side `isCritical` tolerance this
/// replaces never errored on these either).
#[tokio::test]
async fn audit_page_critical_filter_four_states() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let critical_id = seed_audit_row(
        &pool,
        &s.id,
        "2026-08-30 10:00:01",
        "tool_denied",
        Some(r#"{"critical":true,"tool_name":"shell"}"#),
    )
    .await;
    seed_audit_row(
        &pool,
        &s.id,
        "2026-08-30 10:00:02",
        "tool_allowed",
        Some(r#"{"critical":false,"tool_name":"read_file"}"#),
    )
    .await;
    // State 3: NULL payload_json entirely.
    seed_audit_row(&pool, &s.id, "2026-08-30 10:00:03", "mode_changed", None).await;
    // State 4: malformed JSON text — json_extract alone would raise,
    // the json_valid guard makes this a plain non-critical row.
    seed_audit_row(
        &pool,
        &s.id,
        "2026-08-30 10:00:04",
        "tool_denied",
        Some("{not valid json"),
    )
    .await;

    // Must not error — the `.unwrap()` here IS the R7 assertion.
    let page = list_audit_events_page(
        &pool,
        &s.id,
        AuditEventPageQuery {
            critical_only: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(page.events.len(), 1);
    assert_eq!(page.events[0].id, critical_id);
    assert_eq!(page.matched, 1);
    assert_eq!(page.total_all, 4);
    assert_eq!(page.total_critical, 1);

    // And the inverse read (critical_only = false) still returns all
    // four rows untouched.
    let page = list_audit_events_page(&pool, &s.id, AuditEventPageQuery::default())
        .await
        .unwrap();
    assert_eq!(page.events.len(), 4);
}

/// AC3: the three counts are exact against seeded rows, in isolation
/// and under every filter combination. Key invariant (R3): the
/// critical count is NOT narrowed by the kind filter, and `matched`
/// tracks the active kind/critical combination while the two totals
/// never move.
#[tokio::test]
async fn audit_page_counts_exact_with_and_without_filters() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    // Deterministic mix: 6 rows — 4 tool_denied (2 critical),
    // 2 mode_changed (0 critical) → totals 6 / 2.
    let seed = [
        (
            "2026-08-30 10:00:01",
            "tool_denied",
            Some(r#"{"critical":true}"#),
        ),
        (
            "2026-08-30 10:00:02",
            "tool_denied",
            Some(r#"{"critical":false}"#),
        ),
        (
            "2026-08-30 10:00:03",
            "tool_denied",
            Some(r#"{"critical":true}"#),
        ),
        ("2026-08-30 10:00:04", "tool_denied", None),
        ("2026-08-30 10:00:05", "mode_changed", None),
        ("2026-08-30 10:00:06", "mode_changed", None),
    ];
    for (ts, kind, payload) in seed {
        seed_audit_row(&pool, &s.id, ts, kind, payload).await;
    }

    // Unfiltered: matched == total_all == 6, total_critical == 2.
    let page = list_audit_events_page(&pool, &s.id, AuditEventPageQuery::default())
        .await
        .unwrap();
    assert_eq!(page.matched, 6);
    assert_eq!(page.total_all, 6);
    assert_eq!(page.total_critical, 2);

    // kind only: matched narrows to 4; critical count stays 2 (R3 —
    // both critical rows are tool_denied here, but the point is the
    // count is computed without the kind predicate).
    let page = list_audit_events_page(
        &pool,
        &s.id,
        AuditEventPageQuery {
            kind: Some("tool_denied".to_string()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(page.matched, 4);
    assert_eq!(page.total_all, 6);
    assert_eq!(page.total_critical, 2);

    // critical only: matched = 2, totals unchanged.
    let page = list_audit_events_page(
        &pool,
        &s.id,
        AuditEventPageQuery {
            critical_only: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(page.matched, 2);
    assert_eq!(page.total_all, 6);
    assert_eq!(page.total_critical, 2);

    // kind + critical: matched = 2 (both criticals are tool_denied).
    let page = list_audit_events_page(
        &pool,
        &s.id,
        AuditEventPageQuery {
            kind: Some("tool_denied".to_string()),
            critical_only: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(page.matched, 2);

    // kind + critical with a disjoint kind: matched = 0 while the
    // totals still report 6 / 2 (chip semantics for a filter combo
    // that matches nothing).
    let page = list_audit_events_page(
        &pool,
        &s.id,
        AuditEventPageQuery {
            kind: Some("mode_changed".to_string()),
            critical_only: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(page.matched, 0);
    assert_eq!(page.total_all, 6);
    assert_eq!(page.total_critical, 2);
}

/// AC1/AC3: a session with zero audit rows (or a nonexistent id)
/// yields a zeroed page — empty `events`, three zero counters — NOT
/// an error. The modal renders its empty-state placeholder against
/// this shape.
#[tokio::test]
async fn audit_page_empty_session_returns_zeroed_page() {
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let page = list_audit_events_page(&pool, &s.id, AuditEventPageQuery::default())
        .await
        .unwrap();
    assert!(page.events.is_empty());
    assert_eq!(page.matched, 0);
    assert_eq!(page.total_all, 0);
    assert_eq!(page.total_critical, 0);

    // Nonexistent session id: same zeroed page.
    let page = list_audit_events_page(
        &pool,
        "no-such-session",
        AuditEventPageQuery {
            critical_only: true,
            kind: Some("tool_denied".to_string()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(page.events.is_empty());
    assert_eq!(page.matched, 0);
    assert_eq!(page.total_all, 0);
    assert_eq!(page.total_critical, 0);
}

/// Wire-shape lock for the page envelope (mirrors
/// `audit_event_row_serializes_to_camel_case_wire_shape`): the page
/// row crosses IPC with camelCase keys (`totalAll` / `totalCritical`),
/// never snake_case, per the db-guidelines house rule.
#[tokio::test]
async fn audit_event_page_row_serializes_to_camel_case_wire_shape() {
    use crate::db::permissions::{AuditEventPageRow, AuditEventRow};
    let row = AuditEventPageRow {
        events: vec![AuditEventRow {
            id: 1,
            session_id: "sess-abc".to_string(),
            ts: "2026-08-30 10:00:00".to_string(),
            kind: "tool_executed".to_string(),
            payload_json: None,
            turn_seq: None,
        }],
        matched: 1,
        total_all: 9,
        total_critical: 3,
    };
    let v: serde_json::Value = serde_json::to_value(&row).unwrap();
    let obj = v.as_object().expect("page must serialize to JSON object");

    assert!(
        obj.contains_key("totalAll")
            && obj.contains_key("totalCritical")
            && obj.contains_key("matched")
            && obj.contains_key("events"),
        "wire shape must use camelCase totalAll/totalCritical + matched/events, got: {:?}",
        obj.keys().collect::<Vec<_>>()
    );
    assert!(
        !obj.contains_key("total_all") && !obj.contains_key("total_critical"),
        "wire shape must NOT leak snake_case count keys"
    );
    assert_eq!(obj.get("totalAll").and_then(|v| v.as_i64()), Some(9));
    assert_eq!(obj.get("totalCritical").and_then(|v| v.as_i64()), Some(3));
    assert_eq!(obj["events"][0]["sessionId"], "sess-abc");
}
