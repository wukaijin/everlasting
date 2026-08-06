//! Group-chat turn-taking orchestrator (07-29-group-chat, Phase 3.4;
//! 08-04 rewrite — `08-04-group-chat-orchestration-rewrite`).
//!
//! `run_group_chat_loop` is an OUTER loop that drives a multi-LLM
//! discussion by repeatedly calling `run_chat_loop` (one call per
//! speaker). All speakers share the same `messages: Vec<ChatMessage>`
//! (reloaded from the DB between turns — `run_chat_loop` returns `()`
//! and persists its state), which is what makes the conversation
//! mutually visible (互见性): each speaker sees every prior utterance.
//!
//! # 08-04 rewrite (design.md)
//!
//! The pre-rewrite orchestrator reloaded the FULL message history and
//! fed it verbatim into every speaker's `run_chat_loop`. That produced
//! three intertwined defects:
//!   1. the previous speaker's already-persisted `tool_result` (role =
//!      user) became the next speaker's tail user message and was
//!      re-persisted → duplicate `tool_result` rows → OpenAI 400 /
//!      Anthropic 2013 → `[生成出错中断]` death loop;
//!   2. a participant's transcript contained the moderator's
//!      `nominate_speaker` / `end_discussion` tool interaction →
//!      identity confusion (a participant thought it WAS the moderator);
//!   3. participants received the arbitration tools (fixed by
//!      `participant_tool_defs`, commit `d2c7c32`, retained).
//!
//! The rewrite changes the per-speaker entry transcript:
//! - **round 0** uses the caller-supplied `messages` (tail = the new
//!   human message, which the entry guard in `run_chat_loop` persists
//!   normally — design D-D);
//! - **later rounds** reload once into `full`, then the moderator gets
//!   `full` verbatim (View-1) while each participant gets
//!   [`participant_view`] (View-2, design D-A) — arbitration
//!   tool_use↔tool_result pairs are stripped as one atomic unit
//!   (llm-contract.md §469) so the participant's identity can't be
//!   confused and no orphan tool block survives.
//!
//! `reload_messages` is retained (design D-B): `run_chat_loop` returns
//! `()` and the orchestrator cannot know which rows it appended, so a
//! reload between speakers is the only resync mechanism. Reload is
//! safe because the entry guard (D-D) prevents re-persisting already-
//! persisted rows.
//!
//! Turn-taking (D3 = moderator-driven + human preemption):
//! ```text
//! loop {
//!   1. moderator turn  → run_chat_loop(moderator, tools=[nominate,end], state)
//!      └─ moderator calls nominate_speaker(X) or end_discussion
//!   2. read SharedTurnState
//!   3. if end_discussion → break
//!   4. resolve nominee X → (provider, system_prompt) from GroupChatCtx
//!   5. reload messages from DB → participant_view()
//!   6. participant X turn → run_chat_loop(X_provider, X_prompt, max_turns=1, speaker=X)
//!   └─ back to 1 (round>0 reloads the moderator view)
//! }
//! ```
//!
//! Fallback (D7 risk): if the moderator fails to nominate (no tool
//! call / unknown name / empty participants), we fall back to
//! round-robin over the participants for one cycle, then give the
//! moderator another chance. After MAX_ORCHESTRATION_ROUNDS we stop.

use std::sync::Arc;

use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

use crate::agent::chat_loop::run_chat_loop;
use crate::agent::group_chat::GroupChatCtx;
use crate::agent::helpers::emit_chat_event_via_sink;
use crate::background_shell::DefaultRegistry;
use crate::db;
use crate::llm::types::{ChatEvent, ChatMessage, ContentBlock, MessageContent, Role, ToolDef};
use crate::memory::MemoryCache;
use crate::skill::loader::SkillCache;
use crate::state::ProviderCatalog;
use crate::tools::end_discussion::END_DISCUSSION_TOOL_NAME;
use crate::tools::nominate_speaker::{
    GroupChatTurnState, SharedTurnState, NOMINATE_SPEAKER_TOOL_NAME,
};
use crate::tools::read_guard::ReadGuard;

/// Hard cap on moderator↔participant round-trips. Prevents a
/// runaway loop where the moderator keeps nominating forever. The
/// D10 context-window compaction still applies per-turn inside each
/// `run_chat_loop`; this is an outer-loop safety bound.
const MAX_ORCHESTRATION_ROUNDS: usize = 30;

/// How many consecutive times the moderator may end its turn WITHOUT
/// calling `nominate_speaker` / `end_discussion` before we give up.
///
/// Root-cause note (sessions a6c87247 / 4a9d3566 / 093823f3): the OLD
/// behavior on "no nomination" was a **round-robin fallback** — it
/// mechanically dispatched `participants[round % len]`. But the
/// moderator often uses natural language to signal intent ("接下来请
/// D4F…") without calling the tool, so round-robin dispatched the
/// WRONG participant. The mis-dispatched speaker then mimicked the
/// intended speaker's voice → full role confusion.
///
/// Fix: instead of round-robin, RETRY the moderator turn (no
/// participant is dispatched) so it gets another chance to call the
/// tool. After this many retries with no nomination, we stop the
/// discussion (the moderator is stuck / the model isn't tool-calling).
const MAX_NO_NOMINATE_STREAK: usize = 3;

