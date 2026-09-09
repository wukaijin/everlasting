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
) -> Result<Json<crate::agent::chat::ChatAcceptance>, AppCommandError> {
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
    let acceptance = chat_inner(
        &state,
        crate::agent::chat::ChatEntry {
            request_id: req.request_id,
            session_id: req.session_id,
            messages: req.messages,
            sink,
            worker_catalog: Some(state.catalog.clone()),
            worker_event_sink,
            resend_seq: req.resend_seq,
            forced_dispatch: req.forced_dispatch,
            // F2 origin:HTTP chat 入口无来源标记(仅调度器 fire 路径传)。
            origin: None,
            resume_group_chat: None,
        },
    )
    .await?;
    Ok(Json(acceptance))
}

// GCE P1a(09-06-gc-p1a-checkpoint-resume):续跑中断的群聊讨论。
// 与 chat 路由同款 sink 注入(HttpSseSink + HttpSseSubagentSink),
// 校验 + ChatEntry 组装在 `resume_group_chat_inner`(Q0 决议 —
// 业务逻辑单份,preempt_group_chat 三件套先例)。
#[derive(Debug, Deserialize)]
pub struct ResumeGroupChatRequest {
    pub session_id: String,
}

pub async fn resume_group_chat(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ResumeGroupChatRequest>,
) -> Result<Json<crate::agent::chat::ChatAcceptance>, AppCommandError> {
    let sink: Arc<dyn ChatEventSink> = Arc::new(HttpSseSink {
        registry: state.sse.clone(),
    });
    let worker_event_sink: Arc<dyn SubagentEventSink> = Arc::new(HttpSseSubagentSink {
        registry: state.sse.clone(),
    });
    let acceptance = crate::agent::chat::resume_group_chat_inner(
        &state,
        req.session_id,
        sink,
        worker_event_sink,
    )
    .await?;
    Ok(Json(acceptance))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/chat", post(chat))
        .route("/resume_group_chat", post(resume_group_chat))
        .with_state(state)
}

