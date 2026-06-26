#![cfg(test)]

use crate::agent::permissions::ask::{ask_path, build_ask_reason};
use crate::agent::permissions::check::check;
use crate::agent::permissions::resolve_ask;
use crate::agent::permissions::run_grant::RunGrantCache;
use crate::agent::permissions::{Decision, PermissionResponse, Risk};

use super::tests_common::*;

/// build_ask_reason: path / shell / web_fetch produce
/// different reason shapes (Q1 "path-based 弹窗判定",
/// Q10 "保留 risk + path 范围行").
#[test]
fn build_ask_reason_mentions_path_for_path_tools() {
    let r = build_ask_reason("read_file", "/etc/passwd", Risk::High);
    assert!(r.contains("read_file"));
    assert!(r.contains("/etc/passwd"));
    assert!(r.contains("高"));
}

#[test]
fn build_ask_reason_mentions_command_for_shell() {
    let r = build_ask_reason("shell", "rm -rf /tmp/foo", Risk::High);
    assert!(r.contains("rm -rf /tmp/foo"));
}

#[test]
fn build_ask_reason_mentions_url_for_web_fetch() {
    let r = build_ask_reason("web_fetch", "https://example.com", Risk::Low);
    assert!(r.contains("https://example.com"));
}

// =====================================================================
// 2026-06-22 (RULE-FrontSubagent-003 fix): worker ask_path
// interactive flow tests. Pre-fix, worker asks collapsed to Deny
// silently (RULE-A-014, 2026-06-20) — workers had no way to
// surface tool_use approvals to the user. Post-fix, worker asks
// go through the same `tokio::select!{cancel, timeout, oneshot}`
// flow as parent asks, just with a worker-owned permission
// session id (`worker:<worker_run_id>`) so the worker's pending
// oneshot does not collide with the parent's pending asks.
//
// The tests below cover all 4 terminal states (allow, deny,
// timeout, cancel) + the permission_session_id isolation invariant.
// =====================================================================

