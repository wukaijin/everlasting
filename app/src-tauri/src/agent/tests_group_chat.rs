//! Group-chat orchestration integration tests (08-04 rewrite,
//! `08-04-group-chat-orchestration-rewrite`).
//!
//! Drives the full multi-round flow through `run_group_chat_loop` with
//! scripted `MockProvider`s (no HTTP, no manual UI):
//!
//! ```text
//! moderator(nominate M1) → M1 → moderator(nominate M2) → M2 →
//! moderator(end_discussion)
//! ```
//!
//! This exercises the three intertwined defects the rewrite fixes:
//!   1. cross-speaker duplicate `tool_result` rows (each `tool_use_id`
//!      must have exactly one user-role `tool_result` row in the DB),
//!   2. participant identity confusion (a participant's transcript must
//!      NOT contain the moderator's arbitration tool interaction),
//!   3. participant arbitration tools (already fixed by `participant_tool_defs`).
//!
//! See `.trellis/tasks/08-04-group-chat-orchestration-rewrite/design.md`
//! (D-A participant_view filtering / D-B reload retention / D-D entry
//! guard / D-F old heuristic guard replaced).

#![cfg(test)]

use std::collections::HashMap;
use std::sync::Arc;

use sqlx::Row;
use tokio_util::sync::CancellationToken;

use super::tests_common::{make_harness, test_messages, MockEmitter, TestHarness};
use crate::agent::chat_loop::run_chat_loop;
use crate::agent::group_chat::{GroupChatCtx, ParticipantConfig};
use crate::agent::group_chat_loop::run_group_chat_loop;
use crate::db;
use crate::llm::error::LlmError;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, ChatMessage, TokenUsage};
use crate::llm::{ContentBlock, MessageContent, Role};
use crate::state::ProviderCatalog;

fn ok_evt(ev: ChatEvent) -> Result<ChatEvent, LlmError> {
    Ok(ev)
}

/// Terminal `Done { stop_reason: Some("end_turn") }`.
fn end_turn() -> Result<ChatEvent, LlmError> {
    ok_evt(ChatEvent::Done {
        stop_reason: Some("end_turn".to_string()),
        usage: Some(TokenUsage::default()),
    })
}

/// Terminal `Done { stop_reason: Some("tool_use") }` — signals the loop
/// to execute the turn's tool calls and continue (moderator tool_use
/// round = 2 sends: tool_use → execute → second send → text/end_turn).
fn tool_use_stop() -> Result<ChatEvent, LlmError> {
    ok_evt(ChatEvent::Done {
        stop_reason: Some("tool_use".to_string()),
        usage: Some(TokenUsage::default()),
    })
}

fn text_turn(text: &str) -> MockResponse {
    MockResponse::Events(vec![
        ok_evt(ChatEvent::Start),
        ok_evt(ChatEvent::Delta {
            text: text.to_string(),
        }),
        end_turn(),
    ])
}

fn tool_turn(id: &str, name: &str, input: serde_json::Value) -> MockResponse {
    MockResponse::Events(vec![
        ok_evt(ChatEvent::Start),
        ok_evt(ChatEvent::ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            input,
        }),
        tool_use_stop(),
    ])
}

/// Moderator tool round (single turn, 08-04 follow-up): one `send`
/// carries the moderator's remark + the arbitration `ToolCall`. With
/// `max_turns=Some(1)` the turn ends right after the tool_result — no
/// second send, no "已把话筒交给 X" filler.
fn mod_tool_turn(id: &str, name: &str, input: serde_json::Value, text: &str) -> MockResponse {
    MockResponse::Events(vec![
        ok_evt(ChatEvent::Start),
        ok_evt(ChatEvent::Delta {
            text: text.to_string(),
        }),
        ok_evt(ChatEvent::ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            input,
        }),
        tool_use_stop(),
    ])
}

const M1_PERSONA: &str = "<M1 persona>";

