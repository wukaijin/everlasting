//! F2 定时任务的调度内核(`08-28-f2-scheduled-tasks` design §4/§5)。
//!
//! 单一调度主体 = daemon 进程:`daemon/server.rs::spawn_task_scheduler`
//! 是唯一的装配 wrapper(仿 `spawn_backup_task` 形,detached;GUI Full
//! 模式零 timer 硬约束不变 —— 只有 daemon bin 调 spawn)。每 30s 一个
//! tick 跑 [`scheduler_tick`]:design §4 的单一扫描算法 ——
//!
//! ```text
//! kill switch(scheduled_tasks_enabled,fail-open)
//! → 逐 enabled 任务:
//!    · F2b gate1:run_count ≥ max_runs(若设)→ 完成落账 + 审计,continue
//!    · most_recent_due(schedule, now, max(created_at, last_fired_at ?? 0))
//!      None → 空转(catch-up 与常规触发同一算法,无独立 pass)
//!             · F2b gate2:ends_at 已过(无未消费 due)→ 完成,continue
//!      Some(due):
//!      · F2b gate3:due > ends_at(若设)→ 不 fire,完成,continue
//!      · 同 target session 本 tick 已 fire → continue(顺延,不前移基准)
//!      · message_queue_enabled=false → 审计 skipped_queue_disabled
//!      · 上次注入条目仍滞留队列 → 审计 skipped_dedup + 落账 last_fired_at=due
//!        (F2b:dedup 跳过不计数 run_count —— prompt 未送达)
//!      · fire(prompt + 注脚 + origin → chat_inner)→ 审计 fired/catchup
//!        (Err ⇒ error + reason);落账 last_fired_at=due + run_count+1
//!      · F2b gate4:run_count+1 ≥ max_runs 或 next_fire > ends_at → 完成
//! ```
//!
//! # origin 载体链(design §4.1,P0 定案)
//!
//! [`TaskOrigin`] 是「消息来源标记」的载体类型:调度器 fire 时构造
//! `TaskOrigin::Scheduled{..}` 塞进 `ChatEntry.origin` → `chat_inner`
//! 路由临界区拷入 `QueuedMessage.origin`(忙时条目由另一个请求的驱动器
//! 在 round>0 消费,请求级上下文会被丢弃,载体必须在 QueuedMessage 上)
//! → 驱动器取 `drained.last()` 经 `ChatLoopRequest.origin` 传入 →
//! `init.rs` persist 门控放宽 + metadata 信封 `scheduled` 键。随
//! `QueuedMessage` 序列化进 `list_queued_messages` 排队占位 IPC(前端
//! 「定时」徽标);**不进** chat 事件主链。
//!
//! # fire 是进程内函数调用,与用户发送同一条链
//!
//! fire 走 [`crate::agent::chat::chat_inner`](与 HTTP handler 同一
//! `_inner` 语义),「闲也入队」免费获得忙时排队 + F3 闸满排队,零新增
//! 并发控制。测试经 [`TickFire`] seam 注入替身(见 `tests_tick.rs`;
//! chat_inner 的 provider 由 catalog 解析,无法注入 mock —— 与 F3 gate
//! 测试同款结论,端到端 LLM 轮由 driver 层 origin 测试 + live 冒烟覆盖)。

pub mod compute;
pub mod tests_lost;
pub mod tests_tick;

pub use compute::{parse_schedule, ScheduleSpec};

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::db::scheduled_tasks::ScheduledTaskRow;
use crate::error::AppCommandError;
use crate::llm::types::{ChatMessage, MessageContent, Role};
use sqlx::SqlitePool;

/// 调度 tick 周期(秒)。宽限 = 2×tick。
pub const SCHEDULER_TICK_SECS: u64 = 30;

/// catch-up 判定宽限:`now - due > 60s` 记 `catchup`,否则 `fired`
/// (design §4,宽限 = 2×tick)。
pub const CATCHUP_GRACE_MS: i64 = 60_000;

/// kill switch 的 config 键(fail-open:仅字面 `"false"` 关)。tick
/// fire 判定与 LLM `schedule_task` tool 的创建侧 gate 共用此常量
/// (`08-29-schedule-task-tool` D3:同键同语义,防两处字面漂移)。
pub const SCHEDULED_TASKS_ENABLED_KEY: &str = "scheduled_tasks_enabled";

/// 消息来源标记。internally-tagged wire 形状
/// `{"kind":"scheduled","task_id":..,"task_name":..,"fired_at":..}`
/// (字段名按声明形态序列化,与 `QueuedMessage` 同为 snake_case)。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TaskOrigin {
    /// 该消息由定时任务注入。`fired_at` 是实际触发时刻(epoch ms)。
    Scheduled {
        task_id: String,
        task_name: String,
        fired_at: i64,
    },
}

