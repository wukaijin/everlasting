//! The `chat` Tauri command — thin pre-flight wrapper.
//!
//! The Tauri command itself is a thin wrapper that:
//! 1. Clones `AppState` handles into a `tauri::async_runtime::spawn`
//!    task.
//! 2. Performs pre-flight catalog resolution (so a missing /
//!    misconfigured model surfaces a clean user-facing error
//!    instead of a stream-time 401).
//! 3. Registers the cancellation token + session→request mapping
//!    (the in-flight cancel hook used by destructive commands).
//! 4. Builds the `AppHandleSink` (the `ChatEventSink` impl that
//!    forwards to Tauri events).
//! 5. Spawns the task and hands control to
//!    [`crate::agent::chat_loop::run_chat_loop`] — the single
//!    agent-loop body shared by production and the integration
//!    tests (P1 RULE-A-006 closure, 2026-06-15).
//!
//! The agent loop body (load session + project, build system
//! prompt, inject B5 memory, per-turn `provider.send` → `select!`
//! over the stream and the cancel token, tool execution under
//! the ⑨ 关 permission layer, persist_turn + TurnComplete emit,
//! synthetic tool_result on cancel, MAX_TURNS fallback) lives
//! entirely in `chat_loop::run_chat_loop`. The duplication
//! between production and test paths that justified
//! `RULE-A-006`'s "partial" status has been removed — the 9
//! `agent_loop_*` integration tests now cover production.

use std::sync::Arc;

use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter, State};
use tokio_util::sync::CancellationToken;

use crate::agent::chat_loop::run_chat_loop;
use crate::agent::provider::{resolve_chat_provider, PreFlightError};
use crate::llm::{ChatEvent, ChatMessage};
use crate::state::{AppState, ChatEventPayload};

// ---------------------------------------------------------------------------
// The per-turn latency helpers (`instant_delta_ms` / `build_turn_latency`)
// were moved into `chat_loop.rs` together with the agent loop body
// during the RULE-A-006 closure migration (2026-06-15). See
// `chat_loop::build_turn_latency` for the implementation.
// ---------------------------------------------------------------------------

