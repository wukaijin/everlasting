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
use std::sync::atomic::Ordering;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::tests_common::{
    chat_loop_deps, chat_loop_request, make_harness, parent_role, test_messages, MockEmitter,
    TestHarness,
};
use crate::agent::chat_loop::run_chat_loop;
use crate::agent::discussion_detail::{AnchorCheck, DiscussionDetail, Stance};
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

/// 09-06-gc-p0: an empty controls registry for orchestrator calls.
/// Tests that exercise inject/preempt pass a map they keep a handle to.
fn fresh_controls() -> Arc<
    tokio::sync::Mutex<
        std::collections::HashMap<String, crate::agent::group_chat::GroupChatControl>,
    >,
> {
    Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()))
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
        project_root: None,
        created_via: None,
        token_budget: None,
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
        fresh_controls(),
        None,
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
        fresh_controls(),
        None,
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
        fresh_controls(),
        None,
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
        fresh_controls(),
        None,
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
        fresh_controls(),
        None,
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
        fresh_controls(),
        None,
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
    // Simulate a previous ended run (with structured detail — C2: a
    // stale detail must not survive into the second run's consumers).
    db::finalize_group_chat_lifecycle(
        &h.db,
        &gc_session_id,
        "max_rounds",
        Some("旧总结"),
        Some(r#"{"conclusions":[{"claim":"旧结论","anchors":[],"stance":"verified"}],"open_questions":[]}"#),
    )
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
        fresh_controls(),
        None,
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
    // C2 (2026-09-09): the stale structured detail from the previous
    // run must be cleared too — the second run closed without
    // structured params, so a surviving detail would feed consumers
    // the FIRST run's anchor-checked conclusions.
    assert!(
        loaded.session.discussion_detail.is_none(),
        "stale discussion_detail from the previous run must not survive, got {:?}",
        loaded.session.discussion_detail
    );
}

/// C2 (2026-09-09) E2E: a structured end_discussion persists as the
/// `discussion_detail` column with anchor `check` results filled by
/// the orchestrator's post-validation against the real project root.
/// Coexists with the narrative `discussion_summary` (both first-class).
#[tokio::test]
async fn group_chat_structured_end_discussion_persists_validated_detail() {
    let (h, gc_session_id) = make_group_chat_harness().await;
    // Real fixture files under the harness project root for anchors.
    std::fs::write(h.project_path.join("fixture_a.rs"), "l1\nl2\nl3\n").unwrap();

    let moderator = Arc::new(MockProvider::new(vec![mod_tool_turn(
        "cd1",
        "end_discussion",
        serde_json::json!({
            "summary": "收官叙事",
            "conclusions": [
                {"claim": "实锚结论", "anchors": [{"path": "fixture_a.rs", "line": 2}], "stance": "verified"},
                {"claim": "越界锚", "anchors": [{"path": "fixture_a.rs", "line": 99}]},
                {"claim": "缺失锚", "anchors": [{"path": "missing.rs"}]},
                {"claim": "无锚推测", "stance": "inferred"},
                {"claim": "争议项", "stance": "disputed"}
            ],
            "open_questions": ["何时复核"]
        }),
        "结束",
    )]));
    let mut catalog: ProviderCatalog = HashMap::new();
    catalog.insert("moderator".to_string(), moderator.clone());
    let catalog = Arc::new(tokio::sync::RwLock::new(catalog));

    let ctx = GroupChatCtx {
        project_root: Some(h.project_path.to_string_lossy().to_string()),
        ..group_chat_ctx()
    };

    let emitter = Arc::new(MockEmitter::new());
    run_group_chat_loop(
        crate::tools::builtin_tools(),
        200_000,
        None,
        "rid-gc-c2".to_string(),
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
        ctx,
        fresh_controls(),
        None,
    )
    .await;

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
        Some("收官叙事"),
        "narrative summary stays first-class alongside the structured detail"
    );
    let raw = loaded
        .session
        .discussion_detail
        .as_deref()
        .expect("structured detail must persist");
    let detail: DiscussionDetail = serde_json::from_str(raw).expect("detail is valid JSON");
    assert_eq!(detail.conclusions.len(), 5);
    assert_eq!(detail.open_questions, vec!["何时复核"]);
    // Anchor checks — annotated, never rewritten (stance untouched).
    assert_eq!(
        detail.conclusions[0].anchors[0].check,
        Some(AnchorCheck::Ok)
    );
    assert_eq!(
        detail.conclusions[1].anchors[0].check,
        Some(AnchorCheck::LineOutOfRange)
    );
    assert_eq!(
        detail.conclusions[2].anchors[0].check,
        Some(AnchorCheck::NotFound)
    );
    assert!(detail.conclusions[3].anchors.is_empty());
    // Stances survive validation verbatim (只标注不修改): declared
    // stances keep their value, omitted stance = inferred default.
    assert_eq!(detail.conclusions[0].stance, Stance::Verified);
    assert_eq!(detail.conclusions[1].stance, Stance::Inferred);
    assert_eq!(detail.conclusions[3].stance, Stance::Inferred);
    assert_eq!(detail.conclusions[4].stance, Stance::Disputed);
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
        fresh_controls(),
        None,
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

// ---------------------------------------------------------------------------
// 09-06-gc-p0-preempt-min-semantics: inject (R1) + preempt wrap-up (R2)
// ---------------------------------------------------------------------------

/// P0 hook sink: acts on `Speaker` events mid-orchestration, exactly
/// where a concurrent API caller would intervene in production.
///
/// - M1's `Speaker` event fires between the moderator's nominate and
///   M1's turn (i.e. the discussion is provably mid-flight): optionally
///   push an inject into the controls buffer and/or set the preempt
///   flag.
/// - The moderator's 2nd `Speaker` event is the preempt WRAP-UP turn:
///   optionally push an inject there (it can only be flushed at exit —
///   the wrap-up already reloaded its history).
///
/// `try_lock` is safe here (and deterministic for the test): the
/// orchestrator only takes the control lock at the round head and at
/// exit — never while an inner turn (whose Speaker events we hook) is
/// running. Panicking on contention keeps a silent hook miss from
/// degrading into a confusing assertion failure downstream.
struct P0HookSink {
    inner: MockEmitter,
    controls: Arc<tokio::sync::Mutex<HashMap<String, crate::agent::group_chat::GroupChatControl>>>,
    sid: String,
    m1_speaker_inject: Option<String>,
    m1_speaker_preempt: bool,
    wrapup_speaker_inject: Option<String>,
    moderator_speaker_count: std::sync::atomic::AtomicUsize,
}

