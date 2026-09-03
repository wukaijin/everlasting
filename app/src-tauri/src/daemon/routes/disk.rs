//! `POST /api/v1/disk/<command>` handlers for the disk domain
//! (F3 磁盘治理 PR3,2026-09-03, task `09-03-f3-disk-governance`)。
//!
//! Same shape as `routes::background_shells`: each handler forwards to
//! the `crate::commands::disk::xxx_inner` single source (Q0), wrapping
//! the result in `Json(...)`. Errors flow through `AppCommandError`'s
//! `IntoResponse` impl. 两条命令都**无请求体参数**(同
//! `get_app_config` 先例 —— httpTransport 恒发 `{}` body,handler 不
//! 取 Json extractor 即忽略之)。

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};

use crate::commands::disk::{get_disk_usage_inner, run_disk_cleanup_inner, DiskUsageReport};
use crate::disk::governor::DiskGovernorOutcome;
use crate::error::AppCommandError;
use crate::state::AppState;

/// 占用概览:各消费点字节数 + 总量(camelCase wire)。无请求体。
pub async fn get_disk_usage(
    State(state): State<Arc<AppState>>,
) -> Result<Json<DiskUsageReport>, AppCommandError> {
    let result = get_disk_usage_inner(&state).await?;
    Ok(Json(result))
}

/// 手动「立即清理」:调 governor `_inner` 族,不查 kill-switch(AC9
/// 手动语义)。返回逐项回收摘要(camelCase wire)。无请求体。
pub async fn run_disk_cleanup(
    State(state): State<Arc<AppState>>,
) -> Result<Json<DiskGovernorOutcome>, AppCommandError> {
    let result = run_disk_cleanup_inner(&state).await?;
    Ok(Json(result))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/get_disk_usage", post(get_disk_usage))
        .route("/run_disk_cleanup", post(run_disk_cleanup))
        .with_state(state)
}
