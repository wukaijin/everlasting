//! Agent Loop body — production + test entry point
//! (P1 RULE-A-006, 2026-06-15).
//!
//! This file is the **single** implementation of the agent loop
//! body, called by both the production `chat` Tauri command
//! (with an `AppHandleSink`) and the integration tests in
//! `agent/tests.rs` (with a `MockEmitter`). Before the
//! RULE-A-006 closure, the production `chat.rs` carried a
//! ~1000-line inline spawn closure that was a faithful copy of
//! `run_chat_loop`; PR4 (06-14-p0-c3-tail-pair-orphan) had
//! already proven the two could drift and would have to be kept
//! in sync. The closure migration removed the copy — production
//! now routes through this function, and the 9 `agent_loop_*`
//! integration tests cover the real production path.
//!
//! All four event channels (`chat-event` / `tool:call` /
//! `tool:result` / `permission:ask`) dispatch through the
//! `dyn ChatEventSink` trait so a `MockEmitter` can record
//! events into a Vec for test assertion. The production
//! `AppHandleSink` forwards to `tauri::AppHandle::emit` for
//! live IPC dispatch. The `permissions::check` Tier 3
//! `permission:ask` path uses the same trait (this is the
//! reason `ChatEventSink` was introduced — and why the trait
//! is now exercised in production at every emit site, not
//! just the test variant).
//!
//! # What this function does NOT do
//!
//! - Does NOT run the catalog lookup / pre-flight. Callers must
//!   resolve a `Provider` themselves and pass it in.
//! - Does NOT own the `AppHandle` / cancellation token
//!   registration — callers register the token in the
//!   `cancellations` map and pass the clone here.
//! - Does NOT call `tauri::async_runtime::spawn`. The caller
//!   decides whether to run inline (tests) or in a background
//!   task (production). Production callers MUST `spawn` to
//!   preserve the existing Tauri command's "return immediately"
//!   semantic.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use sqlx::SqlitePool;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::agent::helpers::{
    build_synthetic_tool_result_message, emit_chat_event_via_sink, persist_turn_cwd,
    CANCELLED_MARKER, ERROR_MARKER,
};
use crate::agent::permissions::{self, Decision, PermissionContext};
use crate::agent::thinking::{flush_pending_thinking, PendingThinking};
use crate::agent::MAX_TURNS;
use crate::background_shell::BackgroundShellRegistry;
use crate::llm::{
    ChatEvent, ChatMessage, ContentBlock, LlmErrorCategory, MessageContent, Provider, Role,
    ToolDef,
};
use crate::memory::MemoryCache;
use crate::projects::boundary::is_within_root;
use crate::skill::loader::SkillCache;
use crate::state::{ChatEventSink, ToolCallPayload};
use crate::tools::read_guard::ReadGuard;
use crate::tools::ToolContext;

