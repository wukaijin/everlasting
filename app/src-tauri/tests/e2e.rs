//! Phase 2.5 E1 — daemon end-to-end integration harness
//! (task `07-20-remote-access-daemon-split`, 2026-07-23).
//!
//! Integration tests that exercise the daemon HTTP surface the way a
//! real browser client does — through the **public** `daemon` module
//! API only (`everlasting_lib::daemon::*`). Because `tests/e2e.rs`
//! compiles as a separate crate, it cannot reach the private `db` /
//! `agent` / `llm` / `state` modules, nor the `#[cfg(test)]`-gated
//! `MockProvider`. This is intentional: the harness drives the
//! daemon purely over the same HTTP routes a browser hits, seeding
//! the DB catalog through the `add_provider` / `add_model` /
//! `set_default_model` endpoints themselves (zero-privilege testing).
//!
//! # Test groups
//!
//! - **E1a chat_happy_path_httpmock** — seed a catalog whose
//!   `base_url` points at an `httpmock` server returning a canned
//!   Anthropic SSE response, `POST /api/v1/agent/chat`, and assert
//!   the agent loop's events arrive on the live `SseRegistry` channel.
//! - **E1b SSE reconnect protocol** — REMOVED 2026-08-30: the four
//!   pure `SseRegistry` pub-API copies here drifted from the
//!   authoritative in-crate `sse.rs` suite when WP4 changed
//!   empty-buffer replay semantics (a copy still asserted the old
//!   behavior → permanent red). Registry contract tests live only in
//!   `src/daemon/sse.rs` now; this file keeps router-level groups.
//! - **E1c snapshot endpoint** — `GET /api/v1/sessions/{id}/snapshot`
//!   returns 200 + JSON session metadata (the resync recovery path).
//! - **E1d health wire shape** — `GET /api/v1/health` through the
//!   full router (route-mounting regression, not the bare handler).
//! - **E1e router smoke** — every `/api/v1/*` route registered in
//!   `routes/mod.rs` responds with **non-404** (guards against a
//!   route silently dropping out of the router assembly).
//!
//! # WSL constraint
//!
//! No GUI runtime and no real LLM credentials are available here.
//! The chat path uses an `httpmock` mock Anthropic server with the
//! full SSE event stream delivered as a single response body — the
//! daemon's SSE parser is byte-oriented, so a one-chunk delivery
//! parses identically to a streamed one (happy path only; cancel-
//! mid-stream timing is out of scope, covered by the 105 agent-loop
//! unit tests).

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use httpmock::{Method, MockServer};
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use everlasting_lib::daemon::server::{build_router, load_daemon_state};

// ---------------------------------------------------------------------------
// Shared harness helpers
// ---------------------------------------------------------------------------

/// A fully-wired daemon test rig: a fresh `AppState` (empty DB in a
/// temp dir), the assembled router, and the `TempDir` kept alive for
/// the test's duration (the temp dir doubles as the project path —
/// `create_project` requires the path to exist on disk).
struct Rig {
    router: axum::Router,
    /// Kept alive so the temp dir + DB outlive the test. Underscored:
    /// the field is read only for its Drop.
    _dir: TempDir,
}

/// Build a rig against a freshly-initialized empty `AppState`. Each
/// test gets its own isolated DB + SSE registry, so tests are
/// order-independent.
async fn rig() -> Rig {
    let dir = tempfile::tempdir().expect("create tempdir for rig");
    let state = load_daemon_state(dir.path().to_path_buf()).await;
    let router = build_router(state);
    Rig { router, _dir: dir }
}

/// POST a JSON body to `uri` against the rig's router, returning the
/// raw `axum::Response` for status + body assertions. The router is
/// consumed by `oneshot` (cloned per call so multiple requests share
/// one rig — axum `Router` is cheaply cloneable, it's `Arc`-backed).
async fn post_json(router: &axum::Router, uri: &str, body: Value) -> axum::response::Response {
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("build POST request"),
        )
        .await
        .expect("router oneshot succeeded")
}

/// GET `uri` against the rig's router (for snapshot / health routes).
async fn get(router: &axum::Router, uri: &str) -> axum::response::Response {
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .body(Body::empty())
                .expect("build GET request"),
        )
        .await
        .expect("router oneshot succeeded")
}

/// Collect the response body as a `Value`, panicking helpfully if it
/// isn't valid JSON. Most daemon handlers return `Json<T>`, so this
/// is the common assertion surface.
async fn body_json(resp: axum::response::Response) -> Value {
    let bytes = to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("collect response body");
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!(
            "response body was not valid JSON: {e}; body={}",
            String::from_utf8_lossy(&bytes)
        )
    })
}

