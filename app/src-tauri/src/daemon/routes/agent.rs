//! `POST /api/v1/agent/chat` — daemon chat handler(P2.3 C5).
//!
//! daemon 版的 `chat` 入口,和 Tauri `chat` 命令(`agent::chat::chat`)
//! 共用 [`chat_inner`] 编排逻辑(Q0 决议 — 业务逻辑单份)。唯一差异:
//! - sink 注入 = `HttpSseSink`(parent chat)+ `HttpSseSubagentSink`
//!   (worker)—— P2.4 C5 (2026-07-22) 完成完整 `SubagentEventSink`
//!   注入:daemon 路径 worker `subagent:event` 现经 SSE live 推送
//!   (P2.3 时 buffer-only,本提交闭合)。两个 sink 各自独立注入
//!   (Phase 1 §3.3 承诺),在 dispatch.rs 经 `new_with_event_sink`
//!   汇合 —— 替代 Tauri 路径的 `AppHandleSink` /
//!   `AppHandleSubagentSink`。
//!
//! handler 立即返回空 body(`chat_inner` 内部 `tokio::spawn` agent
//! loop,fire-and-forget);前端 `httpTransport` 进入 LRU 缓存等
//! `chat-event` / `tool:call` / `tool:result` SSE 事件。

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;

use crate::agent::chat::chat_inner;
use crate::agent::subagent::{ForcedDispatch, SubagentEventSink};
use crate::daemon::sse::{HttpSseSink, HttpSseSubagentSink};
use crate::error::AppCommandError;
use crate::llm::ChatMessage;
use crate::state::{AppState, ChatEventSink};

/// `POST /api/v1/agent/chat` 请求体。字段 snake_case(design §3.2,
/// 与其他 domain handler 一致;前端 `httpTransport` 在 C6/C8 负责
/// Tauri camelCase 特例 `resendSeq` / `forcedDispatch` → HTTP
/// snake_case 的字段映射)。
#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub request_id: String,
    pub session_id: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub resend_seq: Option<i64>,
    #[serde(default)]
    pub forced_dispatch: Option<ForcedDispatch>,
}

pub async fn chat(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ChatRequest>,
) -> Result<Json<()>, AppCommandError> {
    let sink: Arc<dyn ChatEventSink> = Arc::new(HttpSseSink {
        registry: state.sse.clone(),
    });
    // P2.4 C5 (2026-07-22): inject the worker's `SubagentEventSink` —
    // daemon-path worker `subagent:event` now streams over SSE live
    // (was buffer-only pre-C5, the gap this closes). Mirrors the
    // Tauri path's `AppHandleSubagentSink` through the same
    // `new_with_event_sink` seam in dispatch.rs.
    let worker_event_sink: Arc<dyn SubagentEventSink> = Arc::new(HttpSseSubagentSink {
        registry: state.sse.clone(),
    });
    chat_inner(
        &state,
        req.request_id,
        req.session_id,
        req.messages,
        sink,
        Some(state.catalog.clone()),
        worker_event_sink,
        req.resend_seq,
        req.forced_dispatch,
    )
    .await?;
    Ok(Json(()))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new().route("/chat", post(chat)).with_state(state)
}
