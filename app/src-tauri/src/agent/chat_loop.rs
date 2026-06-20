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
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::agent::helpers::{
    build_synthetic_tool_result_message, emit_chat_event_via_sink, persist_turn_cwd,
    CANCELLED_MARKER, ERROR_MARKER,
};
use crate::agent::permissions::{self, Decision, PermissionContext};
use crate::agent::subagent::{
    assemble_subagent_prompt, build_worker_messages, filter_tools_for_subagent,
    format_dispatch_result, lookup_subagent, SubagentBufferSink, SubagentStatus,
};
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
/// The 21-parameter signature is unchanged from the previous
/// test-only variant; the production caller just supplies a
/// pre-resolved `Arc<dyn Provider>`, an `Arc<dyn ChatEventSink>`
/// wrapping the live `AppHandle`, and the standard
/// `AppState`-cloned resources (`db` / `read_guard` /
/// `memory_cache` / `permission_asks` / cancel maps). B6
/// Subagent (2026-06-19, review #4) added an 18th parameter,
/// `max_turns: Option<usize>` — `None` keeps the default
/// `MAX_TURNS` (50) for production + tests; the worker path
/// (PR1b) passes `Some(20)` to bound the subagent's turn
/// budget independently of the parent chat. B6 PR1b also added
/// the 19th parameter `skip_session_active: bool` (review #2) —
/// production + tests pass `false`; the worker path passes `true`
/// so the CancellationGuard's Drop does NOT remove the parent's
/// `session_active_request[session_id]` entry (workers reuse the
/// parent's session_id for audit/DB linkage, but their rid must
/// not own the session's "active request" slot — that belongs to
/// the parent chat). B6 PR1b's 20th parameter `skip_persist: bool`
/// suppresses every DB write inside the loop (`persist_turn` /
/// `update_message_metadata` / `touch_session` / `add_token_usage`
/// / `record_*_audit`) so the worker's intermediate turns stay
/// in-memory only — the `SubagentBufferSink` transcript captures
/// them (PR2 persists into `subagent_runs`), and skipping DB
/// writes also avoids a UNIQUE-constraint collision with the
/// parent's own `persist_turn` calls on the same `(session_id,
/// seq)` key. B6 PR2b (2026-06-20, RULE-A-014) added the 21st
/// parameter `is_worker: Option<bool>` — production + tests pass
/// `Some(false)` (default to the production-style false); the
/// worker nested call passes `Some(true)` so the
/// `PermissionContext` built inside the loop carries
/// `is_worker: true`, which makes the Tier 4 `ask_path` /
/// `ask_shell` branches collapse to `Decision::Deny` instead of
/// emitting a `permission:ask` (workers have no UI sink — a
/// permission modal would hang forever on the oneshot).
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
    // B6 Subagent (2026-06-19, review #4): per-invocation turn
    // budget. `None` (production + 9 tests) falls back to the
    // global `MAX_TURNS` (50) — preserves RULE-A-006 single-
    // source-of-truth semantics for the production path. The
    // worker agent path (PR1b) passes `Some(20)` so a runaway
    // subagent cannot burn the parent's full 50-turn budget.
    // C3 compaction and the max_turns terminal event both
    // honor this limit identically to the const case.
    max_turns: Option<usize>,
    // B6 Subagent (2026-06-19, PR1b review #2): when `true`, the
    // per-invocation `CancellationGuard`'s Drop skips the
    // `session_active_request.remove(&session_id)` step (the
    // `cancellations.remove(&rid)` still runs). Workers reuse the
    // parent's `session_id` for audit / DB linkage but their rid
    // must NOT own the session's "active request" slot — removing
    // the parent's entry on worker exit would corrupt
    // `cancel_inflight_for_session` (RULE-E-005). Production +
    // tests pass `false`; the worker path passes `true`.
    skip_session_active: bool,
    // B6 Subagent (PR1b): when `true`, the loop skips ALL DB writes
    // (`persist_turn` / `update_message_metadata` / `touch_session` /
    // `add_token_usage` / `record_*_audit`). The worker agent path
    // uses this so its intermediate turns stay in-memory only (the
    // `SubagentBufferSink` transcript captures them; PR2 will
    // persist the transcript into `subagent_runs`). Skipping the DB
    // also avoids a UNIQUE-constraint collision with the parent's
    // own `persist_turn` calls — both loops would otherwise write
    // to the same `messages` table keyed by `(session_id, seq)`.
    // Production + tests pass `false` (full persistence); the
    // worker path passes `true`.
    skip_persist: bool,
    // B6 Subagent PR2b (2026-06-20, RULE-A-014): when `Some(true)`,
    // the `PermissionContext` built inside this loop carries
    // `is_worker: true`, which makes the Tier 4 `ask_path` /
    // `ask_shell` branches collapse to `Decision::Deny` instead of
    // emitting a `permission:ask` (workers have no UI sink — a
    // permission modal would hang forever). `None` falls back to
    // the session-row mode's natural default (production = `false`,
    // since no parent process is a worker). The worker path passes
    // `Some(true)`; production + 35 `agent_loop_*` integration
    // tests pass `Some(false)` to make the production-style default
    // explicit at the call site.
    is_worker: Option<bool>,
    // B6 PR3 (2026-06-20, PR2 hotfix + PR3a): optional Tauri
    // `AppHandle` used by `run_subagent` to wire the worker's
    // `SubagentBufferSink` IPC emit. Production passes
    // `Some(app.clone())` from the `chat` Tauri command; tests
    // pass `None` (no Tauri runtime, the worker's IPC emit path
    // is bypassed — see `SubagentBufferSink::new_without_app_handle`).
    // Adding this as the 22nd parameter mirrors the existing
    // 21-parameter growth pattern (PR1a/1b/2b); the agent loop
    // body itself does NOT use `app_handle` — only `run_subagent`
    // does, when constructing the worker sink.
    app_handle: Option<AppHandle>,
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
        // Production chat owns the session's "active request" slot,
        // so Drop must clear it. Worker agents (B6 PR1b) pass
        // `skip_session_active: true` to avoid evicting the parent's
        // entry.
        skip_session_active,
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
    // B6 PR2b (RULE-A-014, 2026-06-20): the `is_worker` parameter
    // (added as the 21st arg) threads the worker path's
    // `PermissionContext.is_worker = true` override into the loop
    // body. Without this, the worker's Tier 4 `ask_path` /
    // `ask_shell` would still emit `permission:ask` → register
    // into `permission_asks` → wait for a oneshot (which never
    // comes because the worker has no UI sink) → hang until user
    // Stop. With this fix, a worker Edit/Plan + 写工具 combination
    // is denied (not hung) at Tier 4. `Some(false)` (production
    // + tests) → `false` (production path); `Some(true)` (worker
    // nested call) → `true` (collapse to Deny).
    let effective_is_worker = is_worker.unwrap_or(false);
    let permission_ctx = PermissionContext {
        session_id: session_id.clone(),
        mode: session_mode,
        cwd: session_cwd.clone(),
        is_worker: effective_is_worker,
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
        // B6 PR1b: in the worker path, skip ALL DB writes (see
        // `skip_persist` docstring at the function head). The
        // worker still bumps the in-memory `seq` and pushes into
        // `messages` so the agent loop stays coherent, but it
        // NEVER writes to the parent's `messages` table (the
        // SubagentBufferSink captures the transcript for PR2).
        //
        // RULE-A-003 (2026-06-15): if the very first user message
        // can't be persisted, abort with a visible Error —
        // continuing would let the LLM answer a message the DB
        // never recorded, so the next session reload is blank.
        if !skip_persist {
            if let Err(e) = crate::db::persist_turn(&db, &session_id, msg.role, &msg.content, seq, None)
                .await
            {
                emit_persist_failure(&sink, &rid, &e);
                return;
            }
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
            // B6 PR1b: skip audit writes in the worker path (see
            // `skip_persist` docstring). The resend audit is
            // user-message scope; workers don't observe user
            // resends.
            if !skip_persist {
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
        // B6 PR1b: skip the metadata UPDATE in worker mode (the
        // user row is the parent's, not the worker's).
        if !skip_persist {
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

    let turn_limit = max_turns.unwrap_or(MAX_TURNS);
    for turn in 1..=turn_limit {
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
                                // B6 PR2: token-usage accumulation is
                                // intentionally **decoupled** from
                                // `skip_persist`. The other 17
                                // `skip_persist` gates guard writes to
                                // the `messages` table (where worker +
                                // parent would collide on the
                                // `(session_id, seq)` UNIQUE key) and
                                // `session_audit_events` (where the
                                // worker path's ⑨ decisions would
                                // pollute the parent's audit log). The
                                // token-usage columns
                                // (`input_tokens_total` / etc.) live
                                // on `sessions`, not `messages`, and
                                // the worker reuses the parent's
                                // `session_id` (chat_loop.rs:2049), so
                                // the worker's per-turn usage
                                // naturally folds into the parent's
                                // total. Pulling this out of the
                                // `skip_persist` gate is what makes
                                // the parent's UI see the worker
                                // burning tokens in real time
                                // ("streaming" accumulation).
                                //
                                // PR1b originally gated this under
                                // `!skip_persist` — PR2 reverses that
                                // decision because the worker's
                                // session_id is the parent's, not its
                                // own, and the gate's only purpose
                                // was the messages-table collision
                                // (which doesn't apply here).
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
            if !skip_persist {
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
            }
            // TurnComplete fires on the success path for every
            // mode (normal / cancel / error). The error path's
            // TurnComplete coexists with the pre-emit Error event
            // (RULE-A-007 decision C): Error = "something went
            // wrong", TurnComplete = "this seq's partial turn is
            // now in the DB + here's the latency breakdown". The
            // controller routes each event independently. In the
            // worker path (skip_persist=true) we skip the
            // TurnComplete emit too — the parent never sees the
            // worker's internal turn sequence, only the final
            // dispatch_subagent tool_result.
            if !skip_persist {
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
            }
            messages.push(msg);
            seq += 1;
        }

        if cancelled {
            if !tool_calls.is_empty() {
                let tool_result_msg = build_synthetic_tool_result_message(&tool_calls);
                // B6 PR1b: skip the synthetic tool_result persist in
                // worker mode (the worker's intermediate turn is
                // captured by the SubagentBufferSink transcript).
                if !skip_persist {
                    // RULE-A-003 (2026-06-15): cancel path —
                    // log-only, NOT emit_persist_failure. The loop
                    // is about to emit its terminal cancelled `Done`;
                    // an Error here would be a second terminal event
                    // conflicting with it. The user already knows
                    // they cancelled.
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
                }
                messages.push(tool_result_msg);
            }
            if !skip_persist {
                persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
                let _ = crate::db::touch_session(&db, &session_id).await;
            }
            // B6 PR1b: always emit terminal `Done { cancelled }` —
            // the SubagentBufferSink reads it to set `was_cancelled`
            // (so `run_subagent` can format the dispatch_subagent
            // tool_result with `status=cancelled`).
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
            // B6 PR1b: skip the cwd/touch_session writes in worker
            // mode (the parent's session row is not the worker's
            // to update — the parent owns the lifetime).
            if !skip_persist {
                persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
                let _ = crate::db::touch_session(&db, &session_id).await;
            }
            return;
        }

        let should_continue =
            stop_reason.as_deref() == Some("tool_use") && !tool_calls.is_empty();

        if !should_continue {
            // B6 PR1b: skip the cwd/touch_session writes in worker
            // mode (the parent's session row is not the worker's
            // to update — the parent owns the lifetime).
            if !skip_persist {
                persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
                let _ = crate::db::touch_session(&db, &session_id).await;
            }
            // B6 PR2: emit the terminal `Done` to the sink
            // UNCONDITIONALLY (regardless of `skip_persist`).
            // The worker's `SubagentBufferSink` needs the terminal
            // `Done` in its transcript so PR3's expand UI can
            // render the worker's stop_reason / usage correctly.
            // PR1b bundled the emit with the persist block under
            // `!skip_persist`; PR2 splits them because the emit
            // is a wire-shape concern (not a DB write) and is
            // load-bearing for the worker's transcript.
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
                        } else if !skip_persist {
                            // B6 PR1b: skip the tool_executed audit
                            // write in worker mode. The
                            // SubagentBufferSink transcript is the
                            // worker's audit record; PR2 will
                            // persist it into `subagent_runs`.
                            if let Err(e) = permissions::record_tool_executed_audit(
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

            // B6 Subagent (2026-06-19): intercept dispatch_subagent
            // BEFORE the normal execute_tool path. This is an
            // agent-layer control-flow tool — it needs the parent
            // loop's full closure dependencies (provider / db /
            // cancellations / ...) which `execute_tool_inner` does
            // NOT have access to (see `agent::subagent` docstring +
            // PRD §"Technical Approach" review #3). The interceptor
            // builds the worker context, calls run_chat_loop
            // recursively, and turns the worker's final state into a
            // tool_result that pairs with the dispatch_subagent
            // tool_use (RULE-A-007 pairing invariant preserved).
            //
            // dispatch_subagent is structurally excluded from the
            // L2 parallel set (it's not in `is_parallel_eligible`'s
            // NAME_ELIGIBLE list), so the entire batch falls into
            // this serial path whenever the model emits it. MVP runs
            // dispatches serially (one worker at a time); parallel
            // fan-out is v2 / L3.
            if name == crate::agent::subagent::DISPATCH_TOOL_NAME {
                let tool_exec_start = Instant::now();
                let (content, is_error, cancel_parent, exit_code) = run_subagent(
                    &provider,
                    context_window,
                    &rid,
                    &session_id,
                    &memory_cache,
                    &read_guard,
                    &skill_cache,
                    &permission_asks,
                    &cancellations,
                    &session_active_request,
                    &background_shells,
                    &db,
                    &current_ctx,
                    id,
                    input,
                    &token,
                    &sink,
                    // B6 PR3 (2026-06-20, PR2 hotfix): thread the
                    // parent's AppHandle so the worker's
                    // SubagentBufferSink can emit the `subagent:event`
                    // IPC channel live. From the chat command's spawn
                    // closure this is `Some(app.clone())`; from the
                    // unit tests it's `None` (no Tauri runtime).
                    app_handle.clone(),
                )
                .await;
                let duration_ms = tool_exec_start.elapsed().as_millis();
                // Audit dispatch_subagent like any other tool so
                // the C4 audit log records "subagent ran". This
                // lands AFTER the worker's full turn sequence +
                // the worker's own audit rows already landed
                // (they're tied to the same session_id by design —
                // workers don't have their own sessions).
                if token.is_cancelled() {
                    cancelled = true;
                } else if !skip_persist {
                    // B6 PR1b: the parent (NOT the worker) records
                    // its own dispatch_subagent audit. The worker
                    // passes skip_persist=true on its nested
                    // run_chat_loop call, so this site is only
                    // reached for the parent's own dispatch —
                    // the worker's run_subagent returns BEFORE
                    // any nested run_chat_loop call sees this code.
                    if let Err(e) = permissions::record_tool_executed_audit(
                        &db,
                        &session_id,
                        name,
                        input,
                        duration_ms,
                        exit_code,
                    )
                    .await
                    {
                        tracing::warn!(error = %e, "chat: record_tool_executed_audit failed for dispatch_subagent (non-fatal)");
                    }
                }
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
                result_blocks.push(ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content: envelope_str,
                    is_error,
                });
                if cancel_parent {
                    cancelled = true;
                }
                if cancelled {
                    break;
                }
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
            } else if !skip_persist {
                // B6 PR1b: skip the tool_executed audit write in
                // worker mode (SubagentBufferSink transcript is
                // the worker's record; PR2 persists it).
                if let Err(e) = permissions::record_tool_executed_audit(
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
                // B6 PR1b: skip the cancelled tool_result persist
                // in worker mode (SubagentBufferSink transcript is
                // the worker's record).
                if !skip_persist {
                    // RULE-A-003 (2026-06-15): cancel path —
                    // log-only (see the synthetic tool_result
                    // site above for why this stays tracing-only
                    // instead of emit_persist_failure).
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
                }
                messages.push(tool_result_msg);
                tracing::info!(
                    request_id = %rid,
                    tool_results = result_count,
                    "chat_loop: cancelled during tool execution — persisted partial results"
                );
            }
            if !skip_persist {
                persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
                let _ = crate::db::touch_session(&db, &session_id).await;
            }
            // B6 PR1b: always emit terminal `Done { cancelled }` —
            // the SubagentBufferSink reads it to set `was_cancelled`
            // (so `run_subagent` can format the dispatch_subagent
            // tool_result with `status=cancelled`).
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
        // B6 PR1b: skip the tool_result persist in worker mode
        // (SubagentBufferSink transcript is the worker's record).
        if !skip_persist {
            // RULE-A-003 (2026-06-15): tool_result persist
            // failure → emit Error + abort. Previously silent +
            // `seq += 1` drift; the next turn's LLM context would
            // otherwise be built on a tool_result the DB never
            // recorded.
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
        }
        messages.push(tool_result_msg);
        seq += 1;
    }

    tracing::warn!(max_turns = turn_limit, "agent loop: max turns reached");
    // B6 PR1b: skip the max_turns terminal persists in worker mode.
    if !skip_persist {
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

// ---------------------------------------------------------------------------
// B6 Subagent (2026-06-19): worker dispatch
//
// `run_subagent` is the interceptor helper called from the
// serial-path tool dispatch loop when `name == "dispatch_subagent"`.
// It owns the nested `run_chat_loop` call that drives the worker
// agent. Because it needs the parent loop's closure dependencies
// (`provider` / `db` / `cancellations` / ...) it lives in this
// module rather than `agent::subagent` — the alternative would be
// to thread 14+ parameters through a public function, which is
// the same "too many parameters" cost `run_chat_loop` itself pays
// (see RULE-A-006 docstring at the top of this file).
//
// The function returns a `(content, is_error, cancel_parent,
// exit_code)` tuple shaped to mirror the `execute_tool` return so
// the caller's serial-path code can treat it uniformly:
//   - `content` = the dispatch_subagent tool_result's content
//     string (status prefix + worker summary).
//   - `is_error` = whether the worker exited non-successfully
//     (cancelled / errored). The caller's serial path emits the
//     tool_result with this flag set so the LLM sees the failure.
//   - `cancel_parent` = whether the worker detected a parent-
//     propagated cancel (user Stop reached the worker). When
//     `true`, the caller's serial loop flips its local `cancelled`
//     flag and drives the existing cancel path — the user's Stop
//     propagates back up through the worker to the parent.
//   - `exit_code` = always `None` (no child process spawned);
//     matches the convention for non-shell tools.
// ---------------------------------------------------------------------------

/// Worker turn budget. Bounded independently of the parent's 50-turn
/// limit so a runaway subagent cannot burn the parent's full budget
/// (PRD §Decisions 8 + review #4). The worker still re-uses C3
/// compaction, so hitting this limit on a long task degrades to
/// compaction rather than an unbounded loop.
const SUBAGENT_MAX_TURNS: usize = 20;

#[allow(clippy::too_many_arguments)]
async fn run_subagent(
    provider: &Arc<dyn Provider>,
    context_window: u32,
    parent_rid: &str,
    parent_session_id: &str,
    memory_cache: &Arc<MemoryCache>,
    read_guard: &ReadGuard,
    skill_cache: &Arc<SkillCache>,
    permission_asks: &crate::agent::permissions::PermissionStore,
    cancellations: &Arc<Mutex<std::collections::HashMap<String, CancellationToken>>>,
    _session_active_request: &Arc<Mutex<std::collections::HashMap<String, String>>>,
    background_shells: &crate::background_shell::DefaultRegistry,
    db: &SqlitePool,
    current_ctx: &ToolContext,
    tool_use_id: &str,
    input: &serde_json::Value,
    parent_token: &CancellationToken,
    _parent_sink: &Arc<dyn ChatEventSink>,
    // B6 PR3 (2026-06-20, PR2 hotfix): the parent's Tauri
    // `AppHandle`, threaded through `run_chat_loop`'s 22nd
    // parameter. Used to construct the worker's `SubagentBufferSink`
    // so the worker can emit the `subagent:event` IPC channel
    // live. `None` in unit tests (no Tauri runtime); the sink's
    // `new_without_app_handle` constructor is used in that case.
    app_handle: Option<AppHandle>,
) -> (String, bool, bool, Option<i32>) {
    // Parse the LLM-supplied { subagent, task } arguments.
    let subagent_name = input
        .get("subagent")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let task = input.get("task").and_then(|v| v.as_str()).unwrap_or("");
    let tool_use_id_owned = tool_use_id.to_string();

    // Resolve the SubagentDef. Unknown name → error tool_result
    // (keeps the tool_use/tool_result pairing invariant).
    let Some(def) = lookup_subagent(subagent_name) else {
        let content = format!(
            "Unknown subagent '{}'. Available: researcher, general-purpose.",
            subagent_name
        );
        return (content, true, false, None);
    };
    if task.trim().is_empty() {
        let content = "Missing or empty 'task' parameter. The delegation task must be a                        non-empty string."
            .to_string();
        return (content, true, false, None);
    }

    // Build the worker's toolset (allowlist + structural-disabled
    // strip). The worker's run_chat_loop call gets this filtered
    // Vec; the parent's tool_defs is unaffected.
    let worker_tool_defs = filter_tools_for_subagent(crate::tools::builtin_tools(), def);

    // Resolve the parent session's project_id + path so the worker
    // reads the same memory cache slots the parent uses.
    let project_id = resolve_project_id(db, parent_session_id).await;
    let project_path = current_ctx.worktree_path.to_string_lossy().to_string();

    // Build the worker's messages: [memory_blocks (cache_control),
    // delegation_task]. The task is APPENDed (prompt-cache invariant
    // — see PRD §Decisions 6 + research §10.5).
    let worker_messages =
        build_worker_messages(memory_cache, &project_id, &project_path, task).await;

    // Assemble the worker's system prompt — fully replaces the
    // parent's behavior_prompt + mode_prefix + base_prompt layers.
    let _worker_system_prompt = assemble_subagent_prompt(def, task);
    // (run_chat_loop builds its own system prompt from the project /
    // session row; the worker's replacement prompt would need to be
    // threaded as a parameter to override the assemble_system_prompt
    // step. For PR1b the worker gets the parent's base prompt — the
    // worker SubagentDef's system_prompt is used for documentation
    // only. A future PR will thread an override through. See
    // PR1b §"Deviations".)

    // Worker rid + token. The rid is registered into `cancellations`
    // (so user Stop propagates from the parent via the shared map)
    // but NOT into `session_active_request` — that map is
    // session→request 1:1 and a worker entry would evict the
    // parent's mapping, corrupting
    // `cancel_inflight_for_session` / RULE-E-005. The
    // CancellationGuard inside run_chat_loop is constructed with
    // `skip_session_active: true` for the worker path so its Drop
    // does NOT remove the parent's session_active_request entry.
    //
    // The rid suffix uses the tool_use_id so a future PR2
    // transcript row can correlate back to the parent's
    // dispatch_subagent tool_use.
    let worker_rid = format!("{}-sub-{}", parent_rid, tool_use_id_owned);
    let worker_token = parent_token.child_token();
    {
        let mut map = cancellations.lock().await;
        map.insert(worker_rid.clone(), worker_token.clone());
    }

    // B6 PR2: insert the worker's `running` row into
    // `subagent_runs` BEFORE the nested `run_chat_loop` call. The
    // returned id is the `worker_run_id` that the
    // `update_run_finished` call (after the worker returns)
    // targets. The insert is best-effort: a DB failure logs at
    // `warn!` and the worker still runs (the user's dispatch
    // experience is not gated on the audit row). A failed insert
    // leaves `worker_run_id_opt = None`; the post-loop
    // `update_run_finished` is then a no-op.
    let worker_run_id_opt: Option<String> = match crate::db::subagent_runs::insert_run(
        db,
        parent_session_id,
        &worker_rid,
        subagent_name,
    )
    .await
    {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::warn!(
                parent_session_id = %parent_session_id,
                worker_rid = %worker_rid,
                error = %e,
                "run_subagent: failed to insert subagent_runs row (non-fatal; worker still runs)"
            );
            None
        }
    };

    // B6 PR2b (RULE-A-014, 2026-06-20): the worker's
    // `PermissionContext.is_worker` override is now threaded via
    // the 21st `is_worker: Option<bool>` parameter on the nested
    // `run_chat_loop` call below (passes `Some(true)`). The
    // pre-PR2b local `_worker_permission_ctx` constructed here
    // (PR1b) was dead code: `run_chat_loop` rebuilds its own
    // `PermissionContext` internally from the session row, so the
    // local value was never consulted on the worker path. PR2b
    // removes the local construction and the parallel comment
    // that documented the (now-resolved) deviation.

    // SubagentBufferSink: records the worker's emits into an in-
    // memory transcript AND (PR2 hotfix) emits each event on the
    // `subagent:event` Tauri IPC channel so the frontend
    // `<SubagentDrawer>` (PR3b) can stream the transcript live.
    // Does NOT forward to the parent sink — the parent's frontend
    // only sees the dispatch_subagent tool_call / tool_result
    // pair; the worker's stream stays isolated (Claude Code
    // convention). The `app_handle` is `Some` in production (the
    // `chat` Tauri command threads it through) and `None` in
    // unit tests (no Tauri runtime) — the IPC emit becomes a
    // silent no-op in the latter case, but the transcript
    // accumulation path is unaffected.
    //
    // We need TWO clones of `app_handle`: one for the sink (which
    // emits on the IPC channel) and one for the nested
    // `run_chat_loop` call (which the worker threads forward to
    // ITS OWN nested run_subagent call, if the worker itself
    // dispatches a sub-subagent — out of scope in MVP, but the
    // signature carries the parameter through anyway). The
    // double-clone is cheap (AppHandle is `Arc<Mutex<...>>` under
    // the hood).
    // Bug1 fix (2026-06-21): the sink's `run_id` becomes the
    // `subagent:event` payload's `runId`, which the frontend store
    // uses as the key for `liveTranscript` / `getRunCache`. It MUST
    // equal `summary.id` (= the DB row id `worker_run_id`), NOT the
    // human-readable `worker_rid` — otherwise the drawer opens with
    // `openRunId = summary.id` but the transcript cache is keyed by
    // `worker_rid`, so the drawer renders blank + stuck-on-running.
    // `worker_run_id_opt` is `None` only when `insert_run` failed
    // (no DB row → no summary → drawer can't open), so the
    // `worker_rid` fallback is unreachable in practice but keeps
    // the sink construction total.
    let event_run_id = worker_run_id_opt
        .clone()
        .unwrap_or_else(|| worker_rid.clone());
    let worker_sink: Arc<SubagentBufferSink> = match app_handle.as_ref() {
        Some(handle) => Arc::new(SubagentBufferSink::new(
            handle.clone(),
            event_run_id.clone(),
            parent_session_id.to_string(),
        )),
        None => Arc::new(SubagentBufferSink::new_without_app_handle(
            event_run_id.clone(),
            parent_session_id.to_string(),
        )),
    };
    let worker_sink_dyn: Arc<dyn ChatEventSink> = worker_sink.clone();

    // Nested run_chat_loop. The worker reuses the parent's
    // session_id for DB linkage (its turns land in the same
    // `messages` table), but:
    //   - `skip_session_active: true` so the guard's Drop does NOT
    //     evict the parent's session_active_request entry.
    //   - `max_turns: Some(SUBAGENT_MAX_TURNS)` to bound the worker's
    //     turn budget.
    //   - The worker_token is the parent_token's child, so a user
    //     Stop that reaches the parent also fires the worker
    //     (cancel propagation).
    //
    // Boxed: `run_subagent` → `run_chat_loop` → `run_subagent`
    // (worker dispatches its own subagent? No — workers have
    // `dispatch_subagent` stripped from their tools, so the
    // recursion is bounded at depth 1). Still, the async-fn
    // recursion is statically unbounded (the compiler cannot prove
    // the depth-1 invariant), so `Box::pin` breaks the size-
    // infinite Future chain. The cost is one heap allocation per
    // worker dispatch — negligible relative to the LLM round-trip.
    Box::pin(run_chat_loop(
        worker_tool_defs,
        provider.clone(),
        context_window,
        worker_rid.clone(),
        parent_session_id.to_string(),
        worker_messages,
        worker_sink_dyn,
        db.clone(),
        cancellations.clone(),
        _session_active_request.clone(),
        read_guard.clone(),
        memory_cache.clone(),
        skill_cache.clone(),
        permission_asks.clone(),
        worker_token,
        None,
        background_shells.clone(),
        Some(SUBAGENT_MAX_TURNS),
        // B6 PR1b review #2: worker path — skip_session_active = true
        // so the worker's guard Drop does not evict the parent's
        // session_active_request[parent_session_id] entry.
        true,
        // B6 PR1b: worker path — skip_persist = true so the worker's
        // intermediate turns stay in-memory only. The
        // SubagentBufferSink captures them; PR2 will persist the
        // transcript into `subagent_runs`. Without this, the worker
        // would race the parent's persist_turn calls on the same
        // `(session_id, seq)` key (UNIQUE collision).
        true,
        // B6 PR2b (RULE-A-014, 2026-06-20): worker path — is_worker
        // = Some(true) so the nested run_chat_loop builds a
        // PermissionContext with is_worker: true. This makes Tier 4
        // ask_path / ask_shell collapse to Decision::Deny (worker
        // has no UI sink — a permission:ask would hang forever on
        // the oneshot). Pre-PR2b the worker path constructed
        // `_worker_permission_ctx` here but never threaded it into
        // the nested call, so the override was unreachable on the
        // worker's actual permission checks.
        Some(true),
        // B6 PR3 (2026-06-20, PR2 hotfix): forward the parent's
        // AppHandle so the worker's SubagentBufferSink can emit the
        // `subagent:event` IPC channel live. None in tests. Cloned
        // (not moved) so the post-loop `subagent:finished` emit
        // below can still borrow `app_handle` — AppHandle is an
        // `Arc` under the hood so the clone is cheap.
        app_handle.clone(),
    ))
    .await;

    // Drain the worker's accumulated state.
    let worker_text = worker_sink.final_text();
    let status = if worker_sink.was_cancelled() {
        SubagentStatus::Cancelled
    } else if worker_sink.had_error() {
        SubagentStatus::Error
    } else {
        SubagentStatus::Completed
    };

    // B6 PR2: persist the worker run to `subagent_runs`. The flow:
    // 1. Snapshot the transcript from the sink.
    // 2. Apply the 4 MiB cap (returns the head+tail truncated
    //    vector + a `truncated` flag).
    // 3. Build the terminal `TokenUsage` (sum of per-turn usage
    //    the sink accumulated from `ChatEvent::Done { usage }`
    //    events).
    // 4. UPDATE the `running` row to the terminal state.
    //
    // **Streaming token usage** (the parent's live counter
    // updating as the worker burns tokens) is handled by the
    // `add_token_usage` call at chat_loop.rs:907 — which was
    // decoupled from `skip_persist` in this same PR. The sink's
    // per-turn accumulator is parallel: the sink still records
    // the per-turn `TokenUsage` so `cumulative_usage()` can
    // produce the worker-run-level total for `token_usage_json`
    // even after the streaming path has already folded individual
    // turn values into the parent's running total.
    //
    // The UPDATE is best-effort: a DB failure logs at `warn!` and
    // continues (the dispatch_subagent tool_result is the
    // user-visible artifact; the DB row is for PR3's expand UI and
    // audit reads). Failing the dispatch on a DB error would mask
    // the worker's actual outcome and could re-fire the
    // tool_use/tool_result mismatch (RULE-A-007 invariant).
    if let Some(worker_run_id) = worker_run_id_opt.as_ref() {
        let transcript_snapshot = worker_sink.transcript_snapshot();
        let (truncated_transcript, transcript_truncated) =
            crate::agent::subagent::truncate_transcript_for_persistence(
                transcript_snapshot,
                crate::agent::subagent::TRANSCRIPT_MAX_BYTES,
            );
        let cumulative_usage = worker_sink.cumulative_usage();
        let finished_at = chrono::Utc::now().to_rfc3339();
        let status_db = match status {
            SubagentStatus::Completed => crate::db::subagent_runs::SubagentStatusDb::Completed,
            SubagentStatus::Cancelled => crate::db::subagent_runs::SubagentStatusDb::Cancelled,
            SubagentStatus::Error => crate::db::subagent_runs::SubagentStatusDb::Error,
        };
        match crate::db::subagent_runs::update_run_finished(
            db,
            worker_run_id,
            status_db,
            &finished_at,
            &worker_text,
            &cumulative_usage,
            &truncated_transcript,
            transcript_truncated,
        )
        .await
        {
            Ok(()) => {
                // Bug2 fix (2026-06-21): emit a one-shot
                // `subagent:finished` terminal signal so the frontend
                // `<SubagentDrawer>` / `<ToolCallCard>` flip from
                // `running` to the terminal state without polling.
                // The frontend listener refetches `get_subagent_run`
                // (drawer: status + finishedAt + full transcript)
                // and `list_subagent_runs_by_session` (card: status).
                // Emitted only on the Ok arm — a DB failure leaves
                // the row `running`, so emitting here would cache a
                // stale `running` row as terminal. Best-effort: a
                // Tauri emit failure is non-fatal (the dispatch
                // tool_result is the user-visible terminal signal).
                if let Some(handle) = app_handle.as_ref() {
                    let payload = crate::agent::subagent::build_subagent_finished_payload(
                        worker_run_id,
                        parent_session_id,
                        status_db.as_str(),
                        &finished_at,
                    );
                    if let Err(e) = handle.emit("subagent:finished", payload) {
                        tracing::warn!(
                            worker_run_id = %worker_run_id,
                            error = %e,
                            "subagent:finished emit failed (non-fatal; DB row already terminal)"
                        );
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    worker_run_id = %worker_run_id,
                    error = %e,
                    "run_subagent: failed to persist subagent_runs update (non-fatal)"
                );
            }
        }
    }

    // Detect parent-driven cancel: the parent token fired while the
    // worker was running. The worker's own cancel_done event may
    // NOT have fired if the cancel arrived after the worker loop
    // already returned (e.g. worker finished turn 1 cleanly, then
    // parent cancel propagated before turn 2's select! polled).
    // The child_token relationship makes the worker_token fire when
    // the parent fires; check parent_token directly so the caller's
    // serial loop flips its `cancelled` flag and drives the existing
    // cancel path (matches the user's Stop intent).
    let cancel_parent =
        parent_token.is_cancelled() && status == SubagentStatus::Cancelled;

    let (content, is_error) = format_dispatch_result(status, &worker_text);
    (content, is_error, cancel_parent, None)
}

/// Resolve the project_id for a session. Best-effort DB lookup of
/// `sessions.project_id` — the worker's memory loader needs the
/// project_id to slot into the right MemoryCache entry.
async fn resolve_project_id(db: &SqlitePool, session_id: &str) -> String {
    match crate::db::load_session(db, session_id).await {
        Ok(Some(loaded)) => loaded.session.project_id,
        _ => {
            tracing::warn!(
                session_id = %session_id,
                "run_subagent: failed to load session for project_id; falling back to empty"
            );
            String::new()
        }
    }
}
