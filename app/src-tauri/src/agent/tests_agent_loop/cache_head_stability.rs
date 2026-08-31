#![cfg(test)]

//! AC1 (08-31-cache-head-volatility): wire-level head-stability
//! regression. The OpenAI-compatible path prefix-caches from byte
//! 0 with NO `cache_control` breakpoints, so the request head —
//! the `system` string + the synthetic instruction messages at
//! `messages[0..1]` — must be **byte-identical** across the turns
//! of one session, including:
//!
//! - a mid-loop workflow state transition (breadcrumb content
//!   flips — the seq-435 incident shape),
//! - a follow-up user request in the SAME session whose
//!   instruction file was edited on disk in between (the seq-437
//!   incident shape, covered by the D2 freeze).
//!
//! All per-turn volatility (breadcrumb, repo HEAD) must appear
//! ONLY in tail blocks of the request.

use std::sync::Arc;

use super::tests_common::{
    chat_loop_deps, chat_loop_request, make_harness, parent_role, test_messages, MockEmitter,
};
use crate::agent::chat_loop::run_chat_loop;
use crate::agent::workflow::{build_workflow_ctx, TaskJson, TaskStatus};
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, ChatMessage, ContentBlock, MessageContent, Role, TokenUsage};

const SLUG: &str = "cache-head-stability-task";
const INSTRUCTION_MARKER_V1: &str = "INSTRUCTION-BODY-V1";

fn fixture_task(status: TaskStatus) -> TaskJson {
    TaskJson {
        id: "cache-head-t1".into(),
        title: "head stability fixture".into(),
        slug: SLUG.into(),
        status,
        workflow_plugin: "dev".into(),
        created_at: "2026-08-31T00:00:00Z".into(),
        updated_at: "2026-08-31T00:00:00Z".into(),
        parent: None,
        summary: String::new(),
        items: vec![],
        completed_at: None,
    }
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

/// Stamp a file's mtime explicitly — the memory loader's mtime
/// fence compares `metadata().modified()`, and a plain rewrite
/// within the same test tick may keep a coarse timestamp on some
/// filesystems, which would make the "disk changed" premise
/// unverifiable.
fn write_with_mtime(path: &std::path::Path, content: &str, stamp_secs: u64) {
    std::fs::write(path, content).unwrap();
    let f = std::fs::File::options().write(true).open(path).unwrap();
    f.set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(stamp_secs))
        .unwrap();
}

/// Serialize one message to a canonical byte string for the
/// head-equality assertions (Debug format is deterministic for
/// the plain-data content types).
fn msg_bytes(m: &ChatMessage) -> String {
    format!("{:?}\n{:?}", m.role, m.content)
}

/// The per-turn state blocks we allow (and require) at the tail:
/// the workflow breadcrumb and (for git projects) the repo HEAD
/// block. Returns the concatenated tail text of a request.
fn tail_text_of(request_messages: &[ChatMessage]) -> String {
    let mut out = String::new();
    if let Some(last) = request_messages.last() {
        match &last.content {
            MessageContent::Blocks(bs) => {
                for b in bs {
                    if let ContentBlock::Text { text, .. } = b {
                        out.push_str(text);
                        out.push('\n');
                    }
                }
            }
            MessageContent::Text(t) => out.push_str(t),
        }
    }
    out
}

/// Extract the synthetic instruction message (messages[0] when
/// memory layers loaded). Fails the test when absent — the
/// premise of this suite is that a head EXISTS to keep stable.
fn instruction_head(request_messages: &[ChatMessage]) -> String {
    let first = &request_messages[0];
    assert_eq!(
        first.role,
        Role::User,
        "messages[0] must be the synthetic instruction user message"
    );
    msg_bytes(first)
}

