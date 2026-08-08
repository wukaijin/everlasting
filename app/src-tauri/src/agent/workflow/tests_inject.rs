#![cfg(test)]

use crate::agent::workflow::inject::*;
use crate::agent::workflow::{default_workflow, TaskStatus, WorkflowDef};
use crate::llm::types::{CacheControl, ChatMessage, ContentBlock, MessageContent, Role};

/// 1-block user message at index 0 — the canonical
/// "B5 instruction-loaded" precondition that S-B
/// requires.
fn fresh_user_message() -> ChatMessage {
    ChatMessage {
        role: Role::User,
        content: MessageContent::Blocks(vec![ContentBlock::Text {
            text: "user-typed content".to_string(),
            cache_control: Some(CacheControl::Ephemeral),
        }]),
        speaker: None,
    }
}

fn sample_ctx_with_task() -> WorkflowCtx {
    let workflow_def = default_workflow();
    WorkflowCtx {
        task_workflow_def: workflow_def.clone(),
        workflow_def,
        current_task: Some(crate::agent::workflow::TaskJson {
            id: "t1".into(),
            title: "Sample task".into(),
            slug: "sample".into(),
            status: TaskStatus::InProgress,
            created_at: "2026-07-08T00:00:00Z".into(),
            updated_at: "2026-07-08T00:00:00Z".into(),
            parent: None,
            summary: "demo".into(),
            items: vec![],
            // Step 3.3: pre-archive fixture.
            completed_at: None,
            workflow_plugin: "dev".into(),
        }),
    }
}

fn sample_ctx_no_task() -> WorkflowCtx {
    let workflow_def = default_workflow();
    WorkflowCtx {
        task_workflow_def: workflow_def.clone(),
        workflow_def,
        current_task: None,
    }
}

// --- append happy-path ----------------------------------------------

#[test]
fn append_workflow_breadcrumb_appends_block_to_user_messages_zero() {
    let mut msgs = vec![fresh_user_message()];
    let initial_block_count = match &msgs[0].content {
        MessageContent::Blocks(bs) => bs.len(),
        _ => unreachable!(),
    };
    let appended = append_workflow_breadcrumb(&mut msgs, &sample_ctx_with_task());
    assert!(appended);

    // First message still user-role Blocks; block count
    // grew by exactly one (no synthetic message added).
    assert_eq!(msgs.len(), 1);
    match &msgs[0].content {
        MessageContent::Blocks(bs) => {
            assert_eq!(bs.len(), initial_block_count + 1);
            // NEW block must be cache_control: None,
            // NOT inheriting the user's marker.
            match bs.last().unwrap() {
                ContentBlock::Text { cache_control, .. } => {
                    assert!(
                        cache_control.is_none(),
                        "breadcrumb block must NOT carry a cache_control marker (S-B)",
                    );
                }
                _ => panic!("expected text block"),
            }
        }
        _ => panic!("messages[0] content shape corrupted"),
    }
}

#[test]
fn append_workflow_breadcrumb_bootstrap_branch_when_no_current_task() {
    let mut msgs = vec![fresh_user_message()];
    let appended = append_workflow_breadcrumb(&mut msgs, &sample_ctx_no_task());
    assert!(appended);
    match &msgs[0].content {
        MessageContent::Blocks(bs) => {
            let last = bs.last().unwrap();
            match last {
                ContentBlock::Text { text, .. } => {
                    assert!(
                        text.contains("workflow-task-meta"),
                        "bootstrap hint must include the meta marker"
                    );
                    assert!(
                        text.contains("create_task"),
                        "bootstrap hint must point at the create_task tool (got: {text})"
                    );
                }
                _ => panic!("expected text block"),
            }
        }
        _ => panic!("messages[0] content shape corrupted"),
    }
}

#[test]
fn breadcrumb_text_reflects_current_state_via_workflow_def() {
    // Confirms the per-turn reflective intent: the
    // block's text mentions the dev plugin's state
    // breadcrumb for the current task's status
    // (`in_progress`).
    let mut msgs = vec![fresh_user_message()];
    append_workflow_breadcrumb(&mut msgs, &sample_ctx_with_task());
    match &msgs[0].content {
        MessageContent::Blocks(bs) => match bs.last().unwrap() {
            ContentBlock::Text { text, .. } => {
                // dev plugin's `in_progress` breadcrumb
                // references `wf-before-dev`; cheap
                // identity probe.
                assert!(
                    text.contains("wf-before-dev"),
                    "in_progress-state breadcrumb text missing: {}",
                    text
                );
            }
            _ => panic!("expected text block"),
        },
        _ => panic!("messages[0] content shape corrupted"),
    }
}