impl P0HookSink {
    fn push_inject(&self, text: &str) {
        let cmap = self.controls.try_lock().expect("controls map lock");
        let control = cmap.get(&self.sid).expect("live discussion control entry");
        let mut inner = control.try_lock().expect("control inner lock");
        inner.pending_injects.push(ChatMessage {
            role: Role::User,
            content: MessageContent::Text(text.to_string()),
            speaker: None,
            attachments: None,
        });
    }
    fn set_preempt(&self) {
        let cmap = self.controls.try_lock().expect("controls map lock");
        let control = cmap.get(&self.sid).expect("live discussion control entry");
        control
            .try_lock()
            .expect("control inner lock")
            .preempt_requested = true;
    }
}

impl crate::state::ChatEventSink for P0HookSink {
    fn emit_chat_event(&self, payload: &crate::state::ChatEventPayload) {
        if let ChatEvent::Speaker { speaker } = &payload.event {
            match speaker.as_str() {
                "M1" => {
                    if let Some(t) = &self.m1_speaker_inject {
                        self.push_inject(t);
                    }
                    if self.m1_speaker_preempt {
                        self.set_preempt();
                    }
                }
                "moderator" => {
                    let n = self.moderator_speaker_count.fetch_add(1, Ordering::SeqCst);
                    // n==1 is the SECOND moderator speaker event = the
                    // preempt wrap-up turn (n==0 is a normal round).
                    if n == 1 {
                        if let Some(t) = &self.wrapup_speaker_inject {
                            self.push_inject(t);
                        }
                    }
                }
                _ => {}
            }
        }
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

/// E1 + E2 (inject): a user message pushed while the discussion is
/// mid-flight is persisted with the `[用户插入]` marker + metadata at
/// the next round head, becomes visible to the NEXT moderator turn,
/// the discussion is NOT killed (group_chat_end), and the inject row
/// lands AFTER the in-flight speaker's rows (seq discipline — the
/// buffer makes a mid-cursor collision architecturally impossible).
#[tokio::test]
async fn group_chat_inject_lands_next_moderator_round_and_survives_discussion() {
    let (h, gc_session_id) = make_group_chat_harness().await;

    let moderator = Arc::new(MockProvider::new(vec![
        mod_tool_turn(
            "i1",
            "nominate_speaker",
            serde_json::json!({"name": "M1"}),
            "开场",
        ),
        mod_tool_turn(
            "i2",
            "end_discussion",
            serde_json::json!({"summary": "## 共识\n- 注入已消化"}),
            "收尾",
        ),
    ]));
    let m1 = Arc::new(MockProvider::new(vec![text_turn("我是 M1")]));
    let mut catalog: ProviderCatalog = HashMap::new();
    catalog.insert("moderator".to_string(), moderator.clone());
    catalog.insert("m1".to_string(), m1.clone());
    let catalog = Arc::new(tokio::sync::RwLock::new(catalog));

    let controls = fresh_controls();
    let emitter = Arc::new(P0HookSink {
        inner: MockEmitter::new(),
        controls: controls.clone(),
        sid: gc_session_id.clone(),
        m1_speaker_inject: Some("补充:重点看看安全问题".to_string()),
        m1_speaker_preempt: false,
        wrapup_speaker_inject: None,
        moderator_speaker_count: std::sync::atomic::AtomicUsize::new(0),
    });

    let token = CancellationToken::new();
    h.cancellations
        .lock()
        .await
        .insert("rid-gc-inject".to_string(), token.clone());
    h.session_active_request
        .lock()
        .await
        .insert(gc_session_id.clone(), "rid-gc-inject".to_string());

    run_group_chat_loop(
        crate::tools::builtin_tools(),
        200_000,
        None,
        "rid-gc-inject".to_string(),
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
        token,
        None,
        h.background_shells.clone(),
        Some(catalog),
        Arc::new(crate::agent::subagent::ThreadLocalSubagentSink),
        h.subagent_cache.clone(),
        h.app_data_dir.clone(),
        h.question_store.clone(),
        group_chat_ctx(),
        controls.clone(),
        None,
    )
    .await;

    // 讨论不死:正常收官 + summary 落库(毁场路径会变成 cancelled/
    // 无 summary,或直接丢掉 M1 的发言)。
    assert_eq!(emitter.inner.error_event_count(), 0);
    let loaded = db::load_session(&h.db, &gc_session_id)
        .await
        .expect("load_session")
        .expect("session exists");
    assert_eq!(
        loaded.session.stop_reason.as_deref(),
        Some("group_chat_end"),
        "inject must NOT kill the discussion"
    );
    assert_eq!(
        loaded.session.discussion_summary.as_deref(),
        Some("## 共识\n- 注入已消化")
    );

    // 注入行:role=user、双轨标记(text 前缀 + metadata.kind)。
    let inject_rows: Vec<_> = loaded
        .messages
        .iter()
        .filter(|m| m.text.starts_with("[用户插入] "))
        .collect();
    assert_eq!(inject_rows.len(), 1, "exactly one persisted inject row");
    let inject = inject_rows[0];
    assert_eq!(inject.role, "user");
    assert_eq!(
        inject
            .metadata
            .as_ref()
            .and_then(|md| md.get("kind"))
            .and_then(|k| k.as_str()),
        Some("user_inject"),
        "inject row must carry metadata.kind=user_inject (R3 schema)"
    );
    assert_eq!(inject.text, "[用户插入] 补充:重点看看安全问题");

    // E2 seq 纪律:注入行落在 M1 发言之后(轮头 drain 时在途游标已
    // 释放),无 UNIQUE 冲突(冲突会以 error 事件 / 行缺失暴露)。
    let m1_max_seq: i64 = loaded
        .messages
        .iter()
        .filter(|m| m.speaker.as_deref() == Some("M1"))
        .map(|m| m.seq)
        .max()
        .expect("M1 spoke");
    assert!(
        inject.seq > m1_max_seq,
        "inject seq {} must follow M1's rows (max {})",
        inject.seq,
        m1_max_seq
    );

    // 下一 moderator 轮(收尾轮,第 2 次 send)的历史里能看到注入。
    let mod_sends = moderator.sent_messages();
    assert!(mod_sends.len() >= 2, "moderator ran >= 2 turns");
    let r1_view_text: String = mod_sends[1]
        .iter()
        .map(|m| m.content.to_text())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        r1_view_text.contains("[用户插入]"),
        "the next moderator turn must see the persisted inject: {r1_view_text:?}"
    );

    // E6:注册表条目在编排器退出后必须清掉。
    assert!(
        !controls.lock().await.contains_key(&gc_session_id),
        "controls entry must be removed at orchestration exit"
    );
}

/// E3 + E5 + flush (preempt happy path): preempt set mid-flight →
/// the in-flight speaker finishes → the wrap-up turn runs with the
/// WRAP-UP prompt (and sees injects drained at the boundary head) →
/// end_discussion persists a summary → terminal stop_reason=
/// "preempted" (distinct from cancelled / group_chat_end). An inject
/// that lands DURING the wrap-up turn is flushed to the DB at exit
/// (best-effort persistence, still no live seq cursor there).
#[tokio::test]
async fn group_chat_preempt_wrapup_produces_summary_and_distinguishable_stop_reason() {
    let (h, gc_session_id) = make_group_chat_harness().await;

    // r0: nominate M1;wrap-up:end_discussion 带总结(第一次即命中)。
    let moderator = Arc::new(MockProvider::new(vec![
        mod_tool_turn(
            "p1",
            "nominate_speaker",
            serde_json::json!({"name": "M1"}),
            "开场",
        ),
        mod_tool_turn(
            "p2",
            "end_discussion",
            serde_json::json!({"summary": "## 打断时共识\n- M1 已发言,结论 A"}),
            "收束",
        ),
    ]));
    let m1 = Arc::new(MockProvider::new(vec![text_turn("我是 M1")]));

    let mut catalog: ProviderCatalog = HashMap::new();
    catalog.insert("moderator".to_string(), moderator.clone());
    catalog.insert("m1".to_string(), m1.clone());
    let catalog = Arc::new(tokio::sync::RwLock::new(catalog));

    let controls = fresh_controls();
    let emitter = Arc::new(P0HookSink {
        inner: MockEmitter::new(),
        controls: controls.clone(),
        sid: gc_session_id.clone(),
        // M1 Speaker 事件(在途发言开始前):既注入又打断。
        m1_speaker_inject: Some("中途补一条".to_string()),
        m1_speaker_preempt: true,
        // wrap-up 轮 Speaker 事件:再注一条(只能走退出 flush)。
        wrapup_speaker_inject: Some("最后一条".to_string()),
        moderator_speaker_count: std::sync::atomic::AtomicUsize::new(0),
    });

    let token = CancellationToken::new();
    h.cancellations
        .lock()
        .await
        .insert("rid-gc-preempt".to_string(), token.clone());
    h.session_active_request
        .lock()
        .await
        .insert(gc_session_id.clone(), "rid-gc-preempt".to_string());

    run_group_chat_loop(
        crate::tools::builtin_tools(),
        200_000,
        None,
        "rid-gc-preempt".to_string(),
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
        token,
        None,
        h.background_shells.clone(),
        Some(catalog),
        Arc::new(crate::agent::subagent::ThreadLocalSubagentSink),
        h.subagent_cache.clone(),
        h.app_data_dir.clone(),
        h.question_store.clone(),
        group_chat_ctx(),
        controls.clone(),
        None,
    )
    .await;

    assert_eq!(emitter.inner.error_event_count(), 0);
    let loaded = db::load_session(&h.db, &gc_session_id)
        .await
        .expect("load_session")
        .expect("session exists");
    // E5:三态可区分 —— preempted ≠ cancelled ≠ group_chat_end。
    assert_eq!(
        loaded.session.stop_reason.as_deref(),
        Some("preempted"),
        "preempt must persist a distinguishable stop_reason"
    );
    assert_ne!("preempted", "cancelled");
    assert_ne!("preempted", "group_chat_end");
    // 收束轮产物:summary 落库为一等字段。
    assert_eq!(
        loaded.session.discussion_summary.as_deref(),
        Some("## 打断时共识\n- M1 已发言,结论 A"),
        "the wrap-up turn's end_discussion summary must persist"
    );
    // 在途发言没被斩:M1 的发言仍在。
    assert!(
        loaded
            .messages
            .iter()
            .any(|m| m.speaker.as_deref() == Some("M1")),
        "the in-flight speaker's remark must survive the preempt"
    );

    // 收束轮换了 WRAP-UP prompt(断言 system 换装,而非只看行为)。
    let systems = moderator.sent_systems();
    assert!(systems.len() >= 2, "moderator ran r0 + wrap-up");
    assert!(
        systems[1]
            .as_deref()
            .unwrap_or_default()
            .contains("WRAP-UP"),
        "wrap-up turn must run with the wrap-up instruction prompt"
    );
    // 轮头 drain 的注入进了收束轮视野(r1 head → wrap-up reload)。
    let mod_sends = moderator.sent_messages();
    let wrapup_view: String = mod_sends[1]
        .iter()
        .map(|m| m.content.to_text())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        wrapup_view.contains("[用户插入] 中途补一条"),
        "wrap-up turn must see the boundary-drained inject: {wrapup_view:?}"
    );