/// Production + test entry point for the agent loop body
/// (P1 RULE-A-006, 2026-06-15). Called by:
///
/// - The `chat` Tauri command in `chat.rs`, which builds an
///   `AppHandleSink` and spawns the call on the Tauri runtime.
///   This is the **production** path — every real chat request
///   routes through here.
/// - The 9 `agent_loop_*` integration tests in `agent/tests.rs`,
///   which build a `MockEmitter` and call this function inline
///   against scripted `MockProvider` responses. These tests now
///   cover the real production path (no separate "test
///   variant" exists).
///
/// The 14-parameter signature is unchanged from the previous
/// test-only variant; the production caller just supplies a
/// pre-resolved `Arc<dyn Provider>`, an `Arc<dyn ChatEventSink>`
/// wrapping the live `AppHandle`, and the standard
/// `AppState`-cloned resources (`db` / `read_guard` /
/// `memory_cache` / `permission_asks` / cancel maps).
///
/// `run_chat_loop` owns the per-turn `CancellationGuard` that
/// removes the (rid → token) and (session_id → rid) entries on
/// every exit path (normal / error / cancel / max_turns /
/// StillOver). The chat command's pre-flight inserts those
/// entries; the agent loop's own RAII Drop cleans them up.
#[allow(clippy::too_many_arguments)]
pub async fn run_chat_loop(
    tool_defs: Vec<ToolDef>,
    provider: Arc<dyn Provider>,
    context_window: u32,
    rid: String,
    session_id: String,
    messages: Vec<ChatMessage>,
    sink: Arc<dyn ChatEventSink>,
    db: SqlitePool,
    cancellations: Arc<Mutex<std::collections::HashMap<String, CancellationToken>>>,
    session_active_request: Arc<Mutex<std::collections::HashMap<String, String>>>,
    read_guard: ReadGuard,
    memory_cache: Arc<MemoryCache>,
    skill_cache: Arc<SkillCache>,
    permission_asks: crate::agent::permissions::PermissionStore,
    token: CancellationToken,
    // D3 PR3 (2026-06-17): resend context. When `Some(seq)`,
    // the user-message persist site (just after this function
    // captures `last_user_snapshot`) writes a `resend_message`
    // audit row pointing at the original user message's seq.
    // `None` for normal first-time sends. Best-effort (DB
    // audit failure does NOT abort the chat — the user has
    // already seen the assistant's new turn stream).
    resend_seq: Option<i64>,
    // L1a (2026-06-19): cross-request background-shell registry.
    // Threaded into the per-turn `ToolContext` so the 3 L1a tools
    // (`run_background_shell` / `shell_status` / `shell_kill`) can
    // call into it. The agent loop itself reads it once per turn
    // (after C3 compaction, before `provider.send`) to drain
    // pending completion notifications and inject them as
    // user-role messages.
    background_shells: crate::background_shell::DefaultRegistry,
) {
    // RAII: removes the (rid → token) AND (session_id → rid)
    // entries on every exit path. Mirrors the original closure's
    // guard. The `tauri::async_runtime::spawn` inside `Drop` is
    // a no-op in the in-process test path (it just enqueues to
    // the global Tokio runtime), but it does no harm and keeps
    // the cancellation-map invariant identical to production.
    let _cancel_guard = crate::state::CancellationGuard {
        cancellations: cancellations.clone(),
        session_active_request: session_active_request.clone(),
        request_id: rid.clone(),
        session_id: session_id.clone(),
    };
    let mut messages = messages;

    // Start seq from the highest existing seq in this session + 1.
    let loaded_session = match crate::db::load_session(&db, &session_id).await {
        Ok(Some(loaded)) => loaded,
        Ok(None) => {
            tracing::warn!(session_id = %session_id, "session not found");
            sink.emit_chat_event(&crate::state::ChatEventPayload {
                request_id: rid.clone(),
                event: ChatEvent::Error {
                    message: format!("session {} not found", session_id),
                    category: LlmErrorCategory::InvalidRequest,
                },
            });
            return;
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to load session");
            return;
        }
    };
    let next_seq = loaded_session
        .messages
        .iter()
        .map(|m| m.seq)
        .max()
        .map(|s| s + 1)
        .unwrap_or(0);
    let mut seq = next_seq;

    // The agent loop uses a directory-bound worktree + cwd. The
    // test setup creates a project whose `path` we use directly
    // (no worktree); we read it from the session's project.
    let project = match crate::db::get_project(&db, &loaded_session.session.project_id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            sink.emit_chat_event(&crate::state::ChatEventPayload {
                request_id: rid.clone(),
                event: ChatEvent::Error {
                    message: format!(
                        "project {} not found for this session",
                        loaded_session.session.project_id
                    ),
                    category: LlmErrorCategory::InvalidRequest,
                },
            });
            return;
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to load project");
            return;
        }
    };
    let session_root_raw = loaded_session
        .session
        .worktree_path
        .clone()
        .unwrap_or_else(|| project.path.clone());
    let worktree_path = match crate::projects::boundary::assert_within_root(
        std::path::Path::new(&session_root_raw),
        std::path::Path::new(&session_root_raw),
    ) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(session_id = %session_id, error = %e, "session root invalid");
            sink.emit_chat_event(&crate::state::ChatEventPayload {
                request_id: rid.clone(),
                event: ChatEvent::Error {
                    message: format!("session root is invalid: {}", e),
                    category: LlmErrorCategory::InvalidRequest,
                },
            });
            return;
        }
    };
    let session_cwd_raw = if loaded_session.session.current_cwd.is_empty() {
        worktree_path.to_string_lossy().to_string()
    } else {
        loaded_session.session.current_cwd.clone()
    };
    let session_cwd = match crate::projects::boundary::assert_within_root(
        &worktree_path,
        std::path::Path::new(&session_cwd_raw),
    ) {
        Ok(p) => p,
        Err(_) => worktree_path.clone(),
    };
    let turn_ctx = ToolContext {
        worktree_path: worktree_path.clone(),
        cwd: session_cwd.clone(),
        // B12 (2026-06-19): per-request checklist handle. Constructed
        // fresh for each `run_chat_loop` call so a new user message
        // (or D3 resend fork) starts with an empty list. The handle
        // is threaded through `ToolContext` so `update_checklist::execute`
        // can atomically mutate it; the same handle is read every turn
        // to build the ephemeral injection block (see `inject_checklist`
        // below).
        checklist: crate::tools::update_checklist::new_handle(),
        // L1a (2026-06-19): cross-request background-shell registry.
        // Pulled from `AppState` (which owns the single in-memory
        // impl); tools consume it from `ToolContext` so the registry
        // isn't plumbed through every tool signature.
        background_shells: background_shells.clone(),
    };
    let mut current_ctx = turn_ctx;
    let mut last_cwd: Option<PathBuf> = None;

    let session_mode = loaded_session.session.mode;
    let permission_ctx = PermissionContext {
        session_id: session_id.clone(),
        mode: session_mode,
        cwd: session_cwd.clone(),
    };
    let mode_prefix = permissions::mode_system_prefix(session_mode);

    // B5 memory is empty in tests (no memory files written to the
    // temp project dir). Skip the synthetic user/assistant
    // inserts when `load_for_session` returns no layers.
    let memory_layers = load_for_session(&memory_cache, &project.id, &project.path).await;
    let instructions_blocks =
        crate::memory::loader::build_instructions_blocks(&memory_layers);
    let has_memory = !instructions_blocks.is_empty();
    if !instructions_blocks.is_empty() {
        messages.insert(
            0,
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(instructions_blocks),
            },
        );
        messages.insert(
            1,
            ChatMessage {
                role: Role::Assistant,
                content: MessageContent::Text(
                    "Understood. I will follow these instructions throughout our session."
                        .to_string(),
                ),
            },
        );
    }

    // B4 skill listing (L0): an independent synthetic user message,
    // decoupled from the memory instructions cache window so skill
    // add/remove does not bust the memory cache breakpoint (PR2
    // brainstorm Q1 decision). Empty when no skill files exist —
    // skipped, symmetric to the memory `instructions_blocks.is_empty()`
    // guard above.
    //
    // Uses `worktree_path` (not `project.path`) so the L0 listing
    // resolves from the same dir the `use_skill` L1 activation
    // (`tools/use_skill.rs`, via `ctx.worktree_path`) consults —
    // otherwise a worktree-attached session would list skills from
    // the main project root but resolve them from the worktree,
    // turning a matching listing into a "not found" on L1.
    // (`worktree_path` already went through `assert_within_root`
    // canonicalize above, so symlinks are resolved consistently on
    // both sides; `SkillCache` keys by the path string, so the L0
    // + L1 cache slots line up.)
    let skill_listing_path = worktree_path.to_string_lossy().to_string();
    let skill_infos =
        crate::skill::loader::list_skill_infos(&skill_cache, Some(&skill_listing_path)).await;
    let skill_blocks = crate::skill::loader::build_skill_listing_block(&skill_infos);
    if !skill_blocks.is_empty() {
        // Insert after the memory user/assistant pair (pos 2) when
        // memory is present, else at the head (pos 0).
        let skill_pos = if has_memory { 2 } else { 0 };
        messages.insert(
            skill_pos,
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(skill_blocks),
            },
        );
    }

    // System prompt is required even in tests; we build a minimal
    // one with empty session metadata. The real builder requires
    // a ProjectRow + SessionRow; tests can use the real ones.
    let head_sha = crate::agent::system_prompt::lookup_head_sha(&worktree_path);
    let base_prompt = crate::agent::system_prompt::build_system_prompt(
        &loaded_session.session,
        &project,
        &worktree_path,
        &head_sha,
    );
    let system_prompt = crate::agent::system_prompt::assemble_system_prompt(
        mode_prefix,
        &base_prompt,
    );
    let _ = &base_prompt;

    // Persist the most recent user message before the agent loop runs.
    //
    // B2 PR3 (2026-06-17): also snap the original (pre-inject)
    // content for the `persist_turn` call below. PR2 stores the
    // raw `@relpath` text as source of truth; PR3 adds the
    // injection manifest to `messages.metadata` so the frontend
    // hint row survives session reload. We keep BOTH the
    // original content (DB `content` + `text` columns) and the
    // manifest (DB `metadata` JSON) — the user sees the
    // original `@relpath` in the bubble and the hint row below
    // it; a reload reads both back.
    //
    // We capture the seq now (before persist) so the
    // `ChatEvent::FileInjections` event below can identify the
    // user row to the frontend (the controller's user-message
    // keys on reload are `${sid}-${seq}`, so `message_seq`
    // round-trips through the DB and matches the rehydrated
    // key).
    let (last_user_snapshot, last_user_seq) = if let Some(last_user) = messages.iter().rev().find(|m| m.role == Role::User) {
        let msg = last_user.clone();
        // RULE-A-003 (2026-06-15): if the very first user message
        // can't be persisted, abort with a visible Error —
        // continuing would let the LLM answer a message the DB
        // never recorded, so the next session reload is blank.
        if let Err(e) = crate::db::persist_turn(&db, &session_id, msg.role, &msg.content, seq, None)
            .await
        {
            emit_persist_failure(&sink, &rid, &e);
            return;
        }
        // D3 PR3 (2026-06-17): if the user hit Resend (instead of
        // Edit), the frontend passed `resend_seq` through the chat
        // IPC. Fire the `resend_message` audit row pointing at the
        // original user message's seq (the one the user clicked
        // Resend on). Best-effort: a failure is logged + swallowed
        // — audit loss is acceptable here because the user has
        // already seen the visual confirmation (the new assistant
        // turn is about to stream). The `content_text_preview`
        // comes from the ORIGINAL message's content (truncated to
        // 80 chars inside the helper), not the new send's text —
        // they're identical because Resend re-fires the same
        // prompt, but we use the ORIGINAL seq to keep the audit
        // link obvious ("you re-ran this row at T").
        //
        // Sits AFTER persist_turn so the audit row's payload can
        // safely reference `seq` (the original row's seq — the
        // user message we just persisted is a NEW row with seq=N+1,
        // not the one being re-run). The `resend_seq` is the seq
        // of the ORIGINAL user message; the new send uses seq=N+1.
        if let Some(original_seq) = resend_seq {
            // Derive a short text preview from the original
            // message's content. `MessageContent` carries
            // `to_text()` which concatenates all text blocks
            // (mirrors the `text` column write). We use the
            // in-memory `msg` (which equals what just got
            // persisted) — same text, same preview budget.
            let preview = msg.content.to_text();
            if let Err(e) = crate::agent::permissions::record_message_resend_audit(
                &db,
                &session_id,
                original_seq,
                &preview,
            )
            .await
            {
                tracing::warn!(
                    error = %e,
                    request_id = %rid,
                    session_id = %session_id,
                    original_seq = original_seq,
                    "chat_loop: record_message_resend_audit failed (non-fatal)"
                );
            }
        }
        // B2 PR3: snap the seq for the FileInjections event;
        // the original (un-injected) content stays in the
        // `messages` vec at this point because the inject
        // pass below mutates the in-memory copy in place —
        // but the DB row is already locked to the original.
        let user_seq = seq;
        seq += 1;
        (Some(msg.content), user_seq)
    } else {
        (None, -1)
    };

    // B2 PR2: expand `@relpath` tokens in user messages into file
    // content (text) or placeholder (image/PDF/Office/binary). Runs
    // AFTER the user message is persisted (DB keeps the original
    // `@relpath` as source of truth) and BEFORE the turn loop, so C3
    // compaction + `provider.send` see the expanded content. A reloaded
    // session re-expands against the current file contents.
    //
    // B2 PR3 (2026-06-17): the function now also returns the
    // per-token injection manifest for the LAST user text message.
    // We (a) persist the manifest as `messages.metadata` on the user
    // row (update, not insert — the row was just written above with
    // `None` metadata), and (b) push a `ChatEvent::FileInjections`
    // event so the live-streaming user message's hint row appears
    // before the assistant starts.
    let (last_user_after_inject, injections) =
        crate::agent::at_file::inject_at_tokens(&mut messages, &current_ctx).await;
    if !injections.is_empty() && last_user_snapshot.is_some() {
        // Update the user row with the injection manifest as
        // metadata. The `update_message_metadata` IPC at the
        // SQL layer (added in this PR — see `db::sessions.rs`)
        // is the single write path; using a fresh SQL UPDATE
        // here keeps the contract that `messages.metadata` is
        // only ever set by the agent loop.
        //
        // B2 PR3 (bug fix 2026-06-17): wrap the manifest in
        // an object envelope `{"injections": [...]}` so the
        // frontend rehydrate path can read it back via
        // `m.metadata.injections` (see
        // `streamController.ts::rehydrateMessages`). The
        // previous form (`serde_json::to_value(&injections)`)
        // serialized the `Vec<InjectionRecord>` directly as a
        // top-level JSON array, which the rehydrate path's
        // `meta.injections` lookup treated as undefined and
        // silently dropped every entry. The envelope leaves
        // room for future metadata fields (latency, tags,
        // links) without another rehydrate-path migration.
        let meta = serde_json::json!({ "injections": &injections });
        if let Err(e) = crate::db::update_message_metadata(
            &db,
            &session_id,
            last_user_seq,
            &meta,
        )
        .await
        {
            tracing::warn!(
                request_id = %rid,
                session_id = %session_id,
                message_seq = last_user_seq,
                error = %e,
                "agent loop: failed to persist injection manifest as messages.metadata (non-fatal)"
            );
        }
        // Live-push the manifest to the frontend. The
        // controller's `handleChatEvent("file_injections")`
        // case patches the user message's `injections` array
        // by `request_id` + `message_seq`.
        emit_chat_event_via_sink(
            &sink,
            &rid,
            &ChatEvent::FileInjections {
                request_id: rid.clone(),
                message_seq: last_user_seq,
                injections: injections.clone(),
            },
        );
    }
    // Silence the unused warning on `last_user_after_inject` —
    // we keep the in-place expansion in `messages` but the
    // returned clone is not needed (the chat loop iterates
    // `messages` directly downstream).
    let _ = last_user_after_inject;

    for turn in 1..=MAX_TURNS {
        // C3 compaction (test pass-through: if messages don't exceed
        // the test's tiny context_window, dropped_count == 0 and
        // the messages vec is unchanged).
        //
        // RULE-A-002 (2026-06-14): `compact_messages` now returns a
        // `DegradationKind` signal. `StillOver` means every safe
        // droppable candidate was exhausted but the budget is still
        // over target — sending the list would 400 on `prompt is
        // too long`. The agent loop emits an `Error` event +
        // terminates the chat instead of silently firing the
        // over-budget request. `None` / `NoCandidates` are safe-to-
        // proceed.
        {
            let compacted =
                crate::agent::context::compact_messages(messages.clone(), context_window).await;
            if compacted.dropped_count > 0 {
                tracing::info!(
                    request_id = %rid,
                    session_id = %session_id,
                    turn,
                    tokens_before = compacted.tokens_before,
                    tokens_after = compacted.tokens_after,
                    dropped_count = compacted.dropped_count,
                    context_window,
                    "agent loop: context compressed (C3)"
                );
            }
            match compacted.degradation {
                crate::agent::context::DegradationKind::None
                | crate::agent::context::DegradationKind::NoCandidates => {
                    messages = compacted.messages;
                }
                crate::agent::context::DegradationKind::StillOver {
                    tokens_after,
                    target,
                } => {
                    // FAIL FAST: surface the over-budget state to
                    // the frontend as a typed Error. Do NOT call
                    // `provider.send` — the response would 400 on
                    // `prompt is too long`. Identical message /
                    // tracing / category to production `chat.rs`.
                    tracing::error!(
                        request_id = %rid,
                        session_id = %session_id,
                        turn,
                        tokens_after,
                        target,
                        "agent loop: C3 compaction exhausted but still over target — aborting turn"
                    );
                    let msg = format!(
                        "Context window exceeded after compaction ({} tokens, target {}). \
                         A single tool_result or message may be too large — try a narrower query.",
                        tokens_after, target
                    );
                    sink.emit_chat_event(&crate::state::ChatEventPayload {
                        request_id: rid.clone(),
                        event: ChatEvent::Error {
                            message: msg,
                            category: LlmErrorCategory::InvalidRequest,
                        },
                    });
                    return;
                }
            }
        }

        let mut turn_send_at: Option<Instant> = None;
        let mut turn_first_delta_at: Option<Instant> = None;
        let mut turn_thinking_start: Option<Instant> = None;
        let mut turn_thinking_done: Option<Instant> = None;
        let mut turn_done_at: Option<Instant> = None;
        let _ = turn_send_at;

        // B12 (2026-06-19): ephemeral checklist injection. Each turn,
        // AFTER C3 compaction and BEFORE `provider.send`, if the
        // checklist Vec is non-empty, build a synthetic user block
        // carrying the full current list + an explicit "in progress"
        // focus marker, and APPEND it to a CLONE of `messages`. The
        // clone is the request body; the persisted `messages` Vec is
        // NEVER mutated by this injection — the block is regenerated
        // from the live Vec every turn.
        //
        // Why APPEND (not prepend)?
        // - **Cache correctness (load-bearing):** the memory
        //   instructions block lives at `messages[0]` and carries a
        //   `cache_control: Ephemeral` breakpoint on its banner block
        //   (see `memory/loader.rs::build_instructions_blocks`). The
        //   breakpoint is part of Anthropic's cache key — everything
        //   BEFORE it must be byte-identical across turns to hit. A
        //   per-turn-mutating checklist block at position 0 would
        //   sit IN FRONT of the memory breakpoint, busting the memory
        //   cache every turn (50 turns × ~100 KiB of instruction
        //   files = the exact cost explosion the B5 memory-caching
        //   work was built to eliminate). Appending keeps the
        //   checklist AFTER the memory breakpoint so the memory cache
        //   window stays intact. This mirrors why the B4 skill block
        //   was placed AFTER the memory pair (position 2), not at
        //   the head — same cache-preservation principle.
        // - Anthropic accepts consecutive user-role messages, so
        //   appending a user block after the user's latest prompt is
        //   wire-legal.
        // - The checklist content being the LAST thing in context is
        //   arguably better for recency: the model sees its current
        //   todo right before generating.
        //
        // Why not push into `messages` (the persisted Vec)?
        // - Replay correctness: the canonical checklist state lives
        //   in the `update_checklist` tool_results (persisted in
        //   history). A reload reconstructs from those tool_results;
        //   an injection block in `messages` would be a duplicate
        //   source of truth that drifts the moment the Vec changes.
        // - Context window: each turn's injection is per-turn-only;
        //   keeping it out of `messages` keeps the persisted history
        //   lean.
        //
        // No `cache_control` on the checklist block itself: the block
        // changes every turn (the LLM mutates the list), so a cache
        // breakpoint would never hit.
        //
        // Empty Vec (turn 1, before any `update_checklist` call) →
        // skip injection entirely, symmetric to memory/skill empty-
        // skip. We use the same `messages.clone()` for `provider.send`
        // whether or not we injected, so the non-checklist path is a
        // single extra `.clone()` per turn (cheap relative to LLM
        // network latency).

        // L1a (2026-06-19): drain completion notifications from the
        // background-shell registry. Each notification is appended
        // as a `user`-role message at the END of the request clone
        // (mirroring the checklist injection rule: APPEND, not
        // prepend, so the memory cache breakpoint at `messages[0]`
        // stays intact — see `.trellis/spec/backend/tool-contract.md`
        // §7 "Wrong vs Correct — injection placement"). The agent
        // loop drains ONCE per turn (not per tool_use): background
        // tasks may complete between turns, but the queue is
        // consumed on the next turn's request. Drained notifications
        // are GONE from the registry (drain_notifications is
        // destructive — see `background_shell::BackgroundShellRegistry`).
        //
        // Each notification produces ONE user message; the LLM tracks
        // multiple completions more reliably when they're separated
        // (a single merged message risks being read as a single
        // event with garbled exit codes).
        //
        // Format (per L1 PRD Q3 + Q4 decisions):
        //   `[system] 后台 shell <shell_session_id> 已完成,exit code <N>。调 shell_status(session_id="<id>") 看输出。`
        // Notifications are kept lean — only exit code + session id;
        // the LLM calls `shell_status` to pull stdout/stderr. Keeps
        // the per-turn context cost bounded for builds that fan out
        // into many background shells.
        let background_notifications =
            background_shells.drain_notifications(&session_id).await;
        let turn_messages = {
            let checklist_snapshot = current_ctx.checklist.lock().await.clone();
            let mut req = messages.clone();
            if !checklist_snapshot.is_empty() {
                let block = crate::tools::update_checklist::render_checklist(
                    &checklist_snapshot,
                );
                let text = format!(
                    "<current-checklist>\nThis is your running progress checklist for the current task. \
                     Items marked `[~]` are in progress; `[x]` are done; `[ ]` are pending. Use the \
                     `update_checklist` tool to mark items done / add new items / reorder as your plan \
                     evolves. The list is re-injected every turn so you don't lose track.\n{}\n</current-checklist>",
                    block
                );
                let checklist_msg = ChatMessage {
                    role: Role::User,
                    content: MessageContent::Blocks(vec![ContentBlock::Text {
                        text,
                        cache_control: None,
                    }]),
                };
                // APPEND, never prepend — see cache-correctness note
                // above. Prepending would bust the memory cache
                // breakpoint at messages[0].
                req.push(checklist_msg);
            }
            // L1a notifications: APPEND after the (optional)
            // checklist block. Same cache-correctness rule — keep
            // the memory breakpoint at messages[0] intact. Each
            // notification gets ONE message so the LLM sees
            // multiple completions as distinct events.
            for note in &background_notifications {
                let text = format!(
                    "[system] 后台 shell {} 已完成,exit code {}。调 shell_status(session_id=\"{}\") 看输出。",
                    note.shell_session_id,
                    note.exit_code
                        .map(|c: i32| c.to_string())
                        .unwrap_or_else(|| "N/A".to_string()),
                    note.shell_session_id,
                );
                let msg = ChatMessage {
                    role: Role::User,
                    content: MessageContent::Blocks(vec![ContentBlock::Text {
                        text,
                        cache_control: None,
                    }]),
                };
                req.push(msg);
            }
            req
        };

        let turn_tool_defs = permissions::filter_tools_for_mode(tool_defs.clone(), session_mode);
        let mut stream = provider.send(
            Some(system_prompt.clone()),
            turn_messages,
            turn_tool_defs,
        );
        turn_send_at = Some(Instant::now());

        let mut text_parts: Vec<String> = Vec::new();
        let mut tool_calls: Vec<(String, String, serde_json::Value)> = Vec::new();
        let mut finalized_thinking: Vec<(String, String)> = Vec::new();
        let mut redacted_thinking_data: Vec<String> = Vec::new();
        let mut pending_thinking: Option<PendingThinking> = None;
        let mut stop_reason: Option<String> = None;
        let mut last_usage: Option<crate::llm::types::TokenUsage> = None;
        let mut had_error = false;
        let mut cancelled = false;

        loop {
            tokio::select! {
                biased;
                _ = token.cancelled() => {
                    tracing::info!(request_id = %rid, "chat: cancellation requested by client");
                    cancelled = true;
                    break;
                }
                event_result = stream.next() => {
                    let Some(event_result) = event_result else { break; };
                    let event = match event_result {
                        Ok(e) => e,
                        // RULE-A-011 (2026-06-19): previously this arm
                        // silently wrapped `LlmError` into a
                        // `ChatEvent::Error` with NO tracing. The
                        // 2026-06-18 incident (`mz8s3hqwx6rmqjswgte`,
                        // messages.seq=37) hit exactly this: the
                        // reqwest 60s total-deadline fired mid-
                        // thinking, the partial turn was persisted,
                        // and the user saw a toast with no Rust-side
                        // breadcrumb. Add `tracing::warn!` so the
                        // next streaming failure is grep-able.
                        // See `.trellis/spec/backend/error-handling.md`
                        // §RULE-A-011.
                        Err(err) => {
                            tracing::warn!(
                                request_id = %rid,
                                turn,
                                // `LlmErrorCategory` only derives Debug
                                // (not Display), so use `?` (Debug)
                                // instead of `%` (Display) — produces the
                                // same five variant names (Auth /
                                // RateLimit / InvalidRequest / Server /
                                // Network) for grep purposes.
                                category = ?err.category(),
                                error = %err,
                                "chat: LLM stream errored"
                            );
                            ChatEvent::Error {
                                message: err.user_message(),
                                category: err.category(),
                            }
                        }
                    };
                    match &event {
                        ChatEvent::Start => {
                            emit_chat_event_via_sink(&sink, &rid, &event);
                        }
                        ChatEvent::Delta { text } => {
                            flush_pending_thinking(&mut pending_thinking, &mut finalized_thinking);
                            text_parts.push(text.clone());
                            if turn_first_delta_at.is_none() {
                                turn_first_delta_at = Some(Instant::now());
                            }
                            if turn_thinking_start.is_some() && turn_thinking_done.is_none() {
                                turn_thinking_done = Some(Instant::now());
                            }
                            emit_chat_event_via_sink(&sink, &rid, &event);
                        }
                        ChatEvent::ThinkingDelta { text } => {
                            let p = pending_thinking.get_or_insert_with(PendingThinking::default);
                            p.text.push_str(text);
                            if turn_thinking_start.is_none() {
                                turn_thinking_start = Some(Instant::now());
                            }
                            emit_chat_event_via_sink(&sink, &rid, &event);
                        }
                        ChatEvent::SignatureDelta { signature } => {
                            let p = pending_thinking.get_or_insert_with(PendingThinking::default);
                            p.signature.push_str(signature);
                            emit_chat_event_via_sink(&sink, &rid, &event);
                        }
                        ChatEvent::RedactedThinkingDelta { data } => {
                            redacted_thinking_data.push(data.clone());
                            emit_chat_event_via_sink(&sink, &rid, &event);
                        }
                        ChatEvent::ToolCall { id, name, input } => {
                            flush_pending_thinking(&mut pending_thinking, &mut finalized_thinking);
                            if turn_thinking_start.is_some() && turn_thinking_done.is_none() {
                                turn_thinking_done = Some(Instant::now());
                            }
                            tool_calls.push((id.clone(), name.clone(), input.clone()));
                            sink.emit_tool_call(&ToolCallPayload {
                                request_id: rid.clone(),
                                id: id.clone(),
                                name: name.clone(),
                                input: input.clone(),
                            });
                        }
                        ChatEvent::Done { stop_reason: sr, usage } => {
                            stop_reason = sr.clone();
                            last_usage = usage.clone();
                            turn_done_at = Some(Instant::now());
                            if turn_thinking_start.is_some() && turn_thinking_done.is_none() {
                                turn_thinking_done = Some(Instant::now());
                            }
                            if let Some(t) = usage {
                                if let Err(e) = crate::db::add_token_usage(&db, &session_id, t).await {
                                    tracing::warn!(error = %e, "chat: failed to accumulate token usage (non-fatal)");
                                }
                            }
                        }
                        ChatEvent::Error { .. } => {
                            if turn_thinking_start.is_some() && turn_thinking_done.is_none() {
                                turn_thinking_done = Some(Instant::now());
                            }
                            emit_chat_event_via_sink(&sink, &rid, &event);
                            had_error = true;
                        }
                        ChatEvent::ToolResult { .. } => {}
                        ChatEvent::TurnComplete { .. } => {
                            tracing::warn!(request_id = %rid, "chat: unexpected TurnComplete in LLM stream");
                        }
                        // B2 PR3: `FileInjections` is emitted ONCE per
                        // user turn from the agent loop's pre-turn
                        // hook (right after `inject_at_tokens` runs) —
                        // NOT from the LLM stream. A `FileInjections`
                        // arriving inside the per-event stream loop
                        // would mean the wire shape leaked (e.g. a
                        // provider re-emitted it). Drop it; the
                        // controller already received the legitimate
                        // one above.
                        ChatEvent::FileInjections { .. } => {
                            tracing::warn!(
                                request_id = %rid,
                                "chat: unexpected FileInjections in LLM stream (ignoring — already emitted pre-turn)"
                            );
                        }
                    }
                    if matches!(event, ChatEvent::Done { .. } | ChatEvent::Error { .. }) {
                        break;
                    }
                }
            }
        }

        // RULE-A-007 (2026-06-17): the error path no longer bails
        // out with raw `return`. Instead — symmetric with the
        // cancel path below — the agent loop flushes any pending
        // thinking, appends an `ERROR_MARKER` to the text, and
        // persists the partial turn so a reload shows the user
        // where the turn broke. Previously the error arm returned
        // immediately, dropping already-rendered
        // `text_parts` / `finalized_thinking` / `tool_calls`
        // — an asymmetry vs the cancel path that did persist.
        if cancelled {
            flush_pending_thinking(&mut pending_thinking, &mut finalized_thinking);
            tracing::info!(
                request_id = %rid,
                "chat: cancelled — persisting partial turn"
            );
        } else if had_error {
            flush_pending_thinking(&mut pending_thinking, &mut finalized_thinking);
            tracing::info!(
                request_id = %rid,
                "chat: errored — persisting partial turn"
            );
        }

        flush_pending_thinking(&mut pending_thinking, &mut finalized_thinking);

        let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
        for (thinking, signature) in &finalized_thinking {
            assistant_blocks.push(ContentBlock::Thinking {
                thinking: thinking.clone(),
                signature: signature.clone(),
            });
        }
        let mut full_text = text_parts.join("");
        if cancelled {
            if full_text.is_empty() {
                full_text = CANCELLED_MARKER.to_string();
            } else {
                full_text.push_str("\n\n");
                full_text.push_str(CANCELLED_MARKER);
            }
        } else if had_error {
            // RULE-A-007 (2026-06-17): symmetric to the
            // CANCELLED_MARKER branch above. Empty-text error →
            // marker alone; non-empty → marker appended after the
            // partial text. The UI renders the marker inline; a
            // reload reads both back from the DB.
            if full_text.is_empty() {
                full_text = ERROR_MARKER.to_string();
            } else {
                full_text.push_str("\n\n");
                full_text.push_str(ERROR_MARKER);
            }
        }
        if !full_text.is_empty() {
            assistant_blocks.push(ContentBlock::Text { text: full_text, cache_control: None });
        }
        for (id, name, input) in &tool_calls {
            assistant_blocks.push(ContentBlock::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
            });
        }
        for data in &redacted_thinking_data {
            assistant_blocks.push(ContentBlock::RedactedThinking { data: data.clone() });
        }

        if !assistant_blocks.is_empty() {
            let msg = ChatMessage {
                role: Role::Assistant,
                content: MessageContent::Blocks(assistant_blocks),
            };
            let turn_latency = build_turn_latency(
                turn_send_at,
                turn_first_delta_at,
                turn_thinking_start,
                turn_thinking_done,
                turn_done_at,
            );
            // RULE-A-003 (2026-06-15): assistant turn persist
            // failure → emit Error + abort. Previously this was a
            // silent log, but the `messages.push` + `seq += 1`
            // below it still ran, drifting the in-memory seq out
            // of sync with the DB. TurnComplete stays on the
            // success path only (unchanged).
            //
            // RULE-A-007 (2026-06-17): on the **error path**,
            // persist failure is log-only (NOT
            // `emit_persist_failure`). The loop already emitted a
            // terminal `ChatEvent::Error` from the per-event arm
            // at line ~598; emitting a second Error here would be
            // a conflicting double-terminal event. The pattern
            // mirrors the cancel path's synthetic tool_result
            // persist (log-only, see below at the `if cancelled`
            // block).
            if let Err(e) = crate::db::persist_turn(
                &db,
                &session_id,
                msg.role,
                &msg.content,
                seq,
                Some(&turn_latency),
            )
            .await
            {
                if had_error {
                    tracing::error!(
                        error = %e,
                        request_id = %rid,
                        "failed to persist errored partial assistant turn (log-only — Error already emitted)"
                    );
                    return;
                } else {
                    emit_persist_failure(&sink, &rid, &e);
                    return;
                }
            }
            // TurnComplete fires on the success path for every
            // mode (normal / cancel / error). The error path's
            // TurnComplete coexists with the pre-emit Error event
            // (RULE-A-007 decision C): Error = "something went
            // wrong", TurnComplete = "this seq's partial turn is
            // now in the DB + here's the latency breakdown". The
            // controller routes each event independently.
            emit_chat_event_via_sink(
                &sink,
                &rid,
                &ChatEvent::TurnComplete {
                    seq,
                    ttfb_ms: turn_latency.ttfb_ms,
                    gen_ms: turn_latency.gen_ms,
                    total_ms: turn_latency.total_ms,
                    thinking_ms: turn_latency.thinking_ms,
                },
            );
            messages.push(msg);
            seq += 1;
        }

        if cancelled {
            if !tool_calls.is_empty() {
                let tool_result_msg = build_synthetic_tool_result_message(&tool_calls);
                // RULE-A-003 (2026-06-15): cancel path — log-only,
                // NOT emit_persist_failure. The loop is about to
                // emit its terminal cancelled `Done`; an Error
                // here would be a second terminal event conflicting
                // with it. The user already knows they cancelled.
                if let Err(e) = crate::db::persist_turn(
                    &db,
                    &session_id,
                    tool_result_msg.role,
                    &tool_result_msg.content,
                    seq,
                    None,
                )
                .await
                {
                    tracing::error!(error = %e, "failed to persist synthetic tool_result turn after cancel");
                }
                messages.push(tool_result_msg);
            }
            persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
            let _ = crate::db::touch_session(&db, &session_id).await;
            emit_chat_event_via_sink(
                &sink,
                &rid,
                &ChatEvent::Done {
                    stop_reason: Some("cancelled".to_string()),
                    usage: None,
                },
            );
            return;
        }

        // RULE-A-007 (2026-06-17): the error path persisted its
        // partial assistant turn above (with ERROR_MARKER + a
        // TurnComplete event). The loop has already emitted its
        // terminal `ChatEvent::Error` from the per-event arm;
        // emitting another terminal `Done` would conflict. Exit
        // without further tool execution / next-turn dispatch —
        // symmetric with the cancel `return` above. The frontend
        // treats the Error event as terminal; no follow-up Done
        // is required.
        if had_error {
            persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
            let _ = crate::db::touch_session(&db, &session_id).await;
            return;
        }

        let should_continue =
            stop_reason.as_deref() == Some("tool_use") && !tool_calls.is_empty();

        if !should_continue {
            persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
            let _ = crate::db::touch_session(&db, &session_id).await;
            emit_chat_event_via_sink(
                &sink,
                &rid,
                &ChatEvent::Done { stop_reason, usage: last_usage },
            );
            return;
        }

        // Execute tools. We intentionally take a simplified
        // permission path for tests: read tools bypass the
        // ask/allow UI, write tools go through the same ⑨ 关
        // check (the test can stub `permissions::check` via
        // the `permission_asks` map being empty — Tier 5
        // default-allow applies to read tools, Tier 3 fires
        // for write tools. Tests that exercise a specific
        // permission denial can pre-populate
        // `permission_asks` with a no-sender entry — the 120s
        // timeout fires and the test exits).
        let mut result_blocks: Vec<ContentBlock> = Vec::new();
        if is_parallel_eligible(&tool_calls, &permission_ctx.cwd) {
            // ---- L2 parallel path (read-only batch) ----
            //
            // All tool_use blocks in this turn are in the
            // {read_file, grep, glob, list_dir, use_skill}
            // whitelist AND every path tool's `path` resolves
            // inside `permission_ctx.cwd` (= session cwd) →
            // run them concurrently via `FuturesUnordered`.
            // `web_fetch` is excluded (Q2) because its Tier 4
            // default is `ask`, which would fire multiple
            // concurrent `permission:ask` modals. Path tools
            // with an out-of-root `path` are also excluded by
            // the same rule (RULE-A-013 follow-up, 2026-06-19)
            // — see `is_parallel_eligible` doc.
            //
            // Permission-silence invariant (Q2 design +
            // RULE-A-013 closure): the concurrent set is
            // ALWAYS silent in every mode —
            //   - `use_skill` is `ToolKind::Other` → Tier 5
            //     default-allow in every mode;
            //   - path tools (`read_file`/`grep`/`glob`/
            //     `list_dir`) with `path` inside the project
            //     root hit Tier 4.1 path-grant or Tier 4.2
            //     inside-root silent Allow in `Edit`/`Yolo`
            //     (and in `Plan` too — `filter_tools_for_mode`
            //     only drops write/edit/shell, so read tools
            //     reach Tier 4 and resolve silently when the
            //     path is inside the project root).
            // The `is_parallel_eligible` predicate guarantees
            // this: a path tool with `path` outside the
            // project root (no `session_tool_permissions`
            // path-glob grant) is pulled back to the serial
            // path, where the existing single-modal UX
            // applies. See DEBT.md RULE-A-013 for the
            // previous open issue now closed.
            //
            // Result ordering: `result_slots` is pre-
            // allocated to the tool_use count and each task
            // writes its block at its OWN index. The LLM
            // context sees tool_results in the SAME order as
            // the tool_use blocks regardless of which task
            // finishes first. `emit_tool_result` fires as
            // each task completes (streaming, matching the
            // serial path's per-iteration emit).
            //
            // Cancel: every task takes `token.clone()` so the
            // execute_tool's `tokio::select!` wrapper cancels
            // each in-flight task independently. RULE-A-004
            // (cancelled tool skips audit) is preserved per-
            // task: a task whose `token.is_cancelled()` is
            // true after execute sets the shared `cancelled`
            // flag and skips the `tool_executed` audit write.
            // Already-completed tasks still get their audit
            // row. The shared flag is read after the join to
            // drive the existing cancel path.
            let n = tool_calls.len();
            let mut result_slots: Vec<Option<ContentBlock>> =
                (0..n).map(|_| None).collect();
            let cancelled_flag = Arc::new(AtomicBool::new(false));
            let mut fu: FuturesUnordered<_> = tool_calls
                .iter()
                .enumerate()
                .map(|(i, (id, name, input))| {
                    let cancelled_flag = cancelled_flag.clone();
                    let sink = sink.clone();
                    let rid = rid.clone();
                    let id = id.clone();
                    let name = name.clone();
                    let input = input.clone();
                    let permission_ctx = permission_ctx.clone();
                    let permission_asks = permission_asks.clone();
                    let db = db.clone();
                    let read_guard = read_guard.clone();
                    let session_id = session_id.clone();
                    let skill_cache = skill_cache.clone();
                    let current_ctx = current_ctx.clone();
                    let token = token.clone();
                    async move {
                        // check + execute live in the SAME task
                        // (Q2 rationale: no ask risk in the
                        // parallel set → no need to split into
                        // a two-phase check-then-execute).
                        let decision = permissions::check(
                            &permission_ctx,
                            &permission_asks,
                            &db,
                            &sink,
                            &name,
                            &input,
                            &id,
                            &token,
                        )
                        .await;
                        if let Decision::Deny { reason, critical: _ } = decision {
                            let envelope = crate::agent::helpers::tool_result_envelope(
                                &reason,
                                &current_ctx.worktree_path,
                            );
                            sink.emit_tool_result(&crate::state::ToolResultPayload {
                                request_id: rid.clone(),
                                tool_use_id: id.clone(),
                                content: envelope.clone(),
                                is_error: true,
                            });
                            return Some((
                                i,
                                ContentBlock::ToolResult {
                                    tool_use_id: id,
                                    content: envelope,
                                    is_error: true,
                                },
                            ));
                        }

                        let tool_exec_start = Instant::now();
                        let (content, is_error, _update, exit_code) =
                            crate::tools::execute_tool(
                                &name,
                                &input,
                                &current_ctx,
                                Some(&read_guard),
                                Some(&session_id),
                                Some(&skill_cache),
                                token.clone(),
                            )
                            .await;
                        let duration_ms = tool_exec_start.elapsed().as_millis();
                        // RULE-A-004 (2026-06-15): a tool cancelled
                        // mid-flight MUST NOT leave a `tool_executed`
                        // audit row. The check + the skip are
                        // back-to-back (no `.await` between them).
                        // We broadcast via the shared AtomicBool so
                        // the main loop flips its local `cancelled`
                        // after the join and drives the existing
                        // cancel path.
                        if token.is_cancelled() {
                            cancelled_flag.store(true, Ordering::SeqCst);
                        } else if let Err(e) = permissions::record_tool_executed_audit(
                            &db,
                            &session_id,
                            &name,
                            &input,
                            duration_ms,
                            exit_code,
                        )
                        .await
                        {
                            tracing::warn!(
                                error = %e,
                                "chat: record_tool_executed_audit failed (non-fatal)"
                            );
                        }
                        // Parallel batch is read-only by
                        // construction (is_parallel_eligible),
                        // so `update.new_cwd` is None for every
                        // task — no `current_ctx.cwd` mutation to
                        // apply. (`use_skill` doesn't cd; only
                        // `shell` does, and shell is excluded.)
                        let envelope_str = crate::agent::helpers::tool_result_envelope(
                            &content,
                            &current_ctx.worktree_path,
                        );
                        sink.emit_tool_result(&crate::state::ToolResultPayload {
                            request_id: rid.clone(),
                            tool_use_id: id.clone(),
                            content: envelope_str.clone(),
                            is_error,
                        });
                        Some((
                            i,
                            ContentBlock::ToolResult {
                                tool_use_id: id,
                                content: envelope_str,
                                is_error,
                            },
                        ))
                    }
                })
                .collect();
            while let Some(maybe_block) = fu.next().await {
                if let Some((i, block)) = maybe_block {
                    result_slots[i] = Some(block);
                }
            }
            // Collapse the slots into ordered result_blocks.
            // Every slot is Some (every task returns a block on
            // every branch — success, deny, cancel-after-execute
            // all emit + return a block); the only way a slot
            // could stay None is if the task panicked, in which
            // case `fu.next()` would have propagated the panic.
            result_blocks = result_slots.into_iter().flatten().collect();
            if cancelled_flag.load(Ordering::SeqCst) {
                cancelled = true;
            }
        } else {
            // ---- Serial path (write / shell / web_fetch /
            //       update_checklist / mixed batch) ----
            // Unchanged from pre-L2 behavior. Any batch that
            // contains a tool outside the read-only whitelist
            // falls back here; web_fetch is excluded from the
            // parallel set (Q2) precisely so its Tier 4 ask can
            // fire through the normal single-modal flow.
            for (id, name, input) in &tool_calls {
            // Run the full 5-tier permission check (matches
            // production). Tests that want a clean
            // tool-execute-and-continue path should pre-load
            // an Allow for the test tool into
            // `session_tool_permissions`, or use a read tool
            // that hits Tier 5 default-allow.
            let decision = permissions::check(
                &permission_ctx,
                &permission_asks,
                &db,
                &sink,
                name,
                input,
                id,
                &token,
            )
            .await;
            if let Decision::Deny { reason, critical: _ } = decision {
                let envelope = crate::agent::helpers::tool_result_envelope(
                    &reason,
                    &current_ctx.worktree_path,
                );
                sink.emit_tool_result(&crate::state::ToolResultPayload {
                    request_id: rid.clone(),
                    tool_use_id: id.clone(),
                    content: envelope.clone(),
                    is_error: true,
                });
                result_blocks.push(ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content: envelope,
                    is_error: true,
                });
                continue;
            }

            let tool_exec_start = Instant::now();
            let (content, is_error, update, exit_code) = crate::tools::execute_tool(
                name,
                input,
                &current_ctx,
                Some(&read_guard),
                Some(&session_id),
                Some(&skill_cache),
                token.clone(),
            )
            .await;
            let duration_ms = tool_exec_start.elapsed().as_millis();
            // RULE-A-004 (2026-06-15): audit AFTER the cancel
            // check. Previously `record_tool_executed_audit` ran
            // before the `token.is_cancelled()` test, so a tool
            // whose execution was interrupted by a cancel (token
            // fired during `execute_tool`) still got a
            // `tool_executed` audit row — lying to the audit log
            // (the tool did not complete from the user's intent;
            // they hit Stop). Now a cancelled-in-flight tool is
            // marked `cancelled` and skipped for auditing. The two
            // checks are back-to-back with no `.await` between
            // them, so the token state is identical across both.
            if token.is_cancelled() {
                cancelled = true;
            } else if let Err(e) = permissions::record_tool_executed_audit(
                &db,
                &session_id,
                name,
                input,
                duration_ms,
                exit_code,
            )
            .await
            {
                tracing::warn!(error = %e, "chat: record_tool_executed_audit failed (non-fatal)");
            }
            if let Some(new_cwd) = update.new_cwd.clone() {
                current_ctx.cwd = new_cwd.clone();
                last_cwd = Some(new_cwd);
            }
            let envelope_str =
                crate::agent::helpers::tool_result_envelope(&content, &current_ctx.worktree_path);
            sink.emit_tool_result(&crate::state::ToolResultPayload {
                request_id: rid.clone(),
                tool_use_id: id.clone(),
                content: envelope_str.clone(),
                is_error,
            });
            result_blocks.push(ContentBlock::ToolResult {
                tool_use_id: id.clone(),
                content: envelope_str,
                is_error,
            });
            if cancelled {
                break;
            }
            }
        }

        if cancelled {
            let result_count = result_blocks.len();
            if !result_blocks.is_empty() {
                let tool_result_msg = ChatMessage {
                    role: Role::User,
                    content: MessageContent::Blocks(result_blocks),
                };
                // RULE-A-003 (2026-06-15): cancel path — log-only
                // (see the synthetic tool_result site above for why
                // this stays tracing-only instead of emit_persist_failure).
                if let Err(e) = crate::db::persist_turn(
                    &db,
                    &session_id,
                    tool_result_msg.role,
                    &tool_result_msg.content,
                    seq,
                    None,
                )
                .await
                {
                    tracing::error!(error = %e, "failed to persist cancelled tool_result turn");
                }
                messages.push(tool_result_msg);
                tracing::info!(
                    request_id = %rid,
                    tool_results = result_count,
                    "chat_loop: cancelled during tool execution — persisted partial results"
                );
            }
            persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
            let _ = crate::db::touch_session(&db, &session_id).await;
            emit_chat_event_via_sink(
                &sink,
                &rid,
                &ChatEvent::Done {
                    stop_reason: Some("cancelled".to_string()),
                    usage: None,
                },
            );
            return;
        }

        let tool_result_msg = ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(result_blocks),
        };
        // RULE-A-003 (2026-06-15): tool_result persist failure →
        // emit Error + abort. Previously silent + `seq += 1` drift;
        // the next turn's LLM context would otherwise be built on a
        // tool_result the DB never recorded.
        if let Err(e) = crate::db::persist_turn(
            &db,
            &session_id,
            tool_result_msg.role,
            &tool_result_msg.content,
            seq,
            None,
        )
        .await
        {
            emit_persist_failure(&sink, &rid, &e);
            return;
        }
        messages.push(tool_result_msg);
        seq += 1;
    }

    tracing::warn!(max_turns = MAX_TURNS, "agent loop: max turns reached");
    persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
    let _ = crate::db::touch_session(&db, &session_id).await;
    emit_chat_event_via_sink(
        &sink,
        &rid,
        &ChatEvent::Done {
            stop_reason: Some("max_turns".to_string()),
            usage: None,
        },
    );
}