/// The core AC1 scenario:
///
/// - Request 1 (3 turns): instruction file V1 on disk; workflow
///   session with a `planning` task. Turn 1 writes the
///   instruction file to V2 AND flips `task.json` to
///   `in_progress` (a state transition — the seq-435 shape plus
///   the seq-437 disk edit in one loop). Turns 2-3 proceed.
/// - Request 2 (same session, new user message — the seq-437
///   shape): the on-disk instruction file is V2 by now.
///
/// Assertions:
/// 1. `system` is byte-identical across ALL sends of both
///    requests (D3: no HEAD SHA inside, nothing per-turn).
/// 2. `messages[0]` (the instruction head) is byte-identical
///    across ALL sends of both requests (D1: the breadcrumb no
///    longer forks it; D2: request 2 reuses the frozen V1
///    content even though the disk now says V2).
/// 3. The state transitions ARE visible — but only in the tail:
///    request 1's later turns carry the `in_progress` breadcrumb
///    at the tail while turn 1 carried `planning`.
#[tokio::test]
async fn workflow_state_transition_and_new_request_keep_wire_head_stable() {
    let h = make_harness().await;

    // ---- shared premises: instruction file V1 + workflow session ----
    write_with_mtime(
        &h.project_path.join("CLAUDE.md"),
        &format!("# project memory\n{INSTRUCTION_MARKER_V1}\n"),
        1_000,
    );
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

    // The mid-loop disk edits the model performs in turn 1:
    // instruction file → V2, task.json status → in_progress.
    let flipped_task_json =
        serde_json::to_string_pretty(&fixture_task(TaskStatus::InProgress)).unwrap();

    // ---- script: request 1 = 3 turns, request 2 = 1 turn ----
    let mock = Arc::new(MockProvider::new(vec![
        // Request 1, turn 1: two tool calls — edit the instruction
        // file, flip the task state.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_edit_instr".into(),
                name: "write_file".into(),
                input: serde_json::json!({
                    "path": "CLAUDE.md",
                    "content": "# project memory\nINSTRUCTION-BODY-V2\n",
                }),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_flip_state".into(),
                name: "write_file".into(),
                input: serde_json::json!({
                    "path": format!(".everlasting/tasks/{SLUG}/task.json"),
                    "content": flipped_task_json,
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Request 1, turn 2: one more tool round so the head is
        // compared across a tool_result boundary after the state
        // transition.
        tool_use_turn("toolu_list", "list_dir", serde_json::json!({"path": "."})),
        // Request 1, turn 3: close the loop (end_turn).
        end_turn("done"),
        // Request 2 (new user message, same session): single turn.
        end_turn("second request done"),
    ]));

    let emitter = Arc::new(MockEmitter::new());

    // ---- request 1 ----
    let wf_ctx = build_workflow_ctx(&h.db, &h.session_id)
        .await
        .unwrap()
        .expect("workflow session must produce a WorkflowCtx");
    let mut request = chat_loop_request(
        vec![],
        mock.clone(),
        200_000,
        "rid-cache-head-r1".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
    );
    request.workflow_ctx = Some(wf_ctx);
    run_chat_loop(request, chat_loop_deps(&h), parent_role(&h)).await;

    // ---- request 2: same session, new user message ----
    let wf_ctx_2 = build_workflow_ctx(&h.db, &h.session_id)
        .await
        .unwrap()
        .expect("workflow ctx resolves for request 2");
    let mut request2 = chat_loop_request(
        vec![],
        mock.clone(),
        200_000,
        "rid-cache-head-r2".into(),
        h.session_id.clone(),
        vec![ChatMessage {
            role: Role::User,
            content: MessageContent::Text("commit".to_string()),
            speaker: None,
            attachments: None,
        }],
        emitter.clone(),
    );
    request2.workflow_ctx = Some(wf_ctx_2);
    run_chat_loop(request2, chat_loop_deps(&h), parent_role(&h)).await;

    assert_eq!(
        mock.call_count(),
        4,
        "script slots: request1 t1-t3 + request2 t1"
    );

    let sent = mock.sent_messages();
    let systems = mock.sent_systems();
    assert_eq!(sent.len(), 4);
    assert_eq!(systems.len(), 4);

    // ---- (1) system byte-stability across every send of both requests ----
    for (i, s) in systems.iter().enumerate() {
        assert_eq!(
            s, &systems[0],
            "system prompt must be byte-identical across turns/requests (send #{i} differs)"
        );
    }
    assert!(
        systems[0]
            .as_deref()
            .unwrap_or_default()
            .contains("Session ID"),
        "sanity: the system prompt is the assembled base prompt"
    );

    // ---- (2) messages[0] (instruction head) byte-stability ----
    let head0 = instruction_head(&sent[0]);
    assert!(
        sent[0]
            .iter()
            .any(|m| matches!(&m.content,
                MessageContent::Blocks(bs) if bs.iter().any(|b|
                    matches!(b, ContentBlock::Text { text, .. } if text.contains(INSTRUCTION_MARKER_V1))))),
        "premise: request 1 turn 1 must carry the V1 instruction content in the head"
    );
    for (i, req) in sent.iter().enumerate() {
        assert_eq!(
            instruction_head(req),
            head0,
            "messages[0] (instruction head) must be byte-identical across sends \
             (send #{i} differs) — breadcrumb/state/HEAD volatility leaked into the head"
        );
    }
    // The D2 freeze specifically: request 2's head still uses the
    // V1 content even though the disk file now says V2 (edited in
    // request 1's turn 1).
    let r2_head_text = match &sent[3][0].content {
        MessageContent::Blocks(bs) => bs
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>(),
        _ => panic!("request 2 head must be Blocks"),
    };
    assert!(
        r2_head_text.contains(INSTRUCTION_MARKER_V1),
        "D2 freeze: request 2's instruction head must reuse the session's first-read \
         content (V1), got: {}",
        r2_head_text
    );
    assert!(
        !r2_head_text.contains("INSTRUCTION-BODY-V2"),
        "D2 freeze: the mid-session disk edit (V2) must NOT surface in the same \
         session's later request head"
    );

    // ---- (3) the state transition IS visible — but only at the tail ----
    // Turn 1 (sent[0]): planning breadcrumb at the tail.
    let tail0 = tail_text_of(&sent[0]);
    assert!(
        tail0.contains("status: planning"),
        "turn 1 tail must carry the planning breadcrumb (got: {})",
        tail0
    );
    // Turn 2+ (sent[1]): the task.json flip is picked up by the
    // turn-top refresh → in_progress breadcrumb at the tail.
    let tail1 = tail_text_of(&sent[1]);
    assert!(
        tail1.contains("status: in_progress"),
        "post-transition turn tail must carry the in_progress breadcrumb (got: {})",
        tail1
    );
    // And crucially: the planning breadcrumb is gone from the
    // LATER tails (it is per-turn, not persisted).
    assert!(
        !tail1.contains("status: planning"),
        "the planning breadcrumb must not persist into later requests' tails"
    );
    // Request 2 (sent[3]) still gets a breadcrumb at its tail
    // (in_progress — the frozen task state on disk).
    let tail3 = tail_text_of(&sent[3]);
    assert!(
        tail3.contains("status: in_progress"),
        "request 2 tail must carry the current-state breadcrumb (got: {})",
        tail3
    );
}

/// D3 companion: the repo HEAD block rides the request tail, and
/// a git project's system prompt stays byte-stable across turns.
/// Uses `make_harness_with_git_repo` so `lookup_head_sha` returns
/// a real SHA; asserts the tail carries it in the `<repo-state>`
/// wrapper on every turn while the system strings match.
#[tokio::test]
async fn repo_state_block_rides_tail_and_system_stays_stable() {
    let h = super::tests_common::make_harness_with_git_repo().await;
    // `make_harness` seeds the project row with `is_git_repo =
    // false`; the on-disk `git init` above doesn't write the DB.
    // Flip the row so the loop's git gating (worktree line +
    // repo-state block) sees a git project, mirroring production.
    crate::db::update_project_git_metadata(&h.db, &h.project_id, true, Some("main"))
        .await
        .unwrap();

    let mock = Arc::new(MockProvider::new(vec![
        tool_use_turn("toolu_ls", "list_dir", serde_json::json!({"path": "."})),
        end_turn("done"),
    ]));
    let emitter = Arc::new(MockEmitter::new());
    run_chat_loop(
        chat_loop_request(
            vec![],
            mock.clone(),
            200_000,
            "rid-repo-state".into(),
            h.session_id.clone(),
            test_messages(),
            emitter.clone(),
        ),
        chat_loop_deps(&h),
        parent_role(&h),
    )
    .await;

    assert_eq!(mock.call_count(), 2);
    let systems = mock.sent_systems();
    assert_eq!(systems.len(), 2);
    assert_eq!(
        systems[0], systems[1],
        "D3: system prompt must be byte-stable across turns (git project)"
    );
    assert!(
        !systems[0].as_deref().unwrap_or_default().contains("HEAD"),
        "D3: the system prompt must not embed the HEAD SHA"
    );

    let sent = mock.sent_messages();
    for (i, req) in sent.iter().enumerate() {
        let tail = tail_text_of(req);
        assert!(
            tail.contains("<repo-state>") && tail.contains("current HEAD:"),
            "send #{i}: the repo-state block must ride the request tail (got tail: {})",
            tail
        );
    }
}