    // flush:wrap-up 期间到达的注入在退出后仍落库(不丢)。
    let inject_texts: Vec<&str> = loaded
        .messages
        .iter()
        .filter(|m| m.text.starts_with("[用户插入] "))
        .map(|m| m.text.as_str())
        .collect();
    assert_eq!(
        inject_texts,
        vec!["[用户插入] 中途补一条", "[用户插入] 最后一条"],
        "both injects persisted (boundary drain + exit flush)"
    );

    // E6:注册表清理。
    assert!(
        !controls.lock().await.contains_key(&gc_session_id),
        "controls entry must be removed at orchestration exit"
    );
}

/// E4 (preempt fallback): the wrap-up turn fails to call
/// end_discussion twice (plain text turns) → forced halt: stop_reason
/// is still "preempted" (distinguishable), summary honestly absent.
#[tokio::test]
async fn group_chat_preempt_wrapup_fallback_halts_without_summary() {
    let (h, gc_session_id) = make_group_chat_harness().await;

    // r0 nominate;两次 wrap-up 都是纯文本(不调 end_discussion)。
    let moderator = Arc::new(MockProvider::new(vec![
        mod_tool_turn(
            "f1",
            "nominate_speaker",
            serde_json::json!({"name": "M1"}),
            "开场",
        ),
        text_turn("我不会收束(第一次)"),
        text_turn("我还是不收束(重试)"),
    ]));
    let m1 = Arc::new(MockProvider::new(vec![text_turn("我是 M1")]));
    let mut catalog: ProviderCatalog = HashMap::new();
    catalog.insert("moderator".to_string(), moderator.clone());
    catalog.insert("m1".to_string(), m1.clone());
    let catalog = Arc::new(tokio::sync::RwLock::new(catalog));

    let controls = fresh_controls();
    let emitter = Arc::new(P0HookSink {
        inner: MockEmitter::new(),
        controls: controls.clone(),
        sid: gc_session_id.clone(),
        m1_speaker_inject: None,
        m1_speaker_preempt: true,
        wrapup_speaker_inject: None,
        moderator_speaker_count: std::sync::atomic::AtomicUsize::new(0),
    });

    let token = CancellationToken::new();
    h.cancellations
        .lock()
        .await
        .insert("rid-gc-fallback".to_string(), token.clone());
    h.session_active_request
        .lock()
        .await
        .insert(gc_session_id.clone(), "rid-gc-fallback".to_string());

    run_group_chat_loop(
        crate::tools::builtin_tools(),
        200_000,
        None,
        "rid-gc-fallback".to_string(),
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
        token,
        None,
        h.background_shells.clone(),
        Some(catalog),
        Arc::new(crate::agent::subagent::ThreadLocalSubagentSink),
        h.subagent_cache.clone(),
        h.app_data_dir.clone(),
        h.question_store.clone(),
        group_chat_ctx(),
        controls.clone(),
        None,
    )
    .await;

    // moderator 恰好 3 次 send:r0 nominate + 收束尝试 ×2。
    assert_eq!(moderator.call_count(), 3, "r0 + two wrap-up attempts");

    let loaded = db::load_session(&h.db, &gc_session_id)
        .await
        .expect("load_session")
        .expect("session exists");
    assert_eq!(
        loaded.session.stop_reason.as_deref(),
        Some("preempted"),
        "forced halt still records the distinguishable preempt reason"
    );
    assert_eq!(
        loaded.session.discussion_summary, None,
        "fallback halt has no summary — honest absence, not a fake one"
    );
    // 兜底立断不再烧轮:M1 之后没有任何 participant 再被点名。
    assert_eq!(emitter.inner.error_event_count(), 0);
    assert!(
        !controls.lock().await.contains_key(&gc_session_id),
        "controls entry must be removed at orchestration exit"
    );
}