#[cfg(test)]
mod resume_tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt; // oneshot

    use super::router;
    use crate::db;
    use crate::state::AppState;

    /// GCE P1a(task 09-06-gc-p1a-checkpoint-resume):`POST
    /// /agent/resume_group_chat` 五类校验拒绝路径 + happy-path 受理。
    /// 校验失败经 `AppCommandError` 返回非 2xx + 明确文案;happy path
    /// 返回 `{"status":"started"}`(编排 spawn 后台跑,空 catalog 下
    /// moderator 解析失败轮空转至 max_rounds,测试只断言受理)。
    #[tokio::test(flavor = "multi_thread")]
    async fn resume_group_chat_route_validates_five_gates() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);
        let pool = &state.db;
        let project = db::projects::create_project(pool, "p1aroute", "/tmp/p1a_route", false, None)
            .await
            .unwrap();

        // ① 非 group_chat session。
        let classic = db::sessions::create_session(
            pool,
            "p1a-classic",
            &project.id,
            "/tmp/p1a_route",
            "GLM-4.7",
            None,
            None,
            None,
        )
        .await
        .unwrap();

        // 群聊 session 甲:interrupted 态 + 断点行(round 1)→ happy path。
        let group_meta = serde_json::json!({"participants": []});
        let resumed = db::sessions::create_session(
            pool,
            "p1a-resumed",
            &project.id,
            "/tmp/p1a_route",
            "GLM-4.7",
            None,
            Some("group_chat"),
            Some(&group_meta.to_string()),
        )
        .await
        .unwrap();
        db::sessions::finalize_group_chat_lifecycle(pool, &resumed.id, "interrupted", None, None)
            .await
            .unwrap();
        db::sessions::upsert_group_chat_checkpoint(pool, &resumed.id, 1, 0)
            .await
            .unwrap();

        // 群聊 session 乙:终局 stop_reason + 残留行(评审 P1-1:模拟
        // 「finalize 成功但删行失败」)→ 必须被第 ⑤ 类校验拒绝。
        let stranded = db::sessions::create_session(
            pool,
            "p1a-stranded",
            &project.id,
            "/tmp/p1a_route",
            "GLM-4.7",
            None,
            Some("group_chat"),
            Some(&group_meta.to_string()),
        )
        .await
        .unwrap();
        db::sessions::finalize_group_chat_lifecycle(
            pool,
            &stranded.id,
            "group_chat_end",
            Some("已收官"),
            None,
        )
        .await
        .unwrap();
        db::sessions::upsert_group_chat_checkpoint(pool, &stranded.id, 3, 0)
            .await
            .unwrap();

        // 群聊 session 丙:无断点行(interrupted 态但行已被清)。
        let rowless = db::sessions::create_session(
            pool,
            "p1a-rowless",
            &project.id,
            "/tmp/p1a_route",
            "GLM-4.7",
            None,
            Some("group_chat"),
            Some(&group_meta.to_string()),
        )
        .await
        .unwrap();
        db::sessions::finalize_group_chat_lifecycle(pool, &rowless.id, "interrupted", None, None)
            .await
            .unwrap();

        // 群聊 session 丁:round 已到上限(预算耗尽)。
        let exhausted = db::sessions::create_session(
            pool,
            "p1a-exhausted",
            &project.id,
            "/tmp/p1a_route",
            "GLM-4.7",
            None,
            Some("group_chat"),
            Some(&group_meta.to_string()),
        )
        .await
        .unwrap();
        db::sessions::finalize_group_chat_lifecycle(pool, &exhausted.id, "interrupted", None, None)
            .await
            .unwrap();
        db::sessions::upsert_group_chat_checkpoint(
            pool,
            &exhausted.id,
            crate::agent::group_chat_loop::MAX_ORCHESTRATION_ROUNDS as i64,
            0,
        )
        .await
        .unwrap();

        // 群聊 session 戊:可续跑但 busy(认领在途)。
        let busy = db::sessions::create_session(
            pool,
            "p1a-busy",
            &project.id,
            "/tmp/p1a_route",
            "GLM-4.7",
            None,
            Some("group_chat"),
            Some(&group_meta.to_string()),
        )
        .await
        .unwrap();
        db::sessions::upsert_group_chat_checkpoint(pool, &busy.id, 2, 0)
            .await
            .unwrap();
        state
            .session_active_request
            .lock()
            .await
            .insert(busy.id.clone(), "rid-live".to_string());

        async fn post_resume(state: &Arc<AppState>, sid: &str) -> (StatusCode, serde_json::Value) {
            let app = router(state.clone());
            let resp = app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/resume_group_chat")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            serde_json::json!({ "session_id": sid }).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = resp.status();
            let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
            )
        }

        // ① 非群聊。
        let (code, body) = post_resume(&state, &classic.id).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "non-group-chat: {body}");
        assert!(body.to_string().contains("不是群聊"));

        // ⑤ 终局残留(评审 P1-1 场景)。
        let (code, body) = post_resume(&state, &stranded.id).await;
        assert_eq!(
            code,
            StatusCode::BAD_REQUEST,
            "terminal stranded row: {body}"
        );
        assert!(body.to_string().contains("已正常结束"));

        // ② busy。
        let (code, body) = post_resume(&state, &busy.id).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "busy: {body}");
        assert!(body.to_string().contains("进行中"));

        // ③ 无断点行。
        let (code, body) = post_resume(&state, &rowless.id).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "rowless: {body}");
        assert!(body.to_string().contains("无可续跑"));

        // ④ 预算耗尽。
        let (code, body) = post_resume(&state, &exhausted.id).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "exhausted: {body}");
        assert!(body.to_string().contains("轮预算"));

        // happy path:interrupted + 行在 + round<MAX → Started。
        let (code, body) = post_resume(&state, &resumed.id).await;
        assert_eq!(code, StatusCode::OK, "happy: {body}");
        assert_eq!(body["status"], serde_json::json!("started"));
        // 等待后台编排(空 catalog → moderator 解析失败 → 轮空转至
        // max_rounds 收官)退出后再结束,避免测试 DB 争用。
        for _ in 0..200 {
            if !state
                .session_active_request
                .lock()
                .await
                .contains_key(&resumed.id)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }
}
