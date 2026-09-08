#![cfg(test)]

//! 调度循环(`scheduler_tick`)集成测试(F2 WP1,implement.md 集成
//! 清单)。经 `TickFire` seam 注入记录替身 —— `chat_inner` 的 provider
//! 由 catalog 解析,无法注入 mock(与 F3 gate 测试同款结论);fire 链路
//! 落库端的产物(metadata.scheduled / lost 审计)由
//! `agent/tests_message_queue.rs` 与 `tests_lost.rs` 的 driver / cancel
//! 层测试覆盖。
//!
//! TempDir 字段**后**于 state 声明(字段按声明序 Drop,db 池必须先于
//! 目录删除销毁)。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use crate::db::scheduled_tasks::{insert_scheduled_task, mark_task_fired, NewScheduledTask};
use crate::llm::types::{ChatMessage, MessageContent, Role};
use crate::scheduler::{self, actions, FireContext, TickFire};
use crate::state::AppState;

struct TickFixture {
    state: Arc<AppState>,
    project_id: String,
    session_id: String,
    /// TempDir guard —— 声明在最后,Drop 晚于 state(db 池)。
    _tmp: tempfile::TempDir,
}

/// `load_from_dir` 的 backfill spawn 经 Tauri async_runtime shim 借用
/// 当前 Tokio runtime —— 必须 multi_thread flavor(同 chat.rs F3 gate
/// 测试)。
async fn make_fixture() -> TickFixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);
    crate::db::create_project(
        &state.db,
        "tick-test-project",
        "/tmp/tick-test",
        false,
        None,
    )
    .await
    .expect("create_project");
    let project = crate::db::list_projects(&state.db, false)
        .await
        .expect("list_projects")
        .into_iter()
        .find(|p| p.name == "tick-test-project")
        .expect("project row");
    let session_id = uuid::Uuid::new_v4().to_string();
    crate::db::create_session(
        &state.db,
        &session_id,
        &project.id,
        "/tmp/tick-test",
        "mock-model",
        None,
        None,
        None,
    )
    .await
    .expect("create_session");
    TickFixture {
        state,
        project_id: project.id,
        session_id,
        _tmp: tmp,
    }
}

