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
/// 空串 = fixed 档新建专用 session(`_inner` 内定案);`target_mode =
/// "per_run"` = 每次执行新建 session(08-31-sched-per-run-session,
/// 不接受同时指定 target_session_id)。`max_runs` / `ends_at` 是 F2b
/// 结束条件(None = 不限)。`model_id`:fixed 专用 session 分支写
/// session 行,per_run 存任务行(None = 沿用全局默认模型)。
#[derive(Debug, Deserialize)]
pub struct CreateScheduledTaskRequest {
    pub project_id: String,
    pub target_session_id: Option<String>,
    pub target_mode: Option<String>,
    pub name: String,
    pub prompt: String,
    pub schedule: String,
    pub enabled: Option<bool>,
    pub max_runs: Option<i64>,
    pub ends_at: Option<i64>,
    pub model_id: Option<String>,
}

pub async fn create_scheduled_task(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateScheduledTaskRequest>,
) -> Result<Json<ScheduledTaskPayload>, AppCommandError> {
    let result = create_scheduled_task_inner(
        &state,
        req.project_id,
        req.target_session_id,
        req.target_mode,
        req.name,
        req.prompt,
        req.schedule,
        req.enabled,
        // wire 面恒 'user'(作者面分离:HTTP/IPC 只属用户;'agent' 仅
        // LLM schedule_task tool 路径,08-29-schedule-task-tool)。
        "user".to_string(),
        req.max_runs,
        req.ends_at,
        req.model_id,
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
    /// 双层 Option:缺省 = 不动;显式 `null` = 清空固定绑定(切
    /// per_run 的绑定侧);字符串 = 设定/换绑。`target_mode` 随同校验。
    #[serde(default, deserialize_with = "deserialize_double_option_string")]
    pub target_session_id: Option<Option<String>>,
    /// `fixed` | `per_run`(缺省不动)。
    pub target_mode: Option<String>,
    /// 双层 Option(per_run 档任务行模型):缺省 = 不动;`null` = 清空。
    #[serde(default, deserialize_with = "deserialize_double_option_string")]
    pub model_id: Option<Option<String>>,
    pub enabled: Option<bool>,
    /// 双层 Option:缺省 = 不动;显式 `null` = 清空为不限;数值 = 写入。
    #[serde(default, deserialize_with = "deserialize_double_option_i64")]
    pub max_runs: Option<Option<i64>>,
    #[serde(default, deserialize_with = "deserialize_double_option_i64")]
    pub ends_at: Option<Option<i64>>,
}

/// serde 双层 Option 反序列化(serde 惯例 "double option" 模式):
/// 字段缺省 → `None`(不动,由 `#[serde(default)]` 提供);显式 `null` →
/// `Some(None)`(清空为不限);数值 → `Some(Some(v))`。serde 对
/// `Option<Option<T>>` 的默认行为无法区分缺省与 `null`,必须包一层
/// `map(Some)` 才能让「显式 null 清空」过 wire。
fn deserialize_double_option_i64<'de, D>(deserializer: D) -> Result<Option<Option<i64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<i64>::deserialize(deserializer)?))
}