/// Worker asks DO use a distinct permission session id
/// (`worker:<worker_run_id>`) as the INTERNAL store key for the
/// `register_ask` / `resolve_ask` oneshot, so they cannot collide
/// with the parent's pending asks. This is the load-bearing
/// invariant of the worker-ask rewrite — without it, the worker's
/// oneshot would either (a) evict a parent pending ask on cancel,
/// or (b) receive the parent's `permission_response` IPC reply.
///
/// The IPC payload's `session_id` field, however, must carry the
/// PARENT session id (not the composite) — the frontend
/// `WorkerAskBanner` groups worker asks by parent session via
/// `ask.sessionId === parentSessionId`
/// (`permissions.ts::pendingWorkerCountForSession`). Carrying the
/// composite on the wire was a cross-layer bug fixed in PR1.5
/// (2026-06-22) — the composite stays internal to the store key.
///
/// The test calls `ask_path` and asserts BOTH:
///   - IPC `payload.session_id == parent session id` (wire shape)
///   - `cancel_session_asks(parent_session_id)` does NOT drop the
///     worker's pending oneshot (proves the entry is keyed under
///     the composite `worker:<worker_run_id>`, not the parent
///     session id; if Fix #1 leaked into `register_ask`, the
///     parent-scoped cancel would drop it).
#[tokio::test]
async fn worker_ask_uses_isolated_permission_session_id() {
    let (pool, store, sink, ctx, token) = worker_ctx_with_db().await;
    let sink_arc: std::sync::Arc<dyn crate::state::ChatEventSink> = sink.clone();

    let store_for_task = store.clone();
    let ctx_for_task = ctx.clone();
    let pool_for_task = pool.clone();
    let token_for_task = token.clone();
    let handle = tokio::spawn(async move {
        ask_path(
            &sink_arc,
            &pool_for_task,
            &store_for_task,
            &ctx_for_task,
            "write_file",
            &serde_json::json!({"path": "/repo/outside/foo.rs"}),
            "/repo/outside/foo.rs",
            Some("/repo/outside/foo.rs"),
            "tu-worker-1",
            &token_for_task,
        )
        .await
    });
    // Give the spawn a moment to register the ask + emit IPC.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // IPC payload assertions.
    let captured_asks = sink.asks.lock().unwrap().clone();
    assert_eq!(captured_asks.len(), 1, "expected 1 IPC emit");
    let payload = &captured_asks[0];
    // PR1.5 (2026-06-22): the IPC `sessionId` MUST be the parent
    // session id (carries through to the frontend's banner filter).
    assert_eq!(
        payload.session_id, "parent-sess",
        "worker ask IPC payload must carry parent session id (for banner routing); \
         PR1.5 cross-layer fix"
    );
    assert_eq!(
        payload.session_id, ctx.session_id,
        "worker IPC payload session_id must equal parent ctx.session_id"
    );
    assert_eq!(
        payload.worker_run_id.as_deref(),
        Some("worker-run-1"),
        "IPC payload must carry worker_run_id"
    );
    assert_eq!(payload.tool_name, "write_file");
    assert_eq!(payload.tool_use_id, "tu-worker-1");

    // PR1.5 regression guard: the INTERNAL store key MUST still be
    // the composite `worker:<worker_run_id>`, NOT the parent
    // session id. If Fix #1 accidentally leaked into register_ask,
    // worker pending asks would collide with parent pending asks
    // (RULE-A-014 regression). We verify by calling
    // `cancel_session_asks(parent_session_id)` — this should NOT
    // drop the worker's pending entry (the entry is bound to the
    // composite session id, not the parent's). If the entry WAS
    // bound to `parent-sess`, the cancel would drop it and the
    // subsequent resolve_ask would fail (rid missing).
    crate::agent::permissions::cancel_session_asks(&store, "parent-sess").await;
    let rid = payload.rid.clone();
    let resolved = resolve_ask(
        &store,
        &rid,
        PermissionResponse::AllowOnce,
    )
    .await;
    assert!(
        resolved,
        "cancel_session_asks(parent_session_id) must NOT drop the worker's \
         pending oneshot — the entry is keyed under the composite \
         `worker:<worker_run_id>` (RULE-A-014 invariant). If this fires, \
         Fix #1 leaked into register_ask and worker/parent asks collide."
    );

    // Let the spawned task complete (resolve_ask above unblocked it).
    let decision = handle.await.expect("join handle");
    assert!(
        matches!(decision, Decision::Allow),
        "resolve_ask(AllowOnce) should win after the parent-scoped cancel \
         no-op'd; got {:?}",
        decision
    );
    // No audit row (RULE-A-016 — worker Allow must NOT write
    // parent's `session_audit_events`).
    let events = crate::db::permissions::list_audit_events(&pool, "parent-sess")
        .await
        .expect("list_audit_events");
    assert!(
        events.is_empty(),
        "RULE-A-016: worker Allow must NOT write any session_audit_events row; \
         got kinds: {:?}",
        events.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>()
    );
}