/// epoch ms(与 `agent/chat.rs` / `db/scheduled_tasks.rs` 同口径)。
pub fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

/// 审计动作枚举的字符串形态(prd R3 + F2b):
/// `fired | catchup | skipped_dedup | skipped_queue_disabled | lost | error
/// | completed`。
pub mod actions {
    pub const FIRED: &str = "fired";
    pub const CATCHUP: &str = "catchup";
    pub const SKIPPED_DEDUP: &str = "skipped_dedup";
    pub const SKIPPED_QUEUE_DISABLED: &str = "skipped_queue_disabled";
    pub const LOST: &str = "lost";
    pub const ERROR: &str = "error";
    /// F2b:任务完成(达次数上限 / 越过结束日期)自动停用。
    pub const COMPLETED: &str = "completed";
}

/// F2b completed 动作的 reason 字段:哪种结束条件触发完成。
pub mod completion_reasons {
    /// 次数上限(run_count 达 max_runs)。
    pub const MAX_RUNS: &str = "max_runs";
    /// 结束日期(到期点越过 ends_at)。
    pub const END_DATE: &str = "end_date";
}

// ---------------------------------------------------------------------------
// fire seam(测试注入点)
// ---------------------------------------------------------------------------

/// 一次 fire 请求的纯数据快照(tick 内已算好的注入参数)。
pub(crate) struct FireContext {
    pub task_id: String,
    pub task_name: String,
    pub prompt: String,
    pub target_session_id: String,
    pub now_ms: i64,
}

/// fire 的异步返回 = [`crate::agent::chat::ChatAcceptance`] 或错误
/// (与 [`crate::agent::chat::chat_inner`] 的签名对齐)。
pub(crate) type FireOutcome = Result<crate::agent::chat::ChatAcceptance, AppCommandError>;

/// fire 动作的抽象:生产实现 = [`fire_via_chat_inner`](构造注入消息 +
/// `HttpSseSink` + 新 uuid request_id → chat_inner);测试注入记录替身。
/// `+ Sync`:tick future 持 `&TickFire` 跨 await 并被 `tokio::spawn`,
/// `&F: Send` 要求 `F: Sync`。
pub(crate) type TickFire = Box<
    dyn for<'a> Fn(
            &'a Arc<crate::state::AppState>,
            FireContext,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = FireOutcome> + Send + 'a>>
        + Send
        + Sync,
>;

/// 生产 fire:构造注入消息(prompt + 触发注脚 + origin)后走
/// [`crate::agent::chat::chat_inner`]。与用户发送同一条链:忙时自动入队、
/// F3 闸满排队、F6 busy 红点 / 完成 toast 全部自动生效。事件经
/// `HttpSseSink` 进 `SseRegistry` → GUI/PWA 订阅者(与
/// `daemon/routes/agent.rs` 的 chat handler 同构造)。
async fn fire_via_chat_inner(state: &Arc<crate::state::AppState>, ctx: FireContext) -> FireOutcome {
    let (message, origin) =
        build_injection_message(&ctx.task_id, &ctx.task_name, &ctx.prompt, ctx.now_ms);
    let sink: Arc<dyn crate::state::ChatEventSink> = Arc::new(crate::daemon::sse::HttpSseSink {
        registry: state.sse.clone(),
    });
    let worker_event_sink: Arc<dyn crate::agent::subagent::SubagentEventSink> =
        Arc::new(crate::daemon::sse::HttpSseSubagentSink {
            registry: state.sse.clone(),
        });
    crate::agent::chat::chat_inner(
        state,
        crate::agent::chat::ChatEntry {
            // 每次 fire 新 uuid request_id(design §4.3)。
            request_id: uuid::Uuid::new_v4().to_string(),
            session_id: ctx.target_session_id,
            messages: vec![message],
            sink,
            worker_catalog: Some(state.catalog.clone()),
            worker_event_sink,
            resend_seq: None,
            forced_dispatch: None,
            origin: Some(origin),
        },
    )
    .await
}

/// 生产 fire 实现(唯一装配)。
fn production_fire() -> TickFire {
    Box::new(|state, ctx| Box::pin(async move { fire_via_chat_inner(state, ctx).await }))
}

