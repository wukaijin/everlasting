//! F2 定时任务(2026-08-28, task `08-28-f2-scheduled-tasks`)— CRUD
//! IPC 四件:list / create / update / delete。
//!
//! 双形态三层同 tunnel config / web_search 先例:`_inner` 业务(Q0 单源)
//! + `#[tauri::command]` 包装 + daemon route(`daemon/routes/scheduled_tasks.rs`)。
//! 校验(design §6):目标 session 必须存在且 `session_type='chat'`(群聊
//! 拒绝,AC7;复用 WP1 的 [`crate::db::scheduled_tasks::validate_target_session`]),
//! project 归属一致,schedule JSON 经 [`crate::scheduler::compute::parse_schedule`]
//! 合法。create 的 `target_session_id` 缺省 = 新建专用 session(标题同任务
//! 名,cwd 取 project 根;复用 [`create_session_inner`])。
//!
//! 参数全部**扁平标量**(IPC 形状铁律,08-21 实证:嵌套 struct 参数在
//! HTTP 模式静默 miss);wire DTO 字段 snake_case(不加
//! `rename_all = "camelCase"`,BACKLOG §5.2 项目决策)。

use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::db::scheduled_tasks as st;
use crate::error::{AppCommandError, ErrorCategory};
use crate::state::AppState;

/// `scheduled_tasks` 行的前端视图(wire: snake_case,BACKLOG §5.2 不加
/// camelCase rename)。`schedule` 是已解析的 preset 对象
/// (`{"kind":"daily","at":"09:00"}` 等);存量行损坏(理论上不可能:
/// 写入时已校验)降级为 `Null`,前端按「未知档位」渲染。`next_fire_at`
/// 是纯展示值(触发判定每 tick 重算,design §2)。
#[derive(Debug, Clone, Serialize)]
pub struct ScheduledTaskPayload {
    pub id: String,
    pub project_id: String,
    pub target_session_id: String,
    pub name: String,
    pub prompt: String,
    pub schedule: serde_json::Value,
    pub enabled: bool,
    /// epoch ms。
    pub created_at: i64,
    /// epoch ms;`None` = 从未触发。
    pub last_fired_at: Option<i64>,
    /// epoch ms;仅 UI 展示。
    pub next_fire_at: i64,
}

impl From<&st::ScheduledTaskRow> for ScheduledTaskPayload {
    fn from(row: &st::ScheduledTaskRow) -> Self {
        let schedule = serde_json::from_str(&row.schedule_json).unwrap_or(serde_json::Value::Null);
        Self {
            id: row.id.clone(),
            project_id: row.project_id.clone(),
            target_session_id: row.target_session_id.clone(),
            name: row.name.clone(),
            prompt: row.prompt.clone(),
            schedule,
            enabled: row.enabled,
            created_at: row.created_at,
            last_fired_at: row.last_fired_at,
            next_fire_at: row.next_fire_at,
        }
    }
}

fn invalid(msg: impl Into<String>) -> AppCommandError {
    AppCommandError::new(ErrorCategory::InvalidRequest, msg)
}

/// `list_scheduled_tasks(projectId?)` — 全量 / 按 project(创建序)。
pub async fn list_scheduled_tasks_inner(
    state: &Arc<AppState>,
    project_id: Option<String>,
) -> Result<Vec<ScheduledTaskPayload>, AppCommandError> {
    let rows = st::list_scheduled_tasks(&state.db, project_id.as_deref())
        .await
        .map_err(|e| anyhow::anyhow!("list_scheduled_tasks failed: {}", e))?;
    Ok(rows.iter().map(ScheduledTaskPayload::from).collect())
}

#[tauri::command]
pub async fn list_scheduled_tasks(
    state: State<'_, Arc<AppState>>,
    project_id: Option<String>,
) -> Result<Vec<ScheduledTaskPayload>, AppCommandError> {
    list_scheduled_tasks_inner(state.inner(), project_id).await
}