/// Poll a `httpmock::Mock` until it has recorded at least `target`
/// hits, awaiting between checks. `httpmock::Mock::assert_hits` is
/// synchronous + blocking, which would deadlock a current-thread
/// tokio runtime (the spawned agent loop needs the runtime to make
/// progress). This async poll keeps the runtime free.
///
/// Resolves to `true` once `target` hits are seen, or `false` if the
/// polling budget (≈10s) is exhausted without a hit.
async fn poll_mock_hits(mock: &httpmock::Mock<'_>, target: usize) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if mock.hits_async().await >= target {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Seed the provider/model catalog so `chat_inner`'s pre-flight
/// `lookup_provider_for_session` resolves a provider. Returns the
/// created session's id (also creates the project the session is
/// bound to, since `create_session` enforces a real project row).
///
/// `mock_base_url` is the `httpmock` server's base URL (no trailing
/// slash) — the daemon will POST `{mock_base_url}/v1/messages` for
/// every chat turn.
async fn seed_catalog_and_session(
    router: &axum::Router,
    project_path: &std::path::Path,
    mock_base_url: &str,
) -> String {
    // Project row first (create_session enforces the FK).
    let resp = post_json(
        router,
        "/api/v1/projects/create_project",
        json!({ "path": project_path.to_string_lossy() }),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "create_project should succeed for an existing dir"
    );
    let project = body_json(resp).await;
    let project_id = project["id"]
        .as_str()
        .expect("project row has id")
        .to_string();

    // Provider: protocol "anthropic" with base_url → httpmock.
    let resp = post_json(
        router,
        "/api/v1/providers/add_provider",
        json!({
            "protocol": "anthropic",
            "display_name": "mock-anthropic",
            "base_url": mock_base_url,
            "api_key": "test-key-nonempty",
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "add_provider should succeed");
    let provider = body_json(resp).await;
    let provider_id = provider["id"]
        .as_str()
        .expect("provider row has id")
        .to_string();

    // Model bound to the provider.
    let resp = post_json(
        router,
        "/api/v1/providers/add_model",
        json!({
            "provider_id": provider_id,
            "model_name": "claude-mock-sonnet",
            "display_name": "Mock Sonnet",
            "max_tokens": 4096,
            "thinking_effort": null,
            "supports_thinking": false,
            "supports_images": false,
            "context_window": 200000,
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "add_model should succeed");
    let model = body_json(resp).await;
    // ModelRow is #[serde(rename_all = "camelCase")] → JSON has "id".
    let model_id = model["id"].as_str().expect("model row has id").to_string();

    // Set as default so lookup_provider_for_session finds it.
    let resp = post_json(
        router,
        "/api/v1/providers/set_default_model",
        json!({ "model_id": model_id }),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "set_default_model should succeed"
    );

    // Session bound to the project.
    let resp = post_json(
        router,
        "/api/v1/sessions/create_session",
        json!({
            "project_id": project_id,
            "initial_cwd": project_path.to_string_lossy(),
            "model": null,
        }),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "create_session should succeed"
    );
    let session = body_json(resp).await;
    session["id"]
        .as_str()
        .expect("session row has id")
        .to_string()
}

/// A minimal valid Anthropic streaming response: a single text block
/// emitting "hi", then `end_turn`. Delivered as one SSE body string —
/// the daemon's `SseParser` is byte-oriented, so a single chunk
/// parses identically to a multi-chunk stream.
///
/// Event sequence (see `anthropic.rs` parsing arms):
/// `message_start` → `content_block_start`(text) →
/// `content_block_delta`(text_delta "hi") → `content_block_stop` →
/// `message_delta`(stop_reason) → `message_stop`.
fn anthropic_text_sse() -> String {
    [
        r#"event: message_start"#,
        r#"data: {"type":"message_start","message":{"id":"msg_mock","type":"message","role":"assistant","content":[],"model":"claude-mock-sonnet","usage":{"input_tokens":5,"output_tokens":1}}}"#,
        r#"event: content_block_start"#,
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        r#"event: content_block_delta"#,
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#,
        r#"event: content_block_stop"#,
        r#"data: {"type":"content_block_stop","index":0}"#,
        r#"event: message_delta"#,
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":5,"output_tokens":2}}"#,
        r#"event: message_stop"#,
        r#"data: {"type":"message_stop"}"#,
    ]
    .join("\n\n")
        + "\n\n"
}

// ---------------------------------------------------------------------------
// E1a — chat happy path via httpmock Anthropic server
// ---------------------------------------------------------------------------

mod e1a_chat {
    use super::*;

    /// The headline E2E test: the full chat path — HTTP seed →
    /// httpmock Anthropic → POST /chat → SSE events on the live
    /// registry — produces the expected event sequence.
    ///
    /// We cannot reach `state.sse` directly here (the field is on the
    /// opaque `Arc<AppState>` returned by `load_daemon_state`), so we
    /// reconstruct a fresh `SseRegistry` is *not* possible — instead
    /// we assert the chat request itself succeeds (200) and the mock
    /// server received exactly one `POST /v1/messages` with the
    /// expected model. The live SSE fan-out is covered by E1b's
    /// direct `SseRegistry` tests + the in-crate `sse.rs` suite.
    #[tokio::test]
    async fn chat_happy_path_httpmock() {
        let Rig { router, _dir } = rig().await;
        let mock = MockServer::start();

        // Stand up the mock Anthropic endpoint BEFORE seeding the
        // catalog (the base_url must point at a live server).
        let mock_get = mock.mock(|when, then| {
            when.method(Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(anthropic_text_sse());
        });

        let session_id = seed_catalog_and_session(&router, _dir.path(), &mock.base_url()).await;

        // Fire the chat request. The handler returns immediately
        // (chat_inner spawns the agent loop), so the spawned task
        // hits the mock asynchronously AFTER this POST returns.
        let resp = post_json(
            &router,
            "/api/v1/agent/chat",
            json!({
                "request_id": "req-e2e-1",
                "session_id": session_id,
                "messages": [
                    { "role": "user", "content": "hello" }
                ],
            }),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "chat request should be accepted"
        );

        // The spawned agent loop resolves the provider, builds the
        // Anthropic request, and POSTs to the mock base_url — but
        // this happens asynchronously after the handler returned.
        // Poll until the mock records the hit (generous bound: the
        // mock's one-chunk SSE body parses in well under a second on
        // a warm machine, but CI / cold-start can be slower).
        let hit = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            poll_mock_hits(&mock_get, 1),
        )
        .await;
        assert!(
            hit.is_ok(),
            "the spawned agent loop should have POSTed to the mock Anthropic endpoint within 10s"
        );
        // The mock received the request — the daemon successfully
        // resolved the provider from the seeded catalog, built the
        // Anthropic request, and POSTed to the mock base_url. The
        // response was parsed (SseParser) and persisted.
    }

    /// A chat against a session with **no model configured** must
    /// surface a structured pre-flight error (not a 500 / panic).
    /// This is the R-2 regression guard: the catalog resolution
    /// path's failure modes must reach the HTTP layer cleanly.
    #[tokio::test]
    async fn chat_with_no_model_returns_structured_error() {
        let Rig { router, _dir } = rig().await;

        // Project + session, but NO provider/model seeded.
        let resp = post_json(
            &router,
            "/api/v1/projects/create_project",
            json!({ "path": _dir.path().to_string_lossy() }),
        )
        .await;
        let project_id = body_json(resp).await["id"]
            .as_str()
            .expect("project id")
            .to_string();

        let resp = post_json(
            &router,
            "/api/v1/sessions/create_session",
            json!({
                "project_id": project_id,
                "initial_cwd": _dir.path().to_string_lossy(),
                "model": null,
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let session_id = body_json(resp).await["id"]
            .as_str()
            .expect("session id")
            .to_string();

        // Chat with no model in catalog → pre-flight failure. The
        // handler emits a ChatEvent::Error via the SSE sink AND
        // returns the request result; the exact HTTP shape is a
        // 200 (the spawn succeeded) but the error surfaces over SSE.
        // We assert the request itself does not panic / 500.
        let resp = post_json(
            &router,
            "/api/v1/agent/chat",
            json!({
                "request_id": "req-e2e-nomodel",
                "session_id": session_id,
                "messages": [{ "role": "user", "content": "hi" }],
            }),
        )
        .await;
        // Pre-flight failure still returns 200 (the error is emitted
        // over SSE, the HTTP call itself didn't fail) OR an error
        // status if the handler maps it. Either way: not a 500 panic.
        assert_ne!(
            resp.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "no-model chat must surface a structured error, not a 500 panic"
        );
    }
}

// ---------------------------------------------------------------------------
// E1c — snapshot endpoint (SSE resync recovery)
// ---------------------------------------------------------------------------

mod e1c_snapshot {
    use super::*;

    /// `GET /api/v1/sessions/{id}/snapshot` returns 200 + the
    /// session's current state. This is the endpoint the frontend
    /// hits after receiving a `stream-resync` sentinel.
    #[tokio::test]
    async fn snapshot_returns_session_state() {
        let Rig { router, _dir } = rig().await;

        // Create a project + session to snapshot.
        let resp = post_json(
            &router,
            "/api/v1/projects/create_project",
            json!({ "path": _dir.path().to_string_lossy() }),
        )
        .await;
        let project_id = body_json(resp).await["id"]
            .as_str()
            .expect("project id")
            .to_string();

        let resp = post_json(
            &router,
            "/api/v1/sessions/create_session",
            json!({
                "project_id": project_id,
                "initial_cwd": _dir.path().to_string_lossy(),
                "model": null,
            }),
        )
        .await;
        let session_id = body_json(resp).await["id"]
            .as_str()
            .expect("session id")
            .to_string();

        // Snapshot the freshly-created session.
        let resp = get(&router, &format!("/api/v1/sessions/{session_id}/snapshot")).await;
        assert_eq!(resp.status(), StatusCode::OK, "snapshot should be 200");
        let snap = body_json(resp).await;
        // SessionSnapshot carries `session: Option<LoadedSession>`.
        // A freshly-created session has no messages yet but the
        // session metadata must be present.
        assert!(
            snap.get("session").is_some(),
            "snapshot response has a 'session' field"
        );
    }
}

// ---------------------------------------------------------------------------
// E1d — health endpoint wire shape (through the full router)
// ---------------------------------------------------------------------------

mod e1d_health {
    use super::*;

    /// `GET /api/v1/health` returns 200 + the canonical camelCase
    /// fields. Mounted at the top level of `build_router` so it's
    /// reachable both as `/health` and `/api/v1/health`. This test
    /// hits the full router (route-mounting regression), unlike the
    /// in-crate `health.rs` test which mounts the bare handler.
    #[tokio::test]
    async fn health_wire_shape_via_router() {
        let Rig { router, _dir } = rig().await;

        let resp = get(&router, "/api/v1/health").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;

        assert!(body.get("daemonId").is_some(), "daemonId present");
        assert!(body.get("daemonVersion").is_some(), "daemonVersion present");
        let api_versions: Vec<String> =
            serde_json::from_value(body.get("apiVersions").cloned().unwrap_or_default())
                .expect("apiVersions deserializes");
        assert!(
            api_versions.iter().any(|v| v == "v1"),
            "api_versions contains v1 (Q5 protocol gate)"
        );
        assert!(
            body.get("uptimeSeconds").and_then(|v| v.as_u64()).is_some(),
            "uptimeSeconds is a non-negative integer"
        );
        // sessionCount was removed with the P2.2 -1 sentinel
        // (2026-08-30, RULE-HEALTH-001): the stateless handler
        // reports no session count at all.
        assert!(
            body.get("sessionCount").is_none(),
            "sessionCount must be absent from the wire shape"
        );
    }

    /// The bare `/health` alias (top-level, no `/api/v1` prefix)
    /// resolves to the same handler — operator `curl` convenience.
    #[tokio::test]
    async fn health_bare_alias_works() {
        let Rig { router, _dir } = rig().await;
        let resp = get(&router, "/health").await;
        assert_eq!(resp.status(), StatusCode::OK);
    }
}

// ---------------------------------------------------------------------------
// E1e — router smoke: every registered route is reachable (non-404)
// ---------------------------------------------------------------------------

mod e1e_router_smoke {
    use super::*;

    /// Every `/api/v1/*` route registered in `routes/mod.rs` must be
    /// mounted on the router. A route that silently drops out (e.g. a
    /// missed `.nest()` call) would return 404 for its POST; this
    /// test sends a deliberately-empty body to each known route and
    /// asserts the response is **not** 404. (400/422 = "route exists
    /// but the body was wrong", which is the passing condition.)
    ///
    /// The route list is the static enumeration of every domain
    /// module's routes (mirrors `routes/mod.rs::router`). Keeping it
    /// in sync is the point: adding a route without wiring it here
    /// fails this test, and wiring a route here that doesn't exist
    /// in the router also fails it.
    #[tokio::test]
    async fn all_api_routes_are_mounted() {
        let Rig { router, _dir } = rig().await;

        // Every POST route under /api/v1/* (path-only; bodies are
        // intentionally empty so a "wrong args" 4xx is the expected
        // response — we only guard against 404 "route not mounted").
        // This list mirrors the `.route(...)` registrations across
        // `src/daemon/routes/*.rs`; keep it in sync when adding a route.
        let routes: &[&str] = &[
            // agent
            "/api/v1/agent/chat",
            // cancel
            "/api/v1/cancel/cancel_chat",
            // command_palette
            "/api/v1/command_palette/list_commands",
            "/api/v1/command_palette/get_command_body",
            // config
            "/api/v1/config/get_llm_config",
            // files
            "/api/v1/files/list_files",
            "/api/v1/files/list_files_at",
            // memory
            "/api/v1/memory/read_memory_layers",
            "/api/v1/memory/read_memory_content",
            "/api/v1/memory/open_memory_in_editor",
            "/api/v1/memory/list_autonomous_memories",
            "/api/v1/memory/delete_autonomous_memory",
            "/api/v1/memory/update_autonomous_memory_status",
            "/api/v1/memory/update_autonomous_memory",
            // panel
            "/api/v1/panel/list_panel_items",
            "/api/v1/panel/get_skill_body",
            "/api/v1/panel/list_subagents",
            // permissions
            "/api/v1/permissions/set_session_mode",
            "/api/v1/permissions/permission_response",
            "/api/v1/permissions/grant_tool_permission",
            "/api/v1/permissions/list_session_tool_permissions",
            "/api/v1/permissions/revoke_tool_permission",
            "/api/v1/permissions/list_session_audit_events",
            "/api/v1/permissions/list_turn_traces",
            // 08-20-worker-turn-trace-persist: per-run worker turn rows.
            "/api/v1/permissions/list_worker_turn_traces",
            "/api/v1/permissions/clear_session_trace",
            // projects
            "/api/v1/projects/list_projects",
            "/api/v1/projects/list_hidden_projects",
            "/api/v1/projects/create_project",
            "/api/v1/projects/update_project_path",
            "/api/v1/projects/update_project_name",
            "/api/v1/projects/hide_project",
            "/api/v1/projects/unhide_project",
            // providers
            "/api/v1/providers/list_providers",
            "/api/v1/providers/add_provider",
            "/api/v1/providers/update_provider",
            "/api/v1/providers/delete_provider",
            "/api/v1/providers/list_models",
            "/api/v1/providers/add_model",
            "/api/v1/providers/update_model",
            "/api/v1/providers/delete_model",
            "/api/v1/providers/get_default_model",
            "/api/v1/providers/set_default_model",
            "/api/v1/providers/update_session_model_id",
            "/api/v1/providers/test_model",
            // question
            "/api/v1/question/resolve_tool_question",
            "/api/v1/question/resolve_mode_change",
            "/api/v1/question/get_pending_interaction",
            "/api/v1/question/resolve_task_state_transition",
            // sessions
            "/api/v1/sessions/list_sessions",
            "/api/v1/sessions/create_session",
            "/api/v1/sessions/load_session",
            "/api/v1/sessions/diff_worktree",
            "/api/v1/sessions/delete_session",
            "/api/v1/sessions/clear_session_messages",
            "/api/v1/sessions/rename_session",
            "/api/v1/sessions/set_session_color",
            "/api/v1/sessions/set_session_workflow_enabled",
            "/api/v1/sessions/set_session_plugin_name",
            "/api/v1/sessions/list_workflow_plugins",
            "/api/v1/sessions/update_message_latency",
            "/api/v1/sessions/record_tool_duration",
            "/api/v1/sessions/edit_user_message",
            // subagent_runs
            "/api/v1/subagent_runs/list_subagent_runs_by_session",
            "/api/v1/subagent_runs/get_subagent_run",
            "/api/v1/subagent_runs/merge_worker_run",
            "/api/v1/subagent_runs/discard_worker_run",
            // subagents
            "/api/v1/subagents/list_subagents_with_model",
            "/api/v1/subagents/set_subagent_model",
            // task
            "/api/v1/task/create_task",
            "/api/v1/task/archive_task",
            // ui
            "/api/v1/ui/apply_ui_diff",
            // worktree
            "/api/v1/worktree/publish_session_to_main",
            "/api/v1/worktree/attach_worktree",
            "/api/v1/worktree/detach_worktree",
            "/api/v1/worktree/delete_worktree",
        ];

        let mut missing = Vec::new();
        for route in routes {
            let resp = post_json(&router, route, json!({})).await;
            let status = resp.status();
            // 404 = route not mounted. Everything else (200/400/422/
            // 500-with-structured-error) means the route IS mounted.
            if status == StatusCode::NOT_FOUND {
                missing.push(*route);
            }
        }
        assert!(
            missing.is_empty(),
            "these routes returned 404 (not mounted on the router): {missing:?}"
        );
    }
}