// ---------------------------------------------------------------------------
// GCE P1a(2026-09-06,task 09-06-gc-p1a-checkpoint-resume)— checkpoint
// 落库与续跑。
// ---------------------------------------------------------------------------

/// P1a AC3(resume 语义):断点续跑——moderator 首轮吃 reload 转录
/// (resume 请求的空 messages 绝不进历史)、首 moderator 轮 system 带
/// RESUME 指令且仅此一轮、轮预算从 start_round 起算(round=1 进入,一个
/// nominate + 一个 end_discussion 即收官)、终局删行 + summary 落库。
#[tokio::test]
async fn group_chat_resume_enters_at_checkpoint_round_with_reload_and_instruction() {
    let (h, gc_session_id) = make_group_chat_harness().await;

    // Seed「interrupted discussion」residue: one prior exchange in the
    // transcript + a checkpoint row saying the discussion reached
    // round 1 (crash right after round 0's upsert).
    db::persist_turn(
        &h.db,
        &gc_session_id,
        Role::User,
        &MessageContent::Text("中断前的议题".to_string()),
        0,
        None,
        None,
    )
    .await
    .expect("seed prior user row");
    db::persist_turn(
        &h.db,
        &gc_session_id,
        Role::Assistant,
        &MessageContent::Text("中断前 M1 的发言".to_string()),
        1,
        None,
        Some("M1"),
    )
    .await
    .expect("seed prior M1 row");
    db::upsert_group_chat_checkpoint(&h.db, &gc_session_id, 1, 2)
        .await
        .expect("seed checkpoint (round=1, stale streak=2 — must not be inherited)");

    // Resumed script: round 1 moderator nominates M1 (first turn →
    // resume instruction), round 2 moderator ends.
    let moderator = Arc::new(MockProvider::new(vec![
        mod_tool_turn(
            "rs1",
            "nominate_speaker",
            serde_json::json!({"name": "M1"}),
            "续场:请 M1 继续",
        ),
        mod_tool_turn(
            "rs2",
            "end_discussion",
            serde_json::json!({"summary": "## 续跑共识"}),
            "续场收束",
        ),
    ]));
    let m1 = Arc::new(MockProvider::new(vec![text_turn("续跑后的 M1 发言")]));
    let mut catalog: ProviderCatalog = HashMap::new();
    catalog.insert("moderator".to_string(), moderator.clone());
    catalog.insert("m1".to_string(), m1.clone());
    let catalog = Arc::new(tokio::sync::RwLock::new(catalog));

    let emitter = Arc::new(MockEmitter::new());
    run_group_chat_loop(
        crate::tools::builtin_tools(),
        200_000,
        None,
        "rid-gc-resume".to_string(),
        gc_session_id.clone(),
        Vec::new(), // resume carries NO new user message
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
        fresh_controls(),
        Some(crate::agent::group_chat::GroupChatResume { start_round: 1 }),
    )
    .await;

    assert_eq!(emitter.error_event_count(), 0, "resume must not error");

    // RESUME instruction: first moderator turn only.
    let systems = moderator.sent_systems();
    assert_eq!(systems.len(), 2, "two moderator sends (nominate + end)");
    assert!(
        systems[0].as_deref().unwrap_or("").contains("## RESUME"),
        "first resumed moderator turn must carry the resume instruction"
    );
    assert!(
        !systems[1].as_deref().unwrap_or("").contains("## RESUME"),
        "subsequent rounds must use the plain moderator prompt"
    );

    // Reload, not the empty `messages` arg: the moderator's first
    // request must contain the seeded prior exchange.
    let first_send = &moderator.sent_messages()[0];
    let texts: Vec<String> = first_send.iter().map(|m| m.content.to_text()).collect();
    assert!(
        texts.iter().any(|t| t.contains("中断前的议题")),
        "resumed moderator must see the reloaded transcript, got: {:?}",
        texts
    );

    // Round budget: entering at round 1 with nominate@1 + end@2 —
    // both sends happened (above) and the discussion closed normally.
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
        Some("## 续跑共识")
    );

    // Terminal exit deletes the checkpoint row.
    assert!(
        db::get_group_chat_checkpoint(&h.db, &gc_session_id)
            .await
            .expect("get checkpoint")
            .is_none(),
        "group_chat_end exit must delete the checkpoint row"
    );
}

