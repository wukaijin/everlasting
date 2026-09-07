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
    /// M4a 定时审议档:存档的展开群聊配置(前端编辑回显用);存量行
    /// 损坏(理论上不可能:写入时已校验)降级 `Null`,与 `schedule` 同款。
    pub group_chat_config: serde_json::Value,
    /// M4a:最近一次 fire 的结局(`started/resumed/skipped_busy/error/
    /// recovered`;`null` = 从未触发)。任务卡「上次 fire 怎么了」直读。
    pub last_fire_outcome: Option<String>,
}

impl From<&st::ScheduledTaskRow> for ScheduledTaskPayload {
    fn from(row: &st::ScheduledTaskRow) -> Self {
        let schedule = serde_json::from_str(&row.schedule_json).unwrap_or(serde_json::Value::Null);
        let group_chat_config = row
            .group_chat_config
            .as_deref()
            .and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or(serde_json::Value::Null);
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
            group_chat_config,
            last_fire_outcome: row.last_fire_outcome.clone(),
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
        Some(v) if v == st::target_modes::GROUP_CHAT => Ok(v.to_string()),
        Some(v) => Err(invalid(format!(
            "target_mode 只支持 fixed / per_run / group_chat,得到 {v}"
        ))),
    }
}

/// M4a:群聊配置校验 + 规范化(struct → 规范 JSON 文本)。校验序:
/// 结构([`st::parse_group_chat_task_config`]:形状 / 非空名单 / 无重名)
/// → 模型存在性(moderator + 全部 participants 查 models 表,任一缺失
/// 400 —— 与 fire 侧 catalog 预检同源不同时,创建时拦住用户输错)。
async fn validate_group_chat_config(
    db: &sqlx::SqlitePool,
    config: &st::GroupChatTaskConfig,
) -> Result<String, AppCommandError> {
    let raw =
        serde_json::to_string(config).map_err(|e| invalid(format!("群聊配置序列化失败: {e}")))?;
    // 结构校验走 parse(与读侧同门,顺带锁规范形)。
    let parsed = st::parse_group_chat_task_config(&raw).map_err(invalid)?;
    let mut missing: Vec<&str> = Vec::new();
    if crate::db::get_model(db, &parsed.moderator_model_id)
        .await
        .map_err(|e| anyhow::anyhow!("create_scheduled_task: load model failed: {}", e))?
        .is_none()
    {
        missing.push("moderator");
    }
    for p in &parsed.participants {
        if crate::db::get_model(db, &p.model_id)
            .await
            .map_err(|e| anyhow::anyhow!("create_scheduled_task: load model failed: {}", e))?
            .is_none()
        {
            missing.push(&p.name);
        }
    }
    if !missing.is_empty() {
        return Err(invalid(format!(
            "群聊配置引用的模型不存在(缺:{}),请刷新模型列表后重选",
            missing.join(" / ")
        )));
    }
    serde_json::to_string(&parsed).map_err(|e| invalid(format!("群聊配置序列化失败: {e}")))
}

/// `create_scheduled_task` — `target_mode` 决定目标解析方式:
/// · fixed(缺省):`target_session_id` 为 `None`/空串时新建专用 session
///   (标题同任务名,cwd 取 project 根),为 `Some` 时校验存在且 classic
///   且 project 归属一致;`model_id` 仅专用 session 分支生效(写入新
///   session 的 per-session 覆盖列)。
/// · per_run(08-31-sched-per-run-session):不绑定固定 session(带
///   `target_session_id` → 400 矛盾);`model_id` 校验存在后存任务行,
///   每次触发新建 session 时应用。
/// · group_chat(M4a 定时审议):fire 时 daemon 建群开跑。不接受
///   `target_session_id`(400)与 `model_id`(moderator 在 config 内);
///   `group_chat_config` 必填(结构 + 模型存在性校验)。
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
    group_chat_config: Option<st::GroupChatTaskConfig>,
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
        group_chat_config,
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
    group_chat_config: Option<st::GroupChatTaskConfig>,
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
    let is_group_chat = target_mode == st::target_modes::GROUP_CHAT;
    // M4a 互斥:group_chat 档的模型绑定全在 config JSON(moderator),
    // 顶层的 model_id(per_run 惯例)在此档无语义 —— 显式拒绝防脏数据。
    if is_group_chat {
        if target_session_id
            .as_deref()
            .map(str::trim)
            .is_some_and(|s| !s.is_empty())
        {
            return Err(invalid(
                "「定时审议」模式下不接受指定目标 session,每次触发自动建群,请二选一",
            ));
        }
        if model_id
            .as_deref()
            .map(str::trim)
            .is_some_and(|s| !s.is_empty())
        {
            return Err(invalid(
                "「定时审议」模式的主持人模型在群聊配置(group_chat_config)内指定,不接受 model_id",
            ));
        }
    } else if group_chat_config.is_some() {
        return Err(invalid(
            "群聊配置(group_chat_config)仅「定时审议」模式接受,请勿与固定目标/每次新建混用",
        ));
    }
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

    // M4a:群聊配置结构 + 模型存在性校验(moderator + 全部 participants),
    // 规范化为 canonical JSON 落库(读侧零分歧,与 schedule 同款定案)。
    let group_chat_config_json = if is_group_chat {
        let config = group_chat_config
            .as_ref()
            .ok_or_else(|| invalid("「定时审议」模式必须提供群聊配置(group_chat_config)"))?;
        Some(validate_group_chat_config(db, config).await?)
    } else {
        None
    };

    let project = crate::db::get_project(db, &project_id)
        .await
        .map_err(|e| anyhow::anyhow!("create_scheduled_task: load project failed: {}", e))?
        .ok_or_else(|| invalid(format!("project {project_id} 不存在")))?;

    // 目标解析(per_run / group_chat 档不绑定任何固定 session):
    // · fixed + 显式 sid → 校验存在 / classic / 归属一致(design §6);
    // · fixed + 缺省 → 新建专用 session(title 同任务名,cwd 取 project 根);
    // · per_run → target 恒 None(CHECK 不变式),带 sid 即矛盾请求;
    // · group_chat → target 恒 None(互斥已在上方拒绝),不建 session
    //   (每次 fire 建群,session 归 fire 侧所有)。
    let resolved_target = if is_group_chat {
        None
    } else if is_per_run {
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
            // 绑定在专用 session 行上,任务行不重复记;group_chat:
            // 顶层 model_id 已被拒绝(恒 None),moderator 在 config JSON。
            model_id: if is_per_run { model_id } else { None },
            name,
            prompt,
            schedule_json,
            enabled,
            created_by,
            next_fire_at,
            max_runs,
            ends_at,
            group_chat_config: group_chat_config_json,
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
    group_chat_config: Option<st::GroupChatTaskConfig>,
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
        group_chat_config,
    )
    .await
}

