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

/// 供调度器与 UI 读取的任务行。`schedule_json` 是原始 JSON 文本
/// (反序列化经 [`crate::scheduler::compute::parse_schedule`],校验
/// 在写入时完成,读取时信任)。
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduledTaskRow {
    pub id: String,
    pub project_id: String,
    pub target_session_id: String,
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
}

/// [`insert_scheduled_task`] 的载荷。`next_fire_at` 由调用方按
/// [`crate::scheduler::compute::next_fire_display`] 计算(展示语义)。
#[derive(Debug, Clone)]
pub struct NewScheduledTask {
    pub project_id: String,
    pub target_session_id: String,
    pub name: String,
    pub prompt: String,
    /// 已过 [`crate::scheduler::compute::parse_schedule`] 校验的 JSON。
    pub schedule_json: String,
    pub enabled: bool,
    pub next_fire_at: i64,
}

/// [`update_scheduled_task`] 的载荷。`None` 字段不动存量;`enabled` 的
/// false→true 跳变触发 `last_fired_at = now`(重启用不补跑)。
#[derive(Debug, Clone, Default)]
pub struct UpdateScheduledTask {
    pub name: Option<String>,
    pub prompt: Option<String>,
    pub schedule_json: Option<String>,
    pub target_session_id: Option<String>,
    pub enabled: Option<bool>,
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
        name: row.try_get("name")?,
        prompt: row.try_get("prompt")?,
        schedule_json: row.try_get("schedule")?,
        enabled: row.try_get::<i64, _>("enabled")? != 0,
        created_by: row.try_get("created_by")?,
        created_at: row.try_get("created_at")?,
        last_fired_at: row.try_get("last_fired_at")?,
        next_fire_at: row.try_get("next_fire_at")?,
    })
}

const SELECT_COLS: &str = "id, project_id, target_session_id, name, prompt, schedule, \
     enabled, created_by, created_at, last_fired_at, next_fire_at";

/// 新建任务。id 服务端生成(uuid);`created_by` 恒 `'user'`(MVP;
/// F2+ agent 复用同表时改由参数区分,prd D5)。
pub async fn insert_scheduled_task(
    pool: &SqlitePool,
    new: NewScheduledTask,
) -> Result<ScheduledTaskRow, sqlx::Error> {
    let id = uuid::Uuid::new_v4().to_string();
    let created_at = now_epoch_ms();
    sqlx::query(
        r#"
 INSERT INTO scheduled_tasks
 (id, project_id, target_session_id, name, prompt, schedule, enabled, created_by, created_at, last_fired_at, next_fire_at)
 VALUES (?, ?, ?, ?, ?, ?, ?, 'user', ?, NULL, ?)
 "#,
    )
    .bind(&id)
    .bind(&new.project_id)
    .bind(&new.target_session_id)
    .bind(&new.name)
    .bind(&new.prompt)
    .bind(&new.schedule_json)
    .bind(new.enabled as i64)
    .bind(created_at)
    .bind(new.next_fire_at)
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
    let rows = match project_id {
        Some(pid) => {
            sqlx::query(&format!(
                "SELECT {SELECT_COLS} FROM scheduled_tasks WHERE project_id = ? ORDER BY created_at ASC, id ASC"
            ))
            .bind(pid)
            .fetch_all(pool)
            .await?
        }
        None => {
            sqlx::query(&format!(
                "SELECT {SELECT_COLS} FROM scheduled_tasks ORDER BY created_at ASC, id ASC"
            ))
            .fetch_all(pool)
            .await?
        }
    };
    rows.iter().map(row_from).collect()
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
    let target_session_id = upd
        .target_session_id
        .unwrap_or(existing.target_session_id.clone());
    let enabled = upd.enabled.unwrap_or(existing.enabled);

    // false→true:用户主动重启用 → 从现在起算,不补跑存量(design §3)。
    let last_fired_at = if !existing.enabled && enabled {
        Some(now_epoch_ms())
    } else {
        existing.last_fired_at
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
 SET name = ?, prompt = ?, schedule = ?, target_session_id = ?, enabled = ?, last_fired_at = ?, next_fire_at = ?
 WHERE id = ?
 "#,
    )
    .bind(&name)
    .bind(&prompt)
    .bind(&schedule_json)
    .bind(&target_session_id)
    .bind(enabled as i64)
    .bind(last_fired_at)
    .bind(next_fire_at)
    .bind(id)
    .execute(pool)
    .await?;
    get_scheduled_task(pool, id).await
}

/// 调度器落账(design §4:fire / dedup 跳过后调用)。`last_fired_at`
/// 恒记**理论到期点 due**(防 interval 相位漂移,design §3);
/// `next_fire_at` 写 `next_fire_display(schedule, due)` 展示值。
/// 触碰 `last_fired_at` 时**不**套 false→true 语义 —— 本函数只被调度器
/// 在任务保持 enabled 的前提下调用。返回 affected 行数(0 = 任务已被删)。
pub async fn mark_task_fired(
    pool: &SqlitePool,
    id: &str,
    last_fired_at: i64,
    next_fire_at: i64,
) -> Result<u64, sqlx::Error> {
    let result =
        sqlx::query("UPDATE scheduled_tasks SET last_fired_at = ?, next_fire_at = ? WHERE id = ?")
            .bind(last_fired_at)
            .bind(next_fire_at)
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
                target_session_id: session_id.to_string(),
                name: "早报".into(),
                prompt: "汇总昨日进展".into(),
                schedule_json: spec_json(r#"{"kind":"daily","at":"09:00"}"#),
                enabled: true,
                next_fire_at: 1_000,
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
        assert_eq!(row.target_session_id, session_id);
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
                target_session_id: session_id.clone(),
                name: "已触发过".into(),
                prompt: "p".into(),
                schedule_json: spec_json(r#"{"kind":"interval","every_min":30}"#),
                enabled: true,
                next_fire_at: 2_000,
            },
        )
        .await
        .expect("insert second");
        let disabled = insert_scheduled_task(
            &pool,
            NewScheduledTask {
                project_id: project_id.clone(),
                target_session_id: session_id.clone(),
                name: "停用".into(),
                prompt: "p".into(),
                schedule_json: spec_json(r#"{"kind":"interval","every_min":60}"#),
                enabled: false,
                next_fire_at: 3_000,
            },
        )
        .await
        .expect("insert third");
        mark_task_fired(&pool, &fired.id, 500, 3_500_000)
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
        mark_task_fired(&pool, &row.id, 123, 4_555)
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
}