/// The moderator's system prompt. Tells it to facilitate + use the
/// `nominate_speaker` / `end_discussion` tools. Built once per
/// orchestration entry (participants list is stable for the run).
fn moderator_system_prompt(ctx: &GroupChatCtx) -> String {
    let roster: Vec<String> = ctx
        .participants
        .iter()
        .map(|p| format!("- {} (model: {})", p.name, p.model))
        .collect();
    format!(
        "You are the MODERATOR of a group chat discussion. Multiple AI participants \
         take turns discussing a topic the user raised. Your job:\n\
         \n\
         1. Facilitate: open the discussion, summarize progress, keep it on track.\n\
         2. Hand the floor to ONE participant at a time by calling \
         `nominate_speaker({{name: \"...\"}})`. Use the participant's exact name.\n\
         3. When the discussion has covered the topic, call \
         `end_discussion({{summary: \"...\"}})`.\n\
         \n\
         The system already labels who is speaking — never start your reply with your \
         OWN name or role (no \"moderator:\" / \"主持人:\" prefix). You MAY address a \
         participant by name in the body when directly replying to them.\n\
         \n\
         Participants (you are NOT one of them — you moderate):\n{}\n\
         \n\
         Participants can see each other's prior remarks, so they can respond, \
         agree, or push back. Pick the order that best explores the topic. Keep \
         the discussion focused; end it when it has run its course.",
        roster.join("\n")
    )
}

/// Nudge appended to the moderator's system prompt when it ended its
/// previous turn(s) without calling `nominate_speaker` /
/// `end_discussion`. Forces a tool call so the orchestrator can
/// dispatch the right participant (or stop). See
/// [`MAX_NO_NOMINATE_STREAK`] for the retry design.
fn moderator_nudge(streak: usize) -> String {
    format!(
        "\n\n\
         IMPORTANT: your previous {streak} turn(s) did NOT call a tool — you spoke \
         but did not hand the floor to anyone. You MUST end THIS turn by calling \
         either `nominate_speaker({{name: \"...\"}})` (to let a participant speak) \
         or `end_discussion({{summary: \"...\"}})` (to close the discussion). \
         Do NOT output only text.",
        streak = streak
    )
}

/// The participant's transcript view (design.md D-A, §4 View-2).
///
/// Scans `full` (the reloaded DB transcript, seq-ascending) and strips
/// every arbitration tool interaction — an assistant row carrying a
/// `nominate_speaker` / `end_discussion` `ToolUse` block plus the
/// immediately-following user row carrying the matching `tool_result`.
/// The strip is **atomic per pair** (llm-contract.md §469): either both
/// the assistant(tool_use) and its user(tool_result) stay, or both go,
/// so the participant's request body never contains an orphan
/// `tool_use` / `tool_result`.
///
/// Only the arbitrator (the moderator) legitimately uses these tools,
/// so they always appear as "someone else's history" from a
/// participant's point of view. Showing them caused the identity-
/// confusion defect (a participant concluded it WAS the moderator and
/// replied as @moderator — DB evidence in research/db-evidence.md §2).
///
/// Rules (design §4):
/// - an assistant row whose blocks include an arbitration ToolUse:
///   keep its non-tool blocks (thinking / text), drop the ToolUse
///   blocks, and skip the immediately-following user row;
/// - an assistant row that consists ONLY of arbitration ToolUse blocks
///   → the whole row is dropped (its user(tool_result) row is still
///   skipped);
/// - everything else (human messages, moderator/participant text,
///   non-arbitration tool pairs) passes through unchanged.
///
/// The `full` rows are DB-persisted per turn, so an arbitration pair
/// is always adjacent in `full` (the tool_result is persisted right
/// after the tool_use assistant row within the same moderator turn) —
/// a one-pass state machine suffices, no backtracking needed.
fn participant_view(full: &[ChatMessage]) -> Vec<ChatMessage> {
    let mut out: Vec<ChatMessage> = Vec::with_capacity(full.len());
    let mut pending_arbitration_tool_use_id: Option<String> = None;
    for m in full {
        if let Some(ref id) = pending_arbitration_tool_use_id {
            // Skip the user row that carries this arbitration pair's
            // tool_result. The row is skipped if ANY of its blocks is
            // the pending pair's tool_result (persisted per-turn rows
            // are a single tool_result for the arbitration call, so in
            // practice the whole row belongs to the pair; a mixed row
            // would be dropped wholesale, which is safe — it can only
            // lose non-arbitration blocks already stripped from the
            // assistant side, never creating an orphan).
            let is_result_row = matches!(
                &m.content,
                MessageContent::Blocks(blocks)
                    if blocks.iter().any(|b| matches!(
                        b,
                        ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == id
                    ))
            );
            pending_arbitration_tool_use_id = None;
            if is_result_row {
                continue;
            }
        }
        let (kept, pending) = participant_view_row(m);
        if let Some(k) = kept {
            out.push(k);
        }
        pending_arbitration_tool_use_id = pending;
    }
    out
}

/// Single-row filter for [`participant_view`]: returns
/// `(Some(row), Some(tool_use_id))` when the row is an assistant row
/// whose arbitration ToolUse pair must be followed by skipping the next
/// user row.
fn participant_view_row(m: &ChatMessage) -> (Option<ChatMessage>, Option<String>) {
    let MessageContent::Blocks(blocks) = &m.content else {
        return (Some(m.clone()), None);
    };
    let arbitration_ids: Vec<String> = blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolUse { id, name, .. }
                if name == NOMINATE_SPEAKER_TOOL_NAME || name == END_DISCUSSION_TOOL_NAME =>
            {
                Some(id.clone())
            }
            _ => None,
        })
        .collect();
    if arbitration_ids.is_empty() {
        // No arbitration tool here — pass the row through unchanged
        // (this includes the moderator's NON-arbitration tool pairs).
        return (Some(m.clone()), None);
    }
    // This assistant row carries arbitration tool use(s). Keep the
    // non-tool blocks (thinking / text) so the participant still sees
    // the moderator's reasoning and remarks; drop the tool blocks.
    let kept_blocks: Vec<ContentBlock> = blocks
        .iter()
        .filter(|b| !matches!(b, ContentBlock::ToolUse { .. }))
        .cloned()
        .collect();
    let first_id = arbitration_ids[0].clone();
    if kept_blocks.is_empty() {
        // Pure tool row → drop the whole row (still expect a
        // following tool_result row to skip).
        (None, Some(first_id))
    } else {
        (
            Some(ChatMessage {
                role: m.role,
                content: MessageContent::Blocks(kept_blocks),
                speaker: m.speaker.clone(),
            }),
            Some(first_id),
        )
    }
}

