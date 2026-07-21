//! `POST /api/v1/agent/chat` — daemon chat handler(P2.3 C5).
//!
//! daemon 版的 `chat` 入口,和 Tauri `chat` 命令(`agent::chat::chat`)
//! 共用 [`chat_inner`] 编排逻辑(Q0 决议 — 业务逻辑单份)。唯一差异:
//! - sink = `HttpSseSink`(从 `state.sse` registry 构造)替代
//!   `AppHandleSink` —— agent loop 的事件经 SSE 推到浏览器,而非
//!   Tauri IPC。
//! - `app_opt = None` —— **渐进方案**(task implement.md C5):daemon
//!   路径没有 `AppHandle`,`run_chat_loop` 的 worker sink 注入走
//!   `SubagentBufferSink::new_without_app_handle`(buffer-only,无
//!   `subagent:event` SSE emit)。parent chat 完整可用;worker 仍运行
//!   + DB 记录,但前端 drawer 看不到实时 worker 事件(完整
//!   `HttpSseSubagentSink` 注入留 P2.3 收尾)。
//!
//! handler 立即返回空 body(`chat_inner` 内部 `tokio::spawn` agent
//! loop,fire-and-forget);前端 `httpTransport` 进入 LRU 缓存等
//! `chat-event` / `tool:call` / `tool:result` SSE 事件。

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;

use crate::agent::chat::chat_inner;
use crate::agent::subagent::ForcedDispatch;
use crate::daemon::sse::HttpSseSink;
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
    chat_inner(
        &state,
        req.request_id,
        req.session_id,
        req.messages,
        sink,
        None,
        req.resend_seq,
        req.forced_dispatch,
    )
    .await?;
    Ok(Json(()))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new().route("/chat", post(chat)).with_state(state)
}
