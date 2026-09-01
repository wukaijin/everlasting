//! Scheduled-task family tools — LLM-facing surface of the F2 daemon
//! scheduler (`08-29-schedule-task-tool`; ROADMAP F1/F2 follow-up
//! "LLM detached dispatch").
//!
//! Three plain-dispatch tools (no `chat_loop` interception — creation
//! is a fast DB write, not a blocking interaction):
//!
//! - `schedule_task` — create a scheduled task (`created_by='agent'`).
//!   At fire time the existing scheduler tick injects the prompt into
//!   the target session's queue (unified chat_inner entry, F1-C); the
//!   origin chain / metadata envelope / footnote are identical to
//!   user-created tasks.
//! - `schedule_status` — list THIS project's agent-created tasks.
//! - `schedule_cancel` — hard-delete an agent-created task by id.
//!
//! # Author-plane separation (作者面分离)
//!
//! Everything this family writes or deletes carries / requires
//! `created_by = 'agent'`. User-created tasks are invisible here and
//! remain manageable only via the Settings UI (which cannot create
//! `'agent'` rows). The cap (D3) and kill-switch create-gate (R5)
//! likewise live on the tool side only — the UI/IPC path
//! (`create_scheduled_task_inner`) is untouched.
//!
//! # Layering
//!
//! Thin wrapper over the shared pool-level core
//! [`crate::commands::scheduled_tasks::create_scheduled_task_in_pool`]
//! (Q0 单源:validation matrix — name/prompt non-empty, end
//! conditions, `parse_schedule` canonicalization, target-session
//! checks, dedicated-session creation — is NOT duplicated here).
//! The pool-level core exists because [`crate::tools::ToolContext`]
//! carries `db` + `project_id` but no `AppState` (D2).
//!
//! # Permission
//!
//! **Silent Allow** (Tier 5 via `ToolKind::Other` default — no
//! `classify_tool` arm): creation has zero immediate side effects;
//! the real execution happens at fire time behind the full
//! mode/permission chain. Compensating controls (prd Q2/Q3 定案):
//! per-project active-task cap, worker/group-chat isolation
//! (`STRUCTURALLY_DISABLED` + `group_chat_tool_defs` whitelist),
//! kill-switch create gate, Settings-UI visibility (`created_by`
//! badge), and the standard `ToolAllowed` audit rows.

use chrono::TimeZone;
use serde_json::json;

use crate::commands::scheduled_tasks::create_scheduled_task_in_pool;
use crate::db::config::get_config_value;
use crate::db::scheduled_tasks as st;
use crate::llm::types::ToolDef;
use crate::scheduler::compute::format_local_hhmm;
use crate::scheduler::SCHEDULED_TASKS_ENABLED_KEY;
use crate::tools::ToolContext;

/// Per-project cap on ACTIVE (enabled=1) agent-created tasks (prd Q3
/// 定案:≤ 20)。The sole compensating control for silent-Allow
/// creation — every fire of a task is a real LLM turn (tokens cost
/// money), so an unbounded author surface is not acceptable. Soft
/// limit by design: the COUNT→INSERT check has a TOCTOU window
/// (two concurrent agent turns can transiently exceed by 1); that is
/// accepted — do NOT "fix" with atomic SQL (design D3).
pub const MAX_ACTIVE_AGENT_TASKS: i64 = 20;

// ---------------------------------------------------------------------------
// schedule_task — create
// ---------------------------------------------------------------------------