/// P1a AC1(行存留分流)+ 评审 P1-1 场景:error 熔断退出**保留**行
/// (round = 终局轮)、且新场开跑会**删旧行重建**(started_at 重置,
/// 复用 session 不携带上一场的断点身份)。
#[tokio::test]
async fn group_chat_error_exit_keeps_checkpoint_and_fresh_run_resets_it() {
    let (h, gc_session_id) = make_group_chat_harness().await;

    // 3 consecutive moderator error turns (the breaker path — the
    // existing error-breaker test's script).
    let error_stream = || {
        MockResponse::Events(vec![
            ok_evt(ChatEvent::Start),
            Err(LlmError::Auth("provider down".to_string())),
        ])
    };
    let moderator = Arc::new(MockProvider::new(vec![
        error_stream(),
        error_stream(),
        error_stream(),
        error_stream(),
    ]));
    let mut catalog: ProviderCatalog = HashMap::new();
    catalog.insert("moderator".to_string(), moderator.clone());
    let catalog = Arc::new(tokio::sync::RwLock::new(catalog));

    let emitter = Arc::new(MockEmitter::new());
    run_group_chat_loop(
        crate::tools::builtin_tools(),
        200_000,
        None,
        "rid-gc-err-keep".to_string(),
        gc_session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations.clone(),
        h.session_active_request.clone(),
        h.read_guard.clone(),
        h.memory_cache.clone(),
        h.skill_cache.clone(),
        h.permission_asks.clone(),
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        Some(catalog),
        Arc::new(crate::agent::subagent::ThreadLocalSubagentSink),
        h.subagent_cache.clone(),
        h.app_data_dir.clone(),
        h.question_store.clone(),
        group_chat_ctx(),
        fresh_controls(),
        None,
    )
    .await;

    let loaded = db::load_session(&h.db, &gc_session_id)
        .await
        .expect("load_session")
        .expect("session exists");
    assert_eq!(loaded.session.stop_reason.as_deref(), Some("error"));

    // Resumable exit: the row survives with the final round.
    let kept = db::get_group_chat_checkpoint(&h.db, &gc_session_id)
        .await
        .expect("get checkpoint")
        .expect("error exit must KEEP the checkpoint row");
    let first_started_at = kept.started_at.clone();
    assert_eq!(
        kept.round, 2,
        "breaker trips on round 2's third consecutive error"
    );

    // Fresh second run: the stale row is deleted at start; this run
    // ends cleanly (single end_discussion round) → row deleted at
    // exit. To observe the RESET (not just the terminal delete), the
    // second run errors again and the kept row's started_at must
    // differ from the first run's.
    let moderator2 = Arc::new(MockProvider::new(vec![
        error_stream(),
        error_stream(),
        error_stream(),
        error_stream(),
    ]));
    let mut catalog2: ProviderCatalog = HashMap::new();
    catalog2.insert("moderator".to_string(), moderator2.clone());
    let catalog2 = Arc::new(tokio::sync::RwLock::new(catalog2));

    run_group_chat_loop(
        crate::tools::builtin_tools(),
        200_000,
        None,
        "rid-gc-err-2nd".to_string(),
        gc_session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations.clone(),
        h.session_active_request.clone(),
        h.read_guard.clone(),
        h.memory_cache.clone(),
        h.skill_cache.clone(),
        h.permission_asks.clone(),
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        Some(catalog2),
        Arc::new(crate::agent::subagent::ThreadLocalSubagentSink),
        h.subagent_cache.clone(),
        h.app_data_dir.clone(),
        h.question_store.clone(),
        group_chat_ctx(),
        fresh_controls(),
        None,
    )
    .await;

    let second = db::get_group_chat_checkpoint(&h.db, &gc_session_id)
        .await
        .expect("get checkpoint")
        .expect("second error exit keeps its own row");
    assert_ne!(
        second.started_at, first_started_at,
        "a FRESH run must delete the stale row (new discussion identity)"
    );
}

