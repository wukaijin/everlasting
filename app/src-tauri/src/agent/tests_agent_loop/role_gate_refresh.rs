//! W1 workflow 角色门 —— 多轮 loop 中 task.json 变更后门判定刷新
//! (RULE-TEST-002,2026-08-27;PRD `08-27-rule-test-002-role-gate-it`)。
//!
//! 回归靶子:08-27 RULE-ARGS-001 迁移期间的真实活引用↔入口快照漂移
//! (`DispatchCtx::workflow_ctx` 若误接 `request.workflow_ctx` 入口快照,
//! 轮顶刷新将永远不可见)。契约:
//! - `DispatchCtx::workflow_ctx`(chat_loop.rs DispatchCtx 构造点)必须是
//!   run_chat_loop 函数域活绑定(`workflow_ctx: &workflow_ctx`);
//! - drive_turn 轮顶(drive.rs R4 块)`resolve_current_task` 把盘上最新
//!   task 刷进这份拷贝。
//!
//! 场景(单一 run_chat_loop):
//! - 盘上预置 `.everlasting/tasks/<slug>/task.json`,status=planning;
//!   入口经生产同款 `build_workflow_ctx` 解析(dev builtin def:
//!   planning→[researcher],in_progress→[implementer, checker])。
//! - 第 1 轮:model 先 write_file 把 task.json 改成 in_progress(即真实
//!   事故形态「loop 中途改盘」),再 dispatch_subagent(checker)。同一轮
//!   内门读的是**本轮 turn-top 快照**(仍是 planning)→ 必须 denial。
//!   这同时确定性锁死了「两轮之间改盘」的时序——盘面变更物理发生在
//!   第 1 轮工具段、第 2 轮刷新之前,无后台竞态。
//! - 第 2 轮:model 再次发起同一个 checker dispatch → 轮顶刷新读到
//!   in_progress → 门放行,worker 真正执行(summary 含 marker 文本)。
//! - 第 3 轮:sentinel end_turn 收尾。
//!
//! 变异验证(PRD R3):两处人工变异均使第 2 轮退化为再 denial → 本用例
//! 转红;(a) chat_loop.rs DispatchCtx 构造点改接 `&request.workflow_ctx`;
//! (b) 移除 drive.rs 轮顶 `resolve_current_task` 刷新。证据见任务目录
//! implement.md。

#![cfg(test)]

use std::sync::Arc;

use super::tests_common::{
    chat_loop_deps, chat_loop_request, make_harness, parent_role, test_messages, MockEmitter,
};
use crate::agent::chat_loop::run_chat_loop;
use crate::agent::workflow::{build_workflow_ctx, TaskJson, TaskStatus};
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, TokenUsage};

/// 门判定用可区分状态对(planning 拒 checker / in_progress 允 checker)
/// + 放行侧 worker summary 的 marker 文本。
const SLUG: &str = "gate-refresh-task";
const ALLOWED_ROLE: &str = "checker";
const WORKER_MARKER: &str = "CHECKER REPORT PASS marker";

fn fixture_task(status: TaskStatus) -> TaskJson {
    TaskJson {
        id: "gate-t1".into(),
        title: "role gate refresh fixture".into(),
        slug: SLUG.into(),
        status,
        workflow_plugin: "dev".into(),
        created_at: "2026-08-27T00:00:00Z".into(),
        updated_at: "2026-08-27T00:00:00Z".into(),
        parent: None,
        summary: String::new(),
        items: vec![],
        completed_at: None,
    }
}

fn tool_use_turn(id: &str, name: &str, input: serde_json::Value) -> MockResponse {
    MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::ToolCall {
            id: id.into(),
            name: name.into(),
            input,
        }),
        Ok(ChatEvent::Done {
            stop_reason: Some("tool_use".into()),
            usage: Some(TokenUsage::default()),
        }),
    ])
}

fn end_turn(text: &str) -> MockResponse {
    MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta { text: text.into() }),
        Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: Some(TokenUsage::default()),
        }),
    ])
}