/// `chat` Tauri command entry. Returns immediately after spawning
/// the agent loop; the actual work runs in the background and
/// communicates with the frontend via `chat-event` / `tool:call` /
/// `tool:result` Tauri events.
///
/// The agent loop body itself (load session + project, build
/// system prompt, inject B5 memory, per-turn `provider.send` →
/// `select!` over the stream and the cancel token, tool execution
/// under the ⑨ 关 permission layer, persist_turn + TurnComplete
/// emit, synthetic tool_result on cancel, MAX_TURNS fallback)
/// lives in [`crate::agent::chat_loop::run_chat_loop`] (P1
/// RULE-A-006 closure, 2026-06-15). The duplication between
/// production and test paths has been removed; the 9
/// `agent_loop_*` integration tests now cover production.
#[tauri::command]
pub async fn chat(
    request_id: String,
    session_id: String,
    messages: Vec<ChatMessage>,
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
    // D3 PR3 (2026-06-17): resend context. When the user clicks
    // Resend on an existing user message, the frontend fires
    // `chat` again with the same content (the original user
    // message is still in `messages`) plus this optional seq.
    // The agent loop's user-message persist site detects the
    // flag and writes a `resend_message` audit row (best-
    // effort). `None` for normal first-time sends. Field
    // name is snake_case to match the other IPC args; serde
    // auto-converts the JS-side `resendSeq: number | null`.
    #[allow(non_snake_case)]
    resendSeq: Option<i64>,
    // explicit-agent-dispatch (2026-06-30): `@@<agent> <task>` prefix
    // parsed by the frontend (`chat.ts send()`). When `Some`,
    // `run_chat_loop`'s turn-1 prefix short-circuits the LLM and
    // dispatches the named worker directly (no `provider.stream`).
    // `None` for normal sends. Mutually exclusive with `resendSeq`
    // (a resend never carries a forced dispatch).
    #[allow(non_snake_case)]
    forcedDispatch: Option<crate::agent::subagent::ForcedDispatch>,
) -> Result<(), String> {
    let tool_defs = state.tools.clone();
    let db = state.db.clone();
    let catalog = state.catalog.clone();
    let cancellations = state.cancellations.clone();
    let session_active_request = state.session_active_request.clone();
    let inflight_exits = state.inflight_exits.clone();
    let read_guard = state.read_guard.clone();
    let memory_cache = state.memory_cache.clone();
    let skill_cache = state.skill_cache.clone();
    let permission_asks = state.permission_asks.clone();
    // L1a (2026-06-19): clone the cross-request background-shell
    // registry BEFORE the spawn so the move closure doesn't
    // capture a borrowed `state`. Threaded into `run_chat_loop` so
    // the agent loop can drain completion notifications each turn
    // and the 3 L1a tools can call into it from `ToolContext`.
    let background_shells = state.background_shells.clone();
    // L3d (2026-06-25): clone the subagent cache so the agent loop
    // can build the dynamic `dispatch_subagent` enum + look up
    // workers by name. Same closure-capture pattern as the other
    // `Arc<...>` handles above.
    let subagent_cache = state.subagent_cache.clone();
    // L3b (2026-06-27): clone the app data dir so the spawn closure
    // can capture it by value (State<'_> is borrowed — the closure
    // must not borrow from it).
    let app_data_dir = state.app_data_dir.clone();
    let rid = request_id;
    // The `app` clone lives on through `AppHandleSink` (built
    // below); the pre-flight error path also uses `app.emit`
    // directly so we keep the receiver alive across both
    // pre-flight + spawn.

    // PR1 pre-flight: look up the catalog for the default model.
    // The failure modes map 1:1 to PRD §Q2's locked-in user-facing
    // messages, surfaced as `ChatEvent::Error` so the frontend can
    // render the same toast path it uses for other LLM errors. We
    // do this BEFORE registering the cancellation token +
    // session_active_request entry because a pre-flight failure
    // is synchronous (no LLM call has started), so there is
    // nothing to cancel.
    let resolved = match lookup_provider_for_session(&session_id, &db, &catalog).await {
        Ok(r) => r,
        Err(err) => {
            let (msg, category) = err.user_message_and_category();
            tracing::warn!(
                request_id = %rid,
                session_id = %session_id,
                error = %msg,
                "chat: pre-flight failed (catalog)"
            );
            let payload = ChatEventPayload {
                request_id: rid,
                event: ChatEvent::Error { message: msg, category },
            };
            app.emit("chat-event", payload).map_err(|e| e.to_string())?;
            return Ok(());
        }
    };
    let provider: Arc<dyn crate::llm::Provider> = resolved.provider;
    // C3 (2026-06-12): capture the context_window for the agent
    // loop's per-turn token-budget compaction. We move this into
    // the spawn closure below so every turn sees the same window
    // (it's a property of the chosen model and is stable within
    // one chat invocation — the user can't change models mid-chat).
    let context_window: u32 = resolved.context_window;
    tracing::info!(
        request_id = %rid,
        session_id = %session_id,
        model = %resolved.model_display_name,
        provider = %resolved.provider_display_name,
        protocol = ?provider.protocol(),
        "chat: provider resolved"
    );

    // Register a cancellation token for this request. The frontend's
    // Stop button calls `cancel_chat(rid)` which fetches this token
    // and triggers it; the agent loop's `tokio::select!` notices and
    // bails out. The entry is removed by the spawn task on every
    // exit path (normal / error / cancel / max_turns) — see the
    // guard at the end of the spawn closure.
    let token = CancellationToken::new();
    {
        let mut map = cancellations.lock().await;
        map.insert(rid.clone(), token.clone());
    }
    // Also register this session → request_id mapping so
    // destructive operations (delete_session, detach_worktree,
    // delete_worktree) can find and cancel the in-flight stream.
    // The entry is removed by the CancellationGuard on Drop.
    {
        let mut map = session_active_request.lock().await;
        map.insert(session_id.clone(), rid.clone());
    }
    // RULE-E-005 (2026-06-15): create the "agent loop exited"
    // signal. The Receiver goes into `inflight_exits` keyed by
    // request_id, so `cancel_inflight_for_session` can hand it to
    // a destructive command, which awaits it (via
    // `await_inflight_exit`) before deleting the worktree/session.
    // The Sender moves into the spawn closure and fires when
    // `run_chat_loop` returns — i.e. the loop has fully exited,
    // including any in-flight tool that was already dispatched when
    // cancel fired. Closing the race where the loop writes into a
    // just-deleted worktree.
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
    {
        let mut map = inflight_exits.lock().await;
        map.insert(rid.clone(), done_rx);
    }

    // P1 RULE-A-006 (2026-06-14): wrap the AppHandle in a
    // ChatEventSink so the agent loop body can dispatch through
    // the trait. The `permissions::check` Tier 3 `permission:ask`
    // emit is the one place inside the agent loop body that
    // needs this trait (the rest of the body still uses
    // `app_handle.emit` directly — see the original closure for
    // the chat-event / tool:call / tool:result emits). The
    // testable variant in `chat_loop.rs` uses the same trait
    // for ALL emits, so tests get a single MockEmitter sink.
    let sink: Arc<dyn crate::state::ChatEventSink> = Arc::new(crate::state::AppHandleSink {
        app: app.clone(),
    });
    let sink_for_spawn = sink.clone();

    tauri::async_runtime::spawn(async move {
        // Agent loop body is now unified with `chat_loop::run_chat_loop`
        // (P1 RULE-A-006 closure, 2026-06-15). The original inline
        // ~1000-line closure was a faithful copy of `run_chat_loop`;
        // routing production through it lets the 9 `agent_loop_*`
        // integration tests cover the production path. Drift hazard
        // is gone: any change to the agent loop body happens in ONE
        // place.
        //
        // Pre-flight + cancellation-token registration + sink build
        // stay in this command (they're the Tauri-specific bits);
        // `run_chat_loop` owns the per-turn loop + DB persistence +
        // all four emit channels (chat-event / tool:call /
        // tool:result / permission:ask) and the `CancellationGuard`
        // that cleans the cancel maps on every exit path.
        run_chat_loop(
            tool_defs,
            provider,
            context_window,
            rid.clone(),
            session_id.clone(),
            messages,
            sink_for_spawn,
            db,
            cancellations,
            session_active_request,
            read_guard,
            memory_cache,
            skill_cache,
            permission_asks,
            token,
            // D3 PR3 (2026-06-17): pass the resend context
            // through so the user-message persist site can
            // fire the `resend_message` audit row when set.
            // `None` for normal sends (the common case).
            resendSeq,
            // L1a (2026-06-19): cross-request registry. Threaded
            // through so the 3 L1a tools can start / query / kill
            // background processes and the agent loop can drain
            // completion notifications each turn.
            background_shells.clone(),
            // B6 Subagent (2026-06-19, review #4): `None` keeps
            // the default `MAX_TURNS` (50) budget for the
            // production chat path. Worker agents (PR1b) pass
            // `Some(20)` to bound their own turn budget.
            None,
            // B6 Subagent (PR1b review #2): production chat owns
            // the session's "active request" slot, so the guard's
            // Drop must clear it. Workers pass `true` to skip.
            false,
            // B6 Subagent (PR1b): production chat persists every
            // turn normally. Workers pass `true` so their
            // intermediate turns stay in-memory only (the
            // SubagentBufferSink captures them; PR2 persists the
            // transcript into `subagent_runs`). Production MUST
            // persist — the user's turns are the source of truth.
            false,
            // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production
            // chat is never a worker. `Some(false)` makes the
            // production-style default explicit at the call site;
            // inside `run_chat_loop` this falls through to the
            // session-row mode (Edit/Plan/Yolo) with
            // `PermissionContext.is_worker = false` — Tier 4 ask
            // is reachable (permission:ask modal works normally).
            Some(false),
            // B6 PR3 (2026-06-20, PR2 hotfix): forward the
            // `app: AppHandle` so `run_subagent` can wire the
            // worker's `SubagentBufferSink` with a live IPC emit
            // path (the `subagent:event` channel). The 22nd
            // `run_chat_loop` parameter; tests pass `None`.
            Some(app.clone()),
            // 2026-06-21 fix (B6 review defect A): production
            // chat is never a worker, so the parent's
            // `assemble_system_prompt(mode_prefix, base_prompt)`
            // path runs unchanged (`None` override → the loop
            // builds the prompt from the project + session
            // row). The worker nested call (in `run_subagent`)
            // passes `Some(assemble_subagent_prompt(def, &task))`
            // to fully replace the parent's prompt with the
            // worker's `SubagentDef.system_prompt`. See the
            // doc comment on `run_chat_loop.system_prompt_override`
            // for the full rationale + the review reference.
            None,
            // 2026-06-22 (RULE-FrontSubagent-003 fix): production
            // chat is never a worker, so `worker_run_id` is
            // `None`. The nested `run_subagent` call passes
            // `Some(worker_run_id_opt)` so the worker's
            // `PermissionContext.worker_run_id` is populated and
            // `ask_path` can route the interactive ask via the
            // `"worker:<worker_run_id>"` permission session id.
            None,
            // L3d (2026-06-25): thread the subagent cache so the
            // loop's per-turn tool list construction can append the
            // dynamic `dispatch_subagent` ToolDef
            // (`definition_with_cache`) and `run_subagent` can look
            // up workers by name across builtin + user + project
            // layers.
            subagent_cache,
            // 2026-06-26 (task 06-26-subagent-per-run-grant):
            // production chat is the parent path — pass `None` so
            // the parent's `PermissionContext.run_grants` is `None`
            // and the Tier 4 grant-check branches in `check.rs`
            // skip the cache lookup entirely. Parent session grants
            // continue to use the `session_tool_permissions` DB
            // table (unchanged behavior). Only the worker nested
            // call (in `run_subagent`) passes `Some(Arc<...>)`.
            None,
            // L3b (2026-06-27): production chat is the parent path —
            // pass `None` so the loop builds the worktree_path from
            // the session row (the parent's session worktree, or
            // the project root if no worktree). Only the isolated
            // worker nested call (in `run_subagent`) passes
            // `Some(worker_worktree_path)` to redirect the worker's
            // tools into an isolated checkout.
            None,
            // L3b (2026-06-27): thread the app data dir so the
            // dispatch_subagent interceptor (`run_subagent`) can
            // compute the worker worktree path when isolation is
            // active. Pass-through — the agent loop body itself
            // does not read this.
            app_data_dir,
            // explicit-agent-dispatch: thread the user-forced
            // dispatch into the loop's turn-1 short-circuit
            // (trailing `forced_dispatch` parameter).
            forcedDispatch,
        )
        .await;
        // RULE-E-005 (2026-06-15): the agent loop has fully exited.
        // Signal any destructive command awaiting the
        // `cancel_inflight_for_session` receiver so it proceeds
        // with the delete. `send` is `Err` if no one is awaiting
        // (no destructive op, or it already drained + timed out) —
        // both are fine, we ignore it.
        let _ = done_tx.send(());
        // Clean up the `inflight_exits` entry (no-op if
        // `cancel_inflight_for_session` already took it). This lives
        // here, not in `CancellationGuard`, because the exit signal
        // is the chat command's concern, not the agent loop's.
        inflight_exits.lock().await.remove(&rid);
    });

    Ok(())
}

