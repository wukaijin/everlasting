//! `POST /api/v1/scheduled_tasks/<command>` handlers for the F2
//! scheduled-tasks domain (2026-08-28, task `08-28-f2-scheduled-tasks`).
//!
//! Thin JSON wrappers mirroring `commands::scheduled_tasks` (Q0 单源):
//! deserialize the snake_case body into the same flat scalar args the
//! Tauri command takes, forward to the `_inner`, wrap the result in
//! `Json(...)`. Errors flow through `AppCommandError`'s `IntoResponse`.

use std::sync::Arc;

use axum::{extract::State, routing::post, Json, Router};
use serde::Deserialize;

use crate::commands::scheduled_tasks::{
    create_scheduled_task_inner, delete_scheduled_task_inner, list_scheduled_tasks_inner,
    update_scheduled_task_inner, ScheduledTaskPayload,
};
use crate::error::AppCommandError;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ListScheduledTasksRequest {
    pub project_id: Option<String>,
}

pub async fn list_scheduled_tasks(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ListScheduledTasksRequest>,
) -> Result<Json<Vec<ScheduledTaskPayload>>, AppCommandError> {
    let result = list_scheduled_tasks_inner(&state, req.project_id).await?;
    Ok(Json(result))
}

/// 请求体 snake_case、扁平标量(同 Tauri command 形参;transport 的
/// 顶层 camel→snake 转换后两形态等价)。`target_session_id` 缺省 /
/// 空串 = 新建专用 session(`_inner` 内定案)。
#[derive(Debug, Deserialize)]
pub struct CreateScheduledTaskRequest {
    pub project_id: String,
    pub target_session_id: Option<String>,
    pub name: String,
    pub prompt: String,
    pub schedule: String,
    pub enabled: Option<bool>,
}

