#![cfg(test)]

//! lost 审计边界(F2 design §4.3-5):仅覆盖 Stop 语义的**两处**清队点
//! —— 驱动器 cancel break(由 `agent/tests_message_queue.rs` 的
//! driver 层测试覆盖)+ Stop 命令 `cancel_chat_inner`(本文件);
//! `sessions.rs` 的破坏性清理(delete_session 等)属会话本身销毁,
//! **不**审计 —— 本文件钉住该负向边界。
//!
//! TempDir 字段后于 state 声明(字段按声明序 Drop,db 池先销毁)。

use std::sync::Arc;

use crate::llm::types::{ChatMessage, MessageContent, Role};
use crate::state::AppState;

struct LostFixture {
    state: Arc<AppState>,
    session_id: String,
    /// TempDir guard —— 声明在最后,Drop 晚于 state(db 池)。
    _tmp: tempfile::TempDir,
}

async fn make_fixture() -> LostFixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);
    crate::db::create_project(
        &state.db,
        "lost-test-project",
        "/tmp/lost-test",
        false,
        None,
    )
    .await
    .expect("create_project");
    let project = crate::db::list_projects(&state.db, false)
        .await
        .expect("list_projects")
        .into_iter()
        .find(|p| p.name == "lost-test-project")
        .expect("project row");
    let session_id = uuid::Uuid::new_v4().to_string();
    crate::db::create_session(
        &state.db,
        &session_id,
        &project.id,
        "/tmp/lost-test",
        "mock-model",
        None,
        None,
        None,
    )
    .await
    .expect("create_session");
    LostFixture {
        state,
        session_id,
        _tmp: tmp,
    }
}

fn user_msg(text: &str) -> ChatMessage {
    ChatMessage {
        role: Role::User,
        content: MessageContent::Text(text.to_string()),
        speaker: None,
        attachments: None,
    }
}

fn sched_origin(task_id: &str) -> crate::scheduler::TaskOrigin {
    crate::scheduler::TaskOrigin::Scheduled {
        task_id: task_id.to_string(),
        task_name: "定时早报".to_string(),
        fired_at: 123,
    }
}

/// 直构队列条目(可指定 uuid + origin)。
fn entry(
    id: &str,
    text: &str,
    origin: Option<crate::scheduler::TaskOrigin>,
) -> crate::agent::message_queue::QueuedMessage {
    crate::agent::message_queue::QueuedMessage {
        id: id.to_string(),
        message: user_msg(text),
        enqueued_at: 0,
        priority: 0,
        origin,
    }
}

/// 目标 session 的 scheduled_task_fired 审计 payload 列表(按写入序)。
async fn audit_payloads(fx: &LostFixture) -> Vec<serde_json::Value> {
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
            serde_json::from_str(&payload).ok()
        })
        .collect()
}

// --- Stop 命令清队:带 origin 条目补 lost 审计,手动条目不审计 ---

#[tokio::test(flavor = "multi_thread")]
async fn cancel_chat_inner_audits_lost_only_for_origin_entries() {
    let fx = make_fixture().await;
    {
        let mut map = fx.state.message_queues.lock().await;
        let q = map.entry(fx.session_id.clone()).or_default();
        q.push_back(entry(
            "sched-1",
            "scheduled body",
            Some(sched_origin("task-1")),
        ));
        q.push_back(entry("manual-1", "manual body", None));
    }
    // Stop 命令的 rid → session 反查入口。
    fx.state
        .session_active_request
        .lock()
        .await
        .insert(fx.session_id.clone(), "rid-stop".to_string());

    let outcome = crate::commands::cancel::cancel_chat_inner(&fx.state, "rid-stop".to_string())
        .await
        .expect("cancel_chat_inner");
    assert_eq!(outcome.cleared_queued, 2, "both entries cleared");
    assert!(
        crate::agent::message_queue::list_session(&fx.state.message_queues, &fx.session_id)
            .await
            .is_empty(),
        "queue emptied by Stop"
    );

    let payloads = audit_payloads(&fx).await;
    assert_eq!(payloads.len(), 1, "exactly the origin entry is audited");
    assert_eq!(payloads[0]["action"], "lost");
    assert_eq!(payloads[0]["task_id"], "task-1");
    assert_eq!(payloads[0]["task_name"], "定时早报");
}

// --- 未知 rid 的 Stop(流已结束):反查不到 session → 清不了队,
// --- 也无 lost 可审(幂等 no-op 契约不破坏)。

#[tokio::test(flavor = "multi_thread")]
async fn cancel_chat_inner_without_session_mapping_is_silent_noop() {
    let fx = make_fixture().await;
    {
        let mut map = fx.state.message_queues.lock().await;
        let q = map.entry(fx.session_id.clone()).or_default();
        q.push_back(entry("sched-1", "body", Some(sched_origin("task-1"))));
    }
    let outcome = crate::commands::cancel::cancel_chat_inner(&fx.state, "rid-gone".to_string())
        .await
        .expect("cancel_chat_inner");
    assert!(!outcome.cancelled);
    assert_eq!(outcome.cleared_queued, 0, "no session mapping → no clear");
    assert!(audit_payloads(&fx).await.is_empty());
}

// --- 负向边界:sessions.rs 破坏性清理不审计(design §4.3-5)---

#[tokio::test(flavor = "multi_thread")]
async fn delete_session_clears_queue_without_lost_audit() {
    let fx = make_fixture().await;
    {
        let mut map = fx.state.message_queues.lock().await;
        let q = map.entry(fx.session_id.clone()).or_default();
        q.push_back(entry("sched-1", "body", Some(sched_origin("task-1"))));
    }
    fx.state
        .session_active_request
        .lock()
        .await
        .insert(fx.session_id.clone(), "rid-active".to_string());

    crate::commands::sessions::delete_session_inner(&fx.state, fx.session_id.clone())
        .await
        .expect("delete_session");

    assert!(
        crate::agent::message_queue::list_session(&fx.state.message_queues, &fx.session_id)
            .await
            .is_empty(),
        "destructive cleanup still empties the queue"
    );
    assert!(
        audit_payloads(&fx).await.is_empty(),
        "session destruction must NOT write lost audit (design §4.3-5: \
         任务已随 FK 级联,挂靠点不复存在)"
    );
}