/// P1a R5(cancelled 留行):mid-run cancel(Stop)退出保留断点行
/// —— resume 可拾起硬停的讨论。
#[tokio::test]
async fn group_chat_cancelled_exit_keeps_checkpoint_row() {
    let (h, gc_session_id) = make_group_chat_harness().await;

    // Sink hook: cancel the token when M1's speaker event fires
    // (mid-run, after round 0's checkpoint upsert).
    struct CancelOnM1 {
        inner: MockEmitter,
        token: CancellationToken,
    }
    impl crate::state::ChatEventSink for CancelOnM1 {
        fn emit_chat_event(&self, payload: &crate::state::ChatEventPayload) {
            if let ChatEvent::Speaker { speaker } = &payload.event {
                if speaker == "M1" {
                    self.token.cancel();
                }
            }
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

    let moderator = Arc::new(MockProvider::new(vec![mod_tool_turn(
        "c1",
        "nominate_speaker",
        serde_json::json!({"name": "M1"}),
        "开场",
    )]));
    let m1 = Arc::new(MockProvider::new(vec![text_turn("被硬停的 M1 发言")]));
    let mut catalog: ProviderCatalog = HashMap::new();
    catalog.insert("moderator".to_string(), moderator.clone());
    catalog.insert("m1".to_string(), m1.clone());
    let catalog = Arc::new(tokio::sync::RwLock::new(catalog));

    let token = CancellationToken::new();
    let emitter = Arc::new(CancelOnM1 {
        inner: MockEmitter::new(),
        token: token.clone(),
    });
    run_group_chat_loop(
        crate::tools::builtin_tools(),
        200_000,
        None,
        "rid-gc-cancel-keep".to_string(),
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
        token,
        None,
        h.background_shells.clone(),
        Some(catalog),
        Arc::new(crate::agent::subagent::ThreadLocalSubagentSink),
        h.subagent_cache.clone(),
        h.app_data_dir.clone(),
        h.question_store.clone(),
        group_chat_ctx(),
        fresh_controls(),
        None,
    )
    .await;

    let loaded = db::load_session(&h.db, &gc_session_id)
        .await
        .expect("load_session")
        .expect("session exists");
    assert_eq!(
        loaded.session.stop_reason.as_deref(),
        Some("cancelled"),
        "hard Stop persists its reason (GC2)"
    );
    assert!(
        db::get_group_chat_checkpoint(&h.db, &gc_session_id)
            .await
            .expect("get checkpoint")
            .is_some(),
        "cancelled (resumable) exit must KEEP the checkpoint row"
    );
}

// ---------------------------------------------------------------------------
// M4a(09-07-gce-m4a-scheduled-deliberation Step 5):定时场终态自动导转录。
// 守卫 = gc_ctx.created_via == Some("scheduled")(评审 P1-4);失败仅 warn
// 不影响终态(M2 先例)。非 scheduled 场不导出(对照组)。
// ---------------------------------------------------------------------------

/// 建「定时场」harness:session metadata 携带 created_via/scheduled_task_name
/// + 参与者;ctx 的 created_via = Some("scheduled")。
async fn make_scheduled_group_chat_harness(
    created_via: Option<&str>,
) -> (TestHarness, String, GroupChatCtx) {
    let h = make_harness().await;
    let gc_session_id = uuid::Uuid::new_v4().to_string();
    let metadata = serde_json::json!({
        "participants": [
            {"name": "M1", "model": "m1", "persona_md": M1_PERSONA},
            {"name": "M2", "model": "m2"}
        ],
        "created_via": created_via,
        "scheduled_task_id": "task-1",
        "scheduled_task_name": "每周审议",
    });
    db::create_session(
        &h.db,
        &gc_session_id,
        &h.project_id,
        h.project_path.to_str().unwrap(),
        "moderator",
        Some("moderator"),
        Some("group_chat"),
        Some(&metadata.to_string()),
    )
    .await
    .expect("create group_chat session");
    let ctx = GroupChatCtx {
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
        project_root: None,
        created_via: created_via.map(str::to_string),
        token_budget: None,
    };
    (h, gc_session_id, ctx)
}

/// 定时场正常收官(group_chat_end)→ 转录自动落
/// `{app_data_dir}/discussions/`,头部含任务名 / 参与者 / summary。
#[tokio::test]
async fn scheduled_discussion_exports_transcript_on_terminal_exit() {
    let (h, gc_session_id, ctx) = make_scheduled_group_chat_harness(Some("scheduled")).await;
    let emitter = Arc::new(MockEmitter::new());
    let mocks = script_group_chat_mocks();

    run_group_chat_loop(
        crate::tools::builtin_tools(),
        200_000,
        None,
        "rid-gc-sched".to_string(),
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
        mocks.catalog.clone(),
        Arc::new(crate::agent::subagent::ThreadLocalSubagentSink),
        h.subagent_cache.clone(),
        h.app_data_dir.clone(),
        h.question_store.clone(),
        ctx,
        fresh_controls(),
        None,
    )
    .await;

    let loaded = db::load_session(&h.db, &gc_session_id)
        .await
        .expect("load")
        .expect("row");
    assert_eq!(
        loaded.session.stop_reason.as_deref(),
        Some("group_chat_end")
    );
    let dir = h.app_data_dir.join("discussions");
    let mut entries = std::fs::read_dir(&dir)
        .expect("discussions dir created")
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 1, "exactly one transcript file");
    let path = entries.remove(0).unwrap().path();
    let fname = path.file_name().unwrap().to_string_lossy().to_string();
    assert!(
        fname.contains("每周审议"),
        "filename carries the sanitized task name: {fname}"
    );
    assert!(fname.ends_with(&format!("-{}.md", &gc_session_id[..8])));
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("定时审议「每周审议」转录"));
    assert!(content.contains("M1/m1"));
    assert!(content.contains("stop_reason: group_chat_end"));
    assert!(content.contains("## discussion_summary"));
}

/// 对照组:非 scheduled 场(GUI/MCP/script)终态**不**导出。
#[tokio::test]
async fn non_scheduled_discussion_does_not_export_transcript() {
    let (h, gc_session_id, ctx) = make_scheduled_group_chat_harness(None).await;
    let mocks = script_group_chat_mocks();

    run_group_chat_loop(
        crate::tools::builtin_tools(),
        200_000,
        None,
        "rid-gc-manual".to_string(),
        gc_session_id.clone(),
        test_messages(),
        Arc::new(MockEmitter::new()),
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
        ctx,
        fresh_controls(),
        None,
    )
    .await;

    let loaded = db::load_session(&h.db, &gc_session_id)
        .await
        .expect("load")
        .expect("row");
    assert_eq!(
        loaded.session.stop_reason.as_deref(),
        Some("group_chat_end")
    );
    assert!(
        !h.app_data_dir.join("discussions").exists(),
        "non-scheduled discussions must not export"
    );
}

/// 导出失败(落点被同名文件占用 → create_dir_all 失败)仅降级 warn:
/// 终态照常落库、编排器照常退出(不影响 stop_reason / checkpoint 清理)。
#[tokio::test]
async fn transcript_export_failure_does_not_break_finalization() {
    let (h, gc_session_id, ctx) = make_scheduled_group_chat_harness(Some("scheduled")).await;
    // 占位文件卡住 discussions 路径 → create_dir_all 必败。
    std::fs::write(h.app_data_dir.join("discussions"), b"not a dir").unwrap();
    let mocks = script_group_chat_mocks();

    run_group_chat_loop(
        crate::tools::builtin_tools(),
        200_000,
        None,
        "rid-gc-fail".to_string(),
        gc_session_id.clone(),
        test_messages(),
        Arc::new(MockEmitter::new()),
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
        ctx,
        fresh_controls(),
        None,
    )
    .await;

    let loaded = db::load_session(&h.db, &gc_session_id)
        .await
        .expect("load")
        .expect("row");
    assert_eq!(
        loaded.session.stop_reason.as_deref(),
        Some("group_chat_end"),
        "terminal state finalized despite the export failure"
    );
    assert!(
        db::get_group_chat_checkpoint(&h.db, &gc_session_id)
            .await
            .expect("get checkpoint")
            .is_none(),
        "terminal-exit checkpoint cleanup still ran"
    );
}

