//! `scheduled_tasks` 表 CRUD(F2 定时任务,`08-28-f2-scheduled-tasks`
//! design §2)。
//!
//! 表 DDL 在 `db/migrations/schema.rs` 追加(幂等 `CREATE TABLE IF NOT
//! EXISTS`,无 schema version 机制的全序列重放模式);双 FK
//! (`projects` / `sessions`)`ON DELETE CASCADE` —— 删 session / project
//! 级联删任务(AC6)。
//!
//! 触发判定**不信任**存库 `next_fire_at`(纯 UI 展示列,调度器每 tick 在
//! Rust 侧重算,design §2/§3);本模块只在写入路径维护一个展示值。
//!
//! 用户侧语义(design §3 定案):`update_scheduled_task` 在 enabled
//! false→true 时把 `last_fired_at = now` —— 显式禁用是用户主动行为,
//! 重新启用从**下一个**到期点开始,不补停用期存量(daemon 非自愿停机
//! 的 D4 补跑语义不受影响,那个判定在调度器内重算)。

use sqlx::SqlitePool;

/// 目标 session 模式(`target_mode` 列;08-31-sched-per-run-session)。
/// `fixed` = 注入到固定目标 session(`target_session_id` 非空,CHECK
/// 不变式);`per_run` = 每次触发自动新建 session(`target_session_id`
/// 恒 NULL,`last_run_session_id` 记最近一次的 run session,无 FK ——
/// 删旧 run session 不得级联删任务)。
pub mod target_modes {
    pub const FIXED: &str = "fixed";
    pub const PER_RUN: &str = "per_run";
}

/// 供调度器与 UI 读取的任务行。`schedule_json` 是原始 JSON 文本
/// (反序列化经 [`crate::scheduler::compute::parse_schedule`],校验
/// 在写入时完成,读取时信任)。
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduledTaskRow {
    pub id: String,
    pub project_id: String,
    /// 固定目标 session;`target_mode = per_run` 时恒 `None`(CHECK
    /// 不变式:fixed ⇔ 非空)。
    pub target_session_id: Option<String>,
    /// 目标模式:[`target_modes::FIXED`] | [`target_modes::PER_RUN`]。
    pub target_mode: String,
    /// per_run 档每次新建 session 绑定的模型(None = 全局默认);
    /// fixed 档不使用(模型绑定在专用 session 行上)。
    pub model_id: Option<String>,
    /// per_run 档最近一次 fire 新建的 session(无 FK;审计锚点 +
    /// 列表展示)。从未 fire 过 = None。
    pub last_run_session_id: Option<String>,
    pub name: String,
    pub prompt: String,
    pub schedule_json: String,
    pub enabled: bool,
    pub created_by: String,
    /// epoch ms。
    pub created_at: i64,
    /// NULL = 从未触发;触发判定基准(调度器侧)。
    pub last_fired_at: Option<i64>,
    /// 纯 UI 展示;不参与触发判定。
    pub next_fire_at: i64,
    /// 已 fire 次数(F2b;只计真正送入 chat_inner 的落账,dedup 跳过不计)。
    pub run_count: i64,
    /// 次数上限;NULL = 不限(F2b prd D10,对所有档位通用)。
    pub max_runs: Option<i64>,
    /// 结束日期(epoch ms,该时刻前含当日的到期点照常触发);NULL = 不限。
    pub ends_at: Option<i64>,
}

/// [`insert_scheduled_task`] 的载荷。`next_fire_at` 由调用方按
/// [`crate::scheduler::compute::next_fire_display`] 计算(展示语义)。
#[derive(Debug, Clone)]
pub struct NewScheduledTask {
    pub project_id: String,
    /// fixed 档 = 目标 session;per_run 档 = None。
    pub target_session_id: Option<String>,
    /// [`target_modes::FIXED`] | [`target_modes::PER_RUN`]。
    pub target_mode: String,
    /// per_run 档的模型绑定(None = 全局默认)。
    pub model_id: Option<String>,
    pub name: String,
    pub prompt: String,
    /// 已过 [`crate::scheduler::compute::parse_schedule`] 校验的 JSON。
    pub schedule_json: String,
    pub enabled: bool,
    /// 作者:`'user'`(UI/IPC 路径)或 `'agent'`(LLM `schedule_task`
    /// tool 路径,`08-29-schedule-task-tool`)。列自 F2 起存在,当时
    /// MVP 恒写 `'user'`;参数化即本任务兑现的预告。
    pub created_by: String,
    pub next_fire_at: i64,
    pub max_runs: Option<i64>,
    pub ends_at: Option<i64>,
}

