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
    /// fixed 档 = 目标 session;per_run 档 = `null`(wire 上前端按
    /// `target_mode` 区分渲染)。
    pub target_session_id: Option<String>,
    /// 目标模式:`fixed` | `per_run`(08-31-sched-per-run-session)。
    pub target_mode: String,
    /// per_run 档每次新建 session 的模型绑定;`null` = 全局默认。
    pub model_id: Option<String>,
    /// per_run 档最近一次 fire 新建的 session;`null` = 从未触发。
    pub last_run_session_id: Option<String>,
    pub name: String,
    pub prompt: String,
    pub schedule: serde_json::Value,
    pub enabled: bool,
    /// 作者:`'user'`(UI/IPC)或 `'agent'`(LLM `schedule_task` tool)。
    pub created_by: String,
    /// epoch ms。
    pub created_at: i64,
    /// epoch ms;`None` = 从未触发。
    pub last_fired_at: Option<i64>,
    /// epoch ms;仅 UI 展示。
    pub next_fire_at: i64,
    /// 已 fire 次数(F2b;dedup 跳过不计数)。
    pub run_count: i64,
    /// 次数上限;`None` = 不限(F2b)。
    pub max_runs: Option<i64>,
    /// 结束日期 epoch ms;`None` = 不限(F2b)。
    pub ends_at: Option<i64>,
}

impl From<&st::ScheduledTaskRow> for ScheduledTaskPayload {
    fn from(row: &st::ScheduledTaskRow) -> Self {
        let schedule = serde_json::from_str(&row.schedule_json).unwrap_or(serde_json::Value::Null);
        Self {
            id: row.id.clone(),
            project_id: row.project_id.clone(),
            target_session_id: row.target_session_id.clone(),
            target_mode: row.target_mode.clone(),
            model_id: row.model_id.clone(),
            last_run_session_id: row.last_run_session_id.clone(),
            name: row.name.clone(),
            prompt: row.prompt.clone(),
            schedule,
            enabled: row.enabled,
            created_by: row.created_by.clone(),
            created_at: row.created_at,
            last_fired_at: row.last_fired_at,
            next_fire_at: row.next_fire_at,
            run_count: row.run_count,
            max_runs: row.max_runs,
            ends_at: row.ends_at,
        }
    }
}

fn invalid(msg: impl Into<String>) -> AppCommandError {
    AppCommandError::new(ErrorCategory::InvalidRequest, msg)
}

/// F2b 结束条件校验:`max_runs ≥ 1`;`ends_at` 必须晚于当前时刻
/// (过去日期的任务一出生即完成,无意义,直接拒绝)。
fn validate_end_conditions(
    max_runs: Option<i64>,
    ends_at: Option<i64>,
) -> Result<(), AppCommandError> {
    if let Some(m) = max_runs {
        if m < 1 {
            return Err(invalid(format!("次数上限必须不小于 1,得到 {m}")));
        }
    }
    if let Some(t) = ends_at {
        if t <= crate::scheduler::now_epoch_ms() {
            return Err(invalid("结束日期必须晚于当前时间"));
        }
    }
    Ok(())
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

/// `target_mode` 归一化 + 校验:None/空 = fixed(缺省向后兼容);
/// 白名单外拒绝(400 中文错误)。独立小函数供 create / update 共用。
fn normalize_target_mode(raw: Option<&str>) -> Result<String, AppCommandError> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(st::target_modes::FIXED.to_string()),
        Some(v) if v == st::target_modes::FIXED => Ok(v.to_string()),
        Some(v) if v == st::target_modes::PER_RUN => Ok(v.to_string()),
        Some(v) => Err(invalid(format!(
            "target_mode 只支持 fixed / per_run,得到 {v}"
        ))),
    }
}

/// `create_scheduled_task` — `target_mode` 决定目标解析方式:
/// · fixed(缺省):`target_session_id` 为 `None`/空串时新建专用 session
///   (标题同任务名,cwd 取 project 根),为 `Some` 时校验存在且 classic
///   且 project 归属一致;`model_id` 仅专用 session 分支生效(写入新
///   session 的 per-session 覆盖列)。
/// · per_run(08-31-sched-per-run-session):不绑定固定 session(带
///   `target_session_id` → 400 矛盾);`model_id` 校验存在后存任务行,
///   每次触发新建 session 时应用。
/// `max_runs` / `ends_at` 是 F2b 结束条件(None = 不限)。`created_by`
/// 标作者:`'user'`(UI/IPC 路径,两个 transport 包装恒传)或 `'agent'`
/// (LLM `schedule_task` tool)。返回新行(id 服务端生成)。
/// 参数保持扁平标量(IPC 形状铁律,providers.rs 同款 allow)。
#[allow(clippy::too_many_arguments)]
pub async fn create_scheduled_task_inner(
    state: &Arc<AppState>,
    project_id: String,
    target_session_id: Option<String>,
    target_mode: Option<String>,
    name: String,
    prompt: String,
    schedule: String,
    enabled: Option<bool>,
    created_by: String,
    max_runs: Option<i64>,
    ends_at: Option<i64>,
    model_id: Option<String>,
) -> Result<ScheduledTaskPayload, AppCommandError> {
    create_scheduled_task_in_pool(
        &state.db,
        project_id,
        target_session_id,
        target_mode,
        name,
        prompt,
        schedule,
        enabled,
        created_by,
        max_runs,
        ends_at,
        model_id,
    )
    .await
}