/// Step 1.4 — artifact 查阅机制:`task.json` 元数据(id/title/
/// slug/status)+ breadcrumb 文本 + prd/design/progress path pointer
/// 必须随 state breadcrumb 一起进 messages[0],让 LLM 知道当前
/// task + 全文去哪读。
#[test]
fn breadcrumb_includes_task_meta_and_artifact_paths() {
    let mut msgs = vec![fresh_user_message()];
    append_workflow_breadcrumb(&mut msgs, &sample_ctx_with_task());
    let text = match &msgs[0].content {
        MessageContent::Blocks(bs) => match bs.last().unwrap() {
            ContentBlock::Text { text, .. } => text.clone(),
            _ => panic!("expected text block"),
        },
        _ => panic!("messages[0] content shape corrupted"),
    };

    // task.json meta 全部进 breadcrumb
    assert!(
        text.contains("task_id: t1"),
        "missing task_id, got: {}",
        text
    );
    assert!(
        text.contains("title: Sample task"),
        "missing title, got: {}",
        text
    );
    assert!(text.contains("slug: sample"), "missing slug, got: {}", text);
    assert!(
        text.contains("status: in_progress"),
        "missing status, got: {}",
        text
    );

    // artifact path pointer — prd/design/progress 全文路径都给
    // LLM 走 read_file 自取,engine 不预加载
    assert!(
        text.contains(".everlasting/tasks/sample/"),
        "missing task_dir pointer, got: {}",
        text
    );
    assert!(
        text.contains("prd.md"),
        "missing prd.md path, got: {}",
        text
    );
    assert!(
        text.contains("design.md"),
        "missing design.md path, got: {}",
        text
    );
    assert!(
        text.contains("progress.md"),
        "missing progress.md path, got: {}",
        text
    );

    // meta 容器包裹(便于 LLM parse)
    assert!(
        text.contains("<workflow-task-meta>") && text.contains("</workflow-task-meta>"),
        "missing <workflow-task-meta> wrapper, got: {}",
        text
    );
}

// --- C4 (2026-07-27): review plugin bootstrap ---------------------
//
// Regression for session 99866757 — review plugin's `None`
// (no-active-task) branch rendered an EMPTY state breadcrumb
// because the fallback key was hard-coded to dev plugin's
// "planning", which doesn't exist in review's state table
// (intake/reviewing/revising/reported). The fix routes the
// fallback through `workflow_def.initial`.

/// Build a WorkflowCtx backed by the real builtin review
/// plugin definition (parsed from the same JSON users get),
/// with no active task — the exact shape of a fresh review
/// session before `create_task` is called.
fn review_ctx_no_task() -> WorkflowCtx {
    let json = crate::agent::workflow::builtin_workflow_json("review")
        .expect("builtin review workflow JSON must be registered");
    let workflow_def: WorkflowDef =
        serde_json::from_str(json).expect("builtin review workflow JSON must parse");
    WorkflowCtx {
        task_workflow_def: workflow_def.clone(),
        workflow_def,
        current_task: None,
    }
}

/// R1: review plugin's `None` branch must resolve the state
/// key from `workflow_def.initial` (= "intake"), NOT the
/// legacy hard-coded "planning". Before the fix this rendered
/// an empty breadcrumb for the entire intake phase.
#[test]
fn breadcrumb_review_plugin_none_branch_uses_initial_state_not_planning() {
    let body = breadcrumb_body(&review_ctx_no_task());
    // review's intake breadcrumb must be present — proving
    // the state key resolved to "intake" (not "planning",
    // which would have produced "").
    assert!(
        body.contains("[Wf · intake · review]"),
        "review plugin None-branch must surface the intake breadcrumb; \
         got empty or wrong state (likely the planning-key regression):\n{}",
        body
    );
    // Sanity: the dev-specific "planning" breadcrumb text
    // (which mentions wf-brainstorm) must NOT leak in.
    assert!(
        !body.contains("wf-brainstorm"),
        "dev plugin's planning breadcrumb leaked into review session:\n{}",
        body
    );
}

