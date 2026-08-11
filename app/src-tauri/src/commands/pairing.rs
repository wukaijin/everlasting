//! 配对码生成 IPC(design §2.5 / implement.md Step 6)。
//!
//! PC 前端展示配对码给用户念给手机。生成**动作**由 PC 触发,但**落库**
//! 在 remote(S1 契约:`/internal/pairing/generate` 是 WSS 内部 RPC,不走
//! 手机 HTTP 路由;remote `handle_internal_rpc` 生成 6 位码绑定请求方
//! node_id,`{code, expires_in: 60}` 以 `Frame::Response` 回 PC)。
//!
//! 本命令经 [`TunnelManager::send_rpc_and_wait`] 走当前 WSS 连接发
//! `Frame::Request` → 等 `Frame::Response` → parse body。tunnel 离线时
//! 返回明确错误 "remote 未连接"(design §6.2 失败模式表)。

use std::sync::Arc;

use everlasting_remote_protocol::Frame;
use serde::Serialize;
use tauri::State;

use crate::error::{AppCommandError, ErrorCategory};
use crate::state::AppState;

/// 配对码生成成功响应(design §2.5 / S1 契约:body `{code, expires_in}`)。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingCodePayload {
    /// 6 位数字码。
    pub code: String,
    /// 有效秒数(remote 契约:60)。
    pub expires_in: u64,
}

/// 生成配对码:经 tunnel 调 remote 的 `/internal/pairing/generate`。
pub async fn generate_pairing_code_inner(
    state: &Arc<AppState>,
) -> Result<PairingCodePayload, AppCommandError> {
    let frame = state
        .tunnel_manager
        .send_rpc_and_wait("POST", "/internal/pairing/generate", Vec::new())
        .await
        .map_err(|msg| {
            // design §6.1:配对失败记 WARN(常见原因 = remote 未连接)。
            tracing::warn!(
                target: crate::daemon::tunnel::TUNNEL_TARGET,
                reason = %msg,
                "tunnel_pairing_failed"
            );
            AppCommandError::new(ErrorCategory::Network, msg)
        })?;
    let Frame::Response { status, body, .. } = frame else {
        return Err(AppCommandError::new(
            ErrorCategory::Server,
            "remote 返回了非预期帧类型(应为 Response)",
        ));
    };
    if status != 200 {
        let msg = String::from_utf8_lossy(&body);
        return Err(AppCommandError::new(
            ErrorCategory::Server,
            format!("remote 生成配对码失败(HTTP {status}): {msg}"),
        ));
    }
    let json: serde_json::Value = serde_json::from_slice(&body).map_err(|e| {
        AppCommandError::new(ErrorCategory::Server, format!("配对码响应解析失败: {e}"))
    })?;
    let code = json
        .get("code")
        .and_then(|c| c.as_str())
        .ok_or_else(|| AppCommandError::new(ErrorCategory::Server, "配对码响应缺少 code 字段"))?
        .to_string();
    let expires_in = json
        .get("expires_in")
        .and_then(|e| e.as_u64())
        .ok_or_else(|| {
            AppCommandError::new(ErrorCategory::Server, "配对码响应缺少 expires_in 字段")
        })?;
    tracing::info!(
        target: crate::daemon::tunnel::TUNNEL_TARGET,
        expires_in,
        "pairing code generated via remote RPC"
    );
    Ok(PairingCodePayload { code, expires_in })
}

#[tauri::command]
pub async fn generate_pairing_code(
    state: State<'_, Arc<AppState>>,
) -> Result<PairingCodePayload, AppCommandError> {
    generate_pairing_code_inner(&state).await
}
