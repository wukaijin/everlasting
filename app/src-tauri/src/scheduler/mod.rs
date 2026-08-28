//! F2 定时任务的调度内核(`08-28-f2-scheduled-tasks` design §4/§5)。
//!
//! 单一调度主体 = daemon 进程:`daemon/server.rs::spawn_task_scheduler`
//! 是唯一的装配 wrapper(仿 `spawn_backup_task` 形,detached;GUI Full
//! 模式零 timer 硬约束不变 —— 只有 daemon bin 调 spawn)。每 30s 一个
//! tick 跑 [`scheduler_tick`]:design §4 的单一扫描算法 ——
//!
//! ```text
//! kill switch(scheduled_tasks_enabled,fail-open)
//! → 逐 enabled 任务:most_recent_due(schedule, now, max(created_at, last_fired_at ?? 0))
//!    None → 空转(catch-up 与常规触发同一算法,无独立 pass)
//!    Some(due):
//!      · 同 target session 本 tick 已 fire → continue(顺延,不前移基准)
//!      · message_queue_enabled=false → 审计 skipped_queue_disabled
//!      · 上次注入条目仍滞留队列 → 审计 skipped_dedup + 落账 last_fired_at=due
//!      · fire(prompt + 注脚 + origin → chat_inner)→ 审计 fired/catchup
//!        (Err ⇒ error + reason);落账 last_fired_at=due + 展示值写回
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

/// 审计动作枚举的字符串形态(prd R3):
/// `fired | catchup | skipped_dedup | skipped_queue_disabled | lost | error`。
pub mod actions {
    pub const FIRED: &str = "fired";
    pub const CATCHUP: &str = "catchup";
    pub const SKIPPED_DEDUP: &str = "skipped_dedup";
    pub const SKIPPED_QUEUE_DISABLED: &str = "skipped_queue_disabled";
    pub const LOST: &str = "lost";
    pub const ERROR: &str = "error";
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
        match crate::db::config::get_config_value(&state.db, "scheduled_tasks_enabled").await {
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
        let not_before = task.created_at.max(task.last_fired_at.unwrap_or(0));
        let Some(due) = compute::most_recent_due(&spec, now_ms, not_before) else {
            continue;
        };
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
                account(&state.db, &task, &spec, due).await;
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
        account(&state.db, &task, &spec, due).await;
        fired_sessions.insert(task.target_session_id.clone());
    }
    if hit_any {
        tracing::debug!(count = fired_sessions.len(), "scheduler tick processed");
    }
}

/// fire 落账:`last_fired_at = due`(理论到期点,防相位漂移,design §3)
/// + 展示用 `next_fire_at` 按 due 重算写回。best-effort(失败 warn)。
async fn account(db: &SqlitePool, task: &ScheduledTaskRow, spec: &ScheduleSpec, due: i64) {
    let next = compute::next_fire_display(spec, due);
    if let Err(e) = crate::db::scheduled_tasks::mark_task_fired(db, &task.id, due, next).await {
        tracing::warn!(task_id = %task.id, error = %e, "scheduler: accounting write failed");
    }
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