/// Group-chat harness: a fresh `TestHarness` + a session marked
/// `group_chat` with participants metadata (`{participants: [...]}`),
/// mirroring what `build_group_chat_ctx` parses at IPC entry.
async fn make_group_chat_harness() -> (TestHarness, String) {
    let h = make_harness().await;
    let gc_session_id = uuid::Uuid::new_v4().to_string();
    let metadata = serde_json::json!({
        "participants": [
            {"name": "M1", "model": "m1", "persona_md": M1_PERSONA},
            {"name": "M2", "model": "m2"}
        ]
    });
    db::create_session(
        &h.db,
        &gc_session_id,
        &h.project_id,
        h.project_path.to_str().unwrap(),
        "moderator",       // session `model` (display name)
        Some("moderator"), // `model_id` = the ProviderCatalog key
        Some("group_chat"),
        Some(&metadata.to_string()),
    )
    .await
    .expect("create group_chat session");
    (h, gc_session_id)
}

/// The three scripted providers (moderator / m1 / m2) + the catalog
/// `worker_catalog` is resolved from.
struct GroupChatMocks {
    moderator: Arc<MockProvider>,
    m1: Arc<MockProvider>,
    m2: Arc<MockProvider>,
    catalog: Option<Arc<tokio::sync::RwLock<ProviderCatalog>>>,
}

/// Script the full flow. NOTE on the moderator send count (08-04
/// follow-up): the moderator runs `max_turns=Some(1)`, so ONE tool
/// round = ONE `send` carrying remark + `ToolCall`; the turn ends
/// right after the tool_result (no second send). Nominate M1 +
/// nominate M2 + end_discussion = 3 rounds = 3 sends. PRD AC3
/// ("participant 彼此的发言") requires both participants to speak, so
/// the moderator must nominate both.
fn script_group_chat_mocks() -> GroupChatMocks {
    let moderator = Arc::new(MockProvider::new(vec![
        // Round 0: nominate M1 (arbitration tool), single send.
        mod_tool_turn(
            "c1",
            "nominate_speaker",
            serde_json::json!({"name": "M1"}),
            "主持人发言",
        ),
        // Round 1: nominate M2.
        mod_tool_turn(
            "c2",
            "nominate_speaker",
            serde_json::json!({"name": "M2"}),
            "主持人:请 M2",
        ),
        // Round 2: end_discussion.
        mod_tool_turn("c3", "end_discussion", serde_json::json!({}), "主持人:结束"),
    ]));
    let m1 = Arc::new(MockProvider::new(vec![text_turn("我是 M1")]));
    let m2 = Arc::new(MockProvider::new(vec![text_turn("我是 M2")]));

    let mut catalog: ProviderCatalog = HashMap::new();
    catalog.insert("moderator".to_string(), moderator.clone());
    catalog.insert("m1".to_string(), m1.clone());
    catalog.insert("m2".to_string(), m2.clone());
    let catalog = Arc::new(tokio::sync::RwLock::new(catalog));

    GroupChatMocks {
        moderator,
        m1,
        m2,
        catalog: Some(catalog),
    }
}

fn group_chat_ctx() -> GroupChatCtx {
    GroupChatCtx {
        participants: vec![
            ParticipantConfig {
                name: "M1".to_string(),
                model: "m1".to_string(),
                persona_md: Some(M1_PERSONA.to_string()),
                order: None,
            },
            ParticipantConfig {
                name: "M2".to_string(),
                model: "m2".to_string(),
                persona_md: None,
                order: None,
            },
        ],
        moderator_model_id: "moderator".to_string(),
    }
}