/// Strip the two moderator-only arbitration tools (`nominate_speaker`
/// / `end_discussion`) from the tool list a participant sees. Only the
/// moderator arbitrates the floor; if a participant is given these
/// tools it tends to call `nominate_speaker` itself, and because the
/// participant's `run_chat_loop` runs with `group_chat_state = None`
/// the interception returns an error tool_result that poisons the
/// reloaded transcript (every subsequent turn errors → loop to
/// MAX_ORCHESTRATION_ROUNDS). Filtering at the source means the model
/// never even sees the tool schema.
fn participant_tool_defs(tool_defs: &[ToolDef]) -> Vec<ToolDef> {
    tool_defs
        .iter()
        .filter(|t| t.name != NOMINATE_SPEAKER_TOOL_NAME && t.name != END_DISCUSSION_TOOL_NAME)
        .cloned()
        .collect()
}

/// A participant's system prompt = its persona (D8: inline markdown
/// from metadata) PLUS an identity-guard block. If no persona, a
/// minimal default so the model knows it's in a group discussion.
///
/// 08-04 rewrite follow-up (identity-confusion residual, user-approved
/// "Prompt 强化 + moderator 单轮"): the wire layer labels each assistant
/// message with its author (OpenAI `name` / Anthropic `@name:` prefix),
/// but weak models routinely ignore the label. DB evidence from the
/// 4th group-chat test (session `b144cc2a-…`, seq 4) shows participant
/// M3 concluding "I (as the system) am prompting the conversation. The
/// first turn was my moderator opening" and replying as `@moderator:`.
/// The identity-guard block below states the role boundaries
/// explicitly so the participant never adopts the moderator's voice.
/// Appended to BOTH the persona and the default template — a persona
/// only describes the persona, it does not defend against role
/// confusion.
fn participant_system_prompt(name: &str, persona_md: Option<&str>) -> String {
    let base = match persona_md {
        Some(p) if !p.trim().is_empty() => p.to_string(),
        _ => format!(
            "You are {}, a participant in a group chat discussion led by a moderator. \
             You can see what everyone else has said. Respond to the topic and to \
             other participants — agree, disagree, build on, or question their points. \
             Be concise and substantive.",
            name
        ),
    };
    format!(
        "{}\n\n\
         ## Group-chat roles (read carefully)\n\
         - You are {} — one of the PARTICIPANTS. A separate moderator runs the discussion\n\
           and assigns turns; you never do.\n\
         - Every message is labeled with its author (user / moderator / {} / other\n\
           participants). The moderator's messages are NOT yours — never reply in the\n\
           moderator's voice and never act as the moderator (no summing up the whole\n\
           discussion, no handing the floor, no nominating speakers, no opening or\n\
           closing the conversation).\n\
         - The system already shows who is speaking, so never start your reply with\n\
           your OWN name or role (no \"{}:\" / \"主持人:\" prefix) — just speak directly.\n\
           You MAY address another participant in the body of your reply, with or\n\
           without an @ (e.g. \"@D4F，你说得对…\" or \"D4F，你说得对…\"), when you are\n\
           directly answering them.\n\
         - Never refer to yourself in the third person.\n\
         - Just say your own piece on the topic and respond to what others said.",
        base, name, name, name
    )
}
/// Reload the session's messages from the DB as `Vec<ChatMessage>`.
/// `run_chat_loop` returns `()` and persists turns to the DB, so the
/// orchestrator reloads between speakers to get the latest shared
/// transcript (each speaker then sees all prior utterances).
async fn reload_messages(db: &SqlitePool, session_id: &str) -> Vec<ChatMessage> {
    let loaded = match db::load_session(db, session_id).await {
        Ok(Some(l)) => l,
        _ => return Vec::new(),
    };
    loaded
        .messages
        .into_iter()
        .map(|m| {
            // MessageRow.content is a serialized MessageContent JSON
            // value; role is a "user"/"assistant" string; speaker is
            // carried on the row's `speaker` column (Phase 4 TODO-B,
            // wired up in `db::types::MessageRow` + `db::load_session`).
            // The reload MUST round-trip the speaker so the next
            // speaker sees the prior speaker's attribution correctly
            // (this is what enables the frontend's per-utterance
            // chip rendering + reload consistency).
            let role = match m.role.as_str() {
                "assistant" => Role::Assistant,
                _ => Role::User,
            };
            let content: MessageContent =
                serde_json::from_value(m.content).unwrap_or(MessageContent::Text(m.text.clone()));
            ChatMessage {
                role,
                content,
                speaker: m.speaker,
            }
        })
        .collect()
}