pub async fn create_scheduled_task(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateScheduledTaskRequest>,
) -> Result<Json<ScheduledTaskPayload>, AppCommandError> {
    let result = create_scheduled_task_inner(
        &state,
        req.project_id,
        req.target_session_id,
        req.name,
        req.prompt,
        req.schedule,
        req.enabled,
    )
    .await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct UpdateScheduledTaskRequest {
    pub id: String,
    pub name: Option<String>,
    pub prompt: Option<String>,
    pub schedule: Option<String>,
    pub target_session_id: Option<String>,
    pub enabled: Option<bool>,
}

pub async fn update_scheduled_task(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateScheduledTaskRequest>,
) -> Result<Json<ScheduledTaskPayload>, AppCommandError> {
    let result = update_scheduled_task_inner(
        &state,
        req.id,
        req.name,
        req.prompt,
        req.schedule,
        req.target_session_id,
        req.enabled,
    )
    .await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct DeleteScheduledTaskRequest {
    pub id: String,
}

pub async fn delete_scheduled_task(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DeleteScheduledTaskRequest>,
) -> Result<Json<bool>, AppCommandError> {
    let result = delete_scheduled_task_inner(&state, req.id).await?;
    Ok(Json(result))
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/list_scheduled_tasks", post(list_scheduled_tasks))
        .route("/create_scheduled_task", post(create_scheduled_task))
        .route("/update_scheduled_task", post(update_scheduled_task))
        .route("/delete_scheduled_task", post(delete_scheduled_task))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt; // oneshot

    async fn post_json(state: &Arc<AppState>, cmd: &str, body: String) -> (StatusCode, String) {
        let app = router(state.clone());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/{cmd}"))
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).to_string())
    }

    async fn seed_project_session(pool: &sqlx::SqlitePool) -> (String, String) {
        let name = format!("sched-route-{}", uuid::Uuid::new_v4().simple());
        let path = format!("/tmp/sched-route-{name}");
        db::create_project(pool, &name, &path, false, None)
            .await
            .unwrap();
        let project = db::list_projects(pool, false)
            .await
            .unwrap()
            .into_iter()
            .find(|p| p.name == name)
            .unwrap();
        let session_id = uuid::Uuid::new_v4().to_string();
        db::create_session(
            pool,
            &session_id,
            &project.id,
            &path,
            "GLM-4.7",
            None,
            None,
            None,
        )
        .await
        .unwrap();
        (project.id, session_id)
    }

    /// Parity smoke(spec backend/daemon-server.md §6 — new IPC commands
    /// get a Router oneshot test):一轮往返打满四条 route —— create
    /// (走「缺省 target = 新建专用 session」分支,证明 transport 接线
    /// + `_inner` 的建 session 复用)→ list → update(enabled=false)
    /// → delete。CRUD 语义本体在 `commands::scheduled_tasks` /
    /// `db::scheduled_tasks` 的单测覆盖,这里钉住双 transport 形状。
    #[tokio::test(flavor = "multi_thread")]
    async fn scheduled_tasks_routes_crud_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);
        let (project_id, _session_id) = seed_project_session(&state.db).await;

        // create:target_session_id 缺省 → 新建专用 session。
        let (status, body) = post_json(
            &state,
            "create_scheduled_task",
            format!(
                r#"{{"project_id":"{project_id}","name":"早报","prompt":"汇总进展","schedule":"{{\"kind\":\"interval\",\"every_min\":30}}"}}"#
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "create route: {body}");
        let created: serde_json::Value = serde_json::from_str(&body).unwrap();
        let task_id = created["id"]
            .as_str()
            .expect("server-side uuid")
            .to_string();
        assert_eq!(created["name"], "早报");
        assert_eq!(created["enabled"], true, "enabled defaults to true");
        assert_eq!(
            created["schedule"]["kind"], "interval",
            "schedule rides the wire as a parsed object"
        );
        assert_eq!(created["schedule"]["every_min"], 30);
        assert!(created["last_fired_at"].is_null(), "never fired on create");
        // 专用 session:标题同任务名、挂同一 project(design §6)。
        let target = created["target_session_id"].as_str().unwrap();
        let dedicated = db::sessions::load_session(&state.db, target)
            .await
            .unwrap()
            .expect("dedicated session row");
        assert_eq!(dedicated.session.title, "早报");
        assert_eq!(dedicated.session.project_id, project_id);

        // list:按 project 过滤命中;shape 上 next_fire_at 是展示值。
        let (status, body) = post_json(
            &state,
            "list_scheduled_tasks",
            format!(r#"{{"project_id":"{project_id}"}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "list route: {body}");
        let listed: Vec<serde_json::Value> = serde_json::from_str(&body).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["id"].as_str(), Some(task_id.as_str()));
        assert!(listed[0]["next_fire_at"].is_number());

        // update:enabled true→false(基准不重置路径)。
        let (status, body) = post_json(
            &state,
            "update_scheduled_task",
            format!(r#"{{"id":"{task_id}","enabled":false}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "update route: {body}");
        let updated: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(updated["enabled"], false);

        // delete:幂等语义 = 第二次删返回 false。
        let (status, body) = post_json(
            &state,
            "delete_scheduled_task",
            format!(r#"{{"id":"{task_id}"}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "delete route: {body}");
        assert_eq!(body, "true");
        let (_, body) = post_json(
            &state,
            "delete_scheduled_task",
            format!(r#"{{"id":"{task_id}"}}"#),
        )
        .await;
        assert_eq!(body, "false");
    }

    /// 校验分支 route smoke:群聊目标(AC7)与非法 schedule 都必须在
    /// route 层拿到 400 + 用户可读中文错误(InvalidRequest → BAD_REQUEST,
    /// `_inner` 校验即 gate,不写库)。
    #[tokio::test(flavor = "multi_thread")]
    async fn scheduled_tasks_routes_reject_group_chat_and_bad_schedule() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);
        let (project_id, _session_id) = seed_project_session(&state.db).await;
        let group_id = uuid::Uuid::new_v4().to_string();
        db::create_session(
            &state.db,
            &group_id,
            &project_id,
            "/tmp/sched-route-group",
            "GLM-4.7",
            None,
            Some("group_chat"),
            None,
        )
        .await
        .unwrap();

        let (status, body) = post_json(
            &state,
            "create_scheduled_task",
            format!(
                r#"{{"project_id":"{project_id}","target_session_id":"{group_id}","name":"x","prompt":"p","schedule":"{{\"kind\":\"interval\",\"every_min\":30}}"}}"#
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "group chat: {body}");
        assert!(body.contains("群聊"), "group gate message: {body}");

        let (status, body) = post_json(
            &state,
            "update_scheduled_task",
            // 更新一个不存在的任务 → 400(任务不存在)。
            r#"{"id":"no-such-task","name":"y"}"#.to_string(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "missing task: {body}");
        assert!(body.contains("不存在"), "missing-task message: {body}");

        let (status, body) = post_json(
            &state,
            "create_scheduled_task",
            format!(
                r#"{{"project_id":"{project_id}","name":"z","prompt":"p","schedule":"{{\"kind\":\"cron\",\"expr\":\"* * * * *\"}}"}}"#
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "bad schedule: {body}");
        assert!(body.contains("schedule"), "schedule gate message: {body}");

        // project 归属一致(design §6):任务挂 A project、目标 session
        // 属 B project → 400(create 与 update 两个分支都要 gate)。
        let (project_b, session_b) = seed_project_session(&state.db).await;
        assert_ne!(project_b, project_id, "seed must yield distinct projects");
        let (status, body) = post_json(
            &state,
            "create_scheduled_task",
            format!(
                r#"{{"project_id":"{project_id}","target_session_id":"{session_b}","name":"x","prompt":"p","schedule":"{{\"kind\":\"interval\",\"every_min\":30}}"}}"#
            ),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "cross-project create: {body}"
        );
        assert!(
            body.contains("不属于"),
            "cross-project create message: {body}"
        );

        // update 分支:先建一个合法任务(目标 = 本 project 的 session),
        // 再把 target 换成 B project 的 session → 400,存量不被改写。
        let (status, body) = post_json(
            &state,
            "create_scheduled_task",
            format!(
                r#"{{"project_id":"{project_id}","target_session_id":"{_session_id}","name":"base","prompt":"p","schedule":"{{\"kind\":\"interval\",\"every_min\":30}}"}}"#
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "baseline task create: {body}");
        let created: serde_json::Value = serde_json::from_str(&body).unwrap();
        let task_id = created["id"].as_str().unwrap().to_string();
        let (status, body) = post_json(
            &state,
            "update_scheduled_task",
            format!(r#"{{"id":"{task_id}","target_session_id":"{session_b}"}}"#),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "cross-project update: {body}"
        );
        assert!(
            body.contains("不属于该任务所在 project"),
            "cross-project update message: {body}"
        );
        let row = db::scheduled_tasks::get_scheduled_task(&state.db, &task_id)
            .await
            .unwrap()
            .expect("task row survives rejected update");
        assert_eq!(
            row.target_session_id, _session_id,
            "rejected update must not mutate the stored target"
        );
    }
}