// ---------------------------------------------------------------------------
// C1.1 ask-free (09-08-gc-c1-stoploss): a group-chat speaker's out-of-root
// read would be a Tier 4 permission ask in classic chat. In a discussion
// it must be denied INSTANTLY at the permission layer — no ask round-trip
// (no `permission_ask` emit, no 120s/8s window), the deny reason lands in
// the tool_result(is_error) content, and the speaker carries on. The deny
// reason string is the ask.rs `ASK_FREE_DENY_REASON` contract.
// ---------------------------------------------------------------------------

fn script_ask_free_mocks(out_path: &str) -> GroupChatMocks {
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
    // M1: turn 1 = out-of-root read (Tier 4 ask face), turn 2 = remark
    // (proves the deny was non-fatal and M1 adapted instead of stalling).
    let m1 = Arc::new(MockProvider::new(vec![
        MockResponse::Events(vec![
            ok_evt(ChatEvent::Start),
            ok_evt(ChatEvent::ToolCall {
                id: "r1".to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({"path": out_path}),
            }),
            tool_use_stop(),
        ]),
        text_turn("M1: 该路径不可读，跳过，结论 A"),
    ]));
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
async fn group_chat_ask_free_denies_out_of_bounds_ask_without_roundtrip() {
    let (h, gc_session_id) = make_group_chat_harness().await;

    // A path OUTSIDE the project root (the harness's project tempdir) —
    // the exact shape that burned D1's 5 × 120s permission waits.
    let outside = std::env::temp_dir().join("everlasting-ask-free-probe-deny.md");

    let emitter = Arc::new(MockEmitter::new());
    let mocks = script_ask_free_mocks(outside.to_str().unwrap());

    run_group_chat_loop(
        crate::tools::builtin_tools(),
        200_000,
        None,
        "rid-ask-free".to_string(),
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
        mocks.catalog.clone(),
        Arc::new(crate::agent::subagent::ThreadLocalSubagentSink),
        h.subagent_cache.clone(),
        h.app_data_dir.clone(),
        h.question_store.clone(),
        group_chat_ctx(),
        fresh_controls(),
        None,
    )
    .await;

    // Core C1.1 assertion: ZERO ask round-trips — the permission layer
    // denied synchronously (this also implies zero waiting: without the
    // ask-free short-circuit this test would emit one ask and stall in
    // the unattended 8s window before a synthetic deny).
    assert!(
        emitter.permission_asks.lock().unwrap().is_empty(),
        "group-chat speaker turns must never surface a permission ask"
    );

    // The deny reason (contract string) reached M1's next turn as the
    // tool_result(is_error) content — the LLM-adaptation channel.
    let m1_sends = mocks.m1.sent_messages();
    assert_eq!(
        mocks.m1.call_count(),
        2,
        "M1 must run TWO turns: denied read then the remark"
    );
    let saw_deny = m1_sends[1].iter().any(|m| match &m.content {
        MessageContent::Blocks(blocks) => blocks.iter().any(|b| match b {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
                ..
            } => {
                tool_use_id == "r1"
                    && *is_error
                    && content.contains(crate::agent::permissions::ask::ASK_FREE_DENY_REASON)
            }
            _ => false,
        }),
        _ => false,
    });
    assert!(
        saw_deny,
        "M1's second turn must see the ask-free deny tool_result: {:#?}",
        m1_sends[1]
    );

    // The discussion completed normally end-to-end.
    assert_eq!(emitter.error_event_count(), 0, "no errors end-to-end");
    let loaded = db::load_session(&h.db, &gc_session_id)
        .await
        .expect("load")
        .expect("row");
    assert_eq!(
        loaded.session.stop_reason.as_deref(),
        Some("group_chat_end"),
        "discussion finishes normally despite the mid-discussion deny"
    );
}

// ---------------------------------------------------------------------------
// C1.2 token budget (09-08-gc-c1-stoploss): three destructive scripts from
// the second-discussion consensus — 永不点名 / 狂调工具 / 每轮报错 — must
// each terminate deterministically at a ROUND HEAD once the declared
// budget (`GroupChatCtx.token_budget`, billed = input+output+cache
// creation+cache_read) is exceeded, with `stop_reason = "budget"` on the
// terminal Done AND the sessions row. Sessions that declare NO budget are
// byte-compatible with pre-C1.2 behavior (every other test in this file
// runs with `token_budget: None` — that IS the control group).
// ---------------------------------------------------------------------------

/// A clean text turn whose `Done` reports `tokens` input (billed).
fn text_turn_with_usage(text: &str, tokens: u32) -> MockResponse {
    MockResponse::Events(vec![
        ok_evt(ChatEvent::Start),
        ok_evt(ChatEvent::Delta {
            text: text.to_string(),
        }),
        ok_evt(ChatEvent::Done {
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage {
                input_tokens: tokens,
                ..TokenUsage::default()
            }),
        }),
    ])
}

/// A tool round (tool_use stop) whose `Done` reports `tokens` input.
fn tool_round_with_usage(id: &str, path: &str, tokens: u32) -> MockResponse {
    MockResponse::Events(vec![
        ok_evt(ChatEvent::Start),
        ok_evt(ChatEvent::ToolCall {
            id: id.to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({ "path": path }),
        }),
        ok_evt(ChatEvent::Done {
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage {
                input_tokens: tokens,
                ..TokenUsage::default()
            }),
        }),
    ])
}

/// Last terminal `stop_reason` the emitter saw (the orchestrator's
/// post-loop Done is the only one carrying `budget`).
fn last_terminal_stop_reason(emitter: &MockEmitter) -> Option<String> {
    emitter
        .chat_events()
        .iter()
        .filter_map(|p| match &p.event {
            ChatEvent::Done { stop_reason, .. } => stop_reason.clone(),
            _ => None,
        })
        .last()
}

