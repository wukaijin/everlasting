//! Group-chat turn-taking orchestrator (07-29-group-chat, Phase 3.4).
//!
//! `run_group_chat_loop` is an OUTER loop that drives a multi-LLM
//! discussion by repeatedly calling `run_chat_loop` (one call per
//! speaker). All speakers share the same `messages: Vec<ChatMessage>`
//! (reloaded from the DB between turns — `run_chat_loop` returns `()`
//! and persists its state), which is what makes the conversation
//! mutually visible (互见性): each speaker sees every prior utterance.
//!
//! Turn-taking (D3 = moderator-driven + human preemption):
//! ```text
//! loop {
//!   1. moderator turn  → run_chat_loop(moderator, tools=[nominate,end], state)
//!      └─ moderator calls nominate_speaker(X) or end_discussion
//!   2. read SharedTurnState
//!   3. if end_discussion → break
//!   4. resolve nominee X → (provider, system_prompt) from GroupChatCtx
//!   5. reload messages from DB
//!   6. participant X turn → run_chat_loop(X_provider, X_prompt, max_turns=1, speaker=X)
//!   7. reload messages from DB
//!   └─ back to 1
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
use crate::background_shell::DefaultRegistry;
use crate::db;
use crate::llm::types::{ChatMessage, MessageContent, Role, ToolDef};
use crate::memory::MemoryCache;
use crate::skill::loader::SkillCache;
use crate::state::ProviderCatalog;
use crate::tools::nominate_speaker::{GroupChatTurnState, SharedTurnState};
use crate::tools::read_guard::ReadGuard;

/// Hard cap on moderator↔participant round-trips. Prevents a
/// runaway loop where the moderator keeps nominating forever. The
/// D10 context-window compaction still applies per-turn inside each
/// `run_chat_loop`; this is an outer-loop safety bound.
const MAX_ORCHESTRATION_ROUNDS: usize = 30;

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
         Participants (you are NOT one of them — you moderate):\n{}\n\
         \n\
         Participants can see each other's prior remarks, so they can respond, \
         agree, or push back. Pick the order that best explores the topic. Keep \
         the discussion focused; end it when it has run its course.",
        roster.join("\n")
    )
}

/// A participant's system prompt = its persona (D8: inline markdown
/// from metadata). If no persona, a minimal default so the model
/// knows it's in a group discussion.
fn participant_system_prompt(name: &str, persona_md: Option<&str>) -> String {
    match persona_md {
        Some(p) if !p.trim().is_empty() => p.to_string(),
        _ => format!(
            "You are {}, a participant in a group chat discussion led by a moderator. \
             You can see what everyone else has said. Respond to the topic and to \
             other participants — agree, disagree, build on, or question their points. \
             Be concise and substantive.",
            name
        ),
    }
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
#[allow(clippy::too_many_arguments)]
pub async fn run_group_chat_loop(
    tool_defs: Vec<ToolDef>,
    context_window: u32,
    rid: String,
    session_id: String,
    mut messages: Vec<ChatMessage>,
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

    for round in 0..MAX_ORCHESTRATION_ROUNDS {
        if token.is_cancelled() {
            break;
        }

        // --- 1. Moderator turn ------------------------------------------------
        // The moderator gets the nominate/end tools (already in
        // builtin_tools) + the shared turn state. Its system prompt
        // is fully replaced (system_prompt_override = Some).
        if let Some(provider) = &moderator_provider {
            run_chat_loop(
                tool_defs.clone(),
                provider.clone(),
                context_window,
                rid.clone(),
                session_id.clone(),
                messages.clone(),
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
                // Moderator gets a few turns to think + call a tool.
                Some(3),
                false, // owns session_active slot
                false, // persist (moderator turns are part of the record)
                Some(false),
                worker_catalog.clone(),
                worker_event_sink.clone(),
                Some(moderator_prompt.clone()),
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
            Some(n) => n,
            None => {
                // Fallback (D7 risk): moderator didn't nominate.
                // Round-robin to the next participant so the
                // discussion isn't stuck.
                if gc_ctx.participants.is_empty() {
                    tracing::warn!(
                        round,
                        "group_chat: no participants to fall back to; stopping"
                    );
                    break;
                }
                let idx = round % gc_ctx.participants.len();
                tracing::warn!(round, fallback=?gc_ctx.participants[idx].name, "group_chat: moderator didn't nominate; round-robin fallback");
                gc_ctx.participants[idx].name.clone()
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

        // --- 5. Reload shared transcript --------------------------------------
        messages = reload_messages(&db, &session_id).await;
        // Tag the final assistant message's speaker? No — the
        // participant's run_chat_loop persists with speaker via the
        // persist_turn path once Phase 3 wires speaker into the
        // loop's assistant-row construction. For now the speaker
        // identity travels via the system prompt + (Phase 2) wire
        // injection.

        let Some(provider) = participant_provider else {
            tracing::warn!(round, model=%participant.model, "group_chat: participant provider unresolved; skipping turn");
            continue;
        };

        // --- 6. Participant turn ----------------------------------------------
        // Single turn (max_turns=1): speak once, then hand back to
        // the moderator. The participant's system prompt is its
        // persona (fully replaces the parent prompt).
        run_chat_loop(
            // Participant does NOT get nominate/end tools — only the
            // moderator arbitrates. Reuse tool_defs but the
            // interception no-ops (group_chat_state = None here).
            crate::tools::builtin_tools(),
            provider,
            context_window,
            rid.clone(),
            session_id.clone(),
            messages.clone(),
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
            None, // group_chat_state — participant doesn't arbitrate
            // Group chat (Phase 4 TODO-A): each participant's
            // assistant turns persist with `speaker = participant.name`
            // (the user-visible display name + session-scoped unique
            // identifier). The reload fuses these into the next
            // speaker's view so every participant sees the full
            // transcript with the correct attribution.
            Some(participant.name.clone()),
        )
        .await;

        // --- 7. Reload for the next moderator turn ----------------------------
        messages = reload_messages(&db, &session_id).await;
    }

    if !token.is_cancelled() {
        tracing::warn!(
            rounds = MAX_ORCHESTRATION_ROUNDS,
            "group_chat: hit max rounds; stopping"
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
