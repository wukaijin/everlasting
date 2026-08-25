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
    get_web_search_config_inner, set_remote_config_inner, set_web_search_config_inner,
    PublicLlmConfig, RemoteConfigPayload, TunnelStatusPayload,
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

// ---- F4 web_search 配置(POST,照 config router 全 POST 先例)----

/// `set_web_search_config` 请求体(snake_case,与 Tauri command 扁平
/// 标量参数一一对应;http.ts `transformArgsTopLevel` 已把 camelCase
/// 顶层 key 扳回 snake)。
#[derive(Deserialize)]
pub struct SetWebSearchConfigRequest {
    pub provider: String,
    pub tavily_api_key: Option<String>,
}

pub async fn get_web_search_config(
    State(state): State<Arc<AppState>>,
) -> Result<Json<crate::tools::web_search::WebSearchConfigPayload>, AppCommandError> {
    let result = get_web_search_config_inner(&state).await?;
    Ok(Json(result))
}

pub async fn set_web_search_config(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SetWebSearchConfigRequest>,
) -> Result<Json<()>, AppCommandError> {
    set_web_search_config_inner(&state, body.provider, body.tavily_api_key).await?;
    Ok(Json(()))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/get_llm_config", post(get_llm_config))
        .route("/get_home_dir", post(get_home_dir))
        .route("/get_remote_config", post(get_remote_config))
        .route("/set_remote_config", post(set_remote_config))
        .route("/get_tunnel_status", post(get_tunnel_status))
        .route("/get_web_search_config", post(get_web_search_config))
        .route("/set_web_search_config", post(set_web_search_config))
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

    /// F4 web_search config route 全链(AC4 后端半):get 默认 auto →
    /// set key(加密落盘)→ get masked 不回明文 → Some("") 清除 → key
    /// 行删除。请求体 snake_case(顶层 camel 已被 http.ts 扳正)。
    #[tokio::test(flavor = "multi_thread")]
    async fn web_search_config_roundtrip_set_mask_clear() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);
        let app = router(state.clone());

        async fn post_json(
            app: &axum::Router,
            uri: &str,
            body: &str,
        ) -> (StatusCode, serde_json::Value) {
            use tower::ServiceExt;
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(uri)
                        .header("content-type", "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = resp.status();
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
            (status, json)
        }

        // 默认态:provider=auto、无 key
        let (code, v) = post_json(&app, "/get_web_search_config", "{}").await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(v["provider"], "auto");
        assert_eq!(v["tavilyKeySet"], false);
        assert_eq!(v["tavilyKeyMasked"], serde_json::Value::Null);

        // 存 key(明文不回显,masked 形态)
        let (code, v) = post_json(
            &app,
            "/set_web_search_config",
            r#"{"provider":"tavily","tavily_api_key":"tvly-abcdEFGH1234"}"#,
        )
        .await;
        assert_eq!(code, StatusCode::OK, "{v}");
        let (code, v) = post_json(&app, "/get_web_search_config", "{}").await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(v["provider"], "tavily");
        assert_eq!(v["tavilyKeySet"], true);
        assert_eq!(v["tavilyKeyMasked"], "tvly-****1234");

        // 非法 provider 拒绝(4xx,不写库)
        let (code, _) = post_json(&app, "/set_web_search_config", r#"{"provider":"brave"}"#).await;
        assert_ne!(code, StatusCode::OK);

        // Some(""):显式清除 → 行删除、key_set 回 false
        let (code, v) = post_json(
            &app,
            "/set_web_search_config",
            r#"{"provider":"ddg","tavily_api_key":""}"#,
        )
        .await;
        assert_eq!(code, StatusCode::OK, "{v}");
        let (code, v) = post_json(&app, "/get_web_search_config", "{}").await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(v["provider"], "ddg");
        assert_eq!(v["tavilyKeySet"], false);
        assert_eq!(v["tavilyKeyMasked"], serde_json::Value::Null);
    }
}