/// The `schedule_task` tool definition. Appended to `builtin_tools()`
/// (family order: task → status → cancel; append-only = provider
/// prefix cache contract).
pub fn definition() -> ToolDef {
    ToolDef {
        name: "schedule_task".to_string(),
        description: Some(
            "Schedule a recurring / future agent task. At each due time the daemon \
             automatically sends `prompt` to the target session and the agent runs it \
             (like the user typing it). Use for \"每天 9 点…\", \"每小时检查…直到…\", \
             \"明早提醒我…\" requests.\n\n\
             `schedule` is a JSON object, exactly one of:\n\
             - {\"kind\":\"daily\",\"at\":\"HH:MM\"}\n\
             - {\"kind\":\"hourly\",\"minute\":0-59}\n\
             - {\"kind\":\"weekly\",\"weekday\":\"mon|tue|wed|thu|fri|sat|sun\",\"at\":\"HH:MM\"}\n\
             - {\"kind\":\"weekdays\",\"at\":\"HH:MM\"} (Mon–Fri)\n\
             - {\"kind\":\"monthly\",\"day\":1-31,\"at\":\"HH:MM\"} (short months without \
             that day skip it)\n\
             - {\"kind\":\"interval\",\"every_min\":N}\n\n\
             Optional end conditions: `max_runs` (integer ≥1, auto-completes after N \
             fires) and/or `end_date` (\"YYYY-MM-DD\", inclusive). For one-shot reminders \
             prefer max_runs=1 + in_current_session=true.\n\n\
             By default the task gets its own dedicated session (titled by `name`); set \
             `in_current_session=true` to fire into THIS session (right for reminders \
             that should appear in this conversation). All times are local."
                .to_string(),
        ),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Short task name (shown in the settings task list; also the dedicated session title). Non-empty."
                },
                "prompt": {
                    "type": "string",
                    "description": "What the agent should do at each fire. Written as an instruction to yourself. Non-empty."
                },
                "schedule": {
                    "type": "object",
                    "description": "One of the six schedule shapes, e.g. {\"kind\":\"daily\",\"at\":\"09:00\"} or {\"kind\":\"interval\",\"every_min\":60}. See tool description."
                },
                "max_runs": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optional: auto-complete after this many fires."
                },
                "end_date": {
                    "type": "string",
                    "description": "Optional: \"YYYY-MM-DD\" local date; dues on that day still fire, after it the task completes."
                },
                "in_current_session": {
                    "type": "boolean",
                    "default": false,
                    "description": "true = fire into the current session instead of a new dedicated one."
                }
            },
            "required": ["name", "prompt", "schedule"]
        }),
    }
}

/// LLM input for `schedule_task` (snake_case, shared-struct exemption
/// — same convention as `request_mode_change`).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ScheduleTaskInput {
    pub name: String,
    pub prompt: String,
    pub schedule: serde_json::Value,
    #[serde(default)]
    pub max_runs: Option<i64>,
    #[serde(default)]
    pub end_date: Option<String>,
    #[serde(default)]
    pub in_current_session: bool,
}

