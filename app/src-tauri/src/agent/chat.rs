//! The `chat` Tauri command — thin pre-flight wrapper.
//!
//! The Tauri command itself is a thin wrapper that constructs the
//! `AppHandleSink` (the `ChatEventSink` impl that forwards to Tauri
//! events) and delegates to [`chat_inner`] — the shared orchestration
//! shared by the Tauri command path and the daemon HTTP handler
//! (P2.3 C5, 2026-07-21, task `07-20-remote-access-daemon-split`).
//!
//! [`chat_inner`] owns:
//! 1. Clones `AppState` handles into a spawn task.
//! 2. Performs pre-flight catalog resolution (so a missing /
//!    misconfigured model surfaces a clean user-facing error
//!    instead of a stream-time 401).
//! 3. Registers the cancellation token + session→request mapping
//!    (the in-flight cancel hook used by destructive commands).
//! 4. Spawns the task and hands control to
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
use tauri::{AppHandle, State};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::agent::chat_loop::run_chat_loop;
use crate::agent::provider::{resolve_chat_provider, PreFlightError};
use crate::agent::subagent::{AppHandleSubagentSink, SubagentEventSink};
use crate::error::AppCommandError;
use crate::llm::{ChatEvent, ChatMessage};
use crate::state::{AppState, ChatEventPayload, ChatEventSink, ProviderCatalog};

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
/// P2.3 C5 (2026-07-21): this is now a thin wrapper that builds the
/// Tauri `AppHandleSink` and forwards to [`chat_inner`] — the
/// transport-agnostic orchestration shared with the daemon's HTTP
/// `chat` handler (`daemon::routes::agent`). The Tauri path passes
/// `Some(app)` so `run_chat_loop` can wire the worker's
/// `SubagentBufferSink` with a live IPC emit; the daemon path passes
/// `None` (渐进方案 — worker events stay buffer-only on the HTTP path
/// until the full `SubagentEventSink` injection lands; parent chat is
/// fully wired via `HttpSseSink`).
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
    #[allow(non_snake_case)] resendSeq: Option<i64>,
    // explicit-agent-dispatch (2026-06-30): `@@<agent> <task>` prefix
    // parsed by the frontend (`chat.ts send()`). When `Some`,
    // `run_chat_loop`'s turn-1 prefix short-circuits the LLM and
    // dispatches the named worker directly (no `provider.stream`).
    // `None` for normal sends. Mutually exclusive with `resendSeq`
    // (a resend never carries a forced dispatch).
    #[allow(non_snake_case)] forcedDispatch: Option<crate::agent::subagent::ForcedDispatch>,
) -> Result<ChatAcceptance, AppCommandError> {
    // Build the `ChatEventSink` adapter BEFORE delegating so the
    // pre-flight error path inside `chat_inner` also emits through
    // the trait (no direct `app.emit` — closes the bypass flagged in
    // REVIEW-remote-access-research-2026-07-20 §P0-D). The sink
    // carries the only `app.clone()` it needs; `chat_inner` reuses
    // the same trait object for both pre-flight + the spawn closure.
    let sink: Arc<dyn ChatEventSink> = Arc::new(crate::state::AppHandleSink { app: app.clone() });
    // P2.4 C5 (2026-07-22): the worker's `SubagentEventSink` — Tauri
    // path forwards worker `subagent:event` over IPC. The daemon path
    // injects `HttpSseSubagentSink` instead (daemon/routes/agent.rs);
    // both flow through `new_with_event_sink` in dispatch.rs.
    let worker_event_sink: Arc<dyn SubagentEventSink> =
        Arc::new(AppHandleSubagentSink { app: app.clone() });
    chat_inner(
        state.inner(),
        request_id,
        session_id,
        messages,
        sink,
        Some(state.inner().catalog.clone()),
        worker_event_sink,
        resendSeq,
        forcedDispatch,
    )
    .await
}