/// [`deserialize_double_option_i64`] 的 String 变体(`target_session_id` /
/// `model_id` 的显式 `null` 清空过 wire)。
fn deserialize_double_option_string<'de, D>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<String>::deserialize(deserializer)?))
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
        req.target_mode,
        req.model_id,
        req.enabled,
        req.max_runs,
        req.ends_at,
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
            row.target_session_id.as_deref(),
            Some(_session_id.as_str()),
            "rejected update must not mutate the stored target"
        );
    }

    /// F2b 结束条件 wire 形状:create 携带 max_runs / ends_at(顺带钉
    /// monthly 新档位过 wire);update 数值写入、显式 `null` 清空
    /// (double option:缺省 = 不动,`null` = 清空);max_runs=0 与过去
    /// ends_at 都 400。
    #[tokio::test(flavor = "multi_thread")]
    async fn scheduled_tasks_routes_end_conditions_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);
        let (project_id, _session_id) = seed_project_session(&state.db).await;

        let (status, body) = post_json(
            &state,
            "create_scheduled_task",
            format!(
                r#"{{"project_id":"{project_id}","name":"限次","prompt":"p","schedule":"{{\"kind\":\"monthly\",\"day\":15,\"at\":\"09:00\"}}","max_runs":5,"ends_at":4102444800000}}"#
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "create with end conditions: {body}");
        let created: serde_json::Value = serde_json::from_str(&body).unwrap();
        let task_id = created["id"].as_str().unwrap().to_string();
        assert_eq!(
            created["schedule"]["kind"], "monthly",
            "new preset rides the wire"
        );
        assert_eq!(created["max_runs"], 5);
        assert_eq!(created["ends_at"], 4_102_444_800_000i64);
        assert_eq!(created["run_count"], 0, "starts at zero");

        // update:数值写入。
        let (status, body) = post_json(
            &state,
            "update_scheduled_task",
            format!(r#"{{"id":"{task_id}","max_runs":10}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "update max_runs: {body}");
        let updated: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(updated["max_runs"], 10);

        // update:显式 null 清空 ends_at;同请求缺省的 max_runs 不动。
        let (status, body) = post_json(
            &state,
            "update_scheduled_task",
            format!(r#"{{"id":"{task_id}","ends_at":null}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "explicit null clears: {body}");
        let updated: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(updated["ends_at"].is_null(), "ends_at cleared to unlimited");
        assert_eq!(updated["max_runs"], 10, "absent sibling field untouched");

        // 校验臂:max_runs=0 → 400。
        let (status, body) = post_json(
            &state,
            "create_scheduled_task",
            format!(
                r#"{{"project_id":"{project_id}","name":"x","prompt":"p","schedule":"{{\"kind\":\"daily\",\"at\":\"09:00\"}}","max_runs":0}}"#
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "max_runs=0: {body}");
        assert!(body.contains("次数上限"), "max_runs gate message: {body}");

        // 校验臂:过去 ends_at → 400。
        let (status, body) = post_json(
            &state,
            "create_scheduled_task",
            format!(
                r#"{{"project_id":"{project_id}","name":"x","prompt":"p","schedule":"{{\"kind\":\"daily\",\"at\":\"09:00\"}}","ends_at":1000}}"#
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "past ends_at: {body}");
        assert!(body.contains("结束日期"), "ends_at gate message: {body}");
    }

    /// 单次档(CH11-1)+ 专用 session 指定模型的 wire 形状:create 携带
    /// 未来 `at_ms` 的 once 档 + `model_id` → 200 且新建 session 的
    /// `model_id` 列被指定值覆盖;过去 at_ms(create 与 update 两臂)与
    /// 不存在的 model_id → 400,且校验失败不留孤儿 session。
    #[tokio::test(flavor = "multi_thread")]
    async fn scheduled_tasks_routes_once_preset_and_model_binding() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);
        let (project_id, _session_id) = seed_project_session(&state.db).await;
        // 迁移 seed 了默认 provider + models,取一个真实 model id。
        let model = db::list_models(&state.db)
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("seeded model");

        let future_at = 4_102_444_800_000i64; // 2100-01-01,必在未来
        let (status, body) = post_json(
            &state,
            "create_scheduled_task",
            format!(
                r#"{{"project_id":"{project_id}","name":"一次性","prompt":"p","schedule":"{{\"kind\":\"once\",\"at_ms\":{future_at}}}","model_id":"{}"}}"#,
                model.model.id
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "once create with model: {body}");
        let created: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            created["schedule"]["kind"], "once",
            "once preset rides the wire"
        );
        assert_eq!(created["schedule"]["at_ms"], future_at);
        // 专用 session 的 per-session 模型覆盖列 = 指定值。
        let target = created["target_session_id"].as_str().unwrap();
        let dedicated = db::sessions::load_session(&state.db, target)
            .await
            .unwrap()
            .expect("dedicated session row");
        assert_eq!(
            dedicated.session.model_id.as_deref(),
            Some(model.model.id.as_str())
        );

        // create:过去 at_ms → 400。
        let (status, body) = post_json(
            &state,
            "create_scheduled_task",
            format!(
                r#"{{"project_id":"{project_id}","name":"x","prompt":"p","schedule":"{{\"kind\":\"once\",\"at_ms\":1000}}"}}"#
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "past once at_ms: {body}");
        assert!(body.contains("晚于当前时间"), "once gate message: {body}");

        // update:换成过去的 once 档 → 400,存量 schedule 不被改写。
        let task_id = created["id"].as_str().unwrap().to_string();
        let (status, body) = post_json(
            &state,
            "update_scheduled_task",
            format!(r#"{{"id":"{task_id}","schedule":"{{\"kind\":\"once\",\"at_ms\":1000}}"}}"#),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "past once at_ms update: {body}"
        );
        assert!(
            body.contains("晚于当前时间"),
            "once update gate message: {body}"
        );
        let row = db::scheduled_tasks::get_scheduled_task(&state.db, &task_id)
            .await
            .unwrap()
            .unwrap();
        assert!(
            row.schedule_json.contains(&future_at.to_string()),
            "rejected update must not mutate the stored schedule"
        );

        // 不存在的 model_id → 400,且不建孤儿 session(校验先于建 session)。
        let sessions_before = db::sessions::list_sessions(&state.db, &project_id)
            .await
            .unwrap()
            .len();
        let (status, body) = post_json(
            &state,
            "create_scheduled_task",
            format!(
                r#"{{"project_id":"{project_id}","name":"y","prompt":"p","schedule":"{{\"kind\":\"interval\",\"every_min\":30}}","model_id":"no-such-model"}}"#
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "bogus model_id: {body}");
        assert!(body.contains("不存在"), "model gate message: {body}");
        let sessions_after = db::sessions::list_sessions(&state.db, &project_id)
            .await
            .unwrap()
            .len();
        assert_eq!(
            sessions_before, sessions_after,
            "rejected model must not leave an orphan dedicated session"
        );
    }

    /// per_run 档(08-31-sched-per-run-session)wire 形状:create 携带
    /// `target_mode:"per_run"` → target_session_id 恒 null、不建 session;
    /// model_id 存任务行。update 在 fixed ↔ per_run 间切换(切 per_run
    /// 显式 null 清绑定;切回 fixed 未选 session → 400);矛盾/非法
    /// target_mode → 400。
    #[tokio::test(flavor = "multi_thread")]
    async fn scheduled_tasks_routes_per_run_mode_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);
        let (project_id, session_id) = seed_project_session(&state.db).await;
        let model = db::list_models(&state.db)
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("seeded model");

        // create per_run + model_id → 200;wire target null;不建 session。
        let sessions_before = db::sessions::list_sessions(&state.db, &project_id)
            .await
            .unwrap()
            .len();
        let (status, body) = post_json(
            &state,
            "create_scheduled_task",
            format!(
                r#"{{"project_id":"{project_id}","name":"每跑","prompt":"p","schedule":"{{\"kind\":\"interval\",\"every_min\":30}}","target_mode":"per_run","model_id":"{}"}}"#,
                model.model.id
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "per_run create: {body}");
        let created: serde_json::Value = serde_json::from_str(&body).unwrap();
        let task_id = created["id"].as_str().unwrap().to_string();
        assert_eq!(created["target_mode"], "per_run");
        assert!(
            created["target_session_id"].is_null(),
            "per_run binds no fixed session: {created}"
        );
        assert_eq!(
            created["model_id"], model.model.id,
            "model binding stored on the task row"
        );
        let sessions_after = db::sessions::list_sessions(&state.db, &project_id)
            .await
            .unwrap()
            .len();
        assert_eq!(
            sessions_before, sessions_after,
            "per_run create must not create any session"
        );

        // 矛盾请求:per_run + 显式 target_session_id → 400。
        let (status, body) = post_json(
            &state,
            "create_scheduled_task",
            format!(
                r#"{{"project_id":"{project_id}","target_session_id":"{session_id}","target_mode":"per_run","name":"x","prompt":"p","schedule":"{{\"kind\":\"interval\",\"every_min\":30}}"}}"#
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "contradiction: {body}");
        assert!(body.contains("二选一"), "contradiction message: {body}");

        // 非法 target_mode → 400。
        let (status, body) = post_json(
            &state,
            "create_scheduled_task",
            format!(
                r#"{{"project_id":"{project_id}","target_mode":"yolo","name":"x","prompt":"p","schedule":"{{\"kind\":\"interval\",\"every_min\":30}}"}}"#
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "bogus mode: {body}");
        assert!(body.contains("target_mode"), "mode gate message: {body}");

        // update:per_run → fixed(选定 session)→ target 落库。
        let (status, body) = post_json(
            &state,
            "update_scheduled_task",
            format!(
                r#"{{"id":"{task_id}","target_mode":"fixed","target_session_id":"{session_id}"}}"#
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "switch to fixed: {body}");
        let updated: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(updated["target_mode"], "fixed");
        assert_eq!(updated["target_session_id"], session_id);

        // update:fixed → per_run(显式 null 清绑定;model_id 同时可清)。
        let (status, body) = post_json(
            &state,
            "update_scheduled_task",
            format!(
                r#"{{"id":"{task_id}","target_mode":"per_run","target_session_id":null,"model_id":null}}"#
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "switch to per_run: {body}");
        let updated: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(updated["target_mode"], "per_run");
        assert!(updated["target_session_id"].is_null());
        assert!(updated["model_id"].is_null(), "model cleared explicitly");

        // update:per_run → fixed 未选 session → 400(存量 target 为 null)。
        let (status, body) = post_json(
            &state,
            "update_scheduled_task",
            r#"{"id":"__T__","target_mode":"fixed"}"#.replace("__T__", &task_id),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "per_run→fixed no target: {body}"
        );
        assert!(body.contains("请先选择"), "pick-session message: {body}");

        // update:per_run + 显式 sid → 400 矛盾(update 侧)。
        let (status, body) = post_json(
            &state,
            "update_scheduled_task",
            format!(
                r#"{{"id":"{task_id}","target_mode":"per_run","target_session_id":"{session_id}"}}"#
            ),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "update contradiction: {body}"
        );
    }
}