/// Create-path gates that exist ONLY on the agent surface, then the
/// shared pool-level core. Order: kill switch → cap → `_in_pool`
/// validation matrix (cheap checks first).
pub async fn execute(
    input: &serde_json::Value,
    ctx: &ToolContext,
    session_id: Option<&str>,
) -> (String, bool) {
    let parsed: ScheduleTaskInput = match serde_json::from_value(input.clone()) {
        Ok(p) => p,
        Err(e) => return (format!("schedule_task: invalid input: {e}"), true),
    };

    // R5 kill switch (create side only). Fail-open, same literal rule
    // as the tick gate — shared const prevents drift.
    match get_config_value(&ctx.db, SCHEDULED_TASKS_ENABLED_KEY).await {
        Ok(Some(v)) if v == "false" => {
            return (
                "创建失败:定时任务功能已被用户关闭(scheduled_tasks_enabled=false),\
                 任务不会触发。请告知用户,不要重试。"
                    .to_string(),
                true,
            );
        }
        _ => {}
    }

    // R4 cap — agent rows only (user path has no cap by design).
    let active = st::count_enabled_by_creator(&ctx.db, &ctx.project_id, "agent")
        .await
        .unwrap_or(0);
    if active >= MAX_ACTIVE_AGENT_TASKS {
        return (
            format!(
                "创建失败:本项目 agent 创建的活跃定时任务已达上限 \
                 {MAX_ACTIVE_AGENT_TASKS}。请先用 schedule_cancel 取消不再需要的任务,\
                 或请用户在设置中管理。"
            ),
            true,
        );
    }

    // schedule must serialize to the canonical string the core
    // validates via parse_schedule (reject non-object early with a
    // clearer message than "serialization failed").
    if !parsed.schedule.is_object() {
        return (
            "schedule 必须是 JSON 对象,例如 {\"kind\":\"daily\",\"at\":\"09:00\"}".to_string(),
            true,
        );
    }
    let schedule_str = serde_json::to_string(&parsed.schedule).unwrap_or_default();

    // end_date: LLM speaks wall-clock dates, the core speaks epoch ms
    // (F2b semantics: inclusive — dues on that day still fire).
    let ends_at = match parsed.end_date.as_deref() {
        Some(d) => match parse_end_of_local_day(d) {
            Some(ms) => Some(ms),
            None => {
                return (
                    format!("end_date 必须是 YYYY-MM-DD 格式的合法日期,得到 {d:?}"),
                    true,
                );
            }
        },
        None => None,
    };

    // R7 target: default = dedicated session (the core's None branch
    // creates + titles it); in_current_session=true reuses THIS chat
    // session (validate_target_session passes — it is a chat session
    // of this very project; group chats never see this tool).
    let target_session_id = if parsed.in_current_session {
        match session_id {
            Some(sid) => Some(sid.to_string()),
            None => {
                return (
                    "in_current_session=true 但当前上下文没有 session id(内部错误)".to_string(),
                    true,
                );
            }
        }
    } else {
        None
    };

    match create_scheduled_task_in_pool(
        &ctx.db,
        ctx.project_id.clone(),
        target_session_id.clone(),
        // agent 路径恒 fixed/专用语义:per_run 是用户 UI 面能力
        // (08-31-sched-per-run-session AC10),tool schema 不暴露。
        None,
        parsed.name,
        parsed.prompt,
        schedule_str,
        None,
        "agent".to_string(),
        parsed.max_runs,
        ends_at,
        // 模型指定是用户 UI 面的能力:agent 建的专用 session 恒绑
        // 全局默认(与 tool 不暴露 model 参数一致)。
        None,
    )
    .await
    {
        Ok(row) => {
            let target = target_session_id
                .unwrap_or_else(|| row.target_session_id.clone().unwrap_or_default());
            (
                json!({
                    "ok": true,
                    "task_id": row.id,
                    "name": row.name,
                    "schedule": row.schedule,
                    "enabled": row.enabled,
                    "run_count": row.run_count,
                    "max_runs": row.max_runs,
                    "ends_at_ms": row.ends_at,
                    "target_session_id": target,
                    "next_fire_at_ms": row.next_fire_at,
                    "next_fire_local": format_local_hhmm(row.next_fire_at),
                    "note": "任务已创建。到期会自动触发;用户也可在设置中查看/暂停/删除。"
                })
                .to_string(),
                false,
            )
        }
        Err(e) => (format!("创建失败:{}", e.message), true),
    }
}

/// `YYYY-MM-DD` → 该日本地时刻 23:59:59.999 的 epoch ms(F2b「结束日
/// 当天仍触发」语义,`_in_pool` 校验 `> now`)。DST 处理镜像
/// `scheduler::compute::local_at`(春令时空缺取 None、秋令时取较早)。
fn parse_end_of_local_day(s: &str) -> Option<i64> {
    let date = chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()?;
    let naive = date.and_hms_milli_opt(23, 59, 59, 999)?;
    match chrono::Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(dt) => Some(dt.timestamp_millis()),
        chrono::LocalResult::Ambiguous(earliest, _) => Some(earliest.timestamp_millis()),
        chrono::LocalResult::None => None,
    }
}

// ---------------------------------------------------------------------------
// schedule_status — list this project's agent tasks
// ---------------------------------------------------------------------------

