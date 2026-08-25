//! 群聊 prompt 与 per-role history 纯函数(拆分自 group_chat_loop.rs,
//! 08-07-large-file-splitting)。
//!
//! 零 IO 模块:moderator/participant system prompt、`role_history` per-role
//! 隔离组装、`group_chat_tool_defs` 工具白名单。编排循环仍在
//! [`super::group_chat_loop`] 中。

use crate::agent::group_chat::GroupChatCtx;
use crate::llm::types::{ChatMessage, ContentBlock, MessageContent, Role, ToolDef};
use crate::tools::end_discussion::END_DISCUSSION_TOOL_NAME;
use crate::tools::nominate_speaker::NOMINATE_SPEAKER_TOOL_NAME;

pub(crate) fn moderator_system_prompt(ctx: &GroupChatCtx) -> String {
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
pub(crate) fn role_history(full: &[ChatMessage], current_role: &str) -> Vec<ChatMessage> {
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
                            attachments: None,
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
const GROUP_CHAT_RESEARCH_TOOLS: &[&str] = &[
    "read_file",
    "grep",
    "glob",
    "list_dir",
    "web_fetch",
    // F4 (2026-08-25): snippet-only web search — read-only fixed-endpoint
    // op, same silent-Allow class as the others.
    "web_search",
];

/// Tools the moderator gets ON TOP of the research whitelist: the two
/// arbitration tools that drive the turn-taking loop. Participants never get
/// these — `participant_tool_defs` used to strip them; the whitelist now
/// achieves the same by simply not listing them for participants.
const MODERATOR_EXTRA_TOOLS: &[&str] = &[NOMINATE_SPEAKER_TOOL_NAME, END_DISCUSSION_TOOL_NAME];

/// Build the tool list a group-chat speaker sees, by whitelist.
/// `is_moderator = true` → research tools + arbitration tools.
/// `is_moderator = false` → research tools only (no arbitration, no
/// write/execute/interaction/skill/checklist tools).
pub(crate) fn group_chat_tool_defs(tool_defs: &[ToolDef], is_moderator: bool) -> Vec<ToolDef> {
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
pub(crate) fn participant_system_prompt(name: &str, persona_md: Option<&str>) -> String {
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
         You MAY research the codebase to ground your remarks — read_file / grep / glob / list_dir / web_fetch / web_search\n\
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