/// `update_scheduled_task` — 部分更新(`None` 字段不动存量)。schedule /
/// target 变更时同样过校验;enabled false→true 由 WP1 db 层置
/// `last_fired_at = now` + `run_count = 0`(重启用不补跑、计数重置,
/// design §3 + F2b D8)。`max_runs` / `ends_at` / `target_session_id` /
/// `model_id` / `group_chat_config` 是 F2b 式双层 Option:外层 `None` =
/// 不动,内层 `None`(wire 显式 `null`)= 清空。目标模式规则:
/// · resolved = per_run:target 写 `Some(None)` 清空;同 patch 再带具体
///   sid → 400 矛盾。`model_id` 可写/清(存任务行)。
/// · resolved = fixed:target `Some(Some(sid))` 且变化 → 存在 + classic +
///   归属校验;patch 缺省而存量无固定目标(per_run 切回未选)→ 400;
///   显式 `Some(None)` → 400(fixed 必须有目标)。
/// · resolved = group_chat(M4a):target 清空(带 sid → 400);config
///   必须在位 —— patch 提供则校验写入,缺省而存量无 config(从别的档
///   切来)→ 400,显式 `Some(None)` 清空 → 400(group_chat 必须持有
///   config);`model_id` 顶层参数在该档无语义(显式设 → 400,存量
///   自动清)。
/// · 切离 group_chat(→ fixed/per_run):config 自动显式清空
///   (`Some(None)`,防跨档脏数据残留);同 patch 显式带 config → 400 互斥。
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
    group_chat_config: Option<Option<st::GroupChatTaskConfig>>,
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
    let is_group_chat = target_mode == st::target_modes::GROUP_CHAT;

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
    if is_group_chat {
        // 切/留 group_chat:清空固定绑定(同 patch 带 sid 即矛盾);
        // 该档不建专用 session,存量 target 只可能是别的档遗留。
        if let Some(Some(sid)) = &target_session_id {
            if !sid.trim().is_empty() {
                return Err(invalid(
                    "「定时审议」模式下不接受指定目标 session,每次触发自动建群,请二选一",
                ));
            }
        }
        if existing.target_session_id.is_some() || target_session_id.is_some() {
            validated_target = Some(None);
        }
    } else if is_per_run {
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
    // 校验照做(防脏数据)。group_chat 档该列无语义(moderator 在
    // config):显式设 → 400,存量自动清(跨档切换不留脏值)。
    let validated_model = if is_group_chat {
        if let Some(Some(mid)) = &model_id {
            if !mid.trim().is_empty() {
                return Err(invalid(
                    "「定时审议」模式的主持人模型在群聊配置(group_chat_config)内指定,不接受 model_id",
                ));
            }
        }
        existing.model_id.is_some().then_some(None)
    } else {
        match &model_id {
            Some(Some(mid)) => {
                let mid = mid.trim();
                let exists = crate::db::get_model(&state.db, mid)
                    .await
                    .map_err(|e| {
                        anyhow::anyhow!("update_scheduled_task: load model failed: {}", e)
                    })?
                    .is_some();
                if !exists {
                    return Err(invalid(format!("模型 {mid} 不存在,请刷新后重选")));
                }
                Some(Some(mid.to_string()))
            }
            other => other.clone(),
        }
    };

    // M4a group_chat_config(双层 Option,语义见函数头):
    // · group_chat:提供则校验写入;缺省须存量已有(否则 400);
    //   显式清空 → 400(CHECK 不变式 group_chat ⇒ config 非空)。
    // · fixed/per_run:显式提供 → 400 互斥;切离时(缺省而存量有)
    //   自动 Some(None) 清空。
    let validated_config: Option<Option<String>> = if is_group_chat {
        match &group_chat_config {
            Some(Some(config)) => Some(Some(validate_group_chat_config(&state.db, config).await?)),
            Some(None) => {
                return Err(invalid(
                    "「定时审议」模式必须持有群聊配置(group_chat_config),不能清空",
                ));
            }
            None => {
                if existing.group_chat_config.is_none() {
                    return Err(invalid(
                        "切换到「定时审议」模式需提供群聊配置(group_chat_config)",
                    ));
                }
                None
            }
        }
    } else {
        match &group_chat_config {
            Some(Some(_)) => {
                return Err(invalid(
                    "群聊配置(group_chat_config)仅「定时审议」模式接受,请先切换目标模式",
                ));
            }
            Some(None) => Some(None),
            None => existing.group_chat_config.is_some().then_some(None),
        }
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
            group_chat_config: validated_config,
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
    group_chat_config: Option<Option<st::GroupChatTaskConfig>>,
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
        group_chat_config,
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
