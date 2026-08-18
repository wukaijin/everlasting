//! `POST /api/v1/sessions/<command>` handlers for the sessions domain.
//!
//! Phase 2.2 B5 skeleton. Each handler deserializes a JSON body
//! into the same args the Tauri command takes, forwards to
//! `crate::commands::sessions::xxx_inner` (Q0 decision — single
//! source of truth), and wraps the result in `Json(...)`. Errors
//! flow through `AppCommandError`'s `IntoResponse` impl.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::agent::question_store::PendingInteractionEntry;
use crate::commands::question::get_pending_interaction_inner;
use crate::commands::sessions::{
    clear_session_messages_inner, compact_session_inner, create_session_inner,
    delete_session_inner, diff_worktree_inner, edit_user_message_inner,
    group_chat_cache_rates_inner, list_sessions_inner, load_session_inner,
    record_tool_duration_inner, rename_session_inner, search_messages_inner,
    set_session_color_inner, set_session_plugin_name_inner, set_session_workflow_enabled_inner,
    update_message_latency_inner, update_session_metadata_inner,
};
use crate::db;
use crate::error::AppCommandError;
use crate::git;
use crate::llm::types::MessageContent;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ListSessionsRequest {
    pub project_id: String,
}

pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ListSessionsRequest>,
) -> Result<Json<Vec<db::SessionSummary>>, AppCommandError> {
    let result = list_sessions_inner(&state, req.project_id).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub project_id: String,
    pub initial_cwd: String,
    pub model: Option<String>,
    // Group chat (07-29-group-chat, Phase 4 Step 3 TODO-E2):
    // session type discriminator + per-session JSON metadata.
    // Serpent-case to match the `CreateSession` Tauri command
    // + the `serde_json::Value` shape. `None` for classic chat.
    pub session_type: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

pub async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<db::SessionRow>, AppCommandError> {
    let result = create_session_inner(
        &state,
        req.project_id,
        req.initial_cwd,
        req.model,
        req.session_type,
        req.metadata,
    )
    .await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct LoadSessionRequest {
    pub session_id: String,
}

pub async fn load_session(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LoadSessionRequest>,
) -> Result<Json<Option<db::LoadedSession>>, AppCommandError> {
    let result = load_session_inner(&state, req.session_id).await?;
    Ok(Json(result))
}

/// `GET /api/v1/sessions/{id}/snapshot` — resync 快照(P2.3 C3)。
///
/// 前端 `httpTransport` 收到 `stream-resync` sentinel 后调此端点:
/// 一次拿回完整 session(`load_session_inner` = session 元数据 +
/// 全部 messages)+ 当前 pending interaction(permission /
/// question / mode_change / task_state_transition 卡片),用快照
/// 替换 store 后重画 UI(design §2.4)。复用既有 `_inner`,无新
/// 业务逻辑——snapshot 是 `load_session` + `get_pending_interaction`
/// 的合并 GET。
#[derive(Debug, Serialize)]
pub struct SessionSnapshot {
    pub session: Option<db::LoadedSession>,
    pub pending_interaction: Option<PendingInteractionEntry>,
}

pub async fn snapshot(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<SessionSnapshot>, AppCommandError> {
    let session = load_session_inner(&state, id.clone()).await?;
    let pending_interaction = get_pending_interaction_inner(&state, id).await?;
    Ok(Json(SessionSnapshot {
        session,
        pending_interaction,
    }))
}

#[derive(Debug, Deserialize)]
pub struct DiffWorktreeRequest {
    pub session_id: String,
}

pub async fn diff_worktree(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DiffWorktreeRequest>,
) -> Result<Json<git::diff::DiffResult>, AppCommandError> {
    let result = diff_worktree_inner(&state, req.session_id).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct DeleteSessionRequest {
    pub session_id: String,
}

pub async fn delete_session(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DeleteSessionRequest>,
) -> Result<Json<()>, AppCommandError> {
    delete_session_inner(&state, req.session_id).await?;
    Ok(Json(()))
}

#[derive(Debug, Deserialize)]
pub struct ClearSessionMessagesRequest {
    pub session_id: String,
}

pub async fn clear_session_messages(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ClearSessionMessagesRequest>,
) -> Result<Json<()>, AppCommandError> {
    clear_session_messages_inner(&state, req.session_id).await?;
    Ok(Json(()))
}

#[derive(Debug, Deserialize)]
pub struct RenameSessionRequest {
    pub session_id: String,
    pub new_title: String,
}

pub async fn rename_session(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RenameSessionRequest>,
) -> Result<Json<()>, AppCommandError> {
    rename_session_inner(&state, req.session_id, req.new_title).await?;
    Ok(Json(()))
}

#[derive(Debug, Deserialize)]
pub struct SetSessionColorRequest {
    pub session_id: String,
    pub color_tag: Option<i32>,
}

pub async fn set_session_color(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetSessionColorRequest>,
) -> Result<Json<()>, AppCommandError> {
    set_session_color_inner(&state, req.session_id, req.color_tag).await?;
    Ok(Json(()))
}

#[derive(Debug, Deserialize)]
pub struct SetSessionWorkflowEnabledRequest {
    pub session_id: String,
    pub enabled: bool,
}

pub async fn set_session_workflow_enabled(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetSessionWorkflowEnabledRequest>,
) -> Result<Json<()>, AppCommandError> {
    set_session_workflow_enabled_inner(&state, req.session_id, req.enabled).await?;
    Ok(Json(()))
}

#[derive(Debug, Deserialize)]
pub struct SetSessionPluginNameRequest {
    pub session_id: String,
    pub name: String,
}

pub async fn set_session_plugin_name(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetSessionPluginNameRequest>,
) -> Result<Json<()>, AppCommandError> {
    set_session_plugin_name_inner(&state, req.session_id, req.name).await?;
    Ok(Json(()))
}

#[derive(Debug, Deserialize)]
pub struct UpdateMessageLatencyRequest {
    pub session_id: String,
    pub seq: i64,
    pub ttfb_ms: Option<i64>,
    pub gen_ms: Option<i64>,
    pub total_ms: Option<i64>,
    pub thinking_ms: Option<i64>,
}

pub async fn update_message_latency(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateMessageLatencyRequest>,
) -> Result<Json<bool>, AppCommandError> {
    let result = update_message_latency_inner(
        &state,
        req.session_id,
        req.seq,
        req.ttfb_ms,
        req.gen_ms,
        req.total_ms,
        req.thinking_ms,
    )
    .await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct RecordToolDurationRequest {
    pub session_id: String,
    pub tool_use_id: String,
    pub duration_ms: i64,
}

pub async fn record_tool_duration(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RecordToolDurationRequest>,
) -> Result<Json<bool>, AppCommandError> {
    let result =
        record_tool_duration_inner(&state, req.session_id, req.tool_use_id, req.duration_ms)
            .await?;
    Ok(Json(result))
}

// Group chat (07-29-group-chat, Phase 4 Step 3 TODO-E4):
// REST mirror of the `update_session_metadata` Tauri command.
// The IPC body carries the per-session JSON metadata blob
// (typically `{participants: [...]}` for group-chat sessions);
// backend writes it verbatim to `sessions.metadata` (TEXT
// column, JSON string).
#[derive(Debug, Deserialize)]
pub struct UpdateSessionMetadataRequest {
    pub session_id: String,
    pub metadata: serde_json::Value,
}

pub async fn update_session_metadata(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateSessionMetadataRequest>,
) -> Result<Json<()>, AppCommandError> {
    update_session_metadata_inner(&state, req.session_id, req.metadata).await?;
    Ok(Json(()))
}

#[derive(Debug, Deserialize)]
pub struct EditUserMessageRequest {
    pub session_id: String,
    pub message_seq: i64,
    pub new_content: MessageContent,
}

pub async fn edit_user_message(
    State(state): State<Arc<AppState>>,
    Json(req): Json<EditUserMessageRequest>,
) -> Result<Json<()>, AppCommandError> {
    edit_user_message_inner(&state, req.session_id, req.message_seq, req.new_content).await?;
    Ok(Json(()))
}

/// `POST /api/v1/sessions/group_chat_cache_rates` — per-speaker
/// (participants + "moderator") latest-turn cache-usage read for
/// the group-chat edit modal (08-10-group-chat-cache-rate).
#[derive(Debug, Deserialize)]
pub struct GroupChatCacheRatesRequest {
    pub session_id: String,
}

pub async fn group_chat_cache_rates(
    State(state): State<Arc<AppState>>,
    Json(req): Json<GroupChatCacheRatesRequest>,
) -> Result<Json<Vec<db::trace::SpeakerCacheUsage>>, AppCommandError> {
    let result = group_chat_cache_rates_inner(&state, req.session_id).await?;
    Ok(Json(result))
}

/// `POST /api/v1/sessions/search_messages` — D2 cross-session
/// full-text search (08-17-cross-session-search). POST per the
/// transport-wide `invoke` contract (all CMD_TO_DOMAIN commands
/// POST; the domain's only GET `/:id/snapshot` is a transport
/// special-case, not a precedent). Title rider + content hits in
/// one round-trip; ≥3-char queries dispatch FTS5, shorter ones
/// fall back to LIKE (2-char Chinese words must stay searchable).
#[derive(Debug, Deserialize)]
pub struct SearchMessagesRequest {
    pub query: String,
    pub project_id: Option<String>,
    pub limit: Option<u32>,
}

pub async fn search_messages(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SearchMessagesRequest>,
) -> Result<Json<Vec<db::search::MessageSearchHit>>, AppCommandError> {
    let result = search_messages_inner(&state, req.query, req.project_id, req.limit).await?;
    Ok(Json(result))
}

/// `POST /api/v1/sessions/list_workflow_plugins` — discover
/// workflow plugins under `<project>/.everlasting/workflow/`.
/// Phase 2.2 follow-up (2026-07-21): this handler was missing
/// from the initial route table even though the Tauri command
/// existed. No `AppState` dependency — the lookup is a pure
/// filesystem read via `crate::agent::workflow::list_plugins`.
#[derive(Debug, Deserialize)]
pub struct ListWorkflowPluginsRequest {
    pub project_path: String,
}

pub async fn list_workflow_plugins(
    Json(req): Json<ListWorkflowPluginsRequest>,
) -> Result<Json<Vec<String>>, AppCommandError> {
    Ok(Json(crate::agent::workflow::list_plugins(
        &req.project_path,
    )))
}

/// `POST /api/v1/sessions/compact_session` — 手动 /compact
/// (08-18-manual-compact-command):空闲期摘要压缩,可选 focus
/// 定向指令。gate 链(群聊/开关/in-flight/provider)全在
/// `compact_session_inner`;此处只做 transport 解包。
#[derive(Debug, Deserialize)]
pub struct CompactSessionRequest {
    pub session_id: String,
    pub focus: Option<String>,
}

pub async fn compact_session(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CompactSessionRequest>,
) -> Result<Json<crate::agent::compaction::ManualCompactionOutcome>, AppCommandError> {
    let result = compact_session_inner(&state, req.session_id, req.focus).await?;
    Ok(Json(result))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/list_sessions", post(list_sessions))
        .route("/create_session", post(create_session))
        .route("/load_session", post(load_session))
        .route("/update_session_metadata", post(update_session_metadata))
        .route("/:id/snapshot", get(snapshot))
        .route("/diff_worktree", post(diff_worktree))
        .route("/delete_session", post(delete_session))
        .route("/clear_session_messages", post(clear_session_messages))
        .route("/rename_session", post(rename_session))
        .route("/set_session_color", post(set_session_color))
        .route(
            "/set_session_workflow_enabled",
            post(set_session_workflow_enabled),
        )
        .route("/set_session_plugin_name", post(set_session_plugin_name))
        .route("/list_workflow_plugins", post(list_workflow_plugins))
        .route("/update_message_latency", post(update_message_latency))
        .route("/record_tool_duration", post(record_tool_duration))
        .route("/edit_user_message", post(edit_user_message))
        .route("/group_chat_cache_rates", post(group_chat_cache_rates))
        .route("/search_messages", post(search_messages))
        .route("/compact_session", post(compact_session))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::llm::types::{ContentBlock, MessageContent, Role};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt; // oneshot

    /// D2 route-level smoke (spec backend/daemon-server.md §6 —
    /// new IPC commands get a Router oneshot test). Seeds a
    /// session + one user message through the production db layer
    /// (exercising both the FTS insert trigger and the
    /// first-user-message auto-title), then POSTs the
    /// `/search_messages` route and checks both hit kinds come
    /// back in one round-trip.
    #[tokio::test(flavor = "multi_thread")]
    async fn search_messages_route_returns_title_and_content_hits() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);
        let pool = &state.db;
        let project = db::projects::create_project(pool, "d2route", "/tmp/d2_route", false, None)
            .await
            .unwrap();
        let session = db::sessions::create_session(
            pool,
            "d2-route-sess-1",
            &project.id,
            "/tmp/d2_route",
            "GLM-4.7",
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let content = MessageContent::Blocks(vec![ContentBlock::Text {
            text: "路由层冒烟:关于 pineapple 的讨论".to_string(),
            cache_control: None,
        }]);
        db::sessions::persist_turn(pool, &session.id, Role::User, &content, 0, None, None)
            .await
            .unwrap();

        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/search_messages")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"query":"pineapple"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let hits: Vec<db::search::MessageSearchHit> = serde_json::from_slice(&body).unwrap();
        assert_eq!(hits.len(), 2, "auto-title + content both hit: {hits:?}");
        assert!(hits
            .iter()
            .any(|h| h.kind == db::search::SearchHitKind::Title));
        let content_hit = hits
            .iter()
            .find(|h| h.kind == db::search::SearchHitKind::Content)
            .unwrap();
        assert_eq!(content_hit.seq, Some(0));
        assert_eq!(content_hit.session_id, session.id);
        assert!(content_hit
            .snippet
            .as_deref()
            .unwrap()
            .contains("pineapple"));
    }

    /// Manual /compact route smoke (spec backend/daemon-server.md §6):
    /// POST `/compact_session` against a **group-chat** session. The
    /// group gate fires before provider resolution, so the route test
    /// needs no models/catalog — it proves the transport wiring (JSON
    /// parse → `_inner` → gate) plus the scope gate's user-readable
    /// rejection in one round-trip. The full manual-compaction pipeline
    /// (mock provider, watermark, seq) is covered in
    /// `agent::tests_agent_loop::manual_compaction`.
    #[tokio::test(flavor = "multi_thread")]
    async fn compact_session_route_rejects_group_chat() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);
        let pool = &state.db;
        let project = db::projects::create_project(pool, "gcsmoke", "/tmp/gc_smoke", false, None)
            .await
            .unwrap();
        let session = db::sessions::create_session(
            pool,
            "gc-compact-smoke-1",
            &project.id,
            "/tmp/gc_smoke",
            "GLM-4.7",
            None,
            Some("group_chat"),
            None,
        )
        .await
        .unwrap();

        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/compact_session")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"session_id":"{}","focus":null}}"#,
                        session.id
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&body).to_string();
        assert!(text.contains("群聊"), "group gate message: {text}");
    }
}
