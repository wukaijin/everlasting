//! `POST /api/v1/group_chat_presets/<command>` handlers for the
//! group_chat_presets domain.
//!
//! GCE-P1(2026-09-12, task `09-12-gc-preset-settings`):用户群聊预设
//! CRUD 四条 route(mirror `commands::group_chat_presets`,Q0 单源)。
//! Each handler deserializes a JSON body into the same args the Tauri
//! command takes, forwards to `crate::commands::group_chat_presets::
//! xxx_inner`, and wraps the result in `Json(...)`. Errors flow through
//! `AppCommandError`'s `IntoResponse` impl(校验失败 → 400)。

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;

use crate::commands::group_chat_presets::{
    create_group_chat_preset_inner, delete_group_chat_preset_inner, list_group_chat_presets_inner,
    update_group_chat_preset_inner, DeletedGroupChatPreset,
};
use crate::db::group_chat_presets::{GcPresetParticipant, GcPresetRow};
use crate::error::AppCommandError;
use crate::state::AppState;

/// 全量列表(无参数;前端 invoke 恒带 `{}` body,本 handler 不消费)。
pub async fn list_group_chat_presets(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<GcPresetRow>>, AppCommandError> {
    let result = list_group_chat_presets_inner(&state).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct CreateGroupChatPresetRequest {
    pub name: String,
    pub description: String,
    pub moderator_model_id: String,
    pub participants: Vec<GcPresetParticipant>,
    /// GCE-P1b(2026-09-12, task `09-12-gc-preset-override`):可选;
    /// Some(∈ 内置四 key)= 内置档覆盖行。serde default = 旧请求体
    /// 缺键仍反序列化(additive,None = 普通用户行)。
    #[serde(default)]
    pub builtin_key: Option<String>,
}

pub async fn create_group_chat_preset(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateGroupChatPresetRequest>,
) -> Result<Json<GcPresetRow>, AppCommandError> {
    let result = create_group_chat_preset_inner(
        &state,
        req.name,
        req.description,
        req.moderator_model_id,
        req.participants,
        req.builtin_key,
    )
    .await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct UpdateGroupChatPresetRequest {
    pub id: String,
    pub name: String,
    pub description: String,
    pub moderator_model_id: String,
    pub participants: Vec<GcPresetParticipant>,
}

pub async fn update_group_chat_preset(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateGroupChatPresetRequest>,
) -> Result<Json<GcPresetRow>, AppCommandError> {
    let result = update_group_chat_preset_inner(
        &state,
        req.id,
        req.name,
        req.description,
        req.moderator_model_id,
        req.participants,
    )
    .await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct DeleteGroupChatPresetRequest {
    pub id: String,
}

pub async fn delete_group_chat_preset(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DeleteGroupChatPresetRequest>,
) -> Result<Json<DeletedGroupChatPreset>, AppCommandError> {
    let result = delete_group_chat_preset_inner(&state, req.id).await?;
    Ok(Json(result))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/list_group_chat_presets", post(list_group_chat_presets))
        .route("/create_group_chat_preset", post(create_group_chat_preset))
        .route("/update_group_chat_preset", post(update_group_chat_preset))
        .route("/delete_group_chat_preset", post(delete_group_chat_preset))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt; // oneshot

    /// 08-17 hotfix 先例:新 IPC 命令必须有一条 Router oneshot 测试锁
    /// wiring(daemon + Tauri + CMD_TO_DOMAIN 三处对齐由
    /// http.routes-sync.test.ts 守卫)。四条路由 CRUD 闭环 + 一条校验
    /// 失败 400。models 表需要行(create 的模型存在性校验经
    /// `db::get_model` 查)—— 用 `AppState::load_from_dir(tempdir)` 建
    /// 真实池后先经 `db::create_provider` + `db::create_model` 种行。
    #[tokio::test(flavor = "multi_thread")]
    async fn group_chat_preset_routes_wire_crud_and_reject_invalid() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);
        let provider = crate::db::create_provider(
            &state.db,
            "anthropic",
            "路由测试供应商",
            "https://api.test",
            "",
        )
        .await
        .unwrap();
        let m1 = crate::db::create_model(
            &state.db,
            &provider.id,
            "route-model-a",
            "路由模型 A",
            None,
            None,
            true,
            false,
            128_000,
        )
        .await
        .unwrap();
        let m2 = crate::db::create_model(
            &state.db,
            &provider.id,
            "route-model-b",
            "路由模型 B",
            None,
            None,
            true,
            false,
            128_000,
        )
        .await
        .unwrap();

        let app = router(state);

        async fn post_json(
            app: &axum::Router,
            uri: &str,
            body: &str,
        ) -> (StatusCode, serde_json::Value) {
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

        let participants = serde_json::json!([
            { "name": "架构", "modelId": m1.id, "persona": "arch" },
            { "name": "产品", "modelId": m2.id, "persona": "product" }
        ]);

        // create → 200,行形状 camelCase(wire 契约,design §2)。
        // 请求体顶层 snake_case(IPC 形状铁律:前端 invoke 的 camelCase
        // 顶层键经 httpTransport transformArgsTopLevel 扳回 snake),
        // 嵌套 participants 走 GcPresetParticipant 的 camelCase serde。
        let (code, v) = post_json(
            &app,
            "/create_group_chat_preset",
            &serde_json::json!({
                "name": "路由评审团",
                "description": "oneshot 测试",
                "moderator_model_id": m1.id,
                "participants": participants,
            })
            .to_string(),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "create: {v}");
        let id = v["id"].as_str().expect("create returns id").to_string();
        assert!(!id.is_empty());
        assert_eq!(v["moderatorModelId"], m1.id.as_str(), "wire camelCase");
        assert_eq!(v["participants"].as_array().unwrap().len(), 2);
        assert_eq!(v["participants"][0]["modelId"], m1.id.as_str());

        // 校验失败路径:撞内置 key → 400(design §3,commands 层单源)。
        let (code, v) = post_json(
            &app,
            "/create_group_chat_preset",
            &serde_json::json!({
                "name": "review",
                "description": "",
                "moderator_model_id": m1.id,
                "participants": participants,
            })
            .to_string(),
        )
        .await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "撞内置 key 必须 400: {v}");
        assert_eq!(v["category"], "InvalidRequest");

        // list → 200 且含刚建的行。
        let (code, v) = post_json(&app, "/list_group_chat_presets", "{}").await;
        assert_eq!(code, StatusCode::OK, "list: {v}");
        assert_eq!(v.as_array().unwrap().len(), 1);
        assert_eq!(v[0]["name"], "路由评审团");

        // update → 200,改名生效。
        let (code, v) = post_json(
            &app,
            "/update_group_chat_preset",
            &serde_json::json!({
                "id": id,
                "name": "改名后",
                "description": "",
                "moderator_model_id": m1.id,
                "participants": participants,
            })
            .to_string(),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "update: {v}");
        assert_eq!(v["name"], "改名后");
        assert!(v["createdAt"].is_string(), "wire camelCase: createdAt");

        // delete → 200 {ok:true};再删同 id 仍 200(幂等)。
        let (code, v) = post_json(
            &app,
            "/delete_group_chat_preset",
            &serde_json::json!({ "id": id }).to_string(),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "delete: {v}");
        assert_eq!(v["ok"], true);
        let (code, v) = post_json(
            &app,
            "/delete_group_chat_preset",
            &serde_json::json!({ "id": id }).to_string(),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "重复 delete 幂等: {v}");
        assert_eq!(v["ok"], true);

        // delete 后 list 归零。
        let (code, v) = post_json(&app, "/list_group_chat_presets", "{}").await;
        assert_eq!(code, StatusCode::OK);
        assert!(v.as_array().unwrap().is_empty());

        // --- GCE-P1b(2026-09-12, task `09-12-gc-preset-override`)
        // 覆盖流:create 带 snake 顶层 `builtin_key` → 响应 camelCase
        // `builtinKey`;同 key 二次 create → 400;delete 覆盖行 =
        // 恢复内置。name 用不撞内置 key 的名字(display 名 ≠ 链接键)。
        let (code, v) = post_json(
            &app,
            "/create_group_chat_preset",
            &serde_json::json!({
                "name": "架构档覆盖",
                "description": "覆盖内置 arch",
                "moderator_model_id": m1.id,
                "participants": participants,
                "builtin_key": "arch",
            })
            .to_string(),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "override create: {v}");
        assert_eq!(v["builtinKey"], "arch", "wire camelCase: builtinKey");
        let override_id = v["id"].as_str().expect("override id").to_string();

        // 同 key 二次 create → 400(每内置 key 至多一条覆盖)。
        let (code, v) = post_json(
            &app,
            "/create_group_chat_preset",
            &serde_json::json!({
                "name": "架构档覆盖二号",
                "description": "",
                "moderator_model_id": m1.id,
                "participants": participants,
                "builtin_key": "arch",
            })
            .to_string(),
        )
        .await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "同 key 二次覆盖: {v}");
        assert_eq!(v["category"], "InvalidRequest");

        // list 看得到覆盖行(带 builtinKey)。
        let (code, v) = post_json(&app, "/list_group_chat_presets", "{}").await;
        assert_eq!(code, StatusCode::OK, "list: {v}");
        assert_eq!(v.as_array().unwrap().len(), 1);
        assert_eq!(v[0]["id"], override_id.as_str());
        assert_eq!(v[0]["builtinKey"], "arch");

        // delete 覆盖行 → 恢复内置(list 不再含覆盖行,删 id 不存在)。
        let (code, v) = post_json(
            &app,
            "/delete_group_chat_preset",
            &serde_json::json!({ "id": override_id }).to_string(),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "delete override: {v}");
        assert_eq!(v["ok"], true);
        let (code, v) = post_json(&app, "/list_group_chat_presets", "{}").await;
        assert_eq!(code, StatusCode::OK);
        assert!(v.as_array().unwrap().is_empty(), "删除覆盖行 = 恢复内置");
    }
}