#[tokio::test]
async fn role_gate_denies_then_allows_after_mid_loop_task_json_status_change() {
    let h = make_harness().await;

    // ---- setup: workflow session 行 + 盘上 planning task.json ----
    crate::db::sessions::set_session_workflow_enabled(&h.db, &h.session_id, true)
        .await
        .unwrap();
    crate::db::sessions::set_session_plugin_name(&h.db, &h.session_id, "dev")
        .await
        .unwrap();
    let task_dir = h.project_path.join(".everlasting").join("tasks").join(SLUG);
    std::fs::create_dir_all(&task_dir).unwrap();
    std::fs::write(
        task_dir.join("task.json"),
        serde_json::to_string_pretty(&fixture_task(TaskStatus::Planning)).unwrap(),
    )
    .unwrap();

    // 入口 ctx 用生产同款构建(IPC entry parity):必须解析出 planning task。
    let wf_ctx = build_workflow_ctx(&h.db, &h.session_id)
        .await
        .unwrap()
        .expect("workflow session must produce a WorkflowCtx");
    assert_eq!(
        wf_ctx
            .current_task
            .as_ref()
            .expect("entry resolves task")
            .status
            .as_str(),
        "planning",
        "precondition: entry snapshot is planning"
    );

    // loop 中途「模型自己改盘」的内容:同一条 task.json,status 翻到
    // in_progress(dev def 里允许 checker 的状态)。
    let mut flipped = fixture_task(TaskStatus::InProgress);
    flipped.updated_at = "2026-08-27T01:00:00Z".into();
    let flipped_json = serde_json::to_string_pretty(&flipped).unwrap();
    let task_json_rel = format!(".everlasting/tasks/{SLUG}/task.json");

    // ---- script:[parent_t1(write+denied dispatch), parent_t2(allowed
    // dispatch), worker_t1(end_turn marker), parent_t3(sentinel)] ----
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1:先改盘(in_progress),同轮再 dispatch checker。
        // 工具按序串行执行:write_file 落盘完成后,角色门读的仍是本轮
        // turn-top 快照(status=planning)→ 必须拒。写两个独立 id 便于
        // 分别断言。
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_wf_flip".into(),
                name: "write_file".into(),
                input: serde_json::json!({
                    "path": task_json_rel,
                    "content": flipped_json,
                }),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_d1_denied".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": ALLOWED_ROLE,
                    "task": "adversarial check (expected denied while planning)",
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Parent turn 2:同一角色再派一次。轮顶 resolve_current_task 刷新
        // 后 current_task.status=in_progress → 门放行 → worker 执行。
        tool_use_turn(
            "toolu_d2_allowed",
            "dispatch_subagent",
            serde_json::json!({
                "subagent": ALLOWED_ROLE,
                "task": "adversarial check (expected allowed after flip)",
            }),
        ),
        // Worker turn 1(read-only checker,共享 cwd,消费 slot 3)。
        end_turn(WORKER_MARKER),
        // Parent turn 3 sentinel:dispatch 结果回来后父循环收尾。
        end_turn("wrapped up"),
    ]));

    let emitter = Arc::new(MockEmitter::new());
    let mut request = chat_loop_request(
        vec![],
        mock.clone(),
        200_000,
        "rid-role-gate-refresh".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
    );
    request.workflow_ctx = Some(wf_ctx);

    run_chat_loop(request, chat_loop_deps(&h), parent_role(&h)).await;

    // ---- 可观测结果断言(AC2 两侧)----

    // Premise guard:mid-loop 改盘确实落了(否则放行断言前提不成立)。
    let on_disk: TaskJson =
        serde_json::from_str(&std::fs::read_to_string(task_dir.join("task.json")).unwrap())
            .unwrap();
    assert_eq!(
        on_disk.status.as_str(),
        "in_progress",
        "turn-1 write_file must flip the on-disk status before turn 2"
    );

    let results = emitter.tool_results_snapshot();
    let by_id = |id: &str| {
        results
            .iter()
            .find(|r| r.tool_use_id == id)
            .unwrap_or_else(|| panic!("tool_result for {id} must be present"))
    };

    // AC2 side A — 第 1 轮 denial 文本断言(且 worker 未执行)。
    let write_res = by_id("toolu_wf_flip");
    assert!(
        !write_res.is_error,
        "write_file flip must succeed for the allow-side premise, got: {}",
        write_res.content
    );
    let d1 = by_id("toolu_d1_denied");
    assert!(
        d1.is_error,
        "round-1 dispatch in planning state must be an error tool_result"
    );
    assert!(
        d1.content.contains("Role gate denied"),
        "round-1 denial text missing, got: {}",
        d1.content
    );
    assert!(
        d1.content.contains("planning"),
        "denial must name the gate state, got: {}",
        d1.content
    );
    assert!(
        !d1.content.contains(WORKER_MARKER),
        "round-1 worker must NOT have executed, got: {}",
        d1.content
    );

    // AC2 side B — 第 2 轮放行的可观测结果断言:门不再拒绝,且
    // checker 真正跑完(summary 携带 worker 终文)。
    let d2 = by_id("toolu_d2_allowed");
    assert!(
        !d2.is_error && !d2.content.contains("Role gate denied"),
        "round-2 same-role dispatch must be allowed after the refresh, got: {}",
        d2.content
    );
    assert!(
        d2.content.contains(WORKER_MARKER),
        "allowed dispatch must carry the executed worker's final text, got: {}",
        d2.content
    );

    // 脚本纪律:[parent t1, parent t2, worker, parent t3] = 4 次 send。
    // 变异回归形态:若第 2 轮仍被拒(worker 不执行),call_count 退化为 3。
    assert_eq!(
        mock.call_count(),
        4,
        "script slots: parent_t1 + parent_t2 + worker_t1 + parent_t3"
    );
}