/// R2: the bootstrap meta block must tell the LLM which
/// plugin it's in, the current state, and the workflow-only
/// tools it has — so it stops treating a review session as
/// a plain chat.
#[test]
fn breadcrumb_review_plugin_none_branch_exposes_plugin_state_and_tools() {
    let body = breadcrumb_body(&review_ctx_no_task());
    assert!(
        body.contains("plugin: review"),
        "bootstrap meta must name the active plugin (got: {body})"
    );
    assert!(
        body.contains("state: intake"),
        "bootstrap meta must name the initial state (got: {body})"
    );
    // Workflow-only tools — mirrors the whitelist in
    // `tools::filter_tools_for_workflow` + dispatch_subagent
    // + update_checklist.
    for tool in [
        "create_task",
        "dispatch_subagent",
        "request_task_state_transition",
        "update_checklist",
    ] {
        assert!(
            body.contains(tool),
            "bootstrap meta must list workflow-only tool `{tool}` (got: {body})"
        );
    }
}

/// R3: review's intake breadcrumb must steer the LLM to read
/// the model catalog from the `dispatch_subagent` tool's enum
/// (NOT shell/SQLite), regressing the session-99866757
/// behavior where the model spent 7 turns curling the daemon.
#[test]
fn breadcrumb_review_intake_discourages_shell_model_lookup() {
    let body = breadcrumb_body(&review_ctx_no_task());
    assert!(
        body.contains("dispatch_subagent") && body.contains("不要用 shell"),
        "review intake breadcrumb must point to dispatch_subagent enum \
         and forbid shell model lookup (got: {body})"
    );
}

// --- S-B guard -----------------------------------------------------

#[test]
fn append_workflow_breadcrumb_skips_when_messages_empty() {
    let mut msgs: Vec<ChatMessage> = vec![];
    // Should NOT prepend a synthetic user message —
    // S-B forbid.
    let appended = append_workflow_breadcrumb(&mut msgs, &sample_ctx_with_task());
    assert!(!appended);
    assert!(
        msgs.is_empty(),
        "S-B: empty Vec stays empty (no synthetic prepend)"
    );
}

#[test]
fn append_workflow_breadcrumb_skips_when_first_message_is_assistant() {
    let mut msgs = vec![ChatMessage {
        role: Role::Assistant,
        content: MessageContent::Blocks(vec![ContentBlock::Text {
            text: "i'm the assistant; not a user".to_string(),
            cache_control: None,
        }]),
        speaker: None,
    }];
    let appended = append_workflow_breadcrumb(&mut msgs, &sample_ctx_with_task());
    assert!(!appended);
    assert_eq!(
        msgs.len(),
        1,
        "S-B: no synthetic prepend; assistant stays at index 0"
    );
    match &msgs[0].content {
        MessageContent::Blocks(bs) => assert_eq!(bs.len(), 1, "no block appended"),
        _ => panic!("messages[0] content shape corrupted"),
    }
}

#[test]
fn append_workflow_breadcrumb_skips_when_user_role_message_is_text_only() {
    // User-role but plain `MessageContent::Text(...)` (no
    // Blocks). The helper cannot append to a Text
    // payload; per S-B, it MUST skip rather than wrap
    // the breadcrumb in a new user message.
    let mut msgs = vec![ChatMessage {
        role: Role::User,
        content: MessageContent::Text("plain string".to_string()),
        speaker: None,
    }];
    let appended = append_workflow_breadcrumb(&mut msgs, &sample_ctx_with_task());
    assert!(!appended);
    assert_eq!(msgs.len(), 1, "S-B: no synthetic prepend");
    match &msgs[0].content {
        MessageContent::Text(s) => assert_eq!(s, "plain string"),
        _ => panic!("messages[0] content shape changed unexpectedly"),
    }
}

// --- coherence with the user-role Blocks precondition ---

#[test]
fn append_workflow_breadcrumb_preserves_existing_blocks_order() {
    let mut msgs = vec![ChatMessage {
        role: Role::User,
        content: MessageContent::Blocks(vec![
            ContentBlock::Text {
                text: "instruction banner".to_string(),
                cache_control: Some(CacheControl::Ephemeral),
            },
            ContentBlock::Text {
                text: "agent body".to_string(),
                cache_control: None,
            },
        ]),
        speaker: None,
    }];
    append_workflow_breadcrumb(&mut msgs, &sample_ctx_with_task());
    match &msgs[0].content {
        MessageContent::Blocks(bs) => {
            // Original first two blocks preserved verbatim.
            assert_eq!(bs.len(), 3);
            match &bs[0] {
                ContentBlock::Text { text, .. } => assert_eq!(text, "instruction banner"),
                _ => panic!(),
            }
            match &bs[1] {
                ContentBlock::Text { text, .. } => assert_eq!(text, "agent body"),
                _ => panic!(),
            }
            // New third block is the breadcrumb.
            match &bs[2] {
                ContentBlock::Text {
                    text,
                    cache_control,
                } => {
                    assert!(cache_control.is_none());
                    assert!(text.contains("workflow-task-meta"));
                }
                _ => panic!(),
            }
        }
        _ => panic!(),
    }
}

