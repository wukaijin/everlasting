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
use std::pin::Pin;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;
// A5+ (2026-07-04): LLM first-byte-safe retry open.
use crate::llm::retry::{retry_open, OpenOutcome, RetryPolicy, RetrySink, RetryingEvent};

use crate::agent::helpers::{
    build_synthetic_tool_result_message, emit_chat_event_via_sink, persist_turn_cwd,
    CANCELLED_MARKER, ERROR_MARKER,
};
use crate::agent::loop_detection;
use crate::agent::permissions::{self, Decision, PermissionContext};
use crate::agent::subagent::SubagentEventSink;
use crate::agent::thinking::{
    flush_ordered_thinking, flush_pending_text, flush_pending_thinking, PendingThinking,
};
use crate::agent::MAX_TURNS;
use crate::background_shell::BackgroundShellRegistry;
use crate::llm::{
    ChatEvent, ChatMessage, ContentBlock, LlmErrorCategory, MessageContent, Provider, Role, ToolDef,
};
use crate::memory::MemoryCache;
use crate::projects::boundary::is_within_root;
use crate::skill::loader::SkillCache;
use crate::state::{ChatEventSink, ProviderCatalog, ToolCallPayload};
use crate::tools::read_guard::ReadGuard;
use crate::tools::ToolContext;
use std::collections::VecDeque;

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
/// `update_message_metadata` / `touch_session` /
/// `update_last_turn_usage` / `record_*_audit`) so the worker's
/// intermediate turns stay
/// in-memory only — the `SubagentBufferSink` transcript captures
/// them (PR2 persists into `subagent_runs`), and skipping DB
/// writes also avoids a UNIQUE-constraint collision with the
/// parent's own `persist_turn` calls on the same `(session_id,
/// seq)` key. B6 PR2b (2026-06-20, RULE-A-014) added the 21st
/// parameter `is_worker: Option<bool>` — production + tests pass
/// `Some(false)` (default to the production-style false); the
/// worker nested call passes `Some(true)` so the
/// `PermissionContext` built inside the loop carries
/// `is_worker: true`. The 2026-06-22 fix (RULE-FrontSubagent-003)
/// added the 24th parameter `worker_run_id: Option<String>` so
/// `ask_path` can build the worker-owned permission session id
/// (`"worker:<worker_run_id>"`) and populate
/// `PermissionAskPayload.worker_run_id` for frontend routing.
/// Together these two params let worker asks enter the
/// interactive round-trip (`register_ask` + `tokio::select!`)
/// instead of pre-2026-06-22's auto-Deny collapse.
///
/// `run_chat_loop` owns the per-turn `CancellationGuard` that
/// removes the (rid → token) and (session_id → rid) entries on
/// every exit path (normal / error / cancel / max_turns /
/// StillOver). The chat command's pre-flight inserts those
/// entries; the agent loop's own RAII Drop cleans them up.
#[allow(clippy::too_many_arguments)]
/// A5+ (2026-07-04, R8): `RetrySink` adapter that forwards retry
/// notices onto the regular `chat-event` IPC channel as a
/// `ChatEvent::Retrying` payload. The frontend `streamController`
/// routes them through its `case 'retrying'` arm and shows a small
/// "↩ 重试中 N/M, Ts 后重发… (reason)" row above the in-flight
/// assistant bubble — without this the user sees a multi-second
/// frozen-looking stream while the backoff sleeps.
///
/// Holds a clone of the chat sink + the request id so each retry
/// notice maps onto the right active request on the frontend.
struct LlmRetrySink {
    sink: Arc<dyn ChatEventSink>,
    rid: String,
}
impl RetrySink for LlmRetrySink {
    fn emit_retrying(&self, event: RetryingEvent) {
        // Trace-log alongside the IPC emit so server-side debugging
        // can grep retry timing without the frontend open.
        tracing::info!(
            request_id = %self.rid,
            attempt = event.attempt,
            max_attempts = event.max_attempts,
            wait_ms = event.wait_ms,
            reason = %event.reason,
            "chat: LLM retry (A5+)"
        );
        emit_chat_event_via_sink(
            &self.sink,
            &self.rid,
            &ChatEvent::Retrying {
                attempt: event.attempt,
                max_attempts: event.max_attempts,
                wait_ms: event.wait_ms,
                reason: event.reason,
            },
        );
    }
}