/// `create_scheduled_task` — `target_session_id` 为 `None` / 空串时新建
/// 专用 session(标题同任务名,cwd 取 project 根);为 `Some` 时校验存在
/// 且 classic 且 project 归属一致。返回新行(id 服务端生成)。
pub async fn create_scheduled_task_inner(
    state: &Arc<AppState>,
    project_id: String,
    target_session_id: Option<String>,
    name: String,
    prompt: String,
    schedule: String,
    enabled: Option<bool>,
) -> Result<ScheduledTaskPayload, AppCommandError> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(invalid("任务名称不能为空"));
    }
    if prompt.trim().is_empty() {
        return Err(invalid("任务提示词(prompt)不能为空"));
    }
    // schedule 先过解析器(中文错误信息直出前端)。
    let spec = crate::scheduler::compute::parse_schedule(&schedule).map_err(invalid)?;
    // schedule 落库统一为「解析后再序列化」的规范形(拒绝多余字段 /
    // 字段别名漂移;与调度器读取侧零分歧)。
    let schedule_json =
        serde_json::to_string(&spec).map_err(|e| invalid(format!("schedule 序列化失败: {e}")))?;

    let project = crate::db::get_project(&state.db, &project_id)
        .await
        .map_err(|e| anyhow::anyhow!("create_scheduled_task: load project failed: {}", e))?
        .ok_or_else(|| invalid(format!("project {project_id} 不存在")))?;

    // 目标 session 解析:显式指定 → 校验;缺省 → 新建专用 session
    // (design §6:title 同任务名,cwd 取 project 根)。
    let trimmed_target = target_session_id.as_deref().map(str::trim);
    let target_session_id = match trimmed_target.filter(|s| !s.is_empty()) {
        Some(sid) => {
            st::validate_target_session(&state.db, sid)
                .await
                .map_err(invalid)?;
            // project 归属一致(design §6):任务挂 A project、目标 session
            // 属 B project 会让「按 project 过滤的列表」显示错位的行。
            let session_project: Option<(String,)> =
                sqlx::query_as("SELECT project_id FROM sessions WHERE id = ?")
                    .bind(sid)
                    .fetch_optional(&state.db)
                    .await
                    .map_err(|e| {
                        anyhow::anyhow!("create_scheduled_task: load session failed: {}", e)
                    })?;
            match session_project {
                Some((pid,)) if pid == project_id => {}
                _ => {
                    return Err(invalid("目标 session 不属于所选 project,请重新选择"));
                }
            }
            sid.to_string()
        }
        None => {
            let session = crate::commands::sessions::create_session_inner(
                state,
                project_id.clone(),
                project.path.clone(),
                None,
                None,
                None,
            )
            .await?;
            // 标题同任务名(design §6);rename 截断 80 字符(db 层)。
            crate::db::rename_session(&state.db, &session.id, &name)
                .await
                .map_err(|e| {
                    anyhow::anyhow!(
                        "create_scheduled_task: rename dedicated session failed: {}",
                        e
                    )
                })?;
            session.id
        }
    };

    let enabled = enabled.unwrap_or(true);
    let next_fire_at =
        crate::scheduler::compute::next_fire_display(&spec, crate::scheduler::now_epoch_ms());
    let row = st::insert_scheduled_task(
        &state.db,
        st::NewScheduledTask {
            project_id,
            target_session_id,
            name,
            prompt,
            schedule_json,
            enabled,
            next_fire_at,
        },
    )
    .await
    .map_err(|e| anyhow::anyhow!("create_scheduled_task: insert failed: {}", e))?;
    Ok(ScheduledTaskPayload::from(&row))
}

#[tauri::command]
pub async fn create_scheduled_task(
    state: State<'_, Arc<AppState>>,
    project_id: String,
    target_session_id: Option<String>,
    name: String,
    prompt: String,
    schedule: String,
    enabled: Option<bool>,
) -> Result<ScheduledTaskPayload, AppCommandError> {
    create_scheduled_task_inner(
        state.inner(),
        project_id,
        target_session_id,
        name,
        prompt,
        schedule,
        enabled,
    )
    .await
}