/// PR1 catalog lookup for the default model.
///
/// Resolve the provider for a chat request, preferring the
/// session's own `model_id` (per-session model override) and
/// falling back to the global `default_model_id`.
///
/// Resolution chain:
/// 1. Read `sessions.model_id` from DB (if set → use it)
/// 2. If NULL or points to missing model → fall back to global
///    `app_config.default_model_id`
/// 3. If still not found → DB slow path (`resolve_chat_provider`)
async fn lookup_provider_for_session(
    session_id: &str,
    db: &SqlitePool,
    catalog: &Arc<tokio::sync::RwLock<crate::state::ProviderCatalog>>,
) -> Result<ResolvedChatProviderWrapper, PreFlightError> {
    // Determine which model_id to use: session override or global default.
    let model_id = resolve_model_id_for_session(session_id, db).await?;

    // Resolve display names + api_key pre-flight from DB.
    let models = crate::db::list_models(db).await.map_err(|e| {
        tracing::error!(error = %e, "lookup_provider_for_session: list_models failed");
        PreFlightError::NoModel
    })?;
    let mwp = models
        .into_iter()
        .find(|m| m.model.id == model_id)
        .ok_or(PreFlightError::NoModel)?;
    let providers = crate::db::list_providers(db).await.map_err(|e| {
        tracing::error!(error = %e, "lookup_provider_for_session: list_providers failed");
        PreFlightError::ProviderMissing
    })?;
    let provider_row = providers
        .into_iter()
        .find(|p| p.id == mwp.model.provider_id)
        .ok_or(PreFlightError::ProviderMissing)?;

    // Pre-flight: empty api_key still applies on the catalog
    // path (the catalog might have been built with an empty
    // key if the user just saved Settings).
    //
    // RULE-D-001: 区分"未填 key"(has_key=false) vs "解密失败"
    // (has_key=true 但明文空, 机器变化), 文案不同.
    if provider_row.api_key.is_empty() {
        return Err(if provider_row.has_key {
            PreFlightError::DecryptFailed {
                provider_display_name: provider_row.display_name.clone(),
            }
        } else {
            PreFlightError::EmptyApiKey {
                provider_display_name: provider_row.display_name.clone(),
            }
        });
    }

    // Fast path: catalog hit. Acquire read lock (concurrent
    // reads don't block each other).
    {
        let guard = catalog.read().await;
        if let Some(arc_provider) = guard.get(&model_id) {
            return Ok(ResolvedChatProviderWrapper {
                provider: arc_provider.clone(),
                model_display_name: mwp.model.display_name.clone(),
                provider_display_name: provider_row.display_name.clone(),
                context_window: mwp.model.context_window,
            });
        }
    }

    // Slow path: catalog miss (e.g. model added/changed but
    // rebuild not yet complete). Fall back to the legacy DB
    // resolver and wrap the resulting Box into an Arc.
    tracing::warn!(
        model_id = %model_id,
        "lookup_provider_for_session: catalog miss, falling back to DB resolver"
    );
    let resolved = resolve_chat_provider(db).await?;
    Ok(ResolvedChatProviderWrapper {
        provider: Arc::from(resolved.provider),
        model_display_name: resolved.model_display_name,
        provider_display_name: resolved.provider_display_name,
        // C3: preserve the context_window from the catalog row we
        // already looked up (`mwp.model.context_window`). The
        // slow-path `resolve_chat_provider` returns a different
        // `ResolvedChatProvider` type that doesn't carry the row
        // metadata, so we read it from the row we already have.
        context_window: mwp.model.context_window,
    })
}

