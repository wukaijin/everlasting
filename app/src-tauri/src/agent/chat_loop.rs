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
use std::sync::Arc;
use std::time::{Duration, Instant};

use sqlx::SqlitePool;
// A5+ (2026-07-04): LLM first-byte-safe retry open.
use crate::llm::retry::{RetrySink, RetryingEvent};

use crate::agent::helpers::{
    build_synthetic_tool_result_message, emit_chat_event_via_sink, persist_turn_cwd,
};
use crate::agent::loop_detection;
use crate::agent::question_store::{
    InteractionResponse, PendingInteraction, Question, QuestionAnswer, QuestionOption,
    ToolQuestionPayload,
};
use crate::agent::MAX_TURNS;
use crate::llm::{ChatEvent, ChatMessage, ContentBlock, LlmErrorCategory, MessageContent, Role};
use crate::projects::boundary::is_within_root;
use crate::state::{ChatEventSink, ToolCallPayload};
use std::collections::VecDeque;

// 08-08-a-class-chat-loop-split: `chat_loop.rs` is now a hub. The per-turn
// 初始化 / 单轮驱动 / 工具派发三大段已平移到 `chat_loop/{init,drive,tools}.rs`
// 子模块(沿用 `subagent/dispatch/` 的 Rust 2018 file + directory 模式)。
// hub 通过 `pub(crate) use <sub>::*;` 全量 re-export,故 `run_chat_loop` 仍以
// 裸名调用 `prepare_loop_state` / `drive_turn` / `dispatch_tool_calls` /
// `finalize_turn`,destructure `LoopInit` / `DriveTurnOutcome` /
// `DispatchOutcome` —— 对调用方零变化。hub 自身保留:run_chat_loop 主体、
// LlmRetrySink、user_message_matches / dd_guard_hit (D-D 守卫)、以及
// latency / persist 失败 / load_for_session / finalize_pending_tool_results
// / L2 parallel-eligibility / DispatchBatch 分类等辅助函数。
pub(crate) mod background_escalation;
pub(crate) mod drive;
pub(crate) mod init;
pub(crate) mod suite;
pub(crate) mod tools;
#[allow(unused_imports)]
pub(crate) use drive::*;
#[allow(unused_imports)]
pub(crate) use init::*;
#[allow(unused_imports)]
pub(crate) use suite::*;
#[allow(unused_imports)]
pub(crate) use tools::*;

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
pub(crate) struct LlmRetrySink {
    pub(crate) sink: Arc<dyn ChatEventSink>,
    pub(crate) session_id: String,
    pub(crate) rid: String,
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
            &self.session_id,
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
pub(crate) fn user_message_matches(db_row: &crate::db::MessageRow, mem_msg: &ChatMessage) -> bool {
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

/// A D-D entry-guard hit: the tail user message is judged already
/// persisted — the persist site must skip `persist_turn` and anchor
/// on `seq` instead.
pub(crate) struct DdGuardHit {
    /// seq of the DB row the tail user message maps to (the at_file
    /// injection anchor; for rewrite products it is not consumed —
    /// see `snapshot`).
    pub(crate) seq: i64,
    /// The `last_user_snapshot` the persist site should return.
    /// `None` for rewrite products (P0-3) so the at_file injection
    /// condition (`injections` non-empty && snapshot `Some`) stays
    /// false — a rewrite row is another speaker's remark, NOT a human
    /// input, and must not trigger `@file` expansion (it would write
    /// the injection manifest to the wrong seq row + misplace the
    /// FileInjections event). `Some(content)` for speaker-None hits
    /// (behavior unchanged).
    pub(crate) snapshot: Option<MessageContent>,
}

/// The D-D entry guard's "already persisted?" decision for a tail user
/// message (design D-F + 08-07-group-chat-role-history-isolation
/// P0-1/P0-3). Returns `Some(hit)` when the caller should treat the
/// message as already in the DB (skip `persist_turn`) — which only
/// ever happens in a group-chat scope (`group_chat_state` present).
///
/// Two match paths:
/// - `msg.speaker.is_some()`: the tail is a `role_history` rewrite
///   product — another speaker's utterance rewritten as `role:user`.
///   The original DB row is an assistant row, so content-matching
///   against user rows can never hit. Anchor on the tail-most user
///   row's seq instead (semantically the closest position; P0-3 makes
///   the seq unreachable as an injection anchor). Safety: in the
///   group-chat codebase a user row with `speaker == Some` can ONLY be
///   a rewrite product — human prompts, tool_results and synthetic
///   tool_results all carry `speaker == None`, so the signal cannot
///   misfire.
/// - `speaker == None`: original content match (`user_message_matches`).
pub(crate) fn dd_guard_hit(
    skip_persist: bool,
    group_chat_state: Option<&crate::tools::nominate_speaker::SharedTurnState>,
    loaded_messages: &[crate::db::MessageRow],
    msg: &ChatMessage,
) -> Option<DdGuardHit> {
    if skip_persist {
        return None;
    }
    let seq = if msg.speaker.is_some() {
        loaded_messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|db_row| db_row.seq)
    } else {
        loaded_messages
            .iter()
            .filter(|m| m.role == "user")
            .find(|db_row| user_message_matches(db_row, msg))
            .map(|db_row| db_row.seq)
    };
    seq.filter(|_| group_chat_state.is_some())
        .map(|seq| DdGuardHit {
            seq,
            snapshot: if msg.speaker.is_some() {
                None
            } else {
                Some(msg.content.clone())
            },
        })
}

pub async fn run_chat_loop(mut request: ChatLoopRequest, deps: ChatLoopDeps, role: CallerRole) {
    // -------------------------------------------------------------
    // RULE-ARGS-001 本地重绑定：与旧 38 位参同名、同类型的一次性 owned
    // 重绑定，所有权来源改为三个套件对象。克隆次数与迁移前的实参传递
    // 一一对应 —— 函数体自此刻起保持旧名引用逐字节不变（design D3）。
    // 各字段的历史契约文档（WP2/D3/L1a/B6/RULE-A-014/RULE-A-015 等）
    // 见 suite.rs 对应字段的 doc comment。
    let provider = request.provider.clone();
    let context_window = request.context_window;
    let rid = request.rid.clone();
    let session_id = request.session_id.clone();
    let sink = request.sink.clone();
    let max_turns = request.max_turns;
    let mut workflow_ctx = request.workflow_ctx.clone();
    let group_chat_state = request.group_chat_state.clone();

    let db = deps.db.clone();
    let cancellations = deps.cancellations.clone();
    let session_active_request = deps.session_active_request.clone();
    let read_guard = deps.read_guard.clone();
    let memory_cache = deps.memory_cache.clone();
    let skill_cache = deps.skill_cache.clone();
    let permission_asks = deps.permission_asks.clone();
    let token = deps.token.clone();
    let background_shells = deps.background_shells.clone();
    let question_store = deps.question_store.clone();
    let subagent_cache = deps.subagent_cache.clone();

    let worker_catalog = role.worker_catalog.clone();
    let worker_event_sink = role.worker_event_sink.clone();
    let forced_dispatch = role.forced_dispatch.clone();
    let app_data_dir = role.app_data_dir.clone();
    let skip_persist = role.skip_persist;
    let skip_session_active = role.skip_session_active;
    let skip_cancellations = role.skip_cancellations;

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
        // F1 queue driver (2026-08-25): continuation rounds pass
        // `true` so both maps survive across inner rounds — see the
        // `skip_cancellations` param doc at the signature tail.
        skip_cancellations,
    };
    // D (2026-08-14, `08-14-c7d-tools-stub-registration`): 每
    // request 读一次 `tools_stub_enabled`(best-effort,缺省 = 开;
    // `"false"` 才关 — fail-open 语义安全,stub 不删能力只延迟披露)。
    // 单行 kv 读,μs 级;与 `workflow_enabled`(session 列)同数量级。
    // 关 → drive.rs 第 4 环直通 + 不 append `load_tool_schemas`
    // (回滚通道,AC5)。
    let stub_on = match crate::db::config::get_config_value(&db, "tools_stub_enabled").await {
        Ok(Some(v)) => v != "false",
        _ => true,
    };
    // unified-context-budget WP2 (2026-08-19): 关卡⑤硬卡开关,同款
    // fail-open 缺省开语义(读法与 `tools_stub_enabled` 一致;关 =
    // send 前行为与 WP1 完全一致,回滚通道 prd AC6)。
    let budget_on = match crate::db::config::get_config_value(&db, "context_budget_enabled").await {
        Ok(Some(v)) => v != "false",
        _ => true,
    };
    // RULE-ARGS-001：原 19 参收敛为三套件引用。messages 经 mem::take
    // 移交（prepare 内加工后经 LoopInit.messages 回流，与旧按值移交等价；
    // request 整体随后续 helper 引用保持存活）。
    let init = match prepare_loop_state(&mut request, &deps, &role).await {
        Ok(i) => i,
        Err(()) => return,
    };
    // `head_sha` is computed in `prepare_loop_state` (mirroring the pre-split
    // init), then unconditionally refreshed at the top of every turn (L1635),
    // so the initial value is never read — hence the allow. Kept for parity
    // with the original init sequence (no behavior change).
    #[allow(unused_assignments)]
    let LoopInit {
        mut messages,
        mut seq,
        loaded_session,
        project,
        worktree_path,
        current_ctx,
        last_cwd,
        mut last_usage_terminal,
        failure_tracker,
        soft_blocked,
        session_mode,
        effective_is_worker,
        mut permission_ctx,
        mode_prefix,
        model_briefs,
        mut head_sha,
        mut system_prompt,
        memory_token,
        digest_on,
        // C3 摘要压缩 PR2 (08-18-llm-context-compaction):水位锚点 +
        // 合成头长度 + 摘要 gate。`summary_anchor` 是跨 turn 可变状态
        // —— drive_turn 每次成功压缩后更新,经 `DriveTurnOutcome`
        // 回写(同 `loop_hit_count` 线程模式,覆盖同 loop 内二次压缩,
        // 评审 P1-1 修正)。
        mut summary_anchor,
        synthetic_prefix_len,
        compaction_on,
        // unified-context-budget WP1 (2026-08-19): system/@files 切片 +
        // 同请求 spans(D10 临时产物,WP2 budget gate 消费)。请求常量,
        // 每 turn 原样穿给 drive_turn。
        system_token,
        at_files_token,
        at_file_spans,
        // WP2: 当前 turn user 槽位(裁剪保护边界)+ memory 目录态快照
        // (臂 3 回退目标)。
        current_user_msg_idx,
        memory_catalog_blocks,
    } = init;

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
        emit_chat_event_via_sink(&sink, &session_id, &rid, &ChatEvent::Start);
        sink.emit_tool_call(&ToolCallPayload {
            request_id: rid.clone(),
            session_id: session_id.clone(),
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
            session_id: session_id.clone(),
            tool_use_id: tool_use_id.clone(),
            content: envelope_str,
            is_error,
            images: None,
        });

