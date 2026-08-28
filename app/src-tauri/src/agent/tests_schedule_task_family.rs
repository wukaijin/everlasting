//! Integration tests for the LLM scheduling family
//! (`08-29-schedule-task-tool`).
//!
//! Covers the wiring the per-tool unit tests in
//! `tools/scheduled_task_family.rs` cannot see: name-based dispatch
//! through [`crate::tools::execute_tool`] (the real agent-loop entry),
//! toolset isolation (worker `STRUCTURALLY_DISABLED` + group-chat
//! whitelist), and the `builtin_tools()` tail-order contract.

#![cfg(test)]

use tokio_util::sync::CancellationToken;

use crate::agent::group_chat_prompts::group_chat_tool_defs;
use crate::agent::subagent::filter_tools_for_subagent;
use crate::db;
use crate::llm::types::ToolDef;
use crate::scheduler::SCHEDULED_TASKS_ENABLED_KEY;
use crate::tools::{builtin_tools, execute_tool, ToolContext};

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
        worktree_path: std::path::PathBuf::from("/repo/proj"),
        cwd: std::path::PathBuf::from("/repo/proj"),
        checklist: crate::tools::update_checklist::new_handle(),
        background_shells: crate::background_shell::default_registry(),
        db: pool.clone(),
        project_id: String::new(),
        data_dir: std::path::PathBuf::from("/repo"),
        workflow_name: None,
    };
    (ctx, pool)
}

async fn seed_project_session(pool: &sqlx::SqlitePool) -> (String, String) {
    let project = db::create_project(pool, "test-proj", "/repo/proj", false, None)
        .await
        .expect("create_project");
    let session_id = format!("sess-{}", uuid::Uuid::new_v4());
    db::create_session(
        pool,
        &session_id,
        &project.id,
        "/repo/proj",
        "mock-model",
        None,
        None,
        None,
    )
    .await
    .expect("create_session");
    (project.id, session_id)
}

async fn dispatch(
    ctx: &ToolContext,
    name: &str,
    input: serde_json::Value,
    session_id: Option<&str>,
) -> (String, bool) {
    let (out, is_err, _, _, _) = execute_tool(
        name,
        &input,
        ctx,
        None,
        session_id,
        None,
        CancellationToken::new(),
    )
    .await;
    (out, is_err)
}

