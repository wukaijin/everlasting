#![cfg(test)]

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::tests_common::{make_harness, MockEmitter};
use crate::agent::chat_loop::run_chat_loop;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, ChatMessage, TokenUsage};
use crate::llm::{MessageContent, Role};

// ---------------------------------------------------------------------------
// 07-06 (am-observability-panel) A11: recall-event integration
// tests. The chat loop emits `ChatEvent::Recall` after the
// session-start FTS hit (R2b) and after each pre-tool pitfall hit
// (R2b). The tests below lock both emits end-to-end.
// ---------------------------------------------------------------------------

/// Session-start FTS recall → `ChatEvent::Recall` with
/// `source: "fts"` is emitted before the LLM stream. The
/// `MockEmitter::chat_events()` Vec carries the event so the
/// test can assert the hit's `memory_id` + `title` + `source`.
#[tokio::test]
async fn agent_loop_emits_recall_on_fts_hit() {
    use crate::db::memories::{insert_memory, MemoryInput, MemoryKind, MemoryScope, MemoryStatus};

    let h = make_harness().await;
    // Insert a candidate memory that the session-start FTS will
    // match against the test user message ("run cargo test in
    // wsl"). Use the project_id the harness seeds (it
    // matches `h.session_id`'s project).
    let project_id = sqlx::query_as::<_, (String,)>("SELECT project_id FROM sessions WHERE id = ?")
        .bind(&h.session_id)
        .fetch_one(&h.db)
        .await
        .expect("session row")
        .0;
    insert_memory(
        &h.db,
        &MemoryInput {
            scope: MemoryScope::Project,
            project_id: Some(project_id.clone()),
            kind: MemoryKind::Fact,
            status: MemoryStatus::Candidate,
            title: "WSL cargo test".into(),
            content: "set PKG_CONFIG_PATH before running cargo test in wsl".into(),
            tags: "[]".into(),
            tool_name: None,
            command_pattern: None,
            path_globs: None,
            source_session_id: Some(h.session_id.clone()),
            source_ref: None,
        },
    )
    .await
    .expect("memory insert");

    // Drive the loop with a text-only response (no tool_use).
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta { text: "ok".into() }),
        Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: Some(TokenUsage::default()),
        }),
    ])]));

    let messages = vec![ChatMessage {
        role: Role::User,
        content: MessageContent::Text("please run cargo test in wsl and check the output".into()),
        speaker: None,
        attachments: None,
    }];

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        None,
        "rid-recall-fts".into(),
        h.session_id.clone(),
        messages,
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        None,
        false,
        false,
        Some(false),
        None,
        std::sync::Arc::new(crate::agent::subagent::ThreadLocalSubagentSink), // worker_event_sink
        None,
        None,
        h.subagent_cache.clone(),
        None,
        None,
        None, // project_main_override (2026-07-29)
        h.app_data_dir.clone(),
        None,
        h.question_store.clone(),
        // W1 (Workflow integration, Phase 0 Step 0.5 — 2026-07-08):
        // workflow_ctx = None (tests don't exercise the workflow
        // breadcrumb injection seam; that lives in separate
        // `agent::workflow::inject` tests).
        None,
        None,
        None,
        h.stub_loaded.clone(),
        // F1 queue driver (2026-08-25): single-shot call site —
        // guard-owned cleanup (not a continuation round).
        false,
    )
    .await;

    // The Recall event must be present in the chat-event stream
    // and carry the inserted memory's title + the fts source
    // marker.
    let recall_events: Vec<_> = emitter
        .chat_events()
        .into_iter()
        .filter_map(|p| match p.event {
            ChatEvent::Recall { hits } => Some(hits),
            _ => None,
        })
        .collect();
    assert_eq!(
        recall_events.len(),
        1,
        "expected exactly 1 ChatEvent::Recall for the FTS hit"
    );
    let hits = &recall_events[0];
    assert_eq!(hits.len(), 1, "expected 1 hit");
    assert_eq!(hits[0].title, "WSL cargo test");
    assert_eq!(hits[0].kind, "fact");
    assert_eq!(hits[0].source, "fts");
}

