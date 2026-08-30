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
    get_web_search_config_inner, set_remote_config_inner, set_tunnel_display_name_inner,
    set_tunnel_node_id_inner, set_web_search_config_inner, PublicLlmConfig, RemoteConfigPayload,
    TunnelStatusPayload,
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

/// `set_tunnel_node_id` 请求体(snake_case,与 Tauri command 扁平标量参数
/// 一一对应;`node_id` 三态:Some(非空) 设置 / Some("") 清除 / 缺省不动)。
#[derive(Deserialize)]
pub struct SetTunnelNodeIdRequest {
    #[serde(default)]
    pub node_id: Option<String>,
}

pub async fn set_tunnel_node_id(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SetTunnelNodeIdRequest>,
) -> Result<Json<()>, AppCommandError> {
    set_tunnel_node_id_inner(&state, body.node_id).await?;
    Ok(Json(()))
}

/// `set_tunnel_display_name` 请求体(snake_case,三态同 node_id:
/// Some(非空) 设置 / Some("") 清除 / 缺省不动)。
#[derive(Deserialize)]
pub struct SetTunnelDisplayNameRequest {
    #[serde(default)]
    pub display_name: Option<String>,
}

pub async fn set_tunnel_display_name(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SetTunnelDisplayNameRequest>,
) -> Result<Json<()>, AppCommandError> {
    set_tunnel_display_name_inner(&state, body.display_name).await?;
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
        .route("/set_tunnel_node_id", post(set_tunnel_node_id))
        .route("/set_tunnel_display_name", post(set_tunnel_display_name))
        .route("/get_tunnel_status", post(get_tunnel_status))
        .route("/get_web_search_config", post(get_web_search_config))
        .route("/set_web_search_config", post(set_web_search_config))
        .route("/get_app_config", post(get_app_config))
        .route("/set_app_config_flag", post(set_app_config_flag))
        .route("/set_app_config_list", post(set_app_config_list))
        .with_state(state)
}

/// F6(2026-08-27):前端可读 app_config 开关面。无请求体(同
/// `get_web_search_config`)。
pub async fn get_app_config(
    State(state): State<Arc<AppState>>,
) -> Result<Json<crate::commands::config::AppConfigPayload>, AppCommandError> {
    let result = crate::commands::config::get_app_config_inner(&state).await?;
    Ok(Json(result))
}

// ---- Settings「通用」开关写入口(2026-08-29, settings-shell 重构)----

/// `set_app_config_flag` 请求体(snake_case,与 Tauri command 扁平标量
/// 参数一一对应;白名单校验在 `_inner`)。
#[derive(Deserialize)]
pub struct SetAppConfigFlagRequest {
    pub key: String,
    pub value: bool,
}

pub async fn set_app_config_flag(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SetAppConfigFlagRequest>,
) -> Result<Json<()>, AppCommandError> {
    crate::commands::config::set_app_config_flag_inner(&state, body.key, body.value).await?;
    Ok(Json(()))
}

// ---- P3b(2026-08-31,评审 W1):列表型 app_config 字段写通道 ----

/// `set_app_config_list` 请求体(snake_case,扁平顶层字段;白名单
/// 校验在 `_inner`)。
#[derive(Deserialize)]
pub struct SetAppConfigListRequest {
    pub key: String,
    pub value: Vec<String>,
}

