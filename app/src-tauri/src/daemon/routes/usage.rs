//! `/api/v1/usage` — 5h 滚动窗口配额路由(08-20-turn-usage-event-quota-view WP2)。
//! 两路由均 POST(镜像 `permissions` 组;httpTransport.invoke 对
//! CMD_TO_DOMAIN 是 POST 硬编码,D2 P1 先例)。
//!
//! 请求结构体 **snake_case 无 rename**:httpTransport 的
//! `transformArgsTopLevel` 把顶层参数 camel→snake 后作为 body(见
//! `transport/http.ts`);camelCase rename 会让字段整体 miss(质检
//! Step 5 修正)。

use std::sync::Arc;

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;

use crate::commands::usage::{set_quota_settings_inner, usage_window_inner};
use crate::error::AppCommandError;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct UsageWindowRequest {
    /// `None` = 全部 provider(缺省视图)。
    pub provider_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SetQuotaSettingsRequest {
    pub window_hours: Option<i64>,
    pub limit_tokens: Option<i64>,
}

pub async fn usage_window(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UsageWindowRequest>,
) -> Result<Json<crate::db::usage::UsageWindowReport>, AppCommandError> {
    let result = usage_window_inner(&state, req.provider_id).await?;
    Ok(Json(result))
}

pub async fn set_quota_settings(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetQuotaSettingsRequest>,
) -> Result<Json<()>, AppCommandError> {
    set_quota_settings_inner(&state, req.window_hours, req.limit_tokens).await?;
    Ok(Json(()))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/usage_window", post(usage_window))
        .route("/set_quota_settings", post(set_quota_settings))
        .with_state(state)
}