/// 建 interval 任务并(可选)预设 last_fired_at。
async fn seed_task(
    fx: &TickFixture,
    every_min: u32,
    last_fired_at: Option<i64>,
    name: &str,
) -> crate::db::scheduled_tasks::ScheduledTaskRow {
    let spec =
        scheduler::parse_schedule(&format!(r#"{{"kind":"interval","every_min":{every_min}}}"#))
            .expect("valid interval schedule");
    let step_ms = (every_min as i64) * 60_000;
    let task = insert_scheduled_task(
        &fx.state.db,
        NewScheduledTask {
            project_id: fx.project_id.clone(),
            target_session_id: Some(fx.session_id.clone()),
            target_mode: crate::db::scheduled_tasks::target_modes::FIXED.into(),
            model_id: None,
            name: name.to_string(),
            prompt: "汇总昨夜进展".into(),
            schedule_json: serde_json::to_string(&spec).unwrap(),
            enabled: true,
            created_by: "user".into(),
            next_fire_at: scheduler::now_epoch_ms() + step_ms,
            max_runs: None,
            ends_at: None,
            group_chat_config: None,
        },
    )
    .await
    .expect("insert task");
    // 回拨 created_at:insert 恒写 now,而触发判定窗口锚在
    // `not_before = max(created_at, last_fired_at)` —— 不回拨则预设的
    // 过去 last_fired_at 永远落不进判定窗口。
    sqlx::query("UPDATE scheduled_tasks SET created_at = ? WHERE id = ?")
        .bind(scheduler::now_epoch_ms() - 6 * 3_600_000)
        .bind(&task.id)
        .execute(&fx.state.db)
        .await
        .expect("backdate created_at");
    if let Some(fired) = last_fired_at {
        // 预设 = 模拟一次真实历史 fire(计一次 run_count,同生产语义)。
        mark_task_fired(
            &fx.state.db,
            &task.id,
            fired,
            fired + step_ms,
            true,
            None,
            None,
        )
        .await
        .expect("preset last_fired_at");
    }
    task
}

/// F2b:seed 后直写结束条件三列(同 created_at 回拨模式)。
async fn set_end_conditions(
    fx: &TickFixture,
    task_id: &str,
    max_runs: Option<i64>,
    ends_at: Option<i64>,
    run_count: i64,
) {
    sqlx::query("UPDATE scheduled_tasks SET max_runs = ?, ends_at = ?, run_count = ? WHERE id = ?")
        .bind(max_runs)
        .bind(ends_at)
        .bind(run_count)
        .bind(task_id)
        .execute(&fx.state.db)
        .await
        .expect("set end conditions");
}

/// 建单次档(CH11-1)任务:at_ms 相对 now 的偏移由调用方定;
/// created_at 恒回拨 6h,使「已过去的 at_ms」能落进判定窗口。
async fn seed_once_task(
    fx: &TickFixture,
    at_offset_ms: i64,
    name: &str,
) -> crate::db::scheduled_tasks::ScheduledTaskRow {
    let now = scheduler::now_epoch_ms();
    let spec = scheduler::parse_schedule(&format!(
        r#"{{"kind":"once","at_ms":{}}}"#,
        now + at_offset_ms
    ))
    .expect("valid once schedule");
    let task = insert_scheduled_task(
        &fx.state.db,
        NewScheduledTask {
            project_id: fx.project_id.clone(),
            target_session_id: Some(fx.session_id.clone()),
            target_mode: crate::db::scheduled_tasks::target_modes::FIXED.into(),
            model_id: None,
            name: name.to_string(),
            prompt: "跑一次就收工".into(),
            schedule_json: serde_json::to_string(&spec).unwrap(),
            enabled: true,
            created_by: "user".into(),
            next_fire_at: now + at_offset_ms.max(0),
            max_runs: None,
            ends_at: None,
            group_chat_config: None,
        },
    )
    .await
    .expect("insert once task");
    sqlx::query("UPDATE scheduled_tasks SET created_at = ? WHERE id = ?")
        .bind(now - 6 * 3_600_000)
        .bind(&task.id)
        .execute(&fx.state.db)
        .await
        .expect("backdate created_at");
    task
}

/// 记录替身:每次 fire 记下 FireContext 并返回 `Queued{uuid}`
/// (uuid 仅 Queued 返回路径可得,design §4.2)。
fn recording_fire(records: Arc<StdMutex<Vec<FireContext>>>) -> TickFire {
    Box::new(move |_state, ctx| {
        let records = records.clone();
        Box::pin(async move {
            records.lock().unwrap().push(ctx);
            Ok(crate::agent::chat::ChatAcceptance::Queued {
                id: uuid::Uuid::new_v4().to_string(),
                position: 1,
            })
        })
    })
}

/// 恒返 Started 的替身(闲时路径,不记 uuid)。
fn started_fire() -> TickFire {
    Box::new(|_state, _ctx| Box::pin(async { Ok(crate::agent::chat::ChatAcceptance::Started) }))
}

/// 恒返「队列满」错误的替身(chat_inner 唯一可预期 Err 形态,AC6)。
fn queue_full_fire() -> TickFire {
    Box::new(|_state, _ctx| {
        Box::pin(async {
            Err(crate::error::AppCommandError::new(
                crate::error::ErrorCategory::InvalidRequest,
                "排队已满(20 条上限)",
            ))
        })
    })
}

/// 目标 session 的 scheduled_task_fired 审计 `(action, reason?)` 序
/// (按写入序)。
async fn audit_actions(fx: &TickFixture) -> Vec<(String, Option<String>)> {
    let rows = sqlx::query(
        "SELECT payload_json FROM session_audit_events \
         WHERE session_id = ? AND kind = 'scheduled_task_fired' ORDER BY id ASC",
    )
    .bind(&fx.session_id)
    .fetch_all(&fx.state.db)
    .await
    .expect("audit query");
    rows.iter()
        .filter_map(|r| {
            use sqlx::Row;
            let payload: String = r.try_get(0).ok()?;
            let v: serde_json::Value = serde_json::from_str(&payload).ok()?;
            Some((
                v["action"].as_str()?.to_string(),
                v["reason"].as_str().map(str::to_string),
            ))
        })
        .collect()
}

async fn task_row(fx: &TickFixture, task_id: &str) -> crate::db::scheduled_tasks::ScheduledTaskRow {
    crate::db::scheduled_tasks::get_scheduled_task(&fx.state.db, task_id)
        .await
        .expect("get task")
        .expect("task row")
}

fn user_msg(text: &str) -> ChatMessage {
    ChatMessage {
        role: Role::User,
        content: MessageContent::Text(text.to_string()),
        speaker: None,
        attachments: None,
    }
}

// --- 实 fire:fired 动作 + 落账记 due + 审计 ---

#[tokio::test(flavor = "multi_thread")]
async fn tick_fires_due_task_fired_action_accounts_due_and_audits() {
    let fx = make_fixture().await;
    let now = scheduler::now_epoch_ms();
    // interval 1min、上次触发 90s 前 → due = now-30s,在 60s 宽限内 → fired。
    let task = seed_task(&fx, 1, Some(now - 90_000), "t1").await;

    let records = Arc::new(StdMutex::new(Vec::new()));
    let mut pending = HashMap::new();
    scheduler::scheduler_tick_with_fire(&fx.state, &mut pending, &recording_fire(records.clone()))
        .await;

    let recs = records.lock().unwrap();
    assert_eq!(recs.len(), 1, "one due task = one fire");
    assert_eq!(recs[0].task_id, task.id);
    assert_eq!(recs[0].target_session_id, fx.session_id);
    assert_eq!(recs[0].prompt, "汇总昨夜进展");
    drop(recs);

    // 落账恒记理论到期点 due = not_before + step(design §3,防相位漂移)。
    let row = task_row(&fx, &task.id).await;
    let due = now - 90_000 + 60_000;
    let accounted = row.last_fired_at.expect("accounted");
    assert!(
        (accounted - due).abs() < 5_000,
        "last_fired_at must be the theoretical due point ({accounted} vs {due})"
    );
    // 展示值 = due + step(interval 网格一致,精确相等)。
    assert_eq!(row.next_fire_at, accounted + 60_000);

    assert_eq!(
        audit_actions(&fx).await,
        vec![(actions::FIRED.to_string(), None)],
        "audit action = fired, no reason"
    );
}

// --- catch-up(超宽限 → catchup;补一次不追多次)---

#[tokio::test(flavor = "multi_thread")]
async fn tick_beyond_grace_audits_catchup_action() {
    let fx = make_fixture().await;
    let now = scheduler::now_epoch_ms();
    // interval 60min、上次触发 90min 前 → due = now-30min > 60s 宽限 → catchup。
    let task = seed_task(&fx, 60, Some(now - 90 * 60_000), "t1").await;

    let records = Arc::new(StdMutex::new(Vec::new()));
    let mut pending = HashMap::new();
    scheduler::scheduler_tick_with_fire(&fx.state, &mut pending, &recording_fire(records.clone()))
        .await;

    assert_eq!(records.lock().unwrap().len(), 1, "catch-up fires once");
    // D4「补一次、不追多次」:90min 窗口只消费最近一个到期点。
    let accounted = task_row(&fx, &task.id)
        .await
        .last_fired_at
        .expect("accounted");
    assert!(
        (accounted - (now - 30 * 60_000)).abs() < 5_000,
        "accounted due is the most recent missed point"
    );
    assert_eq!(
        audit_actions(&fx).await,
        vec![(actions::CATCHUP.to_string(), None)]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn tick_is_idempotent_when_rerun_immediately() {
    let fx = make_fixture().await;
    let now = scheduler::now_epoch_ms();
    seed_task(&fx, 1, Some(now - 90_000), "t1").await;

    let records = Arc::new(StdMutex::new(Vec::new()));
    let fire = recording_fire(records.clone());
    let mut pending = HashMap::new();
    // catch-up 与常规触发同一算法:同窗口立即重评不双 fire。
    scheduler::scheduler_tick_with_fire(&fx.state, &mut pending, &fire).await;
    scheduler::scheduler_tick_with_fire(&fx.state, &mut pending, &fire).await;
    assert_eq!(
        records.lock().unwrap().len(),
        1,
        "second tick must not re-fire (due consumed by accounting)"
    );
}

// --- 同 session 双任务 deferral(同 tick 至多 fire 一个)---

#[tokio::test(flavor = "multi_thread")]
async fn same_session_two_tasks_defer_second_to_next_tick() {
    let fx = make_fixture().await;
    let now = scheduler::now_epoch_ms();
    // 两个 interval 1min 任务绑同一 session,同时到期(list_enabled 按
    // last_fired_at + id 升序,消费顺序确定)。
    let first = seed_task(&fx, 1, Some(now - 90_000), "first").await;
    let second = seed_task(&fx, 1, Some(now - 90_000), "second").await;

    let records = Arc::new(StdMutex::new(Vec::new()));
    let fire = recording_fire(records.clone());
    let mut pending = HashMap::new();

    scheduler::scheduler_tick_with_fire(&fx.state, &mut pending, &fire).await;
    assert_eq!(
        records.lock().unwrap().len(),
        1,
        "same target session: only one fire per tick"
    );
    // 顺延者不前移基准(deferral 无落账、无审计)。
    let first_fired = task_row(&fx, &first.id).await.last_fired_at != Some(now - 90_000);
    let (fired_id, deferred_id) = if first_fired {
        (&first.id, &second.id)
    } else {
        (&second.id, &first.id)
    };
    assert_eq!(
        task_row(&fx, deferred_id).await.last_fired_at,
        Some(now - 90_000),
        "deferred task basis must NOT advance"
    );
    assert_eq!(
        audit_actions(&fx).await.len(),
        1,
        "deferral itself is not audited (only the fired task)"
    );

    // 下一个 tick:先 fire 的任务基准在宽限内无新 due;顺延者被消费。
    scheduler::scheduler_tick_with_fire(&fx.state, &mut pending, &fire).await;
    let recs = records.lock().unwrap();
    assert_eq!(recs.len(), 2, "deferred task fires on the next tick");
    assert_eq!(
        recs[0].task_id, *fired_id,
        "first tick fired the earlier task"
    );
    assert_eq!(recs[1].task_id, *deferred_id, "the deferred task is second");
    drop(recs);
}

// --- 去重(上次条目滞留队列)---

#[tokio::test(flavor = "multi_thread")]
async fn dedup_skips_when_previous_entry_still_queued() {
    let fx = make_fixture().await;
    let now = scheduler::now_epoch_ms();
    let task = seed_task(&fx, 1, Some(now - 90_000), "t1").await;

    // 上一次 fire 的条目仍在队列(未被消费)+ 去重表有它的 uuid。
    const STUCK: &str = "stuck-entry-uuid";
    fx.state
        .message_queues
        .lock()
        .await
        .entry(fx.session_id.clone())
        .or_default()
        .push_back(crate::agent::message_queue::QueuedMessage {
            id: STUCK.to_string(),
            message: user_msg("stale injection"),
            enqueued_at: now - 60_000,
            priority: 0,
            origin: Some(scheduler::TaskOrigin::Scheduled {
                task_id: task.id.clone(),
                task_name: task.name.clone(),
                fired_at: now - 60_000,
            }),
        });
    let mut pending = HashMap::new();
    pending.insert(task.id.clone(), STUCK.to_string());

    let records = Arc::new(StdMutex::new(Vec::new()));
    scheduler::scheduler_tick_with_fire(&fx.state, &mut pending, &recording_fire(records.clone()))
        .await;

    assert!(records.lock().unwrap().is_empty(), "dedup must not fire");
    // 该 due 点标记为已消费(防每 tick 重判)。
    let accounted = task_row(&fx, &task.id)
        .await
        .last_fired_at
        .expect("accounted");
    assert!(
        (accounted - (now - 30_000)).abs() < 5_000,
        "dedup branch accounts due"
    );
    assert_eq!(
        audit_actions(&fx).await,
        vec![(actions::SKIPPED_DEDUP.to_string(), None)]
    );
    // F2b:dedup 跳过不计数(prompt 未送达,仅消费 due 点)。
    assert_eq!(task_row(&fx, &task.id).await.run_count, 1, "preset only");
}

#[tokio::test(flavor = "multi_thread")]
async fn consumed_uuid_no_longer_blocks_next_fire() {
    let fx = make_fixture().await;
    let now = scheduler::now_epoch_ms();
    let task = seed_task(&fx, 1, Some(now - 90_000), "t1").await;

    // 去重表有 uuid 但队列已空(条目已被消费)→ 不拦。
    let mut pending = HashMap::new();
    pending.insert(task.id.clone(), "already-drained".to_string());

    let records = Arc::new(StdMutex::new(Vec::new()));
    scheduler::scheduler_tick_with_fire(&fx.state, &mut pending, &recording_fire(records.clone()))
        .await;
    assert_eq!(records.lock().unwrap().len(), 1);
}

// --- kill switch ---

#[tokio::test(flavor = "multi_thread")]
async fn kill_switch_makes_the_tick_idle() {
    let fx = make_fixture().await;
    let now = scheduler::now_epoch_ms();
    let task = seed_task(&fx, 1, Some(now - 90_000), "t1").await;
    crate::db::config::set_config_value(&fx.state.db, "scheduled_tasks_enabled", "false")
        .await
        .expect("set kill switch");

    let records = Arc::new(StdMutex::new(Vec::new()));
    let mut pending = HashMap::new();
    scheduler::scheduler_tick_with_fire(&fx.state, &mut pending, &recording_fire(records.clone()))
        .await;

    assert!(records.lock().unwrap().is_empty(), "kill switch = no fire");
    assert!(audit_actions(&fx).await.is_empty(), "no audit when idle");
    assert_eq!(
        task_row(&fx, &task.id).await.last_fired_at,
        Some(now - 90_000),
        "basis untouched"
    );
}

// --- message_queue 关闭(legacy 分支对忙 session 是取消语义,避开)---

#[tokio::test(flavor = "multi_thread")]
async fn queue_disabled_skips_with_audit_and_no_accounting() {
    let fx = make_fixture().await;
    let now = scheduler::now_epoch_ms();
    let task = seed_task(&fx, 1, Some(now - 90_000), "t1").await;
    crate::db::config::set_config_value(&fx.state.db, "message_queue_enabled", "false")
        .await
        .expect("disable queue");

    let records = Arc::new(StdMutex::new(Vec::new()));
    let mut pending = HashMap::new();
    scheduler::scheduler_tick_with_fire(&fx.state, &mut pending, &recording_fire(records.clone()))
        .await;

    assert!(
        records.lock().unwrap().is_empty(),
        "legacy branch must be avoided"
    );
    assert_eq!(
        audit_actions(&fx).await,
        vec![(actions::SKIPPED_QUEUE_DISABLED.to_string(), None)]
    );
    // 无落账:恢复开启后该 due 点仍会被消费(区别于 dedup 的落账语义)。
    assert_eq!(
        task_row(&fx, &task.id).await.last_fired_at,
        Some(now - 90_000)
    );
}

// --- fire Err:error 审计(附 reason)+ 仍落账 ---

#[tokio::test(flavor = "multi_thread")]
async fn fire_error_audits_error_with_queue_full_reason_and_accounts() {
    let fx = make_fixture().await;
    let now = scheduler::now_epoch_ms();
    let task = seed_task(&fx, 1, Some(now - 90_000), "t1").await;

    let mut pending = HashMap::new();
    scheduler::scheduler_tick_with_fire(&fx.state, &mut pending, &queue_full_fire()).await;

    assert_eq!(
        audit_actions(&fx).await,
        vec![(actions::ERROR.to_string(), Some("queue_full".to_string()))],
        "error audit carries the classified reason"
    );
    // due 仍被消费(防重试风暴);丢失由审计兜底(design §4.3)。
    let accounted = task_row(&fx, &task.id)
        .await
        .last_fired_at
        .expect("accounted");
    assert!((accounted - (now - 30_000)).abs() < 5_000);
}

// --- Started 路径不记 uuid(design §4.2 定案)---

#[tokio::test(flavor = "multi_thread")]
async fn started_acceptance_does_not_record_pending_uuid() {
    let fx = make_fixture().await;
    let now = scheduler::now_epoch_ms();
    let task = seed_task(&fx, 1, Some(now - 90_000), "t1").await;

    let mut pending = HashMap::new();
    scheduler::scheduler_tick_with_fire(&fx.state, &mut pending, &started_fire()).await;
    assert!(
        !pending.contains_key(&task.id),
        "Started carries no queue uuid → no dedup key"
    );
    assert_eq!(
        audit_actions(&fx).await,
        vec![(actions::FIRED.to_string(), None)]
    );
}

// --- F2b:结束条件(次数上限 / 结束日期)---

/// gate 4 + gate 1:max_runs=1 的任务第 1 次 fire 后立即完成(enabled=0、
/// completed 审计恰好一次),下 tick 不再扫描。
#[tokio::test(flavor = "multi_thread")]
async fn max_runs_task_completes_after_nth_fire() {
    let fx = make_fixture().await;
    let now = scheduler::now_epoch_ms();
    let task = seed_task(&fx, 1, Some(now - 90_000), "t1").await;
    // 预设的 last_fired_at 已带 run_count=1;上限改 2 → 本次 fire 后
    // new_run_count=2 达限 → 完成。
    set_end_conditions(&fx, &task.id, Some(2), None, 1).await;

    let records = Arc::new(StdMutex::new(Vec::new()));
    let fire = recording_fire(records.clone());
    let mut pending = HashMap::new();
    scheduler::scheduler_tick_with_fire(&fx.state, &mut pending, &fire).await;

    assert_eq!(records.lock().unwrap().len(), 1, "fires the nth run");
    let row = task_row(&fx, &task.id).await;
    assert_eq!(row.run_count, 2, "run_count incremented to the limit");
    assert!(!row.enabled, "completed task auto-disabled");
    assert_eq!(
        audit_actions(&fx).await,
        vec![
            (actions::FIRED.to_string(), None),
            (actions::COMPLETED.to_string(), Some("max_runs".to_string())),
        ],
        "fired then completed(max_runs), exactly once each"
    );

    // 完成后任务退出 enabled 扫描集:重跑 tick 无 fire、无重复审计。
    scheduler::scheduler_tick_with_fire(&fx.state, &mut pending, &fire).await;
    assert_eq!(records.lock().unwrap().len(), 1, "no fire after completion");
    assert_eq!(audit_actions(&fx).await.len(), 2, "completed audits once");
}

/// gate 1(兜底):run_count 已达上限的任务直接完成,不 fire。
#[tokio::test(flavor = "multi_thread")]
async fn max_runs_at_limit_task_does_not_fire() {
    let fx = make_fixture().await;
    let now = scheduler::now_epoch_ms();
    let task = seed_task(&fx, 1, Some(now - 90_000), "t1").await;
    set_end_conditions(&fx, &task.id, Some(1), None, 1).await;

    let records = Arc::new(StdMutex::new(Vec::new()));
    let mut pending = HashMap::new();
    scheduler::scheduler_tick_with_fire(&fx.state, &mut pending, &recording_fire(records.clone()))
        .await;

    assert!(records.lock().unwrap().is_empty(), "no fire at limit");
    let row = task_row(&fx, &task.id).await;
    assert!(!row.enabled);
    assert_eq!(
        audit_actions(&fx).await,
        vec![(actions::COMPLETED.to_string(), Some("max_runs".to_string()))]
    );
}

/// gate 3:due 已越过 ends_at → 不 fire,直接完成(end_date)。
#[tokio::test(flavor = "multi_thread")]
async fn ends_at_past_due_does_not_fire_and_completes() {
    let fx = make_fixture().await;
    let now = scheduler::now_epoch_ms();
    let task = seed_task(&fx, 1, Some(now - 90_000), "t1").await;
    // due = now-30s;ends_at = now-45s → due > ends_at。
    set_end_conditions(&fx, &task.id, None, Some(now - 45_000), 1).await;

    let records = Arc::new(StdMutex::new(Vec::new()));
    let mut pending = HashMap::new();
    scheduler::scheduler_tick_with_fire(&fx.state, &mut pending, &recording_fire(records.clone()))
        .await;

    assert!(
        records.lock().unwrap().is_empty(),
        "past-end due must not fire"
    );
    let row = task_row(&fx, &task.id).await;
    assert!(!row.enabled);
    assert_eq!(row.run_count, 1, "run_count untouched (no fire)");
    assert_eq!(
        audit_actions(&fx).await,
        vec![(actions::COMPLETED.to_string(), Some("end_date".to_string()))]
    );
}

/// D9(含当日):due ≤ ends_at 照常 fire;若下一到期点越过 ends_at,
/// fire 后立即完成(end_date)。
#[tokio::test(flavor = "multi_thread")]
async fn ends_at_inclusive_fires_then_completes() {
    let fx = make_fixture().await;
    let now = scheduler::now_epoch_ms();
    let task = seed_task(&fx, 1, Some(now - 90_000), "t1").await;
    // due = now-30s ≤ ends_at(now-10s)→ fire;next = now+30s > ends_at
    // → gate 4 完成。
    set_end_conditions(&fx, &task.id, None, Some(now - 10_000), 1).await;

    let records = Arc::new(StdMutex::new(Vec::new()));
    let mut pending = HashMap::new();
    scheduler::scheduler_tick_with_fire(&fx.state, &mut pending, &recording_fire(records.clone()))
        .await;

    assert_eq!(
        records.lock().unwrap().len(),
        1,
        "fires within the end date"
    );
    let row = task_row(&fx, &task.id).await;
    assert_eq!(row.run_count, 2);
    assert!(!row.enabled, "no next due within ends_at → completed");
    assert_eq!(
        audit_actions(&fx).await,
        vec![
            (actions::FIRED.to_string(), None),
            (actions::COMPLETED.to_string(), Some("end_date".to_string())),
        ]
    );
}

/// ends_at 尚远:fire 正常,任务保持 enabled(完成逻辑不误伤)。
#[tokio::test(flavor = "multi_thread")]
async fn ends_at_far_future_keeps_task_enabled() {
    let fx = make_fixture().await;
    let now = scheduler::now_epoch_ms();
    let task = seed_task(&fx, 1, Some(now - 90_000), "t1").await;
    set_end_conditions(&fx, &task.id, None, Some(now + 3_600_000), 1).await;

    let records = Arc::new(StdMutex::new(Vec::new()));
    let mut pending = HashMap::new();
    scheduler::scheduler_tick_with_fire(&fx.state, &mut pending, &recording_fire(records.clone()))
        .await;

    assert_eq!(records.lock().unwrap().len(), 1);
    let row = task_row(&fx, &task.id).await;
    assert!(row.enabled, "ends_at not reached → stays enabled");
    assert_eq!(
        audit_actions(&fx).await,
        vec![(actions::FIRED.to_string(), None)]
    );
}

// --- 单次档(CH11-1)---

/// 到点 → fire 恰一次 → gate 4 即时完成(reason=once);重跑 tick 无 fire。
#[tokio::test(flavor = "multi_thread")]
async fn once_task_fires_once_then_completes() {
    let fx = make_fixture().await;
    // at_ms = now-30s(刚过点,进宽限内 → fired 动作)。
    let task = seed_once_task(&fx, -30_000, "t1").await;

    let records = Arc::new(StdMutex::new(Vec::new()));
    let mut pending = HashMap::new();
    scheduler::scheduler_tick_with_fire(&fx.state, &mut pending, &recording_fire(records.clone()))
        .await;

    assert_eq!(records.lock().unwrap().len(), 1, "fires the single run");
    let row = task_row(&fx, &task.id).await;
    assert_eq!(row.run_count, 1);
    // 落账记理论到期点(seed 的 at_ms 本身)。
    let at_ms = match scheduler::parse_schedule(&task.schedule_json).unwrap() {
        scheduler::ScheduleSpec::Once { at_ms } => at_ms,
        _ => panic!("seeded once schedule"),
    };
    assert_eq!(row.last_fired_at, Some(at_ms), "accounted at the due point");
    assert!(!row.enabled, "single due point consumed → completed");

    // 重跑:enabled=0 出扫描集,无 fire、无重复审计。
    scheduler::scheduler_tick_with_fire(&fx.state, &mut pending, &recording_fire(records.clone()))
        .await;
    assert_eq!(records.lock().unwrap().len(), 1, "no second fire");
}

/// 未到点:不 fire,保持 enabled(等点)。
#[tokio::test(flavor = "multi_thread")]
async fn once_task_future_point_waits() {
    let fx = make_fixture().await;
    let task = seed_once_task(&fx, 3_600_000, "t1").await;

    let records = Arc::new(StdMutex::new(Vec::new()));
    let mut pending = HashMap::new();
    scheduler::scheduler_tick_with_fire(&fx.state, &mut pending, &recording_fire(records.clone()))
        .await;

    assert!(records.lock().unwrap().is_empty(), "not due yet");
    let row = task_row(&fx, &task.id).await;
    assert!(row.enabled, "future once point stays enabled");
    assert_eq!(row.run_count, 0);
    assert!(audit_actions(&fx).await.is_empty());
}

/// None 分支兜底:at_ms 已被消费(last_fired_at = at_ms)但完成写
/// 未落(等价「重启用已过期的一次性任务」)→ 完成(reason=once),不 fire。
#[tokio::test(flavor = "multi_thread")]
async fn once_task_consumed_point_completes_without_firing() {
    let fx = make_fixture().await;
    let now = scheduler::now_epoch_ms();
    let task = seed_once_task(&fx, -3_600_000, "t1").await;
    // 模拟历史 fire 已消费 due 点但 enabled 仍为 1(完成写丢失/被重启用)。
    mark_task_fired(
        &fx.state.db,
        &task.id,
        now - 3_600_000,
        now + 86_400_000,
        true,
        None,
        None,
    )
    .await
    .expect("preset consumed point");

    let records = Arc::new(StdMutex::new(Vec::new()));
    let mut pending = HashMap::new();
    scheduler::scheduler_tick_with_fire(&fx.state, &mut pending, &recording_fire(records.clone()))
        .await;

    assert!(records.lock().unwrap().is_empty(), "no due left to fire");
    let row = task_row(&fx, &task.id).await;
    assert!(!row.enabled, "expired once task completes");
    assert_eq!(row.run_count, 1, "count untouched");
    assert_eq!(
        audit_actions(&fx).await,
        vec![(actions::COMPLETED.to_string(), Some("once".to_string()))]
    );
}

// --- per_run 档(08-31-sched-per-run-session)---

use crate::db::scheduled_tasks::target_modes;

/// per_run 任务 seed:不绑定固定 session(target NULL),可选模型绑定,
/// created_at 回拨同 [`seed_task`]。
async fn seed_per_run_task(
    fx: &TickFixture,
    every_min: u32,
    name: &str,
    model_id: Option<&str>,
) -> crate::db::scheduled_tasks::ScheduledTaskRow {
    let spec =
        scheduler::parse_schedule(&format!(r#"{{"kind":"interval","every_min":{every_min}}}"#))
            .expect("valid interval schedule");
    let step_ms = (every_min as i64) * 60_000;
    let task = insert_scheduled_task(
        &fx.state.db,
        NewScheduledTask {
            project_id: fx.project_id.clone(),
            target_session_id: None,
            target_mode: target_modes::PER_RUN.into(),
            model_id: model_id.map(str::to_string),
            name: name.to_string(),
            prompt: "每轮全新开始".into(),
            schedule_json: serde_json::to_string(&spec).unwrap(),
            enabled: true,
            created_by: "user".into(),
            next_fire_at: scheduler::now_epoch_ms() + step_ms,
            max_runs: None,
            ends_at: None,
            group_chat_config: None,
        },
    )
    .await
    .expect("insert per_run task");
    sqlx::query("UPDATE scheduled_tasks SET created_at = ? WHERE id = ?")
        .bind(scheduler::now_epoch_ms() - 6 * 3_600_000)
        .bind(&task.id)
        .execute(&fx.state.db)
        .await
        .expect("backdate created_at");
    task
}

/// 指定 session 的 scheduled_task_fired 审计 `(action, reason?)` 序。
async fn audit_actions_for(fx: &TickFixture, session_id: &str) -> Vec<(String, Option<String>)> {
    let rows = sqlx::query(
        "SELECT payload_json FROM session_audit_events \
         WHERE session_id = ? AND kind = 'scheduled_task_fired' ORDER BY id ASC",
    )
    .bind(session_id)
    .fetch_all(&fx.state.db)
    .await
    .expect("audit query");
    rows.iter()
        .filter_map(|r| {
            use sqlx::Row;
            let payload: String = r.try_get(0).ok()?;
            let v: serde_json::Value = serde_json::from_str(&payload).ok()?;
            Some((
                v["action"].as_str()?.to_string(),
                v["reason"].as_str().map(str::to_string),
            ))
        })
        .collect()
}

/// per_run fire:tick 内新建 run session,fire 注入该新 session(AC2),
/// `last_run_session_id` 落账,审计挂新 session(AC3),模型绑定应用(AC4)。
#[tokio::test(flavor = "multi_thread")]
async fn per_run_task_creates_run_session_and_fires_into_it() {
    let fx = make_fixture().await;
    let now = scheduler::now_epoch_ms();
    let model = crate::db::list_models(&fx.state.db)
        .await
        .expect("list models")
        .into_iter()
        .next()
        .expect("seeded model");
    let task = seed_per_run_task(&fx, 1, "早报", Some(&model.model.id)).await;

    let records = Arc::new(StdMutex::new(Vec::new()));
    let mut pending = HashMap::new();
    scheduler::scheduler_tick_with_fire(&fx.state, &mut pending, &recording_fire(records.clone()))
        .await;

    // 块作用域取记录(guard 随块结束释放,不跨下方 await;clippy
    // await_holding_lock 不识别 drop(),存量用例 265 行同款告警)。
    let run_sid = {
        let recs = records.lock().unwrap();
        assert_eq!(recs.len(), 1, "one due per_run task = one fire");
        assert_ne!(
            recs[0].target_session_id, fx.session_id,
            "fire must target the freshly created session, not the fixture session"
        );
        recs[0].target_session_id.clone()
    };

    // 新 session 行:同 project、标题 = 「任务名 YYYY-MM-DD HH:MM」、
    // 模型绑定写入 per-session 覆盖列。
    let session = crate::db::sessions::load_session(&fx.state.db, &run_sid)
        .await
        .expect("load run session")
        .expect("run session row");
    assert_eq!(session.session.project_id, fx.project_id);
    assert_eq!(
        session.session.title,
        format!("早报 {}", scheduler::compute::format_local_hhmm(now)),
        "run session title = task name + local fire time"
    );
    assert_eq!(
        session.session.model_id.as_deref(),
        Some(model.model.id.as_str()),
        "task-level model binding applied to the new session"
    );

    // 落账:last_run_session_id = 新 session。
    let row = task_row(&fx, &task.id).await;
    assert_eq!(
        row.last_run_session_id.as_deref(),
        Some(run_sid.as_str()),
        "accounting records the run session"
    );
    assert!(row.last_fired_at.is_some(), "due consumed");

    // 审计挂新 session(AC3);固定 fixture session 上无审计。
    assert_eq!(
        audit_actions_for(&fx, &run_sid).await,
        vec![(actions::FIRED.to_string(), None)]
    );
    assert!(audit_actions(&fx).await.is_empty());
}

/// per_run 不参与「同 session 每 tick 一 fire」:同 tick 两个 per_run
/// 任务都各自建 session 并 fire(目标互不相同)。
#[tokio::test(flavor = "multi_thread")]
async fn two_per_run_tasks_same_tick_fire_independently() {
    let fx = make_fixture().await;
    let _a = seed_per_run_task(&fx, 1, "任务A", None).await;
    let _b = seed_per_run_task(&fx, 1, "任务B", None).await;

    let records = Arc::new(StdMutex::new(Vec::new()));
    let mut pending = HashMap::new();
    scheduler::scheduler_tick_with_fire(&fx.state, &mut pending, &recording_fire(records.clone()))
        .await;

    let recs = records.lock().unwrap();
    assert_eq!(recs.len(), 2, "no deferral across per_run tasks");
    assert_ne!(
        recs[0].target_session_id, recs[1].target_session_id,
        "each fire gets its own fresh session"
    );
}

/// per_run 绕过 queue-disabled gate 与队列去重(pending 滞留 + 开关全关
/// 仍照常 fire,AC5)。
#[tokio::test(flavor = "multi_thread")]
async fn per_run_ignores_queue_disabled_and_pending_dedup() {
    let fx = make_fixture().await;
    let task = seed_per_run_task(&fx, 1, "每跑", None).await;
    // message_queue_enabled=false(fail-open 默认开,须显式写 "false")。
    crate::db::config::set_config_value(&fx.state.db, "message_queue_enabled", "false")
        .await
        .expect("set queue flag");

    let records = Arc::new(StdMutex::new(Vec::new()));
    // 预置一个「上一次 fire 的条目仍滞留队列」假 uuid:fixed 档会被
    // dedup 拦下,per_run 无共享队列,不应受影响。
    let mut pending = HashMap::new();
    pending.insert(task.id.clone(), uuid::Uuid::new_v4().to_string());
    scheduler::scheduler_tick_with_fire(&fx.state, &mut pending, &recording_fire(records.clone()))
        .await;

    assert_eq!(
        records.lock().unwrap().len(),
        1,
        "per_run fires despite queue-disabled + pending dedup"
    );
    let run_sid = records.lock().unwrap()[0].target_session_id.clone();
    let actions = audit_actions_for(&fx, &run_sid).await;
    assert_eq!(
        actions,
        vec![(actions::FIRED.to_string(), None)],
        "fired (not skipped_queue_disabled / skipped_dedup)"
    );
}

/// `create_run_session` 纯函数性:project 不存在 → Err(字符串 reason),
/// 不留半成品。
#[tokio::test(flavor = "multi_thread")]
async fn create_run_session_missing_project_errors() {
    let fx = make_fixture().await;
    let row = crate::db::scheduled_tasks::ScheduledTaskRow {
        id: "t".into(),
        project_id: "no-such-project".into(),
        target_session_id: None,
        target_mode: target_modes::PER_RUN.into(),
        model_id: None,
        last_run_session_id: None,
        name: "孤儿".into(),
        prompt: "p".into(),
        schedule_json: r#"{"kind":"interval","every_min":30}"#.into(),
        enabled: true,
        created_by: "user".into(),
        created_at: 1,
        last_fired_at: None,
        next_fire_at: 1,
        run_count: 0,
        max_runs: None,
        ends_at: None,
        group_chat_config: None,
        last_fire_outcome: None,
    };
    let err = scheduler::create_run_session(&fx.state.db, &row, scheduler::now_epoch_ms())
        .await
        .expect_err("missing project must error");
    assert!(err.contains("不存在"), "error names the project: {err}");
}

// --- M4a 群聊档 fire_group_chat(09-07-gce-m4a-scheduled-deliberation;
// 评审五组:tick 路由 / 计数矩阵 / resume 兜底 / 迁移校验(在
// migrations_tests)/ 对照组(上方 fixed/per_run 既有用例))---

/// 建群聊任务(interval N 分钟,可选预设历史 fire)。config 用种子
/// model id(catalog 预检过);`bogus_model = true` 时引用不存在的模型。
async fn seed_group_task(
    fx: &TickFixture,
    every_min: u32,
    name: &str,
    bogus_model: bool,
) -> crate::db::scheduled_tasks::ScheduledTaskRow {
    let spec =
        scheduler::parse_schedule(&format!(r#"{{"kind":"interval","every_min":{every_min}}}"#))
            .expect("valid interval schedule");
    let (m0, m1) = if bogus_model {
        ("no-such-model".to_string(), "also-missing".to_string())
    } else {
        let models = crate::db::list_models(&fx.state.db)
            .await
            .expect("list models");
        let mut ids = models.into_iter().map(|m| m.model.id);
        (
            ids.next().expect("seeded model 0"),
            ids.next().expect("seeded model 1"),
        )
    };
    let config = serde_json::json!({
        "moderator_model_id": m0,
        "participants": [{ "name": "架构", "model_id": m1, "persona_md": "视角" }],
    })
    .to_string();
    let task = insert_scheduled_task(
        &fx.state.db,
        NewScheduledTask {
            project_id: fx.project_id.clone(),
            target_session_id: None,
            target_mode: crate::db::scheduled_tasks::target_modes::GROUP_CHAT.into(),
            model_id: None,
            name: name.to_string(),
            prompt: "复盘本周架构".into(),
            schedule_json: serde_json::to_string(&spec).unwrap(),
            enabled: true,
            created_by: "user".into(),
            next_fire_at: scheduler::now_epoch_ms() + 60_000,
            max_runs: None,
            ends_at: None,
            group_chat_config: Some(config),
        },
    )
    .await
    .expect("insert group task");
    sqlx::query("UPDATE scheduled_tasks SET created_at = ? WHERE id = ?")
        .bind(scheduler::now_epoch_ms() - 6 * 3_600_000)
        .bind(&task.id)
        .execute(&fx.state.db)
        .await
        .expect("backdate created_at");
    task
}

/// 建一个 prior 群聊 session 并挂到任务的 last_run_session_id(直写)。
/// `stop_reason` = None 表示停摆场;checkpoint = Some(round) upsert 行。
async fn attach_prior_discussion(
    fx: &TickFixture,
    task_id: &str,
    stop_reason: Option<&str>,
    checkpoint_round: Option<i64>,
) -> String {
    let sid = uuid::Uuid::new_v4().to_string();
    let metadata = r#"{"participants":[{"name":"架构","model":"m1"}],"created_via":"scheduled"}"#;
    crate::db::create_session(
        &fx.state.db,
        &sid,
        &fx.project_id,
        "/tmp/tick-test",
        "mod",
        None,
        Some("group_chat"),
        Some(metadata),
    )
    .await
    .expect("create prior group session");
    if let Some(reason) = stop_reason {
        crate::db::finalize_group_chat_lifecycle(&fx.state.db, &sid, reason, None)
            .await
            .expect("set prior stop_reason");
    }
    if let Some(round) = checkpoint_round {
        crate::db::upsert_group_chat_checkpoint(&fx.state.db, &sid, round, 0)
            .await
            .expect("upsert checkpoint");
    }
    sqlx::query("UPDATE scheduled_tasks SET last_run_session_id = ? WHERE id = ?")
        .bind(&sid)
        .bind(task_id)
        .execute(&fx.state.db)
        .await
        .expect("attach prior session");
    sid
}

/// 开新场(无 last_run_session_id):tick 走四态第 4 臂 → 建群
/// (metadata 三键归因 + participants + moderator 绑定)→ chat_inner 受理
/// (测试环境 keyless provider → preflight 恒 Started,确定性)→ 落账。
#[tokio::test(flavor = "multi_thread")]
async fn group_chat_task_opens_new_discussion_with_attribution_metadata() {
    let fx = make_fixture().await;
    let task = seed_group_task(&fx, 1, "每周审议", false).await;
    let models = crate::db::list_models(&fx.state.db).await.unwrap();
    let m0 = models[0].model.id.clone();
    let m1 = models[1].model.id.clone();

    let mut pending = HashMap::new();
    scheduler::scheduler_tick(&fx.state, &mut pending).await;

    let row = task_row(&fx, &task.id).await;
    assert_eq!(row.run_count, 1, "accepted open-new counts");
    assert_eq!(row.last_fire_outcome.as_deref(), Some("started"));
    let sid = row
        .last_run_session_id
        .clone()
        .expect("run session recorded");
    let loaded = crate::db::sessions::load_session(&fx.state.db, &sid)
        .await
        .unwrap()
        .expect("discussion session");
    assert_eq!(
        loaded.session.session_type,
        crate::db::SessionType::GroupChat
    );
    assert_eq!(
        loaded.session.model_id.as_deref(),
        Some(m0.as_str()),
        "moderator bound to the per-session override column"
    );
    let meta = loaded.session.metadata.expect("metadata");
    assert_eq!(meta["created_via"], "scheduled");
    assert_eq!(meta["scheduled_task_id"], *task.id);
    assert_eq!(meta["scheduled_task_name"], "每周审议");
    assert_eq!(meta["participants"][0]["model"], *m1);
    assert_eq!(
        audit_actions_for(&fx, &sid).await,
        vec![(actions::FIRED_GROUP_CHAT.to_string(), None)]
    );
    assert!(
        row.last_fired_at.is_some(),
        "due consumed (last_fired_at recorded)"
    );
}

/// gce-m4c(09-08):`group_chat_config.token_budget` → fire 建群 metadata
/// 的 `token_budget` 键。两态锁定:Some → 键带上原值;None(键缺失)→
/// **不写 null 键**(转录导出/检索按原始 JSON 读 metadata,缺键 = 不限
/// 的既有语义不变)。预算到线的**停机生效**由 tests_group_chat 的 C1.2
/// 剧本证明(metadata 键 → 轮头硬停),这里锁透传链本身。
#[tokio::test(flavor = "multi_thread")]
async fn group_chat_fire_carries_token_budget_only_when_declared() {
    // parse 校验:0 不是合法预算(不限 = 省略键,不是 0)。
    assert!(crate::db::scheduled_tasks::parse_group_chat_task_config(
        r#"{"moderator_model_id":"m","participants":[{"name":"a","model_id":"m"}],"token_budget":0}"#
    )
    .is_err());

    let fx = make_fixture().await;
    let models = crate::db::list_models(&fx.state.db).await.unwrap();
    let mut ids = models.into_iter().map(|m| m.model.id);
    let (m0, m1) = (
        ids.next().expect("seeded model 0"),
        ids.next().expect("seeded model 1"),
    );

    let spec = scheduler::parse_schedule(r#"{"kind":"interval","every_min":30}"#).expect("valid");
    let with_budget = serde_json::json!({
        "moderator_model_id": m0,
        "participants": [{ "name": "架构", "model_id": m1, "persona_md": "视角" }],
        "token_budget": 400_000u64,
    })
    .to_string();
    let task_with = insert_scheduled_task(
        &fx.state.db,
        NewScheduledTask {
            project_id: fx.project_id.clone(),
            target_session_id: None,
            target_mode: crate::db::scheduled_tasks::target_modes::GROUP_CHAT.into(),
            model_id: None,
            name: "带预算".into(),
            prompt: "复盘".into(),
            schedule_json: serde_json::to_string(&spec).unwrap(),
            enabled: true,
            created_by: "user".into(),
            next_fire_at: scheduler::now_epoch_ms() + 60_000,
            max_runs: None,
            ends_at: None,
            group_chat_config: Some(with_budget),
        },
    )
    .await
    .expect("insert budget task");
    let without_budget = serde_json::json!({
        "moderator_model_id": m0,
        "participants": [{ "name": "架构", "model_id": m1, "persona_md": "视角" }],
    })
    .to_string();
    let task_without = insert_scheduled_task(
        &fx.state.db,
        NewScheduledTask {
            project_id: fx.project_id.clone(),
            target_session_id: None,
            target_mode: crate::db::scheduled_tasks::target_modes::GROUP_CHAT.into(),
            model_id: None,
            name: "不带预算".into(),
            prompt: "复盘".into(),
            schedule_json: serde_json::to_string(&spec).unwrap(),
            enabled: true,
            created_by: "user".into(),
            next_fire_at: scheduler::now_epoch_ms() + 60_000,
            max_runs: None,
            ends_at: None,
            group_chat_config: Some(without_budget),
        },
    )
    .await
    .expect("insert budget-less task");
    // due 判定走 schedule(interval)vs created_at —— 回填 6h 让两个任务
    // 都到点(seed_group_task 同款手法;next_fire_at 只是展示列)。
    for id in [&task_with.id, &task_without.id] {
        sqlx::query("UPDATE scheduled_tasks SET created_at = ? WHERE id = ?")
            .bind(scheduler::now_epoch_ms() - 6 * 3_600_000)
            .bind(id)
            .execute(&fx.state.db)
            .await
            .expect("backdate created_at");
    }

    let mut pending = HashMap::new();
    scheduler::scheduler_tick(&fx.state, &mut pending).await;

    for (task, expect_budget) in [(&task_with, true), (&task_without, false)] {
        let row = task_row(&fx, &task.id).await;
        assert_eq!(
            row.last_fire_outcome.as_deref(),
            Some("started"),
            "task {}: fire accepted",
            task.name
        );
        let sid = row.last_run_session_id.clone().expect("run session");
        let loaded = crate::db::sessions::load_session(&fx.state.db, &sid)
            .await
            .unwrap()
            .expect("discussion session");
        let meta = loaded.session.metadata.expect("metadata");
        if expect_budget {
            assert_eq!(
                meta["token_budget"],
                serde_json::json!(400_000u64),
                "declared budget lands in session metadata verbatim"
            );
        } else {
            assert!(
                meta.get("token_budget").is_none(),
                "no null key for budget-less tasks: {meta}"
            );
        }
    }
}

/// 四态第 1 臂 busy:上一场仍在跑(内存注册表)→ 跳过 + 审计
/// skipped_busy;消费 due 但**不计数**;last_run_session_id 保留旧值
/// (COALESCE);不建新 session。
#[tokio::test(flavor = "multi_thread")]
async fn group_chat_task_busy_prior_session_skips_without_counting() {
    let fx = make_fixture().await;
    let task = seed_group_task(&fx, 1, "每周审议", false).await;
    let prior = attach_prior_discussion(&fx, &task.id, Some("interrupted"), Some(3)).await;
    // busy:编排器注册形态(session_active_request 含 prior)。
    fx.state
        .session_active_request
        .lock()
        .await
        .insert(prior.clone(), "rid-live".to_string());
    let sessions_before = crate::db::sessions::list_sessions(&fx.state.db, &fx.project_id)
        .await
        .unwrap()
        .len();

    let mut pending = HashMap::new();
    scheduler::scheduler_tick(&fx.state, &mut pending).await;

    let row = task_row(&fx, &task.id).await;
    assert_eq!(row.run_count, 0, "busy skip must not burn the budget");
    assert_eq!(row.last_fire_outcome.as_deref(), Some("skipped_busy"));
    assert_eq!(
        row.last_run_session_id.as_deref(),
        Some(prior.as_str()),
        "COALESCE keeps the prior session"
    );
    assert!(row.last_fired_at.is_some(), "due consumed");
    assert_eq!(
        audit_actions_for(&fx, &prior).await,
        vec![(actions::SKIPPED_BUSY.to_string(), None)]
    );
    assert_eq!(
        crate::db::sessions::list_sessions(&fx.state.db, &fx.project_id)
            .await
            .unwrap()
            .len(),
        sessions_before,
        "no new discussion opened while busy"
    );
}

/// 四态第 2 臂 interrupted + checkpoint(round<30):自动 resume。
#[tokio::test(flavor = "multi_thread")]
async fn group_chat_task_resumes_interrupted_prior_discussion() {
    let fx = make_fixture().await;
    let task = seed_group_task(&fx, 1, "每周审议", false).await;
    let prior = attach_prior_discussion(&fx, &task.id, Some("interrupted"), Some(3)).await;
    let sessions_before = crate::db::sessions::list_sessions(&fx.state.db, &fx.project_id)
        .await
        .unwrap()
        .len();

    let mut pending = HashMap::new();
    scheduler::scheduler_tick(&fx.state, &mut pending).await;

    let row = task_row(&fx, &task.id).await;
    assert_eq!(row.run_count, 1, "accepted resume counts");
    assert_eq!(row.last_fire_outcome.as_deref(), Some("resumed"));
    assert_eq!(row.last_run_session_id.as_deref(), Some(prior.as_str()));
    assert_eq!(
        audit_actions_for(&fx, &prior).await,
        vec![(actions::RESUMED_GROUP_CHAT.to_string(), None)]
    );
    assert_eq!(
        crate::db::sessions::list_sessions(&fx.state.db, &fx.project_id)
            .await
            .unwrap()
            .len(),
        sessions_before,
        "resume reuses the old discussion; no new session"
    );
}

/// 四态第 2 臂僵尸场:interrupted + checkpoint round ≥ 30(编排器死于
/// 轮帽中途,终态无人写)→ 补 finalize(error)+ recovered + 开新场;
/// 旧场 stop_reason 落 error(GUI 假「进行中」消失)。
#[tokio::test(flavor = "multi_thread")]
async fn group_chat_task_zombie_round_cap_recovers_then_opens_new() {
    let fx = make_fixture().await;
    let task = seed_group_task(&fx, 1, "每周审议", false).await;
    let prior = attach_prior_discussion(
        &fx,
        &task.id,
        Some("interrupted"),
        Some(crate::agent::group_chat_loop::MAX_ORCHESTRATION_ROUNDS as i64),
    )
    .await;

    let mut pending = HashMap::new();
    scheduler::scheduler_tick(&fx.state, &mut pending).await;

    // 旧场:finalize(error)落地。
    let prior_loaded = crate::db::sessions::load_session(&fx.state.db, &prior)
        .await
        .unwrap()
        .expect("prior row");
    assert_eq!(prior_loaded.session.stop_reason.as_deref(), Some("error"));
    assert_eq!(
        audit_actions_for(&fx, &prior).await,
        vec![(actions::RECOVERED.to_string(), None)]
    );
    // 新场:开跑 + 落账。
    let row = task_row(&fx, &task.id).await;
    assert_eq!(row.run_count, 1);
    assert_eq!(row.last_fire_outcome.as_deref(), Some("started"));
    let new_sid = row.last_run_session_id.clone().expect("new session");
    assert_ne!(new_sid, prior);
    assert_eq!(
        audit_actions_for(&fx, &new_sid).await,
        vec![(actions::FIRED_GROUP_CHAT.to_string(), None)]
    );
}

/// 四态第 3 臂停摆场:stop_reason NULL 且无 checkpoint(编排器 spawn 后
/// 首轮 checkpoint upsert 前 daemon 重启;boot sweep 不治)→ 补
/// finalize(error)+ recovered + 开新场。
#[tokio::test(flavor = "multi_thread")]
async fn group_chat_task_stale_session_recovers_then_opens_new() {
    let fx = make_fixture().await;
    let task = seed_group_task(&fx, 1, "每周审议", false).await;
    let prior = attach_prior_discussion(&fx, &task.id, None, None).await;

    let mut pending = HashMap::new();
    scheduler::scheduler_tick(&fx.state, &mut pending).await;

    let prior_loaded = crate::db::sessions::load_session(&fx.state.db, &prior)
        .await
        .unwrap()
        .expect("prior row");
    assert_eq!(
        prior_loaded.session.stop_reason.as_deref(),
        Some("error"),
        "stale session finalized with the existing error vocabulary"
    );
    assert_eq!(
        audit_actions_for(&fx, &prior).await,
        vec![(actions::RECOVERED.to_string(), None)]
    );
    let row = task_row(&fx, &task.id).await;
    assert_eq!(row.run_count, 1);
    assert_eq!(row.last_fire_outcome.as_deref(), Some("started"));
    assert_ne!(row.last_run_session_id.as_deref(), Some(prior.as_str()));
}

/// 终态旧场(group_chat_end):四态第 4 臂直接开新场(不 resume、
/// 不 finalize)。
#[tokio::test(flavor = "multi_thread")]
async fn group_chat_task_terminal_prior_session_opens_new_without_resume() {
    let fx = make_fixture().await;
    let task = seed_group_task(&fx, 1, "每周审议", false).await;
    let prior = attach_prior_discussion(&fx, &task.id, Some("group_chat_end"), None).await;

    let mut pending = HashMap::new();
    scheduler::scheduler_tick(&fx.state, &mut pending).await;

    let row = task_row(&fx, &task.id).await;
    assert_eq!(row.run_count, 1);
    assert_eq!(row.last_fire_outcome.as_deref(), Some("started"));
    assert_ne!(row.last_run_session_id.as_deref(), Some(prior.as_str()));
    // 终态旧场不被触碰(无 recovered / resumed 审计)。
    assert!(audit_actions_for(&fx, &prior).await.is_empty());
}

/// 计数矩阵之 catalog 预检臂:config 引用的模型不存在 → audit error
/// (model_missing)+ 不建场 + 不计数(due 消费)—— 防 model 被删后
/// 每周期落空壳 session。
#[tokio::test(flavor = "multi_thread")]
async fn group_chat_task_precheck_failure_skips_session_creation() {
    let fx = make_fixture().await;
    let task = seed_group_task(&fx, 1, "每周审议", true).await;
    let prior = attach_prior_discussion(&fx, &task.id, Some("group_chat_end"), None).await;
    let sessions_before = crate::db::sessions::list_sessions(&fx.state.db, &fx.project_id)
        .await
        .unwrap()
        .len();

    let mut pending = HashMap::new();
    scheduler::scheduler_tick(&fx.state, &mut pending).await;

    let row = task_row(&fx, &task.id).await;
    assert_eq!(row.run_count, 0, "precheck miss must not count");
    assert_eq!(row.last_fire_outcome.as_deref(), Some("error"));
    assert_eq!(
        crate::db::sessions::list_sessions(&fx.state.db, &fx.project_id)
            .await
            .unwrap()
            .len(),
        sessions_before,
        "no shell session created"
    );
    let audits = audit_actions_for(&fx, &prior).await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].0, actions::ERROR);
    assert!(
        audits[0]
            .1
            .as_deref()
            .unwrap_or_default()
            .contains("model_missing"),
        "reason names the missing models: {:?}",
        audits[0].1
    );
}

/// resume 兜底组:五闸拒绝(busy 竞态 / checkpoint 已删等)→ audit
/// error + 本期不动(**绝不双开场**)。经 dispatch seam 注入拒绝替身
/// 确定性构造(生产路径的五闸已在路由预检中等价覆盖)。
#[tokio::test(flavor = "multi_thread")]
async fn fire_group_chat_resume_rejected_leaves_task_alone() {
    use crate::scheduler::{fire_group_chat, GroupChatDispatch};
    let fx = make_fixture().await;
    let task = seed_group_task(&fx, 1, "每周审议", false).await;
    let prior = attach_prior_discussion(&fx, &task.id, Some("interrupted"), Some(3)).await;
    // attach 用裸 SQL 改 last_run_session_id;直调 fire_group_chat 传的
    // 是内存结构体,必须重载 DB 行,否则该字段仍是 NULL → 路由误判
    // OpenNew(经 scheduler_tick 的用例不受影响,tick 内部自取行)。
    let task = task_row(&fx, &task.id).await;
    let sessions_before = crate::db::sessions::list_sessions(&fx.state.db, &fx.project_id)
        .await
        .unwrap()
        .len();
    let spec = scheduler::parse_schedule(r#"{"kind":"interval","every_min":1}"#).unwrap();
    let dispatch = GroupChatDispatch {
        open: Box::new(|_state, _sid, _topic| {
            Box::pin(async { Ok(crate::agent::chat::ChatAcceptance::Started) })
        }),
        resume: Box::new(|_state, _sid| {
            Box::pin(async {
                Err(crate::error::AppCommandError::new(
                    crate::error::ErrorCategory::InvalidRequest,
                    "该会话有进行中的讨论,不能续跑",
                ))
            })
        }),
    };

    fire_group_chat(
        &fx.state,
        &task,
        &serde_json::from_str(task.group_chat_config.as_deref().unwrap()).unwrap(),
        1_000,
        &spec,
        scheduler::now_epoch_ms(),
        &dispatch,
    )
    .await;

    let row = task_row(&fx, &task.id).await;
    assert_eq!(row.run_count, 0, "rejected resume must not count");
    assert_eq!(row.last_fire_outcome.as_deref(), Some("error"));
    assert_eq!(
        row.last_run_session_id.as_deref(),
        Some(prior.as_str()),
        "prior session untouched"
    );
    let audits = audit_actions_for(&fx, &prior).await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].0, actions::ERROR);
    assert!(audits[0]
        .1
        .as_deref()
        .unwrap_or_default()
        .contains("resume_rejected"));
    assert_eq!(
        crate::db::sessions::list_sessions(&fx.state.db, &fx.project_id)
            .await
            .unwrap()
            .len(),
        sessions_before,
        "never opens a second discussion on rejection"
    );
}

/// resume 兜底组:interrupted 但 checkpoint 行已删(闸③必拒的前置
/// 判定)→ 同款「audit error + 不动」兜底,不开新场。
#[tokio::test(flavor = "multi_thread")]
async fn fire_group_chat_interrupted_without_checkpoint_rejects() {
    use crate::scheduler::{fire_group_chat, GroupChatDispatch};
    let fx = make_fixture().await;
    let task = seed_group_task(&fx, 1, "每周审议", false).await;
    let prior = attach_prior_discussion(&fx, &task.id, Some("interrupted"), None).await;
    // 同 resume_rejected 用例:attach 后必须重载 DB 行再直调 fire。
    let task = task_row(&fx, &task.id).await;
    let sessions_before = crate::db::sessions::list_sessions(&fx.state.db, &fx.project_id)
        .await
        .unwrap()
        .len();
    let spec = scheduler::parse_schedule(r#"{"kind":"interval","every_min":1}"#).unwrap();
    // open 若被调到即失败 —— 本路径必须 return 于 open 之前。
    let dispatch = GroupChatDispatch {
        open: Box::new(|_state, _sid, _topic| {
            Box::pin(async {
                panic!("open must not be reached on the no-checkpoint rejection arm")
            })
        }),
        resume: Box::new(|_state, _sid| {
            Box::pin(async { Ok(crate::agent::chat::ChatAcceptance::Started) })
        }),
    };

    fire_group_chat(
        &fx.state,
        &task,
        &serde_json::from_str(task.group_chat_config.as_deref().unwrap()).unwrap(),
        1_000,
        &spec,
        scheduler::now_epoch_ms(),
        &dispatch,
    )
    .await;

    let row = task_row(&fx, &task.id).await;
    assert_eq!(row.run_count, 0);
    assert_eq!(row.last_fire_outcome.as_deref(), Some("error"));
    let audits = audit_actions_for(&fx, &prior).await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].0, actions::ERROR);
    assert!(
        audits[0]
            .1
            .as_deref()
            .unwrap_or_default()
            .contains("no checkpoint"),
        "reason names the missing checkpoint: {:?}",
        audits[0].1
    );
    assert_eq!(
        crate::db::sessions::list_sessions(&fx.state.db, &fx.project_id)
            .await
            .unwrap()
            .len(),
        sessions_before
    );
}

/// 对照组红线:kill switch 对群聊档同效(`scheduled_tasks_enabled =
/// false` → tick 空转,零审计零落账)。
#[tokio::test(flavor = "multi_thread")]
async fn group_chat_task_respects_kill_switch() {
    let fx = make_fixture().await;
    let task = seed_group_task(&fx, 1, "每周审议", false).await;
    crate::db::config::set_config_value(&fx.state.db, "scheduled_tasks_enabled", "false")
        .await
        .unwrap();

    let mut pending = HashMap::new();
    scheduler::scheduler_tick(&fx.state, &mut pending).await;

    let row = task_row(&fx, &task.id).await;
    assert_eq!(row.run_count, 0);
    assert!(
        row.last_fired_at.is_none(),
        "no due consumed under the kill switch"
    );
    assert!(row.last_run_session_id.is_none());
}