pub fn status_definition() -> ToolDef {
    ToolDef {
        name: "schedule_status".to_string(),
        description: Some(
            "List the scheduled tasks YOU (the agent) created in the current project — \
             id, name, schedule, enabled state, next fire time, run count. Use before \
             creating (avoid duplicates) and before schedule_cancel (get the task_id). \
             Tasks the user created in the settings UI are NOT shown here."
                .to_string(),
        ),
        input_schema: json!({
            "type": "object",
            "properties": {}
        }),
    }
}

/// No input fields; empty-object schema. Read-only, always available
/// (kill switch only gates fires + create — management stays usable).
pub async fn status_execute(ctx: &ToolContext) -> (String, bool) {
    match st::list_scheduled_tasks_filtered(&ctx.db, Some(&ctx.project_id), Some("agent")).await {
        Ok(rows) if rows.is_empty() => ("当前项目还没有你创建的定时任务。".to_string(), false),
        Ok(rows) => {
            let mut out = format!("共 {} 个你创建的定时任务:\n", rows.len());
            for r in rows {
                let ends = r
                    .ends_at
                    .map(|ms| format!(" 结束:{}", format_local_hhmm(ms)))
                    .unwrap_or_default();
                let runs = match r.max_runs {
                    Some(m) => format!("{}/{}", r.run_count, m),
                    None => format!("{}", r.run_count),
                };
                out.push_str(&format!(
                    "- {} 「{}」 {} [{}] 下次:{} 已跑:{}{}\n  id={} target={}\n",
                    if r.enabled { "启用" } else { "已停用" },
                    r.name,
                    r.schedule_json,
                    if r.enabled { "on" } else { "off" },
                    format_local_hhmm(r.next_fire_at),
                    runs,
                    ends,
                    r.id,
                    r.target_session_id.as_deref().unwrap_or("(per_run)"),
                ));
            }
            (out, false)
        }
        Err(e) => (format!("查询失败:{e}"), true),
    }
}

// ---------------------------------------------------------------------------
// schedule_cancel — hard-delete an agent task by id
// ---------------------------------------------------------------------------

pub fn cancel_definition() -> ToolDef {
    ToolDef {
        name: "schedule_cancel".to_string(),
        description: Some(
            "Cancel (permanently delete) a scheduled task that YOU created. Get the \
             task_id from schedule_status first. Deleting a user-created task is \
             rejected — tell the user to manage those in the settings UI."
                .to_string(),
        ),
        input_schema: json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "The id of the task to cancel (from schedule_status)."
                }
            },
            "required": ["task_id"]
        }),
    }
}

/// Ownership guard: only `created_by='agent'` rows are deletable
/// (AC7). Missing id = idempotent success (mirrors the UI delete
/// contract: already-gone is not an error).
pub async fn cancel_execute(input: &serde_json::Value, ctx: &ToolContext) -> (String, bool) {
    let task_id = match input.get("task_id").and_then(|v| v.as_str()) {
        Some(id) if !id.trim().is_empty() => id.trim().to_string(),
        _ => return ("task_id 必填".to_string(), true),
    };

    match st::get_scheduled_task(&ctx.db, &task_id).await {
        Ok(Some(row)) => {
            if row.created_by != "agent" {
                return (
                    format!(
                        "取消失败:任务「{}」是用户在设置中创建的,agent 不能删除\
                         (可请用户自行管理)。",
                        row.name
                    ),
                    true,
                );
            }
            match st::delete_scheduled_task(&ctx.db, &task_id).await {
                Ok(true) => (
                    json!({"ok": true, "cancelled": task_id, "name": row.name}).to_string(),
                    false,
                ),
                Ok(false) => (
                    json!({"ok": true, "cancelled": task_id, "note": "已被删除(幂等)"}).to_string(),
                    false,
                ),
                Err(e) => (format!("取消失败:{e}"), true),
            }
        }
        Ok(None) => (
            json!({"ok": true, "cancelled": task_id, "note": "任务不存在(可能已删除,幂等成功)"})
                .to_string(),
            false,
        ),
        Err(e) => (format!("取消失败:{e}"), true),
    }
}