        // ⑥ the worker summary is also this turn's assistant text —
        //    emit it as a Delta so the frontend renders an assistant
        //    message carrying the result (the SubagentDrawer already
        //    showed the live worker output; this lands the summary in
        //    the main conversation).
        emit_chat_event_via_sink(
            &sink,
            &session_id,
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
            attachments: None,
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
                emit_persist_failure(&sink, &session_id, &rid, &e);
                return;
            }
            emit_chat_event_via_sink(
                &sink,
                &session_id,
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
            &session_id,
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
    // MAX_TURNS softcap (08-18-max-turns-softcap): the fixed
    // `for turn in 1..=turn_limit` became a budget-carrying `loop`.
    // `turn` still starts at 1 and increments by exactly 1 per
    // executed turn (drive_turn's turn param / loop_intervention id
    // / DBG semantics unchanged) — the softcap ask consumes one
    // turn number (budget+1) without executing a turn body.
    // Worker subagents break at the boundary and take today's hard
    // terminal (AC4: worker + group chat stay hard-capped; group
    // chat never enters this loop — it has its own outer
    // MAX_ORCHESTRATION_ROUNDS loop in group_chat_loop.rs).
    // Env hook EVERLASTING_SOFTCAP_TURN_BOUNDARY (QA/live, design §1)
    // lowers the FIRST ask point by replacing the initial budget —
    // applied once here, NOT re-read per iteration (a per-turn re-read
    // would re-ask every turn after a「继续」grant, since the env
    // value stays fixed while the budget grows). Grants extend the
    // budget naturally, so the second ask lands at boundary+200.
    // Unset/unparseable → `turn_limit`, byte-identical to today's
    // `turn > turn_limit` judgment.
    let mut turns_budget = softcap_boundary(turn_limit);
    // RULE-ARGS-001（design D2/TurnFrame）：每轮常量派生帧 + 轮计数 +
    // softcap force 标志并入帧，由轮间逻辑推进（替代旧裸局部
    // `turn` / `force_compaction`）。cwd 状态对独立为 TurnHot。
    let mut frame = TurnFrame {
        loaded_session: &loaded_session,
        project: &project,
        worktree_path: &worktree_path,
        mode_prefix,
        model_briefs: &model_briefs,
        session_mode,
        effective_is_worker,
        stub_on,
        budget_on,
        memory_token,
        digest_on,
        synthetic_prefix_len,
        compaction_on,
        system_token,
        at_files_token,
        at_file_spans: &at_file_spans,
        current_user_msg_idx,
        memory_catalog_blocks: &memory_catalog_blocks,
        turn: 0usize,
        force_compaction: false,
    };
    let mut hot = TurnHot {
        current_ctx,
        last_cwd,
    };
    // Set when the softcap helper already emitted the terminal
    // (Done{max_turns} / Done{cancelled}) so the post-loop tail
    // does not double-emit. Worker break leaves it false → the
    // tail emits exactly as today.
    let mut softcap_terminal_emitted = false;
    loop {
        frame.turn += 1;
        if frame.turn > turns_budget {
            if effective_is_worker || group_chat_state.is_some() {
                // Worker / group-chat path: hard cap unchanged —
                // break into the post-loop hard terminal (worker
                // messages capture included, 1096-1098 original
                // semantics). Group chat reuses run_chat_loop for
                // each per-speaker segment with max_turns=1
                // (group_chat_loop.rs §54) — its budget exhaustion
                // is the 30-round orchestration's business, NOT a
                // softcap ask (R4: 仅单聊主 loop; a softcap here
                // would also hang speaker turns that end on
                // tool_use with nobody watching the store).
                break;
            }
            match ask_turn_limit_softcap(
                &request,
                &deps,
                &role,
                TurnBudgetAsk {
                    turn: frame.turn,
                    turns_budget,
                    compaction_on,
                    seq,
                    last_usage_terminal,
                    last_cwd: hot.last_cwd.as_deref(),
                },
            )
            .await
            {
                SoftcapOutcome::Continue => {
                    turns_budget += TURN_LIMIT_GRANT;
                }
                SoftcapOutcome::CompactContinue => {
                    turns_budget += TURN_LIMIT_GRANT;
                    frame.force_compaction = true;
                }
                SoftcapOutcome::Terminal => {
                    softcap_terminal_emitted = true;
                    break;
                }
            }
            // Re-arm: the consumed turn number was budget+1, the
            // next iteration starts budget+2.
            continue;
        }
        // E2 trace (2026-07-14): update the per-turn seq on the
        // permission context so `record_audit` can pass it to
        // `record_audit_event` for audit turn alignment.
        let drive_outcome = match drive_turn(
            &request,
            &deps,
            &role,
            &mut frame,
            TurnCarry {
                messages,
                seq,
                head_sha,
                system_prompt,
                permission_ctx,
                loop_window,
                loop_hit_count,
                last_usage_terminal,
                workflow_ctx,
                summary_anchor,
            },
            &hot,
        )
        .await
        {
            Ok(o) => o,
            Err(()) => return,
        };
        // One-shot consumption (design §1): the force flag armed by
        // the softcap「压缩后续跑」answer applied to exactly this
        // drive_turn call — compaction failure / gate-off fall
        // through naturally, the flag never leaks into later turns.
        frame.force_compaction = false;
        // Write the cross-turn state back to the function-scope bindings
        // (these are declared above the loop and must persist across turns —
        // a `let` destructure inside the loop body would shadow + drop them
        // each iteration). tool_calls / loop_hint / cancelled are turn-local.
        messages = drive_outcome.messages;
        seq = drive_outcome.seq;
        head_sha = drive_outcome.head_sha;
        system_prompt = drive_outcome.system_prompt;
        permission_ctx = drive_outcome.permission_ctx;
        loop_window = drive_outcome.loop_window;
        loop_hit_count = drive_outcome.loop_hit_count;
        // C3 摘要压缩 PR2:水位锚点回写(同 loop 二次压缩时,
        // 下一 turn 的 prior-summary 来自循环内 anchor 而非 init 种子)。
        summary_anchor = drive_outcome.summary_anchor;
        last_usage_terminal = drive_outcome.last_usage_terminal;
        workflow_ctx = drive_outcome.workflow_ctx;
        let tool_calls = drive_outcome.tool_calls;
        let loop_hint = drive_outcome.loop_hint;
        let mut cancelled = drive_outcome.cancelled;

        let dispatch_outcome = dispatch_tool_calls(
            &request,
            &deps,
            &role,
            DispatchCtx {
                tool_calls,
                permission_ctx: permission_ctx.clone(),
                current_ctx: hot.current_ctx.clone(),
                last_cwd: hot.last_cwd.clone(),
                cancelled,
                session_mode,
                failure_tracker: failure_tracker.clone(),
                soft_blocked: soft_blocked.clone(),
                seq,
                stub_on,
                workflow_ctx: &workflow_ctx,
            },
        )
        .await;
        // Write the mutated fields back to their function/turn-loop bindings
        // (current_ctx / last_cwd are function-scope and must persist across
        // turns; cancelled is turn-loop-local). result_blocks is the dispatch
        // output consumed by the persist site below. （RULE-ARGS-001：二者现居 TurnHot）
        cancelled = dispatch_outcome.cancelled;
        hot.current_ctx = dispatch_outcome.current_ctx;
        hot.last_cwd = dispatch_outcome.last_cwd;
        let result_blocks = dispatch_outcome.result_blocks;

        if finalize_turn(
            &request,
            &deps,
            &role,
            FinalizeFrame {
                result_blocks,
                loop_hint: &loop_hint,
                cancelled,
                seq,
                messages: &mut messages,
                last_cwd: &hot.last_cwd,
            },
        )
        .await
        .is_err()
        {
            return;
        }
        seq += 1;
    }

    // MAX_TURNS softcap (08-18-max-turns-softcap): the hard terminal
    // is shared by the worker break (today's behavior, byte-identical)
    // and skipped when the softcap helper already emitted its own
    // terminal (Done{max_turns} stop / timeout stop / Done{cancelled}).
    if !softcap_terminal_emitted {
        emit_max_turns_terminal(
            &request,
            &deps,
            &role,
            HardTurnsTerminal {
                budget: turns_budget,
                last_usage_terminal,
                last_cwd: hot.last_cwd.as_deref(),
            },
        )
        .await;
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

// ---------------------------------------------------------------------------
// MAX_TURNS softcap (08-18-max-turns-softcap) — 撞线询问替代硬终断
// ---------------------------------------------------------------------------

/// 「继续」的加成粒度(决议表 2026-08-19):再给一整个预算
/// (= `MAX_TURNS` = 200),计数归零可再跑;400/600… 每次撞线再问。
pub(crate) const TURN_LIMIT_GRANT: usize = MAX_TURNS;

/// 软卡询问的缺省超时(决议:10 分钟无响应 → 停止,与今日
/// max_turns 行为等价;unattended 不烧钱)。env
/// `EVERLASTING_SOFTCAP_TIMEOUT_MS` 覆盖(ms;parse 失败回退缺省)
/// —— 仅软卡专项测试设置(env 调试钩子先例:`P1_DBG`,
/// drive.rs;Rust 2021 `env::set_var` 安全)。
fn softcap_ask_timeout() -> Duration {
    const DEFAULT: Duration = Duration::from_secs(600);
    match std::env::var("EVERLASTING_SOFTCAP_TIMEOUT_MS") {
        Ok(v) => v
            .trim()
            .parse::<u64>()
            .map(Duration::from_millis)
            .unwrap_or(DEFAULT),
        Err(_) => DEFAULT,
    }
}

/// 撞线边界(design §1 测试钩子,QA/live 用):env
/// `EVERLASTING_SOFTCAP_TURN_BOUNDARY` 只在 loop 初始化时应用一次
/// (替换初始 budget);未设 / parse 失败 → `turn_limit`(生产行为与
/// 今日 `turn > turn_limit` 判定完全一致)。
fn softcap_boundary(turn_limit: usize) -> usize {
    match std::env::var("EVERLASTING_SOFTCAP_TURN_BOUNDARY") {
        Ok(v) => v.trim().parse::<usize>().unwrap_or(turn_limit),
        Err(_) => turn_limit,
    }
}

/// The shared max_turns hard terminal, extracted VERBATIM from the
/// pre-softcap post-loop block (persist_turn_cwd + touch_session +
/// `Done{stop_reason:"max_turns"}`; stop_reason string unchanged —
/// 前端 / worker 父 loop / trace 消费方零感知). Used by:
///
/// - the worker boundary break — byte-identical to today's
///   behavior (`skip_persist` skips the persists in worker mode
///   exactly as the old inline `if !skip_persist` did, AC4);
/// - the softcap 停止 / 超时停止 / register 降级 paths (AC2:
///   "与今日 max_turns 终态等价" 字面成立 —— 同一函数体).
/// RULE-ARGS-001 收编（原 8 参 → 四元）：终态预算参数并入
/// [`HardTurnsTerminal`]。体内在头部做与旧位参同名同形的重绑定，
/// 正文逐字节不变。
pub(crate) async fn emit_max_turns_terminal(
    request: &ChatLoopRequest,
    deps: &ChatLoopDeps,
    role: &CallerRole,
    tail: HardTurnsTerminal<'_>,
) {
    let db = &deps.db;
    let session_id = &request.session_id;
    let sink = &request.sink;
    let rid = &request.rid;
    let skip_persist = role.skip_persist;
    let HardTurnsTerminal {
        budget,
        last_usage_terminal,
        last_cwd,
    } = tail;
    tracing::warn!(max_turns = budget, "agent loop: max turns reached");
    // B6 PR1b: skip the max_turns terminal persists in worker mode.
    if !skip_persist {
        persist_turn_cwd(db, session_id, last_cwd).await;
        let _ = crate::db::touch_session(db, session_id).await;
        emit_chat_event_via_sink(
            sink,
            session_id,
            rid,
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
}

/// [`ask_turn_limit_softcap`] 的三分支结果(design §1)。
enum SoftcapOutcome {
    /// 用户选「继续(+200 轮)」—— 预算 +`TURN_LIMIT_GRANT` 续跑。
    Continue,
    /// 用户选「压缩后续跑」—— 预算 +`TURN_LIMIT_GRANT`,且下一
    /// turn 强制触发自动摘要压缩(force flag,一次性)。
    CompactContinue,
    /// 终止。终态事件(`Done{max_turns}` 或 `Done{cancelled}`)
    /// 已在 helper 内 emit —— 调用方置
    /// `softcap_terminal_emitted` 后 break,循环后尾巴不再 emit。
    Terminal,
}

/// MAX_TURNS 软卡询问(design §2.1):主 loop 撞线边界上的浮动
/// 询问卡。结构照抄 C2+ 主动干预(drive.rs 的 register →
/// `emit_tool_question` → biased select),**新增超时臂**。
///
/// 撞线点在循环边界:上一轮 tool results 已由 `finalize_turn`
/// 落库(DB 尾部干净,无孤儿 tool_use),因此停止 / 超时分支
/// **不需要** `finalize_pending_tool_results`,直接走
/// `emit_max_turns_terminal`(AC2 字节级等价)。
///
/// payload 条件构建(决议 4):`compaction_on` 时三选项
/// (继续 / 压缩后续跑 / 停止),否则两选项(卡片不展示选了
/// 也无效的选项)。解析按 label 精确匹配,未匹配 / 畸形 → 停止
/// (防御默认,C2+ 同款)。
/// RULE-ARGS-001 收编（原 13 参 → 四元）：套件三引用 + [`TurnBudgetAsk`]。
/// 体内头部重绑定保持正文逐字节不变。
async fn ask_turn_limit_softcap(
    request: &ChatLoopRequest,
    deps: &ChatLoopDeps,
    role: &CallerRole,
    ask: TurnBudgetAsk<'_>,
) -> SoftcapOutcome {
    let question_store = &deps.question_store;
    let sink = &request.sink;
    let rid = &request.rid;
    let session_id = &request.session_id;
    let db = &deps.db;
    let token = &deps.token;
    let skip_persist = role.skip_persist;
    let TurnBudgetAsk {
        turn,
        turns_budget,
        compaction_on,
        seq,
        last_usage_terminal,
        last_cwd,
    } = ask;
    let continue_label = format!("继续(+{} 轮)", TURN_LIMIT_GRANT);
    // 2026-09-03 (task 09-03-ask-no-timeout): global switch — when on,
    // the timeout arm below becomes `pending()` so the softcap card
    // hangs until the user picks an option or stops the run (no more
    // automatic `timeout_stopped`). Read once at ask time; a mid-ask
    // flip does not retroactively settle an already-pending card.
    let no_timeout = crate::agent::permissions::ask::ask_no_timeout_enabled(db).await;
    let compact_label = "压缩后续跑";
    let stop_label = "停止";
    let mut options = vec![QuestionOption {
        label: continue_label.clone(),
        description: Some("再给一整个轮数预算继续运行;再次撞线时会重新询问".to_string()),
        preview: None,
    }];
    if compaction_on {
        options.push(QuestionOption {
            label: compact_label.to_string(),
            description: Some("先强制一次摘要压缩收窄上下文,再继续(同样 +200 轮)".to_string()),
            preview: None,
        });
    }
    options.push(QuestionOption {
        label: stop_label.to_string(),
        description: Some("按轮数上限结束本次运行(发送新消息即可继续)".to_string()),
        preview: None,
    });
    let payload = ToolQuestionPayload {
        session_id: session_id.to_string(),
        // turn = 撞线时的 turn 号(budget+1);前端 streamEvents 按
        // `turn_limit_softcap_` 前缀识别为浮动卡(同
        // `loop_intervention_` 先例,2026-07-28 事故教训:绝不能 tag
        // 成 Question —— 无 tool_use 锚点会永不渲染)。
        tool_use_id: format!("turn_limit_softcap_{turn}"),
        questions: vec![Question {
            question: format!(
                "本轮对话已达到 {} 轮上限(agent 仍在工作中)。是否继续?",
                turns_budget
            ),
            header: Some("轮数上限确认".to_string()),
            options,
            multi_select: false,
            allow_custom: false,
        }],
        ts: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0),
    };

    match question_store
        .register(
            session_id,
            &payload.tool_use_id,
            // Register as `TurnLimitSoftcap`(NOT `Question`)so the
            // frontend renders a floating card(C2+ 同款)。
            PendingInteraction::TurnLimitSoftcap(payload.clone()),
        )
        .await
    {
        Ok(rx) => {
            // Audit: action = "asked" lands immediately after
            // register succeeds(best-effort,warn+swallow,同 C2+;
            // 软卡仅在非 worker 主 loop 可达,skip_persist 恒 false)。
            let _ = crate::agent::permissions::audit::record_turn_limit_softcap_audit(
                db,
                session_id,
                turn,
                turns_budget,
                "asked",
                Some(seq),
            )
            .await;
            sink.emit_tool_question(&payload);
            tracing::info!(
                request_id = %rid,
                session_id = %session_id,
                turn,
                budget = turns_budget,
                "turn-limit softcap: question asked"
            );
            // Four-arm biased select: cancel / timeout / rx(design §2.1).
            // timeout 臂在 `no_timeout` 时恒 pending —— 撞线卡一直挂到
            // 用户点选 / Stop(见函数头 no_timeout 注释)。
            let mut timeout_arm = Box::pin(async move {
                if no_timeout {
                    std::future::pending::<()>().await
                } else {
                    tokio::time::sleep(softcap_ask_timeout()).await
                }
            });
            tokio::select! {
                            biased;
                            _ = token.cancelled() => {
                                // User hit Stop while the question was pending:
                                // clear the slot, emit Done{cancelled}(循环边界
                                // 无孤儿 tool_use,无需 finalize_pending_tool_results)。
                                question_store.remove(session_id).await;
                                let _ = crate::agent::permissions::audit::record_turn_limit_softcap_audit(
                                    db, session_id, turn, turns_budget, "cancelled", Some(seq),
                                ).await;
                                if !skip_persist {
                                    persist_turn_cwd(db, session_id, last_cwd).await;
                                    let _ = crate::db::touch_session(db, session_id).await;
                                }
                                emit_chat_event_via_sink(
                                    sink,
                                    session_id,
                                    rid,
                                    &ChatEvent::Done {
                                        stop_reason: Some("cancelled".to_string()),
                                        usage: None,
                                    },
                                );
                                SoftcapOutcome::Terminal
                            }
                            _ = timeout_arm.as_mut() => {
                                // 10min(缺省)无响应 → 停止(决议:unattended
                                // 不替用户确认烧钱;停止 = 今日行为零回归)。
                                question_store.remove(session_id).await;
                                let _ = crate::agent::permissions::audit::record_turn_limit_softcap_audit(
                                    db, session_id, turn, turns_budget, "timeout_stopped", Some(seq),
                                ).await;
                                emit_max_turns_terminal(
                                    request,
                                    deps,
                                    role,
                                    HardTurnsTerminal {
                                        budget: turns_budget,
                                        last_usage_terminal,
                                        last_cwd,
                                    },
                                )
                                .await;
                                SoftcapOutcome::Terminal
                            }
                            resp = rx => {
                                // resolve 已原子移除槽位(Answered / Cancelled 路径)。
                                match resp {
                                    Ok(InteractionResponse::Answered(value)) => {
                                        let answers: Vec<QuestionAnswer> =
                                            serde_json::from_value(value).unwrap_or_default();
                                        let chosen = answers
                                            .first()
                                            .map(|a| a.options.first().cloned().unwrap_or_default())
                                            .unwrap_or_default();
                                        if chosen == continue_label {
                                            let _ = crate::agent::permissions::audit::record_turn_limit_softcap_audit(
                                                db, session_id, turn, turns_budget, "continued", Some(seq),
                                            ).await;
                                            SoftcapOutcome::Continue
                                        } else if chosen == compact_label && compaction_on {
                                            let _ = crate::agent::permissions::audit::record_turn_limit_softcap_audit(
                                                db, session_id, turn, turns_budget, "compacted_continued", Some(seq),
                                            ).await;
                                            SoftcapOutcome::CompactContinue
                                        } else {
                                            // 「停止」/ 未匹配畸形载荷 → 防御默认停止
                                            // (C2+ 同款)。
                                            let _ = crate::agent::permissions::audit::record_turn_limit_softcap_audit(
                                                db, session_id, turn, turns_budget, "stopped", Some(seq),
                                            ).await;
            emit_max_turns_terminal(
                                                request,
                                                deps,
                                                role,
                                                HardTurnsTerminal {
                                                    budget: turns_budget,
                                                    last_usage_terminal,
                                                    last_cwd,
                                                },
                                            )
                                            .await;
                                            SoftcapOutcome::Terminal
                                        }
                                    }
                                    Ok(InteractionResponse::Cancelled) => {
                                        // 用户点「跳过」→ 视同停止(安全默认,
                                        // C2+ cancel 同款)。
                                        let _ = crate::agent::permissions::audit::record_turn_limit_softcap_audit(
                                            db, session_id, turn, turns_budget, "stopped", Some(seq),
                                        ).await;
            emit_max_turns_terminal(
                                            request,
                                            deps,
                                            role,
                                            HardTurnsTerminal {
                                                budget: turns_budget,
                                                last_usage_terminal,
                                                last_cwd,
                                            },
                                        )
                                        .await;
                                        SoftcapOutcome::Terminal
                                    }
                                    Err(_recv_err) => {
                                        // Sender dropped(槽位被 remove 等)→ 视同
                                        // cancel(C2+ 同款安全默认)。
                                        tracing::warn!(
                                            "turn-limit softcap oneshot dropped without response — treating as cancelled"
                                        );
                                        let _ = crate::agent::permissions::audit::record_turn_limit_softcap_audit(
                                            db, session_id, turn, turns_budget, "cancelled", Some(seq),
                                        ).await;
                                        if !skip_persist {
                                            persist_turn_cwd(db, session_id, last_cwd).await;
                                            let _ = crate::db::touch_session(db, session_id).await;
                                        }
                                        emit_chat_event_via_sink(
                                            sink,
                                            session_id,
                                            rid,
                                            &ChatEvent::Done {
                                                stop_reason: Some("cancelled".to_string()),
                                                usage: None,
                                            },
                                        );
                                        SoftcapOutcome::Terminal
                                    }
                                }
                            }
                        }
        }
        // AlreadyPending(理论上不可达:turn 内 ask_user_question 在
        // finalize 前已 resolve;防御)→ warn + 降级为今日硬停行为。
        Err(crate::agent::question_store::QuestionStoreError::AlreadyPending) => {
            tracing::warn!(
                session_id = %session_id,
                "turn-limit softcap register: a question is already pending — degrading to hard stop"
            );
            let _ = crate::agent::permissions::audit::record_turn_limit_softcap_audit(
                db,
                session_id,
                turn,
                turns_budget,
                "stopped",
                Some(seq),
            )
            .await;
            emit_max_turns_terminal(
                request,
                deps,
                role,
                HardTurnsTerminal {
                    budget: turns_budget,
                    last_usage_terminal,
                    last_cwd,
                },
            )
            .await;
            SoftcapOutcome::Terminal
        }
        Err(e) => {
            // `NotFound` 等不可达分支(防御,同 C2+ register 错误处理)。
            tracing::error!(
                error = %e,
                "turn-limit softcap register: unexpected store error — degrading to hard stop"
            );
            let _ = crate::agent::permissions::audit::record_turn_limit_softcap_audit(
                db,
                session_id,
                turn,
                turns_budget,
                "stopped",
                Some(seq),
            )
            .await;
            emit_max_turns_terminal(
                request,
                deps,
                role,
                HardTurnsTerminal {
                    budget: turns_budget,
                    last_usage_terminal,
                    last_cwd,
                },
            )
            .await;
            SoftcapOutcome::Terminal
        }
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
pub(crate) fn build_turn_latency(
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
pub(crate) fn emit_persist_failure(
    sink: &Arc<dyn ChatEventSink>,
    sid: &str,
    rid: &str,
    err: &sqlx::Error,
) {
    tracing::error!(error = %err, "agent loop: persist_turn failed");
    sink.emit_chat_event(&crate::state::ChatEventPayload {
        request_id: rid.to_string(),
        session_id: sid.to_string(),
        event: ChatEvent::Error {
            message: format!(
                "保存对话记录失败(可能磁盘满或数据库被占用),请重试。详情: {}",
                err
            ),
            category: LlmErrorCategory::Server,
        },
    });
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
pub(crate) async fn finalize_pending_tool_results(
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
