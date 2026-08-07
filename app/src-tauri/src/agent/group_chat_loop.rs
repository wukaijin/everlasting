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
//! - **later rounds** reload once into `full`, then EVERY speaker
//!   (moderator and participants alike) gets a per-role isolated
//!   history via [`role_history`] (08-07-group-chat-role-history-
//!   isolation, replaces the 08-04 `participant_view`) — each role
//!   sees only its own assistant rows verbatim + other speakers'
//!   remarks rewritten as `role:user`, other speakers' thinking/tool
//!   pairs dropped (kills the multi-identity-assistant 串台 root cause
//!   and sidesteps the Anthropic signature round-trip constraint).
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
//!   5. reload messages from DB → role_history(full, X.name)
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

// R2 (08-07-group-chat-review-fixes): `stop_reason` values the
// orchestrator emits on `Done` to surface its boundary behavior to the
// user (D2: reuse `Done.stop_reason`, no new ChatEvent variant). The
// frontend reads these in its `done` handler to (a) decide finalize for
// the terminal ones and (b) render a transient notice line. These are
// FREE-FORM STRINGS, not a typed enum — matching the existing convention
// (`end_turn` / `cancelled` / `group_chat_end` / `loop_terminated` /
// `max_turns` / `tool_use` are all bare string literals).
//
// Terminal (loop exited → this is the last Done the orchestrator emits):
//   - `max_rounds`: the outer loop hit MAX_ORCHESTRATION_ROUNDS. This is
//                   the ONLY non-normal terminal reason left.
// Non-terminal (emitted mid-loop, the discussion continues — the frontend
// must NOT finalize on these, it only shows a one-line notice):
//   - `nominee_unknown`       : the nominated name is not in the roster;
//                               the moderator gets another round.
//   - `participant_unresolved`: the participant's model could not be
//                               resolved from the catalog; turn skipped.
//
// History note: 08-07-group-chat-review-fixes originally added
// `moderator_stuck` for the "moderator fails to nominate after N retries"
// case. 08-07-group-chat-toolset-and-identity R2 removed that streak
// mechanism entirely (it couldn't distinguish "moderator is researching"
// from "moderator is stuck" — DB session 8be4687f seq 1/3/5 showed
// legitimate research being a valid no-nominate run). Now the moderator
// simply gets another round on no-nominate, and `MAX_ORCHESTRATION_ROUNDS`
// is the only bound. So `moderator_stuck` / `STOP_REASON_MODERATOR_STUCK`
// / the streak counter / `moderator_nudge` are all gone.
pub const STOP_REASON_GROUP_CHAT_END: &str = "group_chat_end";
pub const STOP_REASON_MAX_ROUNDS: &str = "max_rounds";
pub const STOP_REASON_NOMINEE_UNKNOWN: &str = "nominee_unknown";
pub const STOP_REASON_PARTICIPANT_UNRESOLVED: &str = "participant_unresolved";

/// Why the outer orchestration loop stopped (R2). Carried out of the
/// `for` loop so the post-loop terminal `Done` can name the cause. The
/// happy path (`end_discussion`) maps to the existing `group_chat_end`;
/// the failure path is `MaxRounds`. A cancel is NOT a `HaltReason` — the
/// cancelled inner turn emits `Done { cancelled }` itself and the
/// post-loop emit is suppressed (unchanged).
enum HaltReason {
    /// Moderator called `end_discussion` (normal end).
    DiscussionEnded,
    /// Outer loop hit `MAX_ORCHESTRATION_ROUNDS`.
    MaxRounds,
}

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
         the discussion focused; end it when it has run its course.\n\
         \n\
         ## Pacing and boundaries (read carefully)\n\
         - You MAY research the codebase (read_file / grep / glob / list_dir / \
         web_fetch) to ground the discussion — a brief investigation is good. \
         But research is a MEANS, not the goal: after a short look, hand the \
         floor to a participant with `nominate_speaker`. Do not stall the \
         discussion by investigating for many rounds without nominating.\n\
         - `nominate_speaker` is the ONLY scheduling mechanism. Do NOT use other \
         tools (checklists, notes, skills, etc.) to build your own \
         speaker-rotation or progress-tracking flow — call `nominate_speaker` \
         to pick the next speaker, or `end_discussion` to close.",
        roster.join("\n")
    )
}

/// Build a role's isolated LLM history from the shared DB transcript
/// (08-07-group-chat-role-history-isolation, design §R1 — replaces the
/// 08-04 `participant_view`).
///
/// Each role sees ONLY its own assistant messages (verbatim, incl.
/// thinking + signature — Anthropic round-trip safe) plus other
/// speakers' utterances rewritten as `role:user`. Other speakers'
/// Thinking blocks are dropped (they carry signatures bound to *their*
/// generation context; re-injecting them into *this* role's context
/// would either break the signature round-trip or be echoed as if this
/// role produced them). Other speakers' tool_use/tool_result pairs are
/// dropped entirely (tool results are NOT shared — relayed only via
/// their text remarks). The moderator's arbitration pairs
/// (nominate/end) are likewise dropped for non-moderator roles — from
/// a participant's point of view they are just another "other speaker's
/// tool pair" (the 08-04 identity-confusion invariant carries over:
/// showing them made a participant conclude it WAS the moderator and
/// reply as `@moderator` — DB evidence in research/db-evidence.md §2).
///
/// 归属策略 (评审 P0-2 修订): 改写行 **保留 speaker 字段、content 不带
/// `@` 前缀**。归属交给 wire 层统一负责——Anthropic `apply_speaker_prefix`
/// 自动插 `@name: `、OpenAI 自动填 `name` 字段。若 content 自带 `@` 前缀
/// 会造成双重前缀(Anthropic `@moderator: @moderator: …`)。speaker 字段
/// 同时是 D-D 入口守卫(P0-1,见 chat_loop.rs)的区分信号。
///
/// # Invariants (locked)
///
/// 1. **Current-role assistant rows stay verbatim** (`role:assistant` +
///    all blocks, incl. thinking + signature) → Anthropic round-trip
///    contract (AC1/AC5).
/// 2. **Other-speaker assistant rows never appear as `assistant`**
///    (认知根因——多身份 assistant 共存——消除);their thinking is dropped
///    (no signature round-trip duty) and their tool pairs are stripped
///    whole (工具结果不共享, R3).
/// 3. **Human prompts** (`speaker == None`) stay `role:user`.
///
/// The one-pass strip's adjacency assumption holds because persisted
/// rows land in adjacent `seq` slots within one speaker turn: an
/// assistant row carrying `ToolUse` is immediately followed by the
/// user row carrying its `tool_result`.
fn role_history(full: &[ChatMessage], current_role: &str) -> Vec<ChatMessage> {
    let mut out: Vec<ChatMessage> = Vec::with_capacity(full.len());
    let mut pending_other_tool_use_ids: Vec<String> = Vec::new();
    for m in full {
        // (1) A row carrying a pending other-speaker tool_result belongs
        // to that stripped tool pair → skip the whole row (one-row
        // lookahead; persisted per-turn rows are a single tool_result
        // for the pending calls, so in practice the whole row belongs
        // to the pair — a mixed row would be dropped wholesale, which
        // is safe: it can only lose blocks already stripped on the
        // assistant side, never creating an orphan).
        if !pending_other_tool_use_ids.is_empty() {
            if row_carries_any_tool_result(m, &pending_other_tool_use_ids) {
                pending_other_tool_use_ids.clear();
                continue;
            }
            pending_other_tool_use_ids.clear();
        }
        // (2) Rewrite by role, then by speaker (评审 P2-1: no duplicated
        // `(Role::User, None)` branch).
        match m.role {
            Role::User => out.push(m.clone()), // human prompt / current role's tool_result
            Role::Assistant => match &m.speaker {
                // 自己:原样保留(role:assistant + 全部 blocks,含 Thinking+signature)。
                Some(sp) if sp == current_role => out.push(m.clone()),
                // 他人:抽 text 改写为 user(不带 `@` 前缀、保留 speaker —— 归属
                // 交给 wire 层);Thinking 丢;ToolUse id 收集待跳过其 tool_result。
                Some(sp) => {
                    // `to_text()` reuses MessageContent's visible-text
                    // extractor (skips thinking by design — same rule as
                    // the DB `text` column).
                    let text = m.content.to_text();
                    if !text.is_empty() {
                        out.push(ChatMessage {
                            role: Role::User,
                            content: MessageContent::Text(text),
                            speaker: Some(sp.clone()),
                        });
                    }
                    pending_other_tool_use_ids.extend(extract_tool_use_ids(&m.content));
                }
                // 群聊 assistant 行必有 speaker(moderator 或 participant.name)。
                // None 说明数据异常(评审 P2-2):debug_assert 报警 + 原样保留交
                // 后续诊断,不静默改写成 `@?`。
                None => {
                    debug_assert!(
                        m.speaker.is_some(),
                        "group-chat assistant row missing speaker: {:?}",
                        m
                    );
                    out.push(m.clone());
                }
            },
        }
    }
    out
}