/// Worker ask resolved with `AllowOnce` → `Decision::Allow`.
/// No audit row is written (RULE-A-016 lineage — worker's
/// resolve events stay in the worker's transcript, NOT in the
/// parent's `session_audit_events`).
#[tokio::test]
async fn worker_ask_allowed_resolves_allow() {
    let (pool, store, sink, ctx, token) = worker_ctx_with_db().await;
    let sink_arc: std::sync::Arc<dyn crate::state::ChatEventSink> = sink.clone();

    // Spawn a resolver that sends `PermissionResponse::AllowOnce`
    // for the FIRST rid that appears in the store.
    let store_for_resolve = store.clone();
    let _resolve_task = tokio::spawn(async move {
        for _ in 0..1000 {
            let map = store_for_resolve.lock().await;
            if let Some((rid, _)) = map.iter().next() {
                let rid = rid.clone();
                drop(map);
                let _ = resolve_ask(
                    &store_for_resolve,
                    &rid,
                    PermissionResponse::AllowOnce,
                )
                .await;
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        panic!("worker ask did not register within 2s");
    });

    let decision = ask_path(
        &sink_arc,
        &pool,
        &store,
        &ctx,
        "write_file",
        &serde_json::json!({"path": "/repo/outside/foo.rs"}),
        "/repo/outside/foo.rs",
        Some("/repo/outside/foo.rs"),
        "tu-worker-1",
        &token,
    )
    .await;

    assert!(matches!(decision, Decision::Allow), "expected Allow, got {:?}", decision);

    // No new audit row (RULE-A-016 — worker Allow does NOT write
    // parent's `session_audit_events`). We assert the only rows
    // present are the pre-existing ones from the test harness
    // setup (none in this minimal harness), so the audit table is
    // empty after the worker allow.
    let events = crate::db::permissions::list_audit_events(&pool, "parent-sess")
        .await
        .expect("list_audit_events");
    assert!(
        events.is_empty(),
        "RULE-A-016: worker Allow must NOT write any session_audit_events \
         row; got kinds: {:?}",
        events.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>()
    );
}

/// Worker ask timed out (no resolve) → `Decision::Deny`.
/// No audit row is written (RULE-A-016 lineage).
#[tokio::test]
async fn worker_ask_timeout_resolves_deny() {
    let (pool, store, sink, ctx, _token) = worker_ctx_with_db().await;
    let sink_arc: std::sync::Arc<dyn crate::state::ChatEventSink> = sink.clone();

    // Outer 130s timeout so the natural 120s ASK_TIMEOUT path
    // runs to completion (instead of being killed by the test
    // runner). If the timeout arm fails to fire, the outer
    // timeout surfaces a clear panic message.
    let decision = tokio::time::timeout(
        std::time::Duration::from_secs(130),
        ask_path(
            &sink_arc,
            &pool,
            &store,
            &ctx,
            "write_file",
            &serde_json::json!({"path": "/repo/outside/foo.rs"}),
            "/repo/outside/foo.rs",
            Some("/repo/outside/foo.rs"),
            "tu-worker-timeout",
            &tokio_util::sync::CancellationToken::new(),
        ),
    )
    .await
    .expect("worker ask_path timed out (the 120s ASK_TIMEOUT arm did not fire)");
    assert!(
        matches!(decision, Decision::Deny { .. }),
        "expected Deny after timeout, got {:?}",
        decision
    );
    if let Decision::Deny { reason, critical } = &decision {
        assert!(reason.contains("timed out"), "reason: {}", reason);
        assert!(!critical);
    } else {
        panic!("expected Deny");
    }

    // No audit row (RULE-A-016 lineage).
    let events = crate::db::permissions::list_audit_events(&pool, "parent-sess")
        .await
        .expect("list_audit_events");
    assert!(
        events.is_empty(),
        "RULE-A-016: worker timeout must NOT write any session_audit_events \
         row; got kinds: {:?}",
        events.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>()
    );
}

/// Worker ask cancelled by parent token (user Stop) →
/// `Decision::Deny`. No audit row (RULE-A-016 lineage).
#[tokio::test]
async fn worker_ask_cancelled_resolves_deny() {
    let (pool, store, sink, ctx, token) = worker_ctx_with_db().await;
    let sink_arc: std::sync::Arc<dyn crate::state::ChatEventSink> = sink.clone();

    let store_for_task = store.clone();
    let pool_for_task = pool.clone();
    let ctx_for_task = ctx.clone();
    let token_for_task = token.clone();
    let handle = tokio::spawn(async move {
        ask_path(
            &sink_arc,
            &pool_for_task,
            &store_for_task,
            &ctx_for_task,
            "write_file",
            &serde_json::json!({"path": "/repo/outside/foo.rs"}),
            "/repo/outside/foo.rs",
            Some("/repo/outside/foo.rs"),
            "tu-worker-cancel",
            &token_for_task,
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    token.cancel();
    let decision = handle.await.expect("join handle");
    assert!(
        matches!(decision, Decision::Deny { .. }),
        "expected Deny after cancel, got {:?}",
        decision
    );
    if let Decision::Deny { reason, .. } = &decision {
        assert!(reason.contains("cancel"), "reason: {}", reason);
    }

    // No audit row (RULE-A-016 lineage).
    let events = crate::db::permissions::list_audit_events(&pool, "parent-sess")
        .await
        .expect("list_audit_events");
    assert!(
        events.is_empty(),
        "RULE-A-016: worker cancel must NOT write any session_audit_events \
         row; got kinds: {:?}",
        events.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>()
    );
}

/// Worker ask user-denied → `Decision::Deny` (the user's
/// reason is surfaced to the worker LLM via the tool_result).
/// No audit row (RULE-A-016 lineage).
#[tokio::test]
async fn worker_ask_user_deny_resolves_deny() {
    let (pool, store, sink, ctx, _token) = worker_ctx_with_db().await;
    let sink_arc: std::sync::Arc<dyn crate::state::ChatEventSink> = sink.clone();

    let store_for_resolve = store.clone();
    let _resolve_task = tokio::spawn(async move {
        for _ in 0..1000 {
            let map = store_for_resolve.lock().await;
            if let Some((rid, _)) = map.iter().next() {
                let rid = rid.clone();
                drop(map);
                let _ = resolve_ask(
                    &store_for_resolve,
                    &rid,
                    PermissionResponse::Deny { reason: "use git clean".to_string() },
                )
                .await;
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        panic!("worker ask did not register within 2s");
    });

    let decision = ask_path(
        &sink_arc,
        &pool,
        &store,
        &ctx,
        "write_file",
        &serde_json::json!({"path": "/repo/outside/foo.rs"}),
        "/repo/outside/foo.rs",
        Some("/repo/outside/foo.rs"),
        "tu-worker-deny",
        &_token,
    )
    .await;

    assert!(
        matches!(decision, Decision::Deny { .. }),
        "expected Deny after user deny, got {:?}",
        decision
    );
    if let Decision::Deny { reason, critical } = &decision {
        assert_eq!(reason, "use git clean");
        assert!(!critical);
    } else {
        panic!("expected Deny");
    }

    // No audit row (RULE-A-016 lineage).
    let events = crate::db::permissions::list_audit_events(&pool, "parent-sess")
        .await
        .expect("list_audit_events");
    assert!(
        events.is_empty(),
        "RULE-A-016: worker user-deny must NOT write any session_audit_events \
         row; got kinds: {:?}",
        events.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>()
    );
}

/// Wire-shape lock: the worker's IPC payload carries
/// `workerRunId` (camelCase) — frontend reads this to route
/// the ask to the SubagentDrawer instead of the parent
/// `<PermissionModal>`.
#[tokio::test]
async fn worker_ask_payload_carries_worker_run_id_camel_case() {
    use std::sync::Mutex as StdMutex;
    use crate::state::{ChatEventPayload, ToolCallPayload, ToolResultPayload};
    use crate::agent::permissions::PermissionAskPayload;

    let (pool, store, _sink, ctx, token) = worker_ctx_with_db().await;

    #[derive(Default)]
    struct LocalSink {
        asks: StdMutex<Vec<PermissionAskPayload>>,
    }
    impl crate::state::ChatEventSink for LocalSink {
        fn emit_chat_event(&self, _p: &ChatEventPayload) {}
        fn emit_tool_call(&self, _p: &ToolCallPayload) {}
        fn emit_tool_result(&self, _p: &ToolResultPayload) {}
        fn emit_permission_ask(
            &self,
            p: PermissionAskPayload,
        ) {
            self.asks.lock().unwrap().push(p);
        }
    }
    let sink_local = std::sync::Arc::new(LocalSink::default());
    let sink_dyn: std::sync::Arc<dyn crate::state::ChatEventSink> = sink_local.clone();

    let store_for_task = store.clone();
    let pool_for_task = pool.clone();
    let ctx_for_task = ctx.clone();
    let token_for_task = token.clone();
    let handle = tokio::spawn(async move {
        let _ = ask_path(
            &sink_dyn,
            &pool_for_task,
            &store_for_task,
            &ctx_for_task,
            "write_file",
            &serde_json::json!({"path": "/repo/outside/foo.rs"}),
            "/repo/outside/foo.rs",
            Some("/repo/outside/foo.rs"),
            "tu-worker-wire",
            &token_for_task,
        )
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    token.cancel();
    let _ = handle.await;

    let asks = sink_local.asks.lock().unwrap().clone();
    assert_eq!(asks.len(), 1);
    let s = serde_json::to_string(&asks[0]).unwrap();
    assert!(
        s.contains("\"workerRunId\":\"worker-run-1\""),
        "wire shape must carry workerRunId camelCase: {}",
        s
    );
}

// =====================================================================
// 2026-06-26 (task 06-26-subagent-per-run-grant): per-run grant
// cache integration tests. Covers the three DoD invariants that
// the unit tests in `tests_run_grant.rs` cannot reach on their own
// (they test the cache type in isolation; these tests exercise the
// ask.rs / check.rs wiring end-to-end):
//
// 1. worker AllowAlways writes the per-run cache (NOT
//    `session_tool_permissions`) — parent session grant table stays
//    empty (zero-privilege-leakage).
// 2. worker AllowOnce does NOT write the cache — only the explicit
//    AllowAlways arm calls `grant_for_run`.
// 3. `check()` consults the per-run cache before falling through to
//    `ask_path` — a pre-filled cache hit short-circuits to Allow
//    WITHOUT emitting a `permission:ask` IPC payload.
//
// These tests are the regression guards for the rules the PRD
// Acceptance Criteria pin. The cache type tests in
// `tests_run_grant.rs` lock the data-structure semantics; these
// lock the wiring.
// =====================================================================

/// Helper: count rows in `session_tool_permissions` for a given
/// session. Used to assert "worker AllowAlways did NOT pollute the
/// parent's grant table" (zero is the only acceptable count — the
/// test harness starts with an empty table and the worker path
/// must never insert into it).
async fn count_session_tool_permissions(pool: &sqlx::SqlitePool, session_id: &str) -> i64 {
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM session_tool_permissions WHERE session_id = ?",
    )
    .bind(session_id)
    .fetch_one(pool)
    .await
    .expect("count session_tool_permissions");
    count.0
}

/// Worker ask AllowAlways writes the per-run in-memory cache (NOT
/// `session_tool_permissions`). After the resolve:
///
/// - `Decision::Allow` (AllowAlways arm)
/// - The cache (`ctx.run_grants`) holds exactly ONE grant for the
///   approved tool/kind/value
/// - The parent session's `session_tool_permissions` table is EMPTY
///   (RULE-A-016 isolation — worker grants do NOT cross the
///   privilege boundary into the parent's persistent grant table)
///
/// This test is the load-bearing regression guard for AC #2
/// ("零污染") + AC #5 ("AllowAlways 写 run cache") in the PRD.
#[tokio::test]
async fn worker_ask_allow_always_writes_run_grant_cache_not_db() {
    let (pool, store, sink, _ctx, token) = worker_ctx_with_db().await;
    let sink_arc: std::sync::Arc<dyn crate::state::ChatEventSink> = sink.clone();

    // Construct a worker ctx that carries a per-run cache (the
    // default `worker_ctx_with_db` returns `run_grants: None`).
    let cache = std::sync::Arc::new(RunGrantCache::new());
    let ctx = crate::agent::permissions::PermissionContext {
        session_id: "parent-sess".to_string(),
        mode: crate::db::Mode::Edit,
        cwd: std::path::PathBuf::from("/repo"),
        is_worker: true,
        worker_run_id: Some("worker-run-grant".to_string()),
        run_grants: Some(cache.clone()),
    };

    // Snapshot the parent grant table BEFORE — must start empty.
    let before =
        count_session_tool_permissions(&pool, "parent-sess").await;
    assert_eq!(before, 0, "test harness: parent grant table must start empty");

    // Spawn a resolver that sends `PermissionResponse::AllowAlways`
    // for the first rid that registers.
    let store_for_resolve = store.clone();
    let _resolve_task = tokio::spawn(async move {
        for _ in 0..1000 {
            let map = store_for_resolve.lock().await;
            if let Some((rid, _)) = map.iter().next() {
                let rid = rid.clone();
                drop(map);
                let _ = resolve_ask(
                    &store_for_resolve,
                    &rid,
                    PermissionResponse::AllowAlways,
                )
                .await;
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        panic!("worker ask did not register within 2s");
    });

    let decision = ask_path(
        &sink_arc,
        &pool,
        &store,
        &ctx,
        // path tool → match_kind='path', match_value=parent + '/*'
        "write_file",
        &serde_json::json!({"path": "/repo/outside/foo.rs"}),
        "/repo/outside/foo.rs",
        Some("/repo/outside/foo.rs"),
        "tu-worker-allow-always",
        &token,
    )
    .await;

    // AllowAlways arm → Allow.
    assert!(
        matches!(decision, Decision::Allow),
        "expected Allow, got {:?}",
        decision
    );

    // Cache holds exactly ONE path-kind grant for write_file on the
    // approved parent dir glob (`/repo/outside/*`).
    assert_eq!(cache.len(), 1, "AllowAlways must write exactly one grant");
    assert!(
        cache.has_run_grant("write_file", "path", "/repo/outside/foo.rs"),
        "cache must match the approved path"
    );
    assert!(
        cache.has_run_grant("write_file", "path", "/repo/outside/other.rs"),
        "cache must match a sibling in the same dir (glob semantics)"
    );
    assert!(
        !cache.has_run_grant("write_file", "path", "/repo/elsewhere/x.rs"),
        "cache must NOT match a path outside the approved glob"
    );

    // Parent session grant table is STILL EMPTY — worker grants do
    // not leak across the privilege boundary.
    let after =
        count_session_tool_permissions(&pool, "parent-sess").await;
    assert_eq!(
        after, 0,
        "RULE-A-016: worker AllowAlways must NOT write to parent's \
         session_tool_permissions (zero leakage); got {} rows",
        after
    );
}

/// Worker ask AllowOnce does NOT write the per-run cache. Only the
/// AllowAlways arm calls `grant_for_run`. After an AllowOnce
/// resolve, the cache remains empty (a subsequent identical tool
/// call would re-emit `permission:ask`). This is AC #5 ("AllowOnce
/// 不写 cache") in the PRD.
#[tokio::test]
async fn worker_ask_allow_once_does_not_write_run_grant_cache() {
    let (pool, store, sink, _ctx, token) = worker_ctx_with_db().await;
    let sink_arc: std::sync::Arc<dyn crate::state::ChatEventSink> = sink.clone();

    let cache = std::sync::Arc::new(RunGrantCache::new());
    let ctx = crate::agent::permissions::PermissionContext {
        session_id: "parent-sess".to_string(),
        mode: crate::db::Mode::Edit,
        cwd: std::path::PathBuf::from("/repo"),
        is_worker: true,
        worker_run_id: Some("worker-run-once".to_string()),
        run_grants: Some(cache.clone()),
    };

    let store_for_resolve = store.clone();
    let _resolve_task = tokio::spawn(async move {
        for _ in 0..1000 {
            let map = store_for_resolve.lock().await;
            if let Some((rid, _)) = map.iter().next() {
                let rid = rid.clone();
                drop(map);
                let _ = resolve_ask(
                    &store_for_resolve,
                    &rid,
                    PermissionResponse::AllowOnce,
                )
                .await;
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        panic!("worker ask did not register within 2s");
    });

    let decision = ask_path(
        &sink_arc,
        &pool,
        &store,
        &ctx,
        "shell",
        &serde_json::json!({"command": "cargo test"}),
        "cargo test",
        None,
        "tu-worker-allow-once",
        &token,
    )
    .await;

    assert!(
        matches!(decision, Decision::Allow),
        "expected Allow, got {:?}",
        decision
    );

    // AllowOnce arm → cache stays empty.
    assert!(
        cache.is_empty(),
        "AllowOnce must NOT write the run-grant cache (only AllowAlways does); \
         got {} grants",
        cache.len()
    );
    assert!(
        !cache.has_run_grant("shell", "prefix", "cargo"),
        "AllowOnce must not authorize future cargo calls"
    );
}

/// `check()` consults the per-run cache before falling through to
/// `ask_path`. A pre-filled cache hit short-circuits to
/// `Decision::Allow` WITHOUT emitting a `permission:ask` IPC
/// payload — the worker's second identical call is silent. This is
/// AC #1 ("worker 第 2 次 web_fetch → 不弹窗,直接放行") in the PRD.
///
/// This is the integration-level guard: `tests_run_grant.rs` locks
/// the cache type semantics; this test locks that `check.rs`'s
/// Tier 4 WebFetch branch actually consults the cache.
#[tokio::test]
async fn check_worker_run_grant_hit_short_circuits_ask_path() {
    let (pool, store, sink, _ctx, token) = worker_ctx_with_db().await;
    let sink_arc: std::sync::Arc<dyn crate::state::ChatEventSink> = sink.clone();

    // Pre-fill the cache with a web_fetch tool-kind grant (mimicking
    // a prior AllowAlways click within the same run).
    let cache = std::sync::Arc::new(RunGrantCache::new());
    cache.grant_for_run(
        "web_fetch",
        &serde_json::json!({"url": "https://example.com/first"}),
        "https://example.com/first",
    );
    assert_eq!(cache.len(), 1, "precondition: cache has one grant");

    let ctx = crate::agent::permissions::PermissionContext {
        session_id: "parent-sess".to_string(),
        mode: crate::db::Mode::Edit,
        cwd: std::path::PathBuf::from("/repo"),
        is_worker: true,
        worker_run_id: Some("worker-run-hit".to_string()),
        run_grants: Some(cache.clone()),
    };

    // Call `check()` directly — the production entry point the
    // agent loop uses. web_fetch is ToolKind::WebFetch → Tier 4
    // WebFetch branch → session_tool_permissions miss (table empty)
    // → run-grant cache HIT (pre-filled) → Allow, no ask_path.
    let decision = check(
        &ctx,
        &store,
        &pool,
        &sink_arc,
        "web_fetch",
        &serde_json::json!({"url": "https://example.com/second"}),
        "tu-worker-cache-hit",
        &token,
    )
    .await;

    assert!(
        matches!(decision, Decision::Allow),
        "cache hit must short-circuit to Allow, got {:?}",
        decision
    );

    // CRITICAL: no `permission:ask` IPC payload was emitted. If the
    // cache hit path fell through to `ask_path`, the sink would
    // have captured one ask payload.
    let captured_asks = sink.asks.lock().unwrap().clone();
    assert!(
        captured_asks.is_empty(),
        "cache hit must NOT emit permission:ask; got {} payloads",
        captured_asks.len()
    );

    // Cache is READ-ONLY on the check path — the read must not
    // mutate the cache.
    assert_eq!(
        cache.len(),
        1,
        "check() read path must not mutate the cache"
    );
}

/// `check()` with `run_grants: None` (the PARENT path) never
/// touches the cache. This is the parent-zero-regression guard
/// (AC #1 in the PRD: "parent 路径误传 `Some` → 主对话行为零回归").
/// The parent path falls through to `check_tool_grant` (DB) →
/// miss → `ask_path` as before.
#[tokio::test]
async fn check_parent_path_with_none_run_grants_falls_through_to_ask() {
    let (pool, store, sink, ctx, token) = worker_ctx_with_db().await;
    let sink_arc: std::sync::Arc<dyn crate::state::ChatEventSink> = sink.clone();

    // ctx.run_grants is None (the default from worker_ctx_with_db).
    // Even though this ctx has is_worker=true (the harness default),
    // the parent PRODUCTION path passes run_grants=None, so this
    // test exercises the "None → skip cache block entirely" branch
    // common to parent + worker-without-cache.
    assert!(ctx.run_grants.is_none(), "precondition: parent path");

    // web_fetch with no grant + no cache → falls through to ask_path.
    // We don't await the ask (it would block on the oneshot); we
    // spawn it and cancel after observing the IPC emit to prove the
    // fall-through happened.
    let pool_t = pool.clone();
    let store_t = store.clone();
    let ctx_t = ctx.clone();
    let token_t = token.clone();
    let handle = tokio::spawn(async move {
        check(
            &ctx_t,
            &store_t,
            &pool_t,
            &sink_arc,
            "web_fetch",
            &serde_json::json!({"url": "https://example.com"}),
            "tu-parent-fallthrough",
            &token_t,
        )
        .await
    });

    // Give the spawn time to emit the ask.
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    token.cancel();
    let _ = handle.await;

    // The fall-through emitted a permission:ask — proving the
    // parent path (run_grants=None) does NOT consult any cache.
    let captured_asks = sink.asks.lock().unwrap().clone();
    assert_eq!(
        captured_asks.len(),
        1,
        "parent path (run_grants=None) must fall through to ask_path \
         and emit one permission:ask; got {}",
        captured_asks.len()
    );
}
