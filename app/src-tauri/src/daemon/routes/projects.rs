//! `POST /api/v1/projects/<command>` handlers for the projects domain.
//!
//! Phase 2.2 B5 skeleton. Each handler deserializes a JSON body
//! into the same args the Tauri command takes, forwards to
//! `crate::commands::projects::xxx_inner` (Q0 decision — single
//! source of truth), and wraps the result in `Json(...)`. Errors
//! flow through `AppCommandError`'s `IntoResponse` impl.

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;

use crate::commands::projects::{
    browse_dir_inner, create_project_inner, hide_project_inner, list_hidden_projects_inner,
    list_projects_inner, unhide_project_inner, update_project_name_inner,
    update_project_path_inner, update_project_sandbox_policy_inner, BrowseDirPayload,
    ListProjectsFilter,
};
use crate::error::AppCommandError;
use crate::projects;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ListProjectsRequest {
    pub filter: Option<ListProjectsFilter>,
}

pub async fn list_projects(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ListProjectsRequest>,
) -> Result<Json<Vec<projects::ProjectRow>>, AppCommandError> {
    let result = list_projects_inner(&state, req.filter).await?;
    Ok(Json(result))
}

pub async fn list_hidden_projects(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<projects::ProjectRow>>, AppCommandError> {
    let result = list_hidden_projects_inner(&state).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct CreateProjectRequest {
    pub path: String,
}

pub async fn create_project(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateProjectRequest>,
) -> Result<Json<projects::ProjectRow>, AppCommandError> {
    let result = create_project_inner(&state, req.path).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct UpdateProjectPathRequest {
    pub id: String,
    pub new_path: String,
}

pub async fn update_project_path(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateProjectPathRequest>,
) -> Result<Json<projects::ProjectRow>, AppCommandError> {
    let result = update_project_path_inner(&state, req.id, req.new_path).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct UpdateProjectNameRequest {
    pub id: String,
    pub new_name: String,
}

pub async fn update_project_name(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateProjectNameRequest>,
) -> Result<Json<projects::ProjectRow>, AppCommandError> {
    let result = update_project_name_inner(&state, req.id, req.new_name).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct UpdateProjectSandboxPolicyRequest {
    pub id: String,
    pub policy: String,
}

/// P3c(design §2):项目沙盒策略档写入。snake_case 扁平顶层字段
/// (IPC 形状铁律),白名单校验在 `_inner`。
pub async fn update_project_sandbox_policy(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateProjectSandboxPolicyRequest>,
) -> Result<Json<projects::ProjectRow>, AppCommandError> {
    let result = update_project_sandbox_policy_inner(&state, req.id, req.policy).await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct HideProjectRequest {
    pub id: String,
}

pub async fn hide_project(
    State(state): State<Arc<AppState>>,
    Json(req): Json<HideProjectRequest>,
) -> Result<Json<()>, AppCommandError> {
    hide_project_inner(&state, req.id).await?;
    Ok(Json(()))
}

#[derive(Debug, Deserialize)]
pub struct UnhideProjectRequest {
    pub id: String,
}

pub async fn unhide_project(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UnhideProjectRequest>,
) -> Result<Json<()>, AppCommandError> {
    unhide_project_inner(&state, req.id).await?;
    Ok(Json(()))
}

/// `browse_dir` 请求体(snake_case,与 Tauri command 扁平标量参数一一
/// 对应;http.ts `transformArgsTopLevel` 已把顶层 `showHidden` 扳回
/// snake)。
#[derive(Debug, Deserialize)]
pub struct BrowseDirRequest {
    pub path: String,
    #[serde(default)]
    pub show_hidden: bool,
}

/// `POST /api/v1/projects/browse_dir` — browser-mode 目录浏览模态框的
/// 数据源(Tauri 原生选目录对话框在 daemon 侧不可用,见
/// `commands::projects::pick_project_dir` 的 Phase 2.2 note)。无
/// AppState 依赖(纯文件系统读,同 `config::get_home_dir`)。
pub async fn browse_dir(
    Json(req): Json<BrowseDirRequest>,
) -> Result<Json<BrowseDirPayload>, AppCommandError> {
    let result = browse_dir_inner(req.path, req.show_hidden).await?;
    Ok(Json(result))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/list_projects", post(list_projects))
        .route("/list_hidden_projects", post(list_hidden_projects))
        .route("/create_project", post(create_project))
        .route("/update_project_path", post(update_project_path))
        .route("/update_project_name", post(update_project_name))
        .route(
            "/update_project_sandbox_policy",
            post(update_project_sandbox_policy),
        )
        .route("/hide_project", post(hide_project))
        .route("/unhide_project", post(unhide_project))
        .route("/browse_dir", post(browse_dir))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt; // oneshot

    /// 2026-09-02 目录浏览模态框 route 冒烟:列目录(仅目录、隐藏
    /// 过滤、大小写不敏感排序)+ 非法路径 4xx。08-17 hotfix 先例:
    /// 新 IPC 命令必须有一条 Router oneshot 测试锁 wiring(daemon +
    /// Tauri + CMD_TO_DOMAIN 三处对齐由 http.routes-sync.test.ts
    /// 守卫)。
    #[tokio::test(flavor = "multi_thread")]
    async fn browse_dir_route_lists_dirs_and_rejects_bad_path() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("alpha")).unwrap();
        std::fs::create_dir(tmp.path().join(".secret")).unwrap();
        std::fs::write(tmp.path().join("f.txt"), "x").unwrap();

        let app = router(Arc::new(
            AppState::load_from_dir(tmp.path().to_path_buf()).await,
        ));

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

        let dir_str = tmp.path().to_string_lossy();
        // 默认:隐藏目录被过滤,文件不出现,parent 指回父目录
        let (code, v) = post_json(
            &app,
            "/browse_dir",
            &serde_json::json!({ "path": dir_str, "show_hidden": false }).to_string(),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let names: Vec<&str> = v["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["alpha"]);
        assert_eq!(
            v["parent"].as_str().unwrap(),
            tmp.path().parent().unwrap().to_string_lossy()
        );

        // show_hidden = true:.secret 出现
        let (code, v) = post_json(
            &app,
            "/browse_dir",
            &serde_json::json!({ "path": dir_str, "show_hidden": true }).to_string(),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let names: Vec<&str> = v["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec![".secret", "alpha"]);

        // 不存在的路径 → 4xx
        let (code, _) = post_json(
            &app,
            "/browse_dir",
            r#"{"path":"/definitely/not/a/real/dir","show_hidden":false}"#,
        )
        .await;
        assert_ne!(code, StatusCode::OK, "不存在的路径必须 4xx");
    }
}
