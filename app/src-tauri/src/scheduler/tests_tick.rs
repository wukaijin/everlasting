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
            target_session_id: fx.session_id.clone(),
            name: name.to_string(),
            prompt: "汇总昨夜进展".into(),
            schedule_json: serde_json::to_string(&spec).unwrap(),
            enabled: true,
            created_by: "user".into(),
            next_fire_at: scheduler::now_epoch_ms() + step_ms,
            max_runs: None,
            ends_at: None,
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
        mark_task_fired(&fx.state.db, &task.id, fired, fired + step_ms, true)
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
            target_session_id: fx.session_id.clone(),
            name: name.to_string(),
            prompt: "跑一次就收工".into(),
            schedule_json: serde_json::to_string(&spec).unwrap(),
            enabled: true,
            created_by: "user".into(),
            next_fire_at: now + at_offset_ms.max(0),
            max_runs: None,
            ends_at: None,
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
