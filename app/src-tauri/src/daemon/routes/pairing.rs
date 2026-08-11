//! `POST /api/v1/pairing/<command>` handlers for the pairing domain
//! (S2, 2026-08-11, task `08-11-tunnel-client`,design §3.1 清单第 5 条)。
//!
//! 只有一个命令 `generate_pairing_code`:PC 前端要配对码 → 经 tunnel WSS
//! 调 remote 的 `/internal/pairing/generate` 内部 RPC(S1 契约) →
//! `{code, expires_in: 60}`。tunnel 离线时返明确错误(design §6.2)。
//!
//! 注:此 domain 与 remote(云服务器)的 `/api/v1/pairing/redeem` **不同**:
//! redeem 是手机在 remote 上兑换码,本 domain 是 PC daemon 的本地 IPC 镜像。

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};

use crate::commands::pairing::{generate_pairing_code_inner, PairingCodePayload};
use crate::error::AppCommandError;
use crate::state::AppState;

pub async fn generate_pairing_code(
    State(state): State<Arc<AppState>>,
) -> Result<Json<PairingCodePayload>, AppCommandError> {
    let result = generate_pairing_code_inner(&state).await?;
    Ok(Json(result))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/generate_pairing_code", post(generate_pairing_code))
        .with_state(state)
}
