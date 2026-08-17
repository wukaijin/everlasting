//! `POST /api/v1/config/<command>` handlers for the config domain.
//!
//! Phase 2.2 B5 skeleton. Each handler deserializes a JSON body
//! into the same args the Tauri command takes, forwards to
//! `crate::commands::config::xxx_inner` (Q0 decision — single
//! source of truth), and wraps the result in `Json(...)`. Errors
//! flow through `AppCommandError`'s `IntoResponse` impl.

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;

use crate::commands::config::{
    get_home_dir_inner, get_llm_config_inner, get_remote_config_inner, get_tunnel_status_inner,
    set_remote_config_inner, PublicLlmConfig, RemoteConfigPayload, TunnelStatusPayload,
};
use crate::error::AppCommandError;
use crate::state::AppState;

pub async fn get_llm_config(
    State(state): State<Arc<AppState>>,
) -> Result<Json<PublicLlmConfig>, AppCommandError> {
    let result = get_llm_config_inner(&state).await?;
    Ok(Json(result))
}

/// `POST /api/v1/config/get_home_dir` — cwd-chip `~` shortening
/// source. Pre-existing gap (08-17 hotfix): the command + Tauri
/// registration + CMD_TO_DOMAIN row all existed, but this daemon
/// route didn't — every browser-mode boot logged
/// `failed to load home dir: 405` and the chat header fell back to
/// full paths. No state dependency (`dirs::home_dir`).
pub async fn get_home_dir() -> Result<Json<Option<String>>, AppCommandError> {
    Ok(Json(get_home_dir_inner()))
}

// ---- S2 remote tunnel 配置(design §3.1,请求体 snake_case —— http.ts 的
// `transformArgsTopLevel` 把前端 camelCase 顶层参数扳回 snake_case)----

/// `set_remote_config` 请求体(与 Tauri command 参数一一对应)。
#[derive(Deserialize)]
pub struct SetRemoteConfigRequest {
    pub remote_url: String,
    pub shared_secret: String,
}

pub async fn get_remote_config(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Option<RemoteConfigPayload>>, AppCommandError> {
    let result = get_remote_config_inner(&state).await?;
    Ok(Json(result))
}

pub async fn set_remote_config(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SetRemoteConfigRequest>,
) -> Result<Json<()>, AppCommandError> {
    set_remote_config_inner(&state, body.remote_url, body.shared_secret).await?;
    Ok(Json(()))
}

pub async fn get_tunnel_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Option<TunnelStatusPayload>>, AppCommandError> {
    let result = get_tunnel_status_inner(&state).await?;
    Ok(Json(result))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/get_llm_config", post(get_llm_config))
        .route("/get_home_dir", post(get_home_dir))
        .route("/get_remote_config", post(get_remote_config))
        .route("/set_remote_config", post(set_remote_config))
        .route("/get_tunnel_status", post(get_tunnel_status))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt; // oneshot

    /// 08-17 hotfix route smoke: `get_home_dir` previously existed as
    /// a Tauri command + CMD_TO_DOMAIN row but had NO daemon route —
    /// every browser-mode boot logged a 405 and the cwd chip fell
    /// back to full paths. Locks the wiring (spec: new IPC commands
    /// get a Router oneshot test).
    #[tokio::test(flavor = "multi_thread")]
    async fn get_home_dir_route_serves_path() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/get_home_dir")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let home: Option<String> = serde_json::from_slice(&body).unwrap();
        assert!(home.is_some(), "CI boxes always have $HOME: {body:?}");
    }
}