// ---------------------------------------------------------------------------
// Tests (AC2 / AC3 / AC5 / AC6 / AC7)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::scheduler::compute::parse_schedule;

    async fn make_ctx() -> (ToolContext, sqlx::SqlitePool) {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("in-memory connect");
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .expect("FK pragma");
        crate::db::migrations::run_migrations(&pool)
            .await
            .expect("migrations");
        let ctx = ToolContext {
            tool_use_id: None,
            escalation: Default::default(),
            worktree_path: std::path::PathBuf::from("/repo/proj"),
            cwd: std::path::PathBuf::from("/repo/proj"),
            checklist: crate::tools::update_checklist::new_handle(),
            background_shells: crate::background_shell::default_registry(),
            db: pool.clone(),
            project_id: String::new(), // caller overwrites per test
            data_dir: std::path::PathBuf::from("/repo"),
            workflow_name: None,
            mode: crate::db::Mode::Edit,
        };
        (ctx, pool)
    }

    async fn seed_project_session(pool: &sqlx::SqlitePool) -> (String, String, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().to_path_buf();
        // `create_project` 生成 id(第二参是 name),必须用返回值。
        let project = db::create_project(pool, "test-proj", path.to_str().unwrap(), false, None)
            .await
            .expect("create_project");
        let project_id = project.id;
        let session_id = format!("sess-{}", uuid::Uuid::new_v4());
        db::create_session(
            pool,
            &session_id,
            &project_id,
            path.to_str().unwrap(),
            "mock-model",
            None,
            None,
            None,
        )
        .await
        .expect("create_session");
        // Keep the tempdir alive for the db path lifetime is not needed
        // (in-memory pool; path string already copied).
        (project_id, session_id, path)
    }

    fn ctx_for(ctx: &ToolContext, project_id: &str) -> ToolContext {
        let mut c = ctx.clone();
        c.project_id = project_id.to_string();
        c
    }

    fn create_input(name: &str, schedule: &str) -> serde_json::Value {
        json!({
            "name": name,
            "prompt": "做一轮检查",
            "schedule": serde_json::from_str::<serde_json::Value>(schedule).unwrap(),
        })
    }

    async fn insert_agent_task(
        pool: &sqlx::SqlitePool,
        project_id: &str,
        session_id: &str,
        name: &str,
    ) -> st::ScheduledTaskRow {
        st::insert_scheduled_task(
            pool,
            st::NewScheduledTask {
                project_id: project_id.to_string(),
                target_session_id: Some(session_id.to_string()),
                target_mode: st::target_modes::FIXED.into(),
                model_id: None,
                name: name.to_string(),
                prompt: "p".into(),
                schedule_json: r#"{"kind":"daily","at":"09:00"}"#.into(),
                enabled: true,
                created_by: "agent".into(),
                next_fire_at: 1_000,
                max_runs: None,
                ends_at: None,
            },
        )
        .await
        .expect("insert agent task")
    }

    // ---- AC2: validation short-circuits, zero DB writes ----

    #[tokio::test]
    async fn ac2_missing_schedule_object_short_circuits() {
        let (ctx, _pool) = make_ctx().await;
        let (out, is_err) = execute(
            &json!({"name":"x","prompt":"y","schedule":"daily"}),
            &ctx,
            None,
        )
        .await;
        assert!(is_err);
        assert!(out.contains("JSON 对象"));
    }

    #[tokio::test]
    async fn ac2_bad_schedule_kind_surfaces_parse_error() {
        let (ctx, pool) = make_ctx().await;
        let (project_id, session_id, _) = seed_project_session(&pool).await;
        let ctx = ctx_for(&ctx, &project_id);
        let (out, is_err) = execute(
            &create_input("x", r#"{"kind":"annually","at":"09:00"}"#),
            &ctx,
            Some(&session_id),
        )
        .await;
        assert!(is_err);
        assert!(
            out.contains("创建失败"),
            "parse_schedule error surfaced: {out}"
        );
        let n: i64 = st::count_enabled_by_creator(&pool, &project_id, "agent")
            .await
            .unwrap();
        assert_eq!(n, 0, "zero writes on validation failure");
    }

    #[tokio::test]
    async fn ac2_empty_name_rejected_by_core_matrix() {
        let (ctx, pool) = make_ctx().await;
        let (project_id, session_id, _) = seed_project_session(&pool).await;
        let ctx = ctx_for(&ctx, &project_id);
        let mut input = create_input("x", r#"{"kind":"interval","every_min":30}"#);
        input["name"] = json!("   ");
        let (out, is_err) = execute(&input, &ctx, Some(&session_id)).await;
        assert!(is_err);
        assert!(out.contains("创建失败"));
        assert!(
            out.contains("名称") || out.contains("名字") || out.contains("name"),
            "unexpected message: {out}"
        );
    }

    #[tokio::test]
    async fn ac2_max_runs_zero_rejected() {
        let (ctx, pool) = make_ctx().await;
        let (project_id, session_id, _) = seed_project_session(&pool).await;
        let ctx = ctx_for(&ctx, &project_id);
        let mut input = create_input("x", r#"{"kind":"interval","every_min":30}"#);
        input["max_runs"] = json!(0);
        let (out, is_err) = execute(&input, &ctx, Some(&session_id)).await;
        assert!(is_err);
        assert!(out.contains("次数上限"), "unexpected: {out}");
    }

    #[tokio::test]
    async fn ac2_bad_end_date_rejected() {
        let (ctx, pool) = make_ctx().await;
        let (project_id, session_id, _) = seed_project_session(&pool).await;
        let ctx = ctx_for(&ctx, &project_id);
        let mut input = create_input("x", r#"{"kind":"interval","every_min":30}"#);
        input["end_date"] = json!("2026-13-40");
        let (out, is_err) = execute(&input, &ctx, Some(&session_id)).await;
        assert!(is_err);
        assert!(out.contains("YYYY-MM-DD"), "unexpected: {out}");
    }

    #[test]
    fn parse_end_of_local_day_inclusive_semantics() {
        let ms = parse_end_of_local_day("2026-08-29").expect("valid date");
        let formatted = format_local_hhmm(ms);
        assert!(formatted.ends_with("23:59"), "end of day, got {formatted}");
        assert!(parse_end_of_local_day("not-a-date").is_none());
        assert!(
            parse_end_of_local_day("2026-02-30").is_none(),
            "invalid day"
        );
    }

    // ---- AC3 + AC1: create paths ----

    #[tokio::test]
    async fn ac3_in_current_session_targets_caller_session() {
        let (ctx, pool) = make_ctx().await;
        let (project_id, session_id, _) = seed_project_session(&pool).await;
        let ctx = ctx_for(&ctx, &project_id);
        let mut input = create_input("当前对话提醒", r#"{"kind":"interval","every_min":60}"#);
        input["in_current_session"] = json!(true);
        input["max_runs"] = json!(1);
        let (out, is_err) = execute(&input, &ctx, Some(&session_id)).await;
        assert!(!is_err, "out: {out}");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["target_session_id"], json!(session_id));
        assert_eq!(v["max_runs"], json!(1));
    }

    #[tokio::test]
    async fn ac1_default_creates_dedicated_session_with_agent_marker() {
        let (ctx, pool) = make_ctx().await;
        let (project_id, _caller_session, _) = seed_project_session(&pool).await;
        let ctx = ctx_for(&ctx, &project_id);
        let (out, is_err) = execute(
            &create_input("每小时巡检", r#"{"kind":"hourly","minute":0}"#),
            &ctx,
            None,
        )
        .await;
        assert!(!is_err, "out: {out}");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let task_id = v["task_id"].as_str().unwrap().to_string();
        let row = st::get_scheduled_task(&pool, &task_id)
            .await
            .unwrap()
            .expect("row exists");
        assert_eq!(row.created_by, "agent", "author marker");
        assert!(
            row.target_session_id
                .as_deref()
                .is_some_and(|t| !t.is_empty()),
            "dedicated session created"
        );
        // The dedicated session exists, belongs to the project, and is
        // titled by the task name.
        let title: (Option<String>,) = sqlx::query_as("SELECT title FROM sessions WHERE id = ?")
            .bind(row.target_session_id.as_deref().expect("target"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(title.0.as_deref(), Some("每小时巡检"));
        let schedule_parsed = parse_schedule(&row.schedule_json);
        assert!(schedule_parsed.is_ok(), "canonical schedule persisted");
    }

    // ---- AC5: kill switch gates create only ----

    #[tokio::test]
    async fn ac5_kill_switch_blocks_create_but_not_status_cancel() {
        let (ctx, pool) = make_ctx().await;
        let (project_id, session_id, _) = seed_project_session(&pool).await;
        let ctx = ctx_for(&ctx, &project_id);
        crate::db::config::set_config_value(&pool, SCHEDULED_TASKS_ENABLED_KEY, "false")
            .await
            .unwrap();

        let (out, is_err) = execute(
            &create_input("x", r#"{"kind":"interval","every_min":30}"#),
            &ctx,
            Some(&session_id),
        )
        .await;
        assert!(is_err);
        assert!(
            out.contains("scheduled_tasks_enabled"),
            "message names the switch: {out}"
        );
        let n = st::count_enabled_by_creator(&pool, &project_id, "agent")
            .await
            .unwrap();
        assert_eq!(n, 0, "no row written while disabled");

        // list/cancel remain usable (R5: management actions always work).
        let (status, status_err) = status_execute(&ctx).await;
        assert!(!status_err);
        assert!(
            status.contains("还没有"),
            "empty list ok even when disabled"
        );
        let (cancel, cancel_err) = cancel_execute(&json!({"task_id": "no-such-id"}), &ctx).await;
        assert!(!cancel_err, "idempotent cancel unaffected: {cancel}");
    }

    // ---- AC6: cap + self-heal path ----

    #[tokio::test]
    async fn ac6_cap_blocks_at_limit_and_cancel_self_heals() {
        let (ctx, pool) = make_ctx().await;
        let (project_id, session_id, _) = seed_project_session(&pool).await;
        let ctx = ctx_for(&ctx, &project_id);
        for i in 0..MAX_ACTIVE_AGENT_TASKS {
            insert_agent_task(&pool, &project_id, &session_id, &format!("t{i}")).await;
        }

        let (out, is_err) = execute(
            &create_input("第21个", r#"{"kind":"interval","every_min":30}"#),
            &ctx,
            Some(&session_id),
        )
        .await;
        assert!(is_err);
        assert!(out.contains("上限"), "cap message: {out}");
        assert!(out.contains("schedule_cancel"), "self-heal guidance: {out}");

        // Self-heal: cancel one → create succeeds again.
        let rows = st::list_scheduled_tasks_filtered(&pool, Some(&project_id), Some("agent"))
            .await
            .unwrap();
        let (cancel, cerr) = cancel_execute(&json!({"task_id": rows[0].id}), &ctx).await;
        assert!(!cerr, "{cancel}");
        let (out, is_err) = execute(
            &create_input("第21个", r#"{"kind":"interval","every_min":30}"#),
            &ctx,
            Some(&session_id),
        )
        .await;
        assert!(!is_err, "after cancel a slot frees up: {out}");

        // User path is uncapped by construction (cap lives in the tool,
        // not the pool core): with MAX agent rows active again, a direct
        // user-row creation still succeeds.
        st::insert_scheduled_task(
            &pool,
            st::NewScheduledTask {
                project_id: project_id.clone(),
                target_session_id: Some(session_id.clone()),
                target_mode: st::target_modes::FIXED.into(),
                model_id: None,
                name: "用户的不受限".into(),
                prompt: "p".into(),
                schedule_json: r#"{"kind":"daily","at":"09:00"}"#.into(),
                enabled: true,
                created_by: "user".into(),
                next_fire_at: 1_000,
                max_runs: None,
                ends_at: None,
            },
        )
        .await
        .expect("user path uncapped with agent rows at the cap");
        let agent_active = st::count_enabled_by_creator(&pool, &project_id, "agent")
            .await
            .unwrap();
        assert_eq!(agent_active, MAX_ACTIVE_AGENT_TASKS, "cap intact");
    }

    // ---- AC7: cancel ownership / idempotency ----

    #[tokio::test]
    async fn ac7_cancel_rejects_user_rows_and_is_idempotent() {
        let (ctx, pool) = make_ctx().await;
        let (project_id, session_id, _) = seed_project_session(&pool).await;
        let ctx = ctx_for(&ctx, &project_id);

        // User-created row → rejected.
        let user_row = st::insert_scheduled_task(
            &pool,
            st::NewScheduledTask {
                project_id: project_id.clone(),
                target_session_id: Some(session_id.clone()),
                target_mode: st::target_modes::FIXED.into(),
                model_id: None,
                name: "用户的".into(),
                prompt: "p".into(),
                schedule_json: r#"{"kind":"daily","at":"09:00"}"#.into(),
                enabled: true,
                created_by: "user".into(),
                next_fire_at: 1_000,
                max_runs: None,
                ends_at: None,
            },
        )
        .await
        .unwrap();
        let (out, is_err) = cancel_execute(&json!({"task_id": user_row.id}), &ctx).await;
        assert!(is_err, "user rows are not cancellable by the agent");
        assert!(out.contains("设置"));
        assert!(
            st::get_scheduled_task(&pool, &user_row.id)
                .await
                .unwrap()
                .is_some(),
            "user row untouched"
        );

        // Agent row → deleted; second call idempotent-success.
        let agent_row = insert_agent_task(&pool, &project_id, &session_id, "agent的").await;
        let (out, is_err) = cancel_execute(&json!({"task_id": agent_row.id}), &ctx).await;
        assert!(!is_err, "{out}");
        assert!(
            st::get_scheduled_task(&pool, &agent_row.id)
                .await
                .unwrap()
                .is_none(),
            "hard-deleted"
        );
        let (out2, is_err2) = cancel_execute(&json!({"task_id": agent_row.id}), &ctx).await;
        assert!(!is_err2, "idempotent on missing id: {out2}");
    }

    // ---- status: author-plane visibility ----

    #[tokio::test]
    async fn status_lists_agent_rows_only() {
        let (ctx, pool) = make_ctx().await;
        let (project_id, session_id, _) = seed_project_session(&pool).await;
        let ctx = ctx_for(&ctx, &project_id);
        insert_agent_task(&pool, &project_id, &session_id, "agent的").await;
        st::insert_scheduled_task(
            &pool,
            st::NewScheduledTask {
                project_id: project_id.clone(),
                target_session_id: Some(session_id.clone()),
                target_mode: st::target_modes::FIXED.into(),
                model_id: None,
                name: "用户的".into(),
                prompt: "p".into(),
                schedule_json: r#"{"kind":"daily","at":"09:00"}"#.into(),
                enabled: true,
                created_by: "user".into(),
                next_fire_at: 1_000,
                max_runs: None,
                ends_at: None,
            },
        )
        .await
        .unwrap();

        let (out, is_err) = status_execute(&ctx).await;
        assert!(!is_err);
        assert!(out.contains("agent的"));
        assert!(!out.contains("用户的"), "user rows invisible (作者面分离)");
        assert!(out.contains("1 个"), "count header");
    }

    // ---- definitions sanity ----

    #[test]
    fn definitions_shapes() {
        let d = definition();
        assert_eq!(d.name, "schedule_task");
        let s = status_definition();
        assert_eq!(s.name, "schedule_status");
        let c = cancel_definition();
        assert_eq!(c.name, "schedule_cancel");
        for def in [d, s, c] {
            assert_eq!(def.input_schema["type"], "object");
        }
    }
}