pub async fn set_app_config_list(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SetAppConfigListRequest>,
) -> Result<Json<()>, AppCommandError> {
    crate::commands::config::set_app_config_list_inner(&state, body.key, body.value).await?;
    Ok(Json(()))
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

    /// 自定义 node_id(08-26-custom-node-id)全链:set 合法值 → key 落库 +
    /// `build_tunnel_config` 换新 id;非法值(大写/下划线/连续连字符/首尾
    /// 连字符/纯中文/纯空格)→ 4xx 且库中无写入;Some("") → key 行删除
    /// (回自动派生);get_remote_config 回显 `nodeId`。
    #[tokio::test(flavor = "multi_thread")]
    async fn tunnel_node_id_set_validate_clear_roundtrip() {
        use crate::daemon::tunnel::config::{KEY_REMOTE_URL, KEY_TUNNEL_NODE_ID};

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

        // 非法值全部 4xx,且 key 不写库(校验失败不落库)
        for bad in [
            "Carlos-Office",  // 大写
            "carlos_office",  // 下划线
            "carlos--office", // 连续连字符
            "-carlos",        // 首连字符
            "carlos-",        // 尾连字符
            "卡洛斯",         // 纯中文
            "   ",            // 纯空格(trim 后空 → 不能当合法值写库)
        ] {
            let (code, body) = post_json(
                &app,
                "/set_tunnel_node_id",
                &format!(r#"{{"node_id":"{bad}"}}"#),
            )
            .await;
            assert_ne!(code, StatusCode::OK, "非法值 {bad:?} 必须拒绝: {body}");
            let stored = crate::db::get_config_value(&state.db, KEY_TUNNEL_NODE_ID)
                .await
                .unwrap();
            assert_eq!(stored, None, "拒绝 {bad:?} 后不得写库");
        }

        // 合法值:落库(去首尾空白)
        let (code, v) = post_json(
            &app,
            "/set_tunnel_node_id",
            r#"{"node_id":" carlos-office "}"#,
        )
        .await;
        assert_eq!(code, StatusCode::OK, "{v}");
        assert_eq!(
            crate::db::get_config_value(&state.db, KEY_TUNNEL_NODE_ID)
                .await
                .unwrap()
                .as_deref(),
            Some("carlos-office")
        );

        // 配好 remote 后再改 id → build_tunnel_config 用新 id 重建配置
        crate::db::set_config_value(&state.db, KEY_REMOTE_URL, "wss://remote.example.com")
            .await
            .unwrap();
        let (code, v) =
            post_json(&app, "/set_tunnel_node_id", r#"{"node_id":"carlos-home"}"#).await;
        assert_eq!(code, StatusCode::OK, "{v}");
        let cfg = state.tunnel_manager.current_config();
        assert_eq!(
            cfg.as_ref().map(|c| c.node_id.as_str()),
            Some("carlos-home"),
            "set 后 tunnel 配置必须带新 node_id"
        );

        // get_remote_config 回显自定义 nodeId
        let (code, v) = post_json(&app, "/get_remote_config", "{}").await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(v["nodeId"], "carlos-home");

        // Some(""):显式清除 → key 行删除、回显回 null、配置回 hostname 派生
        let (code, v) = post_json(&app, "/set_tunnel_node_id", r#"{"node_id":""}"#).await;
        assert_eq!(code, StatusCode::OK, "{v}");
        assert_eq!(
            crate::db::get_config_value(&state.db, KEY_TUNNEL_NODE_ID)
                .await
                .unwrap(),
            None
        );
        let cfg = state.tunnel_manager.current_config();
        assert_eq!(
            cfg.as_ref().map(|c| c.node_id.as_str()),
            Some(
                crate::daemon::tunnel::node_id::sanitize(
                    &hostname::get()
                        .unwrap()
                        .into_string()
                        .expect("test env has hostname")
                )
                .as_str()
            ),
            "清除后 node_id 回 hostname 派生"
        );
        let (code, v) = post_json(&app, "/get_remote_config", "{}").await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(v["nodeId"], serde_json::Value::Null);
    }

    /// 自定义 display_name(08-26 增补需求 6/7)全链,镜像 node_id roundtrip:
    /// 合法值(含中文,无字符集限制)落库 + `build_tunnel_config` 取到;
    /// trim 空 / 超 64 字符拒绝且不写库;Some("") → key 删行回默认
    /// hostname;get_remote_config 回显 `displayName`。
    #[tokio::test(flavor = "multi_thread")]
    async fn tunnel_display_name_set_validate_clear_roundtrip() {
        use crate::daemon::tunnel::config::{KEY_REMOTE_URL, KEY_TUNNEL_DISPLAY_NAME};

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

        // 非法值全部 4xx,且 key 不写库(空白串 / 超 64 字符——长度按字符数
        // 计,65 个中文不能因 UTF-8 字节数被误判,65 个 ASCII 同样拒绝)
        for bad in ["   ", &"名".repeat(65), &"a".repeat(65)] {
            let (code, body) = post_json(
                &app,
                "/set_tunnel_display_name",
                &serde_json::json!({ "display_name": bad }).to_string(),
            )
            .await;
            assert_ne!(
                code,
                StatusCode::OK,
                "非法值({} 字符)必须拒绝: {body}",
                bad.chars().count()
            );
            let stored = crate::db::get_config_value(&state.db, KEY_TUNNEL_DISPLAY_NAME)
                .await
                .unwrap();
            assert_eq!(stored, None, "拒绝后不得写库");
        }

        // 合法值:含中文、去首尾空白、64 字符边界(恰好 64 个中文)通过
        let (code, v) = post_json(
            &app,
            "/set_tunnel_display_name",
            r#"{"display_name":" 卡洛斯的办公机 "}"#,
        )
        .await;
        assert_eq!(code, StatusCode::OK, "{v}");
        assert_eq!(
            crate::db::get_config_value(&state.db, KEY_TUNNEL_DISPLAY_NAME)
                .await
                .unwrap()
                .as_deref(),
            Some("卡洛斯的办公机")
        );
        let (code, v) = post_json(
            &app,
            "/set_tunnel_display_name",
            &serde_json::json!({ "display_name": "名".repeat(64) }).to_string(),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "64 字符边界必须通过: {v}");

        // 配好 remote 后 set → build_tunnel_config 用新 display_name 重建配置
        crate::db::set_config_value(&state.db, KEY_REMOTE_URL, "wss://remote.example.com")
            .await
            .unwrap();
        let (code, v) = post_json(
            &app,
            "/set_tunnel_display_name",
            r#"{"display_name":"公司台式机"}"#,
        )
        .await;
        assert_eq!(code, StatusCode::OK, "{v}");
        let cfg = state.tunnel_manager.current_config();
        assert_eq!(
            cfg.as_ref().map(|c| c.display_name.as_str()),
            Some("公司台式机"),
            "set 后 tunnel 配置必须带新 display_name"
        );

        // get_remote_config 回显自定义 displayName
        let (code, v) = post_json(&app, "/get_remote_config", "{}").await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(v["displayName"], "公司台式机");

        // Some(""):显式清除 → key 行删除、回显回 null、配置回默认 hostname
        // (default_display_name = hostname 原文,不净化)
        let (code, v) = post_json(&app, "/set_tunnel_display_name", r#"{"display_name":""}"#).await;
        assert_eq!(code, StatusCode::OK, "{v}");
        assert_eq!(
            crate::db::get_config_value(&state.db, KEY_TUNNEL_DISPLAY_NAME)
                .await
                .unwrap(),
            None
        );
        let cfg = state.tunnel_manager.current_config();
        let host = hostname::get()
            .unwrap()
            .into_string()
            .expect("test env has hostname");
        assert_eq!(
            cfg.as_ref().map(|c| c.display_name.as_str()),
            Some(host.as_str()),
            "清除后 display_name 回默认 hostname"
        );
        let (code, v) = post_json(&app, "/get_remote_config", "{}").await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(v["displayName"], serde_json::Value::Null);
    }

    /// Settings「通用」开关(2026-08-29 settings-shell)route 全链:
    /// set false → get_app_config 读回 false;set true 回 true;
    /// 白名单外 key → 4xx 且不落库。
    #[tokio::test(flavor = "multi_thread")]
    async fn set_app_config_flag_route_set_reject_roundtrip() {
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

        for (key, field) in [
            ("turn_complete_notify_enabled", "turnCompleteNotifyEnabled"),
            ("scheduled_tasks_enabled", "scheduledTasksEnabled"),
        ] {
            let (code, v) = post_json(
                &app,
                "/set_app_config_flag",
                &serde_json::json!({ "key": key, "value": false }).to_string(),
            )
            .await;
            assert_eq!(code, StatusCode::OK, "{key}: {v}");
            let (_, v) = post_json(&app, "/get_app_config", "{}").await;
            assert_eq!(
                v[field], false,
                "{key} 写 false 后 get_app_config 应读回 false"
            );

            let (code, v) = post_json(
                &app,
                "/set_app_config_flag",
                &serde_json::json!({ "key": key, "value": true }).to_string(),
            )
            .await;
            assert_eq!(code, StatusCode::OK, "{key}: {v}");
            let (_, v) = post_json(&app, "/get_app_config", "{}").await;
            assert_eq!(v[field], true);
        }

        // 白名单外 key → 4xx(InvalidRequest),不落库
        let (code, body) = post_json(
            &app,
            "/set_app_config_flag",
            r#"{"key":"remote_url","value":true}"#,
        )
        .await;
        assert_ne!(code, StatusCode::OK, "白名单外 key 必须拒绝: {body}");
        let stored = crate::db::get_config_value(&state.db, "remote_url")
            .await
            .unwrap();
        assert_eq!(stored, None, "拒绝的 key 不得写库");
    }

    /// P3b(2026-08-31,评审 W1):`set_app_config_list` 路由 roundtrip
    /// + 白名单拒绝。08-17 hotfix 先例:新 IPC 命令必须有一条 Router
    /// oneshot 测试锁 wiring(daemon + Tauri 双端)。
    #[tokio::test(flavor = "multi_thread")]
    async fn set_app_config_list_route_roundtrip_and_reject() {
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

        // 白名单内 key:写数组 → get_app_config 读回(生效清单含 ~/.cargo)。
        let (code, _) = post_json(
            &app,
            "/set_app_config_list",
            r#"{"key":"sandbox_extra_writable","value":["/opt/cache","/data"]}"#,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let (_, v) = post_json(&app, "/get_app_config", "{}").await;
        let list = v["sandboxExtraWritable"].as_array().expect("list field");
        let strs: Vec<&str> = list.iter().map(|x| x.as_str().unwrap()).collect();
        assert!(
            strs.contains(&"/opt/cache") && strs.contains(&"/data"),
            "{strs:?}"
        );
        assert!(
            strs.iter().any(|p| p.ends_with(".cargo")),
            "默认项并入: {strs:?}"
        );

        // 白名单外 key → 4xx,不落库。
        let (code, body) = post_json(
            &app,
            "/set_app_config_list",
            r#"{"key":"remote_url","value":["http://evil"]}"#,
        )
        .await;
        assert_ne!(code, StatusCode::OK, "白名单外 list key 必须拒绝: {body}");
        let stored = crate::db::get_config_value(&state.db, "remote_url")
            .await
            .unwrap();
        assert_eq!(stored, None, "拒绝的 list key 不得写库");
    }
}