/// Pre-tool pitfall recall → `ChatEvent::Recall` with
/// `source: "pitfall"` is emitted on the active-pitfall branch.
/// (Verified + full-match would emit a SoftBlock; the recall
/// event is emitted on the SoftBlock branch too, but the test
/// here exercises the more common Footnote tier to keep the
/// MockProvider / pre-tool surface focused.)
#[tokio::test]
async fn agent_loop_emits_recall_on_pitfall_hit() {
    use crate::db::memories::{insert_memory, MemoryInput, MemoryKind, MemoryScope, MemoryStatus};
    use serde_json::json;

    let h = make_harness().await;
    let project_id = sqlx::query_as::<_, (String,)>("SELECT project_id FROM sessions WHERE id = ?")
        .bind(&h.session_id)
        .fetch_one(&h.db)
        .await
        .expect("session row")
        .0;
    // Insert an ACTIVE pitfall (so recall_pitfall_with_hits will
    // surface it; verified + full-match would short-circuit via
    // SoftBlock which the test avoids by not setting
    // command_pattern / path_globs on the row).
    // The memory_id is generated inside `insert_memory` (UUIDv7);
    // we capture it for the post-run assertion via
    // `get_memory_by_id(title)`. The simplest path: insert and
    // then look up by title to get the auto-generated id.
    insert_memory(
        &h.db,
        &MemoryInput {
            scope: MemoryScope::Project,
            project_id: Some(project_id),
            kind: MemoryKind::Pitfall,
            status: MemoryStatus::Active,
            title: "WIP: do not pass --no-default-features".into(),
            content: "agent ran into a build breakage; never pass --no-default-features".into(),
            tags: "[]".into(),
            tool_name: Some("shell".into()),
            command_pattern: Some("cargo test".into()),
            path_globs: None,
            source_session_id: Some(h.session_id.clone()),
            source_ref: None,
        },
    )
    .await
    .expect("pitfall insert");

    // The user prompt's recall_text is empty (no FTS matches
    // against a fact with title "WIP: do not pass" + content
    // about cargo — irrelevant to the user message below), so
    // the only Recall event on the stream is the pre-tool
    // pitfall one.
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: shell tool_use → triggers pre-tool pitfall recall.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "running".into(),
            }),
            Ok(ChatEvent::ToolCall {
                id: "tu-pitfall".into(),
                name: "shell".into(),
                input: json!({"command": "cargo test --no-default-features"}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Turn 2: text-only Done.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "done".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    let messages = vec![ChatMessage {
        role: Role::User,
        content: MessageContent::Text("run cargo test please".into()),
        speaker: None,
        attachments: None,
    }];

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        None,
        "rid-recall-pitfall".into(),
        h.session_id.clone(),
        messages,
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        None,
        false,
        false,
        Some(false),
        None,
        std::sync::Arc::new(crate::agent::subagent::ThreadLocalSubagentSink), // worker_event_sink
        None,
        None,
        h.subagent_cache.clone(),
        None,
        None,
        None, // project_main_override (2026-07-29)
        h.app_data_dir.clone(),
        None,
        h.question_store.clone(),
        // W1 (Workflow integration, Phase 0 Step 0.5 — 2026-07-08):
        // workflow_ctx = None (tests don't exercise the workflow
        // breadcrumb injection seam; that lives in separate
        // `agent::workflow::inject` tests).
        None,
        None,
        None,
        h.stub_loaded.clone(),
        // F1 queue driver (2026-08-25): single-shot call site —
        // guard-owned cleanup (not a continuation round).
        false,
    )
    .await;

    // The Recall event must be present with `source: "pitfall"`.
    // (`bump_hit_count` runs best-effort + fire-and-forget; the
    // event is emitted synchronously from the pre-tool seam so
    // it lands on the chat-event stream in turn 1.)
    let pitfall_recalls: Vec<_> = emitter
        .chat_events()
        .into_iter()
        .filter_map(|p| match p.event {
            ChatEvent::Recall { hits } => {
                if hits.iter().all(|h| h.source == "pitfall") {
                    Some(hits)
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();
    assert!(
        !pitfall_recalls.is_empty(),
        "expected at least one ChatEvent::Recall with source=pitfall"
    );
    let hits = pitfall_recalls.last().unwrap();
    // The inserted pitfall's `memory_id` is a UUIDv7 generated
    // inside `insert_memory`; we assert on the row's kind +
    // title instead of the id (the id is the join key, but the
    // title + kind are the user-visible surface the chip renders).
    let hit = hits
        .iter()
        .find(|h| h.title == "WIP: do not pass --no-default-features")
        .expect("inserted pitfall must appear in the recall event");
    assert_eq!(hit.kind, "pitfall");
}