/// `update_scheduled_task` — 部分更新(`None` 字段不动存量)。schedule /
/// target_session 变更时同样过校验;enabled false→true 由 WP1 db 层置
/// `last_fired_at = now`(重启用不补跑,design §3)。`next_fire_at` 不由
/// 本命令维护(展示值由 db 层在 schedule/enabled 跳变时重推,权威触发
/// 判定每 tick 重算)。目标不存在返回 `InvalidRequest`(编辑竞态:
/// 行已被他端删除时前端 toast,不静默)。
pub async fn update_scheduled_task_inner(
    state: &Arc<AppState>,
    id: String,
    name: Option<String>,
    prompt: Option<String>,
    schedule: Option<String>,
    target_session_id: Option<String>,
    enabled: Option<bool>,
) -> Result<ScheduledTaskPayload, AppCommandError> {
    let existing = st::get_scheduled_task(&state.db, &id)
        .await
        .map_err(|e| anyhow::anyhow!("update_scheduled_task: load failed: {}", e))?
        .ok_or_else(|| invalid(format!("定时任务 {id} 不存在")))?;

    // schedule 合法性(提供才校验;非法拒绝**不写库**)。
    let schedule_json = match schedule.as_deref() {
        Some(json) => {
            let spec = crate::scheduler::compute::parse_schedule(json).map_err(invalid)?;
            Some(
                serde_json::to_string(&spec)
                    .map_err(|e| invalid(format!("schedule 序列化失败: {e}")))?,
            )
        }
        None => None,
    };
    // 新目标 session:存在 + classic + 归属任务的 project(design §6)。
    let validated_target = match target_session_id.as_deref() {
        Some(sid) if sid.trim() != existing.target_session_id => {
            let sid = sid.trim();
            st::validate_target_session(&state.db, sid)
                .await
                .map_err(invalid)?;
            let session_project: Option<(String,)> =
                sqlx::query_as("SELECT project_id FROM sessions WHERE id = ?")
                    .bind(sid)
                    .fetch_optional(&state.db)
                    .await
                    .map_err(|e| {
                        anyhow::anyhow!("update_scheduled_task: load session failed: {}", e)
                    })?;
            match session_project {
                Some((pid,)) if pid == existing.project_id => {}
                _ => {
                    return Err(invalid("目标 session 不属于该任务所在 project"));
                }
            }
            Some(sid.to_string())
        }
        _ => None,
    };

    let updated = st::update_scheduled_task(
        &state.db,
        &id,
        st::UpdateScheduledTask {
            name: name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()),
            prompt: prompt.filter(|p| !p.trim().is_empty()),
            schedule_json,
            target_session_id: validated_target,
            enabled,
        },
    )
    .await
    .map_err(|e| anyhow::anyhow!("update_scheduled_task failed: {}", e))?
    .ok_or_else(|| invalid(format!("定时任务 {id} 不存在")))?;
    Ok(ScheduledTaskPayload::from(&updated))
}

#[tauri::command]
pub async fn update_scheduled_task(
    state: State<'_, Arc<AppState>>,
    id: String,
    name: Option<String>,
    prompt: Option<String>,
    schedule: Option<String>,
    target_session_id: Option<String>,
    enabled: Option<bool>,
) -> Result<ScheduledTaskPayload, AppCommandError> {
    update_scheduled_task_inner(
        state.inner(),
        id,
        name,
        prompt,
        schedule,
        target_session_id,
        enabled,
    )
    .await
}

/// `delete_scheduled_task` — 硬删。返回是否真删了一行(`false` = 已被
/// 他端删除,前端按幂等成功处理)。
pub async fn delete_scheduled_task_inner(
    state: &Arc<AppState>,
    id: String,
) -> Result<bool, AppCommandError> {
    let deleted = st::delete_scheduled_task(&state.db, &id)
        .await
        .map_err(|e| anyhow::anyhow!("delete_scheduled_task failed: {}", e))?;
    Ok(deleted)
}

#[tauri::command]
pub async fn delete_scheduled_task(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<bool, AppCommandError> {
    delete_scheduled_task_inner(state.inner(), id).await
}
