//! `POST /api/v1/evl_cli/<command>` handlers(Settings「CLI (evl)」分类)。
//!
//! Same shape as `routes::disk`: each handler forwards to the
//! `crate::commands::evl_cli::xxx_inner` single source (Q0), wrapping the
//! result in `Json(...)`. Errors flow through `AppCommandError`'s
//! `IntoResponse` impl. 两条命令都**无请求体参数**(检测与安装的落点均
//! 由 daemon 自身解析:app_data_dir + home)。

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};

use crate::commands::evl_cli::{detect_evl_inner, install_evl_inner, EvlCliStatusPayload};
use crate::error::AppCommandError;
use crate::state::AppState;

/// 检测宿主机 Node / evl 状态(安装形态 / 版本 / PATH)。
pub async fn detect_evl(
    State(state): State<Arc<AppState>>,
) -> Result<Json<EvlCliStatusPayload>, AppCommandError> {
    let result = detect_evl_inner(&state).await?;
    Ok(Json(result))
}

/// 安装 / 更新 daemon 内嵌的 evl CLI(写出 `{app_data_dir}/cli/` +
/// symlink `~/.local/bin/evl`),返回安装后的检测 payload。
pub async fn install_evl(
    State(state): State<Arc<AppState>>,
) -> Result<Json<EvlCliStatusPayload>, AppCommandError> {
    let result = install_evl_inner(&state).await?;
    Ok(Json(result))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/detect_evl", post(detect_evl))
        .route("/install_evl", post(install_evl))
        .with_state(state)
}