/// 构造注入消息(design §4.3):正文 = prompt + 触发注脚(给模型带
/// 日期上下文,`YYYY-MM-DD HH:MM` 本地时间),origin =
/// `TaskOrigin::Scheduled{..}`(fired_at = 实际触发时刻)。纯函数,
/// 单测锁定注脚格式与 origin 三键。
pub(crate) fn build_injection_message(
    task_id: &str,
    task_name: &str,
    prompt: &str,
    now_ms: i64,
) -> (ChatMessage, TaskOrigin) {
    let content = format!(
        "{prompt}\n\n（本条由定时任务「{task_name}」于 {} 自动触发）",
        compute::format_local_hhmm(now_ms)
    );
    (
        ChatMessage {
            role: Role::User,
            content: MessageContent::Text(content),
            speaker: None,
            attachments: None,
        },
        TaskOrigin::Scheduled {
            task_id: task_id.to_string(),
            task_name: task_name.to_string(),
            fired_at: now_ms,
        },
    )
}

// ---------------------------------------------------------------------------
// tick(单一扫描算法)
// ---------------------------------------------------------------------------

/// 生产入口:每 tick 由 `spawn_task_scheduler` 调用一次。
pub(crate) async fn scheduler_tick(
    state: &Arc<crate::state::AppState>,
    pending_by_task: &mut HashMap<String, String>,
) {
    scheduler_tick_with_fire(state, pending_by_task, &production_fire()).await;
}