/// [`update_scheduled_task`] 的载荷。`None` 字段不动存量;`enabled` 的
/// false→true 跳变触发 `last_fired_at = now` + `run_count = 0`(重启用
/// 不补跑、计数重置,design §3 + F2b D8)。`max_runs` / `ends_at` /
/// `target_session_id` / `model_id` 是双层 Option:外层 `None` = 不动,
/// 内层 `None` = 显式清空(per_run 切换时 target 清空)。
#[derive(Debug, Clone, Default)]
pub struct UpdateScheduledTask {
    pub name: Option<String>,
    pub prompt: Option<String>,
    pub schedule_json: Option<String>,
    pub target_session_id: Option<Option<String>>,
    pub target_mode: Option<String>,
    pub model_id: Option<Option<String>>,
    pub enabled: Option<bool>,
    pub max_runs: Option<Option<i64>>,
    pub ends_at: Option<Option<i64>>,
}

/// epoch ms。与 `agent/chat.rs` 路由临界区的 `now_ms` 同口径。
fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

fn row_from(row: &sqlx::sqlite::SqliteRow) -> Result<ScheduledTaskRow, sqlx::Error> {
    use sqlx::Row;
    Ok(ScheduledTaskRow {
        id: row.try_get("id")?,
        project_id: row.try_get("project_id")?,
        target_session_id: row.try_get("target_session_id")?,
        target_mode: row.try_get("target_mode")?,
        model_id: row.try_get("model_id")?,
        last_run_session_id: row.try_get("last_run_session_id")?,
        name: row.try_get("name")?,
        prompt: row.try_get("prompt")?,
        schedule_json: row.try_get("schedule")?,
        enabled: row.try_get::<i64, _>("enabled")? != 0,
        created_by: row.try_get("created_by")?,
        created_at: row.try_get("created_at")?,
        last_fired_at: row.try_get("last_fired_at")?,
        next_fire_at: row.try_get("next_fire_at")?,
        run_count: row.try_get("run_count")?,
        max_runs: row.try_get("max_runs")?,
        ends_at: row.try_get("ends_at")?,
    })
}

const SELECT_COLS: &str = "id, project_id, target_session_id, target_mode, model_id, \
     last_run_session_id, name, prompt, schedule, \
     enabled, created_by, created_at, last_fired_at, next_fire_at, run_count, max_runs, ends_at";

/// 新建任务。id 服务端生成(uuid);`created_by` 随载荷(`'user'` =
/// UI/IPC 路径,`'agent'` = LLM `schedule_task` tool)。
pub async fn insert_scheduled_task(
    pool: &SqlitePool,
    new: NewScheduledTask,
) -> Result<ScheduledTaskRow, sqlx::Error> {
    let id = uuid::Uuid::new_v4().to_string();
    let created_at = now_epoch_ms();
    sqlx::query(
        r#"
 INSERT INTO scheduled_tasks
 (id, project_id, target_session_id, target_mode, model_id, last_run_session_id, name, prompt, schedule, enabled, created_by, created_at, last_fired_at, next_fire_at, run_count, max_runs, ends_at)
 VALUES (?, ?, ?, ?, ?, NULL, ?, ?, ?, ?, ?, ?, NULL, ?, 0, ?, ?)
 "#,
    )
    .bind(&id)
    .bind(&new.project_id)
    .bind(&new.target_session_id)
    .bind(&new.target_mode)
    .bind(&new.model_id)
    .bind(&new.name)
    .bind(&new.prompt)
    .bind(&new.schedule_json)
    .bind(new.enabled as i64)
    .bind(&new.created_by)
    .bind(created_at)
    .bind(new.next_fire_at)
    .bind(new.max_runs)
    .bind(new.ends_at)
    .execute(pool)
    .await?;
    Ok(get_scheduled_task(pool, &id)
        .await?
        .expect("row just inserted"))
}

/// 全量 / 按 project 列表(创建序)。
pub async fn list_scheduled_tasks(
    pool: &SqlitePool,
    project_id: Option<&str>,
) -> Result<Vec<ScheduledTaskRow>, sqlx::Error> {
    list_scheduled_tasks_filtered(pool, project_id, None).await
}

/// 带作者过滤的列表变体。`created_by = Some("agent")` 供 LLM
/// `schedule_status` tool 只看自己建的任务(`08-29-schedule-task-tool`,
/// 作者面分离:用户建的任务仍归 Settings UI);`None` = 不过滤(既有
/// UI/IPC 路径)。两过滤器自由组合。
pub async fn list_scheduled_tasks_filtered(
    pool: &SqlitePool,
    project_id: Option<&str>,
    created_by: Option<&str>,
) -> Result<Vec<ScheduledTaskRow>, sqlx::Error> {
    let mut sql = format!("SELECT {SELECT_COLS} FROM scheduled_tasks WHERE 1=1");
    if project_id.is_some() {
        sql.push_str(" AND project_id = ?");
    }
    if created_by.is_some() {
        sql.push_str(" AND created_by = ?");
    }
    sql.push_str(" ORDER BY created_at ASC, id ASC");
    let mut q = sqlx::query(&sql);
    if let Some(pid) = project_id {
        q = q.bind(pid);
    }
    if let Some(by) = created_by {
        q = q.bind(by);
    }
    let rows = q.fetch_all(pool).await?;
    rows.iter().map(row_from).collect()
}