/// Collect every `ToolUse` id in a message's content (the ids whose
/// tool_result rows must be skipped when stripping another speaker's
/// tool pair).
fn extract_tool_use_ids(c: &MessageContent) -> Vec<String> {
    match c {
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect(),
        MessageContent::Text(_) => Vec::new(),
    }
}

/// Whether the message carries a `tool_result` for any of the given
/// tool_use ids (i.e. it is the other half of a stripped tool pair).
fn row_carries_any_tool_result(m: &ChatMessage, ids: &[String]) -> bool {
    matches!(
        &m.content,
        MessageContent::Blocks(blocks)
            if blocks.iter().any(|b| matches!(
                b,
                ContentBlock::ToolResult { tool_use_id, .. } if ids.contains(tool_use_id)
            ))
    )
}

/// Group-chat research tool whitelist (08-07-group-chat-toolset-and-identity R1).
/// Both the moderator and participants get exactly these read-only / evidence-
/// gathering tools — a group-chat discussion reads the codebase to ground the
/// conversation, but does NOT modify it and does NOT run side-effecting tools.
///
/// This replaced the pre-R1 `participant_tool_defs` BLACKLIST ("strip the two
/// arbitration tools"). The blacklist leaked `use_skill` / `update_checklist` /
/// `shell` / `write_file` / ... into group chats, which DB session `8be4687f`
/// showed weak models abusing: M3 called `use_skill("group-chat-director")` (a
/// hallucinated skill — no `<available-skills>` block is injected under
/// `system_prompt_override`, so the model fabricated one) and `update_checklist`
/// to build its own speaker-rotation flow, hijacking the moderator's job. The
/// whitelist is exhaustive: a newly added `builtin_tools` entry does NOT enter
/// group chat unless explicitly added here, so this class of leak can't recur.
const GROUP_CHAT_RESEARCH_TOOLS: &[&str] = &["read_file", "grep", "glob", "list_dir", "web_fetch"];

/// Tools the moderator gets ON TOP of the research whitelist: the two
/// arbitration tools that drive the turn-taking loop. Participants never get
/// these — `participant_tool_defs` used to strip them; the whitelist now
/// achieves the same by simply not listing them for participants.
const MODERATOR_EXTRA_TOOLS: &[&str] = &[NOMINATE_SPEAKER_TOOL_NAME, END_DISCUSSION_TOOL_NAME];