/// tick 主体(带 fire seam)。`pending_by_task` 是驱动器进程内的
/// 「任务 → 上次 fire 的队列条目 uuid」去重表(design §4.2:uuid 仅
/// `Queued` 返回路径可得,`Started` 不记);跨 tick 持有,由
/// `spawn_task_scheduler` 的循环体持有传入。
pub(crate) async fn scheduler_tick_with_fire(
    state: &Arc<crate::state::AppState>,
    pending_by_task: &mut HashMap<String, String>,
    fire: &TickFire,
) {
    // kill switch:`scheduled_tasks_enabled`(fail-open,仅字面 "false"
    // 关 —— 同 memory_digest / tools_stub 先例)。
    let enabled =
        match crate::db::config::get_config_value(&state.db, SCHEDULED_TASKS_ENABLED_KEY).await {
            Ok(Some(v)) => v != "false",
            _ => true,
        };
    if !enabled {
        return;
    }
    let now_ms = now_epoch_ms();
    // message_queue 开关(legacy 分支对忙 session 是「取消在跑轮 + 顶替」
    // 语义,fire 必须避开,design §4)。
    let queue_enabled =
        match crate::db::config::get_config_value(&state.db, "message_queue_enabled").await {
            Ok(Some(v)) => v != "false",
            _ => true,
        };
    let tasks = match crate::db::scheduled_tasks::list_enabled_scheduled_tasks(&state.db).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(error = %e, "scheduler tick: list tasks failed (non-fatal)");
            return;
        }
    };
    if tasks.is_empty() {
        return;
    }

    let mut fired_sessions: HashSet<String> = HashSet::new();
    let mut hit_any = false;
    for task in tasks {
        let spec = match parse_schedule(&task.schedule_json) {
            Ok(s) => s,
            Err(e) => {
                // 存量行损坏(不应发生:写入时已校验)→ 跳过不 fire。
                tracing::warn!(task_id = %task.id, error = %e, "scheduler: invalid schedule, skipping");
                continue;
            }
        };
        // F2b gate 1:次数上限已达 → 完成出扫描集(正常路径第 N 次 fire
        // 后已由 gate 4 即时完成;这里兜底存量数据 / 完成写失败的场景)。
        if task.max_runs.is_some_and(|m| task.run_count >= m) {
            complete_task(&state.db, &task, completion_reasons::MAX_RUNS).await;
            continue;
        }
        let not_before = task.created_at.max(task.last_fired_at.unwrap_or(0));
        let Some(due) = compute::most_recent_due(&spec, now_ms, not_before) else {
            // F2b gate 2:ends_at 已过且无未消费 due —— 未来到期点全部
            // 越过 ends_at,任务已死,完成出扫描集(不常驻空转)。
            if task.ends_at.is_some_and(|t| now_ms > t) {
                complete_task(&state.db, &task, completion_reasons::END_DATE).await;
            }
            continue;
        };
        // F2b gate 3:最近未消费到期点已越过结束日期 → 不 fire,完成。
        // 反之 due ≤ ends_at 照常 fire(含 catchup,D9:结束日当天仍触发)。
        if task.ends_at.is_some_and(|t| due > t) {
            complete_task(&state.db, &task, completion_reasons::END_DATE).await;
            continue;
        }
        hit_any = true;
        // 同 target session 每 tick 至多 fire 一个任务,余者顺延下 tick
        // (消掉「多任务同 tick 同 session」对 RULE-QUEUE-001 的确定性
        // 触发;不前移 last_fired_at,下 tick 自然重判,design §4)。
        if fired_sessions.contains(&task.target_session_id) {
            tracing::info!(
                task_id = %task.id,
                task_name = %task.name,
                session_id = %task.target_session_id,
                "scheduler: same-session task deferred to next tick"
            );
            continue;
        }
        if !queue_enabled {
            audit_task(&state.db, &task, actions::SKIPPED_QUEUE_DISABLED, None).await;
            continue;
        }
        // 去重:上一次 fire 的条目仍在队列未被消费 → 本轮跳过(interval
        // 任务遇慢轮的堆积闸)。该 due 点标记为已消费,prompt 该轮放弃。
        if let Some(prev_uuid) = pending_by_task.get(&task.id) {
            let still_queued = crate::agent::message_queue::list_session(
                &state.message_queues,
                &task.target_session_id,
            )
            .await
            .iter()
            .any(|e| e.id == *prev_uuid);
            if still_queued {
                tracing::info!(
                    task_id = %task.id,
                    task_name = %task.name,
                    "scheduler: previous injection still queued, skipping (dedup)"
                );
                audit_task(&state.db, &task, actions::SKIPPED_DEDUP, None).await;
                // F2b:dedup 跳过不计数(prompt 未送达),仅消费 due 点。
                account(&state.db, &task, &spec, due, false).await;
                continue;
            }
        }

        // fire(宽限 = 2×tick,超宽限 = 停机错过 → catchup)。
        let action = if now_ms - due > CATCHUP_GRACE_MS {
            actions::CATCHUP
        } else {
            actions::FIRED
        };
        let ctx = FireContext {
            task_id: task.id.clone(),
            task_name: task.name.clone(),
            prompt: task.prompt.clone(),
            target_session_id: task.target_session_id.clone(),
            now_ms,
        };
        match fire(state, ctx).await {
            Ok(crate::agent::chat::ChatAcceptance::Queued { id, .. }) => {
                // uuid 仅 Queued 返回路径可得;记进去重表(§4.2)。
                pending_by_task.insert(task.id.clone(), id);
                audit_task(&state.db, &task, action, None).await;
            }
            Ok(crate::agent::chat::ChatAcceptance::Started) => {
                // 闲时条目即时被驱动器消费,无需去重(§4.2 定案,不记)。
                audit_task(&state.db, &task, action, None).await;
            }
            Err(e) => {
                // fire 失败仍落账(due 已消费,防每 tick 重试风暴),
                // error 审计附 reason 兜底可观测(design §4.3)。
                let reason = fire_error_reason(&e);
                tracing::warn!(
                    task_id = %task.id,
                    task_name = %task.name,
                    reason = %reason,
                    "scheduler: fire failed (accounted, not retried this due point)"
                );
                audit_task(&state.db, &task, actions::ERROR, Some(&reason)).await;
            }
        }
        // F2b:fire 落账计数(Queued/Started/Error 三种结局都算一次
        // 「已触发」——error 也消费了 due 且有审计可查)。
        account(&state.db, &task, &spec, due, true).await;
        // F2b gate 4:本次 fire 后已达上限 / 下一到期点越过结束日期 →
        // 即时完成(不等下 tick 扫死任务;enabled=0 使 completed 审计
        // 天然只发一次)。
        let new_run_count = task.run_count + 1;
        let next = compute::next_fire_display(&spec, due);
        let reached_max = task.max_runs.is_some_and(|m| new_run_count >= m);
        let past_end = task.ends_at.is_some_and(|t| next > t);
        if reached_max || past_end {
            let reason = if reached_max {
                completion_reasons::MAX_RUNS
            } else {
                completion_reasons::END_DATE
            };
            complete_task(&state.db, &task, reason).await;
        }
        fired_sessions.insert(task.target_session_id.clone());
    }
    if hit_any {
        tracing::debug!(count = fired_sessions.len(), "scheduler tick processed");
    }
}

/// fire 落账:`last_fired_at = due`(理论到期点,防相位漂移,design §3)
/// + 展示用 `next_fire_at` 按 due 重算写回;`count_fire` 控制 `run_count+1`
/// 与否(F2b:dedup 跳过不计数)。best-effort(失败 warn)。
async fn account(
    db: &SqlitePool,
    task: &ScheduledTaskRow,
    spec: &ScheduleSpec,
    due: i64,
    count_fire: bool,
) {
    let next = compute::next_fire_display(spec, due);
    if let Err(e) =
        crate::db::scheduled_tasks::mark_task_fired(db, &task.id, due, next, count_fire).await
    {
        tracing::warn!(task_id = %task.id, error = %e, "scheduler: accounting write failed");
    }
}