/// F5 per-turn latency helper — builds a [`crate::db::MessageLatency`]
/// from the 5 per-turn `Instant` baselines the agent loop tracks
/// (`send_at` / `first_delta_at` / `thinking_start` /
/// `thinking_done` / `done_at`). `ttfb_ms` / `gen_ms` /
/// `total_ms` / `thinking_ms` are independently `None` when the
/// corresponding boundary wasn't reached — e.g. a turn that
/// emitted `tool_call` straight from `thinking_delta` with no
/// text delta has `ttfb_ms = None` and `gen_ms = None`, but
/// `total_ms` and `thinking_ms` are set.
///
/// Used by `run_chat_loop` (now the production + test entry
/// point) right before the
/// `persist_turn(latency: Some(&MessageLatency))` call (the 4
/// columns go into the same INSERT) and again (with the same
/// values) when emitting `ChatEvent::TurnComplete` to the
/// frontend.
fn build_turn_latency(
    turn_send_at: Option<Instant>,
    turn_first_delta_at: Option<Instant>,
    turn_thinking_start: Option<Instant>,
    turn_thinking_done: Option<Instant>,
    turn_done_at: Option<Instant>,
) -> crate::db::MessageLatency {
    crate::db::MessageLatency {
        ttfb_ms: instant_delta_ms(turn_send_at, turn_first_delta_at),
        gen_ms: instant_delta_ms(turn_first_delta_at, turn_done_at),
        total_ms: instant_delta_ms(turn_send_at, turn_done_at),
        thinking_ms: instant_delta_ms(turn_thinking_start, turn_thinking_done),
    }
}