/// AC1 + AC2 + AC3 + AC4 + AC5 — the full multi-round integration test.
///
/// Assertions:
/// - no `ChatEvent::Error` (AC1),
/// - each `tool_use_id` has exactly 1 user-role `tool_result` row and
///   the human "hello" message is persisted exactly once (AC2),
/// - M1 / M2 `sent_messages` contain no nominate_speaker /
///   end_discussion tool_use / tool_result blocks, and M2's view
///   contains M1's remark (AC3),
/// - system prompts: M1 = persona, M2 = default participant template,
///   moderator = moderator template (AC4),
/// - moderator `sent_messages` contains its own nominate tool_use (AC5).
#[tokio::test]
async fn group_chat_full_multi_round_flow_no_errors_no_duplicate_tool_results() {
    let (h, gc_session_id) = make_group_chat_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mocks = script_group_chat_mocks();

    run_group_chat_loop(
        crate::tools::builtin_tools(),
        200_000,
        "rid-gc".to_string(),
        gc_session_id.clone(),
        test_messages(), // [user "hello"] — round-0 tail, genuinely new
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
        mocks.catalog.clone(),
        Arc::new(crate::agent::subagent::ThreadLocalSubagentSink),
        h.subagent_cache.clone(),
        h.app_data_dir.clone(),
        h.question_store.clone(),
        group_chat_ctx(),
    )
    .await;

    // ---- AC1: no Error events (a 400 would land here as
    // `ChatEvent::Error`, e.g. from duplicate tool_results). ----
    assert_eq!(
        emitter.error_event_count(),
        0,
        "full multi-round group chat must run without any ChatEvent::Error"
    );

    // ---- AC2: DB has exactly one tool_result row per tool_use_id and
    // the round-0 human message is persisted exactly once. ----
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT role, content FROM messages WHERE session_id = ? ORDER BY seq")
            .bind(&gc_session_id)
            .fetch_all(&h.db)
            .await
            .expect("fetch messages");
    let mut c1 = 0i64;
    let mut c2 = 0i64;
    let mut c3 = 0i64;
    let mut hello_rows = 0i64;
    for (role, content_json) in &rows {
        let content: MessageContent =
            serde_json::from_str(content_json).unwrap_or(MessageContent::Text(String::new()));
        match content {
            MessageContent::Text(t) if role == "user" && t == "hello" => hello_rows += 1,
            MessageContent::Blocks(blocks) => {
                for b in blocks {
                    if let ContentBlock::ToolResult { tool_use_id, .. } = b {
                        match tool_use_id.as_str() {
                            "c1" => c1 += 1,
                            "c2" => c2 += 1,
                            "c3" => c3 += 1,
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }
    assert_eq!(
        c1, 1,
        "tool_use c1 (nominate M1) must have exactly 1 tool_result row"
    );
    assert_eq!(
        c2, 1,
        "tool_use c2 (nominate M2) must have exactly 1 tool_result row"
    );
    assert_eq!(
        c3, 1,
        "tool_use c3 (end_discussion) must have exactly 1 tool_result row"
    );
    assert_eq!(
        hello_rows, 1,
        "round-0 human message must be persisted exactly once"
    );

    // ---- Terminal signal (08-04 follow-up "终止事件 + 逐轮流式"): the
    // orchestrator must emit exactly one terminal `Done { stop_reason:
    // "group_chat_end" }` so the frontend keeps the request alive across
    // the inner per-speaker turns and only finalizes on this signal.
    let chat_events = emitter.chat_events();
    let group_chat_end_dones = chat_events
        .iter()
        .filter(|e| {
            matches!(
                &e.event,
                ChatEvent::Done {
                    stop_reason: Some(s),
                    ..
                } if s == "group_chat_end"
            )
        })
        .count();
    assert_eq!(
        group_chat_end_dones, 1,
        "exactly one terminal Done{{group_chat_end}} must be emitted"
    );

    // ---- Speaker 事件 (08-04 follow-up "实时 speaker 标识"): the
    // orchestrator must announce each upcoming speaker BEFORE its turn
    // so the frontend can stamp the placeholder's speaker chip live.
    // 3 moderator rounds + M1 + M2 = 5 announcements.
    let speaker_events: Vec<String> = chat_events
        .iter()
        .filter_map(|e| match &e.event {
            ChatEvent::Speaker { speaker } => Some(speaker.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        speaker_events,
        vec![
            "moderator".to_string(),
            "M1".to_string(),
            "moderator".to_string(),
            "M2".to_string(),
            "moderator".to_string(),
        ],
        "one Speaker event per inner turn, in turn order"
    );

    // ---- call counts (mock script must match the real loop's turn
    // structure exactly; exhaustion = InvalidRequest → test fails). ----
    assert_eq!(
        mocks.moderator.call_count(),
        3,
        "moderator: 3 single-turn arbitration rounds (max_turns=1)"
    );
    assert_eq!(mocks.m1.call_count(), 1, "M1 speaks once");
    assert_eq!(mocks.m2.call_count(), 1, "M2 speaks once");

    // ---- AC3: participant views must NOT contain arbitration tool
    // blocks, and M2's view must include M1's remark. ----
    let m1_view = mocks.m1.sent_messages();
    assert!(
        !m1_view.is_empty(),
        "M1 must have sent at least one request"
    );
    let m1_text = m1_view
        .iter()
        .flat_map(|msgs| msgs.iter())
        .map(|m| m.content.to_text())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        m1_text.contains("主持人发言"),
        "M1 must see the moderator's text, got: {m1_text:?}"
    );
    assert!(
        !m1_text.contains("nominate_speaker"),
        "M1 must not see nominate_speaker, got: {m1_text:?}"
    );
    assert!(
        !m1_text.contains("end_discussion"),
        "M1 must not see end_discussion, got: {m1_text:?}"
    );
    assert!(
        !has_tool_block(&m1_view),
        "M1's transcript must contain no tool_use / tool_result blocks at all (arbitration pairs filtered)"
    );

    let m2_view = mocks.m2.sent_messages();
    assert!(
        !m2_view.is_empty(),
        "M2 must have sent at least one request"
    );
    let m2_text = m2_view
        .iter()
        .flat_map(|msgs| msgs.iter())
        .map(|m| m.content.to_text())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        m2_text.contains("我是 M1"),
        "M2 must see M1's remark (AC3 彼此发言), got: {m2_text:?}"
    );
    assert!(
        !m2_text.contains("nominate_speaker"),
        "M2 must not see nominate_speaker, got: {m2_text:?}"
    );
    assert!(
        !m2_text.contains("end_discussion"),
        "M2 must not see end_discussion, got: {m2_text:?}"
    );
    assert!(
        !has_tool_block(&m2_view),
        "M2's transcript must contain no tool blocks"
    );

    // ---- AC4: system prompts. M1 = its persona + identity-guard
    // block; M2 (no persona) = default participant template +
    // identity-guard block; moderator = the moderator template. ----
    let m1_system = mocks.m1.sent_systems()[0]
        .clone()
        .expect("M1 system prompt");
    assert!(
        m1_system.starts_with(M1_PERSONA),
        "M1's system prompt must start with its persona, got: {m1_system:?}"
    );
    assert!(
        m1_system.contains("Group-chat roles (read carefully)"),
        "M1's system prompt must carry the identity-guard block, got: {m1_system:?}"
    );
    assert!(
        m1_system.contains("The moderator's messages are NOT yours"),
        "M1 must be told it is not the moderator, got: {m1_system:?}"
    );
    let m2_system = mocks.m2.sent_systems()[0]
        .clone()
        .expect("M2 system prompt");
    assert!(
        m2_system.starts_with(
            "You are M2, a participant in a group chat discussion led by a moderator. \
             You can see what everyone else has said. Respond to the topic and to \
             other participants — agree, disagree, build on, or question their points. \
             Be concise and substantive."
        ),
        "M2 (no persona) must get the default participant template, got: {m2_system:?}"
    );
    assert!(
        m2_system.contains("Group-chat roles (read carefully)"),
        "M2's system prompt must carry the identity-guard block, got: {m2_system:?}"
    );
    let mod_system = mocks.moderator.sent_systems()[0]
        .clone()
        .expect("moderator system prompt");
    assert!(
        mod_system.contains("You are the MODERATOR of a group chat discussion"),
        "moderator must get the moderator template, got: {mod_system:?}"
    );
    assert!(
        mod_system.contains("- M1 (model: m1)") && mod_system.contains("- M2 (model: m2)"),
        "moderator template must list the roster"
    );

    // ---- AC5: moderator sees its OWN arbitration tool_use across
    // rounds (it must NOT be filtered from the moderator's view). ----
    let mod_messages = mocks.moderator.sent_messages();
    assert!(
        mod_messages.len() >= 3,
        "moderator must send at least its round-1 + round-2 + round-3 requests"
    );
    let mod_has_nominate = mod_messages.iter().flat_map(|msgs| msgs.iter()).any(|m| {
        matches!(
            &m.content,
            MessageContent::Blocks(blocks)
                if blocks.iter().any(|b| matches!(
                    b,
                    ContentBlock::ToolUse { name, .. } if name == "nominate_speaker"
                ))
        )
    });
    assert!(
        mod_has_nominate,
        "moderator's transcript must contain its own nominate tool_use (AC5)"
    );
}

/// True if any message in the per-request snapshots carries a
/// `ToolUse` / `ToolResult` block.
fn has_tool_block(snapshots: &[Vec<ChatMessage>]) -> bool {
    snapshots.iter().flat_map(|msgs| msgs.iter()).any(|m| {
        matches!(
            &m.content,
            MessageContent::Blocks(blocks)
                if blocks.iter().any(|b| matches!(b, ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. }))
        )
    })
}

// ---------------------------------------------------------------------------
// D-D entry-guard regression tests (design.md §5)
// ---------------------------------------------------------------------------

/// Reconstruct the in-memory transcript the way group-chat's
/// `reload_messages` does (from the DB rows) so the entry guard sees
/// an already-persisted tail user message.
async fn reloaded_transcript(db: &sqlx::SqlitePool, session_id: &str) -> Vec<ChatMessage> {
    let loaded = db::load_session(db, session_id).await.unwrap().unwrap();
    loaded
        .messages
        .iter()
        .map(|m| ChatMessage {
            role: if m.role == "assistant" {
                Role::Assistant
            } else {
                Role::User
            },
            content: serde_json::from_value(m.content.clone()).unwrap(),
            speaker: m.speaker.clone(),
        })
        .collect()
}

/// D-D regression: `group_chat_state = None` (classic chat) must NOT
/// trigger the skip — a reloaded user message is a genuinely new send
/// in a single-agent chat and is persisted normally.
#[tokio::test]
async fn entry_guard_does_not_skip_when_group_chat_state_none() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    // Seed the DB with one user row so a reloaded transcript's tail
    // user message content-matches a DB row.
    db::persist_turn(
        &h.db,
        &h.session_id,
        Role::User,
        &MessageContent::Text("hello".to_string()),
        0,
        None,
        None,
    )
    .await
    .expect("seed hello");

    let messages = reloaded_transcript(&h.db, &h.session_id).await;
    assert_eq!(messages.len(), 1, "seed sanity");

    let mock = Arc::new(MockProvider::new(vec![text_turn("reply")]));
    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-guard-none".to_string(),
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
        Some(1), // single turn
        false,
        false,
        Some(false),
        None, // worker_catalog
        Arc::new(crate::agent::subagent::ThreadLocalSubagentSink),
        None, // system_prompt_override
        None, // worker_run_id
        h.subagent_cache.clone(),
        None,
        None,
        None,
        h.app_data_dir.clone(),
        None,
        h.question_store.clone(),
        None,
        None, // group_chat_state = None → guard must NOT skip
        None, // current_speaker
    )
    .await;

    assert_eq!(emitter.error_event_count(), 0);
    let user_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE session_id = ? AND role = 'user'")
            .bind(&h.session_id)
            .fetch_one(&h.db)
            .await
            .unwrap();
    assert_eq!(
        user_rows, 2,
        "group_chat_state=None must NOT skip the reloaded user message (re-persisted)"
    );
}

/// D-D: `group_chat_state = Some(...)` (a group-chat speaker) DOES
/// trigger the skip when the tail user message content-matches any DB
/// user row — this is what prevents the duplicate tool_result rows.
#[tokio::test]
async fn entry_guard_skips_when_group_chat_state_some_and_tail_matches_db() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    db::persist_turn(
        &h.db,
        &h.session_id,
        Role::User,
        &MessageContent::Text("hello".to_string()),
        0,
        None,
        None,
    )
    .await
    .expect("seed hello");

    let messages = reloaded_transcript(&h.db, &h.session_id).await;

    let mock = Arc::new(MockProvider::new(vec![text_turn("reply")]));
    let turn_state: crate::tools::nominate_speaker::SharedTurnState = Arc::new(
        tokio::sync::Mutex::new(crate::tools::nominate_speaker::GroupChatTurnState::default()),
    );
    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-guard-some".to_string(),
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
        Some(1),
        false,
        false,
        Some(false),
        None,
        Arc::new(crate::agent::subagent::ThreadLocalSubagentSink),
        None,
        None,
        h.subagent_cache.clone(),
        None,
        None,
        None,
        h.app_data_dir.clone(),
        None,
        h.question_store.clone(),
        None,
        Some(turn_state), // group_chat_state = Some → guard skips
        None,
    )
    .await;

    assert_eq!(emitter.error_event_count(), 0);
    let user_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE session_id = ? AND role = 'user'")
            .bind(&h.session_id)
            .fetch_one(&h.db)
            .await
            .unwrap();
    assert_eq!(
        user_rows, 1,
        "group_chat_state=Some must skip the already-persisted reloaded user message"
    );
}