/// The outer orchestrator. Invoked from `chat_inner` when
/// `session_type == group_chat` (Phase 3.5). Mirrors the parameter
/// set `chat_inner`'s spawn closure passes to `run_chat_loop`, so it
/// can forward them to each per-speaker `run_chat_loop` call.
///
/// Returns `()` like `run_chat_loop` — all state is persisted to the
/// DB + emitted via `sink`.
///
/// # 08-04 rewrite (design.md §6) — per-speaker entry transcripts
///
/// - **round 0**: the moderator enters with the caller-supplied
///   `messages` verbatim (tail = the freshly-composed human message,
///   which the entry guard in `run_chat_loop` persists normally —
///   D-D). No reload here: the session has no prior rows yet.
/// - **round > 0**: reload once into `full` (D-B — `run_chat_loop`
///   returns `()` and the orchestrator cannot know which rows it
///   appended, so a reload is the only resync). The moderator gets
///   `full` verbatim (View-1, its own arbitration history is kept).
/// - **participant turn**: reload again into `full` (fresh — the
///   moderator's just-persisted rows must be visible) then enter with
///   [`participant_view`]`(&full)` (View-2, D-A): arbitration
///   tool_use↔tool_result pairs stripped as atomic units.
#[allow(clippy::too_many_arguments)]
pub async fn run_group_chat_loop(
    tool_defs: Vec<ToolDef>,
    context_window: u32,
    rid: String,
    session_id: String,
    messages: Vec<ChatMessage>,
    sink: Arc<dyn crate::state::ChatEventSink>,
    db: SqlitePool,
    cancellations: Arc<tokio::sync::Mutex<std::collections::HashMap<String, CancellationToken>>>,
    session_active_request: Arc<tokio::sync::Mutex<std::collections::HashMap<String, String>>>,
    read_guard: ReadGuard,
    memory_cache: Arc<MemoryCache>,
    skill_cache: Arc<SkillCache>,
    permission_asks: crate::agent::permissions::PermissionStore,
    token: CancellationToken,
    resend_seq: Option<i64>,
    background_shells: DefaultRegistry,
    worker_catalog: Option<Arc<tokio::sync::RwLock<ProviderCatalog>>>,
    worker_event_sink: Arc<dyn crate::agent::subagent::SubagentEventSink>,
    subagent_cache: Arc<crate::agent::subagent::SubagentCache>,
    app_data_dir: std::path::PathBuf,
    question_store: crate::agent::question_store::QuestionStore,
    gc_ctx: GroupChatCtx,
) {
    // Shared turn state — the moderator's run_chat_loop writes here
    // via the nominate_speaker / end_discussion interception.
    let turn_state: SharedTurnState = Arc::new(tokio::sync::Mutex::new(GroupChatTurnState {
        next_speaker: None,
        discussion_ended: false,
    }));

    let moderator_prompt = moderator_system_prompt(&gc_ctx);
    let moderator_provider =
        resolve_provider(&worker_catalog, &gc_ctx.moderator_model_id, &db).await;

    // Consecutive moderator turns that ended WITHOUT a
    // `nominate_speaker` / `end_discussion` call. Each such turn bumps
    // this; a nomination / end resets it to 0. At MAX_NO_NOMINATE_STREAK
    // we stop retrying (the moderator is stuck). See the fallback logic
    // below the turn-state read for the full rationale.
    let mut no_nominate_streak: usize = 0;

    for round in 0..MAX_ORCHESTRATION_ROUNDS {
        if token.is_cancelled() {
            break;
        }

        // --- 1. Moderator turn ------------------------------------------------
        // The moderator gets the nominate/end tools (already in
        // builtin_tools) + the shared turn state. Its system prompt
        // is fully replaced (system_prompt_override = Some).
        //
        // View-1 (design §4): the moderator sees `full` verbatim —
        // including its OWN arbitration history from prior rounds
        // (跨轮连贯, AC5). round 0 uses the caller-supplied `messages`
        // (tail = the new human message); later rounds reload once.
        let full = if round == 0 {
            messages.clone()
        } else {
            reload_messages(&db, &session_id).await
        };
        // 08-04 follow-up (实时 speaker 标识): announce the upcoming
        // speaker BEFORE the inner `run_chat_loop` so the frontend can
        // stamp this speaker's name on the placeholder that's about to
        // stream (the per-speaker `Delta` / `Done` wire events carry no
        // speaker). The frontend's `speaker` case stashes it and
        // `start` applies it to the freshly-pushed placeholder.
        emit_chat_event_via_sink(
            &sink,
            &rid,
            &ChatEvent::Speaker {
                speaker: "moderator".to_string(),
            },
        );
        if let Some(provider) = &moderator_provider {
            // Build the prompt: base + nudge if this is a retry (the
            // moderator ended prior turns without calling a tool).
            let prompt = if no_nominate_streak > 0 {
                format!(
                    "{}{}",
                    moderator_prompt,
                    moderator_nudge(no_nominate_streak)
                )
            } else {
                moderator_prompt.clone()
            };
            run_chat_loop(
                tool_defs.clone(),
                provider.clone(),
                context_window,
                rid.clone(),
                session_id.clone(),
                full,
                sink.clone(),
                db.clone(),
                cancellations.clone(),
                session_active_request.clone(),
                read_guard.clone(),
                memory_cache.clone(),
                skill_cache.clone(),
                permission_asks.clone(),
                token.clone(),
                resend_seq,
                background_shells.clone(),
                // Single turn (08-04 follow-up, user-approved "moderator
                // 单轮"): the moderator must nominate / end in ONE turn.
                // The pre-follow-up `Some(3)` let it burn a second LLM
                // call on turn-2 filler text ("（已把话筒交给 M3，等待发
                // 言…）") — that filler is first-person arbitration
                // narration which weak participants misread as their own
                // voice (identity confusion, DB session b144cc2a seq 3-4).
                // With max_turns=1 the turn ends right after the
                // tool_result, and the moderator only speaks again on its
                // next round (retry / nomination loop).
                Some(1),
                false, // owns session_active slot
                false, // persist (moderator turns are part of the record)
                Some(false),
                worker_catalog.clone(),
                worker_event_sink.clone(),
                Some(prompt),
                None,
                subagent_cache.clone(),
                None,
                None,
                None,
                app_data_dir.clone(),
                None, // forced_dispatch
                question_store.clone(),
                None, // workflow_ctx
                Some(turn_state.clone()),
                // Group chat (Phase 4 TODO-A): the moderator is a
                // distinct speaker — its assistant turns persist with
                // `speaker = "moderator"` (the fixed identifier; the
                // moderator's own persona is the built-in
                // `moderator_system_prompt` template, NOT a per-
                // session customizable config). The frontend renders
                // this as the "主持人" chip; reload reads it back
                // from the same column so cross-turn visibility is
                // preserved.
                Some("moderator".to_string()),
            )
            .await;
        }

        // --- 2/3. Read turn state ---------------------------------------------
        let (next_speaker, ended) = {
            let mut st = turn_state.lock().await;
            (st.next_speaker.take(), st.discussion_ended)
        };
        if ended {
            tracing::info!(round, "group_chat: moderator ended discussion");
            break;
        }

        let nominee_name = match next_speaker {
            Some(n) => {
                // The moderator nominated — reset the streak and proceed.
                no_nominate_streak = 0;
                n
            }
            None => {
                // The moderator ended its turn WITHOUT calling
                // nominate_speaker / end_discussion. The OLD behavior
                // (pre-08-06) was a round-robin fallback: dispatch
                // `participants[round % len]`. That was the ROOT CAUSE of
                // the group-chat role confusion across three sessions
                // (a6c87247 / 4a9d3566 / 093823f3): the moderator often
                // signals intent in natural language ("接下来请 D4F…")
                // without calling the tool, so round-robin dispatched the
                // WRONG participant. The mis-dispatched speaker then
                // mimicked the intended speaker's voice → full role
                // collapse.
                //
                // Fix: DON'T dispatch anyone. Retry the moderator turn
                // (it will see its own prior non-tool text in the
                // reloaded history + get a nudge in its system prompt).
                // After MAX_NO_NOMINATE_STREAK retries, give up and stop.
                no_nominate_streak += 1;
                if no_nominate_streak > MAX_NO_NOMINATE_STREAK {
                    tracing::warn!(
                        round,
                        streak = no_nominate_streak,
                        "group_chat: moderator did not nominate after {} retries; stopping discussion",
                        MAX_NO_NOMINATE_STREAK
                    );
                    break;
                }
                tracing::warn!(
                    round,
                    streak = no_nominate_streak,
                    "group_chat: moderator did not nominate; retrying moderator turn (no participant dispatched)"
                );
                continue;
            }
        };

        // --- 4. Resolve nominee → provider + prompt ---------------------------
        let participant = match gc_ctx.participant_by_name(&nominee_name) {
            Some(p) => p.clone(),
            None => {
                tracing::warn!(round, nominee=%nominee_name, "group_chat: nominee not in roster; skipping turn");
                continue;
            }
        };
        let participant_provider = resolve_provider(&worker_catalog, &participant.model, &db).await;
        let participant_prompt =
            participant_system_prompt(&participant.name, participant.persona_md.as_deref());

        let Some(provider) = participant_provider else {
            tracing::warn!(round, model=%participant.model, "group_chat: participant provider unresolved; skipping turn");
            continue;
        };

        // --- 5. Reload + filter the shared transcript --------------------------
        // Reload once (D-B) to pick up the moderator's just-persisted
        // rows (its tool_result + text), then build the participant's
        // View-2 (D-A): arbitration tool_use↔tool_result pairs are
        // stripped atomically so the participant never sees the
        // moderator's floor-arbitration interaction (identity-confusion
        // root cause, research/db-evidence.md §2).
        let full = reload_messages(&db, &session_id).await;
        let view = participant_view(&full);

        // 08-04 follow-up (实时 speaker 标识): announce the participant
        // before its turn so the frontend stamps the name on the
        // placeholder about to stream.
        emit_chat_event_via_sink(
            &sink,
            &rid,
            &ChatEvent::Speaker {
                speaker: participant.name.clone(),
            },
        );

        // --- 6. Participant turn ----------------------------------------------
        // Single turn (max_turns=1): speak once, then hand back to
        // the moderator. The participant's system prompt is its
        // persona (fully replaces the parent prompt).
        run_chat_loop(
            // Participant does NOT get the nominate_speaker /
            // end_discussion arbitration tools — only the moderator
            // arbitrates the floor. Previously this passed the full
            // `builtin_tools()` (which include both), so a participant
            // that *chose* to call nominate_speaker hit the
            // `group_chat_state = None` interception below and got
            // back "this tool is only available in a group chat
            // session." That error tool_result then poisoned the
            // reloaded transcript, erroring every subsequent turn
            // and looping until MAX_ORCHESTRATION_ROUNDS. Filter the
            // two tools out at the source so participants never see
            // them (mirrors filter_tools_for_workflow's shape).
            participant_tool_defs(&tool_defs),
            provider,
            context_window,
            rid.clone(),
            session_id.clone(),
            // View-2 (D-A): arbitration pairs stripped — the tail user
            // row is an already-persisted message (the entry guard
            // skips re-persisting it) and the moderator's tool
            // interaction is invisible to the participant.
            view,
            sink.clone(),
            db.clone(),
            cancellations.clone(),
            session_active_request.clone(),
            read_guard.clone(),
            memory_cache.clone(),
            skill_cache.clone(),
            permission_asks.clone(),
            token.clone(),
            None,
            background_shells.clone(),
            Some(1), // single turn
            false,
            false,
            Some(false),
            worker_catalog.clone(),
            worker_event_sink.clone(),
            Some(participant_prompt),
            None,
            subagent_cache.clone(),
            None,
            None,
            None,
            app_data_dir.clone(),
            None,
            question_store.clone(),
            None,
            // group_chat_state — the participant shares the SAME
            // turn_state Arc so the D-D entry guard's scope condition
            // (`group_chat_state.is_some()`) holds and it skips
            // re-persisting the already-persisted tail user message
            // (e.g. the round-0 human text), satisfying PRD AC2
            // "无多写" / implement.md "round-0 人类消息恰 1 条".
            //
            // Deviation note (design.md §6 wrote `None` here): the
            // guard needs this scope for the participant too — design
            // §5's own reliability argument covers "每个 speaker 入口
            // transcript 的尾部 user 消息必是已落库行 → 跳过", which
            // only holds when the participant also passes Some.
            // Arbitration safety is NOT weakened: `participant_tool_defs`
            // above already strips nominate_speaker / end_discussion
            // from the participant's schema, so the interception branch
            // can never fire for a participant.
            Some(turn_state.clone()),
            // Group chat (Phase 4 TODO-A): each participant's
            // assistant turns persist with `speaker = participant.name`
            // (the user-visible display name + session-scoped unique
            // identifier). The reload fuses these into the next
            // speaker's view so every participant sees the full
            // transcript with the correct attribution.
            Some(participant.name.clone()),
        )
        .await;

        // --- 7. Loop back to the moderator --------------------------------
        // No reload here: the next iteration's `round > 0` branch reloads
        // `full` (fresh, includes this participant's rows) before the
        // moderator's next turn.
    }

    if !token.is_cancelled() {
        tracing::warn!(
            rounds = MAX_ORCHESTRATION_ROUNDS,
            "group_chat: hit max rounds; stopping"
        );
    }

    // Terminal signal for the frontend (08-04 follow-up, user-approved
    // "终止事件 + 逐轮流式"): the orchestrator shares ONE `rid` across
    // every inner `run_chat_loop` (moderator + participants), and each
    // inner turn emits its own `Done` (moderator `max_turns` /
    // participant `end_turn`). The frontend cannot know the discussion
    // has actually ENDED from those inner Dones — it would finalize
    // the request after the FIRST speaker turn and silently drop every
    // later event (the "群聊内容不实时出现" bug). Emit a dedicated
    // terminal `Done { stop_reason: "group_chat_end" }` so the
    // frontend keeps the request alive across inner turns and only
    // finalizes on this signal. Not emitted when cancelled (the
    // cancelled inner turn already emitted `Done { cancelled }`, which
    // the frontend treats as terminal).
    if !token.is_cancelled() {
        emit_chat_event_via_sink(
            &sink,
            &rid,
            &ChatEvent::Done {
                stop_reason: Some("group_chat_end".to_string()),
                usage: None,
            },
        );
    }
}

