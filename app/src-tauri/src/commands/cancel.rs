//! `cancel_chat` Tauri command (PR5 — in-flight cancel).
//!
//! The frontend's Stop button invokes this with the current
//! `request_id`. Looks up the matching `CancellationToken` and
//! calls `.cancel()` on it; the agent loop's `tokio::select!`
//! notices on the next event boundary and bails out cleanly
//! (partial turn is persisted; a `done` event with
//! `stop_reason: "cancelled"` is emitted).
//!
//! Idempotent: a missing `request_id` is a silent no-op (the user
//! may have clicked Stop after the stream already finished).
//! Re-cancelling an already-cancelled token is also a no-op.

use std::sync::Arc;

use tauri::State;

use crate::error::AppCommandError;
use crate::state::AppState;

/// Phase 2.2 `_inner` (Q0 decision): business logic, callable from
/// both the Tauri command wrapper below and the axum route handler
/// in `daemon::routes::cancel`. Takes `&Arc<AppState>` (vs Tauri's
/// `State<'_, Arc<AppState>>`) so the daemon path doesn't need the
/// Tauri-specific `State` wrapper.
/// F1 消息队列(2026-08-25):取消结果。`cleared_queued` 是随本次
/// 取消一并丢弃的排队消息条数(PRD R7 —— Stop/edit/resend/retry
/// 的「已丢弃 N 条」toast 数据源,后端 SoT)。wire 形状 camelCase。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelOutcome {
    pub cancelled: bool,
    pub cleared_queued: usize,
}

pub async fn cancel_chat_inner(
    state: &Arc<AppState>,
    request_id: String,
) -> Result<CancelOutcome, AppCommandError> {
    let token = {
        let map = state.cancellations.lock().await;
        map.get(&request_id).cloned()
    };
    let mut cancelled = false;
    if let Some(t) = token {
        t.cancel();
        cancelled = true;
        tracing::info!(request_id = %request_id, "cancel_chat: token cancelled");
    } else {
        tracing::debug!(
            request_id = %request_id,
            "cancel_chat: no active request (likely already finished)"
        );
    }
    // F1:Stop 语义 = 停当前轮 + 清空该 session 队列(PRD D3)。rid →
    // session 反查(session_active_request 是 session→rid 正向映射;
    // 单用户场景下条目数 ≤ 活跃请求数,线性扫无压力)。
    let session_id = {
        let map = state.session_active_request.lock().await;
        map.iter().find_map(|(s, r)| {
            if r == &request_id {
                Some(s.clone())
            } else {
                None
            }
        })
    };
    let cleared_queued = match &session_id {
        Some(sid) => {
            // F2 定时任务(design §4.3-5):清队**前**快照,对带 origin
            // 的条目补 `lost` 审计(best-effort)。快照与清队之间驱动器
            // 可能并发 drain —— 可接受(调度器侧该轮已落账,审计兜底
            // 可观测)。sessions.rs 的破坏性清理不在此列(会话销毁,
            // 不审计)。
            let snapshot =
                crate::agent::message_queue::list_session(&state.message_queues, sid).await;
            crate::scheduler::audit_lost_queued_entries(&state.db, sid, &snapshot).await;
            crate::agent::message_queue::clear_session(&state.message_queues, sid).await
        }
        None => 0,
    };
    Ok(CancelOutcome {
        cancelled,
        cleared_queued,
    })
}

#[tauri::command]
pub async fn cancel_chat(
    request_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<CancelOutcome, AppCommandError> {
    cancel_chat_inner(&state, request_id).await
}

/// 09-06-gc-p0-preempt-min-semantics R2:preempt a LIVE group-chat
/// discussion by session. Unlike `cancel_chat` (rid-scoped hard Stop:
/// kills the in-flight speaker, `stop_reason=cancelled`, no summary),
/// preempt is turn-boundary and graceful: the in-flight speaker
/// finishes, the orchestrator runs one moderator wrap-up turn
/// (`end_discussion` → `discussion_summary`), and the discussion ends
/// with `stop_reason="preempted"`. The 1:1 internal core of M3's
/// future `interrupt_discussion`.
///
/// Errors when the session has no live discussion (the controls
/// registry has no entry) — callers should surface that loudly (it is
/// NOT the idempotent-silent face `cancel_chat` has: preempt is an
/// explicit user/API action against a specific discussion).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreemptOutcome {
    pub preempted: bool,
}

pub async fn preempt_group_chat_inner(
    state: &Arc<AppState>,
    session_id: String,
) -> Result<PreemptOutcome, AppCommandError> {
    // 锁纪律:controls 最后获取(见 state.rs 字段注释);本函数不触
    // 其他 AppState 锁,无序约束实际生效,但保持纪律以防调用方组合。
    let control = {
        let map = state.group_chat_controls.lock().await;
        map.get(&session_id).cloned()
    };
    match control {
        Some(c) => {
            c.lock().await.preempt_requested = true;
            tracing::info!(session_id = %session_id, "preempt_group_chat: requested");
            Ok(PreemptOutcome { preempted: true })
        }
        None => Err(anyhow::anyhow!("该会话当前没有进行中的群聊讨论").into()),
    }
}

#[tauri::command]
pub async fn preempt_group_chat(
    session_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<PreemptOutcome, AppCommandError> {
    preempt_group_chat_inner(&state, session_id).await
}