/// Resolve the effective model_id for a session: prefer the
/// session's own `model_id` override, fall back to the global
/// `default_model_id`.
async fn resolve_model_id_for_session(
    session_id: &str,
    db: &SqlitePool,
) -> Result<String, PreFlightError> {
    // Try session's own model_id first.
    let session = crate::db::load_session(db, session_id).await.map_err(|e| {
        tracing::error!(error = %e, "resolve_model_id_for_session: load_session failed");
        PreFlightError::NoModel
    })?;
    if let Some(mid) = session.and_then(|s| s.session.model_id) {
        // Verify the model still exists in the catalog (not deleted).
        let models = crate::db::list_models(db).await.map_err(|e| {
            tracing::error!(error = %e, "resolve_model_id_for_session: list_models failed");
            PreFlightError::NoModel
        })?;
        if models.iter().any(|m| m.model.id == mid) {
            return Ok(mid);
        }
        tracing::warn!(
            session_id = %session_id,
            model_id = %mid,
            "resolve_model_id_for_session: session model_id points to deleted model, falling back to default"
        );
    }

    // Fallback: global default.
    crate::db::get_config_value(db, "default_model_id")
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "resolve_model_id_for_session: get_config_value failed");
            PreFlightError::NoModel
        })?
        .ok_or(PreFlightError::NoModel)
}

/// Thin wrapper holding the resolved provider as an Arc (so we
/// can share the catalog's pre-built instance) plus the display
/// names used for logging.
pub struct ResolvedChatProviderWrapper {
    pub provider: Arc<dyn crate::llm::Provider>,
    pub model_display_name: String,
    pub provider_display_name: String,
    /// C3 (2026-06-12): the model's `context_window` in tokens,
    /// sourced from `ModelRow.context_window`. Used by
    /// [`crate::agent::context::compact_messages`] in the agent
    /// loop to decide when to trim old messages. Always set from
    /// the resolved catalog row, so callers can rely on it being
    /// non-zero for any model the user can actually pick.
    pub context_window: u32,
}