/// Resolve a model_id to a provider via the catalog. Tries the
/// shared `worker_catalog` (the live, hot-reloaded map) first; that
/// is the same catalog `chat_inner` uses. Returns `None` (logged) on
/// miss so the orchestrator can skip the turn rather than crash.
async fn resolve_provider(
    catalog: &Option<Arc<tokio::sync::RwLock<ProviderCatalog>>>,
    model_id: &str,
    _db: &SqlitePool,
) -> Option<Arc<dyn crate::llm::Provider>> {
    let catalog = catalog.as_ref()?;
    let guard = catalog.read().await;
    guard.get(model_id).cloned().or_else(|| {
        tracing::warn!(
            model_id,
            "group_chat: model_id not in catalog; turn will be skipped"
        );
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: participants must NOT receive the nominate_speaker /
    /// end_discussion arbitration tools. The moderator's full
    /// `builtin_tools()` set contains both; `participant_tool_defs`
    /// must strip them. Previously the participant call site passed
    /// the unfiltered set and a participant that called
    /// nominate_speaker poisoned the transcript.
    #[test]
    fn participant_tool_defs_strips_arbitration_tools() {
        let full = crate::tools::builtin_tools();
        let names: Vec<&str> = full.iter().map(|t| t.name.as_str()).collect();
        // Sanity: the source set actually contains both (otherwise the
        // filter is a silent no-op and the bug would be masked).
        assert!(
            names.contains(&NOMINATE_SPEAKER_TOOL_NAME),
            "builtin_tools must include nominate_speaker for this test to be meaningful"
        );
        assert!(
            names.contains(&END_DISCUSSION_TOOL_NAME),
            "builtin_tools must include end_discussion for this test to be meaningful"
        );

        let participant_tools = participant_tool_defs(&full);
        let p_names: Vec<&str> = participant_tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            !p_names.contains(&NOMINATE_SPEAKER_TOOL_NAME),
            "participant must not see nominate_speaker, got: {:?}",
            p_names
        );
        assert!(
            !p_names.contains(&END_DISCUSSION_TOOL_NAME),
            "participant must not see end_discussion, got: {:?}",
            p_names
        );
        // Non-arbitration tools are preserved.
        assert!(
            p_names.contains(&"read_file"),
            "participant should still see read_file, got: {:?}",
            p_names
        );
    }

    /// Regression (08-04 follow-up, user decision "例子移除@，在和别人交流时允许@别人"):
    /// the participant identity-guard block must (a) NOT showcase an `@`-prefix
    /// example (naming `@moderator:` self-primed the models into writing
    /// `@moderator:` / `@M3:` self-labels — DB sessions 2bbc0d55 / 7bb0c351 show
    /// `@M3:  @M3:  @D4F`-style noise accumulating), (b) forbid starting a reply
    /// with your OWN name/role, and (c) explicitly ALLOW @-mentioning another
    /// participant in the body.
    #[test]
    fn participant_prompt_forbids_self_label_but_allows_mentions() {
        let p = participant_system_prompt("M3", None);
        // (a) no `@`-prefixed example in the guard block (it self-primes).
        assert!(
            !p.contains("@moderator:"),
            "guard block must not showcase an @-prefix example: {p:?}"
        );
        // (b) never start with your OWN name/role. (Line-continuation
        // `\n\` breaks "with your OWN" across a newline in the string —
        // match the two fragments separately.)
        assert!(
            p.contains("never start your reply with") && p.contains("your OWN name or role"),
            "must forbid self-label prefixes: {p:?}"
        );
        // (c) @-mentioning another participant in the body is allowed.
        assert!(
            p.contains("without an @ (e.g. \"@D4F，你说得对…\"") || p.contains("@D4F，你说得对"),
            "must explicitly allow @-mentioning others in the body: {p:?}"
        );
    }

    /// The moderator prompt must carry the same "no self-label prefix" rule.
    #[test]
    fn moderator_prompt_forbids_self_label() {
        let ctx = GroupChatCtx {
            participants: vec![crate::agent::group_chat::ParticipantConfig {
                name: "M3".to_string(),
                model: "m3".to_string(),
                persona_md: None,
                order: None,
            }],
            moderator_model_id: "mod".to_string(),
        };
        let p = moderator_system_prompt(&ctx);
        assert!(
            p.contains("never start your reply with") && p.contains("your OWN name or role"),
            "moderator prompt must forbid self-label prefixes: {p:?}"
        );
        assert!(
            !p.contains("@moderator:"),
            "moderator prompt must not showcase an @-prefix example: {p:?}"
        );
    }

    /// 08-06 fix: the no-nominate nudge must (a) name BOTH arbitration
    /// tools, (b) forbid outputting only text, and (c) carry the streak
    /// count. The base moderator prompt (streak 0) must NOT carry the
    /// nudge.
    #[test]
    fn moderator_nudge_forces_tool_call() {
        let n = moderator_nudge(2);
        assert!(
            n.contains("nominate_speaker") && n.contains("end_discussion"),
            "nudge must name both tools: {n:?}"
        );
        assert!(
            n.contains("Do NOT output only text"),
            "nudge must forbid text-only: {n:?}"
        );
        assert!(
            n.contains("previous 2 turn(s)"),
            "nudge must carry streak count: {n:?}"
        );
    }

    /// 08-06 fix: MAX_NO_NOMINATE_STREAK sanity — must be ≥ 1 (retry at
    /// least once) and small enough to bound cost.
    #[test]
    fn max_no_nominate_streak_is_sane() {
        assert!(MAX_NO_NOMINATE_STREAK >= 1, "must retry at least once");
        assert!(MAX_NO_NOMINATE_STREAK <= 5, "too many retries is wasteful");
    }

    // -------------------------------------------------------------------
    // participant_view unit tests (design.md §4 + llm-contract.md §469)
    // -------------------------------------------------------------------

    fn user_text(text: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: MessageContent::Text(text.to_string()),
            speaker: None,
        }
    }

    fn assistant_blocks(blocks: Vec<ContentBlock>) -> ChatMessage {
        ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Blocks(blocks),
            speaker: Some("moderator".to_string()),
        }
    }

    fn tool_use(id: &str, name: &str) -> ContentBlock {
        ContentBlock::ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input: serde_json::json!({}),
        }
    }

    fn tool_result(id: &str) -> ContentBlock {
        ContentBlock::ToolResult {
            tool_use_id: id.to_string(),
            content: "Floor handed to M1.".to_string(),
            is_error: false,
        }
    }

    /// design.md §4 View-2: an arbitration pair (assistant tool_use row
    /// + its following user tool_result row) is stripped atomically,
    /// the assistant row's non-tool blocks are preserved, and no orphan
    /// tool_use / tool_result survives (llm-contract.md §469).
    #[test]
    fn participant_view_strips_arbitration_pair_keeps_text() {
        let full = vec![
            user_text("hello"),
            assistant_blocks(vec![
                ContentBlock::Thinking {
                    thinking: "think".to_string(),
                    signature: "sig".to_string(),
                },
                ContentBlock::Text {
                    text: "主持人发言".to_string(),
                    cache_control: None,
                },
                tool_use("c1", "nominate_speaker"),
            ]),
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![tool_result("c1")]),
                speaker: None,
            },
            assistant_blocks(vec![ContentBlock::Text {
                text: "M1 发言".to_string(),
                cache_control: None,
            }]),
        ];

        let view = participant_view(&full);
        assert_eq!(
            view.len(),
            3,
            "arbitration pair dropped; user + moderator text + M1 text kept"
        );
        assert_eq!(view[0].content.to_text(), "hello");
        let mod_text = view[1].content.to_text();
        assert!(
            mod_text.contains("主持人发言"),
            "moderator text kept: {mod_text:?}"
        );
        assert!(
            !has_arbitration_blocks(&view),
            "no arbitration tool_use / tool_result may survive in the participant view"
        );
        // Pair atomicity: no ToolUse without its ToolResult and vice versa.
        assert!(no_orphan_pairs(&view));
    }

    /// No arbitration interaction present → the view is unchanged
    /// (all rows pass through verbatim).
    #[test]
    fn participant_view_no_arbitration_pair_passes_through() {
        let full = vec![
            user_text("hello"),
            assistant_blocks(vec![ContentBlock::Text {
                text: "主持人发言".to_string(),
                cache_control: None,
            }]),
            assistant_blocks(vec![ContentBlock::Text {
                text: "M1 发言".to_string(),
                cache_control: None,
            }]),
        ];
        let view = participant_view(&full);
        assert_eq!(view.len(), full.len(), "no arbitration → unchanged");
        for (v, f) in view.iter().zip(full.iter()) {
            assert_eq!(v.content.to_text(), f.content.to_text());
            assert_eq!(v.role, f.role);
        }
    }

    /// A NON-arbitration tool pair (e.g. the moderator's `read_file`)
    /// must pass through the filter INTACT — `participant_view` only
    /// strips arbitration (`nominate_speaker` / `end_discussion`)
    /// pairs. Guards against the filter accidentally broadening to
    /// "drop all tool interactions" (which would leave the
    /// participant blind to the moderator's non-arbitration work).
    #[test]
    fn participant_view_non_arbitration_tool_pair_passes_through() {
        let full = vec![
            user_text("hello"),
            assistant_blocks(vec![
                ContentBlock::Text {
                    text: "先看下文件".to_string(),
                    cache_control: None,
                },
                tool_use("r1", "read_file"),
            ]),
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![tool_result("r1")]),
                speaker: None,
            },
            assistant_blocks(vec![ContentBlock::Text {
                text: "M1 发言".to_string(),
                cache_control: None,
            }]),
        ];
        let view = participant_view(&full);
        assert_eq!(
            view.len(),
            4,
            "non-arbitration pair passes through unchanged"
        );
        // No arbitration ToolUse may survive (the loose `has_arbitration_blocks`
        // helper treats ANY ToolResult as arbitration, so it is not usable
        // here — a non-arbitration result legitimately survives).
        assert!(
            !view.iter().any(|m| {
                matches!(
                    &m.content,
                    MessageContent::Blocks(blocks)
                        if blocks.iter().any(|b| matches!(
                            b,
                            ContentBlock::ToolUse { name, .. }
                                if name == NOMINATE_SPEAKER_TOOL_NAME
                                    || name == END_DISCUSSION_TOOL_NAME
                        ))
                )
            }),
            "no arbitration tool_use may survive in the participant view"
        );
        // The read_file pair must still be present (pair atomicity).
        assert!(no_orphan_pairs(&view));
        let tool_ids: Vec<String> = view
            .iter()
            .filter_map(|m| match &m.content {
                MessageContent::Blocks(blocks) => Some(blocks.clone()),
                _ => None,
            })
            .flatten()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, .. } => Some(id),
                ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id),
                _ => None,
            })
            .collect();
        assert_eq!(tool_ids, vec!["r1".to_string(), "r1".to_string()]);
    }

    /// A pure tool row (only arbitration ToolUse blocks, no think/text)
    /// → the whole row is dropped AND its following tool_result row is
    /// skipped (no orphan tool_result).
    #[test]
    fn participant_view_pure_tool_row_dropped_with_result_row() {
        let full = vec![
            assistant_blocks(vec![tool_use("c1", "nominate_speaker")]),
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![tool_result("c1")]),
                speaker: None,
            },
            assistant_blocks(vec![ContentBlock::Text {
                text: "M1 发言".to_string(),
                cache_control: None,
            }]),
        ];
        let view = participant_view(&full);
        assert_eq!(view.len(), 1, "pure tool row + its result row both dropped");
        assert_eq!(view[0].content.to_text(), "M1 发言");
        assert!(no_orphan_pairs(&view));
    }

    /// Two consecutive moderator arbitration rounds: both pairs are
    /// stripped, later pairs do not disturb earlier text, and the tail
    /// stays clean (no orphan at the boundary).
    #[test]
    fn participant_view_two_consecutive_moderator_rounds() {
        let full = vec![
            user_text("hello"),
            assistant_blocks(vec![
                ContentBlock::Text {
                    text: "主持人:先请 M1".to_string(),
                    cache_control: None,
                },
                tool_use("c1", "nominate_speaker"),
            ]),
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![tool_result("c1")]),
                speaker: None,
            },
            assistant_blocks(vec![ContentBlock::Text {
                text: "M1 发言".to_string(),
                cache_control: None,
            }]),
            assistant_blocks(vec![
                ContentBlock::Text {
                    text: "主持人:再请 M2".to_string(),
                    cache_control: None,
                },
                tool_use("c2", "nominate_speaker"),
            ]),
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![tool_result("c2")]),
                speaker: None,
            },
            assistant_blocks(vec![ContentBlock::Text {
                text: "M2 发言".to_string(),
                cache_control: None,
            }]),
        ];
        let view = participant_view(&full);
        let text = view
            .iter()
            .map(|m| m.content.to_text())
            .collect::<Vec<_>>()
            .join("|");
        assert_eq!(
            text, "hello|主持人:先请 M1|M1 发言|主持人:再请 M2|M2 发言",
            "both arbitration pairs stripped, all text preserved in order: {text:?}"
        );
        assert!(no_orphan_pairs(&view));
    }

    fn has_arbitration_blocks(msgs: &[ChatMessage]) -> bool {
        msgs.iter().any(|m| {
            matches!(
                &m.content,
                MessageContent::Blocks(blocks)
                    if blocks.iter().any(|b| matches!(
                        b,
                        ContentBlock::ToolUse { name, .. }
                            if name == NOMINATE_SPEAKER_TOOL_NAME || name == END_DISCUSSION_TOOL_NAME
                    ) || matches!(b, ContentBlock::ToolResult { .. }))
            )
        })
    }

    /// llm-contract.md §469 Pair Atomicity check: every ToolUse has a
    /// matching ToolResult and vice versa (within the message list).
    fn no_orphan_pairs(msgs: &[ChatMessage]) -> bool {
        let mut use_ids = Vec::new();
        let mut result_ids = Vec::new();
        for m in msgs {
            if let MessageContent::Blocks(blocks) = &m.content {
                for b in blocks {
                    match b {
                        ContentBlock::ToolUse { id, .. } => use_ids.push(id.clone()),
                        ContentBlock::ToolResult { tool_use_id, .. } => {
                            result_ids.push(tool_use_id.clone())
                        }
                        _ => {}
                    }
                }
            }
        }
        use_ids.iter().all(|id| result_ids.contains(id))
            && result_ids.iter().all(|id| use_ids.contains(id))
    }
}