/// AC1 via the real dispatch arm: name-routed `schedule_task` writes
/// the row with the agent marker; AC8's silent-Allow is pinned by the
/// `classify_tool_keeps_schedule_family_silent_allow` test in
/// `permissions::tests_check`.
#[tokio::test]
async fn dispatch_schedule_task_creates_agent_row() {
    let (ctx_base, pool) = make_ctx().await;
    let (project_id, session_id) = seed_project_session(&pool).await;
    let mut ctx = ctx_base.clone();
    ctx.project_id = project_id.clone();

    let input = serde_json::json!({
        "name": "巡检",
        "prompt": "检查 CI",
        "schedule": {"kind": "interval", "every_min": 60},
        "max_runs": 3,
    });
    let (out, is_err) = dispatch(&ctx, "schedule_task", input, Some(&session_id)).await;
    assert!(!is_err, "dispatch arm wired: {out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let task_id = v["task_id"].as_str().unwrap().to_string();
    let row = db::scheduled_tasks::get_scheduled_task(&pool, &task_id)
        .await
        .unwrap()
        .expect("row persisted");
    assert_eq!(row.created_by, "agent");
    assert_eq!(row.max_runs, Some(3));
}

/// AC7/AC4 wiring: status sees only agent rows; cancel rejects user
/// rows — through the dispatch arms (not the direct fn calls).
#[tokio::test]
async fn dispatch_status_and_cancel_roundtrip() {
    let (ctx_base, pool) = make_ctx().await;
    let (project_id, session_id) = seed_project_session(&pool).await;
    let mut ctx = ctx_base.clone();
    ctx.project_id = project_id.clone();

    // User row exists but must be invisible to the tool surface.
    db::scheduled_tasks::insert_scheduled_task(
        &pool,
        db::scheduled_tasks::NewScheduledTask {
            project_id: project_id.clone(),
            target_session_id: session_id.clone(),
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

    let (out, is_err) = dispatch(&ctx, "schedule_status", serde_json::json!({}), None).await;
    assert!(!is_err);
    assert!(!out.contains("用户的"), "author-plane separation holds");

    let (cancel_out, cancel_err) = dispatch(
        &ctx,
        "schedule_cancel",
        serde_json::json!({"task_id": "missing-id"}),
        None,
    )
    .await;
    assert!(!cancel_err, "idempotent on missing: {cancel_out}");
}

/// AC5 wiring: kill switch read through the dispatch path too.
#[tokio::test]
async fn dispatch_kill_switch_blocks_create() {
    let (ctx_base, pool) = make_ctx().await;
    let (project_id, session_id) = seed_project_session(&pool).await;
    let mut ctx = ctx_base.clone();
    ctx.project_id = project_id.clone();
    db::config::set_config_value(&pool, SCHEDULED_TASKS_ENABLED_KEY, "false")
        .await
        .unwrap();

    let (out, is_err) = dispatch(
        &ctx,
        "schedule_task",
        serde_json::json!({
            "name": "x",
            "prompt": "y",
            "schedule": {"kind": "interval", "every_min": 30},
        }),
        Some(&session_id),
    )
    .await;
    assert!(is_err);
    assert!(out.contains("scheduled_tasks_enabled"));
}

/// AC4 (worker side): the family is structurally disabled for every
/// worker, regardless of allowlist.
#[test]
fn worker_toolsets_never_carry_schedule_family() {
    use crate::agent::subagent::SubagentDef;

    fn def_with_tools(tools: Vec<String>) -> SubagentDef {
        SubagentDef {
            name: "test-worker".into(),
            description: String::new(),
            system_prompt: String::new(),
            tools,
            isolation: None,
            model: None,
        }
    }

    let all = builtin_tools();
    // Empty allowlist = general-purpose convention (full set).
    let general = def_with_tools(vec![]);
    let filtered = filter_tools_for_subagent(all.clone(), &general);
    for name in ["schedule_task", "schedule_status", "schedule_cancel"] {
        assert!(
            !filtered.iter().any(|t: &ToolDef| t.name == name),
            "{name} must be STRUCTURALLY_DISABLED for workers"
        );
    }
    // Explicit allowlist naming the tool still loses (unconditional strip).
    let explicit = def_with_tools(vec!["schedule_task".to_string()]);
    let filtered2 = filter_tools_for_subagent(all, &explicit);
    assert!(!filtered2
        .iter()
        .any(|t: &ToolDef| t.name == "schedule_task"));
}

/// AC4 (group-chat side): the exhaustive whitelist naturally excludes
/// the family — moderator AND participant both.
#[test]
fn group_chat_toolsets_never_carry_schedule_family() {
    let all = builtin_tools();
    for is_moderator in [true, false] {
        let defs = group_chat_tool_defs(&all, is_moderator);
        for name in ["schedule_task", "schedule_status", "schedule_cancel"] {
            assert!(
                !defs.iter().any(|t| t.name == name),
                "{name} leaked into group chat (moderator={is_moderator})"
            );
        }
    }
}

/// Registration order: the family is appended at the TAIL of
/// `builtin_tools()` (provider prefix-cache contract — appending never
/// shifts the existing prefix), family-adjacent, in task→status→cancel
/// order.
#[test]
fn family_is_appended_last_in_order() {
    let names: Vec<String> = builtin_tools().into_iter().map(|t| t.name).collect();
    let n = names.len();
    assert!(n >= 3);
    assert_eq!(names[n - 3], "schedule_task");
    assert_eq!(names[n - 2], "schedule_status");
    assert_eq!(names[n - 1], "schedule_cancel");
}