/// F2b 任务完成落账:`enabled = 0`(退出调度扫描,审计天然只发一次)
/// + `completed` 审计(附 reason:max_runs / end_date)。best-effort,
/// 与 fire 审计同语义(失败不阻断 tick)。
async fn complete_task(db: &SqlitePool, task: &ScheduledTaskRow, reason: &str) {
    if let Err(e) = crate::db::scheduled_tasks::mark_task_completed(db, &task.id).await {
        tracing::warn!(task_id = %task.id, error = %e, "scheduler: completion write failed");
    }
    tracing::info!(
        task_id = %task.id,
        task_name = %task.name,
        reason,
        "scheduler: task completed (auto-disabled)"
    );
    audit_task(db, task, actions::COMPLETED, Some(reason)).await;
}

/// fire 审计(best-effort,挂目标 session,沿 resend 审计惯例不进事务;
/// design §4.3)。payload:`{task_id, task_name, action[, reason]}`。
async fn audit_task(db: &SqlitePool, task: &ScheduledTaskRow, action: &str, reason: Option<&str>) {
    if let Err(e) = crate::agent::permissions::record_scheduled_task_audit(
        db,
        &task.target_session_id,
        &task.id,
        &task.name,
        action,
        reason,
    )
    .await
    {
        tracing::warn!(
            task_id = %task.id,
            action,
            error = %e,
            "scheduler: audit write failed (best-effort)"
        );
    }
}

/// `chat_inner` 错误 → 审计 reason。唯一可预期的 Err 是队列满(AC6),
/// 归类为 `queue_full`;其余(理论上不可达的 preflight 类)截断原文。
fn fire_error_reason(e: &AppCommandError) -> String {
    let msg = e.message.as_str();
    if msg.contains("排队已满") {
        "queue_full".to_string()
    } else {
        format!("chat_error: {}", msg.chars().take(120).collect::<String>())
    }
}

// ---------------------------------------------------------------------------
// lost 审计(Stop 语义两处清队点的 best-effort 兜底,design §4.3-5)
// ---------------------------------------------------------------------------

/// Stop 语义清队**之前**调用:对快照中带 origin 的条目逐条落
/// `lost` 审计(best-effort)。仅覆盖两处 Stop 语义清队点(驱动器
/// cancel break `agent/chat.rs` + Stop 命令 `commands/cancel.rs`);
/// `sessions.rs` 的破坏性清理三处(delete_session / clear_session_messages
/// 等)属会话/消息本身销毁,**不审计**(design §4.3-5 定案)。
/// 快照与清队之间驱动器可能并发 drain —— best-effort 可接受。
pub(crate) async fn audit_lost_queued_entries(
    db: &SqlitePool,
    session_id: &str,
    entries: &[crate::agent::message_queue::QueuedMessage],
) {
    for entry in entries {
        let Some(crate::scheduler::TaskOrigin::Scheduled {
            task_id, task_name, ..
        }) = &entry.origin
        else {
            continue;
        };
        if let Err(e) = crate::agent::permissions::record_scheduled_task_audit(
            db,
            session_id,
            task_id,
            task_name,
            actions::LOST,
            None,
        )
        .await
        {
            tracing::warn!(
                session_id = %session_id,
                task_id = %task_id,
                error = %e,
                "lost audit write failed (best-effort)"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// fire 注入消息的纯函数行为锁(design §4.3):正文 = prompt +
    /// 触发注脚(本地 `YYYY-MM-DD HH:MM`),role 恒 user,origin =
    /// `Scheduled` 三键(task_id / task_name / fired_at = 触发时刻)。
    #[test]
    fn build_injection_message_appends_footnote_and_carries_origin() {
        let now = now_epoch_ms();
        let (msg, origin) = build_injection_message("task-1", "早报", "汇总昨夜进展", now);

        assert_eq!(msg.role, Role::User);
        assert_eq!(
            msg.content,
            MessageContent::Text(format!(
                "汇总昨夜进展\n\n（本条由定时任务「早报」于 {} 自动触发）",
                compute::format_local_hhmm(now)
            )),
            "body = prompt + full-width-paren footnote with local timestamp"
        );
        assert_eq!(
            origin,
            TaskOrigin::Scheduled {
                task_id: "task-1".to_string(),
                task_name: "早报".to_string(),
                fired_at: now,
            },
            "origin carries the task_id/task_name/fired_at triple"
        );
    }
}