// --- resolve_current_task on a tempdir ---

#[tokio::test]
async fn resolve_current_task_returns_none_when_no_tasks_dir() {
    let d = tempfile::tempdir().unwrap();
    assert!(resolve_current_task(d.path()).await.is_none());
}

#[tokio::test]
async fn resolve_current_task_picks_first_unfinished_task() {
    // Setup: 3 task dirs in `.everlasting/tasks/`:
    //   - `archived-task` → status = Done (skip)
    //   - `do-this-now`   → status = Implement (pick
    //                        FIRST by lexicographic
    //                        slug sort)
    //   - `zzz-other`      → status = Planning (also
    //                         unfinished)
    // Expected: `do-this-now` wins.
    let project = tempfile::tempdir().unwrap();
    let tasks = project.path().join(".everlasting/tasks");
    for (slug, status) in [
        ("archived-task", TaskStatus::Done),
        ("do-this-now", TaskStatus::InProgress),
        ("zzz-other", TaskStatus::Planning),
    ] {
        let dir = tasks.join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        let task = crate::agent::workflow::TaskJson {
            id: format!("id-{slug}"),
            title: slug.to_string(),
            slug: slug.to_string(),
            status,
            created_at: "2026-07-08T00:00:00Z".into(),
            updated_at: "2026-07-08T00:00:00Z".into(),
            parent: None,
            summary: String::new(),
            items: vec![],
            // Step 3.3: pre-archive fixture.
            completed_at: None,
            workflow_plugin: "dev".into(),
        };
        crate::agent::workflow::write_task(project.path(), &task).unwrap();
    }

    let resolved = resolve_current_task(project.path()).await;
    let resolved = resolved.expect("at least one unfinished task exists");
    assert_eq!(resolved.slug, "do-this-now");
    assert_eq!(resolved.status, TaskStatus::InProgress);
}

#[tokio::test]
async fn resolve_current_task_returns_none_when_all_done() {
    let project = tempfile::tempdir().unwrap();
    let tasks = project.path().join(".everlasting/tasks");
    for slug in ["a-task", "b-task"] {
        let dir = tasks.join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        let task = crate::agent::workflow::TaskJson {
            id: format!("id-{slug}"),
            title: slug.to_string(),
            slug: slug.to_string(),
            status: TaskStatus::Done,
            created_at: "2026-07-08T00:00:00Z".into(),
            updated_at: "2026-07-08T00:00:00Z".into(),
            parent: None,
            summary: String::new(),
            items: vec![],
            // Step 3.3: pre-archive fixture.
            completed_at: None,
            workflow_plugin: "dev".into(),
        };
        crate::agent::workflow::write_task(project.path(), &task).unwrap();
    }
    assert!(resolve_current_task(project.path()).await.is_none());
}

#[tokio::test]
async fn resolve_current_task_skips_corrupt_task_json_and_picks_valid_one() {
    // Defensive: the per-file error swallow contract.
    // A corrupt task.json must NOT break the resolution —
    // we walk past it to the next valid one.
    let project = tempfile::tempdir().unwrap();
    let tasks = project.path().join(".everlasting/tasks");
    // bad: invalid JSON
    std::fs::create_dir_all(tasks.join("bad")).unwrap();
    std::fs::write(
        tasks.join("bad/task.json"),
        b"this is not valid json at all",
    )
    .unwrap();
    // good: valid task
    std::fs::create_dir_all(tasks.join("good")).unwrap();
    let task = crate::agent::workflow::TaskJson {
        id: "id-good".into(),
        title: "Good".into(),
        slug: "good".into(),
        status: TaskStatus::Planning,
        created_at: "2026-07-08T00:00:00Z".into(),
        updated_at: "2026-07-08T00:00:00Z".into(),
        parent: None,
        summary: String::new(),
        items: vec![],
        // Step 3.3: pre-archive fixture.
        completed_at: None,
        workflow_plugin: "dev".into(),
    };
    crate::agent::workflow::write_task(project.path(), &task).unwrap();

    let resolved = resolve_current_task(project.path()).await;
    assert!(resolved.is_some(), "should walk past bad task.json");
    assert_eq!(resolved.unwrap().slug, "good");
}
