//! F1 消息队列(2026-08-25)— R8 用户侧 IPC 三件 + 视图水合。
//!
//! 全部为用户发起的会话内队列管理操作,**不涉 agent 工具面**,不走
//! ⑨ 权限决策层(同 `list_sessions` 类只读/会话内写)。业务逻辑
//! 薄封装 [`crate::agent::message_queue`],`_inner` 形态供 daemon
//! 路由复用(P2.2 Q0 惯例)。

use crate::agent::message_queue::{self, QueueError, QueuedMessage};
use crate::error::AppCommandError;
use crate::state::AppState;
use std::sync::Arc;
use tauri::State;

/// `list_queued_messages(session_id)` — 排队视图水合的 SoT 读
/// (design §7;切 session / 页面刷新 / PWA 第二端均从后端重建)。
pub async fn list_queued_messages_inner(
    state: &Arc<AppState>,
    session_id: String,
) -> Result<Vec<QueuedMessage>, AppCommandError> {
    Ok(message_queue::list_session(&state.message_queues, &session_id).await)
}

#[tauri::command]
pub async fn list_queued_messages(
    state: State<'_, Arc<AppState>>,
    #[allow(non_snake_case)] sessionId: String,
) -> Result<Vec<QueuedMessage>, AppCommandError> {
    list_queued_messages_inner(state.inner(), sessionId).await
}

/// `remove_queued_message(session_id, id)` — R8 撤销(删除单条)。
/// not-found → Err(drain 窗口竞态:注入已开始,前端 toast「已开始处理」)。
pub async fn remove_queued_message_inner(
    state: &Arc<AppState>,
    session_id: String,
    id: String,
) -> Result<(), AppCommandError> {
    message_queue::remove_by_id(&state.message_queues, &session_id, &id)
        .await
        .map_err(|e| match e {
            QueueError::NotFound => {
                AppCommandError::new(crate::error::ErrorCategory::InvalidRequest, e.to_string())
            }
            QueueError::Full => unreachable!("remove never hits capacity"),
        })?;
    tracing::info!(session_id = %session_id, queued_id = %id, "queued message revoked");
    Ok(())
}

#[tauri::command]
pub async fn remove_queued_message(
    state: State<'_, Arc<AppState>>,
    #[allow(non_snake_case)] sessionId: String,
    id: String,
) -> Result<(), AppCommandError> {
    remove_queued_message_inner(state.inner(), sessionId, id).await
}

/// `recall_queued_message(session_id, id)` — R8 修改 = 单条退回输入框:
/// 从队列移除并返回原文(含 attachments 引用),由前端回填 composer。
pub async fn recall_queued_message_inner(
    state: &Arc<AppState>,
    session_id: String,
    id: String,
) -> Result<QueuedMessage, AppCommandError> {
    let msg = message_queue::remove_by_id(&state.message_queues, &session_id, &id)
        .await
        .map_err(|e| match e {
            QueueError::NotFound => {
                AppCommandError::new(crate::error::ErrorCategory::InvalidRequest, e.to_string())
            }
            QueueError::Full => unreachable!("recall never hits capacity"),
        })?;
    tracing::info!(session_id = %session_id, queued_id = %id, "queued message recalled to composer");
    Ok(msg)
}

#[tauri::command]
pub async fn recall_queued_message(
    state: State<'_, Arc<AppState>>,
    #[allow(non_snake_case)] sessionId: String,
    id: String,
) -> Result<QueuedMessage, AppCommandError> {
    recall_queued_message_inner(state.inner(), sessionId, id).await
}