/// [`create_scheduled_task_in_pool`] 的 pool 级核心:只依赖 DB,不依赖
/// `AppState`(`08-29-schedule-task-tool` D2 —— tool 层只有
/// `ToolContext.db`;Q0 单源不变,`_inner` 是薄包装,专用 session 分支
/// 经 [`crate::commands::sessions::create_session_in_pool`] 同理)。
#[allow(clippy::too_many_arguments)]
pub async fn create_scheduled_task_in_pool(
    db: &sqlx::SqlitePool,
    project_id: String,
    target_session_id: Option<String>,
    target_mode: Option<String>,
    name: String,
    prompt: String,
    schedule: String,
    enabled: Option<bool>,
    created_by: String,
    max_runs: Option<i64>,
    ends_at: Option<i64>,
    model_id: Option<String>,
) -> Result<ScheduledTaskPayload, AppCommandError> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(invalid("任务名称不能为空"));
    }
    if prompt.trim().is_empty() {
        return Err(invalid("任务提示词(prompt)不能为空"));
    }
    let target_mode = normalize_target_mode(target_mode.as_deref())?;
    let is_per_run = target_mode == st::target_modes::PER_RUN;
    validate_end_conditions(max_runs, ends_at)?;
    // schedule 先过解析器(中文错误信息直出前端)。
    let spec = crate::scheduler::compute::parse_schedule(&schedule).map_err(invalid)?;
    // 单次档的时刻必须在未来(过去时刻一出生即完成,无意义,与 F2b
    // 「过去 ends_at 直接拒绝」同一定案)。
    if let crate::scheduler::ScheduleSpec::Once { at_ms } = &spec {
        if *at_ms <= crate::scheduler::now_epoch_ms() {
            return Err(invalid("单次任务的触发时间必须晚于当前时间"));
        }
    }
    // schedule 落库统一为「解析后再序列化」的规范形(拒绝多余字段 /
    // 字段别名漂移;与调度器读取侧零分歧)。
    let schedule_json =
        serde_json::to_string(&spec).map_err(|e| invalid(format!("schedule 序列化失败: {e}")))?;
    // 指定模型:必须在 catalog 中存在(校验先于建 session,失败不留
    // 孤儿行)。fixed 档仅新建专用 session 分支使用(写入 session 行);
    // per_run 档存任务行,每次新建 run session 时应用。
    let model_id = match model_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(mid) => {
            let exists = crate::db::get_model(db, mid)
                .await
                .map_err(|e| anyhow::anyhow!("create_scheduled_task: load model failed: {}", e))?
                .is_some();
            if !exists {
                return Err(invalid(format!("模型 {mid} 不存在,请刷新后重选")));
            }
            Some(mid.to_string())
        }
        None => None,
    };

    let project = crate::db::get_project(db, &project_id)
        .await
        .map_err(|e| anyhow::anyhow!("create_scheduled_task: load project failed: {}", e))?
        .ok_or_else(|| invalid(format!("project {project_id} 不存在")))?;

    // 目标解析(per_run 档不绑定任何固定 session):
    // · fixed + 显式 sid → 校验存在 / classic / 归属一致(design §6);
    // · fixed + 缺省 → 新建专用 session(title 同任务名,cwd 取 project 根);
    // · per_run → target 恒 None(CHECK 不变式),带 sid 即矛盾请求。
    let resolved_target = if is_per_run {
        if target_session_id
            .as_deref()
            .map(str::trim)
            .is_some_and(|s| !s.is_empty())
        {
            return Err(invalid(
                "「每次新建 session」模式下不接受指定目标 session,请二选一",
            ));
        }
        None
    } else {
        match target_session_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(sid) => {
                st::validate_target_session(db, sid)
                    .await
                    .map_err(invalid)?;
                // project 归属一致(design §6):任务挂 A project、目标 session
                // 属 B project 会让「按 project 过滤的列表」显示错位的行。
                let session_project: Option<(String,)> =
                    sqlx::query_as("SELECT project_id FROM sessions WHERE id = ?")
                        .bind(sid)
                        .fetch_optional(db)
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
                Some(sid.to_string())
            }
            None => {
                let session = crate::commands::sessions::create_session_in_pool(
                    db,
                    project_id.clone(),
                    project.path.clone(),
                    None,
                    None,
                    None,
                )
                .await?;
                // 标题同任务名(design §6);rename 截断 80 字符(db 层)。
                crate::db::rename_session(db, &session.id, &name)
                    .await
                    .map_err(|e| {
                        anyhow::anyhow!(
                            "create_scheduled_task: rename dedicated session failed: {}",
                            e
                        )
                    })?;
                // 指定模型 → 写 per-session 覆盖列(缺省:create_session_in_pool
                // 已绑全局默认,不动)。每轮 chat 经 resolve_model_id_for_session
                // 优先取该列 —— 定时注入的轮次由此固定模型,不随全局默认漂移。
                if let Some(mid) = &model_id {
                    crate::db::update_session_model_id(db, &session.id, mid)
                        .await
                        .map_err(|e| {
                            anyhow::anyhow!(
                                "create_scheduled_task: bind dedicated session model failed: {}",
                                e
                            )
                        })?;
                }
                Some(session.id)
            }
        }
    };

    let enabled = enabled.unwrap_or(true);
    let next_fire_at =
        crate::scheduler::compute::next_fire_display(&spec, crate::scheduler::now_epoch_ms());
    let row = st::insert_scheduled_task(
        db,
        st::NewScheduledTask {
            project_id,
            target_session_id: resolved_target,
            target_mode,
            // per_run:存任务行(每轮建 session 时应用);fixed:模型
            // 绑定在专用 session 行上,任务行不重复记。
            model_id: if is_per_run { model_id } else { None },
            name,
            prompt,
            schedule_json,
            enabled,
            created_by,
            next_fire_at,
            max_runs,
            ends_at,
        },
    )
    .await
    .map_err(|e| anyhow::anyhow!("create_scheduled_task: insert failed: {}", e))?;
    Ok(ScheduledTaskPayload::from(&row))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn create_scheduled_task(
    state: State<'_, Arc<AppState>>,
    project_id: String,
    target_session_id: Option<String>,
    target_mode: Option<String>,
    name: String,
    prompt: String,
    schedule: String,
    enabled: Option<bool>,
    max_runs: Option<i64>,
    ends_at: Option<i64>,
    model_id: Option<String>,
) -> Result<ScheduledTaskPayload, AppCommandError> {
    create_scheduled_task_inner(
        state.inner(),
        project_id,
        target_session_id,
        target_mode,
        name,
        prompt,
        schedule,
        enabled,
        "user".to_string(),
        max_runs,
        ends_at,
        model_id,
    )
    .await
}