/// Transport-agnostic chat orchestration (P2.3 C5, 2026-07-21).
///
/// Shared by the Tauri `chat` command (sink = `AppHandleSink`,
/// `app_opt = Some(app)`) and the daemon HTTP `chat` handler
/// (`daemon::routes::agent`: sink = `HttpSseSink`, `app_opt = None`).
/// Both construct their own sink + decide `app_opt`, then hand off
/// here for the identical pre-flight + cancellation-registration +
/// `run_chat_loop` spawn.
///
/// **spawn runtime**: `tokio::spawn` (was `tauri::async_runtime::spawn`
/// pre-P2.3). In the Tauri process, Tauri 2's `async_runtime` is a
/// tokio multi-thread runtime, so a Tauri command executes inside a
/// tokio runtime context — `tokio::spawn` resolves to the same handle
/// `tauri::async_runtime::spawn` used, i.e. zero behavior change for
/// the Tauri path. In the daemon process (`#[tokio::main]`), this is
/// the native spawn. Unifying on `tokio::spawn` is what lets one
/// function serve both transports.
///
/// **`app_opt` (渐进方案)**: the 22nd `run_chat_loop` param. Tauri
/// passes `Some(app)` so worker dispatch can wire
/// `SubagentBufferSink` with a live IPC emit. The daemon path passes
/// `None` — workers still run (dispatch_subagent executes, DB records
/// the run), but worker events stay buffer-only (no `subagent:event`
/// SSE emit). Full `SubagentEventSink` injection (so the daemon path
/// gets live worker events via `HttpSseSubagentSink`) is deferred to
/// a follow-up — see task `07-20-remote-access-daemon-split` implement.md
/// C5 "完整 subagent sink 注入" 复盘点.
#[allow(clippy::too_many_arguments)]
/// F1 消息队列(2026-08-25):`chat` 入口的受理结果(design §2/§8)。
///
/// - `Started`:session 空闲,请求已认领 slot 并开跑 —— 响应形状与
///   F1 前的 unit 语义等价(流照常走事件通道)。
/// - `Queued`:session 忙,消息已入队,本次 RPC 无流;`id` 是队列项
///   uuid(R8 撤销/退回的稳定寻址键 —— 位置随增删漂移,前端占位
///   必须按 id 而非 position 寻址,评审 Round 2 P1 修复);
///   `position` 为 1-based 队尾位次(前端排队徽标展示用)。
///
/// wire 形状 `{ "status": "started" }` / `{ "status": "queued",
/// "id": "...", "position": N }`(serde tag)。向后兼容:闲时字段
/// 叠加而非 rename,旧前端把非 unit 返回值忽略也不影响流。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ChatAcceptance {
    Started,
    Queued { id: String, position: usize },
}