fn instant_delta_ms(start: Option<Instant>, end: Option<Instant>) -> Option<i64> {
    match (start, end) {
        (Some(s), Some(e)) => {
            let d = e.saturating_duration_since(s);
            Some(i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        }
        _ => None,
    }
}

/// RULE-A-003 (2026-06-15): `persist_turn` failure is no longer
/// silent. On the **normal** persist sites (initial user message,
/// assistant turn, tool_result turn) a failure now emits a typed
/// `ChatEvent::Error { Server }` so the frontend surfaces it —
/// disk-full / DB-lock contention would otherwise leave the next
/// session reload blank (the message was rendered to the user but
/// never reached the DB). The caller then `return`s, matching
/// RULE-A-002's `StillOver` pattern (data-integrity failure →
/// emit Error + terminate the loop).
///
/// The **cancel-path** persist sites (synthetic tool_result after
/// cancel, cancelled tool_result turn) intentionally do NOT call
/// this — they stay `tracing::error!`-only so the loop still emits
/// its single terminal cancelled `Done` event instead of two
/// terminal events (Error + Done) that would conflict.
fn emit_persist_failure(sink: &Arc<dyn ChatEventSink>, rid: &str, err: &sqlx::Error) {
    tracing::error!(error = %err, "agent loop: persist_turn failed");
    sink.emit_chat_event(&crate::state::ChatEventPayload {
        request_id: rid.to_string(),
        event: ChatEvent::Error {
            message: format!(
                "保存对话记录失败(可能磁盘满或数据库被占用),请重试。详情: {}",
                err
            ),
            category: LlmErrorCategory::Server,
        },
    });
}

async fn load_for_session(
    cache: &Arc<MemoryCache>,
    project_id: &str,
    project_path: &str,
) -> Vec<crate::memory::MemoryLayer> {
    crate::memory::loader::load_for_session(cache, project_id, project_path).await
}

/// L2 (2026-06-19): decide whether a single turn's `tool_use`
/// batch is eligible for concurrent execution.
///
/// The whole batch runs concurrently iff **every** tool_use name
/// is in the read-only silent-allow set
/// `{read_file, grep, glob, list_dir, use_skill}`. Any other
/// tool (write_file / edit_file / shell / web_fetch /
/// update_checklist / future tools) → fall back to the serial
/// path with identical pre-L2 behavior.
///
/// Why a whole-batch predicate (not per-tool dispatch)?
/// - **Q1**: zero dependency analysis. A mixed batch (read +
///   write) is conservatively serialized. Per-tool dispatch
///   would require a write-conflict detector (same-file
///   read+edit, etc.) which is out of scope for MVP.
/// - **Q2**: `web_fetch` is excluded even though it's
///   technically read-only — its Tier 4 default is `ask`, and
///   parallel-modal is an unsolved UX (multiple concurrent
///   `permission:ask` events from the same turn). Letting it
///   go serial preserves the single-modal flow.
/// - **`update_checklist`** is excluded by Q1's "writing to
///   agent-managed state" categorization (it mutates the per-
///   request checklist handle); even though the mutation is
///   atomic (Mutex), serializing keeps the audit order
///   predictable.
///
/// **RULE-A-013 follow-up (2026-06-19)**: in addition to the
/// name whitelist, the predicate also rejects any path tool
/// (`read_file` / `grep` / `glob` / `list_dir`) whose `path`
/// argument resolves to **outside** `root`. A path tool with
/// `path` outside the project root would fall through
/// `permissions::check` Tier 4.1 to `ask_path` (no
/// `session_tool_permissions` path-grant hits), and a parallel
/// batch would emit multiple concurrent `permission:ask`
/// modals. The fix is **plan (a)** from DEBT RULE-A-013: push
/// the path check into the predicate so the silent-allow
/// invariant ("the concurrent set is ALWAYS silent") is
/// absolute, not just in the common case. `use_skill` is
/// exempt from the path check (no `path` arg, `ToolKind::Other`
/// → Tier 5 default-allow).
///
/// Path resolution mirrors `agent/permissions/mod.rs:560-571`:
/// absolute `path` is taken as-is; relative `path` is joined
/// onto `root` (the `permission_ctx.cwd`, which equals the
/// session cwd at L2 batch entry — L2 is read-only, no
/// per-batch cwd change). A missing / empty `path` is treated
/// as eligible (the tool layer's schema validation is the
/// fallback; mirroring the permission layer's "no path → Allow"
/// convention). The check delegates to
/// `projects::boundary::is_within_root` (non-failing boolean,
/// already 8-case covered in its unit tests) — we do NOT
/// duplicate the lexical-normalize / parent-walk logic here.
///
/// The empty-batch case: `should_continue` is only true when
/// `!tool_calls.is_empty()` (chat_loop.rs above), so this
/// function is never called with an empty slice in production.
/// Returns `false` defensively for the empty case (the serial
/// path's `for` loop is a no-op anyway).
pub(crate) fn is_parallel_eligible(
    tool_calls: &[(String, String, serde_json::Value)],
    root: &Path,
) -> bool {
    /// Tool names that **always** qualify (name-only check).
    /// `use_skill` has no `path` arg and is exempt from the
    /// path check below.
    const NAME_ELIGIBLE: &[&str] = &["read_file", "grep", "glob", "list_dir", "use_skill"];
    /// Path-bearing tools that get the extra `is_within_root`
    /// check. `use_skill` is intentionally NOT in this list.
    const PATH_TOOLS: &[&str] = &["read_file", "grep", "glob", "list_dir"];

    if tool_calls.is_empty() {
        return false;
    }
    for (_, name, input) in tool_calls {
        if !NAME_ELIGIBLE.contains(&name.as_str()) {
            return false;
        }
        if PATH_TOOLS.contains(&name.as_str()) {
            // Mirror permissions/mod.rs:560-571 path resolution.
            // None / empty path → treat as eligible (tool layer
            // validates; permission layer also tolerates no-path).
            if let Some(p) = input.get("path").and_then(|v| v.as_str()) {
                if !p.is_empty() {
                    let abs = if Path::new(p).is_absolute() {
                        PathBuf::from(p)
                    } else {
                        root.join(p)
                    };
                    if !is_within_root(root, &abs) {
                        return false;
                    }
                }
            }
        }
    }
    true
}
