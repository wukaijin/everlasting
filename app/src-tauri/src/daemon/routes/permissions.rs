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
    list_session_tool_permissions_inner, list_turn_traces_inner, list_worker_turn_traces_inner,
    permission_response_inner, revoke_tool_permission_inner, set_session_mode_inner,
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
        .route("/list_turn_traces", post(list_turn_traces))
        .route("/list_worker_turn_traces", post(list_worker_turn_traces))
        .route("/clear_session_trace", post(clear_session_trace))
        .with_state(state)
}