/// Build the tool list a group-chat speaker sees, by whitelist.
/// `is_moderator = true` → research tools + arbitration tools.
/// `is_moderator = false` → research tools only (no arbitration, no
/// write/execute/interaction/skill/checklist tools).
fn group_chat_tool_defs(tool_defs: &[ToolDef], is_moderator: bool) -> Vec<ToolDef> {
    let allow: Vec<&str> = if is_moderator {
        GROUP_CHAT_RESEARCH_TOOLS
            .iter()
            .chain(MODERATOR_EXTRA_TOOLS.iter())
            .copied()
            .collect()
    } else {
        GROUP_CHAT_RESEARCH_TOOLS.to_vec()
    };
    tool_defs
        .iter()
        .filter(|t| allow.contains(&t.name.as_str()))
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
         - Just say your own piece on the topic and respond to what others said.\n\
         \n\
         ## Research is allowed (08-07-group-chat-role-history-isolation follow-up)\n\
         You MAY research the codebase to ground your remarks — read_file / grep / glob / list_dir / web_fetch\n\
         are available to you, and a brief look at the\n\
         code before you speak is good (the moderator will verify / build on it).\n\
         But research is a MEANS, not the goal: after a short look, say your piece\n\
         and end your turn. Do not stall the discussion by investigating for many\n\
         turns without speaking.\n\
         \n\
         ## Do NOT take over the moderator's job (read carefully)\n\
         Even if the discussion seems stalled, or the moderator takes several turns\n\
         before nominating, you must NOT step in as the host. Specifically:\n\
         - Do NOT build your own speaker-rotation or progress checklist (no\n\
           \"next let's hear from X\", no tracking who has spoken).\n\
         - Do NOT address the room as the moderator (no opening, closing, or\n\
           summarizing the whole discussion on everyone's behalf).\n\
         - Do NOT invent or invoke system tools / skills to legitimize hosting —\n\
           the arbitration tools (nominate_speaker / end_discussion) are\n\
           moderator-only and you never call them; the RESEARCH tools above are\n\
           yours to use, but only to ground your own remarks, never to \"run\" the\n\
           discussion. If you find yourself wanting to \"run\" the discussion, stop:\n\
           you are a participant. Speak only your own view, then end your turn. The\n\
           moderator will pick the next speaker.",
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
///   appended, so a reload is the only resync). The moderator enters
///   with [`role_history`]`(&full, "moderator")` (08-07-group-chat-
///   role-history-isolation — its own assistant history verbatim,
///   incl. its arbitration history for 跨轮连贯).
/// - **participant turn**: reload again into `full` (fresh — the
///   moderator's just-persisted rows must be visible) then enter with
///   [`role_history`]`(&full, &participant.name)`: own assistant rows
///   verbatim, other speakers' remarks as `role:user`, other speakers'
///   thinking / tool pairs (incl. the moderator's arbitration pairs)
///   dropped.
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

    // R2 (08-07-group-chat-toolset-and-identity): the reason the loop
    // stopped, if it exited via a `break`. Stays `None` on cancel (the
    // post-loop emit is suppressed) and is set to `DiscussionEnded` by
    // the `end_discussion` path, and `MaxRounds` if the loop ran to
    // completion. The pre-R2 `ModeratorStuck` variant + the
    // `no_nominate_streak` counter are GONE — a no-nominate round is no
    // longer treated as "stuck" (DB session 8be4687f showed legitimate
    // research runs that legitimately don't nominate); the moderator
    // simply gets another round, bounded only by MAX_ORCHESTRATION_ROUNDS.
    let mut halt_reason: Option<HaltReason> = None;

    for round in 0..MAX_ORCHESTRATION_ROUNDS {
        if token.is_cancelled() {
            break;
        }

        // --- 1. Moderator turn ------------------------------------------------
        // The moderator gets the nominate/end tools (already in
        // builtin_tools) + the shared turn state. Its system prompt
        // is fully replaced (system_prompt_override = Some).
        //
        // R1 (08-07-group-chat-role-history-isolation): the moderator
        // gets its OWN isolated history via `role_history` — its own
        // assistant rows verbatim (incl. its arbitration history from
        // prior rounds — 跨轮连贯) + participants' remarks rewritten as
        // user. The pre-R1 `full` verbatim view mixed EVERY speaker's
        // assistant rows + thinking into one context (串台源, same as
        // participant_view). round 0 uses the caller-supplied `messages`
        // (tail = the new human message); later rounds reload once.
        let full = if round == 0 {
            messages.clone()
        } else {
            reload_messages(&db, &session_id).await
        };
        let history = role_history(&full, "moderator");
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
            // R2: the moderator prompt is now STABLE across rounds (no
            // per-streak nudge — the streak mechanism is gone). Pacing
            // guidance ("research then nominate") lives in the prompt
            // itself (R3), not in a dynamically-appended nudge.
            let prompt = moderator_prompt.clone();
            run_chat_loop(
                // R1 (08-07-group-chat-toolset-and-identity): moderator gets
                // the research whitelist + arbitration tools (not the full
                // builtin_tools set — that leaked update_checklist/use_skill/
                // shell into group chat, which weak models abused).
                group_chat_tool_defs(&tool_defs, true),
                provider.clone(),
                context_window,
                rid.clone(),
                session_id.clone(),
                history,
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
            halt_reason = Some(HaltReason::DiscussionEnded);
            break;
        }

        let nominee_name = match next_speaker {
            Some(n) => n,
            None => {
                // R2 (08-07-group-chat-toolset-and-identity): the moderator
                // ended its turn WITHOUT calling nominate_speaker /
                // end_discussion. The OLD behavior (pre-08-06) was a
                // round-robin fallback that dispatched the WRONG participant
                // → role collapse (sessions a6c87247 / 4a9d3566 / 093823f3).
                // 08-06 fixed that by retrying the moderator turn, and 08-07
                // bounded the retries with a streak counter (ModeratorStuck).
                //
                // This task (R2) drops the streak counter: DB session
                // 8be4687f showed the moderator legitimately running several
                // research rounds (seq 1/3/5) without nominating, and the
                // streak=3 bound mis-killed that as "stuck". Now we simply
                // retry the moderator turn with NO bound other than
                // MAX_ORCHESTRATION_ROUNDS — the moderator sees its own prior
                // text in the reloaded history and the pacing guidance in its
                // system prompt (R3) nudges it toward nominating. If it truly
                // never nominates, the outer round cap stops the discussion
                // (HaltReason::MaxRounds → stop_reason "max_rounds").
                tracing::warn!(
                    round,
                    "group_chat: moderator did not nominate; retrying moderator turn (no participant dispatched)"
                );
                continue;
            }
        };

        // --- 4. Resolve nominee → provider + prompt ---------------------------
        let participant = match gc_ctx.participant_by_name(&nominee_name) {
            Some(p) => p.clone(),
            None => {
                // R2: surface the skip to the user. This is NON-terminal —
                // the discussion continues (the moderator gets another
                // round to nominate a valid name). Emit a `Done` whose
                // `stop_reason` is NOT `group_chat_end`, so the frontend's
                // finalize gate (which keys on `group_chat_end` /
                // `cancelled` / the new terminal reasons) does NOT
                // finalize; it only renders a one-line notice. The
                // orchestrator's own post-loop terminal `Done` is still
                // what finalizes the request.
                tracing::warn!(round, nominee=%nominee_name, "group_chat: nominee not in roster; skipping turn");
                emit_chat_event_via_sink(
                    &sink,
                    &rid,
                    &ChatEvent::Done {
                        stop_reason: Some(STOP_REASON_NOMINEE_UNKNOWN.to_string()),
                        usage: None,
                    },
                );
                continue;
            }
        };
        let participant_provider = resolve_provider(&worker_catalog, &participant.model, &db).await;
        let participant_prompt =
            participant_system_prompt(&participant.name, participant.persona_md.as_deref());

        let Some(provider) = participant_provider else {
            // R2: same non-terminal notice pattern as nominee_unknown —
            // the participant's model couldn't be resolved (catalog miss),
            // so the turn is skipped but the discussion continues.
            tracing::warn!(round, model=%participant.model, "group_chat: participant provider unresolved; skipping turn");
            emit_chat_event_via_sink(
                &sink,
                &rid,
                &ChatEvent::Done {
                    stop_reason: Some(STOP_REASON_PARTICIPANT_UNRESOLVED.to_string()),
                    usage: None,
                },
            );
            continue;
        };

        // --- 5. Reload + assemble the shared transcript ------------------------
        // Reload once (D-B) to pick up the moderator's just-persisted
        // rows (its tool_result + text), then build the participant's
        // per-role history (08-07-group-chat-role-history-isolation
        // R1, replaces the 08-04 View-2 `participant_view`): only the
        // participant's own assistant rows stay verbatim, the
        // moderator's arbitration tool_use↔tool_result pairs are
        // stripped atomically, and every other speaker's remarks arrive
        // as `role:user` (identity-confusion root cause, research/
        // db-evidence.md §2).
        let full = reload_messages(&db, &session_id).await;
        let history = role_history(&full, &participant.name);

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
        // Multi-turn (max_turns=20, 08-07-group-chat-review-fixes R3):
        // a participant may gather evidence from the codebase
        // (read_file / grep / glob / list_dir / web_fetch) across
        // several turns before delivering its substantive remark. The
        // pre-R3 `Some(1)` gave participants the tool schema but no
        // follow-up turn to act on a tool_result, so "read then speak"
        // was impossible. 20 is generous enough for a focused
        // investigation while still bounding cost (the outer
        // `MAX_ORCHESTRATION_ROUNDS` is the hard cap; 20 is the
        // per-participant inner cap).
        //
        // The moderator stays at `max_turns = Some(1)` (unchanged) —
        // see the moderator call site above for the rationale (nominate
        // must end the turn immediately, and single-turn suppresses
        // the identity-confusing first-person arbitration filler that
        // 08-04 follow-up removed).
        //
        // The participant's system prompt is its persona (fully
        // replaces the parent prompt).
        run_chat_loop(
            // R1 (08-07-group-chat-toolset-and-identity): participant gets the
            // research whitelist ONLY (read_file/grep/glob/list_dir/web_fetch).
            // No arbitration tools (only the moderator arbitrates) AND no
            // write/execute/skill/checklist tools — DB session 8be4687f seq 9
            // showed a participant abusing use_skill (hallucinated
            // "group-chat-director") + update_checklist (self-built speaker
            // rotation) to hijack the moderator. The whitelist supersedes the
            // old `participant_tool_defs` blacklist, which only stripped the
            // two arbitration tools and leaked everything else.
            group_chat_tool_defs(&tool_defs, false),
            provider,
            context_window,
            rid.clone(),
            session_id.clone(),
            // Per-role history: the participant's own assistant rows
            // verbatim + other speakers' remarks as user — the tail user
            // row is an already-persisted message (the entry guard skips
            // re-persisting it) and the moderator's tool interaction is
            // invisible to the participant.
            history,
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
            Some(20), // participant multi-turn — may read the codebase before speaking
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

    // R2: if the loop ran to completion (no `break` set a HaltReason),
    // the cause is max-rounds exhaustion. Set it here so the post-loop
    // terminal `Done` carries the right `stop_reason`.
    if !token.is_cancelled() && halt_reason.is_none() {
        tracing::warn!(
            rounds = MAX_ORCHESTRATION_ROUNDS,
            "group_chat: hit max rounds; stopping"
        );
        halt_reason = Some(HaltReason::MaxRounds);
    }

    // Terminal signal for the frontend (08-04 follow-up, user-approved
    // "终止事件 + 逐轮流式"): the orchestrator shares ONE `rid` across
    // every inner `run_chat_loop` (moderator + participants), and each
    // inner turn emits its own `Done` (moderator `max_turns` /
    // participant `end_turn`). The frontend cannot know the discussion
    // has actually ENDED from those inner Dones — it would finalize
    // the request after the FIRST speaker turn and silently drop every
    // later event (the "群聊内容不实时出现" bug). Emit a dedicated
    // terminal `Done` so the frontend keeps the request alive across
    // inner turns and only finalizes on this signal. Not emitted when
    // cancelled (the cancelled inner turn already emitted
    // `Done { cancelled }`, which the frontend treats as terminal).
    //
    // R2 (08-07): the `stop_reason` now reflects WHY the loop ended, so
    // the frontend can finalize the terminal cases AND show the user a
    // notice when the discussion ended abnormally (moderator stuck /
    // max rounds). `DiscussionEnded` keeps the original `group_chat_end`
    // value (backward-compat for any frontend path keyed on it); the two
    // failure paths get their own values and are added to the frontend's
    // finalize whitelist alongside `group_chat_end` / `cancelled`.
    if !token.is_cancelled() {
        let stop_reason = match halt_reason {
            Some(HaltReason::DiscussionEnded) => STOP_REASON_GROUP_CHAT_END,
            Some(HaltReason::MaxRounds) => STOP_REASON_MAX_ROUNDS,
            // Unreachable: !cancelled → halt_reason was set by a break or
            // the MaxRounds default above. Defensive fallback keeps the
            // emit shape stable if a future path forgets to set it.
            None => STOP_REASON_GROUP_CHAT_END,
        };
        emit_chat_event_via_sink(
            &sink,
            &rid,
            &ChatEvent::Done {
                stop_reason: Some(stop_reason.to_string()),
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

    /// R1 (08-07-group-chat-toolset-and-identity): the moderator's tool set
    /// is the research whitelist PLUS the two arbitration tools — and NOTHING
    /// else. This replaces the pre-R1 blacklist; DB session 8be4687f showed
    /// the full `builtin_tools()` set leaking `update_checklist` / `use_skill`
    /// into group chat, which weak models abused to self-build a speaker
    /// rotation and hallucinate a skill. The whitelist is exhaustive.
    #[test]
    fn group_chat_tool_defs_moderator_has_research_plus_arbitration() {
        let full = crate::tools::builtin_tools();
        // Sanity: the research + arbitration tools actually exist in
        // builtin_tools (otherwise the whitelist is a silent no-op and the
        // test would pass vacuously).
        let src_names: Vec<&str> = full.iter().map(|t| t.name.as_str()).collect();
        for needed in [
            "read_file",
            "grep",
            "glob",
            "list_dir",
            "web_fetch",
            NOMINATE_SPEAKER_TOOL_NAME,
            END_DISCUSSION_TOOL_NAME,
        ] {
            assert!(
                src_names.contains(&needed),
                "builtin_tools must include {needed}"
            );
        }

        let mod_tools = group_chat_tool_defs(&full, true);
        let m_names: Vec<&str> = mod_tools.iter().map(|t| t.name.as_str()).collect();

        // Has the research whitelist + arbitration.
        for required in [
            "read_file",
            "grep",
            "glob",
            "list_dir",
            "web_fetch",
            NOMINATE_SPEAKER_TOOL_NAME,
            END_DISCUSSION_TOOL_NAME,
        ] {
            assert!(
                m_names.contains(&required),
                "moderator must see {required}, got: {m_names:?}"
            );
        }
        // Does NOT contain the abuse-prone / irrelevant tools (the leak that
        // caused 8be4687f seq 9). Enumerate the full deny-set so a future
        // builtin_tools addition is caught here if someone wrongly whitelists it.
        for forbidden in [
            "use_skill",
            "update_checklist",
            "shell",
            "write_file",
            "edit_file",
            "run_background_shell",
            "shell_status",
            "shell_kill",
            "merge_worker",
            "discard_worker",
            "remember",
            "ask_user_question",
            "use_ui",
            "request_mode_change",
            "request_task_state_transition",
            "create_task",
        ] {
            assert!(
                !m_names.contains(&forbidden),
                "moderator must NOT see {forbidden}, got: {m_names:?}"
            );
        }
    }

    /// R1: the participant's tool set is the research whitelist ONLY — no
    /// arbitration tools (only the moderator arbitrates) AND none of the
    /// abuse-prone / write / execute / interaction tools. This is stricter
    /// than the old `participant_tool_defs` blacklist, which only stripped
    /// the two arbitration tools.
    #[test]
    fn group_chat_tool_defs_participant_has_research_only() {
        let full = crate::tools::builtin_tools();
        let p_tools = group_chat_tool_defs(&full, false);
        let p_names: Vec<&str> = p_tools.iter().map(|t| t.name.as_str()).collect();

        // Has exactly the research whitelist.
        for required in ["read_file", "grep", "glob", "list_dir", "web_fetch"] {
            assert!(
                p_names.contains(&required),
                "participant must see {required}, got: {p_names:?}"
            );
        }
        // No arbitration tools (the original participant_tool_defs guarantee,
        // now achieved via whitelist instead of blacklist).
        for arb in [NOMINATE_SPEAKER_TOOL_NAME, END_DISCUSSION_TOOL_NAME] {
            assert!(
                !p_names.contains(&arb),
                "participant must NOT see {arb}, got: {p_names:?}"
            );
        }
        // No abuse-prone / write / execute / interaction tools.
        for forbidden in [
            "use_skill",
            "update_checklist",
            "shell",
            "write_file",
            "edit_file",
            "run_background_shell",
            "shell_status",
            "shell_kill",
            "remember",
            "ask_user_question",
        ] {
            assert!(
                !p_names.contains(&forbidden),
                "participant must NOT see {forbidden}, got: {p_names:?}"
            );
        }
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

    // -------------------------------------------------------------------
    // R3 (08-07-group-chat-toolset-and-identity): prompt pacing/takeover
    // hardening. DB session 8be4687f showed the moderator researching for
    // many rounds without nominating (seq 1/3/5) and a participant (M3,
    // seq 9) hijacking the host role via update_checklist + a hallucinated
    // skill. R3 adds explicit pacing guidance to the moderator prompt and
    // an explicit anti-takeover block to the participant prompt.
    // -------------------------------------------------------------------

    /// R3: the moderator prompt must guide "research → nominate" pacing and
    /// forbid building a self-rolled speaker-rotation flow (the seq 1/3/5
    /// over-research + the seq-12-pre "want to use update_checklist" failure
    /// modes).
    #[test]
    fn moderator_prompt_guides_research_to_nominate() {
        let ctx = GroupChatCtx {
            participants: vec![crate::agent::group_chat::ParticipantConfig {
                name: "M3".to_string(),
                model: "m3".to_string(),
                persona_md: None,
            }],
            moderator_model_id: "mod".to_string(),
        };
        let p = moderator_system_prompt(&ctx);
        // Pacing: research is allowed but bounded — must hand the floor.
        assert!(
            p.contains("research is a MEANS") && p.contains("hand the floor"),
            "moderator prompt must frame research as means + require nominating: {p:?}"
        );
        // Boundary: nominate_speaker is the ONLY scheduling mechanism; no
        // self-built rotation flow.
        assert!(
            p.contains("ONLY scheduling mechanism") && p.contains("Do NOT use other"),
            "moderator prompt must forbid self-built speaker-rotation: {p:?}"
        );
    }

    /// R3: the participant prompt must explicitly forbid taking over the
    /// moderator's job — the three failure modes from seq 9 (self-built
    /// checklist, addressing the room as host, inventing system tools/skills
    /// to legitimize hosting).
    #[test]
    fn participant_prompt_forbids_takeover() {
        let p = participant_system_prompt("M3", None);
        assert!(
            p.contains("must NOT step in as the host"),
            "participant prompt must forbid taking over the host role: {p:?}"
        );
        // No self-built speaker rotation / checklist (seq 9's update_checklist).
        assert!(
            p.contains("speaker-rotation or progress checklist"),
            "participant prompt must forbid self-built rotation/checklist: {p:?}"
        );
        // No inventing system tools/skills to legitimize hosting (seq 9's
        // hallucinated use_skill("group-chat-director")).
        assert!(
            p.contains("Do NOT invent or invoke system tools"),
            "participant prompt must forbid inventing tools/skills to host: {p:?}"
        );
    }

    /// R1 follow-up (08-07-group-chat-role-history-isolation): the
    /// participant prompt must EXPLICITLY allow + encourage codebase
    /// research (read_file / grep / glob / list_dir / web_fetch are in
    /// the participant's tool whitelist, but the pre-follow-up prompt
    /// never mentioned them — combined with role_history stripping all
    /// other-speaker tool pairs, participants stopped using tools
    /// entirely; DB session 6c00f286: zero participant tool calls vs
    /// 4/5 earlier sessions with participant tool use).
    #[test]
    fn participant_prompt_encourages_research() {
        let p = participant_system_prompt("M3", None);
        assert!(
            p.contains("You MAY research the codebase"),
            "participant prompt must allow research: {p:?}"
        );
        assert!(
            p.contains("read_file / grep / glob / list_dir / web_fetch"),
            "participant prompt must name the research whitelist: {p:?}"
        );
        assert!(
            p.contains("research is a MEANS, not the goal"),
            "participant prompt must bound research (brief look then speak): {p:?}"
        );
        // The allowance must NOT leak the arbitration tools.
        assert!(
            !p.contains("nominate_speaker / end_discussion are available"),
            "arbitration tools must stay moderator-only: {p:?}"
        );
        assert!(
            p.contains("moderator-only"),
            "prompt must state arbitration tools are moderator-only: {p:?}"
        );
    }

    // -------------------------------------------------------------------
    // role_history unit tests (08-07-group-chat-role-history-isolation
    // design §R1 + llm-contract.md §469) — replaces the 08-04
    // participant_view suite. Each test locks one rewrite rule of the
    // per-role isolation assembler.
    // -------------------------------------------------------------------

    fn user_text(text: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: MessageContent::Text(text.to_string()),
            speaker: None,
        }
    }

    fn assistant_blocks_speaker(blocks: Vec<ContentBlock>, speaker: &str) -> ChatMessage {
        ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Blocks(blocks),
            speaker: Some(speaker.to_string()),
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

    /// R1: the current role's own assistant rows are preserved VERBATIM
    /// — `role:assistant` + ALL blocks, including Thinking + signature
    /// (Anthropic round-trip contract, AC1/AC5).
    #[test]
    fn role_history_current_role_assistant_verbatim() {
        let full = vec![
            user_text("hello"),
            assistant_blocks_speaker(
                vec![
                    ContentBlock::Thinking {
                        thinking: "my reasoning".to_string(),
                        signature: "sig-own".to_string(),
                    },
                    ContentBlock::Text {
                        text: "M1 发言".to_string(),
                        cache_control: None,
                    },
                ],
                "M1",
            ),
            assistant_blocks_speaker(
                vec![ContentBlock::Text {
                    text: "主持人发言".to_string(),
                    cache_control: None,
                }],
                "moderator",
            ),
        ];

        let history = role_history(&full, "M1");

        assert_eq!(history.len(), 3);
        assert_eq!(history[0], full[0], "human prompt unchanged");
        let own = &history[1];
        assert_eq!(own.role, Role::Assistant, "own row stays assistant");
        assert_eq!(own.speaker.as_deref(), Some("M1"));
        assert_eq!(own.content, full[1].content, "own blocks verbatim");
        assert!(
            matches!(
                &own.content,
                MessageContent::Blocks(blocks)
                    if blocks.iter().any(|b| matches!(
                        b,
                        ContentBlock::Thinking { signature, .. } if signature == "sig-own"
                    ))
            ),
            "own Thinking block + signature must survive: {:?}",
            own.content
        );
        // The other speaker (moderator) is rewritten to user.
        assert_eq!(history[2].role, Role::User);
        assert_eq!(history[2].speaker.as_deref(), Some("moderator"));
    }

    /// R1 + P0-2 (评审修订): another speaker's assistant row is rewritten
    /// to `role:user` with the content stripped to a single Text block —
    /// NO `@` prefix (the wire layer adds it) — and the `speaker` field
    /// preserved for wire attribution. Thinking blocks never survive.
    #[test]
    fn role_history_other_speaker_rewritten_as_user() {
        let full = vec![
            user_text("topic"),
            assistant_blocks_speaker(
                vec![
                    ContentBlock::Thinking {
                        thinking: "moderator reasoning".to_string(),
                        signature: "sig-mod".to_string(),
                    },
                    ContentBlock::Text {
                        text: "主持人发言".to_string(),
                        cache_control: None,
                    },
                ],
                "moderator",
            ),
        ];

        let history = role_history(&full, "M1");

        assert_eq!(history.len(), 2);
        let rewritten = &history[1];
        assert_eq!(
            rewritten.role,
            Role::User,
            "other speaker never stays assistant"
        );
        assert_eq!(
            rewritten.content,
            MessageContent::Text("主持人发言".to_string()),
            "single Text block, verbatim text — no @ prefix, no Thinking"
        );
        assert_eq!(
            rewritten.speaker.as_deref(),
            Some("moderator"),
            "speaker field preserved for the wire layer's attribution"
        );
        assert!(
            !rewritten.content.to_text().contains('@'),
            "content must NOT carry the @ prefix (apply_speaker_prefix would double it)"
        );
    }

    /// R1: other speakers' Thinking / RedactedThinking blocks never enter
    /// the current role's context — they carry signatures bound to THEIR
    /// generation context (re-injecting would break the signature
    /// round-trip or be echoed as if this role produced them).
    #[test]
    fn role_history_other_thinking_dropped() {
        let full = vec![
            user_text("topic"),
            assistant_blocks_speaker(
                vec![
                    ContentBlock::Thinking {
                        thinking: "moderator reasoning".to_string(),
                        signature: "sig-mod".to_string(),
                    },
                    ContentBlock::RedactedThinking {
                        data: "opaque".to_string(),
                    },
                    ContentBlock::Text {
                        text: "主持人发言".to_string(),
                        cache_control: None,
                    },
                ],
                "moderator",
            ),
            assistant_blocks_speaker(
                vec![
                    ContentBlock::Thinking {
                        thinking: "other participant reasoning".to_string(),
                        signature: "sig-other".to_string(),
                    },
                    ContentBlock::Text {
                        text: "M2 发言".to_string(),
                        cache_control: None,
                    },
                ],
                "M2",
            ),
        ];

        let history = role_history(&full, "M1");

        assert!(
            !history.iter().any(|m| matches!(
                &m.content,
                MessageContent::Blocks(blocks)
                    if blocks.iter().any(|b| matches!(
                        b,
                        ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. }
                    ))
            )),
            "no other speaker's thinking may survive: {:?}",
            history
        );
        assert_eq!(history[1].content.to_text(), "主持人发言");
        assert_eq!(history[2].content.to_text(), "M2 发言");
    }

    /// R3: another speaker's tool pair (assistant ToolUse + its user
    /// tool_result row) is stripped WHOLE — tool results are NOT shared,
    /// only the text remark is relayed. No orphan survives.
    #[test]
    fn role_history_other_tool_pair_dropped() {
        let full = vec![
            user_text("topic"),
            assistant_blocks_speaker(
                vec![
                    ContentBlock::Text {
                        text: "看下文件".to_string(),
                        cache_control: None,
                    },
                    tool_use("r1", "read_file"),
                ],
                "moderator",
            ),
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![tool_result("r1")]),
                speaker: None,
            },
            assistant_blocks_speaker(
                vec![ContentBlock::Text {
                    text: "M1 发言".to_string(),
                    cache_control: None,
                }],
                "M1",
            ),
        ];

        let history = role_history(&full, "M1");

        assert_eq!(history.len(), 3, "tool pair dropped, text remarks kept");
        let text: Vec<String> = history.iter().map(|m| m.content.to_text()).collect();
        assert_eq!(text, vec!["topic", "看下文件", "M1 发言"]);
        assert!(
            !history.iter().any(|m| matches!(
                &m.content,
                MessageContent::Blocks(blocks)
                    if blocks.iter().any(|b| matches!(
                        b,
                        ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. }
                    ))
            )),
            "other speaker's tool pair must be fully stripped: {:?}",
            history
        );
        assert!(no_orphan_pairs(&history));
    }

    /// R3: the CURRENT role's own tool pair is preserved verbatim
    /// (role pairing stays legal — OpenAI rejects a tool_result without
    /// its tool_use with HTTP 400).
    #[test]
    fn role_history_own_tool_pair_preserved() {
        let full = vec![
            user_text("topic"),
            assistant_blocks_speaker(
                vec![
                    ContentBlock::Text {
                        text: "看下文件".to_string(),
                        cache_control: None,
                    },
                    tool_use("r1", "read_file"),
                ],
                "M1",
            ),
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![tool_result("r1")]),
                speaker: None,
            },
        ];

        let history = role_history(&full, "M1");

        assert_eq!(history.len(), 3);
        assert_eq!(history[1], full[1], "own tool_use row verbatim");
        assert_eq!(history[2], full[2], "own tool_result row kept");
        assert!(no_orphan_pairs(&history));
    }

    /// R1: the human's original prompt (`speaker == None`) is preserved
    /// as `role:user` — the discussion topic is never lost.
    #[test]
    fn role_history_human_prompt_preserved() {
        let full = vec![user_text("聊聊这个项目")];
        let history = role_history(&full, "M1");
        assert_eq!(history, full);
        assert_eq!(history[0].role, Role::User);
        assert_eq!(history[0].speaker, None);
    }

    /// R1 (08-04 invariant carried over): the moderator's arbitration
    /// pair (nominate/end) never reaches a participant — showing it made
    /// participants conclude they WERE the moderator (DB session
    /// evidence, research/db-evidence.md §2). Here the pair is stripped
    /// via the generic "other speaker's tool pair" path; the moderator's
    /// text remark is still relayed.
    #[test]
    fn role_history_moderator_arbitration_dropped_for_participant() {
        let full = vec![
            user_text("hello"),
            assistant_blocks_speaker(
                vec![
                    ContentBlock::Text {
                        text: "先请 M1".to_string(),
                        cache_control: None,
                    },
                    tool_use("c1", "nominate_speaker"),
                ],
                "moderator",
            ),
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![tool_result("c1")]),
                speaker: None,
            },
            assistant_blocks_speaker(
                vec![ContentBlock::Text {
                    text: "M1 发言".to_string(),
                    cache_control: None,
                }],
                "M1",
            ),
        ];

        let history = role_history(&full, "M1");

        assert_eq!(history.len(), 3, "arbitration pair dropped");
        assert_eq!(history[1].content.to_text(), "先请 M1");
        assert_eq!(history[1].role, Role::User);
        assert!(
            !has_arbitration_blocks(&history),
            "no arbitration may survive in the participant history"
        );
        assert!(no_orphan_pairs(&history));
    }

    /// R1: the moderator keeps its OWN arbitration history (it must know
    /// who it has already nominated — 跨轮连贯, AC5). The pair is its
    /// own speaker, so it passes through verbatim.
    #[test]
    fn role_history_moderator_keeps_own_arbitration() {
        let full = vec![
            user_text("hello"),
            assistant_blocks_speaker(
                vec![
                    ContentBlock::Text {
                        text: "先请 M1".to_string(),
                        cache_control: None,
                    },
                    tool_use("c1", "nominate_speaker"),
                ],
                "moderator",
            ),
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![tool_result("c1")]),
                speaker: None,
            },
        ];

        let history = role_history(&full, "moderator");

        assert_eq!(history.len(), 3, "own pair preserved verbatim");
        assert_eq!(history[1], full[1]);
        assert_eq!(history[2], full[2]);
        assert!(no_orphan_pairs(&history));
    }

    /// R1: a participant speaking multiple times keeps ALL its own
    /// assistant rows (every turn where speaker == current_role) while
    /// other speakers' rows rewrite to user — the isolation must not
    /// collapse into "keep only the latest turn".
    #[test]
    fn role_history_multiturn_same_role_preserved() {
        let full = vec![
            user_text("topic"),
            assistant_blocks_speaker(
                vec![ContentBlock::Text {
                    text: "M1 第一轮".to_string(),
                    cache_control: None,
                }],
                "M1",
            ),
            assistant_blocks_speaker(
                vec![ContentBlock::Text {
                    text: "主持人插话".to_string(),
                    cache_control: None,
                }],
                "moderator",
            ),
            assistant_blocks_speaker(
                vec![ContentBlock::Text {
                    text: "M1 第二轮".to_string(),
                    cache_control: None,
                }],
                "M1",
            ),
        ];

        let history = role_history(&full, "M1");

        let assistant_texts: Vec<String> = history
            .iter()
            .filter(|m| m.role == Role::Assistant)
            .map(|m| m.content.to_text())
            .collect();
        assert_eq!(
            assistant_texts,
            vec!["M1 第一轮".to_string(), "M1 第二轮".to_string()],
            "all own turns preserved as assistant"
        );
        let user_speakers: Vec<&str> = history
            .iter()
            .filter(|m| m.role == Role::User)
            .filter_map(|m| m.speaker.as_deref())
            .collect();
        assert_eq!(
            user_speakers,
            vec!["moderator"],
            "moderator remark rewritten to user"
        );
    }

    /// AC5 contract-level lock: the current role's Thinking block
    /// signature must round-trip COMPLETE through the assembler (the
    /// Anthropic API 400s on a dropped/mutated signature), and other
    /// speakers' signatures must NOT leak into this role's context.
    #[test]
    fn role_history_signature_roundtrip_contract() {
        let full = vec![
            assistant_blocks_speaker(
                vec![
                    ContentBlock::Thinking {
                        thinking: "reasoning".to_string(),
                        signature: "sig-abc-123".to_string(),
                    },
                    ContentBlock::Text {
                        text: "M1 发言".to_string(),
                        cache_control: None,
                    },
                ],
                "M1",
            ),
            assistant_blocks_speaker(
                vec![
                    ContentBlock::Thinking {
                        thinking: "moderator reasoning".to_string(),
                        signature: "sig-mod".to_string(),
                    },
                    ContentBlock::Text {
                        text: "主持人发言".to_string(),
                        cache_control: None,
                    },
                ],
                "moderator",
            ),
        ];

        let history = role_history(&full, "M1");

        let signatures: Vec<String> = history
            .iter()
            .filter_map(|m| match &m.content {
                MessageContent::Blocks(blocks) => blocks.iter().find_map(|b| match b {
                    ContentBlock::Thinking { signature, .. } => Some(signature.clone()),
                    _ => None,
                }),
                _ => None,
            })
            .collect();
        assert_eq!(
            signatures,
            vec!["sig-abc-123".to_string()],
            "own signature round-trips verbatim; the moderator's must not leak"
        );
    }

    /// P0-2 (评审修订): 归属策略方案 (a) —— 改写行 content 不带 `@` 前缀、
    /// 保留 speaker。经 wire 层序列化后,Anthropic `apply_speaker_prefix`
    /// 恰好插入一次 `@name: `(无双重前缀);OpenAI 侧 speaker 经 wire 转换
    /// 保留(适配器填 `name` 字段)。
    #[test]
    fn role_history_wire_no_double_prefix() {
        let full = vec![
            user_text("topic"),
            assistant_blocks_speaker(
                vec![ContentBlock::Text {
                    text: "主持人发言".to_string(),
                    cache_control: None,
                }],
                "moderator",
            ),
        ];
        let history = role_history(&full, "M1");
        let rewritten = &history[1];
        assert_eq!(rewritten.role, Role::User);
        assert_eq!(rewritten.speaker.as_deref(), Some("moderator"));
        assert_eq!(
            rewritten.content.to_text(),
            "主持人发言",
            "no @ prefix in the rewrite row"
        );

        // Anthropic wire: serialize the history the way the provider
        // body is built (ChatMessage JSON carries `speaker`), then run
        // the attribution pass.
        let mut body = serde_json::json!({
            "messages": history
                .iter()
                .map(|m| serde_json::to_value(m).unwrap())
                .collect::<Vec<_>>()
        });
        crate::llm::provider::anthropic::apply_speaker_prefix(&mut body);
        let wire_text = body["messages"][1]["content"].as_str().unwrap();
        assert!(
            wire_text.starts_with("@moderator: "),
            "Anthropic wire must attribute the rewrite row once: {wire_text:?}"
        );
        assert_eq!(
            wire_text.matches("@moderator:").count(),
            1,
            "no double @ prefix: {wire_text:?}"
        );
        assert!(
            body["messages"][1].get("speaker").is_none(),
            "apply_speaker_prefix must strip the speaker field from the wire body"
        );

        // OpenAI wire: the rewritten row converts to a user message that
        // still carries `speaker` (the adapter emits the native `name`
        // field from it) with the bare text (no @ prefix).
        let wire = crate::llm::provider::wire::chat_request_to_wire(
            crate::llm::types::ChatRequest {
                model: "m".to_string(),
                max_tokens: 1000,
                messages: history.clone(),
                system: None,
                stream: false,
                tools: vec![],
                thinking: None,
            },
            None,
        );
        match &wire.messages[1] {
            crate::llm::provider::wire::WireMessage::User { content, speaker } => {
                assert_eq!(content, "主持人发言");
                assert_eq!(speaker.as_deref(), Some("moderator"));
            }
            other => panic!("rewritten row must map to WireMessage::User, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // R1 (08-07-group-chat-review-fixes): identity-correctness contract
    // baseline. These tests are the AUTOMATED regression floor for the
    // self-awareness dimension — they do NOT run a real model, so they
    // cannot prove a weak model won't role-collapse at runtime. What
    // they DO prove: the structural invariants that any prompt-level
    // defense relies on (per-role isolated transcript history + an
    // unambiguous role-boundary system prompt) hold even under the
    // worst input we've actually seen in production (DB sessions
    // a6c87247 / b144cc2a: same-model combination + a weak model that
    // prefixes its reply with another participant's name). If someone
    // later weakens `role_history` or strips the identity-guard
    // block from the prompt, these fail immediately.
    // -----------------------------------------------------------------

    /// P1-1 (08-07-group-chat-role-history-isolation 评审,语义重写):
    /// the 08-04 `participant_view` structural contract ("the view never
    /// rewrites content — mislabeled rows pass through verbatim") is
    /// REPLACED by `role_history`'s per-speaker attribution contract: a
    /// mislabeled assistant row (`speaker="M3"` but content reads as
    /// D4F) is rewritten to `role:user + speaker=M3` with the TEXT
    /// unchanged (role_history reassigns role/speaker and drops
    /// thinking — it does not sanitize text). Arbitration stripping +
    /// pair atomicity invariants carry over unchanged.
    #[test]
    fn identity_contract_view_holds_under_same_model_and_mislabel() {
        // Worst-case `full`: the moderator's arbitration pair + an
        // assistant row whose speaker is "M3" but whose CONTENT reads
        // as if D4F is speaking (the role-collapse signature from DB
        // session a6c87247 seq 16).
        let full = vec![
            user_text("聊聊这个项目"),
            // Moderator arbitration pair — must strip atomically (now
            // via the generic "other speaker's tool pair" path).
            assistant_blocks_speaker(
                vec![
                    ContentBlock::Text {
                        text: "先请 M3".to_string(),
                        cache_control: None,
                    },
                    tool_use("c1", "nominate_speaker"),
                ],
                "moderator",
            ),
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![tool_result("c1")]),
                speaker: None,
            },
            // The weak-model mislabel row: speaker claims M3 but content
            // reads as D4F speaking.
            assistant_blocks_speaker(
                vec![ContentBlock::Text {
                    text: "@D4F: 接过 M3 留的钩子…".to_string(),
                    cache_control: None,
                }],
                "M3",
            ),
        ];

        let view = role_history(&full, "M1");

        // Structural invariant 1: zero arbitration blocks survive.
        let c1_survives = view.iter().any(|m| match &m.content {
            MessageContent::Blocks(blocks) => blocks.iter().any(
                |b| matches!(b, ContentBlock::ToolUse { id, name, .. }
                    if (name == NOMINATE_SPEAKER_TOOL_NAME || name == END_DISCUSSION_TOOL_NAME)
                        && id == "c1")
                    || matches!(b, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "c1"),
            ),
            _ => false,
        });
        assert!(
            !c1_survives,
            "arbitration pair c1 must be fully stripped: {view:?}"
        );
        // Structural invariant 2: pair atomicity (no orphan).
        assert!(
            no_orphan_pairs(&view),
            "no orphan tool_use/result: {view:?}"
        );
        // Structural invariant 3 (P1-1 语义重写): the mislabeled row is
        // RE-ATTRIBUTED, not passed through as assistant — it becomes
        // role:user + speaker=M3, text byte-identical (no sanitization:
        // role_history only reassigns role/speaker and drops thinking).
        let mislabeled = view
            .iter()
            .find(|m| m.speaker.as_deref() == Some("M3"))
            .expect("mislabeled row must survive re-attribution");
        assert_eq!(
            mislabeled.role,
            Role::User,
            "mislabeled row must NOT survive as assistant (per-role isolation)"
        );
        assert_eq!(
            mislabeled.content.to_text(),
            "@D4F: 接过 M3 留的钩子…",
            "text is NOT sanitized — only role/speaker are reassigned"
        );
    }

    /// R1: under a same-model combination (moderator and a participant
    /// share the same model_id — the hardest identity case, mandated
    /// supported by PRD D2), the system prompts still draw an
    /// unambiguous role boundary. The participant prompt must assert
    /// it is the participant + forbid adopting the moderator's voice;
    /// the moderator prompt must list the participant in the roster +
    /// not contain the participant's identity-guard wording. This is
    /// the prompt-level structural floor that the view test above
    /// depends on.
    #[test]
    fn identity_contract_prompts_separate_roles_under_same_model() {
        // Same-model combination: moderator_model_id == participant.model.
        // The hardest identity case (PRD D2 mandates it supported).
        let same_model = "deepseek-v4-flash";
        let ctx = GroupChatCtx {
            participants: vec![crate::agent::group_chat::ParticipantConfig {
                name: "D4F".to_string(),
                model: same_model.to_string(),
                persona_md: None,
            }],
            moderator_model_id: same_model.to_string(),
        };

        let participant_prompt = participant_system_prompt("D4F", None);
        let moderator_prompt = moderator_system_prompt(&ctx);

        // Participant prompt: asserts it is D4F + the identity guard.
        assert!(
            participant_prompt.contains("You are D4F"),
            "participant prompt must name the participant: {participant_prompt:?}"
        );
        assert!(
            participant_prompt.contains("The moderator's messages are NOT yours"),
            "participant prompt must carry the role-boundary guard: {participant_prompt:?}"
        );
        assert!(
            participant_prompt.contains("never start your reply with")
                && participant_prompt.contains("your OWN name or role"),
            "participant prompt must forbid self-label prefixes: {participant_prompt:?}"
        );

        // Moderator prompt: lists D4F in the roster + the moderator's
        // own no-self-label rule. Must NOT carry the participant's
        // "you are D4F" wording (would be a role leak).
        assert!(
            moderator_prompt.contains("D4F"),
            "moderator prompt must list the participant: {moderator_prompt:?}"
        );
        assert!(
            moderator_prompt.contains("You are the MODERATOR"),
            "moderator prompt must assert the moderator role: {moderator_prompt:?}"
        );
        assert!(
            !moderator_prompt.contains("You are D4F"),
            "moderator prompt must NOT carry the participant's identity wording (role leak): {moderator_prompt:?}"
        );
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
