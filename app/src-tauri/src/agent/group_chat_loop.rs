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

use crate::agent::chat_loop::{
    run_chat_loop, CallerRole, ChatLoopDeps, ChatLoopDepsParts, ChatLoopRequest,
};
use crate::agent::group_chat::GroupChatCtx;
use crate::agent::group_chat_prompts::{
    group_chat_tool_defs, moderator_system_prompt, participant_system_prompt, role_history,
};
use crate::agent::helpers::emit_chat_event_via_sink;
use crate::background_shell::DefaultRegistry;
use crate::db;
use crate::llm::types::{ChatEvent, ChatMessage, MessageContent, Role, ToolDef};
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
pub(crate) async fn reload_messages(db: &SqlitePool, session_id: &str) -> Vec<ChatMessage> {
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
            // B1 (2026-08-16): reconstruct image-attachment refs from
            // `messages.metadata.attachments` so every participant's
            // request rebuilds the history's ImageRef blocks (the
            // per-speaker `run_chat_loop` attach pass consumes this
            // field). Classic chat rides the same manifest through the
            // frontend history instead.
            let attachments: Option<Vec<crate::llm::types::AttachmentRef>> = m
                .metadata
                .as_ref()
                .and_then(|md| md.get("attachments").cloned())
                .and_then(|v| serde_json::from_value(v).ok())
                .filter(|v: &Vec<crate::llm::types::AttachmentRef>| !v.is_empty());
            ChatMessage {
                role,
                content,
                speaker: m.speaker,
                attachments,
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
    // 08-20-turn-usage-event-quota-view WP2: 父 session 解析模型的
    // provider id(群聊 speaker 各自模型解析失败/未覆盖时的归因兜底)。
    provider_id: Option<String>,
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
    let (moderator_provider, moderator_provider_id) =
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
            &session_id,
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
            // RULE-ARGS-001：每 speaker 重建三套件（moderator）。
            // 历史契约注记迁入 suite.rs 对应字段文档：
            // - max_turns=Some(1)（08-04 follow-up「moderator 单轮」，用户决议）；
            // - system_prompt_override=Some(prompt) —— 完全替换父提示词；
            // - group_chat_state=Some(turn_state)：nominate/end 拦截回写点；
            // - current_speaker="moderator"（Phase 4 TODO-A 固定标识）；
            // - is_worker=Some(false)、skip 三兄弟 false（moderator 回合
            //   属于会话记录，guard-owned 清理照旧）；
            // - 群聊传空 stub registry：stubify/append gate `!is_group_chat`
            //   与拦截 gate（group_chat_state.is_some()）都会挡掉本路径，
            //   registry 不被读写，只作占位。
            let deps = ChatLoopDeps::from(ChatLoopDepsParts {
                db: db.clone(),
                cancellations: cancellations.clone(),
                session_active_request: session_active_request.clone(),
                read_guard: read_guard.clone(),
                memory_cache: memory_cache.clone(),
                skill_cache: skill_cache.clone(),
                permission_asks: permission_asks.clone(),
                token: token.clone(),
                background_shells: background_shells.clone(),
                stub_loaded: std::sync::Arc::new(crate::tools::stub::StubRegistry::new()),
                question_store: question_store.clone(),
                subagent_cache: subagent_cache.clone(),
            });
            let role = CallerRole {
                is_worker: Some(false),
                skip_session_active: false,
                skip_persist: false,
                skip_cancellations: false,
                worker_catalog: worker_catalog.clone(),
                worker_event_sink: worker_event_sink.clone(),
                system_prompt_override: Some(prompt),
                worker_run_id: None,
                run_grants: None,
                worktree_override: None,
                project_main_override: None,
                app_data_dir: app_data_dir.clone(),
                forced_dispatch: None,
            };
            run_chat_loop(
                ChatLoopRequest {
                    tool_defs: group_chat_tool_defs(&tool_defs, true),
                    provider: provider.clone(),
                    context_window,
                    provider_id: moderator_provider_id.clone().or(provider_id.clone()),
                    rid: rid.clone(),
                    session_id: session_id.clone(),
                    messages: history,
                    sink: sink.clone(),
                    resend_seq,
                    max_turns: Some(1),
                    workflow_ctx: None,
                    group_chat_state: Some(turn_state.clone()),
                    current_speaker: Some("moderator".to_string()),
                },
                deps,
                role,
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
                    &session_id,
                    &rid,
                    &ChatEvent::Done {
                        stop_reason: Some(STOP_REASON_NOMINEE_UNKNOWN.to_string()),
                        usage: None,
                    },
                );
                continue;
            }
        };
        let (participant_provider, participant_provider_id) =
            resolve_provider(&worker_catalog, &participant.model, &db).await;
        let participant_prompt =
            participant_system_prompt(&participant.name, participant.persona_md.as_deref());

        let Some(provider) = participant_provider else {
            // R2: same non-terminal notice pattern as nominee_unknown —
            // the participant's model couldn't be resolved (catalog miss),
            // so the turn is skipped but the discussion continues.
            tracing::warn!(round, model=%participant.model, "group_chat: participant provider unresolved; skipping turn");
            emit_chat_event_via_sink(
                &sink,
                &session_id,
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
            &session_id,
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
        // RULE-ARGS-001：每 speaker 重建三套件（participant）。
        // 历史契约注记迁入 suite.rs 对应字段文档：
        // - max_turns=Some(20)（08-07 R3 participant 多轮调研预算，
        //   外层 MAX_ORCHESTRATION_ROUNDS 是硬帽）；
        // - system_prompt_override=Some(persona)，完全替换父提示词；
        // - group_chat_state 同样 Some（D-D 守卫的范围条件需要——参与者
        //   视图的尾条 user 也是已落库行，避免重持久化；仲裁工具已被
        //   白名单剥离，拦截分支不会触发，design deviation note 见旧注释）；
        // - current_speaker=Some(name)：Phase 4 TODO-A 归因持久化。
        let deps = ChatLoopDeps::from(ChatLoopDepsParts {
            db: db.clone(),
            cancellations: cancellations.clone(),
            session_active_request: session_active_request.clone(),
            read_guard: read_guard.clone(),
            memory_cache: memory_cache.clone(),
            skill_cache: skill_cache.clone(),
            permission_asks: permission_asks.clone(),
            token: token.clone(),
            background_shells: background_shells.clone(),
            stub_loaded: std::sync::Arc::new(crate::tools::stub::StubRegistry::new()),
            question_store: question_store.clone(),
            subagent_cache: subagent_cache.clone(),
        });
        let role = CallerRole {
            is_worker: Some(false),
            skip_session_active: false,
            skip_persist: false,
            skip_cancellations: false,
            worker_catalog: worker_catalog.clone(),
            worker_event_sink: worker_event_sink.clone(),
            system_prompt_override: Some(participant_prompt),
            worker_run_id: None,
            run_grants: None,
            worktree_override: None,
            project_main_override: None,
            app_data_dir: app_data_dir.clone(),
            forced_dispatch: None,
        };
        run_chat_loop(
            ChatLoopRequest {
                tool_defs: group_chat_tool_defs(&tool_defs, false),
                provider,
                context_window,
                provider_id: participant_provider_id.or(provider_id.clone()),
                rid: rid.clone(),
                session_id: session_id.clone(),
                messages: history,
                sink: sink.clone(),
                resend_seq: None,
                max_turns: Some(20),
                workflow_ctx: None,
                group_chat_state: Some(turn_state.clone()),
                current_speaker: Some(participant.name.clone()),
            },
            deps,
            role,
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
            &session_id,
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
// 08-20-turn-usage-event-quota-view WP2: 同时返回该模型的 provider 行
// id(catalog 键是 model_id,`get_model` 一查可得;查不到 = None,
// caller 落父默认或 NULL)。
async fn resolve_provider(
    catalog: &Option<Arc<tokio::sync::RwLock<ProviderCatalog>>>,
    model_id: &str,
    db: &SqlitePool,
) -> (Option<Arc<dyn crate::llm::Provider>>, Option<String>) {
    let Some(catalog) = catalog.as_ref() else {
        return (None, None);
    };
    let guard = catalog.read().await;
    let provider = guard.get(model_id).cloned().or_else(|| {
        tracing::warn!(
            model_id,
            "group_chat: model_id not in catalog; turn will be skipped"
        );
        None
    });
    let provider_id = crate::db::models::get_model(db, model_id)
        .await
        .ok()
        .flatten()
        .map(|m| m.provider_id);
    (provider, provider_id)
}
