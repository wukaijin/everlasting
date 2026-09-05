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
//! (D-B reload retention / D-D entry guard / D-F old heuristic guard
//! replaced) + `08-07-group-chat-role-history-isolation` (per-role
//! `role_history` assembler replaces the 08-04 participant_view).

#![cfg(test)]

use std::collections::HashMap;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::tests_common::{
    chat_loop_deps, chat_loop_request, make_harness, parent_role, test_messages, MockEmitter,
    TestHarness,
};
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
            },
            ParticipantConfig {
                name: "M2".to_string(),
                model: "m2".to_string(),
                persona_md: None,
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
        None,
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
            attachments: None,
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
        {
            let mut request = chat_loop_request(
                vec![],
                mock.clone(),
                200_000,
                "rid-guard-none".to_string(),
                h.session_id.clone(),
                messages,
                emitter.clone(),
            );
            request.max_turns = Some(1); // single turn
            request
        },
        chat_loop_deps(&h),
        parent_role(&h),
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
        {
            let mut request = chat_loop_request(
                vec![],
                mock.clone(),
                200_000,
                "rid-guard-some".to_string(),
                h.session_id.clone(),
                messages,
                emitter.clone(),
            );
            request.max_turns = Some(1); // single turn
                                         // group_chat_state = Some → guard skips the reloaded user message.
            request.group_chat_state = Some(turn_state);
            request
        },
        chat_loop_deps(&h),
        parent_role(&h),
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

// ---------------------------------------------------------------------------
// R3 (08-07-group-chat-review-fixes): participant multi-turn evidence
// gathering. Participants now run at max_turns=20, so a participant may
// call a tool (read_file) in one turn and deliver its remark in a second
// turn after seeing the tool_result. The pre-R3 max_turns=1 made this
// impossible — the tool schema was present but there was no follow-up
// turn to act on the result.
// ---------------------------------------------------------------------------

/// Script a participant (M1) that gathers evidence before speaking: its
/// first scripted response is a `read_file` tool_use (stop_reason=
/// `tool_use` → the loop executes the tool and continues to a second
/// turn); its second response is the substantive text remark
/// (stop_reason=`end_turn` → the participant turn ends). The moderator
/// nominates M1 once then ends, so only M1 needs the 2-response script.
fn script_participant_evidence_mocks(notes_abs_path: &str) -> GroupChatMocks {
    let moderator = Arc::new(MockProvider::new(vec![
        // Round 0: nominate M1.
        mod_tool_turn(
            "c1",
            "nominate_speaker",
            serde_json::json!({"name": "M1"}),
            "主持人:请 M1 调研",
        ),
        // Round 1: end_discussion.
        mod_tool_turn("c2", "end_discussion", serde_json::json!({}), "主持人:结束"),
    ]));
    // M1's two responses: evidence read, then remark. The order is
    // consumed FIFO by MockProvider, matching the participant's turn
    // progression (turn 1 tool_use → turn 2 text).
    let m1 = Arc::new(MockProvider::new(vec![
        MockResponse::Events(vec![
            ok_evt(ChatEvent::Start),
            ok_evt(ChatEvent::ToolCall {
                id: "r1".to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({"path": notes_abs_path}),
            }),
            tool_use_stop(),
        ]),
        // Second send (after the tool_result lands): deliver the remark.
        text_turn("M1 看过了，结论是 A"),
    ]));
    // M2 is never nominated in this flow; an empty script is fine
    // because MockProvider is only consumed when its model is dispatched.
    let m2 = Arc::new(MockProvider::new(vec![]));

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

#[tokio::test]
async fn participant_gathers_evidence_then_speaks_across_turns() {
    let (h, gc_session_id) = make_group_chat_harness().await;

    // Seed a real file in the project tempdir so read_file succeeds and
    // the tool_result is non-error — this exercises the genuine
    // "gather evidence then speak" path rather than an error fallback.
    let notes_path = h.project_path.join("notes.md");
    tokio::fs::write(&notes_path, "# notes\n决策: 选 A\n")
        .await
        .expect("seed notes.md");
    let notes_abs = notes_path.to_str().unwrap().to_string();

    let emitter = Arc::new(MockEmitter::new());
    let mocks = script_participant_evidence_mocks(&notes_abs);

    run_group_chat_loop(
        crate::tools::builtin_tools(),
        200_000,
        None,
        "rid-r3".to_string(),
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

    // AC: no errors — the tool round + the follow-up turn both completed.
    assert_eq!(
        emitter.error_event_count(),
        0,
        "participant multi-turn evidence flow must run without ChatEvent::Error"
    );

    // AC: M1 was sent two requests (turn 1 = tool_use, turn 2 = remark).
    // This is the core R3 assertion — the pre-R3 max_turns=1 would have
    // ended M1's turn right after the tool_result and the second send
    // (the actual remark) would never happen.
    assert_eq!(
        mocks.m1.call_count(),
        2,
        "M1 must run TWO turns: read_file then speak (max_turns >= 2)"
    );

    // AC: the second send M1 saw contains the read_file tool_result as a
    // user-role message (the loop persisted it and reloaded), proving
    // the evidence gathered in turn 1 was visible in turn 2.
    let m1_sends = mocks.m1.sent_messages();
    let second_send = &m1_sends[1];
    let saw_read_result = second_send.iter().any(|m| match &m.content {
        MessageContent::Blocks(blocks) => blocks.iter().any(
            |b| matches!(b, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "r1"),
        ),
        _ => false,
    });
    assert!(
        saw_read_result,
        "M1's second turn must see the read_file tool_result from turn 1: {second_send:?}"
    );

    // AC: the participant's remark made it into the DB as an assistant
    // row (turn 2 persisted). Sanity-check it carries the expected text.
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT text FROM messages WHERE session_id = ? AND role = 'assistant' ORDER BY seq",
    )
    .bind(&gc_session_id)
    .fetch_all(&h.db)
    .await
    .expect("fetch assistant rows");
    let any_m1_remark = rows.iter().any(|t| t.contains("M1 看过了，结论是 A"));
    assert!(
        any_m1_remark,
        "M1's substantive remark must be persisted: {rows:?}"
    );
}

// ---------------------------------------------------------------------------
// R2 (08-07-group-chat-review-fixes): the orchestrator's previously-silent
// boundary paths (max rounds / nominee unknown / participant unresolved)
// now emit Done { stop_reason } so the frontend can surface them. The
// `moderator_stuck` path that 08-07 originally added was REMOVED by
// 08-07-group-chat-toolset-and-identity R2 (the streak mechanism couldn't
// distinguish "researching" from "stuck"; now bounded only by
// MAX_ORCHESTRATION_ROUNDS → stop_reason "max_rounds"). This test covers
// the non-terminal nominee_unknown shape (mid-loop → Done but discussion
// continues). The terminal max_rounds shape is covered by the existing
// full-multi-round flow test's group_chat_end + the stop_reason doc above.
// ---------------------------------------------------------------------------

/// Script a moderator that nominates an unknown name ("Nobody"), then on
/// its next round ends the discussion. The unknown-nominee round must
/// emit a non-terminal `Done { stop_reason: "nominee_unknown" }` (the
/// discussion continues — the moderator gets another round), and the
/// final terminal Done is still `group_chat_end`.
#[tokio::test]
async fn orchestrator_emits_nonterminal_done_for_unknown_nominee() {
    let (h, gc_session_id) = make_group_chat_harness().await;
    let emitter = Arc::new(MockEmitter::new());

    let moderator = Arc::new(MockProvider::new(vec![
        // Round 0: nominate a name NOT in the roster (M1/M2 exist; Nobody doesn't).
        mod_tool_turn(
            "c1",
            "nominate_speaker",
            serde_json::json!({"name": "Nobody"}),
            "主持人:请 Nobody",
        ),
        // Round 1: end the discussion (so the loop terminates cleanly).
        mod_tool_turn("c2", "end_discussion", serde_json::json!({}), "主持人:结束"),
    ]));
    let m1 = Arc::new(MockProvider::new(vec![]));
    let m2 = Arc::new(MockProvider::new(vec![]));
    let mut catalog: ProviderCatalog = HashMap::new();
    catalog.insert("moderator".to_string(), moderator.clone());
    catalog.insert("m1".to_string(), m1.clone());
    catalog.insert("m2".to_string(), m2.clone());
    let catalog = Arc::new(tokio::sync::RwLock::new(catalog));

    run_group_chat_loop(
        crate::tools::builtin_tools(),
        200_000,
        None,
        "rid-r2-nominee".to_string(),
        gc_session_id.clone(),
        test_messages(),
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
        Some(catalog),
        Arc::new(crate::agent::subagent::ThreadLocalSubagentSink),
        h.subagent_cache.clone(),
        h.app_data_dir.clone(),
        h.question_store.clone(),
        group_chat_ctx(),
    )
    .await;

    assert_eq!(
        emitter.error_event_count(),
        0,
        "unknown-nominee skip must not emit ChatEvent::Error"
    );

    let dones: Vec<String> = emitter
        .chat_events()
        .iter()
        .filter_map(|p| match &p.event {
            ChatEvent::Done { stop_reason, .. } => stop_reason.clone(),
            _ => None,
        })
        .collect();

    // The non-terminal nominee_unknown Done must appear (mid-loop).
    assert!(
        dones.iter().any(|s| s == "nominee_unknown"),
        "the unknown-nominee round must emit Done{{stop_reason=nominee_unknown}}: {dones:?}"
    );
    // The discussion continued and ended cleanly → terminal Done is group_chat_end.
    assert_eq!(
        dones[dones.len() - 1],
        "group_chat_end",
        "the terminal Done must be group_chat_end (discussion ended normally): {dones:?}"
    );
    // The nominee_unknown must come BEFORE the terminal group_chat_end
    // (it's a mid-loop event, the end is post-loop).
    let nominee_idx = dones.iter().position(|s| s == "nominee_unknown");
    let end_idx = dones.iter().position(|s| s == "group_chat_end");
    assert!(
        nominee_idx.is_some() && end_idx.is_some() && nominee_idx < end_idx,
        "nominee_unknown must precede group_chat_end: {dones:?}"
    );
}

// ---------------------------------------------------------------------------
// GC1/GC2/GC5/GC7 (2026-09-05, BUGLIST-group-chat): discussion-lifecycle
// regressions — orchestration-grained busy, persisted stop_reason +
// discussion_summary, and the consecutive-ERROR_MARKER circuit breaker.
// ---------------------------------------------------------------------------

/// GC1: sink that snapshots the session's busy state (the
/// `session_active_request` map, same source `list_sessions_inner`
/// patches from) at EVERY chat event emitted during the orchestration —
/// including the `Speaker` events that fire in the inter-turn gaps.
/// `try_lock` (sync context): a contended lock skips that snapshot
/// (the orchestrator only holds the lock at start/end, so misses are
/// practically impossible).
struct BusyProbeSink {
    s2p: Arc<tokio::sync::Mutex<HashMap<String, String>>>,
    sid: String,
    inner: MockEmitter,
    busy_snapshots: Arc<std::sync::Mutex<Vec<bool>>>,
}

impl BusyProbeSink {
    fn snapshot(&self) {
        if let Ok(g) = self.s2p.try_lock() {
            self.busy_snapshots
                .lock()
                .unwrap()
                .push(g.contains_key(&self.sid));
        }
    }
}

impl crate::state::ChatEventSink for BusyProbeSink {
    fn emit_chat_event(&self, payload: &crate::state::ChatEventPayload) {
        self.snapshot();
        self.inner.emit_chat_event(payload);
    }
    fn emit_tool_call(&self, payload: &crate::state::ToolCallPayload) {
        self.inner.emit_tool_call(payload);
    }
    fn emit_tool_result(&self, payload: &crate::state::ToolResultPayload) {
        self.inner.emit_tool_result(payload);
    }
    fn emit_permission_ask(&self, payload: crate::agent::permissions::PermissionAskPayload) {
        self.inner.emit_permission_ask(payload);
    }
}

/// GC1 + GC2 + GC7: busy must be orchestration-grained (no dip in the
/// inter-turn gaps), the maps must be cleaned exactly at orchestration
/// exit, and the end state (stop_reason + discussion_summary) must be
/// persisted on the session row.
#[tokio::test]
async fn group_chat_busy_holds_across_turns_and_lifecycle_persists() {
    let (h, gc_session_id) = make_group_chat_harness().await;

    // Script: nominate M1 → M1 speaks → end_discussion with a summary.
    let moderator = Arc::new(MockProvider::new(vec![
        mod_tool_turn(
            "b1",
            "nominate_speaker",
            serde_json::json!({"name": "M1"}),
            "开场",
        ),
        mod_tool_turn(
            "b2",
            "end_discussion",
            serde_json::json!({"summary": "## 共识清单\n- 先做 A"}),
            "收尾",
        ),
    ]));
    let m1 = Arc::new(MockProvider::new(vec![text_turn("我是 M1")]));
    let mut catalog: ProviderCatalog = HashMap::new();
    catalog.insert("moderator".to_string(), moderator.clone());
    catalog.insert("m1".to_string(), m1.clone());
    let catalog = Arc::new(tokio::sync::RwLock::new(catalog));

    let probe = Arc::new(BusyProbeSink {
        s2p: h.session_active_request.clone(),
        sid: gc_session_id.clone(),
        inner: MockEmitter::new(),
        busy_snapshots: Arc::new(std::sync::Mutex::new(Vec::new())),
    });

    // Mimic chat_inner's legacy-path registration (group chat never
    // enters the F1 routing critical section): claim the session slot
    // + the rid's cancel token BEFORE the orchestration starts.
    let token = CancellationToken::new();
    h.cancellations
        .lock()
        .await
        .insert("rid-gc-busy".to_string(), token.clone());
    h.session_active_request
        .lock()
        .await
        .insert(gc_session_id.clone(), "rid-gc-busy".to_string());

    run_group_chat_loop(
        crate::tools::builtin_tools(),
        200_000,
        None,
        "rid-gc-busy".to_string(),
        gc_session_id.clone(),
        test_messages(),
        probe.clone(),
        h.db.clone(),
        h.cancellations.clone(),
        h.session_active_request.clone(),
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        token,
        None,
        h.background_shells.clone(),
        Some(catalog),
        Arc::new(crate::agent::subagent::ThreadLocalSubagentSink),
        h.subagent_cache.clone(),
        h.app_data_dir.clone(),
        h.question_store.clone(),
        group_chat_ctx(),
    )
    .await;

    // GC1: every mid-orchestration snapshot saw busy=true — including
    // the Speaker events fired in the inter-turn gaps (pre-fix, the
    // first inner guard's Drop evicted the entry and busy dipped).
    let snapshots = probe.busy_snapshots.lock().unwrap().clone();
    assert!(
        snapshots.len() >= 5,
        "expected a meaningful number of event-time snapshots, got {snapshots:?}"
    );
    assert!(
        snapshots.iter().all(|b| *b),
        "busy must hold for the WHOLE orchestration (no inter-turn dip): {snapshots:?}"
    );

    // GC1: orchestration exit is the single cleanup point — both maps
    // are empty afterwards (the discussion is over, busy=false for
    // good, not resurrected).
    assert!(
        !h.session_active_request
            .lock()
            .await
            .contains_key(&gc_session_id),
        "session_active_request must be cleared at orchestration exit"
    );
    assert!(
        !h.cancellations.lock().await.contains_key("rid-gc-busy"),
        "cancellations entry must be cleared at orchestration exit"
    );

    // GC2 + GC7: lifecycle persisted on the session row (first-class
    // fields, no tool_result parsing).
    let loaded = db::load_session(&h.db, &gc_session_id)
        .await
        .expect("load_session")
        .expect("session exists");
    assert_eq!(
        loaded.session.stop_reason.as_deref(),
        Some("group_chat_end")
    );
    assert_eq!(
        loaded.session.discussion_summary.as_deref(),
        Some("## 共识清单\n- 先做 A"),
        "end_discussion summary must be persisted as a first-class field"
    );

    // GC2: the list_sessions summary carries the stop reason too (the
    // poller's 「!busy + stop_reason → ended」derivation).
    let summaries = db::list_sessions(&h.db, &h.project_id)
        .await
        .expect("list_sessions");
    let me = summaries
        .iter()
        .find(|s| s.id == gc_session_id)
        .expect("group-chat session in summaries");
    assert_eq!(me.stop_reason.as_deref(), Some("group_chat_end"));
}

/// GC5: three consecutive ERROR_MARKER turns trip the breaker — the
/// discussion halts with terminal Done + persisted stop_reason
/// "error", and other speakers see the failed turns as system notes,
/// never the raw marker.
#[tokio::test]
async fn group_chat_error_breaker_halts_after_consecutive_error_turns() {
    let (h, gc_session_id) = make_group_chat_harness().await;

    // Auth is non-retryable (retry.rs: deterministic failure class) —
    // one send per error turn, so the scripted counts below are exact.
    let boom = || {
        MockResponse::ErrThenEnd(LlmError::Auth(
            "simulated provider auth failure".to_string(),
        ))
    };
    // r0: nominate M1 → M1 errors (streak 1)
    // r1: nominate M1 → M1 errors (streak 2)
    // r2: nominate M2 → M2 errors (streak 3 → breaker trips)
    let moderator = Arc::new(MockProvider::new(vec![
        mod_tool_turn(
            "e1",
            "nominate_speaker",
            serde_json::json!({"name": "M1"}),
            "第一次点名",
        ),
        mod_tool_turn(
            "e2",
            "nominate_speaker",
            serde_json::json!({"name": "M1"}),
            "再给 M1 一次",
        ),
        mod_tool_turn(
            "e3",
            "nominate_speaker",
            serde_json::json!({"name": "M2"}),
            "换 M2",
        ),
    ]));
    let m1 = Arc::new(MockProvider::new(vec![boom(), boom()]));
    let m2 = Arc::new(MockProvider::new(vec![boom()]));
    let mut catalog: ProviderCatalog = HashMap::new();
    catalog.insert("moderator".to_string(), moderator.clone());
    catalog.insert("m1".to_string(), m1.clone());
    catalog.insert("m2".to_string(), m2.clone());
    let catalog = Arc::new(tokio::sync::RwLock::new(catalog));

    let emitter = Arc::new(MockEmitter::new());
    run_group_chat_loop(
        crate::tools::builtin_tools(),
        200_000,
        None,
        "rid-gc-err".to_string(),
        gc_session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations.clone(),
        h.session_active_request.clone(),
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        Some(catalog),
        Arc::new(crate::agent::subagent::ThreadLocalSubagentSink),
        h.subagent_cache.clone(),
        h.app_data_dir.clone(),
        h.question_store.clone(),
        group_chat_ctx(),
    )
    .await;

    // Terminal Done carries the breaker reason.
    let dones: Vec<String> = emitter
        .chat_events()
        .iter()
        .filter_map(|p| match &p.event {
            ChatEvent::Done { stop_reason, .. } => stop_reason.clone(),
            _ => None,
        })
        .collect();
    assert_eq!(
        dones.last().map(String::as_str),
        Some("error"),
        "breaker halt must be the terminal Done, got {dones:?}"
    );

    // GC2: persisted for post-hoc「这场为何结束」.
    let loaded = db::load_session(&h.db, &gc_session_id)
        .await
        .expect("load_session")
        .expect("session exists");
    assert_eq!(loaded.session.stop_reason.as_deref(), Some("error"));
    assert!(
        loaded.session.discussion_summary.is_none(),
        "no end_discussion happened → no summary"
    );

    // The loop actually STOPPED at round 2 (3 moderator sends, no 4th).
    assert_eq!(moderator.call_count(), 3);
    assert_eq!(m1.call_count(), 2);
    assert_eq!(m2.call_count(), 1);

    // GC5 view: M2's request history must show M1's failed turns as
    // system notes, never the raw ERROR_MARKER text.
    let m2_text = m2
        .sent_messages()
        .iter()
        .flat_map(|msgs| msgs.iter())
        .map(|m| m.content.to_text())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !m2_text.contains(crate::agent::helpers::ERROR_MARKER),
        "other speakers must not see the raw ERROR_MARKER: {m2_text:?}"
    );
    assert!(
        m2_text.contains("系统注记"),
        "M2 must see the system note for M1's failed turns: {m2_text:?}"
    );
}

/// GC2: a discussion that never nominates runs to
/// MAX_ORCHESTRATION_ROUNDS and the stop reason persisted is
/// distinguishable from a normal end (group_chat_end) and from the
/// breaker (error).
#[tokio::test]
async fn group_chat_max_rounds_persists_stop_reason() {
    let (h, gc_session_id) = make_group_chat_harness().await;

    // 30 rounds of moderator research text, never nominating → the
    // outer loop exhausts MAX_ORCHESTRATION_ROUNDS.
    let moderator = Arc::new(MockProvider::new(
        (0..30).map(|_| text_turn("调研中…")).collect(),
    ));
    let mut catalog: ProviderCatalog = HashMap::new();
    catalog.insert("moderator".to_string(), moderator.clone());
    let catalog = Arc::new(tokio::sync::RwLock::new(catalog));

    let emitter = Arc::new(MockEmitter::new());
    run_group_chat_loop(
        crate::tools::builtin_tools(),
        200_000,
        None,
        "rid-gc-mr".to_string(),
        gc_session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations.clone(),
        h.session_active_request.clone(),
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        Some(catalog),
        Arc::new(crate::agent::subagent::ThreadLocalSubagentSink),
        h.subagent_cache.clone(),
        h.app_data_dir.clone(),
        h.question_store.clone(),
        group_chat_ctx(),
    )
    .await;

    assert_eq!(moderator.call_count(), 30);
    let dones: Vec<String> = emitter
        .chat_events()
        .iter()
        .filter_map(|p| match &p.event {
            ChatEvent::Done { stop_reason, .. } => stop_reason.clone(),
            _ => None,
        })
        .collect();
    assert_eq!(
        dones.last().map(String::as_str),
        Some("max_rounds"),
        "terminal Done must be max_rounds, got {dones:?}"
    );
    let loaded = db::load_session(&h.db, &gc_session_id)
        .await
        .expect("load_session")
        .expect("session exists");
    assert_eq!(loaded.session.stop_reason.as_deref(), Some("max_rounds"));
}

/// GC2: a REUSED group-chat session clears the previous run's
/// lifecycle columns at the new orchestration's start (the poller
/// must never see the stale reason while a new discussion runs).
#[tokio::test]
async fn group_chat_second_run_clears_stale_stop_reason() {
    let (h, gc_session_id) = make_group_chat_harness().await;
    // Simulate a previous ended run.
    db::finalize_group_chat_lifecycle(&h.db, &gc_session_id, "max_rounds", Some("旧总结"))
        .await
        .expect("seed previous lifecycle");

    // Second run: nominate nobody for one round... the loop only
    // clears at start; a single round-0 moderator text turn then a
    // second scripted end_discussion ends the run cleanly.
    let moderator = Arc::new(MockProvider::new(vec![mod_tool_turn(
        "r1",
        "end_discussion",
        serde_json::json!({}),
        "直接结束",
    )]));
    let mut catalog: ProviderCatalog = HashMap::new();
    catalog.insert("moderator".to_string(), moderator.clone());
    let catalog = Arc::new(tokio::sync::RwLock::new(catalog));

    let emitter = Arc::new(MockEmitter::new());
    run_group_chat_loop(
        crate::tools::builtin_tools(),
        200_000,
        None,
        "rid-gc-reuse".to_string(),
        gc_session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations.clone(),
        h.session_active_request.clone(),
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        Some(catalog),
        Arc::new(crate::agent::subagent::ThreadLocalSubagentSink),
        h.subagent_cache.clone(),
        h.app_data_dir.clone(),
        h.question_store.clone(),
        group_chat_ctx(),
    )
    .await;

    let loaded = db::load_session(&h.db, &gc_session_id)
        .await
        .expect("load_session")
        .expect("session exists");
    assert_eq!(
        loaded.session.stop_reason.as_deref(),
        Some("group_chat_end"),
        "the NEW run's reason must win"
    );
    // end_discussion without an explicit summary stores the tool's
    // default remark — never the PREVIOUS run's summary.
    assert_ne!(
        loaded.session.discussion_summary.as_deref(),
        Some("旧总结"),
        "stale summary from the previous run must not survive"
    );
}

/// GC4: self-referential `@<speaker>:` prefixes are stripped at the
/// PERSIST site — the stored transcript (and therefore every later
/// own-history view that feeds the imitation loop) stays clean, and a
/// prefix-only turn collapses to no row at all.
#[tokio::test]
async fn group_chat_strips_own_prefix_on_persist() {
    let (h, gc_session_id) = make_group_chat_harness().await;

    // r0: moderator opens with a self-prefixed remark + nominates M1.
    // r1: moderator closes with an ACCUMULATED double prefix.
    // M1's turn is prefix-only ("@M1:") — must leave no row.
    let moderator = Arc::new(MockProvider::new(vec![
        mod_tool_turn(
            "p1",
            "nominate_speaker",
            serde_json::json!({"name": "M1"}),
            "@moderator: 点名",
        ),
        mod_tool_turn(
            "p2",
            "end_discussion",
            serde_json::json!({}),
            "@moderator: @moderator: 收工",
        ),
    ]));
    let m1 = Arc::new(MockProvider::new(vec![text_turn("@M1:")]));
    let mut catalog: ProviderCatalog = HashMap::new();
    catalog.insert("moderator".to_string(), moderator.clone());
    catalog.insert("m1".to_string(), m1.clone());
    let catalog = Arc::new(tokio::sync::RwLock::new(catalog));

    let emitter = Arc::new(MockEmitter::new());
    run_group_chat_loop(
        crate::tools::builtin_tools(),
        200_000,
        None,
        "rid-gc-prefix".to_string(),
        gc_session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations.clone(),
        h.session_active_request.clone(),
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        Some(catalog),
        Arc::new(crate::agent::subagent::ThreadLocalSubagentSink),
        h.subagent_cache.clone(),
        h.app_data_dir.clone(),
        h.question_store.clone(),
        group_chat_ctx(),
    )
    .await;

    // Moderator rows: real content survives, ZERO self-prefix layers.
    let mod_texts: Vec<String> =
        sqlx::query_as::<_, (String,)>("SELECT text FROM messages WHERE session_id = ? AND speaker = 'moderator' AND role = 'assistant'")
            .bind(&gc_session_id)
            .fetch_all(&h.db)
            .await
            .expect("fetch moderator rows")
            .into_iter()
            .map(|t| t.0)
            .collect();
    assert!(
        mod_texts.iter().any(|t| t.contains("点名")),
        "moderator content must survive the strip: {mod_texts:?}"
    );
    assert!(
        mod_texts.iter().any(|t| t.contains("收工")),
        "closing remark must survive (accumulated double prefix stripped): {mod_texts:?}"
    );
    assert!(
        mod_texts.iter().all(|t| !t.contains("@moderator:")),
        "no self-prefix layer may persist (snowball source): {mod_texts:?}"
    );

    // M1's prefix-only turn must leave NO assistant row (the observed
    // seq9/seq38 prefix-only messages are gone at the root).
    let m1_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE session_id = ? AND speaker = 'M1'")
            .bind(&gc_session_id)
            .fetch_one(&h.db)
            .await
            .expect("count M1 rows");
    assert_eq!(
        m1_rows, 0,
        "prefix-only turn must not persist any row (speaker='M1')"
    );

    // The discussion still ended normally.
    let loaded = db::load_session(&h.db, &gc_session_id)
        .await
        .expect("load_session")
        .expect("session exists");
    assert_eq!(
        loaded.session.stop_reason.as_deref(),
        Some("group_chat_end")
    );
}