/// 指定 project + 作者的 enabled(活跃)任务数。LLM `schedule_task`
/// 反滥用上限的计数来源(`08-29-schedule-task-tool` D3;上限只约束
/// agent 路径,UI 创建不限)。
pub async fn count_enabled_by_creator(
    pool: &SqlitePool,
    project_id: &str,
    created_by: &str,
) -> Result<i64, sqlx::Error> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM scheduled_tasks WHERE project_id = ? AND created_by = ? AND enabled = 1",
    )
    .bind(project_id)
    .bind(created_by)
    .fetch_one(pool)
    .await?;
    Ok(n)
}

/// enabled 任务的调度扫描序:`last_fired_at` NULLS FIRST(从未触发的
/// 任务先评估)再按时间升序。调度循环(design §4)的消费入口。
pub async fn list_enabled_scheduled_tasks(
    pool: &SqlitePool,
) -> Result<Vec<ScheduledTaskRow>, sqlx::Error> {
    // SQLite 3.30 前无 NULLS FIRST 关键字,用 IS NULL 布尔序(等价且
    // 全版本可用):NULL 排最前,再按 last_fired_at 升序。
    let rows = sqlx::query(&format!(
        "SELECT {SELECT_COLS} FROM scheduled_tasks WHERE enabled = 1 \
         ORDER BY (last_fired_at IS NULL) DESC, last_fired_at ASC, id ASC"
    ))
    .fetch_all(pool)
    .await?;
    rows.iter().map(row_from).collect()
}

