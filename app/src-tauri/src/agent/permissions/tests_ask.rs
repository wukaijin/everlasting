#![cfg(test)]

use crate::agent::permissions::ask::{ask_path, build_ask_reason};
use crate::agent::permissions::resolve_ask;
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