#[tokio::test]
async fn group_chat_budget_halts_when_moderator_never_nominates() {
    let (h, gc_session_id) = make_group_chat_harness().await;

    // Moderator burns 100 billed tokens per round and NEVER nominates —
    // with no budget this runs to MAX_ORCHESTRATION_ROUNDS (max_rounds);
    // with budget=250 it must halt deterministically at a round head
    // (after the 3rd turn spent = 300 > 250).
    let moderator = Arc::new(MockProvider::new(
        (0..5)
            .map(|i| text_turn_with_usage(&format!("独白 {i}"), 100))
            .collect::<Vec<_>>(),
    ));
    let mut catalog: ProviderCatalog = HashMap::new();
    catalog.insert("moderator".to_string(), moderator.clone());
    let catalog = Arc::new(tokio::sync::RwLock::new(catalog));

    let mut ctx = group_chat_ctx();
    ctx.token_budget = Some(250);

    let emitter = Arc::new(MockEmitter::new());
    run_group_chat_loop(
        crate::tools::builtin_tools(),
        200_000,
        None,
        "rid-budget-1".to_string(),
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
        ctx,
        fresh_controls(),
        None,
    )
    .await;

    assert_eq!(
        mocks_call_count(&moderator),
        3,
        "3 turns burned before the round-head trip"
    );
    assert_eq!(
        last_terminal_stop_reason(&emitter).as_deref(),
        Some("budget")
    );
    let loaded = db::load_session(&h.db, &gc_session_id)
        .await
        .expect("load")
        .expect("row");
    assert_eq!(loaded.session.stop_reason.as_deref(), Some("budget"));
}

#[tokio::test]
async fn group_chat_budget_halts_on_tool_loop_burn() {
    let (h, gc_session_id) = make_group_chat_harness().await;

    let notes_path = h.project_path.join("budget-notes.md");
    tokio::fs::write(&notes_path, "# notes\n")
        .await
        .expect("seed");
    let notes_abs = notes_path.to_str().unwrap().to_string();

    // M1's ONE speaker turn loops 3 tool rounds × 400 billed tokens, then
    // a 50-token remark — 1250 > budget=1000 accumulated WITHIN the turn
    // (the 逐块累计 face). The next round head must halt before anyone
    // else speaks.
    let m1 = Arc::new(MockProvider::new(vec![
        tool_round_with_usage("b1", &notes_abs, 400),
        tool_round_with_usage("b2", &notes_abs, 400),
        tool_round_with_usage("b3", &notes_abs, 400),
        text_turn_with_usage("M1 结论", 50),
    ]));
    let moderator = Arc::new(MockProvider::new(vec![
        mod_tool_turn(
            "c1",
            "nominate_speaker",
            serde_json::json!({"name": "M1"}),
            "请 M1",
        ),
        // Never reached — budget trips at the round-1 head.
        mod_tool_turn("c2", "end_discussion", serde_json::json!({}), "结束"),
    ]));
    let mut catalog: ProviderCatalog = HashMap::new();
    catalog.insert("moderator".to_string(), moderator.clone());
    catalog.insert("m1".to_string(), m1.clone());
    let catalog = Arc::new(tokio::sync::RwLock::new(catalog));

    let mut ctx = group_chat_ctx();
    ctx.token_budget = Some(1000);

    let emitter = Arc::new(MockEmitter::new());
    run_group_chat_loop(
        crate::tools::builtin_tools(),
        200_000,
        None,
        "rid-budget-2".to_string(),
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
        ctx,
        fresh_controls(),
        None,
    )
    .await;

    assert_eq!(
        mocks_call_count(&moderator),
        1,
        "round-1 head trips before the 2nd arbitration"
    );
    assert_eq!(
        last_terminal_stop_reason(&emitter).as_deref(),
        Some("budget")
    );
    let loaded = db::load_session(&h.db, &gc_session_id)
        .await
        .expect("load")
        .expect("row");
    assert_eq!(loaded.session.stop_reason.as_deref(), Some("budget"));
}

#[tokio::test]
async fn group_chat_budget_halts_on_error_turns_with_usage() {
    let (h, gc_session_id) = make_group_chat_harness().await;

    // Error turns report NO usage (Done never fires) — they contribute 0
    // to the tally (documented boundary; the GC5 breaker owns pure-error
    // burn). M1 errors every time; M2's clean turns burn 600 billed each.
    // budget=500: after M2's first clean turn spent=600 > 500 → the
    // round-2 head halts on BUDGET, before M1's second error can stack
    // the GC5 breaker (which would have said "error").
    let boom = || MockResponse::ErrThenEnd(LlmError::Auth("simulated auth failure".to_string()));
    let m1 = Arc::new(MockProvider::new(vec![boom()]));
    let m2 = Arc::new(MockProvider::new(vec![text_turn_with_usage(
        "M2 发言",
        600,
    )]));
    let moderator = Arc::new(MockProvider::new(vec![
        mod_tool_turn(
            "c1",
            "nominate_speaker",
            serde_json::json!({"name": "M1"}),
            "请 M1",
        ),
        mod_tool_turn(
            "c2",
            "nominate_speaker",
            serde_json::json!({"name": "M2"}),
            "换 M2",
        ),
        // Never reached.
        mod_tool_turn("c3", "end_discussion", serde_json::json!({}), "结束"),
    ]));
    let mut catalog: ProviderCatalog = HashMap::new();
    catalog.insert("moderator".to_string(), moderator.clone());
    catalog.insert("m1".to_string(), m1.clone());
    catalog.insert("m2".to_string(), m2.clone());
    let catalog = Arc::new(tokio::sync::RwLock::new(catalog));

    let mut ctx = group_chat_ctx();
    ctx.token_budget = Some(500);

    let emitter = Arc::new(MockEmitter::new());
    run_group_chat_loop(
        crate::tools::builtin_tools(),
        200_000,
        None,
        "rid-budget-3".to_string(),
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
        ctx,
        fresh_controls(),
        None,
    )
    .await;

    assert_eq!(
        last_terminal_stop_reason(&emitter).as_deref(),
        Some("budget"),
        "budget must win over the error breaker when billed tokens accumulated"
    );
    let loaded = db::load_session(&h.db, &gc_session_id)
        .await
        .expect("load")
        .expect("row");
    assert_eq!(loaded.session.stop_reason.as_deref(), Some("budget"));
}

fn mocks_call_count(p: &std::sync::Arc<MockProvider>) -> usize {
    p.call_count()
}