/// 按 id 读取。
pub async fn get_scheduled_task(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<ScheduledTaskRow>, sqlx::Error> {
    let row = sqlx::query(&format!(
        "SELECT {SELECT_COLS} FROM scheduled_tasks WHERE id = ?"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(row_from).transpose()
}

/// 部分更新。enabled false→true 时 `last_fired_at = now`(design §3:
/// 重启用从下一个到期点开始,不补停用期);其余跳变不触碰基准。
/// schedule / target_session 变更时展示值 `next_fire_at` 同步重算。
/// `target_session_id` / `model_id` 是双层 Option(外层不动 / 内层清空)。
/// 返回更新后的行;id 不存在返回 `None`。
pub async fn update_scheduled_task(
    pool: &SqlitePool,
    id: &str,
    upd: UpdateScheduledTask,
) -> Result<Option<ScheduledTaskRow>, sqlx::Error> {
    let Some(existing) = get_scheduled_task(pool, id).await? else {
        return Ok(None);
    };
    let name = upd.name.unwrap_or(existing.name.clone());
    let prompt = upd.prompt.unwrap_or(existing.prompt.clone());
    let schedule_changed = upd.schedule_json.is_some();
    let enabled_changed = upd.enabled.is_some();
    let schedule_json = upd.schedule_json.unwrap_or(existing.schedule_json.clone());
    let target_session_id = match upd.target_session_id {
        None => existing.target_session_id.clone(),
        Some(v) => v,
    };
    let target_mode = upd.target_mode.unwrap_or(existing.target_mode.clone());
    let model_id = match upd.model_id {
        None => existing.model_id.clone(),
        Some(v) => v,
    };
    let enabled = upd.enabled.unwrap_or(existing.enabled);

    // false→true:用户主动重启用 → 从现在起算,不补跑存量(design §3);
    // 结束条件计数同步清零(F2b D8:重新启用 = 重新计次)。
    let re_enabled = !existing.enabled && enabled;
    let last_fired_at = if re_enabled {
        Some(now_epoch_ms())
    } else {
        existing.last_fired_at
    };
    let run_count = if re_enabled { 0 } else { existing.run_count };
    let max_runs = match upd.max_runs {
        None => existing.max_runs,
        Some(v) => v,
    };
    let ends_at = match upd.ends_at {
        None => existing.ends_at,
        Some(v) => v,
    };

    // 展示值重算时机:schedule 或 enabled 跳变时按当前时刻重推
    // (停用任务存「schedule 的下一个到期点」灰显,design §2)。
    let next_fire_at = if schedule_changed || enabled_changed {
        crate::scheduler::compute::parse_schedule(&schedule_json)
            .ok()
            .map(|spec| crate::scheduler::compute::next_fire_display(&spec, now_epoch_ms()))
            .unwrap_or(existing.next_fire_at)
    } else {
        existing.next_fire_at
    };

    sqlx::query(
        r#"
 UPDATE scheduled_tasks
 SET name = ?, prompt = ?, schedule = ?, target_session_id = ?, target_mode = ?, model_id = ?, enabled = ?, last_fired_at = ?, next_fire_at = ?, run_count = ?, max_runs = ?, ends_at = ?
 WHERE id = ?
 "#,
    )
    .bind(&name)
    .bind(&prompt)
    .bind(&schedule_json)
    .bind(&target_session_id)
    .bind(&target_mode)
    .bind(&model_id)
    .bind(enabled as i64)
    .bind(last_fired_at)
    .bind(next_fire_at)
    .bind(run_count)
    .bind(max_runs)
    .bind(ends_at)
    .bind(id)
    .execute(pool)
    .await?;
    get_scheduled_task(pool, id).await
}

/// 调度器落账(design §4:fire / dedup 跳过后调用)。`last_fired_at`
/// 恒记**理论到期点 due**(防 interval 相位漂移,design §3);
/// `next_fire_at` 写 `next_fire_display(schedule, due)` 展示值。
/// `count_fire` 控制 `run_count + 1` 与否(F2b):真正送入 chat_inner
/// 的落账(Queued/Started/Error)计一次;dedup 跳过(prompt 未送达)
/// 仅消费 due 点不计数。`last_run_session_id` 是 per_run 档本次新建的
/// run session:`Some` 覆写、`None` 保留旧值(COALESCE;fixed 档与
/// dedup 跳过路径恒传 None)。
/// 触碰 `last_fired_at` 时**不**套 false→true 语义 —— 本函数只被调度器
/// 在任务保持 enabled 的前提下调用。返回 affected 行数(0 = 任务已被删)。
pub async fn mark_task_fired(
    pool: &SqlitePool,
    id: &str,
    last_fired_at: i64,
    next_fire_at: i64,
    count_fire: bool,
    last_run_session_id: Option<&str>,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE scheduled_tasks SET last_fired_at = ?, next_fire_at = ?, run_count = run_count + ?, \
         last_run_session_id = COALESCE(?, last_run_session_id) WHERE id = ?",
    )
    .bind(last_fired_at)
    .bind(next_fire_at)
    .bind(count_fire as i64)
    .bind(last_run_session_id)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// 任务完成落账(F2b D8:达 `max_runs` 或到期点越过 `ends_at`)。
/// 仅置 `enabled = 0` 让任务退出调度扫描;`last_fired_at` /
/// `next_fire_at` / `run_count` / 结束条件三列保持落账原值(UI 依其
/// 渲染「已完成 N/M」/「已结束」)。对已 disabled 行重写是列级 no-op
/// (SQLite `rows_affected` 仍计 1,它数的是 WHERE 命中数);返回
/// affected 行数(0 = 行已被删)。
pub async fn mark_task_completed(pool: &SqlitePool, id: &str) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("UPDATE scheduled_tasks SET enabled = 0 WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

/// 硬删。返回是否真删了一行。
pub async fn delete_scheduled_task(pool: &SqlitePool, id: &str) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM scheduled_tasks WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// 目标 session 校验(R4/AC7):必须存在且 `session_type = 'chat'`
/// (群聊不走消息队列路由,legacy 分支对忙 session 是取消语义,fire
/// 必须避开)。`Err` 携带中文用户可读信息。
pub async fn validate_target_session(
    pool: &SqlitePool,
    target_session_id: &str,
) -> Result<(), String> {
    let row = sqlx::query("SELECT session_type FROM sessions WHERE id = ?")
        .bind(target_session_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("校验目标 session 失败: {e}"))?;
    let Some(row) = row else {
        return Err(format!("目标 session {target_session_id} 不存在"));
    };
    use sqlx::Row;
    let session_type: String = row
        .try_get(0)
        .map_err(|e| format!("校验目标 session 失败: {e}"))?;
    if session_type != "chat" {
        return Err("定时任务只能绑定普通聊天 session(群聊不支持定时注入)".to_string());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests(RULE-TESTPOOL-001:一律 test_pool,in-memory + FK pragma + 迁移)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::test_pool;

    fn spec_json(json: &str) -> String {
        // 经解析器确保测试用的 JSON 合法(与生产写入同门)。
        let spec = crate::scheduler::compute::parse_schedule(json).expect("valid schedule");
        serde_json::to_string(&spec).unwrap()
    }

    async fn seed_project_session(pool: &SqlitePool) -> (String, String) {
        // path 有 UNIQUE 索引 —— 同一测试内 seed 多次须用不同路径。
        let name = format!("sched-test-{}", uuid::Uuid::new_v4().simple());
        let path = format!("/tmp/sched-test-{name}");
        crate::db::create_project(pool, &name, &path, false, None)
            .await
            .expect("create_project");
        let project = crate::db::list_projects(pool, false)
            .await
            .expect("list_projects")
            .into_iter()
            .find(|p| p.name == name)
            .expect("project row");
        let session_id = uuid::Uuid::new_v4().to_string();
        crate::db::create_session(
            pool,
            &session_id,
            &project.id,
            &path,
            "mock-model",
            None,
            None,
            None,
        )
        .await
        .expect("create_session");
        (project.id, session_id)
    }

    async fn insert_sample(
        pool: &SqlitePool,
        project_id: &str,
        session_id: &str,
    ) -> ScheduledTaskRow {
        insert_scheduled_task(
            pool,
            NewScheduledTask {
                project_id: project_id.to_string(),
                target_session_id: Some(session_id.to_string()),
                target_mode: target_modes::FIXED.into(),
                model_id: None,
                name: "早报".into(),
                prompt: "汇总昨日进展".into(),
                schedule_json: spec_json(r#"{"kind":"daily","at":"09:00"}"#),
                enabled: true,
                created_by: "user".into(),
                next_fire_at: 1_000,
                max_runs: None,
                ends_at: None,
            },
        )
        .await
        .expect("insert")
    }

    #[tokio::test]
    async fn insert_list_get_roundtrip() {
        let pool = test_pool().await;
        let (project_id, session_id) = seed_project_session(&pool).await;
        let row = insert_sample(&pool, &project_id, &session_id).await;

        assert!(!row.id.is_empty(), "server-side uuid id");
        assert_eq!(row.project_id, project_id);
        assert_eq!(row.target_session_id.as_deref(), Some(session_id.as_str()));
        assert_eq!(row.target_mode, "fixed", "default mode on plain insert");
        assert_eq!(row.name, "早报");
        assert!(row.enabled);
        assert_eq!(row.created_by, "user");
        assert!(row.last_fired_at.is_none(), "never fired on insert");
        assert_eq!(row.next_fire_at, 1_000);
        assert!(row.created_at > 0, "epoch ms created_at");

        let listed = list_scheduled_tasks(&pool, Some(&project_id))
            .await
            .expect("list by project");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, row.id);
        // 其它 project 过滤为空;全量可见。
        assert!(list_scheduled_tasks(&pool, Some("nope"))
            .await
            .expect("list other project")
            .is_empty());
        assert_eq!(
            list_scheduled_tasks(&pool, None)
                .await
                .expect("list all")
                .len(),
            1
        );
        assert_eq!(
            get_scheduled_task(&pool, &row.id)
                .await
                .expect("get")
                .expect("found")
                .id,
            row.id
        );
    }

    #[tokio::test]
    async fn list_enabled_orders_null_last_fired_first() {
        let pool = test_pool().await;
        let (project_id, session_id) = seed_project_session(&pool).await;
        let never = insert_sample(&pool, &project_id, &session_id).await;
        let fired = insert_scheduled_task(
            &pool,
            NewScheduledTask {
                project_id: project_id.clone(),
                target_session_id: Some(session_id.clone()),
                target_mode: target_modes::FIXED.into(),
                model_id: None,
                name: "已触发过".into(),
                prompt: "p".into(),
                schedule_json: spec_json(r#"{"kind":"interval","every_min":30}"#),
                enabled: true,
                created_by: "user".into(),
                next_fire_at: 2_000,
                max_runs: None,
                ends_at: None,
            },
        )
        .await
        .expect("insert second");
        let disabled = insert_scheduled_task(
            &pool,
            NewScheduledTask {
                project_id: project_id.clone(),
                target_session_id: Some(session_id.clone()),
                target_mode: target_modes::FIXED.into(),
                model_id: None,
                name: "停用".into(),
                prompt: "p".into(),
                schedule_json: spec_json(r#"{"kind":"interval","every_min":60}"#),
                enabled: false,
                created_by: "user".into(),
                next_fire_at: 3_000,
                max_runs: None,
                ends_at: None,
            },
        )
        .await
        .expect("insert third");
        mark_task_fired(&pool, &fired.id, 500, 3_500_000, true, None)
            .await
            .expect("mark fired");

        let enabled = list_enabled_scheduled_tasks(&pool)
            .await
            .expect("list enabled");
        let ids: Vec<&str> = enabled.iter().map(|t| t.id.as_str()).collect();
        // NULLS FIRST:从未触发的在前;停用任务不出现。
        assert_eq!(ids, vec![never.id.as_str(), fired.id.as_str()]);
        assert!(!ids.contains(&disabled.id.as_str()));
    }

    #[tokio::test]
    async fn update_partial_fields_and_reenable_resets_last_fired() {
        let pool = test_pool().await;
        let (project_id, session_id) = seed_project_session(&pool).await;
        let row = insert_sample(&pool, &project_id, &session_id).await;
        mark_task_fired(&pool, &row.id, 123, 4_555, true, None)
            .await
            .expect("mark fired");

        // 部分更新:name 动,prompt/schedule 不动;enabled 不变时
        // last_fired_at 基准保留。
        let upd = update_scheduled_task(
            &pool,
            &row.id,
            UpdateScheduledTask {
                name: Some("晚报".into()),
                ..Default::default()
            },
        )
        .await
        .expect("update")
        .expect("row exists");
        assert_eq!(upd.name, "晚报");
        assert_eq!(upd.prompt, "汇总昨日进展");
        assert_eq!(upd.last_fired_at, Some(123), "no re-enable → keep basis");

        // true→false:停用,基准仍保留(再启用才重置)。
        let upd = update_scheduled_task(
            &pool,
            &row.id,
            UpdateScheduledTask {
                enabled: Some(false),
                ..Default::default()
            },
        )
        .await
        .expect("update")
        .expect("row exists");
        assert!(!upd.enabled);
        assert_eq!(upd.last_fired_at, Some(123));
        // 停用任务存「schedule 下一到期点」展示值(被重算)。
        assert!(upd.next_fire_at > 123);

        // false→true:重启用 → last_fired_at = now(不补停用期存量)。
        let before = now_epoch_ms();
        let upd = update_scheduled_task(
            &pool,
            &row.id,
            UpdateScheduledTask {
                enabled: Some(true),
                ..Default::default()
            },
        )
        .await
        .expect("update")
        .expect("row exists");
        assert!(upd.enabled);
        let reset = upd.last_fired_at.expect("re-enable must set last_fired_at");
        assert!(
            reset >= before,
            "re-enable resets basis to now ({reset} >= {before})"
        );
    }

    #[tokio::test]
    async fn update_missing_row_returns_none() {
        let pool = test_pool().await;
        let result = update_scheduled_task(
            &pool,
            "no-such-id",
            UpdateScheduledTask {
                name: Some("x".into()),
                ..Default::default()
            },
        )
        .await
        .expect("update missing");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn delete_removes_row() {
        let pool = test_pool().await;
        let (project_id, session_id) = seed_project_session(&pool).await;
        let row = insert_sample(&pool, &project_id, &session_id).await;
        assert!(delete_scheduled_task(&pool, &row.id).await.expect("delete"));
        assert!(get_scheduled_task(&pool, &row.id)
            .await
            .expect("get")
            .is_none());
        assert!(!delete_scheduled_task(&pool, &row.id)
            .await
            .expect("delete again"));
    }

    #[tokio::test]
    async fn fk_cascade_on_session_and_project_delete() {
        let pool = test_pool().await;
        let (project_id, session_id) = seed_project_session(&pool).await;
        let row = insert_sample(&pool, &project_id, &session_id).await;

        // 删目标 session → 任务级联删除(AC6;依赖 init_pool 的 FK pragma)。
        crate::db::delete_session(&pool, &session_id)
            .await
            .expect("delete session");
        assert!(
            get_scheduled_task(&pool, &row.id)
                .await
                .expect("get")
                .is_none(),
            "task must cascade-delete with its target session"
        );

        // 删 project → 剩余任务级联删除。
        let (project_id2, session_id2) = seed_project_session(&pool).await;
        let row2 = insert_sample(&pool, &project_id2, &session_id2).await;
        sqlx::query("DELETE FROM projects WHERE id = ?")
            .bind(&project_id2)
            .execute(&pool)
            .await
            .expect("delete project");
        assert!(
            get_scheduled_task(&pool, &row2.id)
                .await
                .expect("get")
                .is_none(),
            "task must cascade-delete with its project"
        );
    }

    #[tokio::test]
    async fn validate_target_session_rejects_missing_and_group_chat() {
        let pool = test_pool().await;
        let (project_id, session_id) = seed_project_session(&pool).await;

        validate_target_session(&pool, &session_id)
            .await
            .expect("chat session valid");
        assert!(
            validate_target_session(&pool, "no-such-session")
                .await
                .is_err(),
            "missing session rejected"
        );

        // 群聊 session 拒绝(AC7)。
        let group_id = uuid::Uuid::new_v4().to_string();
        crate::db::create_session(
            &pool,
            &group_id,
            &project_id,
            "/tmp/sched-test",
            "mock-model",
            None,
            Some("group_chat"),
            None,
        )
        .await
        .expect("create group session");
        let err = validate_target_session(&pool, &group_id)
            .await
            .expect_err("group chat rejected");
        assert!(
            err.contains("群聊"),
            "message should name group chat: {err}"
        );
    }

    // --- F2b:结束条件列 ---

    /// 计数语义:count_fire 才 +1(dedup 跳过路径不计数);完成落账只翻
    /// enabled,其余列保持;幂等(已 disabled 再写 no-op)。
    #[tokio::test]
    async fn mark_fired_counts_and_completion_flips_enabled_only() {
        let pool = test_pool().await;
        let (project_id, session_id) = seed_project_session(&pool).await;
        let row = insert_sample(&pool, &project_id, &session_id).await;

        mark_task_fired(&pool, &row.id, 100, 200, true, None)
            .await
            .expect("fire");
        assert_eq!(
            get_scheduled_task(&pool, &row.id)
                .await
                .unwrap()
                .unwrap()
                .run_count,
            1,
            "count_fire=true increments"
        );
        mark_task_fired(&pool, &row.id, 300, 400, false, None)
            .await
            .expect("dedup-style account");
        assert_eq!(
            get_scheduled_task(&pool, &row.id)
                .await
                .unwrap()
                .unwrap()
                .run_count,
            1,
            "count_fire=false (dedup skip) must not increment"
        );

        let affected = mark_task_completed(&pool, &row.id).await.expect("complete");
        assert_eq!(affected, 1);
        let done = get_scheduled_task(&pool, &row.id).await.unwrap().unwrap();
        assert!(!done.enabled, "completed = disabled");
        assert_eq!(done.run_count, 1, "count preserved");
        assert_eq!(done.last_fired_at, Some(300), "basis preserved");
        assert_eq!(done.next_fire_at, 400, "display value preserved");

        // 重写是列级 no-op(SQLite rows_affected 数 WHERE 命中,仍为 1),
        // 列值不动。
        let affected = mark_task_completed(&pool, &row.id).await.expect("again");
        assert_eq!(affected, 1, "matched row still counts as changed");
        assert_eq!(
            get_scheduled_task(&pool, &row.id)
                .await
                .unwrap()
                .unwrap()
                .run_count,
            1
        );
    }

    /// update 的双层 Option:外层 None 不动、Some(Some) 写入、Some(None)
    /// 清空;重新启用(false→true)时 run_count 清零。
    #[tokio::test]
    async fn update_end_conditions_set_clear_and_reenable_resets_count() {
        let pool = test_pool().await;
        let (project_id, session_id) = seed_project_session(&pool).await;
        let row = insert_scheduled_task(
            &pool,
            NewScheduledTask {
                project_id: project_id.clone(),
                target_session_id: Some(session_id.clone()),
                target_mode: target_modes::FIXED.into(),
                model_id: None,
                name: "限时".into(),
                prompt: "p".into(),
                schedule_json: spec_json(r#"{"kind":"interval","every_min":30}"#),
                enabled: true,
                created_by: "user".into(),
                next_fire_at: 1_000,
                max_runs: Some(3),
                ends_at: Some(9_999_999),
            },
        )
        .await
        .expect("insert with end conditions");
        assert_eq!(row.max_runs, Some(3));
        assert_eq!(row.ends_at, Some(9_999_999));
        assert_eq!(row.run_count, 0, "starts at zero");

        // 外层 None(name-only patch):结束条件不动。
        let upd = update_scheduled_task(
            &pool,
            &row.id,
            UpdateScheduledTask {
                name: Some("改名".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(upd.max_runs, Some(3), "untouched");
        assert_eq!(upd.ends_at, Some(9_999_999), "untouched");

        // Some(Some(5)):写入。
        let upd = update_scheduled_task(
            &pool,
            &row.id,
            UpdateScheduledTask {
                max_runs: Some(Some(5)),
                ends_at: Some(Some(12_345)),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(upd.max_runs, Some(5));
        assert_eq!(upd.ends_at, Some(12_345));

        // Some(None):清空为不限。
        let upd = update_scheduled_task(
            &pool,
            &row.id,
            UpdateScheduledTask {
                max_runs: Some(None),
                ends_at: Some(None),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(upd.max_runs, None, "explicit null clears");
        assert_eq!(upd.ends_at, None, "explicit null clears");

        // 计数两次 → 停用 → 重启用:run_count 清零(F2b D8)。
        mark_task_fired(&pool, &row.id, 100, 200, true, None)
            .await
            .expect("fire 1");
        mark_task_fired(&pool, &row.id, 400, 500, true, None)
            .await
            .expect("fire 2");
        assert_eq!(
            get_scheduled_task(&pool, &row.id)
                .await
                .unwrap()
                .unwrap()
                .run_count,
            2
        );
        update_scheduled_task(
            &pool,
            &row.id,
            UpdateScheduledTask {
                enabled: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
        let upd = update_scheduled_task(
            &pool,
            &row.id,
            UpdateScheduledTask {
                enabled: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert!(upd.enabled);
        assert_eq!(upd.run_count, 0, "re-enable resets the run count");
        assert!(
            upd.last_fired_at.is_some(),
            "re-enable also resets the basis (existing semantics)"
        );
    }

    #[tokio::test]
    async fn created_by_agent_persists_and_filters_by_creator() {
        let pool = test_pool().await;
        let (project_id, session_id) = seed_project_session(&pool).await;
        let agent_row = insert_scheduled_task(
            &pool,
            NewScheduledTask {
                project_id: project_id.clone(),
                target_session_id: Some(session_id.clone()),
                target_mode: target_modes::FIXED.into(),
                model_id: None,
                name: "agent 排的".into(),
                prompt: "每小时检查".into(),
                schedule_json: spec_json(r#"{"kind":"interval","every_min":60}"#),
                enabled: true,
                created_by: "agent".into(),
                next_fire_at: 1_000,
                max_runs: None,
                ends_at: None,
            },
        )
        .await
        .expect("insert agent row");
        assert_eq!(agent_row.created_by, "agent");
        let _user_row = insert_sample(&pool, &project_id, &session_id).await;

        // created_by 过滤正负向(作者面分离:schedule_status 只见 agent 行)。
        let agent_only = list_scheduled_tasks_filtered(&pool, Some(&project_id), Some("agent"))
            .await
            .expect("list agent");
        assert_eq!(agent_only.len(), 1);
        assert_eq!(agent_only[0].id, agent_row.id);
        let all = list_scheduled_tasks(&pool, Some(&project_id))
            .await
            .expect("list all");
        assert_eq!(all.len(), 2, "None 过滤不受影响(UI 路径行为零变化)");
    }

    // --- per_run 档(08-31-sched-per-run-session)---

    /// per_run 行 roundtrip(target NULL + 三新列)+ update 的 target
    /// 三态(缺省不动 / `Some(Some)` 切 fixed 设目标 / `Some(None)` 切
    /// per_run 清空)+ `last_run_session_id` 的 COALESCE 语义 + 删 run
    /// session 不级联删任务(无 FK,AC7)。
    #[tokio::test]
    async fn per_run_roundtrip_update_three_state_target_and_no_run_cascade() {
        let pool = test_pool().await;
        let (project_id, session_id) = seed_project_session(&pool).await;
        let row = insert_scheduled_task(
            &pool,
            NewScheduledTask {
                project_id: project_id.clone(),
                target_session_id: None,
                target_mode: target_modes::PER_RUN.into(),
                model_id: Some("m-1".into()),
                name: "每跑一新".into(),
                prompt: "p".into(),
                schedule_json: spec_json(r#"{"kind":"interval","every_min":30}"#),
                enabled: true,
                created_by: "user".into(),
                next_fire_at: 1_000,
                max_runs: None,
                ends_at: None,
            },
        )
        .await
        .expect("insert per_run");
        assert_eq!(
            row.target_session_id, None,
            "per_run binds no fixed session"
        );
        assert_eq!(row.target_mode, "per_run");
        assert_eq!(row.model_id.as_deref(), Some("m-1"));
        assert_eq!(row.last_run_session_id, None, "never fired");

        // 外层 None(name-only patch):target/mode/model 不动。
        let upd = update_scheduled_task(
            &pool,
            &row.id,
            UpdateScheduledTask {
                name: Some("改名".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(upd.target_session_id, None);
        assert_eq!(upd.target_mode, "per_run");
        assert_eq!(upd.model_id.as_deref(), Some("m-1"));

        // fire 落账带 run session → 记录;None(dedup 路径)保留旧值。
        mark_task_fired(&pool, &row.id, 100, 200, true, Some("run-sid"))
            .await
            .expect("fire with run session");
        mark_task_fired(&pool, &row.id, 300, 400, false, None)
            .await
            .expect("dedup-style accounting");
        let after = get_scheduled_task(&pool, &row.id).await.unwrap().unwrap();
        assert_eq!(
            after.last_run_session_id.as_deref(),
            Some("run-sid"),
            "COALESCE keeps the previous run session on None"
        );

        // Some(Some) + mode fixed:切回固定目标。
        let upd = update_scheduled_task(
            &pool,
            &row.id,
            UpdateScheduledTask {
                target_session_id: Some(Some(session_id.clone())),
                target_mode: Some(target_modes::FIXED.into()),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(upd.target_session_id.as_deref(), Some(session_id.as_str()));
        assert_eq!(upd.target_mode, "fixed");

        // Some(None) + mode per_run:清空固定绑定(同语句内两列一起变,
        // CHECK 不变式在语句级成立)。
        let upd = update_scheduled_task(
            &pool,
            &row.id,
            UpdateScheduledTask {
                target_session_id: Some(None),
                target_mode: Some(target_modes::PER_RUN.into()),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(upd.target_session_id, None);
        assert_eq!(upd.target_mode, "per_run");

        // 删除真实 run session → 任务存活(last_run_session_id 无 FK,
        // 区别于 fixed 档 target 的级联语义,AC7)。
        let run_sid = uuid::Uuid::new_v4().to_string();
        crate::db::create_session(
            &pool,
            &run_sid,
            &project_id,
            "/tmp/sched-test-per-run",
            "mock-model",
            None,
            None,
            None,
        )
        .await
        .expect("create run session");
        mark_task_fired(&pool, &row.id, 500, 600, true, Some(&run_sid))
            .await
            .expect("fire #2");
        crate::db::delete_session(&pool, &run_sid)
            .await
            .expect("delete run session");
        assert!(
            get_scheduled_task(&pool, &row.id).await.unwrap().is_some(),
            "deleting a run session must NOT cascade-delete a per_run task"
        );
    }
}