// See DEBT.md RULE-ARGS-001 (parameter-object epic, tracked separately).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn chat_inner(
    state: &Arc<AppState>,
    request_id: String,
    session_id: String,
    messages: Vec<ChatMessage>,
    sink: Arc<dyn ChatEventSink>,
    // P2.4 C5 (2026-07-22): worker dispatch context (replaces
    // `app_opt: Option<AppHandle>`). Forwarded to `run_chat_loop`'s
    // `worker_catalog` + `worker_event_sink`. Tauri passes
    // (state.catalog, AppHandleSubagentSink); daemon passes
    // (state.catalog, HttpSseSubagentSink) — the daemon path now
    // gets live worker `subagent:event` (was buffer-only pre-C5).
    worker_catalog: Option<Arc<RwLock<ProviderCatalog>>>,
    worker_event_sink: Arc<dyn SubagentEventSink>,
    resend_seq: Option<i64>,
    forced_dispatch: Option<crate::agent::subagent::ForcedDispatch>,
) -> Result<ChatAcceptance, AppCommandError> {
    let tool_defs = state.tools.clone();
    // B1 (2026-08-16): image-attachment caps, enforced at the shared
    // entry so both transports return a clear error instead of a
    // mid-loop failure. Two tiers (design §3.1, review P1-4 修正版):
    // ① per-turn NEW images ≤ 10 (the staging limit, mirrored
    // server-side); ② request TOTAL (new + history rebuilt) ≤ 20 —
    // history images ride every request, so a long image-heavy
    // conversation could otherwise exceed the providers' per-request
    // image limits (both Anthropic and OpenAI cap at 20).
    {
        let mut total_images = 0usize;
        for m in &messages {
            if let Some(refs) = &m.attachments {
                total_images += refs.len();
            }
        }
        let new_this_turn = messages
            .iter()
            .rev()
            .find(|m| m.role == crate::llm::types::Role::User)
            .and_then(|m| m.attachments.as_ref())
            .map(|refs| refs.len())
            .unwrap_or(0);
        if new_this_turn > 10 {
            return Err(anyhow::anyhow!(
                "单轮最多附加 10 张图片(本次 {} 张),请删减后再发送",
                new_this_turn
            )
            .into());
        }
        if total_images > 20 {
            return Err(anyhow::anyhow!(
                "请求内图片总数超过 20 张上限(历史 {} 张 + 新图),建议新建 session 或删减历史图片",
                total_images
            )
            .into());
        }
    }
    let db = state.db.clone();
    let catalog = state.catalog.clone();
    let cancellations = state.cancellations.clone();
    let session_active_request = state.session_active_request.clone();
    let inflight_exits = state.inflight_exits.clone();
    let read_guard = state.read_guard.clone();
    let memory_cache = state.memory_cache.clone();
    let skill_cache = state.skill_cache.clone();
    let permission_asks = state.permission_asks.clone();
    // 2026-06-30 (`ask_user_question` task): clone the parallel
    // `QuestionStore` from `AppState`. Source-of-truth for the
    // in-flight `ask_user_question` oneshot map (frontend IPC
    // resolves through `commands::question::resolve_tool_question`).
    // Must be cloned BEFORE the spawn closure so the captured
    // value doesn't outlive the borrowed `state`'s lifetime
    // (the borrow checker rejects `state.foo` references inside
    // an `async move` block on `tokio::spawn`).
    let question_store = state.question_store.clone();
    // W1 (Workflow integration, Phase 0 Step 0.5 — 2026-07-08):
    // build the per-session workflow context BEFORE the spawn.
    // Async, resolves `sessions.workflow_enabled` from the DB
    // (Step 0.1 column) plus a `.everlasting/tasks/` listing
    // (Step 0.4). Returns `None` for non-workflow sessions —
    // the agent loop's per-turn helper short-circuits and the
    // session byte-identity vs. pre-Step-0.5 is preserved.
    //
    // We deliberately resolve BEFORE the spawn closure (vs.
    // doing it inside `tokio::spawn`) so the resolved value can
    // be moved into the closure by value without re-entering
    // async land per entry. The cost is bounded (~10 ms warm)
    // — same shape as other pre-spawn clones (background_shells
    // / subagent_cache).
    let workflow_ctx = match crate::agent::workflow::build_workflow_ctx(&db, &session_id).await {
        Ok(ctx) => ctx,
        Err(e) => {
            // Defensive: DB error during context resolution
            // MUST NOT abort the chat loop. The hook is
            // session-level opt-in; if we can't resolve, we
            // drop workflow context for this turn and the
            // session behaves like a non-workflow session.
            // The `e` carries DB error metadata we surface
            // as a warn so the user can see it in the dev
            // console / Phase 3 audit log.
            tracing::warn!(
                session_id = %session_id,
                error = %e,
                "build_workflow_ctx failed; this turn proceeds without workflow context (per-turn helper will short-circuit)",
            );
            None
        }
    };
    // L1a (2026-06-19): clone the cross-request background-shell
    // registry BEFORE the spawn so the move closure doesn't
    // capture a borrowed `state`. Threaded into `run_chat_loop` so
    // the agent loop can drain completion notifications each turn
    // and the 3 L1a tools can call into it from `ToolContext`.
    let background_shells = state.background_shells.clone();
    // Group chat (07-29-group-chat): resolve the group-chat context
    // BEFORE the spawn, mirroring `build_workflow_ctx`. `None` for
    // classic-chat sessions (zero overhead). When `Some`, the spawn
    // closure routes to `run_group_chat_loop` instead of the plain
    // `run_chat_loop`.
    let group_chat_ctx =
        match crate::agent::group_chat::build_group_chat_ctx(&db, &session_id).await {
            Ok(ctx) => ctx,
            Err(e) => {
                tracing::warn!(
                    session_id = %session_id,
                    error = %e,
                    "build_group_chat_ctx failed; proceeding as classic chat",
                );
                None
            }
        };
    // L3d (2026-06-25): clone the subagent cache so the agent loop
    // can build the dynamic `dispatch_subagent` enum + look up
    // workers by name. Same closure-capture pattern as the other
    // `Arc<...>` handles above.
    let subagent_cache = state.subagent_cache.clone();
    // L3b (2026-06-27): clone the app data dir so the spawn closure
    // can capture it by value (state is borrowed — the closure
    // must not borrow from it).
    let app_data_dir = state.app_data_dir.clone();
    // D (2026-08-14, `08-14-c7d-tools-stub-registration`): clone the
    // session-keyed stub loaded-set registry for the spawn closure.
    // The classic-chat path's `run_chat_loop` uses it for stubify
    // (第 4 环) + the load_tool_schemas / 直呼自愈 interception
    // (跨 request 粘性 — AC4)。
    let stub_loaded = state.stub_loaded.clone();
    let rid = request_id;
    let sink_for_spawn = sink.clone();

    // ------------------------------------------------------------------
    // F1 消息队列(2026-08-25):统一数据路径路由临界区(design §2)。
    //
    // 「查忙 + 入队 + 认领 slot + 注册 token/done」在同一临界区一次
    // 完成 —— claim 即注册,消灭旧序"3a 取消 → 注册"之间的查忙竞态
    // 窗。锁纪律:先 `message_queues` 后 `session_active_request`,
    // 全仓固定;区内只有锁获取,无 DB/网络 await。
    //
    // - 忙 → 仅入队,返回 `Queued{position}`(本次 RPC 无流)。
    // - 闲 → 入队 + 认领,继续走 preflight 并 spawn 驱动器;preflight
    //   失败时回滚全部注册并弹出本条队列项(`pushed_id` 寻址)。
    // - 群聊 / 开关关闭 / 无 user 尾条 → Legacy:行为与 F1 前逐字节
    //   一致(下方 3a 防御性取消 + 既有注册序列原样保留)。
    //
    // 统一数据路径下**每一条发送都先进队**(含触发驱动器的这条),
    // 驱动器只从队列消费 —— 错误终止后的滞留项与新消息天然 FIFO
    // 同注(P1-1 修正的落点),D-D 入口不变量(尾条未落库)对注入
    // 轮自动成立。
    // ------------------------------------------------------------------
    let message_queues = state.message_queues.clone();
    let queue_enabled =
        match crate::db::config::get_config_value(&db, "message_queue_enabled").await {
            Ok(Some(v)) => v != "false",
            _ => true, // fail-open,同 memory_digest / tools_stub 先例
        };
    let token = CancellationToken::new();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
    let mut done_rx = Some(done_rx);
    let mut drive_claimed = false;
    let mut queued_position: Option<usize> = None;
    let mut pushed_id: Option<String> = None;
    'routing: {
        if group_chat_ctx.is_some() || !queue_enabled {
            break 'routing;
        }
        let Some(tail) = messages
            .last()
            .filter(|m| m.role == crate::llm::types::Role::User)
            .cloned()
        else {
            break 'routing;
        };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or_default();
        let mut qmap = message_queues.lock().await;
        let busy = {
            let active = session_active_request.lock().await;
            active.contains_key(&session_id)
        };
        let q = qmap.entry(session_id.clone()).or_default();
        let pushed = match crate::agent::message_queue::push(q, tail, now_ms) {
            Ok(id) => id,
            Err(e) => {
                // AC6:队列满(20)。用户可感知错误,不入队不认领。
                tracing::warn!(session_id = %session_id, error = %e, "chat: enqueue rejected");
                drop(qmap);
                return Err(anyhow::anyhow!(e.to_string()).into());
            }
        };
        pushed_id = Some(pushed);
        queued_position = Some(q.len());
        if !busy {
            session_active_request
                .lock()
                .await
                .insert(session_id.clone(), rid.clone());
            cancellations
                .lock()
                .await
                .insert(rid.clone(), token.clone());
            if let Some(rx) = done_rx.take() {
                inflight_exits.lock().await.insert(rid.clone(), rx);
            }
            drive_claimed = true;
        }
    }
    if let Some(position) = queued_position {
        if !drive_claimed {
            // id = 本条队列项的 uuid(`push` 已生成)——R8 撤销/退回
            // 的稳定寻址键,前端占位存 id、按 id 直达(评审 Round 2)。
            return Ok(ChatAcceptance::Queued {
                id: pushed_id.unwrap_or_default(),
                position,
            });
        }
    }

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
            // transport-abstraction 2026-07-20 (P1.3): route the
            // pre-flight error through the same `ChatEventSink`
            // trait the rest of the loop uses, instead of calling
            // `app.emit` directly. The sink logs emit failures
            // (`AppHandleSink::emit_chat_event`) — the IPC
            // double-fault `AppCommandError` shape from the
            // previous `app.emit(...).map_err(...)?` would not
            // translate to the Phase 2 HTTP transport anyway.
            sink.emit_chat_event(&ChatEventPayload {
                request_id: rid.clone(),
                event: ChatEvent::Error {
                    message: msg,
                    category,
                },
            });
            // F1(2026-08-25):Drive 路径在路由临界区已认领 slot +
            // 注册 token/done,preflight 失败必须全部回滚,否则
            // session 被一个永远不会跑的"假在途请求"卡死。
            if drive_claimed {
                cancellations.lock().await.remove(&rid);
                session_active_request.lock().await.remove(&session_id);
                inflight_exits.lock().await.remove(&rid);
                if let Some(id) = &pushed_id {
                    // best-effort:并发入队可能已在我们身后排队,
                    // 按 uuid 只弹自己这条。
                    let _ =
                        crate::agent::message_queue::remove_by_id(&message_queues, &session_id, id)
                            .await;
                }
            }
            return Ok(ChatAcceptance::Started);
        }
    };
    let provider: Arc<dyn crate::llm::Provider> = resolved.provider;
    // C3 (2026-06-12): capture the context_window for the agent
    // loop's per-turn token-budget compaction. We move this into
    // the spawn closure below so every turn sees the same window
    // (it's a property of the chosen model and is stable within
    // one chat invocation — the user can't change models mid-chat).
    let context_window: u32 = resolved.context_window;
    // 08-20-turn-usage-event-quota-view WP2: provider 归因随同一
    // wrapper 穿进 spawn closure(同 context_window 的稳定性论证)。
    let provider_id: Option<String> = resolved.provider_id;
    tracing::info!(
        request_id = %rid,
        session_id = %session_id,
        model = %resolved.model_display_name,
        provider = %resolved.provider_display_name,
        protocol = ?provider.protocol(),
        "chat: provider resolved"
    );

    // F1(2026-08-25):统一数据路径下,Drive 请求的 slot/token/done
    // 注册已前移进上方路由临界区(claim 即注册)。此处的 3a 防御性
    // 取消 + 注册序列仅对 **Legacy 路径**(queue 关闭 / 群聊 / 无
    // user 尾条)保留,行为与 F1 前逐字节一致。
    if !drive_claimed {
        // 3a (2026-07-28, defensive depth): cancel any prior in-flight
        // chat on this session BEFORE registering the new one. Mirrors
        // the invariant already enforced by every destructive command
        // (delete_session / clear_session_messages / edit_user_message /
        // detach_worktree / delete_worktree) — see `commands/sessions.rs`
        // and `commands/worktree.rs` for the same pattern.
        //
        // The frontend's `chatStore.send` early-returns on
        // `isCurrentSessionStreaming` (chat.ts:952), and
        // `editMessage` / `resendMessage` / `retryChat` each explicitly
        // cancel before re-invoking. So in the current code base this
        // cancel-then-await is a no-op for the normal paths. It is,
        // however, load-bearing defense in depth: any future caller
        // (or a race where the frontend guard mis-fires) that leaks a
        // second chat request into the same session would otherwise
        // strand the old agent loop holding stale
        // `session_active_request` + `cancellations` state, and the
        // old loop's tool execution would continue writing into a
        // session that's already moved on.
        //
        // `await_inflight_exit` returns immediately on `None` (no
        // prior in-flight), so the common cold-start path is a no-op.
        let exit_rx = crate::agent::helpers::cancel_inflight_for_session(
            &cancellations,
            &session_active_request,
            &inflight_exits,
            &session_id,
        )
        .await;
        crate::agent::helpers::await_inflight_exit(exit_rx, "chat").await;

        // Register a cancellation token for this request. The frontend's
        // Stop button calls `cancel_chat(rid)` which fetches this token
        // and triggers it; the agent loop's `tokio::select!` notices and
        // bails out. The entry is removed by the spawn task on every
        // exit path (normal / error / cancel / max_turns) — see the
        // guard at the end of the spawn closure.
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
        //
        // (F1:the channel itself is created up-front beside the routing
        // critical section; here we only register the receiver.)
        if let Some(rx) = done_rx.take() {
            let mut map = inflight_exits.lock().await;
            map.insert(rid.clone(), rx);
        }
    }

    // P1 RULE-A-006 (2026-06-14): the sink was passed in by the
    // caller (constructed before pre-flight). The `permissions::check`
    // Tier 3 `permission:ask` emit is the one place inside the agent
    // loop body that needs this trait. The testable variant in
    // `chat_loop.rs` uses the same trait for ALL emits, so tests
    // get a single MockEmitter sink.

    tokio::spawn(async move {
        // Agent loop body is now unified with `chat_loop::run_chat_loop`
        // (P1 RULE-A-006 closure, 2026-06-15). The original inline
        // ~1000-line closure was a faithful copy of `run_chat_loop`;
        // routing production through it lets the 9 `agent_loop_*`
        // integration tests cover the production path. Drift hazard
        // is gone: any change to the agent loop body happens in ONE
        // place.
        //
        // Pre-flight + cancellation-token registration + sink build
        // stay in `chat_inner` (they're the transport-agnostic bits);
        // `run_chat_loop` owns the per-turn loop + DB persistence +
        // all four emit channels (chat-event / tool:call /
        // tool:result / permission:ask) and the `CancellationGuard`
        // that cleans the cancel maps on every exit path.
        //
        // Group chat (07-29-group-chat): when the session is
        // group_chat, route to the outer orchestrator instead. It
        // internally calls `run_chat_loop` per speaker (moderator +
        // participants), so all the loop's guarantees (persistence,
        // cancel, tool exec) still hold.
        if let Some(gc_ctx) = group_chat_ctx {
            crate::agent::group_chat_loop::run_group_chat_loop(
                tool_defs,
                context_window,
                provider_id,
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
                resend_seq,
                background_shells.clone(),
                worker_catalog.clone(),
                worker_event_sink.clone(),
                subagent_cache,
                app_data_dir,
                question_store,
                gc_ctx,
            )
            .await;
        } else if drive_claimed {
            // F1 消息队列(2026-08-25):队列驱动器路径(design §1)。
            // 统一数据路径下每条发送都入队;驱动器只从队列消费,
            // 正常结束后 drain 非空则自动续轮。内层 run_chat_loop
            // 全部双抑制 guard(skip_session_active + skip_cancellations),
            // slot/token/done 生命周期由驱动器自持(退出时统一清理)。
            run_queue_driver(QueueDriverDeps {
                tool_defs,
                provider,
                context_window,
                provider_id,
                rid: rid.clone(),
                session_id: session_id.clone(),
                inner_sink: sink_for_spawn,
                db,
                cancellations: cancellations.clone(),
                session_active_request: session_active_request.clone(),
                read_guard,
                memory_cache,
                skill_cache,
                permission_asks,
                token: token.clone(),
                resend_seq,
                forced_dispatch,
                background_shells: background_shells.clone(),
                worker_catalog: worker_catalog.clone(),
                worker_event_sink: worker_event_sink.clone(),
                subagent_cache,
                app_data_dir,
                question_store,
                workflow_ctx,
                stub_loaded,
                queues: message_queues.clone(),
            })
            .await;
        } else {
            run_chat_loop(
                tool_defs,
                provider,
                context_window,
                provider_id,
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
                resend_seq,
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
                // P2.4 C5 (2026-07-22): forward the worker dispatch
                // context (catalog + event sink) to `run_chat_loop`'s
                // 22nd/23rd params. Closes the daemon-path gap — worker
                // `subagent:event` now reaches the transport live (was
                // buffer-only pre-C5).
                worker_catalog.clone(),
                worker_event_sink.clone(),
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
                // project_main_override (2026-07-29): production chat is
                // the parent path → `None` (the loop falls back to
                // worktree_path, which IS the project root here). Only the
                // isolated worker nested call passes Some(project_main).
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
                forced_dispatch,
                // 2026-06-30 (`ask_user_question` task): pass the
                // `QuestionStore` cloned above (captured-by-value in
                // the spawn closure). The `ask_user_question`
                // interception in `chat_loop.rs` reads it; workers
                // won't (the tool is in `STRUCTURALLY_DISABLED` so
                // the worker's tool list strips it).
                question_store,
                // W1 (Workflow integration, Phase 0 Step 0.5
                // — 2026-07-08): per-session workflow context.
                // Eagerly resolved at IPC entry (DB read + at
                // most a handful of small task.json reads,
                // ~10 ms cost on a warm pool); cached for the
                // entire 200-turn loop. `None` for non-workflow
                // sessions — the per-turn helper short-circuits
                // and the session behaves byte-identically to
                // pre-Step-0.5.
                workflow_ctx,
                // Group chat (07-29-group-chat): `None` here at the
                // classic-chat call site. The group-chat orchestration
                // (Phase 3.5) wraps this call — when the session is
                // group_chat, `run_group_chat_loop` is entered instead
                // and IT constructs the `SharedTurnState` to thread
                // through its own `run_chat_loop` calls. So the
                // classic-chat path stays None (the nominate/end
                // interception no-ops if somehow invoked).
                None,
                // Group chat (07-29-group-chat, Phase 4 TODO-A): the
                // classic-chat path never carries a speaker — its
                // assistant turns persist with `speaker = NULL` (the
                // pre-Phase 4 default, unchanged). The
                // `run_group_chat_loop` orchestration passes
                // `Some("moderator")` / `Some(participant.name)` for
                // its own dispatch sites.
                None,
                // D (2026-08-14): thread the stub loaded-set registry
                // (classic-chat path — stubify + interception use it).
                stub_loaded,
                // F1 queue driver: the driver suppresses both guard
                // cleanups on every inner round (see skip_cancellations
                // param doc). This single-shot call site is only reached
                // by legacy paths (queue disabled / group chat) — keep
                // guard-owned cleanup.
                false,
            )
            .await;
        } // end else (classic-chat path)
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

    Ok(ChatAcceptance::Started)
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
pub(crate) async fn lookup_provider_for_session(
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
                provider_id: Some(mwp.model.provider_id.clone()),
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
        provider_id: Some(mwp.model.provider_id.clone()),
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
    /// 08-20-turn-usage-event-quota-view WP2: the resolved model's
    /// provider row id (`ModelRow.provider_id` already at hand on
    /// both resolution arms). Threaded into `run_chat_loop` →
    /// `drive_turn` → the Done-arm `upsert_turn_trace_token` as
    /// the `turn_trace.provider_id` attribution (5h-window quota
    /// aggregation's grouping key). `Provider` trait has no id
    /// member, hence the explicit field.
    pub provider_id: Option<String>,
}

// ---------------------------------------------------------------------------
// F1 消息队列(2026-08-25):队列驱动器(design §1)
// ---------------------------------------------------------------------------

/// 驱动器依赖包。与 [`run_chat_loop`] 的参数一一对应(外加队列句柄
/// 与 token),仅由 `chat_inner` 的 spawn 闭包与集成测试构造。不并入
/// `run_chat_loop` 签名 —— 驱动器是 loop 之上的编排层,不是新的
/// loop 形态。
pub(crate) struct QueueDriverDeps {
    pub(crate) tool_defs: Vec<crate::llm::types::ToolDef>,
    pub(crate) provider: Arc<dyn crate::llm::Provider>,
    pub(crate) context_window: u32,
    pub(crate) provider_id: Option<String>,
    pub(crate) rid: String,
    pub(crate) session_id: String,
    /// 未包装的 inner sink;每轮经 [`DriverSink`] 包装后传入内层
    /// run,Done 被吞、真退出时补发(design §3)。
    pub(crate) inner_sink: Arc<dyn ChatEventSink>,
    pub(crate) db: sqlx::SqlitePool,
    pub(crate) cancellations:
        Arc<tokio::sync::Mutex<std::collections::HashMap<String, CancellationToken>>>,
    pub(crate) session_active_request:
        Arc<tokio::sync::Mutex<std::collections::HashMap<String, String>>>,
    pub(crate) read_guard: crate::tools::read_guard::ReadGuard,
    pub(crate) memory_cache: Arc<crate::memory::MemoryCache>,
    pub(crate) skill_cache: Arc<crate::skill::loader::SkillCache>,
    pub(crate) permission_asks: crate::agent::permissions::PermissionStore,
    pub(crate) token: CancellationToken,
    pub(crate) resend_seq: Option<i64>,
    pub(crate) forced_dispatch: Option<crate::agent::subagent::ForcedDispatch>,
    pub(crate) background_shells: crate::background_shell::DefaultRegistry,
    pub(crate) worker_catalog: Option<Arc<RwLock<ProviderCatalog>>>,
    pub(crate) worker_event_sink: Arc<dyn SubagentEventSink>,
    pub(crate) subagent_cache: Arc<crate::agent::subagent::SubagentCache>,
    pub(crate) app_data_dir: std::path::PathBuf,
    pub(crate) question_store: crate::agent::question_store::QuestionStore,
    pub(crate) workflow_ctx: Option<crate::agent::workflow::WorkflowCtx>,
    pub(crate) stub_loaded: std::sync::Arc<crate::tools::stub::StubRegistry>,
    pub(crate) queues: crate::agent::message_queue::SharedQueues,
}

/// 队列驱动器主循环。
///
/// ```text
/// loop {
///   drained = drain(queue)                    // FIFO 全队
///   空 → break(Done 补发)
///   round>0 → emit TurnContinuation{count}    // 前端续轮渲染边界
///   input   = reload_messages(db) ++ drained  // D-D:尾条未落库
///   run_chat_loop(..., 双抑制 guard, ...)
///   cancelled → 清空队列, break
///   errored / 续轮触顶 → 队列保留, break
/// }
/// 真结束(非 error)→ 补发最后被吞的 Done
/// 清理 cancellations[rid] + session_active_request[sid]
/// ```
///
/// # 为什么不用独立 rid/新请求实现续轮
///
/// 复用群聊"单 rid 保活跨内层 turn"机制(design §3):前端 finalize
/// 只认 Done,中间轮的 Done 被 [`DriverSink`] 吞掉,单请求因此跨多
/// 轮存活,TurnComplete/trace/token 记账全部按既有 per-turn 契约
/// 工作,前端零新增 wire 语义(除 additive 的 TurnContinuation)。
pub(crate) async fn run_queue_driver(deps: QueueDriverDeps) {
    use crate::agent::message_queue::{DriverSink, MAX_CONTINUATION_ROUNDS};

    let (driver_sink, status) = DriverSink::new(deps.inner_sink.clone());
    let driver_sink: Arc<dyn ChatEventSink> = Arc::new(driver_sink);
    let mut round = 0usize;

    loop {
        let drained = crate::agent::message_queue::drain_all(&deps.queues, &deps.session_id).await;
        if drained.is_empty() {
            // claim 时至少压入一条,正常不应为空;防御性兜底。
            break;
        }
        if round > 0 {
            // 续轮渲染边界:先于本轮任何内层事件(sink 保序)。
            // 见 ChatEvent::TurnContinuation 文档 —— 不能用 Start 兼任。
            deps.inner_sink.emit_chat_event(&ChatEventPayload {
                request_id: deps.rid.clone(),
                event: ChatEvent::TurnContinuation {
                    count: drained.len(),
                },
            });
        }
        // turn 输入 = DB 权威历史 + 排队尾条。与群聊 D-B 决策同构:
        // 不信任客户端历史,reload 是唯一事实源(B1 attachments 经
        // metadata 重建)。drained 尾条未落库,D-D 持久化点自然写入。
        let mut turn_messages =
            crate::agent::group_chat_loop::reload_messages(&deps.db, &deps.session_id).await;
        turn_messages.extend(drained.iter().map(|qm| qm.message.clone()));

        // resend 审计与 @@ 强制派发都是 round-0 语义(D8 保证 @@
        // 永不入队 —— 这里只是纵深防御:round>0 一律 None)。
        let (round_resend, round_forced) = if round == 0 {
            (deps.resend_seq, deps.forced_dispatch.clone())
        } else {
            (None, None)
        };

        run_chat_loop(
            deps.tool_defs.clone(),
            deps.provider.clone(),
            deps.context_window,
            deps.provider_id.clone(),
            deps.rid.clone(),
            deps.session_id.clone(),
            turn_messages,
            driver_sink.clone(),
            deps.db.clone(),
            deps.cancellations.clone(),
            deps.session_active_request.clone(),
            deps.read_guard.clone(),
            deps.memory_cache.clone(),
            deps.skill_cache.clone(),
            deps.permission_asks.clone(),
            deps.token.clone(),
            round_resend,
            deps.background_shells.clone(),
            // max_turns=None(生产预算);skip_session_active=true 与
            // skip_cancellations=true —— slot/rid 条目跨轮存活,由驱
            // 动器在最终退出时统一清理(guard 全程被抑制)。
            None,
            true,
            // skip_persist=false:注入轮必须落库(用户消息是事实源)。
            false,
            Some(false),
            deps.worker_catalog.clone(),
            deps.worker_event_sink.clone(),
            None,
            None,
            deps.subagent_cache.clone(),
            None,
            None,
            None,
            deps.app_data_dir.clone(),
            round_forced,
            deps.question_store.clone(),
            deps.workflow_ctx.clone(),
            None,
            None,
            deps.stub_loaded.clone(),
            true,
        )
        .await;

        // --- 轮间边界判定(design §4 取消矩阵)---
        if deps.token.is_cancelled() {
            // Stop / defense-in-depth 替换:清空队列(PRD R7)。
            crate::agent::message_queue::clear_session(&deps.queues, &deps.session_id).await;
            break;
        }
        let errored = status.lock().expect("driver status lock").errored;
        if errored {
            // 错误终止:非 user 过错,队列保留(PRD R3);不补发 Done
            // —— Error 事件已触发前端 finalize,再补会双重终结。
            break;
        }
        if round + 1 >= MAX_CONTINUATION_ROUNDS {
            tracing::warn!(
                session_id = %deps.session_id,
                rounds = MAX_CONTINUATION_ROUNDS,
                "chat: continuation round cap hit; retaining queue"
            );
            break;
        }
        round += 1;
    }

    // 真结束(非 error 路径):补发最后一轮被吞的 Done —— 这是整个
    // 请求唯一到达前端的 Done,前端在此 finalize。
    {
        let st = status.lock().expect("driver status lock");
        if !st.errored {
            if let Some((stop_reason, usage)) = st.last_done.clone() {
                deps.inner_sink.emit_chat_event(&ChatEventPayload {
                    request_id: deps.rid.clone(),
                    event: ChatEvent::Done { stop_reason, usage },
                });
            }
        }
    }

    // 驱动器自持生命周期收尾:guard 全程双抑制,两个 map 条目由这里
    // 统一注销(done_tx.send + inflight_exits 清理沿用 spawn 闭包既
    // 有尾段,不变)。反搁浅语义:清理即最终态 —— 若此后仍有并发入
    // 队,路由临界区会看到"闲"并为新消息 spawn 新驱动器,滞留项随
    // 其首轮 drain 一起注入(统一数据路径保证顺序)。
    deps.cancellations.lock().await.remove(&deps.rid);
    // rid 守卫(评审 Round 2 P3):仅当 slot 仍指向本驱动器时才注销 ——
    // 近不可能的"运行中被 legacy 3a 替换重注册"场景下,无条件按
    // session 删会误摘新请求的注册。
    deps.session_active_request
        .lock()
        .await
        .retain(|_sid, r| r != &deps.rid);
}