/// `update_scheduled_task` — 部分更新(`None` 字段不动存量)。schedule /
/// target 变更时同样过校验;enabled false→true 由 WP1 db 层置
/// `last_fired_at = now` + `run_count = 0`(重启用不补跑、计数重置,
/// design §3 + F2b D8)。`max_runs` / `ends_at` / `target_session_id` /
/// `model_id` 是 F2b 式双层 Option:外层 `None` = 不动,内层 `None`
/// (wire 显式 `null`)= 清空(target 清空即切 per_run 的绑定侧;
/// `target_mode` 须随同变更,校验见下)。目标模式规则:
/// · resolved = per_run:target 写 `Some(None)` 清空;同 patch 再带具体
///   sid → 400 矛盾。`model_id` 可写/清(存任务行)。
/// · resolved = fixed:target `Some(Some(sid))` 且变化 → 存在 + classic +
///   归属校验;patch 缺省而存量无固定目标(per_run 切回未选)→ 400;
///   显式 `Some(None)` → 400(fixed 必须有目标)。
/// `next_fire_at` 不由本命令维护(展示值由 db 层在 schedule/enabled 跳变时
/// 重推,权威触发判定每 tick 重算)。目标不存在返回 `InvalidRequest`
/// (编辑竞态:行已被他端删除时前端 toast,不静默)。
#[allow(clippy::too_many_arguments)]
pub async fn update_scheduled_task_inner(
    state: &Arc<AppState>,
    id: String,
    name: Option<String>,
    prompt: Option<String>,
    schedule: Option<String>,
    target_session_id: Option<Option<String>>,
    target_mode: Option<String>,
    model_id: Option<Option<String>>,
    enabled: Option<bool>,
    max_runs: Option<Option<i64>>,
    ends_at: Option<Option<i64>>,
) -> Result<ScheduledTaskPayload, AppCommandError> {
    let existing = st::get_scheduled_task(&state.db, &id)
        .await
        .map_err(|e| anyhow::anyhow!("update_scheduled_task: load failed: {}", e))?
        .ok_or_else(|| invalid(format!("定时任务 {id} 不存在")))?;

    let target_mode = match &target_mode {
        Some(v) => normalize_target_mode(Some(v))?,
        None => existing.target_mode.clone(),
    };
    let is_per_run = target_mode == st::target_modes::PER_RUN;

    // F2b 结束条件:只校验显式写入的值(清空动作不校验);
    // 过去的 ends_at 会立刻完成,视为误操作直接拒绝。
    if let Some(v) = max_runs {
        validate_end_conditions(v, None)?;
    }
    if let Some(v) = ends_at {
        validate_end_conditions(None, v)?;
    }

    // schedule 合法性(提供才校验;非法拒绝**不写库**)。单次档时刻
    // 必须在未来(编辑一次性任务 = 重定时刻,过期即拒,与 create 同款)。
    let schedule_json = match schedule.as_deref() {
        Some(json) => {
            let spec = crate::scheduler::compute::parse_schedule(json).map_err(invalid)?;
            if let crate::scheduler::ScheduleSpec::Once { at_ms } = &spec {
                if *at_ms <= crate::scheduler::now_epoch_ms() {
                    return Err(invalid("单次任务的触发时间必须晚于当前时间"));
                }
            }
            Some(
                serde_json::to_string(&spec)
                    .map_err(|e| invalid(format!("schedule 序列化失败: {e}")))?,
            )
        }
        None => None,
    };

    // 目标解析(语义见函数头)。validated = None 表示「不写/清空」,
    // 与「target_mode 变更必须落库」一起组装双层 Option。
    let mut validated_target: Option<Option<String>> = None;
    if is_per_run {
        // 切 per_run:清空固定绑定;同 patch 带 sid 即矛盾。
        if let Some(Some(sid)) = &target_session_id {
            if !sid.trim().is_empty() {
                return Err(invalid(
                    "「每次新建 session」模式下不接受指定目标 session,请二选一",
                ));
            }
        }
        if existing.target_session_id.is_some() || target_session_id.is_some() {
            validated_target = Some(None);
        }
    } else {
        match &target_session_id {
            Some(Some(sid))
                if sid.trim() != existing.target_session_id.as_deref().unwrap_or("") =>
            {
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
                validated_target = Some(Some(sid.to_string()));
            }
            Some(Some(_)) => {
                // 与存量相同:显式写回(无害,保持 wire 幂等)。
                validated_target = target_session_id.clone();
            }
            Some(None) => {
                return Err(invalid("固定目标模式下必须指定目标 session"));
            }
            None => {
                if existing.target_session_id.is_none() {
                    return Err(invalid(
                        "该任务当前为「每次新建 session」模式,切换固定目标请先选择 session",
                    ));
                }
                // per_run → fixed 且未换 target(存量非空):无需写
                // target,mode 单独落库。
            }
        }
    }

    // model_id:外层不动;内层 None 清空 / Some 校验存在后写。
    // fixed 档该列不参与语义(仅 per_run 存任务行),但写入无害、
    // 校验照做(防脏数据)。
    let validated_model = match &model_id {
        Some(Some(mid)) => {
            let mid = mid.trim();
            let exists = crate::db::get_model(&state.db, mid)
                .await
                .map_err(|e| anyhow::anyhow!("update_scheduled_task: load model failed: {}", e))?
                .is_some();
            if !exists {
                return Err(invalid(format!("模型 {mid} 不存在,请刷新后重选")));
            }
            Some(Some(mid.to_string()))
        }
        other => other.clone(),
    };

    let updated = st::update_scheduled_task(
        &state.db,
        &id,
        st::UpdateScheduledTask {
            name: name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()),
            prompt: prompt.filter(|p| !p.trim().is_empty()),
            schedule_json,
            target_session_id: validated_target,
            target_mode: (target_mode != existing.target_mode).then_some(target_mode),
            model_id: validated_model,
            enabled,
            max_runs,
            ends_at,
        },
    )
    .await
    .map_err(|e| anyhow::anyhow!("update_scheduled_task failed: {}", e))?
    .ok_or_else(|| invalid(format!("定时任务 {id} 不存在")))?;
    Ok(ScheduledTaskPayload::from(&updated))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn update_scheduled_task(
    state: State<'_, Arc<AppState>>,
    id: String,
    name: Option<String>,
    prompt: Option<String>,
    schedule: Option<String>,
    target_session_id: Option<Option<String>>,
    target_mode: Option<String>,
    model_id: Option<Option<String>>,
    enabled: Option<bool>,
    max_runs: Option<Option<i64>>,
    ends_at: Option<Option<i64>>,
) -> Result<ScheduledTaskPayload, AppCommandError> {
    update_scheduled_task_inner(
        state.inner(),
        id,
        name,
        prompt,
        schedule,
        target_session_id,
        target_mode,
        model_id,
        enabled,
        max_runs,
        ends_at,
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