/// Does the in-memory tail user message `mem_msg` correspond to an
/// already-persisted DB row `db_row`? Used by `run_chat_loop`'s
/// user-message persist site to skip re-persisting a message that
/// `reload_messages` fed back in (group_chat per-speaker loop), which
/// would otherwise write a duplicate tool_result and trip OpenAI 400
/// "Messages with role 'tool' must be a response to a preceding message
/// with 'tool_calls'".
///
/// Conservative — it is always safe to return `false` (the original
/// persist path runs and at worst writes an idempotent-looking row); it
/// is NEVER safe to wrongly return `true` (a genuine new message would
/// be dropped). Two cases:
///   - tool_result blocks: match by `tool_use_id` (globally unique, the
///     most stable key — content text / is_error could legitimately
///     differ across a re-serialize).
///   - plain text: `to_text()` byte equality.
///
/// The 08-04 rewrite (design.md D-D/D-F) matches against ANY user-role
/// DB row (the caller scans `loaded_session.messages`), NOT a length/
/// tail-alignment criterion — a filtered participant view has fewer
/// rows than the DB, so a length check would always misfire.
fn user_message_matches(db_row: &crate::db::MessageRow, mem_msg: &ChatMessage) -> bool {
    if db_row.role != "user" {
        return false;
    }
    // Collect tool_use_ids from the in-memory message's blocks.
    let mem_ids: Vec<&str> = match &mem_msg.content {
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
                _ => None,
            })
            .collect(),
        MessageContent::Text(_) => Vec::new(),
    };
    if !mem_ids.is_empty() {
        // tool_result: compare tool_use_ids against the DB row's
        // deserialized content. Deserialize failure → no match (safe).
        let db_content: MessageContent = match serde_json::from_value(db_row.content.clone()) {
            Ok(c) => c,
            Err(_) => return false,
        };
        let db_ids: Vec<&str> = match &db_content {
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
                    _ => None,
                })
                .collect(),
            MessageContent::Text(_) => Vec::new(),
        };
        return !mem_ids.is_empty() && mem_ids == db_ids;
    }
    // Plain text user message: byte equality against the DB `text`
    // column. A fresh send's text never equals the prior persisted row.
    mem_msg.content.to_text() == db_row.text
}

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
    // `update_last_turn_usage` / `record_*_audit`). The worker agent path
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
    // `is_worker: true`, which gates `ask_path` into the worker's
    // interactive round-trip branch (the 2026-06-22 fix replaced
    // the pre-fix Tier 4 collapse-to-Deny with a `register_ask` +
    // `tokio::select!{cancel, timeout, oneshot}` flow keyed under
    // the worker-owned permission session id). `None` falls back
    // to the session-row mode's natural default (production =
    // `false`, since no parent process is a worker). The worker
    // path passes `Some(true)`; production + 35 `agent_loop_*`
    // integration tests pass `Some(false)` to make the
    // production-style default explicit at the call site.
    is_worker: Option<bool>,
    // P2.4 C5 (2026-07-22): the worker dispatch context, replacing
    // the `app_handle: Option<AppHandle>` 22nd parameter. The old
    // param served two uses — (1) wiring the worker's
    // `SubagentBufferSink` IPC emit, (2) snapshotting
    // `AppState.catalog` for worker model resolution. Both are now
    // explicit + transport-agnostic so the daemon path (no
    // `AppHandle`) gets them too:
    //   - `worker_catalog`: `Some(state.catalog.clone())` in
    //     production (Tauri + daemon); `None` in tests.
    //   - `worker_event_sink`: `AppHandleSubagentSink` (Tauri IPC),
    //     `HttpSseSubagentSink` (daemon SSE — was buffer-only
    //     pre-C5), `ThreadLocalSubagentSink` (tests).
    // The agent loop body itself does NOT use either — only
    // `run_subagent` does, when constructing the worker sink.
    worker_catalog: Option<Arc<RwLock<ProviderCatalog>>>,
    worker_event_sink: Arc<dyn SubagentEventSink>,
    // 2026-06-21 fix (B6 review defect A): the worker's
    // `assemble_subagent_prompt(def, task)` output was previously
    // dead code (`_worker_system_prompt` discarded at
    // `chat_loop.rs:2052`); the worker actually inherited the
    // parent's `assemble_system_prompt(mode_prefix, base_prompt)`
    // output, which made `SubagentDef.system_prompt` effectively
    // documentation-only and produced prompt/permission
    // contradictions in Edit/Plan mode. The fix threads the
    // worker's overridden prompt as a parameter: when `Some(p)`,
    // the loop uses `p` directly (skipping the parent's
    // `assemble_system_prompt` step). When `None`, the loop
    // builds the prompt from the project + session row (the
    // production + test path). The `run_subagent` worker
    // nested call passes `Some(assemble_subagent_prompt(def,
    // &task))`; the production `chat` command passes `None`.
    // 4 指令文件 prompt caching is unaffected — the 4
    // instructions live in a separate user-role synthetic
    // message with its own `cache_control: Ephemeral`
    // breakpoint (see `build_instructions_blocks`), independent
    // of the system role.
    system_prompt_override: Option<String>,
    // 2026-06-22 (RULE-FrontSubagent-003 fix): the worker's
    // `subagent_runs.id` (DB row UUID, NOT the human-readable
    // `worker_rid`). Threaded into the `PermissionContext` built
    // inside this loop so `ask_path` can:
    //
    // 1. Build the worker-owned permission session id
    //    (`"worker:<worker_run_id>"`) so the oneshot map entry
    //    does not collide with the parent's pending asks.
    // 2. Populate `PermissionAskPayload.worker_run_id` so the
    //    frontend `<SubagentDrawer>` routes the ask to the
    //    correct worker row instead of the global
    //    `<PermissionModal>`.
    //
    // `None` for the parent path (production chat + 35
    // `agent_loop_*` integration tests); `Some(worker_run_id)`
    // for the nested call inside `run_subagent` (B6 PR1a+
    // 2026-06-22 follow-up). The companion `is_worker: Some(true)`
    // gates the worker's `ask_path` branch — this field carries
    // the routing key.
    worker_run_id: Option<String>,
    // L3d (2026-06-25): the process-wide subagent cache. Used by
    // the loop's per-turn tool list construction (line ~957) to
    // append the dynamic `dispatch_subagent` ToolDef via
    // `definition_with_cache(&subagent_cache, project_path)`, and
    // by `run_subagent` to look up the dispatched subagent across
    // builtin + user + project layers (`cache.lookup(project_path,
    // name)` replaces the static `lookup_subagent(name)`).
    //
    // Threaded here (rather than read off `AppState` mid-loop)
    // because the loop's signature already carries every other
    // `Arc<...>` handle (memory_cache / skill_cache / etc.) —
    // uniform treatment keeps the test + production paths
    // shape-identical. The cache is read-through + mtime-fenced
    // so adding / editing / deleting a `.md` is picked up on the
    // next chat turn without a reload command.
    subagent_cache: Arc<crate::agent::subagent::SubagentCache>,
    // 2026-06-26 (task `06-26-subagent-per-run-grant`): per-run
    // in-memory grant cache for worker subagents. `Some(Arc<...>)`
    // on the worker path (the Arc is constructed fresh in
    // `run_subagent` per worker); `None` on the parent path
    // (production chat + tests — never read, never written).
    //
    // Threaded into `PermissionContext.run_grants` so `check.rs`
    // Tier 4's three branches (Path / Shell / WebFetch) can
    // consult the cache before falling through to `ask_path`, and
    // `ask_path`'s worker `AllowAlways` arm can write to it. The
    // cache dies with the worker's `run_chat_loop` invocation —
    // it does NOT persist to `session_tool_permissions`
    // (RULE-A-016 isolation: worker grants must not cross the
    // privilege boundary into the parent session's grant table).
    run_grants: Option<std::sync::Arc<crate::agent::permissions::RunGrantCache>>,
    // L3b (2026-06-27): worker worktree isolation override. When
    // `Some(path)`, the loop uses `path` as the worker's worktree
    // root INSTEAD of the session row's `worktree_path` (which is
    // the PARENT session's worktree — the root cause of worker
    // reuse of the parent's checkout). The path is also the basis
    // for the worker's `cwd` (initialized to the path itself,
    // since a fresh worktree has no `current_cwd`). The path is
    // assumed to be inside the project root (the caller —
    // `run_subagent` — verifies via `assert_within_root` before
    // passing it).
    //
    // When `None`, the loop builds the worktree_path + cwd from
    // the session row as before (the production chat + test path,
    // AND the non-isolated worker path).
    //
    // Mirrors the `system_prompt_override` pattern (override the
    // session-row-derived value at the loop's ToolContext
    // construction site). Production chat + tests pass `None`; the
    // worker path passes `Some(worker_worktree_path)` when
    // isolation is active, `None` otherwise.
    worktree_override: Option<PathBuf>,
    // project_main_override (2026-07-29): the worker's ORIGINAL project
    // main repo path when `worktree_override` is `Some` (i.e. the worker is
    // isolated). Threads into `PermissionContext.project_main_path` so the
    // permission layer's inside-check anchors on the project root, NOT the
    // worker's own checkout subtree. `None` for production chat, non-isolated
    // workers, and tests — in those cases `project_main_path` falls back to
    // `worktree_path` (which IS the project root for them).
    project_main_override: Option<PathBuf>,
    // L3b (2026-06-27): the app's data directory, threaded so the
    // dispatch_subagent interceptor can compute the worker
    // worktree path (`<app_data_dir>/worktrees/<project_uuid>/
    // worker/<run_id>`) when isolation is active. Production
    // threads `AppState.app_data_dir.clone()`; tests pass an
    // empty path (most tests dispatch non-isolated workers or
    // `researcher` which never needs a worktree).
    //
    // This is a pass-through parameter — the agent loop body
    // itself does NOT read it; only `run_subagent` (the
    // dispatch_subagent interceptor) does, when creating the
    // worker worktree.
    app_data_dir: PathBuf,
    // explicit-agent-dispatch (2026-06-30): when `Some(fd)`, the
    // loop's turn-1 prefix short-circuits the LLM — it synthesizes a
    // `dispatch_subagent` tool_use from `fd` and calls `run_subagent`
    // directly (NO `provider.stream`), then emits the worker's summary
    // as the turn's assistant text and exits. `None` = normal LLM-
    // driven loop (production chat without `@@` prefix + worker nested
    // calls + all tests). See `ForcedDispatch` + the prefix block
    // below the user-message persist site.
    forced_dispatch: Option<crate::agent::subagent::ForcedDispatch>,
    // 2026-06-30 (`ask_user_question` task): parallel
    // `QuestionStore` for the blocking reverse-question tool.
    // Threads through so `chat_loop.rs`'s
    // `tool_name == "ask_user_question"` interception can call
    // `ask_user_question::execute_blocking(input, session_id,
    // tool_use_id, &question_store, &sink, &token)`. The
    // production `chat` Tauri command sources it from
    // `AppState.question_store.clone()`; tests pass
    // `h.question_store.clone()` (each test gets a fresh
    // registry). Worker nested calls (via `run_subagent`) carry
    // the parent's — but since `ask_user_question` is in
    // `STRUCTURALLY_DISABLED`, a worker never reaches the
    // intercept, so the store is unused on the worker path.
    // Appended at the tail rather than inserted mid-signature
    // so the 28→29 expansion is one trailing argument — tests
    // need only add a final positional `h.question_store.clone()
    // // 29` (or `None` for legacy 28-arg tests).
    question_store: crate::agent::question_store::QuestionStore,
    // W1 (Workflow integration, Phase 0 Step 0.5 — 2026-07-08):
    // per-session workflow context. `None` for non-workflow
    // sessions (zero overhead — the per-turn loop short-
    // circuits). When `Some(ctx)`, `messages[0]` gets the
    // state breadcrumb + current-task metadata appended on
    // every turn (mirrors `memory_recall`'s per-turn
    // injection seam; same S-B guard — no prepend of
    // synthetic user messages).
    //
    // 29→30 arg. Still appended at the tail per the same
    // convention as `question_store` (keeps existing
    // test fixtures on one-line edits when they upgrade
    // from 29 to 30).
    mut workflow_ctx: Option<crate::agent::workflow::WorkflowCtx>,
    // Group chat (07-29-group-chat): shared turn state for the
    // `nominate_speaker` / `end_discussion` interception. `None`
    // for classic-chat + worker paths (the interception no-ops with
    // an error tool_result if these tools are somehow invoked
    // outside group chat). Appended at the tail per the same
    // convention as `workflow_ctx` (one-line test-fixture edits).
    group_chat_state: Option<crate::tools::nominate_speaker::SharedTurnState>,
    // Group chat (07-29-group-chat, Phase 4 TODO-A): per-turn
    // speaker. `None` for normal chat / subagent / review /
    // moderator-of-self paths; `Some("moderator")` for the
    // moderator turn in group_chat; `Some(participant.name)` for
    // the participant turn. Carried into the assistant persist
    // site (line ~2129) so messages are stored with the
    // originating speaker for frontend speaker-chip rendering +
    // reload consistency. Read-only — never affects tool routing,
    // role mapping, or wire shape (the wire layer operates on
    // `Role::User`/`Role::Assistant` regardless). Live tests
    // (~58 callsites) + 4 production callsites pass `None`; the
    // two `run_group_chat_loop` dispatch sites pass `Some(name)`.
    current_speaker: Option<String>,
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
    // L3b (2026-06-27): when `worktree_override` is `Some(path)`,
    // use the override INSTEAD of the session row's worktree_path.
    // The override is the worker's isolated git worktree (created
    // by `git::worktree::create_worker`); the session row's
    // worktree_path is the PARENT session's worktree, which is the
    // root cause of worker reuse of the parent's checkout. The
    // override path is already asserted to be inside the project
    // root by `run_subagent` before being passed here, so the
    // `assert_within_root` call below still passes (we use the
    // override as both the path AND the canonicalization target).
    let session_root_raw = worktree_override
        .clone()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or(session_root_raw);
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
    // L3b (2026-06-27): a worker worktree is a fresh checkout with
    // no `current_cwd` history — the worker starts at the worktree
    // root, NOT the parent session's `current_cwd` (which would
    // point at a path inside the parent's checkout, not the
    // worker's). The override path wins; non-override path keeps
    // the legacy behavior (read `current_cwd` from the session row).
    let session_cwd_raw = if worktree_override.is_some() {
        worktree_path.to_string_lossy().to_string()
    } else if loaded_session.session.current_cwd.is_empty() {
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
    // project_main_path (2026-07-29): the inside-check anchor for the
    // permission layer. For a non-isolated worker / parent session,
    // `worktree_path` IS the project root → fall back to it. For an
    // isolated worker, `worktree_path` is its checkout subtree and the
    // real project root comes via `project_main_override` (set by
    // `run_subagent` to the project's main repo path). Canonicalize the
    // override to match `is_within_root`'s lexical expectation; if it's
    // missing/invalid (tests, degenerate cases) fall back to worktree_path
    // so behavior matches the old code rather than panicking.
    let project_main_path = match &project_main_override {
        Some(p) if !p.as_os_str().is_empty() => {
            crate::projects::boundary::resolve_path(&p.to_string_lossy(), &worktree_path)
        }
        _ => worktree_path.clone(),
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
        // L3b PR3 (2026-06-27): DB pool for the `merge_worker` /
        // `discard_worker` tools. These tools read the
        // `subagent_runs` row to find the worker worktree path +
        // the parent project root, then call libgit2 to merge /
        // destroy. The pool is `Clone` (Arc-internal) so the
        // per-turn `ToolContext::clone()` pattern is unaffected.
        db: db.clone(),
        // P2 (2026-06-29): the session's `projects.id` UUID. The
        // `remember` tool binds project-scope memories to this id;
        // the session-start recall filters by the same id. Worker
        // subagents reuse the parent's project (their worktree is a
        // checkout OF the parent's project), so the worker path
        // also carries the parent's project_id.
        project_id: project.id.clone(),
        // 06-30 follow-up: pass through the app-global data
        // directory so tool-layer helpers that need to construct
        // absolute paths (e.g. lazy auto-attach on the
        // merge_worker path) can read it from `ToolContext`
        // without changing every tool-execute signature. The
        // value here is identical to `state.app_data_dir` — we
        // clone the `app_data_dir` parameter that's already in
        // scope on the chat_loop function.
        data_dir: app_data_dir.clone(),
        // Step 1.5 (07-08-workflow-integration): propagate the
        // active plugin name so `tools::use_skill` can load
        // plugin-layer skills (e.g. `wf-overview`). `None` for
        // non-workflow sessions — the loader treats that as
        // "no plugin layer, fall through to project/user".
        // `workflow_ctx` is already in scope (declared on the
        // `run_chat_loop` signature at line 406) and is
        // populated by `lib.rs::chat` from the session's
        // `workflow_enabled` toggle + the active plugin.
        workflow_name: workflow_ctx.as_ref().map(|c| c.workflow_def.name.clone()),
    };
    let mut current_ctx = turn_ctx;
    let mut last_cwd: Option<PathBuf> = None;
    // 2026-06-21 (R3): the per-turn `last_usage` is re-declared
    // at the top of each iteration of the `for turn in 1..=turn_limit`
    // loop, so the synthetic `max_turns` terminal site
    // (chat_loop.rs:1797-1820) cannot read it directly. Track
    // the most recent value here at the function scope so the
    // terminal site can forward it to the sink (and the sink
    // can route it into `cumulative_usage()` exactly once, via
    // the R3 stop_reason guard). Pre-R3 the synthetic terminal
    // hard-coded `usage: None`, which produced the
    // `subagent_runs.token_usage_json == 0` regression on
    // `max_turns` exits (c27f3fd7 worker run).
    let mut last_usage_terminal: Option<crate::llm::types::TokenUsage> = None;

    // P4 (2026-06-29, 06-29-am-p4-event-reflect): per-session
    // failure tracker. Created once at the top of `run_chat_loop`
    // and shared (via `Arc`) across the two tool-emit sites —
    // the parallel-batch L2 path's `FuturesUnordered` task AND
    // the serial path's `for (id, name, input) in &tool_calls`
    // loop both feed outcomes into the same tracker. When the
    // "≥2 consecutive failures → success" pattern lands for
    // a tool, the tracker fires a fire-and-forget LLM reflection
    // that produces a `kind=pitfall, status=active` row in
    // `autonomous_memories` — which the P3 pre-tool recall
    // surfaces on the next session (or even later in the same
    // session if a worker re-tries the same operation). v1
    // accepts session-boundary reset (no cross-session carry
    // of "tools that were flaky yesterday"; spike-007 §10
    // extension point).
    let failure_tracker = Arc::new(Mutex::new(crate::agent::auto_reflect::FailureTracker::new()));

    // P5 (2026-06-29, 06-29-am-p5-quality): session-scoped soft-block
    //记账. When a verified pitfall soft-blocks a tool_use, its
    // memory_id lands here; the next hit on the same pitfall degrades
    // to Footnote + normal execution (the dead-loop guard, design D1).
    // Same lifecycle as `failure_tracker` (loop-local, dropped on
    // exit) — no cross-session carry.
    let soft_blocked: Arc<Mutex<std::collections::HashSet<String>>> = Arc::default();

    let session_mode = loaded_session.session.mode;
    // B6 PR2b (RULE-A-014, 2026-06-20): the `is_worker` parameter
    // (added as the 21st arg) threads the worker path's
    // `PermissionContext.is_worker = true` override into the loop
    // body. The 2026-06-22 fix (RULE-FrontSubagent-003) further
    // added `worker_run_id` (the 24th arg) so `ask_path` can route
    // worker asks via a worker-owned permission session id
    // (`"worker:<worker_run_id>"`) + propagate `worker_run_id`
    // into the IPC payload for frontend routing. Pre-fix (PR2b)
    // the worker path collapsed Tier 4 ask_path → Deny (no UI
    // sink — would hang on oneshot); post-fix the worker enters
    // the interactive round-trip and waits for the user. Yolo
    // mode still bypasses the whole Tier 4 above (in `check`),
    // so a worker under Yolo never reaches `ask_path`.
    let effective_is_worker = is_worker.unwrap_or(false);
    let permission_ctx = PermissionContext {
        session_id: session_id.clone(),
        mode: session_mode,
        cwd: session_cwd.clone(),
        is_worker: effective_is_worker,
        // 2026-06-22 (RULE-FrontSubagent-003 fix): carry the
        // worker_run_id through so `ask_path` can build the
        // worker-owned permission session id and propagate the
        // worker_run_id into `PermissionAskPayload.worker_run_id`.
        // `None` for the parent path (production chat + tests);
        // `Some(...)` for the worker nested call. The
        // `effective_is_worker` gate above is the actual
        // "is this a worker?" predicate — this field is just
        // the routing key, used only when `effective_is_worker`
        // is true.
        worker_run_id: worker_run_id.clone(),
        // 2026-06-26 (task 06-26-subagent-per-run-grant): per-run
        // in-memory grant cache. `None` for the parent path
        // (production chat + tests) — the Tier 4 grant-check
        // branches in `check.rs` skip the cache lookup entirely
        // when this is `None`. `Some(Arc<...>)` for the worker
        // path — the Arc is constructed fresh in `run_subagent`
        // per worker, so concurrent workers have isolated caches.
        run_grants: run_grants.clone(),
        // read-side boundary decouple (2026-07-01): deny-list/allow-list
        // 的"项目外"判定锚点(项目根). 见 PermissionContext.worktree_path doc.
        worktree_path: worktree_path.clone(),
        // 2026-07-29: inside-check anchor (项目根). 隔离 worker 的
        // worktree_path 指向 checkout 子树,不能作锚点 —— 用真实项目根.
        project_main_path: project_main_path.clone(),
        // E2 trace: per-turn seq, updated at the top of each turn
        // before the tool-execution phase. None at construction
        // (pre-turn-loop); the turn loop sets Some(seq) per turn.
        turn_seq: None,
    };
    let mut permission_ctx = permission_ctx;
    let mode_prefix = permissions::mode_system_prefix(session_mode);

    // B6+ B (task 07-06-b6plus-b-dispatch-model-arg): snapshot the
    // model list once per `run_chat_loop` invocation to build the
    // dynamic `model` enum on the `dispatch_subagent` tool schema
    // (display_name values — the system prompt does not list models,
    // so the enum is the LLM's only discovery channel). Placed here
    // (after `effective_is_worker` is known and `db` is in scope)
    // but still OUTSIDE the turn loop (1511), so it runs once per
    // chat invocation regardless of turn count. The worker path
    // snapshots too (harmless — `definition_with_cache` is gated on
    // `effective_is_worker == false` below, so the worker never
    // consumes the snapshot). Models change at low frequency; CRUD
    // during a session is reflected next session and covered by the
    // catalog-miss fallback in `resolve_worker_provider`.
    let model_briefs: Vec<crate::agent::subagent::ModelBrief> =
        match crate::db::list_models(&db).await {
            Ok(rows) => rows
                .into_iter()
                .map(|mwp| crate::agent::subagent::ModelBrief {
                    id: mwp.model.id,
                    display_name: mwp.model.display_name,
                })
                .collect(),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "list_models snapshot failed; dispatch_subagent `model` enum will be empty"
                );
                vec![]
            }
        };

    // B5 memory is empty in tests (no memory files written to the
    // temp project dir). Skip the synthetic user/assistant
    // inserts when `load_for_session` returns no layers.
    let memory_layers = load_for_session(&memory_cache, &project.id, &project.path).await;
    let instructions_blocks = crate::memory::loader::build_instructions_blocks(&memory_layers);
    let has_memory = !instructions_blocks.is_empty();
    if !instructions_blocks.is_empty() {
        messages.insert(
            0,
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(instructions_blocks),
                speaker: None,
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
                speaker: None,
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
    //
    // Step 1.1 (07-08-workflow-integration): when the session is a
    // workflow session, also consult the plugin layer (highest
    // precedence: plugin > project > user). The plugin layer reads
    // `<project>/.everlasting/workflow/<name>/skills/` and silently
    // falls through to project / user when the plugin dir is absent
    // — non-workflow callers get the old project-overrides-user
    // behavior byte-identical (see skill::loader::merge_skill_layers).
    let skill_listing_path = worktree_path.to_string_lossy().to_string();
    let skill_wf_name = workflow_ctx
        .as_ref()
        .map(|ctx| ctx.workflow_def.name.clone());
    let skill_infos = crate::skill::loader::list_skill_infos_with_workflow(
        &skill_cache,
        Some(&skill_listing_path),
        skill_wf_name.as_deref(),
    )
    .await;
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
                speaker: None,
            },
        );
    }

    // P2 RULE-A-005 (2026-06-24, fix 1 of 3 P2 open rules):
    // `head_sha` is now MUTABLE and refreshed at the start of every
    // turn (before `provider.send`) so the LLM sees the current HEAD
    // after a mid-session commit. Pre-fix: `head_sha` was a one-shot
    // `let` at chat_loop.rs:492 — the 50-turn loop sent a stale SHA
    // for every turn after turn 1, drifting the LLM's mental model of
    // the repo state. The cost is one extra `lookup_head_sha` (libgit2
    // `Repository::open` + `head().peel_to_commit()`) per turn —
    // negligible relative to LLM network latency.
    //
    // Cache-correctness (RULE-A-005 invariant, verified in
    // prd §6.1): the head_sha field lives inside `build_system_prompt`
    // output, which is fed into the provider's **system** role string.
    // The 4 instruction files (User/Project × CLAUDE.md/AGENTS.md)
    // are injected as a SEPARATE user-role synthetic message via
    // `memory::loader::build_instructions_blocks` and carry their own
    // `cache_control: Ephemeral` breakpoint — independent of the
    // system role. So a per-turn system-prompt mutation does NOT
    // bust the memory cache. The 4 instruction blocks stay cache-hot
    // across the 50-turn loop.
    let mut head_sha = crate::agent::system_prompt::lookup_head_sha(&worktree_path);
    // The 2026-06-21 B6 review defect A fix (the worker's
    // `SubagentDef.system_prompt` override via the 23rd parameter)
    // short-circuits below — when `Some(p)`, the worker uses `p`
    // directly and never calls `assemble_system_prompt` or
    // `build_system_prompt`. The production + 35 test path passes
    // `None`, so this branch runs on every parent turn.
    let mut system_prompt = match system_prompt_override {
        Some(ref p) => p.clone(),
        None => {
            let base_prompt = crate::agent::system_prompt::build_system_prompt(
                &loaded_session.session,
                &project,
                &worktree_path,
                &head_sha,
            );
            crate::agent::system_prompt::assemble_system_prompt(mode_prefix, &base_prompt)
        }
    };

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
    //
    // Group chat (08-04 dedup rewrite, design.md D-D): detect when the
    // tail user message is ALREADY in the DB (happens in group_chat's
    // per-speaker loop, where `reload_messages` feeds an already-
    // persisted tool_result — stored as role=user — back in as the
    // tail). Blindly re-persisting it wrote a duplicate tool_result row
    // with no matching tool_calls, which OpenAI rejects with HTTP 400
    // "Messages with role 'tool' must be a response to a preceding
    // message with 'tool_calls'", erroring every subsequent turn. When
    // the tail user message is already in the DB, skip the persist +
    // resend audit + seq bump, and point `user_seq` at the existing row
    // so the FileInjections metadata update below still targets the
    // right row. Normal single-agent chat is unaffected: its tail is a
    // freshly-composed user message that does NOT match any DB row, so
    // this guard returns false and the original path runs verbatim.
    //
    // The guard replaces the pre-08-04 heuristic (design D-F): the old
    // version required `messages.len() == loaded_session.messages.len()`,
    // which is always false once the memory/skill injection pass above
    // inserts synthetic user rows into `messages` — and it was NOT
    // scoped to `group_chat_state`, so it could never fire at all. The
    // new version (a) requires `group_chat_state.is_some()` (a group-
    // chat speaker — ordinary chat never enters), and (b) content-matches
    // the tail user message against ANY user-role row in the loaded
    // session (not just the tail / not length-based), so a filtered
    // participant view (fewer rows than the DB) is still matched
    // correctly.
    //
    // Known cosmetic boundary (design §5): a human re-sending the EXACT
    // same text in a group chat (cancel + resend identical text) is
    // judged already-persisted and skipped → the transcript loses one
    // text row. Harmless: it does not break tool pairing and never
    // triggers a 400.
    let (last_user_snapshot, last_user_seq) =
        if let Some(last_user) = messages.iter().rev().find(|m| m.role == Role::User) {
            let msg = last_user.clone();
            let already_in_db = (!skip_persist)
                .then(|| {
                    loaded_session
                        .messages
                        .iter()
                        .filter(|m| m.role == "user")
                        .find(|db_row| user_message_matches(db_row, &msg))
                })
                .flatten()
                .filter(|_| group_chat_state.is_some())
                .map(|db_row| db_row.seq);
            if let Some(existing_seq) = already_in_db {
                // Already persisted (group_chat reload path): do not
                // write a new row, do not bump seq. `user_seq` points
                // at the existing row so FileInjections metadata
                // update + memory injection target the right message.
                let user_seq = existing_seq;
                (Some(msg.content), user_seq)
            } else {
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
                    if let Err(e) = crate::db::persist_turn(
                        &db,
                        &session_id,
                        msg.role,
                        &msg.content,
                        seq,
                        None,
                        msg.speaker.as_deref(),
                    )
                    .await
                    {
                        emit_persist_failure(&sink, &rid, &e);
                        return;
                    }
                }
                // D3 PR3 (2026-06-17): if the user hit Resend (instead of
                // Edit), the frontend passed `resend_seq` through the chat
                // IPC. Fire the `resend_message` audit row pointing at
                // the original user message's seq (the one the user clicked
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
                            None,
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
            }
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
            if let Err(e) =
                crate::db::update_message_metadata(&db, &session_id, last_user_seq, &meta).await
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

    // -----------------------------------------------------------------
    // explicit-agent-dispatch (2026-06-30): forced dispatch prefix.
    //
    // When the user typed `@@<agent> <task>`, the frontend parsed it
    // into `forced_dispatch` and threaded it here. We short-circuit
    // the LLM entirely — NO `provider.stream` — and dispatch the named
    // worker directly via the SAME `run_subagent` call the LLM-driven
    // interceptor uses at chat_loop.rs:2376. The worker's summary
    // becomes this turn's assistant text. Forced dispatch runs exactly
    // one turn (no follow-up LLM loop) — the user already decided
    // which agent runs; the main agent is never asked to judge.
    // -----------------------------------------------------------------
    if let Some(fd) = &forced_dispatch {
        // `forced_{rid}-{seq}` is unique within the session (rid is
        // per-request, seq is per-turn) and the `forced_` prefix
        // namespaces it away from any LLM-emitted tool_use id.
        let tool_use_id = format!("forced_{}-{}", rid, seq);
        let mut input = serde_json::json!({
            "subagent": fd.subagent,
            "task": fd.task,
        });
        // B6+ B: thread the per-dispatch model override (resolved to
        // an id by the frontend's `resolveModelInput`) into the
        // synthesized tool_use's input so `run_subagent` picks it up
        // via `input.get("model")` — same code path as the LLM-driven
        // dispatch (`resolve_model_by_name_or_id` accepts the id).
        if let Some(mid) = &fd.model_id {
            input["model"] = serde_json::Value::String(mid.clone());
        }
        let dispatch_name = crate::agent::subagent::DISPATCH_TOOL_NAME;

        // ① open the assistant message + emit the synthetic
        //    dispatch_subagent tool_use so the UI renders the same
        //    tool card + opens the SubagentDrawer.
        emit_chat_event_via_sink(&sink, &rid, &ChatEvent::Start);
        sink.emit_tool_call(&ToolCallPayload {
            request_id: rid.clone(),
            id: tool_use_id.clone(),
            name: dispatch_name.to_string(),
            input: input.clone(),
        });

        // ② run the worker. Parameters mirror the LLM-driven
        //    interceptor at chat_loop.rs:2376-2419 verbatim
        //    (force_readonly=false, parallel=false → single serial
        //    dispatch; isolation falls back to the subagent's
        //    frontmatter default via `resolve_isolation`).
        let tool_exec_start = std::time::Instant::now();
        let (content, is_error, cancel_parent, exit_code) =
            crate::agent::subagent::dispatch::run_subagent(
                &provider,
                worker_catalog.clone(),
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
                &tool_use_id,
                &input,
                &token,
                &sink,
                worker_event_sink.clone(),
                false,
                &subagent_cache,
                &app_data_dir,
                false,
                // 2026-06-30 (`ask_user_question`): thread the
                // parent's QuestionStore. Worker can't reach it.
                &question_store,
                // W1 (Workflow integration, Step 2.4 — 2026-07-08):
                // workflow role-gate enforcement. Pass the
                // session's WorkflowCtx (None for non-workflow
                // sessions → gate short-circuits → legacy
                // dispatch shape preserved).
                workflow_ctx.as_ref(),
            )
            .await;
        let duration_ms = tool_exec_start.elapsed().as_millis();

        // ③ cancel propagation — user Stop reached the worker, or the
        //    worker detected a parent-propagated cancel.
        let cancelled = token.is_cancelled() || cancel_parent;

        // ④ audit. The parent records its own dispatch_subagent audit
        //    (mirrors chat_loop.rs:2448; skipped on cancel + worker
        //    skip_persist paths).
        if !cancelled && !skip_persist {
            if let Err(e) = crate::agent::permissions::record_tool_executed_audit(
                &db,
                &session_id,
                dispatch_name,
                &input,
                duration_ms,
                exit_code,
                Some(seq),
            )
            .await
            {
                tracing::warn!(
                    error = %e,
                    request_id = %rid,
                    "chat_loop: record_tool_executed_audit failed for forced dispatch (non-fatal)"
                );
            }
        }

        // ⑤ emit the dispatch_subagent tool_result (worker summary).
        let envelope_str =
            crate::agent::helpers::tool_result_envelope(&content, &current_ctx.worktree_path);
        sink.emit_tool_result(&crate::state::ToolResultPayload {
            request_id: rid.clone(),
            tool_use_id: tool_use_id.clone(),
            content: envelope_str,
            is_error,
        });

        // ⑥ the worker summary is also this turn's assistant text —
        //    emit it as a Delta so the frontend renders an assistant
        //    message carrying the result (the SubagentDrawer already
        //    showed the live worker output; this lands the summary in
        //    the main conversation).
        emit_chat_event_via_sink(
            &sink,
            &rid,
            &ChatEvent::Delta {
                text: content.clone(),
            },
        );

        // ⑦ persist the assistant turn: Blocks = [synthetic
        //    ToolUse(dispatch), Text(summary)]. The ToolUse block keeps
        //    a reload self-consistent (the assistant turn visibly
        //    performed the dispatch). No separate tool_result message
        //    row — the worker's result lives in `subagent_runs` + the
        //    summary text; a stray tool_result row without a following
        //    LLM turn would orphan it.
        let assistant_msg = ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![
                ContentBlock::ToolUse {
                    id: tool_use_id.clone(),
                    name: dispatch_name.to_string(),
                    input: input.clone(),
                },
                ContentBlock::Text {
                    text: content.clone(),
                    cache_control: None,
                },
            ]),
            speaker: None,
        };
        if !skip_persist {
            if let Err(e) = crate::db::persist_turn(
                &db,
                &session_id,
                assistant_msg.role,
                &assistant_msg.content,
                seq,
                None,
                assistant_msg.speaker.as_deref(),
            )
            .await
            {
                emit_persist_failure(&sink, &rid, &e);
                return;
            }
            emit_chat_event_via_sink(
                &sink,
                &rid,
                &ChatEvent::TurnComplete {
                    seq,
                    ttfb_ms: Some(0),
                    gen_ms: Some(0),
                    total_ms: Some(duration_ms as i64),
                    thinking_ms: Some(0),
                },
            );
            let _ = crate::db::touch_session(&db, &session_id).await;
        }
        messages.push(assistant_msg);
        // (No `seq += 1` — forced dispatch returns immediately, so
        // the bump is never read again. The assistant row's seq was
        // already consumed by `persist_turn` above.)

        // ⑧ terminal Done. cancelled → "cancelled"; else "end_turn".
        emit_chat_event_via_sink(
            &sink,
            &rid,
            &ChatEvent::Done {
                stop_reason: Some(if cancelled { "cancelled" } else { "end_turn" }.to_string()),
                usage: None,
            },
        );
        return;
    }

    // ⑬ loop detection (C2): sliding window of recent tool calls,
    // checked once per turn after tool_calls are collected. Declared
    // OUTSIDE the turn loop so it accumulates across turns — and
    // since B6 worker subagents reuse `run_chat_loop`, the worker
    // inherits detection too (with its own shorter max_turns budget).
    let mut loop_window: VecDeque<loop_detection::ToolCall> =
        VecDeque::with_capacity(loop_detection::SOFT_WINDOW);
    // C2+ (2026-07-05): per-`run_chat_loop`-local consecutive-hit
    // counter for the active-intervention state machine. Lives next
    // to `loop_window` so it accumulates across turns the same way
    // (and the worker inherits it via its nested `run_chat_loop`
    // call, with its own independent count). Reset to 0 on any
    // `LoopVerdict::None` turn (consecutiveness is the signal);
    // incremented on any `HardLoop` / `SoftLoop` verdict; triggers
    // active intervention (QuestionStore ask) at >= 3.
    let mut loop_hit_count: u32 = 0;

    let turn_limit = max_turns.unwrap_or(MAX_TURNS);
    for turn in 1..=turn_limit {
        // E2 trace (2026-07-14): update the per-turn seq on the
        // permission context so `record_audit` can pass it to
        // `record_audit_event` for audit turn alignment.
        permission_ctx.turn_seq = Some(seq);
        // P2 RULE-A-005 (2026-06-24, fix 1 of 3 P2 open rules):
        // refresh `head_sha` + rebuild `system_prompt` at the start of
        // EVERY turn. The LLM only consumes `system_prompt` once per
        // turn (at `provider.send`), so refreshing at turn entry is
        // equivalent to "after every tool execute" — the next
        // `provider.send` (this turn, or the next turn's) sees the
        // current HEAD. Pre-fix: `head_sha` was a one-shot `let` at
        // chat_loop.rs:492 (pre-fix line number), so the LLM saw a
        // stale SHA on turn 2+ even after a tool call committed. The
        // `system_prompt_override` worker path is unchanged: when the
        // 23rd param is `Some(p)`, the worker's
        // `SubagentDef.system_prompt` is the canonical prompt and the
        // parent's per-turn rebuild is skipped (workers don't observe
        // the parent's HEAD anyway — the worker's own lookup is
        // handled inside its nested `run_chat_loop` invocation).
        //
        // Cost: 1 extra `lookup_head_sha` per turn (libgit2
        // `Repository::open` + `head().peel_to_commit()` —
        // sub-millisecond for a local repo, negligible relative to
        // LLM network latency). Memory cache is NOT busted — the
        // instructions blocks live in a separate user-role synthetic
        // message with their own `cache_control: Ephemeral`
        // breakpoint (see prd §6.1 + the `build_instructions_blocks`
        // docstring in `memory/loader.rs`).
        if system_prompt_override.as_ref().is_none() {
            head_sha = crate::agent::system_prompt::lookup_head_sha(&worktree_path);
            let base_prompt = crate::agent::system_prompt::build_system_prompt(
                &loaded_session.session,
                &project,
                &worktree_path,
                &head_sha,
            );
            system_prompt =
                crate::agent::system_prompt::assemble_system_prompt(mode_prefix, &base_prompt);
        }

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
            // E2 trace (2026-07-14): record C3 compaction observation
            // (both normal compaction + StillOver error). Always-on
            // emit + persist; best-effort on the DB write.
            if compacted.dropped_count > 0
                || matches!(
                    compacted.degradation,
                    crate::agent::context::DegradationKind::StillOver { .. }
                )
            {
                crate::agent::trace::record_compaction(
                    &sink,
                    &db,
                    &rid,
                    &session_id,
                    seq,
                    &compacted,
                )
                .await;
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

        let mut turn_first_delta_at: Option<Instant> = None;
        let mut turn_thinking_start: Option<Instant> = None;
        let mut turn_thinking_done: Option<Instant> = None;
        let mut turn_done_at: Option<Instant> = None;

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
        let background_notifications = background_shells.drain_notifications(&session_id).await;
        let turn_messages = {
            let checklist_snapshot = current_ctx.checklist.lock().await.clone();
            let mut req = messages.clone();
            if !checklist_snapshot.is_empty() {
                let block = crate::tools::update_checklist::render_checklist(&checklist_snapshot);
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
                    speaker: None,
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
                    speaker: None,
                };
                req.push(msg);
            }
            // P2 (2026-06-29): autonomous-memory session-start
            // recall. Per-turn (PRD decision 6): query = the most
            // recent user message text. The recall text is
            // appended to the instruction message's block list
            // (messages[0]) in the REQUEST clone — the persisted
            // `messages` Vec is byte-identical across turns (the
            // recall block is per-turn-only, like the B12
            // checklist). The banner + instruction-body prefix
            // stays stable so the Anthropic cache window stays
            // warm. See `agent::memory_recall` for the cache-
            // correctness rationale + the candidate-inclusion
            // ADR-lite (P5 tightens back to active/verified).
            //
            // Skip when `skip_persist` (worker path): the worker
            // reuses the parent's session_id, and surfacing the
            // parent's memories in the worker's context would
            // (a) confuse the worker's focused task and (b) bump
            // hit_count on rows the worker didn't actually
            // contribute to. The worker has its own context.
            if !skip_persist {
                let query = messages
                    .iter()
                    .rev()
                    .find(|m| m.role == Role::User)
                    .map(|m| m.content.to_text())
                    .unwrap_or_default();
                if !query.trim().is_empty() {
                    // 07-06 (am-observability-panel R2b / A8): use
                    // the rows-aware sibling so we can emit a
                    // `ChatEvent::Recall` after the recall block
                    // is appended. The original 4 P2 unit tests
                    // keep working because `build_recall_text` is
                    // a thin wrapper that drops the rows.
                    if let Some((recall_text, recall_rows)) =
                        crate::agent::memory_recall::build_recall_text_with_rows(
                            &db,
                            &project.id,
                            &query,
                        )
                        .await
                    {
                        crate::agent::memory_recall::inject_recall_into_turn(&mut req, recall_text);
                        // Emit the R2b chat-event for the frontend's
                        // "本次召回" chip. Worker sink
                        // (SubagentBufferSink) does not forward
                        // to the IPC channel — AC7 is enforced by
                        // the sink abstraction, not by an extra
                        // check here.
                        crate::agent::memory_recall::emit_recall_event(
                            sink.as_ref(),
                            &rid,
                            &recall_rows,
                            "fts",
                        );
                    }
                }

                // W1 (Workflow integration, Phase 0 Step 0.5
                // — 2026-07-08): per-turn breadcrumb injection.
                // Sibling to the recall-injection block above
                // (not nested inside `if
                // !query.trim().is_empty()`) because the
                // breadcrumb is unconditional on recall state
                // — even when there's no user query, a
                // workflow session opening a new turn still
                // wants the state breadcrumb in front of the
                // LLM.
                //
                // Runs AFTER `inject_recall_into_turn` so the
                // recall text (when present) lands at the
                // head of `messages[0]`'s block list
                // (chronologically first per-turn) and the
                // breadcrumb sits just below it
                // (chronologically last).
                //
                // Both injectors share `messages[0]` and rely
                // on the SAME S-B guard (skip-not-prepend);
                // see
                // `agent::workflow::inject::append_workflow_breadcrumb`
                // for the rationale.
                //
                // Nested inside `if !skip_persist` because
                // workers reuse the parent's session_id and
                // the parent's workflow state is NOT what
                // the worker should be reminded of. Workers
                // call this function with `workflow_ctx =
                // None`, so the inner `if workflow_ctx` gate
                // would also short-circuit — keeping them
                // together makes the intent clear in one
                // block ("non-worker path mutations on
                // messages[0]").
                // R4 (07-10-workflow-task-json-hardening): refresh
                // `current_task` off disk at turn top so the breadcrumb
                // reflects mid-loop state changes from the previous turn
                // (a transition the user allowed, or a create_task tool
                // call). `workflow_ctx` is an owned `Option` param (mut-
                // bound); the mut borrow ends at the block's `}`, before
                // the `append_workflow_breadcrumb` read below — so the
                // immutable re-borrow there is fine. Only workflow
                // sessions have a ctx to refresh; non-workflow stays None.
                if let Some(ref mut ctx) = workflow_ctx {
                    ctx.current_task = crate::agent::workflow::inject::resolve_current_task(
                        &current_ctx.worktree_path,
                    )
                    .await;
                }
                if let Some(ref ctx) = workflow_ctx {
                    let appended =
                        crate::agent::workflow::inject::append_workflow_breadcrumb(&mut req, ctx);
                    // E2 trace (2026-07-14): record breadcrumb snapshot.
                    // Lives in chat_loop (not inject.rs) so it has access
                    // to seq + db + sink. Only fires when the breadcrumb
                    // was actually appended (S-B guard passed).
                    if appended {
                        let slug = ctx.current_task.as_ref().map(|t| t.slug.clone());
                        let status = ctx
                            .current_task
                            .as_ref()
                            .map(|t| t.status.as_str().to_string());
                        let text = crate::agent::workflow::inject::breadcrumb_body(ctx);
                        crate::agent::trace::record_breadcrumb(
                            &sink,
                            &db,
                            &rid,
                            &session_id,
                            seq,
                            slug.as_deref(),
                            status.as_deref(),
                            &text,
                        )
                        .await;
                    }
                }
            }
            req
        };

        let mut turn_tool_defs = crate::tools::filter_tools_for_workflow(
            permissions::filter_tools_for_mode(tool_defs.clone(), session_mode),
            workflow_ctx.is_some(),
        );
        // L3d (2026-06-25): append the dynamic `dispatch_subagent`
        // ToolDef so the enum reflects builtin + user + project
        // subagents merged by `SubagentCache` (mtime-fenced scan).
        // The static `dispatch_subagent` definition is no longer in
        // `builtin_tools()` (it would freeze the enum at startup);
        // we rebuild it here every turn so a freshly-written `.md`
        // is picked up on the next chat turn. `filter_tools_for_mode`
        // keeps dispatch_subagent in every mode (it is a
        // `Risk::Low` discovery tool — the worker's actual writes /
        // shells go through their own Tier 4 permission check).
        //
        // WORKER NESTING GUARD (permission-layer.md §"Subagent
        // availability"): a worker (`effective_is_worker == true`)
        // MUST NOT see `dispatch_subagent` in its turn tool list.
        // The B6 `filter_tools_for_subagent` strips
        // `dispatch_subagent` from the worker's *initial*
        // `worker_tool_defs` (`dispatch.rs:187`), but that filter
        // only applies to the seed list — this per-turn append runs
        // inside the nested `run_chat_loop` and would otherwise
        // re-introduce the ToolDef on every turn, defeating the
        // `STRUCTURALLY_DISABLED` no-nesting invariant. Skip the
        // append when we are inside a worker run.
        //
        // `worktree_path` is in scope from `run_chat_loop`'s top-level
        // session load (canonicalized via `assert_within_root`) — it
        // matches what `MemoryCache` / `SkillCache` use, so the
        // subagent `<project>/.everlasting/agents/*.md` dir lines up
        // with the project's other namespace dirs.
        if !effective_is_worker {
            let project_path = worktree_path.to_string_lossy().to_string();
            // C5: thread the active plugin name so plugin-layer agents
            // (e.g. review's `reviewer`) reach the dispatch enum.
            let workflow_name = workflow_ctx.as_ref().map(|c| c.workflow_def.name.as_str());
            let dispatch_def = crate::agent::subagent::definition_with_cache(
                &subagent_cache,
                &project_path,
                workflow_name,
                &model_briefs,
            )
            .await;
            turn_tool_defs.push(dispatch_def);
        }
        let turn_tool_defs = turn_tool_defs;
        let mut text_parts: Vec<String> = Vec::new();
        let mut tool_calls: Vec<(String, String, serde_json::Value)> = Vec::new();
        let mut finalized_thinking: Vec<(String, String)> = Vec::new();
        let mut pending_thinking: Option<PendingThinking> = None;
        // 交错思考(interleaved thinking): `ordered_blocks` 按 LLM 真实
        // 流式到达顺序累积 ContentBlock,落库时用它替代旧的"按类型分桶
        // 硬编码排序"(thinking→text→tool_use→redacted)。这样 DB 里
        // 保留 [think→text→tool] 的真实流序,reload 后前端可据此渲染
        // 出 Claude.ai/Cursor 式的连续流动形态。
        //
        // 配套的 `pending_text` 是"当前正在累积的文本块"——`Delta` 事件
        // 是逐段来的,不能每段都 push 一个 Text 块(会产生碎片)。遇到
        // 非文本边界(thinking flush / tool call / redacted / turn end)
        // 时把 pending_text flush 成一个 Text 块,实现"文本与思考/工具
        // 按真实顺序交错"。多个相邻 Text 块在语义上等价(Anthropic 接受
        // 多 Text 块),但保留了"思考夹在两段文本之间"这种流序信息。
        //
        // 红线:`ToolResult` 永远不进 `ordered_blocks`(它只进 user-role
        // message,见 §1.1 ToolResult 边界)。本累加器只装 assistant
        // 允许的 Thinking/Text/ToolUse/RedactedThinking。
        let mut ordered_blocks: Vec<ContentBlock> = Vec::new();
        let mut pending_text: Option<String> = None;
        let mut stop_reason: Option<String> = None;
        let mut last_usage: Option<crate::llm::types::TokenUsage> = None;
        let mut had_error = false;
        let mut cancelled = false;
        // A5+ (2026-07-04): first-byte-safe retry around `provider.send`.
        // See llm/retry.rs + docs/research/llm-network-resilience-survey.md.
        // On retryable first-byte failure (Network/Server/RateLimit) the
        // request is re-issued with Full Jitter backoff, bounded by budget;
        // the instant any Ok event arrives retry stops (prd R3 — tools only
        // execute after the stream completes, so pre-first-byte retry is
        // provably side-effect-free, no idempotency key needed).
        let mut rng = fastrand::Rng::new();
        let retry_sink = LlmRetrySink {
            sink: sink.clone(),
            rid: rid.clone(),
        };
        let outcome = retry_open(
            provider.as_ref(),
            Some(system_prompt.clone()),
            turn_messages,
            turn_tool_defs,
            &RetryPolicy::default(),
            &token,
            &retry_sink,
            &mut rng,
        )
        .await;
        // P2 RULE-A-009: `turn_send_at` marks when the LLM stream became
        // ready (post-retry). The other 4 `turn_*_at` vars stay declared at
        // the top of the loop body (conditionally assigned; `None` default
        // is load-bearing for `is_none()` checks).
        let turn_send_at = Some(Instant::now());
        let mut stream: Pin<
            Box<
                dyn futures_util::Stream<
                        Item = Result<crate::llm::types::ChatEvent, crate::llm::error::LlmError>,
                    > + Send,
            >,
        > = match outcome {
            OpenOutcome::Stream(s) => s,
            OpenOutcome::Cancelled => {
                // Retry gave up because the user cancelled during open
                // or backoff. Set the cancel flag and feed an empty
                // stream so the per-event loop below exits immediately
                // (None arm); the post-loop persist handles
                // CANCELLED_MARKER. The biased `token.cancelled()` arm
                // also fires on the first select iteration as backup.
                cancelled = true;
                Box::pin(futures_util::stream::empty())
            }
        };

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
                            // 流序: 文本到达前先把可能 pending 的 thinking
                            // finalize,并按真实顺序(thinking 在前)填入
                            // ordered_blocks。文本累积到 pending_text,
                            // 等下一个非文本边界再 flush 成 Text 块。
                            flush_pending_thinking(&mut pending_thinking, &mut finalized_thinking);
                            flush_ordered_thinking(&mut finalized_thinking, &mut ordered_blocks);
                            pending_text.get_or_insert_with(String::new).push_str(text);
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
                            // 流序: 一个 thinking 块开始前,先把之前累积的
                            // 文本 flush 成 Text 块(思考夹在文本之间时,
                            // 前段文本应排在思考之前)。
                            flush_pending_text(&mut pending_text, &mut ordered_blocks);
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
                            // 流序: redacted 到达前先 flush 可能 pending 的
                            // thinking + text,保持顺序。
                            flush_pending_thinking(&mut pending_thinking, &mut finalized_thinking);
                            flush_ordered_thinking(&mut finalized_thinking, &mut ordered_blocks);
                            flush_pending_text(&mut pending_text, &mut ordered_blocks);
                            ordered_blocks.push(ContentBlock::RedactedThinking { data: data.clone() });
                            emit_chat_event_via_sink(&sink, &rid, &event);
                        }
                        ChatEvent::ToolCall { id, name, input } => {
                            // 流序: 工具调用前先 flush pending thinking + text。
                            flush_pending_thinking(&mut pending_thinking, &mut finalized_thinking);
                            flush_ordered_thinking(&mut finalized_thinking, &mut ordered_blocks);
                            flush_pending_text(&mut pending_text, &mut ordered_blocks);
                            if turn_thinking_start.is_some() && turn_thinking_done.is_none() {
                                turn_thinking_done = Some(Instant::now());
                            }
                            tool_calls.push((id.clone(), name.clone(), input.clone()));
                            ordered_blocks.push(ContentBlock::ToolUse {
                                id: id.clone(),
                                name: name.clone(),
                                input: input.clone(),
                            });
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
                            // 2026-06-21 (R3): mirror the per-turn
                            // `last_usage` to the function-scope
                            // `last_usage_terminal` so the
                            // synthetic `max_turns` terminal site
                            // (chat_loop.rs:1797-1820) can forward
                            // it to the sink. The sink's R3 guard
                            // ensures the value reaches
                            // `cumulative_usage()` exactly once
                            // (no double-count). Pre-R3 this
                            // mirror did not exist; the terminal
                            // hard-coded `usage: None`, which
                            // produced the all-zero
                            // `subagent_runs.token_usage_json`
                            // regression.
                            last_usage_terminal = usage.clone();
                            turn_done_at = Some(Instant::now());
                            if turn_thinking_start.is_some() && turn_thinking_done.is_none() {
                                turn_thinking_done = Some(Instant::now());
                            }
                            if let Some(t) = usage {
                                // 2026-06-26 (token-usage snapshot fix +
                                // RULE-A-015 reversal): the per-turn
                                // `update_last_turn_usage` is now BACK
                                // inside the `!skip_persist` gate.
                                //
                                // PR1b originally gated this under
                                // `!skip_persist`. PR2a (RULE-A-015)
                                // pulled it OUT, citing "token-usage
                                // metadata lives on `sessions`, not
                                // `messages`, so the worker should
                                // still stream its per-turn usage into
                                // the parent's accumulator." That was
                                // correct under the A4 cumulative
                                // model — the worker's tokens added to
                                // the parent's running total.
                                //
                                // The snapshot model reverses this.
                                // `update_last_turn_usage` OVERWRITES
                                // the parent's `last_*` columns (not
                                // accumulates). If the worker (which
                                // reuses the parent's `session_id` —
                                // see dispatch.rs) ran unguarded, every
                                // worker turn would OVERWRITE the
                                // parent's snapshot with worker
                                // numbers. The parent UI would
                                // oscillate between parent-turn and
                                // worker-turn values; on a multi-worker
                                // dispatch the last-writer-wins
                                // outcome would be arbitrary. Worker
                                // token usage stays isolated in
                                // `subagent_runs.token_usage_json`
                                // (written at worker exit by
                                // `dispatch.rs::run_subagent`).
                                if !skip_persist {
                                    if let Err(e) = crate::db::update_last_turn_usage(&db, &session_id, t).await {
                                        tracing::warn!(error = %e, "chat: failed to update last-turn token usage (non-fatal)");
                                    }
                                    // E2 trace (2026-07-14): persist per-turn
                                    // token usage to turn_trace (worker-gated
                                    // by !skip_persist, same as
                                    // update_last_turn_usage — RULE-A-015).
                                    if let Err(e) = crate::db::trace::upsert_turn_trace_token(&db, &session_id, seq, t).await {
                                        tracing::warn!(error = %e, "trace: upsert_turn_trace_token failed (non-fatal)");
                                    }
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
                        // A5+ (2026-07-04): `Retrying` is emitted
                        // directly by `LlmRetrySink` (NOT via this
                        // per-event stream loop), so reaching this
                        // arm means a provider somehow re-emitted a
                        // retrying notice we already pushed to the
                        // frontend. Drop it (the legitimate one
                        // already shipped via `LlmRetrySink::emit_retrying`).
                        ChatEvent::Retrying { .. } => {
                            tracing::warn!(
                                request_id = %rid,
                                "chat: unexpected Retrying in LLM stream (ignoring — emitted via LlmRetrySink)"
                            );
                        }
                        // 07-06 (am-observability-panel R2b): `Recall`
                        // is emitted via `emit_recall_event` at the
                        // recall-injection seam (FTS) and the
                        // pre-tool pitfall seam (per tool_use).
                        // Reaching this arm means the LLM stream
                        // somehow re-emitted a recall notice we
                        // already pushed. Drop it (the legitimate
                        // one is on the chat-event channel; the
                        // controller will dedup by `rid`).
                        ChatEvent::Recall { .. } => {
                            tracing::warn!(
                                request_id = %rid,
                                "chat: unexpected Recall in LLM stream (ignoring — emitted via emit_recall_event)"
                            );
                        }
                        // 08-04 group-chat follow-up: `Speaker` is
                        // emitted by the orchestrator
                        // (`run_group_chat_loop`) before each inner
                        // speaker turn — NOT by a provider. Reaching
                        // this arm means the wire shape leaked. Drop
                        // it (the controller already received the
                        // legitimate one on the chat-event channel).
                        ChatEvent::Speaker { .. } => {
                            tracing::warn!(
                                request_id = %rid,
                                "chat: unexpected Speaker in LLM stream (ignoring — emitted by group_chat orchestrator)"
                            );
                        }
                        // E2 trace (2026-07-14): the 3 trace events are
                        // emitted by `agent::trace::record_*` (NOT by the
                        // LLM stream), so reaching this arm means a
                        // provider somehow re-emitted a trace event we
                        // already pushed. Drop it (same pattern as
                        // `Recall` / `Retrying` / `FileInjections`).
                        ChatEvent::ContextCompacted { .. }
                        | ChatEvent::LoopHint { .. }
                        | ChatEvent::WorkflowBreadcrumb { .. } => {
                            tracing::warn!(
                                request_id = %rid,
                                "chat: unexpected trace event in LLM stream (ignoring)"
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

        // 交错思考: 落库用 `ordered_blocks`(按真实流序累积),替代旧的
        // "按类型分桶硬编码排序"。这里做 turn-end 兜底 flush:
        // 1. 把所有已 finalize 的 thinking 按序填入
        // 2. 把最后一段 pending_text flush 成一个 Text 块
        // 之后再追加 cancel/error marker(独立 Text 块,见下)。
        // `finalized_thinking` / `pending_text` 在循环内的每个
        // 非文本边界已被增量 flush 过,这里只兜底"turn 结束时仍
        // pending 的尾部"(正常 turn 的最后一段文本/思考)。
        flush_ordered_thinking(&mut finalized_thinking, &mut ordered_blocks);
        flush_pending_text(&mut pending_text, &mut ordered_blocks);

        // RULE-A-007 (2026-06-17) + 交错思考调整: cancel/error marker
        // 追加为一个**独立 Text 块**到 ordered_blocks 末尾(而非旧
        // 逻辑里追加到 `full_text` 字符串内)。语义保持等价:
        //   - 空 turn(无文本) → 只有 marker 一个 Text 块
        //   - 非空 turn → 前段文本块 + marker 块
        // marker 文本带 `\n\n` 前缀(非空时),使 `to_text()` 把多个
        // Text 块 join 后的字符串与旧逻辑(`full_text + "\n\n" + marker`)
        // 完全一致 —— 前端用 `includes`/`endsWith` 识别 marker
        // (chat.ts ERROR_MARKER_LOCAL)的逻辑不受影响。
        // marker 作为独立块,渲染层未来可选择单独样式(对齐 §6.2)。
        let had_text = !text_parts.is_empty();
        if cancelled {
            let marker = if had_text {
                format!("\n\n{}", CANCELLED_MARKER)
            } else {
                CANCELLED_MARKER.to_string()
            };
            ordered_blocks.push(ContentBlock::Text {
                text: marker,
                cache_control: None,
            });
        } else if had_error {
            let marker = if had_text {
                format!("\n\n{}", ERROR_MARKER)
            } else {
                ERROR_MARKER.to_string()
            };
            ordered_blocks.push(ContentBlock::Text {
                text: marker,
                cache_control: None,
            });
        }

        // `assistant_blocks` 直接复用流序累积的 `ordered_blocks`。
        // 旧的分桶循环(thinking→text→tool_use→redacted 硬编码)已删除
        // —— 所有块在循环内已按真实到达顺序填入。
        let assistant_blocks = ordered_blocks;

        if !assistant_blocks.is_empty() {
            let msg = ChatMessage {
                role: Role::Assistant,
                content: MessageContent::Blocks(assistant_blocks),
                // Group chat (Phase 4 TODO-A): carry the originating
                // speaker into the assistant persist site. `None` for
                // classic chat / subagent / review (no behavior change
                // vs. pre-Phase 4); `Some("moderator" | participant.name)`
                // for group chat turns. The DB column is nullable
                // (Phase 1 migration) so existing rows / sessions are
                // unaffected.
                speaker: current_speaker.clone(),
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
                    msg.speaker.as_deref(),
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
            // Symmetric with the cancel path above (chat_loop.rs
            // ~1457): if the model emitted tool_use before the
            // stream errored, the assistant(tool_use) turn pushed
            // at line ~1453 would be orphaned (no matching
            // tool_result) → the next request fails upstream with
            // HTTP 400 "insufficient tool messages following
            // tool_calls" (OpenAI) / 2013 (Anthropic). Push one
            // synthetic is_error tool_result per emitted tool_use
            // so the pair stays atomic (llm-contract.md §469).
            // Persist is log-only (RULE-A-007 decision B): the
            // terminal Error already fired, a persist failure here
            // must not emit a second terminal.
            if !tool_calls.is_empty() {
                let tool_result_msg = build_synthetic_tool_result_message(&tool_calls);
                if !skip_persist {
                    if let Err(e) = crate::db::persist_turn(
                        &db,
                        &session_id,
                        tool_result_msg.role,
                        &tool_result_msg.content,
                        seq,
                        None,
                        None,
                    )
                    .await
                    {
                        tracing::error!(
                            error = %e,
                            "failed to persist synthetic tool_result turn after error"
                        );
                    }
                }
                messages.push(tool_result_msg);
            }
            // B6 PR1b: skip the cwd/touch_session writes in worker
            // mode (the parent's session row is not the worker's
            // to update — the parent owns the lifetime).
            if !skip_persist {
                persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
                let _ = crate::db::touch_session(&db, &session_id).await;
            }
            return;
        }

        let should_continue = stop_reason.as_deref() == Some("tool_use") && !tool_calls.is_empty();

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
                &ChatEvent::Done {
                    stop_reason,
                    usage: last_usage,
                },
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
        // ⑬ loop detection (C2): feed this turn's tool_calls into the
        // sliding window, then run the two-level detector. On a hit we
        // keep a hint string to prepend to the result message (soft —
        // we never skip execution and never terminate; MAX_TURNS stays
        // the hard backstop). Per §2.5.8 this is tracing-only, no
        // AuditKind row.
        for (_id, name, input) in &tool_calls {
            loop_window.push_back(loop_detection::ToolCall::new(name.clone(), input.clone()));
        }
        while loop_window.len() > loop_detection::SOFT_WINDOW {
            loop_window.pop_front();
        }
        let loop_verdict = loop_detection::detect(&loop_window.iter().cloned().collect::<Vec<_>>());
        // C2+ (2026-07-05): maintain the consecutive-hit counter and
        // trigger active intervention at >= 3. The counter is
        // per-`run_chat_loop`-local (declared next to `loop_window`
        // outside the turn loop) so it accumulates across turns; on
        // any non-loop turn it resets to 0 (consecutiveness is the
        // signal). See `design.md §2` for the full state machine.
        let verdict_kind_str: Option<&'static str> = match &loop_verdict {
            loop_detection::LoopVerdict::HardLoop { .. } => Some("hard"),
            loop_detection::LoopVerdict::SoftLoop { .. } => Some("soft"),
            loop_detection::LoopVerdict::None => None,
        };
        let mut loop_hint: Option<String> = loop_verdict.hint_text();
        if verdict_kind_str.is_some() {
            tracing::warn!(verdict = ?loop_verdict, "agent loop ⑬: loop detected (soft hint)");
        }

        // C2+ active-intervention state machine. Only the main loop
        // drives the QuestionStore ask — worker subagents (which
        // reuse `run_chat_loop` with `effective_is_worker = true`)
        // take a direct-break short-circuit below so they don't
        // occupy the parent's QuestionStore slot or interrupt the
        // user. The worker's break surfaces to the parent via its
        // `Done { stop_reason: "loop_terminated" }` and the parent
        // sees the result in `dispatch_subagent`'s tool_result.
        if verdict_kind_str.is_some() {
            loop_hit_count = loop_hit_count.saturating_add(1);
        } else {
            loop_hit_count = 0;
        }

        // E2 trace (2026-07-14): record C2 soft hint (1-2 consecutive
        // hits, before the ≥3 active-intervention threshold). The ≥3
        // path already writes `loop_intervention` audit rows; this
        // trace covers the pre-intervention turns only.
        if verdict_kind_str.is_some() && loop_hit_count < 3 {
            if let Some(vk) = verdict_kind_str {
                crate::agent::trace::record_loop_hint(
                    &sink,
                    &db,
                    &rid,
                    &session_id,
                    seq,
                    loop_hit_count,
                    vk,
                )
                .await;
            }
        }

        if loop_hit_count >= 3 && verdict_kind_str.is_some() {
            // Reached the consecutive-hit threshold on a loop turn.
            // Worker path: direct break (R5) — no QuestionStore
            // round-trip, no audit row. The worker's loop_terminated
            // Done will be observed by `run_subagent` and surfaced
            // to the parent LLM via `format_dispatch_result*` (PR3
            // extends the formatter to detect `loop_terminated` and
            // append the "worker 因循环被终止" line; for PR2 we
            // only need the stop_reason itself to terminate the
            // worker's loop cleanly).
            if effective_is_worker {
                tracing::info!(
                    hit_count = loop_hit_count,
                    verdict = ?loop_verdict,
                    "C2+ worker path: breaking loop (direct break, no ask)"
                );
                if !skip_persist {
                    persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
                    let _ = crate::db::touch_session(&db, &session_id).await;
                }
                emit_chat_event_via_sink(
                    &sink,
                    &rid,
                    &ChatEvent::Done {
                        stop_reason: Some("loop_terminated".to_string()),
                        usage: last_usage.clone(),
                    },
                );
                return;
            }

            // Main loop: build the fixed payload (PRD R2) and drive
            // the QuestionStore round-trip.
            let verdict_kind_str_expect =
                verdict_kind_str.expect("non-None verdict at C2+ trigger");
            let payload = crate::agent::question_store::ToolQuestionPayload {
                session_id: session_id.clone(),
                tool_use_id: format!("loop_intervention_{}", turn),
                questions: vec![crate::agent::question_store::Question {
                    question: "检测到 agent 似乎在循环重复相同操作（已连续 3 次触发循环检测，\
                               注入的软提示未能让模型纠正）。是否终止本次 agent loop？"
                        .to_string(),
                    header: Some("循环检测干预".to_string()),
                    options: vec![
                        crate::agent::question_store::QuestionOption {
                            label: "终止 loop".into(),
                            description: Some("停止本次 agent loop，保留已生成的内容".to_string()),
                            preview: None,
                        },
                        crate::agent::question_store::QuestionOption {
                            label: "继续".into(),
                            description: Some(
                                "清零计数器继续，给模型再次自我纠正的机会".to_string(),
                            ),
                            preview: None,
                        },
                    ],
                    multi_select: false,
                    allow_custom: false,
                }],
                ts: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0),
            };

            // Audit: action = "asked" lands immediately after
            // register succeeds (best-effort; helper is warn+swallow
            // on DB error — matches record_audit / record_tool_executed_audit).
            let _ = crate::agent::permissions::audit::record_loop_intervention_audit(
                &db,
                &session_id,
                None,
                loop_hit_count,
                verdict_kind_str_expect,
                "asked",
                Some(seq),
            )
            .await;

            // Try to register the pending question. `AlreadyPending`
            // (the LLM concurrently drove an `ask_user_question`
            // tool_use that's still waiting on a resolve) → log +
            // fall through to the original hint path (don't block
            // the loop; next turn we'll try again).
            match question_store
                .register(
                    &session_id,
                    &format!("loop_intervention_{}", turn),
                    // C2+ registers as `LoopIntervention` (NOT
                    // `Question`) so the frontend can render it as a
                    // floating card. `Question` would require an
                    // `ask_user_question` tool_use block anchor that
                    // doesn't exist for a synthetic intervention →
                    // the card would never render (2026-07-28
                    // incident, session e8a1ad96…).
                    crate::agent::question_store::PendingInteraction::LoopIntervention(
                        payload.clone(),
                    ),
                )
                .await
            {
                Ok(rx) => {
                    sink.emit_tool_question(&payload);
                    tracing::info!(
                        hit_count = loop_hit_count,
                        verdict = ?loop_verdict,
                        "C2+ active intervention: question asked"
                    );
                    // Three-arm select: cancel / answer / dropped.
                    tokio::select! {
                        biased;
                        _ = token.cancelled() => {
                            // User hit Stop while the question was
                            // pending. Same cancel-cleanup as the
                            // ask_user_question tool path: clear the
                            // slot, emit Done{cancelled} (NOT
                            // Done{loop_terminated} — the user
                            // initiated this exit, not the C2+
                            // intervention).
                            question_store.remove(&session_id).await;
                            // 3b: synthesize the tool_result for the
                            // assistant's already-persisted tool_use
                            // blocks (this is the cancel-during-
                            // intervention arm — the per-stream
                            // `cancelled` flag is NOT set here, so
                            // the existing tail-pair repair at line
                            // 2101 doesn't fire; only this helper
                            // does).
                            finalize_pending_tool_results(
                                &db,
                                &session_id,
                                &tool_calls,
                                seq,
                                skip_persist,
                            )
                            .await;
                            if !skip_persist {
                                persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
                                let _ = crate::db::touch_session(&db, &session_id).await;
                            }
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
                        resp = rx => {
                            match resp {
                                Ok(crate::agent::question_store::InteractionResponse::Answered(value)) => {
                                    // Inspect the first answer's
                                    // selected options. Treat
                                    // "终止 loop" (or empty / no
                                    // match) as terminate, "继续"
                                    // as continue. `Cancelled` is
                                    // handled in the next arm.
                                    //
                                    // The `Answered` value is a
                                    // `serde_json::Value` (unified
                                    // `InteractionResponse`); the
                                    // C2+ intervention uses the
                                    // question shape (we registered
                                    // `PendingInteraction::Question`
                                    // above) so the value is a
                                    // JSON-serialized
                                    // `Vec<QuestionAnswer>`.
                                    let answers: Vec<crate::agent::question_store::QuestionAnswer> =
                                        serde_json::from_value(value).unwrap_or_default();
                                    let chosen = answers
                                        .first()
                                        .map(|a| a.options.first().cloned().unwrap_or_default())
                                        .unwrap_or_default();
                                    if chosen == "继续" {
                                        let _ = crate::agent::permissions::audit::record_loop_intervention_audit(
                                            &db,
                                            &session_id,
                                            None,
                                            loop_hit_count,
                                            verdict_kind_str_expect,
                                            "continued",
                                        Some(seq),
                                        ).await;
                                        // Reset the counter so the
                                        // model gets a fresh 3-strike
                                        // budget. Replace the soft
                                        // hint with a stronger one
                                        // that tells the model the
                                        // user explicitly confirmed
                                        // the loop — repeating the
                                        // same call will not make
                                        // progress.
                                        loop_hit_count = 0;
                                        loop_hint = Some(
                                            "loop intervention: 用户已确认你在循环重复操作并选择继续。\
                                             请立即改变策略或停止 — 重复相同调用不会取得进展。"
                                                .to_string(),
                                        );
                                        // Fall through to the
                                        // normal result_blocks
                                        // construction (the
                                        // enhanced hint above will
                                        // be prepended like any
                                        // other loop hint).
                                    } else {
                                        // "终止 loop" (or any
                                        // non-"继续" selection —
                                        // defensive: defaults to
                                        // terminate so a malformed
                                        // payload doesn't loop
                                        // forever).
                                        let _ = crate::agent::permissions::audit::record_loop_intervention_audit(
                                            &db,
                                            &session_id,
                                            None,
                                            loop_hit_count,
                                            verdict_kind_str_expect,
                                            "terminated",
                                        Some(seq),
                                        ).await;
                                        // 3b: synthesize the
                                        // tool_result for the
                                        // assistant's already-
                                        // persisted tool_use blocks.
                                        finalize_pending_tool_results(
                                            &db,
                                            &session_id,
                                            &tool_calls,
                                            seq,
                                            skip_persist,
                                        )
                                        .await;
                                        if !skip_persist {
                                            persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
                                            let _ = crate::db::touch_session(&db, &session_id).await;
                                        }
                                        emit_chat_event_via_sink(
                                            &sink,
                                            &rid,
                                            &ChatEvent::Done {
                                                stop_reason: Some("loop_terminated".to_string()),
                                                usage: None,
                                            },
                                        );
                                        return;
                                    }
                                }
                                Ok(crate::agent::question_store::InteractionResponse::Cancelled) => {
                                    // User clicked "跳过" on the
                                    // intervention card → treat as
                                    // "终止 loop" (same rationale
                                    // as the cancel arm of
                                    // ask_user_question: the user
                                    // dismissed the question, the
                                    // safe default is to stop).
                                    let _ = crate::agent::permissions::audit::record_loop_intervention_audit(
                                        &db,
                                        &session_id,
                                        None,
                                        loop_hit_count,
                                        verdict_kind_str_expect,
                                        "terminated",
                                    Some(seq),
                                    ).await;
                                    // 3b: synthesize the tool_result
                                    // for the assistant's already-
                                    // persisted tool_use blocks.
                                    finalize_pending_tool_results(
                                        &db,
                                        &session_id,
                                        &tool_calls,
                                        seq,
                                        skip_persist,
                                    )
                                    .await;
                                    if !skip_persist {
                                        persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
                                        let _ = crate::db::touch_session(&db, &session_id).await;
                                    }
                                    emit_chat_event_via_sink(
                                        &sink,
                                        &rid,
                                        &ChatEvent::Done {
                                            stop_reason: Some("loop_terminated".to_string()),
                                            usage: None,
                                        },
                                    );
                                    return;
                                }
                                Err(_recv_err) => {
                                    // Sender dropped (e.g. resolve
                                    // ran on a stale session id
                                    // after the cancel arm cleaned
                                    // the entry). Treat as session-
                                    // cancelled — safe default
                                    // matching the permission-store /
                                    // ask_user_question parity.
                                    tracing::warn!(
                                        "C2+ oneshot dropped without response — treating as cancelled"
                                    );
                                    // 3b: synthesize the tool_result
                                    // the assistant turn already
                                    // emitted (line 2097) so the DB
                                    // does not end with an orphan
                                    // tool_use that crashes the next
                                    // LLM call.
                                    finalize_pending_tool_results(
                                        &db,
                                        &session_id,
                                        &tool_calls,
                                        seq,
                                        skip_persist,
                                    )
                                    .await;
                                    if !skip_persist {
                                        persist_turn_cwd(&db, &session_id, last_cwd.as_deref()).await;
                                        let _ = crate::db::touch_session(&db, &session_id).await;
                                    }
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
                            }
                        }
                    }
                }
                Err(crate::agent::question_store::QuestionStoreError::AlreadyPending) => {
                    // LLM concurrently drove an ask_user_question
                    // that's still pending — the single-pending gate
                    // refuses our register. Per design §5 we
                    // gracefully degrade: skip C2+ this turn (the
                    // soft hint still lands), try again next turn.
                    tracing::warn!(
                        session_id = %session_id,
                        "C2+ skipped: a question is already pending (LLM-driven ask_user_question)"
                    );
                }
                Err(e) => {
                    // `NotFound` is not reachable from `register`
                    // (defensive branch matching ask_user_question).
                    tracing::error!(
                        error = %e,
                        "C2+ register: unexpected store error"
                    );
                }
            }
        }

        let mut result_blocks: Vec<ContentBlock> = Vec::new();
        if is_parallel_eligible(&tool_calls, &permission_ctx.project_main_path) {
            // ---- L2 parallel path (read-only batch) ----
            //
            // All tool_use blocks in this turn are in the
            // {read_file, grep, glob, list_dir, use_skill}
            // whitelist AND every path tool's `path` resolves
            // inside the project root (`permission_ctx.project_main_path`)
            // → run them concurrently via `FuturesUnordered`.
            // `web_fetch` is excluded (Q2) because its Tier 4
            // default is `ask`, which would fire multiple
            // concurrent `permission:ask` modals. Path tools
            // with an out-of-root `path` are also excluded by
            // the same rule (RULE-A-013 follow-up, 2026-06-19)
            // — see `is_parallel_eligible` doc.
            //
            // Anchor note (2026-07-29): the root MUST be
            // `project_main_path`, not `cwd`/`worktree_path`. For an
            // isolated worker the latter both point at the worker's
            // checkout subtree, so reads of the project's source
            // files (by their original absolute path) would fail
            // `is_within_root` → the whole batch is demoted to serial
            // → each read tool hits `check()` separately → magnified
            // permission prompts. See `PermissionContext.project_main_path`.
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
            let mut result_slots: Vec<Option<ContentBlock>> = (0..n).map(|_| None).collect();
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
                    // P4 (2026-06-29, 06-29-am-p4-event-reflect):
                    // the per-session failure tracker + provider
                    // + project_id are cloned into the parallel
                    // task so the L2 batch's tool outcomes feed
                    // the same in-session state machine as the
                    // serial path. The reflection is fire-and-
                    // forget (tokio::spawn inside) so the parallel
                    // path's perf is unaffected.
                    let failure_tracker = failure_tracker.clone();
                    let provider = provider.clone();
                    let project_id = current_ctx.project_id.clone();
                    // P5 (2026-06-29, 06-29-am-p5-quality): clone
                    // the soft-block ledger so the parallel task
                    // can both read (recall decision) + write
                    // (record a freshly-soft-blocked memory_id) it.
                    let soft_blocked = soft_blocked.clone();
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
                        if let Decision::Deny {
                            reason,
                            critical: _,
                        } = decision
                        {
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

                        // P3 + P5 (2026-06-29, 06-29-am-p3-tool-recall
                        // + 06-29-am-p5-quality): Tier 1 Hooks —
                        // pre-tool pitfall recall. Runs AFTER
                        // `check()` returns Allow and BEFORE
                        // `execute_tool()`. P5 introduces the
                        // tiered `PitfallRecall`:
                        //   - SoftBlock { hint, memory_id } —
                        //     verified + full trigger-key match +
                        //     not-yet-blocked this session → short-
                        //     circuit execute_tool, record memory_id,
                        //     surface hint as is_error=false tool_result.
                        //   - Footnote(text) — active / candidate /
                        //     partial / second-hit → execute normally,
                        //     prepend text to the result content.
                        //   - None — no recall.
                        // The dead-loop guard (design D1): the second
                        // hit on the same SoftBlock'd pitfall
                        // degrades to Footnote because the memory_id
                        // is now in `soft_blocked`.
                        // 07-06 (am-observability-panel R2b / A9):
                        // the rows-aware sibling carries the raw hit
                        // set so we can emit a `ChatEvent::Recall`
                        // for the frontend's "本次召回" chip.
                        // Worker isolation: this chat loop is the
                        // PARENT path — worker's `SubagentBufferSink`
                        // does not forward to the main chat IPC.
                        let blocked_snapshot = soft_blocked.lock().await.clone();
                        let (pitfall_recall, pitfall_rows) = permissions::recall_pitfall_with_hits(
                            &db,
                            &name,
                            &input,
                            &blocked_snapshot,
                        )
                        .await;
                        // Emit pre-tool pitfall recall hit (R2b).
                        // None → skip (no rows).
                        if !pitfall_rows.is_empty() {
                            crate::agent::memory_recall::emit_recall_event(
                                sink.as_ref(),
                                &rid,
                                &pitfall_rows,
                                "pitfall",
                            );
                        }
                        // SoftBlock: short-circuit. The tool is NOT
                        // executed; we surface the hint as a non-error
                        // tool_result so the LLM re-judges (adjust /
                        // abandon / insist). Mirrors Decision::Deny's
                        // "skip execute + return ToolResult" structure
                        // (above), but `is_error: false` so the LLM
                        // doesn't think the tool is broken.
                        if let permissions::PitfallRecall::SoftBlock { hint, memory_id } =
                            pitfall_recall
                        {
                            // Record in the session ledger (D1).
                            soft_blocked.lock().await.insert(memory_id);
                            let envelope = crate::agent::helpers::tool_result_envelope(
                                &hint,
                                &current_ctx.worktree_path,
                            );
                            sink.emit_tool_result(&crate::state::ToolResultPayload {
                                request_id: rid.clone(),
                                tool_use_id: id.clone(),
                                content: envelope.clone(),
                                is_error: false,
                            });
                            // No tool_executed audit (tool didn't run
                            // — RULE-A-004's intent applies: don't
                            // lie to the audit log). No P4 reflect
                            // either (no real outcome to learn from).
                            return Some((
                                i,
                                ContentBlock::ToolResult {
                                    tool_use_id: id,
                                    content: envelope,
                                    is_error: false,
                                },
                            ));
                        }
                        // Footnote or None: extract the optional text
                        // for the prepend-after-execute path.
                        let pitfall_footnote = match pitfall_recall {
                            permissions::PitfallRecall::Footnote(text) => Some(text),
                            permissions::PitfallRecall::None => None,
                            permissions::PitfallRecall::SoftBlock { .. } => {
                                // Unreachable — handled above.
                                None
                            }
                        };

                        let tool_exec_start = Instant::now();
                        let (content, is_error, _update, exit_code) = crate::tools::execute_tool(
                            &name,
                            &input,
                            &current_ctx,
                            Some(&read_guard),
                            Some(&session_id),
                            Some(&skill_cache),
                            token.clone(),
                        )
                        .await;
                        // P3: prepend the pitfall footnote (if any)
                        // to the tool result content BEFORE the
                        // envelope wrap, so the LLM reads the hint
                        // together with the tool output. is_error
                        // is preserved (an error message preceded
                        // by a pitfall hint is still an error).
                        let content = if let Some(footnote) = pitfall_footnote {
                            format!("{}{}", footnote, content)
                        } else {
                            content
                        };
                        // P4 (2026-06-29, 06-29-am-p4-event-reflect):
                        // record this tool_use outcome into the
                        // in-session failure tracker. On the
                        // "≥2 fails then success" pattern, the
                        // tracker fires a fire-and-forget LLM
                        // reflection that produces a pitfall memory
                        // row. Skipped on cancel (the in-flight tool
                        // was interrupted, not a real outcome) — same
                        // intent as the audit-skip above.
                        if !token.is_cancelled() {
                            crate::agent::auto_reflect::try_record_outcome(
                                &failure_tracker,
                                provider.clone(),
                                db.clone(),
                                &rid,
                                &session_id,
                                &project_id,
                                &name,
                                &input,
                                is_error,
                                &content,
                            )
                            .await;
                        }
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
                                Some(seq),
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
            //
            // L3a (2026-06-24) / L3b PR2 (2026-06-27): before the
            // regular serial `for` loop, classify the batch for
            // the concurrent dispatch_subagent path. A **pure**
            // batch of ≥2 dispatch_subagent tool_uses (no other
            // tools mixed in) within
            // `DELEGATION_MAX_CONCURRENT_CHILDREN` (env, default 10)
            // runs concurrently via `FuturesUnordered` (each worker
            // in its own per-worker worktree — L3b PR1 isolation,
            // not the L3a read-only scope). A pure batch OVER the
            // limit is hard-rejected (every tool_use gets a
            // tool_error tool_result — no truncation, no queuing,
            // mirrors Hermes). Anything else (single dispatch, or
            // a mixed batch) falls through to the regular serial
            // `for` loop unchanged.
            let dispatch_batch =
                classify_dispatch_batch(&tool_calls, delegation_max_concurrent_children());
            match dispatch_batch {
                DispatchBatch::OverLimit {
                    count,
                    max_concurrent,
                } => {
                    // Hard reject: every dispatch_subagent tool_use
                    // gets a tool_error tool_result. None execute.
                    // The LLM sees N uniform failure signals + a
                    // hint to re-plan (reduce the batch or split).
                    tracing::warn!(
                        count,
                        max_concurrent,
                        "L3a: pure dispatch batch over concurrent limit — hard rejecting all"
                    );
                    for (id, _name, _input) in &tool_calls {
                        let reject_content = format!(
                            "Concurrent dispatch limit reached: {count} dispatch_subagent \
                             calls in one turn exceeds the limit of {max_concurrent}. Reduce \
                             the number of concurrent subagents per turn (dispatch fewer at \
                             once, or split across turns), or raise the limit via the \
                             DELEGATION_MAX_CONCURRENT_CHILDREN environment variable. No \
                             subagents were dispatched."
                        );
                        let envelope_str = crate::agent::helpers::tool_result_envelope(
                            &reject_content,
                            &current_ctx.worktree_path,
                        );
                        sink.emit_tool_result(&crate::state::ToolResultPayload {
                            request_id: rid.clone(),
                            tool_use_id: id.clone(),
                            content: envelope_str.clone(),
                            is_error: true,
                        });
                        result_blocks.push(ContentBlock::ToolResult {
                            tool_use_id: id.clone(),
                            content: envelope_str,
                            is_error: true,
                        });
                    }
                }
                DispatchBatch::Concurrent { count: _ } => {
                    // ---- L3b PR2 concurrent dispatch path (pure
                    //      dispatch_subagent batch, ≥2 workers,
                    //      each in its own worker worktree) ----
                    //
                    // L3a (2026-06-24) launched this branch with
                    // `force_readonly = true` to dissolve the 3
                    // concurrent write races via a read-only
                    // scope (no writes = no race). L3b PR1
                    // (2026-06-27) introduced per-worker worktree
                    // isolation; L3b PR2 (2026-06-27, this change)
                    // **removes the read-only scope** for the
                    // concurrent path — each concurrent worker now
                    // runs in its own `worker/<run_id>` worktree
                    // (general-purpose builtin defaults to
                    // `isolation: Some(true)`, so writes land on
                    // the worker's branch, not the parent's).
                    //
                    // Mirror the L2 parallel-read path's structure
                    // (FuturesUnordered + result_slots[i] + shared
                    // cancelled flag), but each task runs
                    // `run_subagent` (with `force_readonly = false`,
                    // serial-only param; the worker worktree
                    // isolation takes its place). The 3-layer
                    // read-only guarantee is **no longer the
                    // concurrent path's safety argument** — the
                    // per-worker worktree is.
                    //
                    // Permission: every dispatch_subagent tool_use
                    // goes through the existing ⑨ check BEFORE the
                    // task is spawned (mirrors the serial path's
                    // pre-execute permission check). A Deny short-
                    // circuits into a tool_result tool_use pairing
                    // without spawning the worker.
                    //
                    // Cancel: each task takes `token.clone()`; the
                    // worker's nested run_chat_loop sees the parent
                    // cancel via `parent_token.child_token()` (the
                    // existing fan-out mechanism). The shared
                    // `cancelled_flag` is set if any worker
                    // returned `cancel_parent = true` (parent-
                    // propagated cancel detected).
                    //
                    // Audit: each successful worker dispatch
                    // records its own `tool_executed` audit row
                    // (same as the serial path's
                    // `record_tool_executed_audit` call). Cancelled
                    // workers skip the audit (RULE-A-004).
                    //
                    // Result ordering: `result_slots[i]` is pre-
                    // allocated to the tool_use count; each task
                    // writes at its OWN index so the LLM context
                    // sees tool_results in tool_use order regardless
                    // of completion order. `emit_tool_result` fires
                    // as each task completes (streaming).
                    let n = tool_calls.len();
                    let mut result_slots: Vec<Option<ContentBlock>> =
                        (0..n).map(|_| None).collect();
                    let cancelled_flag = Arc::new(AtomicBool::new(false));
                    let mut fu: FuturesUnordered<_> = tool_calls
                        .iter()
                        .enumerate()
                        .map(|(i, (id, name, input))| {
                            let sink = sink.clone();
                            let rid = rid.clone();
                            let id = id.clone();
                            let name = name.clone();
                            let input = input.clone();
                            let permission_ctx = permission_ctx.clone();
                            let permission_asks = permission_asks.clone();
                            let db = db.clone();
                            let session_id = session_id.clone();
                            let token = token.clone();
                            let provider = provider.clone();
                            let memory_cache = memory_cache.clone();
                            let read_guard = read_guard.clone();
                            let skill_cache = skill_cache.clone();
                            let cancellations = cancellations.clone();
                            let session_active_request = session_active_request.clone();
                            let background_shells = background_shells.clone();
                            let current_ctx = current_ctx.clone();
                            let worker_event_sink = worker_event_sink.clone();
                            let worker_catalog = worker_catalog.clone();
                            let skip_persist = skip_persist;
                            let cancelled_flag = cancelled_flag.clone();
                            // W1 Step 2.4: workflow role-gate
                            // needs WorkflowCtx inside the
                            // closure (the concurrent batch
                            // path). Clone like the other
                            // captured variables so the
                            // closure doesn't move the
                            // outer-scope `workflow_ctx`
                            // (which is also borrowed by the
                            // per-turn breadcrumb injection
                            // on line 1532 — moving would
                            // break that site on iteration
                            // N+1).
                            let workflow_ctx = workflow_ctx.clone();
                            // W1 Step 2.4: workflow role-gate
                            // needs WorkflowCtx inside the
                            // closure (the concurrent batch
                            // path). Clone like the other
                            // captured variables so the
                            // closure doesn't move the
                            // outer-scope `workflow_ctx`
                            // (which is also borrowed by the
                            // per-turn breadcrumb injection
                            // on line 1532 — moving would
                            // break that site on iteration
                            // N+1).
                            let workflow_ctx = workflow_ctx.clone();
                            let subagent_cache = subagent_cache.clone();
                            let app_data_dir = app_data_dir.clone();
                            // 2026-06-30 (`ask_user_question` task):
                            // clone the QuestionStore for this task
                            // body. The worker's toolset strips
                            // `ask_user_question` via
                            // `STRUCTURALLY_DISABLED`, so this clone
                            // is never actually used by the worker's
                            // run_subagent on this concurrent branch —
                            // pass for signature uniformity with the
                            // serial dispatch path. (The cargo borrow
                            // checker requires clone() here regardless
                            // because each concurrent task takes
                            // ownership of its slot via `async move`.)
                            let question_store = question_store.clone();
                            async move {
                                // Pre-execute ⑨ permission check
                                // (mirrors the serial path's
                                // permissions::check before execute).
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
                                let (content, is_error, cancel_parent, exit_code) =
                                    crate::agent::subagent::dispatch::run_subagent(
                                        &provider,
                                        worker_catalog.clone(),
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
                                        &id,
                                        &input,
                                        &token,
                                        &sink,
                                        worker_event_sink.clone(),
                                        // L3b PR2 (2026-06-27): the
                                        // concurrent branch no longer
                                        // forces read-only. Per-worker
                                        // worktree isolation (PR1) is
                                        // the new safety argument;
                                        // `force_readonly` is now
                                        // serial-only. `general-purpose`
                                        // builtin defaults to
                                        // `isolation: Some(true)`, so
                                        // each concurrent worker lands
                                        // its writes on its own
                                        // `worker/<run_id>` branch.
                                        false,
                                        // L3d (2026-06-25): thread the
                                        // subagent cache so the worker
                                        // resolves across builtin + user
                                        // + project layers.
                                        &subagent_cache,
                                        // L3b (2026-06-27): thread the
                                        // app data dir for worker
                                        // worktree path computation.
                                        &app_data_dir,
                                        // B (2026-06-30): concurrent batch →
                                        // `parallel=true` force-isolates
                                        // writable workers onto their own
                                        // `worker/<run_id>` branch.
                                        true,
                                        // 2026-06-30 (`ask_user_question`):
                                        // thread the parent's QuestionStore.
                                        // Worker never reaches the
                                        // intercept (tool is in
                                        // STRUCTURALLY_DISABLED) — pass-
                                        // through only.
                                        &question_store,
                                        // W1 Step 2.4: workflow
                                        // role-gate. Same as the
                                        // forced-dispatch path: pass
                                        // the session's WorkflowCtx
                                        // (None for non-workflow).
                                        workflow_ctx.as_ref(),
                                    )
                                    .await;
                                let duration_ms = tool_exec_start.elapsed().as_millis();
                                // RULE-A-004 + audit (same shape as
                                // the serial dispatch path).
                                if token.is_cancelled() || cancel_parent {
                                    cancelled_flag.store(true, Ordering::SeqCst);
                                } else if !skip_persist {
                                    if let Err(e) = permissions::record_tool_executed_audit(
                                        &db,
                                        &session_id,
                                        &name,
                                        &input,
                                        duration_ms,
                                        exit_code,
                                    Some(seq),
                                    )
                                    .await
                                    {
                                        tracing::warn!(
                                            error = %e,
                                            "L3a concurrent dispatch: record_tool_executed_audit failed (non-fatal)"
                                        );
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
                    result_blocks = result_slots.into_iter().flatten().collect();
                    if cancelled_flag.load(Ordering::SeqCst) {
                        cancelled = true;
                    }
                }
                DispatchBatch::Serial => {
                    // Regular serial path (existing behavior, unchanged).
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
                        if let Decision::Deny {
                            reason,
                            critical: _,
                        } = decision
                        {
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

                        // 2026-06-30 (`ask_user_question` task): block the
                        // current turn on a reverse-question card. This is
                        // the same "control-flow tool" interception pattern as
                        // `dispatch_subagent` below — the tool needs
                        // QuestionStore + ChatEventSink access that
                        // `execute_tool_inner` doesn't have. We route to
                        // `ask_user_question::execute_blocking`, which
                        // internally:
                        //   1. validates the schema (short-circuits with
                        //      `is_error: true` on boundary violations),
                        //   2. registers a oneshot in QuestionStore,
                        //   3. emits `tool:question` to the frontend,
                        //   4. `tokio::select! { cancel | oneshot }` until
                        //      resolve.
                        //
                        // `is_parallel_eligible` (the L2 whitelist of
                        // `read_file` / `grep` / `glob` / `list_dir` /
                        // `use_skill`) does NOT include this name → mixed
                        // batches fall to the serial path (this branch)
                        // automatically (per design §6.3 + the
                        // `dispatch_subagent` precedent).
                        //
                        // The interception here is structurally identical to
                        // the dispatch_subagent one just below: same audit
                        // row, same `tool:result` IPC emit, same
                        // `ContentBlock::ToolResult` push, same cancel
                        // propagation. The only difference is the work
                        // function — `execute_blocking` instead of
                        // `run_subagent` — and `execute_blocking` does NOT
                        // need the `cancel_parent` separate flag (it
                        // collapses into the session cancel token directly).
                        // We accept +1 turn counter cost for the blocking
                        // tool's recover (v1 trade-off — see PRD §R3).
                        if name == "ask_user_question" {
                            let tool_exec_start = Instant::now();
                            let (content, is_error, _update, exit_code) =
                                crate::tools::ask_user_question::execute_blocking(
                                    input,
                                    &session_id,
                                    id,
                                    &question_store,
                                    &sink,
                                    &token,
                                )
                                .await;
                            let duration_ms = tool_exec_start.elapsed().as_millis();
                            if token.is_cancelled() {
                                cancelled = true;
                            } else if !skip_persist {
                                if let Err(e) = permissions::record_tool_executed_audit(
                                    &db,
                                    &session_id,
                                    name,
                                    input,
                                    duration_ms,
                                    exit_code,
                                    Some(seq),
                                )
                                .await
                                {
                                    tracing::warn!(error = %e, "chat: record_tool_executed_audit failed for ask_user_question (non-fatal)");
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
                            if cancelled {
                                break;
                            }
                            continue;
                        }

                        // Group chat (07-29-group-chat): the
                        // moderator calls `nominate_speaker` to hand
                        // the floor to a participant, or
                        // `end_discussion` to stop. Both are SIGNAL
                        // tools (non-blocking) — they record into the
                        // shared `group_chat_state` (read by
                        // `run_group_chat_loop` after this loop
                        // returns) and emit a confirmation as
                        // tool_result so the moderator's turn ends
                        // at a clean boundary.
                        //
                        // No audit row + no oneshot (unlike
                        // ask_user_question): these are pure
                        // in-process signals. When `group_chat_state`
                        // is `None` (classic chat / worker), the call
                        // is a misuse — return an error tool_result.
                        if name == crate::tools::nominate_speaker::NOMINATE_SPEAKER_TOOL_NAME
                            || name == crate::tools::end_discussion::END_DISCUSSION_TOOL_NAME
                        {
                            let (content, is_error) = match &group_chat_state {
                                Some(state) if name
                                    == crate::tools::nominate_speaker::NOMINATE_SPEAKER_TOOL_NAME =>
                                {
                                    crate::tools::nominate_speaker::execute_intercept(
                                        state, input,
                                    )
                                    .await
                                }
                                Some(state) if name
                                    == crate::tools::end_discussion::END_DISCUSSION_TOOL_NAME =>
                                {
                                    crate::tools::end_discussion::execute_intercept(
                                        state, input,
                                    )
                                    .await
                                }
                                _ => (
                                    format!(
                                        "{}: this tool is only available in a group chat session.",
                                        name
                                    ),
                                    true,
                                ),
                            };
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
                            continue;
                        }

                        // 2026-07-07 (`request_mode_change` task):
                        // block the current turn on a mode-switch
                        // request card. Same "control-flow tool"
                        // interception pattern as `ask_user_question`
                        // / `dispatch_subagent` below — the tool
                        // needs `QuestionStore` + `ChatEventSink` +
                        // `db` access that `execute_tool_inner`
                        // doesn't have. We route to
                        // `request_mode_change::execute_blocking`,
                        // which internally:
                        //   1. validates the schema (short-circuits
                        //      with `is_error: true` on boundary
                        //      violations),
                        //   2. noop-checks `target == current` (skips
                        //      card + IPC when nothing to do, PRD R7),
                        //   3. records a `mode_change_requested`
                        //      audit row,
                        //   4. registers a oneshot in
                        //      `QuestionStore` (tagged
                        //      `PendingInteraction::ModeChange`),
                        //   5. emits `mode:change:request` to the
                        //      frontend,
                        //   6. `tokio::select! { cancel | oneshot }`
                        //      until resolve. The actual mode
                        //      application happens in the
                        //      `resolve_mode_change` IPC handler
                        //      (single source of truth for mode
                        //      change side effects, per design §5.5
                        //      "Yolo 二次守门的一致性").
                        //
                        // `is_parallel_eligible` (the L2 whitelist
                        // of `read_file` / `grep` / `glob` /
                        // `list_dir` / `use_skill`) does NOT
                        // include this name → mixed batches fall to
                        // the serial path (this branch) automatically
                        // (per design §6.3 + the `dispatch_subagent`
                        // precedent).
                        //
                        // We accept +1 turn counter cost for the
                        // blocking tool's recover (v1 trade-off —
                        // see PRD §R3).
                        if name == "request_mode_change" {
                            let tool_exec_start = Instant::now();
                            let (content, is_error, _update, exit_code) =
                                crate::tools::request_mode_change::execute_blocking(
                                    input,
                                    &session_id,
                                    id,
                                    session_mode,
                                    &db,
                                    &question_store,
                                    &sink,
                                    &token,
                                    Some(seq),
                                )
                                .await;
                            let duration_ms = tool_exec_start.elapsed().as_millis();
                            if token.is_cancelled() {
                                cancelled = true;
                            } else if !skip_persist {
                                if let Err(e) = permissions::record_tool_executed_audit(
                                    &db,
                                    &session_id,
                                    name,
                                    input,
                                    duration_ms,
                                    exit_code,
                                    Some(seq),
                                )
                                .await
                                {
                                    tracing::warn!(error = %e, "chat: record_tool_executed_audit failed for request_mode_change (non-fatal)");
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
                            if cancelled {
                                break;
                            }
                            continue;
                        }

                        // 2026-07-08 (`07-08-workflow-integration`
                        // Phase 3 Step 3.1): block the current turn
                        // on a workflow state-transition request
                        // card. Mirrors the `request_mode_change`
                        // interception branch above — same blocking-
                        // tool pattern, same
                        // `name == "..."` short-circuit BEFORE the
                        // normal `execute_tool` path.
                        //
                        // The tool needs (a) the
                        // `current_state` + `current_slug` from
                        // the active workflow session's
                        // `WorkflowCtx.current_task` (read once per
                        // turn at the top of the loop), (b) the
                        // `QuestionStore` + `ChatEventSink
                        // ::emit_task_state_transition` (already
                        // threaded through), and (c) the DB pool for
                        // audit. The actual `task.json` mutation +
                        // `from → to` Rust hook dispatch happens in
                        // the `resolve_task_state_transition` IPC
                        // handler (single source of truth for
                        // state-transition side effects — see design
                        // §5.2 M-A / Q9). The tool stays a "request
                        // → wait → return" carrier.
                        //
                        // `is_parallel_eligible` does NOT include
                        // this name → mixed batches fall to the
                        // serial path (this branch) automatically
                        // (mirrors the `request_mode_change`
                        // posture).
                        //
                        // We accept +1 turn counter cost for the
                        // blocking tool's recover (v1 trade-off —
                        // matches the request_mode_change branch).
                        if name == "request_task_state_transition" {
                            let tool_exec_start = Instant::now();
                            // Pull the workflow session's current
                            // task state + slug for the noop check.
                            // `build_workflow_ctx` is called once
                            // per turn at the top of the loop; we
                            // reuse the snapshot here. When the
                            // session is non-workflow the
                            // `WorkflowCtx` is `None` and the tool
                            // gates with `is_error: true` (the
                            // structured message
                            // "no active workflow task").
                            // R3 (07-10-workflow-task-json-hardening):
                            // resolve fresh off disk instead of the frozen
                            // workflow_ctx.current_task snapshot. The agent may
                            // have created / mutated the task earlier in this
                            // same loop (create_task tool / write_file), and the
                            // transition gate must see the real on-disk state —
                            // matches the apply-side resolve_task_state_transition
                            // "read fresh off disk" posture (question.rs). Only
                            // workflow sessions can transition; a workflow session
                            // with no on-disk task short-circuits to (None, None)
                            // → "no active workflow task".
                            let (current_state, current_slug) = if workflow_ctx.is_some() {
                                let fresh = crate::agent::workflow::inject::resolve_current_task(
                                    &current_ctx.worktree_path,
                                )
                                .await;
                                match fresh {
                                    Some(t) => (Some(t.status.clone()), Some(t.slug.clone())),
                                    None => (None, None),
                                }
                            } else {
                                (None, None)
                            };
                            let (content, is_error, _update, exit_code) =
                                crate::tools::request_task_state_transition::execute_blocking(
                                    input,
                                    &session_id,
                                    id,
                                    current_state,
                                    current_slug,
                                    &db,
                                    &question_store,
                                    &sink,
                                    &token,
                                    // C5: validate transitions against the TASK's
                                    // owning plugin, not the session plugin — a dev
                                    // task keeps dev's transition rules even when
                                    // the session is switched to review.
                                    workflow_ctx.as_ref().map(|c| &c.task_workflow_def),
                                    Some(seq),
                                )
                                .await;
                            let duration_ms = tool_exec_start.elapsed().as_millis();
                            if token.is_cancelled() {
                                cancelled = true;
                            } else if !skip_persist {
                                if let Err(e) = permissions::record_tool_executed_audit(
                                    &db,
                                    &session_id,
                                    name,
                                    input,
                                    duration_ms,
                                    exit_code,
                                    Some(seq),
                                )
                                .await
                                {
                                    tracing::warn!(error = %e, "chat: record_tool_executed_audit failed for request_task_state_transition (non-fatal)");
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
                            if cancelled {
                                break;
                            }
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
                            let (content, is_error, cancel_parent, exit_code) =
                                crate::agent::subagent::dispatch::run_subagent(
                                    &provider,
                                    worker_catalog.clone(),
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
                                    // P2.4 C5 (2026-07-22): inject the worker's
                                    // `SubagentEventSink` (replaces the forwarded
                                    // `app_handle`). See the `worker_event_sink`
                                    // param doc on `run_chat_loop`.
                                    worker_event_sink.clone(),
                                    // L3a (2026-06-24): serial path keeps the
                                    // worker's full toolset (write/shell/web for
                                    // general-purpose), gated by `is_worker: true`
                                    // at the ⑨ permission layer. The concurrent
                                    // branch below passes `true` to force read-only.
                                    false,
                                    // L3d (2026-06-25): thread the subagent cache so
                                    // `run_subagent` can look up the dispatched
                                    // subagent across builtin + user + project layers
                                    // (replaces the static `lookup_subagent(name)`).
                                    &subagent_cache,
                                    // L3b (2026-06-27): thread the app data dir so
                                    // `run_subagent` can compute the worker worktree
                                    // path when isolation is active.
                                    &app_data_dir,
                                    // B (2026-06-30): serial dispatch → `parallel=false`,
                                    // isolation falls back to the subagent's default
                                    // (general-purpose now shared).
                                    false,
                                    // 2026-06-30 (`ask_user_question`): thread the
                                    // parent's QuestionStore. Worker can't reach it
                                    // (STRUCTURALLY_DISABLED) but the signature
                                    // requires it.
                                    &question_store,
                                    // W1 Step 2.4: workflow role-gate.
                                    // Serial dispatch — same contract
                                    // as the concurrent + forced paths:
                                    // pass the session's WorkflowCtx
                                    // (None for non-workflow sessions).
                                    workflow_ctx.as_ref(),
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
                                    Some(seq),
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

                        // P3 + P5 (2026-06-29, 06-29-am-p3-tool-recall +
                        // 06-29-am-p5-quality): Tier 1 Hooks — pre-tool pitfall
                        // recall. Runs AFTER `check()` returns Allow and the
                        // dispatch_subagent intercept, BEFORE `execute_tool()`.
                        // P5 introduces the tiered `PitfallRecall`:
                        //   - SoftBlock { hint, memory_id } — verified + full
                        //     trigger-key match + not-yet-blocked this session →
                        //     short-circuit execute_tool, record memory_id,
                        //     surface hint as is_error=false tool_result.
                        //   - Footnote(text) — active / candidate / partial /
                        //     second-hit → execute normally, prepend text.
                        //   - None — no recall.
                        // The dead-loop guard (design D1): the second hit on the
                        // same SoftBlock'd pitfall degrades to Footnote because
                        // the memory_id is now in `soft_blocked`.
                        // 07-06 (am-observability-panel R2b / A9): the
                        // rows-aware sibling carries the raw hit set so we
                        // can emit a `ChatEvent::Recall` for the frontend's
                        // "本次召回" chip. Worker isolation is the same as
                        // the parallel path above.
                        let blocked_snapshot = soft_blocked.lock().await.clone();
                        let (pitfall_recall, pitfall_rows) = permissions::recall_pitfall_with_hits(
                            &db,
                            name,
                            input,
                            &blocked_snapshot,
                        )
                        .await;
                        if !pitfall_rows.is_empty() {
                            crate::agent::memory_recall::emit_recall_event(
                                sink.as_ref(),
                                &rid,
                                &pitfall_rows,
                                "pitfall",
                            );
                        }
                        // SoftBlock: short-circuit. The tool is NOT executed; we
                        // surface the hint as a non-error tool_result so the LLM
                        // re-judges (adjust / abandon / insist). `is_error=false`
                        // so the LLM doesn't think the tool is broken — semantics
                        // are "experience hint", not "tool failed".
                        if let permissions::PitfallRecall::SoftBlock { hint, memory_id } =
                            pitfall_recall
                        {
                            // Record in the session ledger (D1).
                            soft_blocked.lock().await.insert(memory_id);
                            let envelope = crate::agent::helpers::tool_result_envelope(
                                &hint,
                                &current_ctx.worktree_path,
                            );
                            sink.emit_tool_result(&crate::state::ToolResultPayload {
                                request_id: rid.clone(),
                                tool_use_id: id.clone(),
                                content: envelope.clone(),
                                is_error: false,
                            });
                            result_blocks.push(ContentBlock::ToolResult {
                                tool_use_id: id.clone(),
                                content: envelope,
                                is_error: false,
                            });
                            // No tool_executed audit (tool didn't run — RULE-
                            // A-004's intent). No P4 reflect either (no real
                            // outcome). Proceed to the next tool_use.
                            continue;
                        }
                        // Footnote or None: extract optional text for the
                        // prepend-after-execute path.
                        let pitfall_footnote = match pitfall_recall {
                            permissions::PitfallRecall::Footnote(text) => Some(text),
                            permissions::PitfallRecall::None => None,
                            permissions::PitfallRecall::SoftBlock { .. } => {
                                // Unreachable — handled above.
                                None
                            }
                        };

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
                        // P3: prepend the pitfall footnote (if any) to the
                        // tool result content BEFORE the envelope wrap, so the
                        // LLM reads the hint together with the tool output.
                        // is_error is preserved (an error message preceded by
                        // a pitfall hint is still an error).
                        let content = if let Some(footnote) = pitfall_footnote {
                            format!("{}{}", footnote, content)
                        } else {
                            content
                        };
                        // P4 (2026-06-29, 06-29-am-p4-event-reflect):
                        // record this tool_use outcome into the
                        // in-session failure tracker. On the "≥2 fails
                        // then success" pattern, the tracker fires a
                        // fire-and-forget LLM reflection that produces
                        // a pitfall memory row. Skipped on cancel (the
                        // in-flight tool was interrupted, not a real
                        // outcome) — same intent as the audit-skip
                        // below.
                        if !token.is_cancelled() {
                            crate::agent::auto_reflect::try_record_outcome(
                                &failure_tracker,
                                provider.clone(),
                                db.clone(),
                                &rid,
                                &session_id,
                                &current_ctx.project_id,
                                name,
                                input,
                                is_error,
                                &content,
                            )
                            .await;
                        }
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
                                Some(seq),
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
                        if cancelled {
                            break;
                        }
                    }
                } // close DispatchBatch::Serial => { … }
            } // close match dispatch_batch
        }

        // ⑬ loop detection (C2): if this turn tripped the detector,
        // append the hint as a Text block AT THE END of the
        // tool_results. Soft nudge only — execution was NOT skipped
        // and the loop is NOT terminated.
        //
        // WHY the END (not position 0): the user-role message built
        // from `result_blocks` is fed through `wire::chat_request_to_wire`,
        // whose `chat_message_to_wire_messages` fans out each block to a
        // separate wire message — Text → `WireMessage::User { content }`,
        // ToolResult → `WireMessage::Tool { tool_call_id, .. }`, in block
        // order. If the hint Text block sits at position 0, the fan-out
        // produces `user(text) → tool×N`, i.e. a `WireMessage::User`
        // inserted BETWEEN the preceding `assistant(tool_calls)` and the
        // `role: "tool"` messages. OpenAI Chat Completions rejects this
        // with HTTP 400 "An assistant message with 'tool_calls' must be
        // followed by tool messages responding to each 'tool_call_id'"
        // (Anthropic has the same Pair Atomicity rule, see
        // llm-contract.md §469 — but OpenAI enforces the *order*
        // strictly while Anthropic tolerates tool_results inside a user
        // message regardless of interleaving). Putting the hint at the
        // END yields `tool×N → user(text)`, which both protocols accept
        // (the tool pair stays contiguous; the trailing user message is a
        // normal follow-up).
        if let Some(hint) = &loop_hint {
            result_blocks.push(ContentBlock::Text {
                text: format!("⚠️  {}\n", hint),
                cache_control: None,
            });
        }

        if cancelled {
            let result_count = result_blocks.len();
            if !result_blocks.is_empty() {
                let tool_result_msg = ChatMessage {
                    role: Role::User,
                    content: MessageContent::Blocks(result_blocks),
                    speaker: None,
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
            speaker: None,
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
                // 2026-06-21 (R3): thread the last turn's
                // cumulative-per-turn usage into the synthetic
                // terminal `Done`. Pre-R3 this site hard-coded
                // `usage: None`, which caused the worker's
                // `subagent_runs.token_usage_json` to be all
                // zeros on `max_turns` exits (the
                // `c27f3fd7-...` regression). The per-turn
                // `Done{usage: Some(t)}` events from the
                // provider stream were already pushed into the
                // sink's `per_turn_usage` Vec (via
                // `subagent.rs:835-849`); the **sink** is
                // responsible for not double-accumulating this
                // synthetic terminal (R3 sink guard skips the
                // push when `stop_reason` is `max_turns` /
                // `cancelled`). The terminal value flows to
                // `cumulative_usage()` exactly once per turn.
                usage: last_usage_terminal,
            },
        );
    }

    // C1 (07-26-subagent-resume): capture the worker's final messages
    // snapshot for persistence. Only worker runs are resume candidates
    // (`effective_is_worker` gate — the parent session is never resumed),
    // and only the normal completion paths reach here (end_turn /
    // max_turns / loop_terminated). Early `return;` exits (session not
    // found, fatal setup errors) skip this — their messages are partial
    // or empty and unsafe to resume from; resume falls back to fresh
    // dispatch in that case (design §5). The sink's default no-op
    // means non-`SubagentBufferSink` sinks (parent / test mocks) pay
    // nothing.
    if effective_is_worker {
        sink.record_worker_messages(&messages);
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

/// 3b (2026-07-28): C2+ loop-intervention's 4 exit arms all `return`
/// without synthesizing the tool_result for the assistant's
/// `tool_use` blocks, which were already persisted at line 2097 just
/// before the intervention fired. Without this repair, the DB ends
/// with an orphan `assistant(tool_use)` and the next LLM call
/// crashes upstream (Anthropic 2013 / OpenAI 400 "tool result must
/// follow tool call"). Mirrors the existing `cancelled` / `had_error`
/// tail-pair repair at lines 2101 / 2157 — those branches fire on
/// the per-stream cancel/error path; this helper fires on the
/// C2+ intervention path, which sets neither flag.
///
/// `seq` is the seq of the assistant turn already persisted (the
/// turn has not yet `seq += 1`'d, so the synthetic tool_result
/// takes the next slot the next normal turn would skip). When
/// `skip_persist` is true (worker subagent path), the worker does
/// not own the session's lifetime; the parent's own run is
/// unaffected and the helper becomes a no-op for the DB.
async fn finalize_pending_tool_results(
    db: &SqlitePool,
    session_id: &str,
    tool_calls: &[(String, String, serde_json::Value)],
    seq: i64,
    skip_persist: bool,
) {
    if tool_calls.is_empty() {
        return;
    }
    let tool_result_msg = build_synthetic_tool_result_message(tool_calls);
    if skip_persist {
        return;
    }
    if let Err(e) = crate::db::persist_turn(
        db,
        session_id,
        tool_result_msg.role,
        &tool_result_msg.content,
        seq,
        None,
        None,
    )
    .await
    {
        tracing::error!(
            error = %e,
            session_id = %session_id,
            seq = seq,
            tool_calls_len = tool_calls.len(),
            "3b: failed to persist synthetic tool_result after C2+ exit (orphan leak — provider will 2013)"
        );
    }
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
    // `root` is the PROJECT MAIN PATH (not the worker's cwd/worktree_path):
    // for an isolated worker the latter both point at its checkout subtree,
    // so reads of project source files would fail `is_within_root` here and
    // demote the whole read-only batch to serial (magnifying permission
    // prompts). See `PermissionContext.project_main_path`.
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
// L3a (2026-06-24): concurrent dispatch_subagent batch
// ---------------------------------------------------------------------------

/// L3a (2026-06-24): maximum number of `dispatch_subagent` workers
/// allowed to run **concurrently** in a single parent turn. Sourced
/// from the `DELEGATION_MAX_CONCURRENT_CHILDREN` env var; defaults
/// to **3** (mirrors Hermes `_DEFAULT_MAX_CONCURRENT_CHILDREN`).
/// Batches with strictly more than this many dispatches are
/// **hard-rejected** (every dispatch_subagent tool_use returns a
/// `tool_error` tool_result — no truncation, no queuing) so the
/// LLM sees a uniform failure signal and can re-plan (reduce the
/// batch or split across turns).
///
/// Read once per call (no caching) — tests that override the env
/// var via `std::env::set_var` in the same process see the new
/// value on the next batch. `cargo test` runs each test in the
/// same process, so a test that sets the env var MUST unset it
/// (or use a local override via `classify_dispatch_batch` /
/// direct constant in the test).
pub(crate) fn delegation_max_concurrent_children() -> usize {
    match std::env::var("DELEGATION_MAX_CONCURRENT_CHILDREN") {
        Ok(v) => v
            .trim()
            .parse::<usize>()
            .unwrap_or(DEFAULT_DELEGATION_MAX_CONCURRENT_CHILDREN),
        Err(_) => DEFAULT_DELEGATION_MAX_CONCURRENT_CHILDREN,
    }
}

/// The default for `DELEGATION_MAX_CONCURRENT_CHILDREN` when the
/// env var is unset or unparseable. Mirrors Hermes' default of 3
/// (`_DEFAULT_MAX_CONCURRENT_CHILDREN`). Kept as a `pub(crate)`
/// const so tests can assert against it without depending on the
/// env-var read.
pub(crate) const DEFAULT_DELEGATION_MAX_CONCURRENT_CHILDREN: usize = 10;

/// Outcome of classifying a turn's tool_calls batch for the L3a
/// concurrent dispatch path. Computed by [`classify_dispatch_batch`]
/// at the entry of the serial-path branch.
#[derive(Debug)]
pub(crate) enum DispatchBatch {
    /// Fewer than 2 dispatch_subagent tool_uses, OR the batch is a
    /// mix (dispatch + non-dispatch). Falls through to the regular
    /// serial `for` loop unchanged (existing behavior preserved).
    Serial,
    /// A pure batch of `count` dispatch_subagent tool_uses that
    /// exceeds `max_concurrent`. The caller MUST reject the entire
    /// batch with a `tool_error` tool_result for each tool_use
    /// (hard reject, no truncation, no queuing — mirrors Hermes).
    OverLimit { count: usize, max_concurrent: usize },
    /// A pure batch of dispatch_subagent tool_uses, all within
    /// the limit. The caller runs them concurrently via
    /// `FuturesUnordered` (each worker forced read-only).
    /// `count` is kept on the variant for debug logging + future
    /// telemetry even though the concurrent branch reads
    /// `tool_calls.len()` directly.
    Concurrent {
        #[allow(dead_code)]
        count: usize,
    },
}

/// Classify a turn's `tool_calls` for the L3a concurrent
/// dispatch path. Counts `dispatch_subagent` tool_uses vs other
/// tool_uses:
/// - `d >= 2 && other == 0 && d <= max` → [`DispatchBatch::Concurrent`]
/// - `d > max` (pure batch over limit) → [`DispatchBatch::OverLimit`]
/// - anything else (`d <= 1` OR `other > 0`) → [`DispatchBatch::Serial`]
///
/// `max_concurrent` is read from [`delegation_max_concurrent_children`]
/// (env-driven, default 3).
pub(crate) fn classify_dispatch_batch(
    tool_calls: &[(String, String, serde_json::Value)],
    max_concurrent: usize,
) -> DispatchBatch {
    let dispatch_name = crate::agent::subagent::DISPATCH_TOOL_NAME;
    let mut dispatch_count = 0usize;
    let mut other_count = 0usize;
    for (_, name, _) in tool_calls {
        if name == dispatch_name {
            dispatch_count += 1;
        } else {
            other_count += 1;
        }
    }
    if dispatch_count >= 2 && other_count == 0 {
        if dispatch_count > max_concurrent {
            DispatchBatch::OverLimit {
                count: dispatch_count,
                max_concurrent,
            }
        } else {
            DispatchBatch::Concurrent {
                count: dispatch_count,
            }
        }
    } else {
        DispatchBatch::Serial
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::MessageRow;
    use serde_json::json;

    fn db_row(role: &str, content: serde_json::Value, text: &str, seq: i64) -> MessageRow {
        MessageRow {
            id: seq,
            session_id: "s".to_string(),
            role: role.to_string(),
            content,
            text: text.to_string(),
            has_tool_calls: false,
            has_tool_results: false,
            created_at: "t".to_string(),
            seq,
            metadata: None,
            ttfb_ms: None,
            gen_ms: None,
            total_ms: None,
            thinking_ms: None,
            speaker: None,
        }
    }

    fn user_msg(content: MessageContent) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content,
            speaker: None,
        }
    }

    /// tool_result: same tool_use_id → match (the group_chat reload
    /// case this whole fix exists for).
    #[test]
    fn user_message_matches_tool_result_same_id() {
        let blocks = json!([{
            "type": "tool_result",
            "tool_use_id": "call_abc",
            "content": "Floor handed to M3."
        }]);
        let row = db_row("user", blocks.clone(), "", 4);
        let mem = user_msg(serde_json::from_value(blocks).unwrap());
        assert!(user_message_matches(&row, &mem));
    }

    /// tool_result: different tool_use_id → no match (different tool
    /// interaction, must not be treated as the same row).
    #[test]
    fn user_message_matches_tool_result_different_id() {
        let row_blocks = json!([{"type":"tool_result","tool_use_id":"call_abc","content":"x"}]);
        let mem_blocks = json!([{"type":"tool_result","tool_use_id":"call_xyz","content":"x"}]);
        let row = db_row("user", row_blocks, "", 4);
        let mem = user_msg(serde_json::from_value(mem_blocks).unwrap());
        assert!(!user_message_matches(&row, &mem));
    }

    /// plain text: identical text → match.
    #[test]
    fn user_message_matches_plain_text_equal() {
        let row = db_row("user", json!("hello"), "hello", 2);
        let mem = user_msg(MessageContent::Text("hello".to_string()));
        assert!(user_message_matches(&row, &mem));
    }

    /// plain text: different text → no match (a fresh send whose text
    /// differs from the prior persisted row — the normal-chat case).
    #[test]
    fn user_message_matches_plain_text_different() {
        let row = db_row("user", json!("old message"), "old message", 2);
        let mem = user_msg(MessageContent::Text("new message".to_string()));
        assert!(!user_message_matches(&row, &mem));
    }

    /// role mismatch (db row is assistant) → no match.
    #[test]
    fn user_message_matches_wrong_role() {
        let row = db_row("assistant", json!("hello"), "hello", 1);
        let mem = user_msg(MessageContent::Text("hello".to_string()));
        assert!(!user_message_matches(&row, &mem));
    }

    /// malformed DB content JSON → no match (safe default: persist).
    #[test]
    fn user_message_matches_malformed_db_content() {
        let row = db_row("user", json!(42), "", 4); // not a valid MessageContent
        let mem = user_msg(MessageContent::Text("hi".to_string()));
        assert!(!user_message_matches(&row, &mem));
    }
}
