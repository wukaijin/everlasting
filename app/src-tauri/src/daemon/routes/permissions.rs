//! `POST /api/v1/permissions/<command>` handlers for the permissions domain.
//!
//! Phase 2.2 B5 skeleton. Each handler deserializes a JSON body
//! into the same args the Tauri command takes, forwards to
//! `crate::commands::permissions::xxx_inner` (Q0 decision — single
//! source of truth), and wraps the result in `Json(...)`. Errors
//! flow through `AppCommandError`'s `IntoResponse` impl.

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;

use crate::commands::permissions::{
    clear_session_trace_inner, grant_tool_permission_inner, list_session_audit_events_inner,
    list_session_audit_events_page_inner, list_session_tool_permissions_inner,
    list_turn_traces_inner, list_worker_turn_traces_inner, permission_response_inner,
    revoke_tool_permission_inner, set_session_mode_inner,
};
use crate::db;
use crate::error::AppCommandError;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct SetSessionModeRequest {
    pub session_id: String,
    pub mode: String,
}

pub async fn set_session_mode(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetSessionModeRequest>,
) -> Result<Json<db::SessionRow>, AppCommandError> {
    let result = set_session_mode_inner(&state, req.session_id, req.mode).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct GrantToolPermissionRequest {
    pub session_id: String,
    pub tool_name: String,
    pub match_kind: Option<String>,
    pub match_value: Option<String>,
}

pub async fn grant_tool_permission(
    State(state): State<Arc<AppState>>,
    Json(req): Json<GrantToolPermissionRequest>,
) -> Result<Json<()>, AppCommandError> {
    grant_tool_permission_inner(
        &state,
        req.session_id,
        req.tool_name,
        req.match_kind,
        req.match_value,
    )
    .await?;
    Ok(Json(()))
}

#[derive(Debug, Deserialize)]
pub struct ListSessionToolPermissionsRequest {
    pub session_id: String,
}

pub async fn list_session_tool_permissions(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ListSessionToolPermissionsRequest>,
) -> Result<Json<Vec<db::PermissionGrantRow>>, AppCommandError> {
    let result = list_session_tool_permissions_inner(&state, req.session_id).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct RevokeToolPermissionRequest {
    pub session_id: String,
    pub tool_name: String,
    pub match_kind: String,
    pub match_value: Option<String>,
}

pub async fn revoke_tool_permission(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RevokeToolPermissionRequest>,
) -> Result<Json<()>, AppCommandError> {
    revoke_tool_permission_inner(
        &state,
        req.session_id,
        req.tool_name,
        req.match_kind,
        req.match_value,
    )
    .await?;
    Ok(Json(()))
}

#[derive(Debug, Deserialize)]
pub struct ListSessionAuditEventsRequest {
    pub session_id: String,
}

pub async fn list_session_audit_events(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ListSessionAuditEventsRequest>,
) -> Result<Json<Vec<db::AuditEventRow>>, AppCommandError> {
    let result = list_session_audit_events_inner(&state, req.session_id).await?;
    Ok(Json(result))
}

/// RULE-PERM-001 (2026-08-30): keyset-paginated audit read. Body is
/// snake_case like every route in this file (`session_id`,
/// `before_ts`, `before_id`, `critical_only`); all cursor / filter /
/// limit fields are optional (missing = first page, unfiltered).
/// The `(ts, id)` cursor halves must travel together — a lone
/// `before_ts` is a 400 from the `_inner`'s validation.
#[derive(Debug, Deserialize)]
pub struct ListSessionAuditEventsPageRequest {
    pub session_id: String,
    pub limit: Option<i64>,
    pub before_ts: Option<String>,
    pub before_id: Option<i64>,
    pub kind: Option<String>,
    #[serde(default)]
    pub critical_only: bool,
}

pub async fn list_session_audit_events_page(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ListSessionAuditEventsPageRequest>,
) -> Result<Json<db::AuditEventPageRow>, AppCommandError> {
    let result = list_session_audit_events_page_inner(
        &state,
        req.session_id,
        db::AuditEventPageQuery {
            limit: req.limit,
            before_ts: req.before_ts,
            before_id: req.before_id,
            kind: req.kind,
            critical_only: req.critical_only,
        },
    )
    .await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct ListTurnTracesRequest {
    pub session_id: String,
}

pub async fn list_turn_traces(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ListTurnTracesRequest>,
) -> Result<Json<Vec<db::trace::TurnTraceRow>>, AppCommandError> {
    let result = list_turn_traces_inner(&state, req.session_id).await?;
    Ok(Json(result))
}

/// 08-20-worker-turn-trace-persist: per-run worker turn rows
/// (SubagentDrawer "Token 明细"). 路由名遵循「命令名即路径段」
/// 惯例(B1 hotfix 2 教训 —— 不做别名)。
#[derive(Debug, Deserialize)]
pub struct ListWorkerTurnTracesRequest {
    pub run_id: String,
}

pub async fn list_worker_turn_traces(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ListWorkerTurnTracesRequest>,
) -> Result<Json<Vec<db::trace::TurnTraceRow>>, AppCommandError> {
    let result = list_worker_turn_traces_inner(&state, req.run_id).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct ClearSessionTraceRequest {
    pub session_id: String,
}

pub async fn clear_session_trace(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ClearSessionTraceRequest>,
) -> Result<Json<()>, AppCommandError> {
    clear_session_trace_inner(&state, req.session_id).await?;
    Ok(Json(()))
}

/// `POST /api/v1/permissions/permission_response` — frontend
/// reply to a `permission:ask` round-trip. Phase 2.2 follow-up
/// (2026-07-21): this handler was missing from the initial route
/// table, which would have caused `agent_loop` to hang forever
/// (the oneshot parked in `PermissionStore` would never be
/// resolved). Body matches the Tauri command's args: `{rid,
/// decision, reason?}` where `decision ∈ {"allow_once",
/// "allow_always", "deny"}`. Returns `{resolved: bool}` — `false`
/// means the rid was unknown / stale (timed out or duplicate),
/// which the frontend treats as a benign no-op.
#[derive(Debug, Deserialize)]
pub struct PermissionResponseRequest {
    pub rid: String,
    pub decision: String,
    pub reason: Option<String>,
}

pub async fn permission_response(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PermissionResponseRequest>,
) -> Result<Json<bool>, AppCommandError> {
    let resolved = permission_response_inner(&state, req.rid, req.decision, req.reason).await?;
    Ok(Json(resolved))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/set_session_mode", post(set_session_mode))
        .route("/permission_response", post(permission_response))
        .route("/grant_tool_permission", post(grant_tool_permission))
        .route(
            "/list_session_tool_permissions",
            post(list_session_tool_permissions),
        )
        .route("/revoke_tool_permission", post(revoke_tool_permission))
        .route(
            "/list_session_audit_events",
            post(list_session_audit_events),
        )
        .route(
            "/list_session_audit_events_page",
            post(list_session_audit_events_page),
        )
        .route("/list_turn_traces", post(list_turn_traces))
        .route("/list_worker_turn_traces", post(list_worker_turn_traces))
        .route("/clear_session_trace", post(clear_session_trace))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    //! Route-level smoke for `list_session_audit_events_page`
    //! (RULE-PERM-001; mirrors the `scheduled_tasks.rs` router-oneshot
    //! precedent — spec daemon-server.md §6 "new IPC commands get a
    //! Router oneshot test"). The db-layer semantics (ordering /
    //! cursor / filters / counts) are pinned in
    //! `db::permissions_tests`; this module pins the transport
    //! wiring: route registered, snake_case body, camelCase page wire.

    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt; // oneshot

    async fn post_json(state: &Arc<AppState>, cmd: &str, body: &str) -> (StatusCode, String) {
        let app = router(state.clone());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/{cmd}"))
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).to_string())
    }

    /// One round trip through the route: unfiltered page (camelCase
    /// envelope + row shape, counts exact), critical_only filter
    /// narrow, and the partial-cursor 400 gate.
    #[tokio::test(flavor = "multi_thread")]
    async fn list_session_audit_events_page_route_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);

        let session_id = uuid::Uuid::new_v4().to_string();
        db::create_session(
            &state.db,
            &session_id,
            crate::projects::DEFAULT_PROJECT_ID,
            "/tmp/audit-page-route",
            "GLM-4.7",
            None,
            None,
            None,
        )
        .await
        .unwrap();
        // 3 rows, 1 critical: covers the count semantics over the wire.
        db::record_audit_event(
            &state.db,
            &session_id,
            "tool_denied",
            Some(r#"{"critical":true,"tool_name":"shell"}"#),
            None,
        )
        .await
        .unwrap();
        db::record_audit_event(
            &state.db,
            &session_id,
            "tool_allowed",
            Some(r#"{"critical":false}"#),
            None,
        )
        .await
        .unwrap();
        db::record_audit_event(&state.db, &session_id, "mode_changed", None, None)
            .await
            .unwrap();

        // Unfiltered first page: page envelope is camelCase
        // (matched / totalAll / totalCritical) and rows ride the same
        // camelCase shape as the full-pull command (sessionId).
        let (status, body) = post_json(
            &state,
            "list_session_audit_events_page",
            &format!(r#"{{"session_id":"{session_id}"}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "page route: {body}");
        let page: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(page["matched"], 3);
        assert_eq!(page["totalAll"], 3);
        assert_eq!(page["totalCritical"], 1);
        let events = page["events"].as_array().expect("events array");
        assert_eq!(events.len(), 3);
        assert_eq!(events[0]["sessionId"], session_id);
        assert!(
            !body.contains("total_all") && !body.contains("session_id\":"),
            "wire must not leak snake_case keys: {body}"
        );

        // critical_only pushed through the route body (snake_case).
        let (status, body) = post_json(
            &state,
            "list_session_audit_events_page",
            &format!(r#"{{"session_id":"{session_id}","critical_only":true}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "critical_only route: {body}");
        let page: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(page["matched"], 1);
        assert_eq!(page["totalAll"], 3, "totals stay unfiltered");
        assert_eq!(page["events"].as_array().unwrap().len(), 1);

        // Partial cursor (before_ts without before_id) → 400
        // InvalidRequest, not a 500 / silent mis-page.
        let (status, body) = post_json(
            &state,
            "list_session_audit_events_page",
            &format!(r#"{{"session_id":"{session_id}","before_ts":"2026-08-30 10:00:00"}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "partial cursor: {body}");
        assert!(
            body.contains("before_id"),
            "error names the missing cursor half: {body}"
        );
    }
}
