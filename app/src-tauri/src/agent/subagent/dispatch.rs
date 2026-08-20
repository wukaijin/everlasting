//! B6 Subagent — worker dispatch (`run_subagent`).
//!
//! Split out of `chat_loop.rs` on 2026-06-23 so the main loop file
//! stays focused on turn orchestration. `run_subagent` is the
//! interceptor helper called from
//! [`crate::agent::chat_loop::run_chat_loop`]'s serial-path tool
//! dispatch when `name == "dispatch_subagent"`; it owns the nested
//! `run_chat_loop` call that drives the worker agent.

use std::sync::Arc;

use sqlx::SqlitePool;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::agent::workflow::WorkflowCtx;
use crate::llm::Provider;
use crate::memory::MemoryCache;
use crate::skill::loader::SkillCache;
use crate::state::{ChatEventSink, ProviderCatalog};
use crate::tools::read_guard::ReadGuard;
use crate::tools::ToolContext;

use super::{SubagentCache, SubagentEventSink};
// ---------------------------------------------------------------------------
// Hub(re-export)与 run_subagent 主体
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// B6 Subagent (2026-06-19): worker dispatch
//
// `run_subagent` is the interceptor helper called from the
// serial-path tool dispatch loop when `name == "dispatch_subagent"`.
// It owns the nested `run_chat_loop` call that drives the worker
// agent. It was extracted from `chat_loop.rs` into this file on
// 2026-06-23, but it still needs the parent loop's closure
// dependencies (`provider` / `db` / `cancellations` / ...) — the
// alternative would be to thread 22+ parameters through a public
// function, which is the same "too many parameters" cost
// `run_chat_loop` itself pays (see RULE-A-006 docstring at the top
// of `chat_loop.rs`).
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
///
/// 2026-06-21 (R1): raised from 20 → 200. The original 20-turn
/// cap was sized for the B6 PR1 demo scenarios (small focused
/// tasks). Real `trellis-implement` runs burn 200+ tool calls
/// (code search + edit + verify + RUSTFLAGS / cargo test cycles
/// + DB inspection + spec re-reads), so 20 was an artificial
/// ceiling that hard-terminated workers mid-task. The 200
/// budget is empirically large enough for the heaviest observed
/// `trellis-implement` run while still bounded enough that a
/// runaway worker cannot burn the parent session's full 50-turn
/// budget (a single worker run at 200 turns is 4× the parent
/// budget — a real cost, but acceptable given R3's token-usage
/// fix (this PR) makes the burn visible). Future cost gates
/// (token / wall-clock second-stage) are explicitly deferred.
pub(crate) const SUBAGENT_MAX_TURNS: usize = 200;

pub(crate) mod drive;
pub(crate) mod finalize;
pub(crate) mod model;
pub(crate) mod parse;
pub(crate) mod plan;
pub(crate) mod prepare;
pub(crate) mod register;

#[allow(unused_imports)]
pub(crate) use drive::*;
#[allow(unused_imports)]
pub(crate) use finalize::*;
#[allow(unused_imports)]
pub(crate) use model::*;
#[allow(unused_imports)]
pub(crate) use parse::*;
#[allow(unused_imports)]
pub(crate) use plan::*;
#[allow(unused_imports)]
pub(crate) use prepare::*;
#[allow(unused_imports)]
pub(crate) use register::*;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_subagent(
    provider: &Arc<dyn Provider>,
    // task 07-03-subagent-frontmatter-model: the process-wide provider
    // catalog, threaded so the worker can resolve its own provider when
    // `def.model` is `Some(model_id)`. `None` in unit tests (no
    // `AppState`) and on any production path without an `AppHandle`;
    // `run_subagent` falls back to the parent provider on `None` or
    // catalog miss (see `resolve_worker_provider` below). The value is
    // `Arc<RwLock<ProviderCatalog>>` (clone-cheap) so the caller can
    // pass `state.catalog.clone()` or `None` uniformly.
    catalog: Option<Arc<RwLock<ProviderCatalog>>>,
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
    // P2.4 C5 (2026-07-22): the worker's `SubagentEventSink`,
    // replacing the `app_handle: Option<AppHandle>` 渐进方案.
    // Injected into the worker's `SubagentBufferSink` (via
    // `new_with_event_sink`) so worker `subagent:event` /
    // `subagent:finished` reach the transport live. Tauri passes
    // `AppHandleSubagentSink` (IPC); daemon passes
    // `HttpSseSubagentSink` (SSE — was buffer-only pre-C5, the
    // gap this closes); tests pass `ThreadLocalSubagentSink`. The
    // sibling `catalog` param (line ~244) covers the model-
    // resolution use the old `app_handle` also served.
    worker_event_sink: Arc<dyn SubagentEventSink>,
    // L3a (2026-06-24) / L3b PR2 (2026-06-27): when `true`, the
    // worker's toolset is additionally forced down to read-only
    // tools (`filter_tools_readonly`) on top of
    // `filter_tools_for_subagent`. **Post-PR2 this is the
    // SERIAL-ONLY path** (the concurrent dispatch branch in
    // `chat_loop.rs` no longer passes `true` — see L3b PR2). The
    // serial path (single dispatch or mixed batch) keeps passing
    // `false`, and the L3a regression
    // (`l3a_single_dispatch_runs_serial_path_unchanged`) continues
    // to pin the behavior.
    //
    // **Why kept after PR2**: the parameter is retained (instead
    // of removed) for two reasons:
    // 1. L3a test compat — the regression test
    //    `l3a_single_dispatch_runs_serial_path_unchanged` was
    //    written against the `force_readonly=true` API shape;
    //    removing it would force that test to re-thread its mock
    //    fixtures.
    // 2. Future "force read-only at the subagent level" feature
    //    (e.g. an LLM opts `general-purpose` into read-only for a
    //    single dispatch) can repurpose this param instead of
    //    adding a new one.
    //
    // The concurrent branch's race-dissolution proof (see
    // `.trellis/spec/backend/agent-loop-architecture.md`
    // §"Pattern: Concurrent isolated dispatch (L3b PR2)") no
    // longer depends on the read-only scope; per-worker worktree
    // isolation (PR1) handles the write race. The `force_readonly`
    // arg remains a SERIAL-only behavioral switch.
    force_readonly: bool,
    // L3d (2026-06-25): the process-wide subagent cache, used to
    // look up the dispatched subagent across builtin + user +
    // project layers (replaces the static `lookup_subagent(name)`
    // — `cache.lookup(project_path, name)` returns a cloned
    // `LoadedSubagent` honoring the project > user > builtin
    // precedence + Q2 tools-inheritance). Read-through + mtime-
    // fenced, so a freshly-written `.md` is picked up on the next
    // chat turn without a reload command.
    subagent_cache: &Arc<SubagentCache>,
    // L3b (2026-06-27): the app's data directory, used to compute
    // the worker worktree path (`<app_data_dir>/worktrees/
    // <project_uuid>/worker/<run_id>`). Production threads the real
    // `AppState.app_data_dir`; tests pass an empty path
    // (`Path::new("")`) since worker isolation is opted into
    // per-subagent and most integration tests dispatch `researcher`
    // (no isolation) or `general-purpose` against a non-isolating
    // fixture. A test that wants to exercise isolation passes a
    // tempdir path + sets up a real git repo.
    app_data_dir: &std::path::Path,
    // B (2026-06-30): `true` when this dispatch comes from the
    // concurrent batch (`DispatchBatch::Concurrent` in `chat_loop.rs`).
    // Combined with `worker_is_writable(def)`, this force-isolates
    // concurrent *write-capable* workers onto their own
    // `worker/<run_id>` branch (replaces the old "general-purpose
    // defaults to isolated" safety argument). Concurrent *read-only*
    // workers and all serial dispatches ignore this and fall back to
    // the subagent's `isolation` default (now `None` = shared).
    parallel: bool,
    // 2026-06-30 (`ask_user_question` task): the parent's
    // `QuestionStore` handle. Threaded into the nested
    // `run_chat_loop` so the signature is shape-identical with
    // the parent path; the worker never reaches the
    // `ask_user_question` interception (the tool is in
    // `STRUCTURALLY_DISABLED` and stripped by
    // `filter_tools_for_subagent`), so the store is unused on
    // this path.
    parent_question_store: &crate::agent::question_store::QuestionStore,
    // W1 (Workflow integration, Step 2.4 — 2026-07-08):
    // the workflow session's context. The role-gate
    // check (see `check_workflow_role_gate` below) reads
    // `workflow_def.roles_by_state` and
    // `current_task.status` from this param. `None` for
    // non-workflow sessions — the gate short-circuits
    // (legacy dispatch shape preserved end-to-end).
    //
    // Step 2.5 will read `workflow_def.delegation_templates`
    // from the same param to inject the delegation
    // template; no signature change at Step 2.5.
    workflow_ctx: Option<&WorkflowCtx>,
) -> (String, bool, bool, Option<i32>) {
    // 阶段 A:解析 + 校验(提取至 `parse_dispatch`)。早返(unknown-name / 空 task /
    // role gate)经 `Err` 原样透传——`is_error` 恒为 true(AC7)。
    let ParsedDispatch {
        subagent_name,
        task,
        project_id,
        project_path,
        loaded,
    } = match parse_dispatch(
        db,
        parent_session_id,
        current_ctx,
        subagent_cache,
        workflow_ctx,
        input,
    )
    .await
    {
        Ok(p) => p,
        Err(early) => return early,
    };
    let def = &loaded.def;
    // 原代码中 `subagent_name` / `task` 是借用 input 的 `&str`;ParsedDispatch 字段
    // owned 化后 shadow 回 `&str`,后续消费(insert / format / env hint)零改动。
    let subagent_name = subagent_name.as_str();
    let task = task.as_str();

    // 阶段 B1:isolation + dispatch_model 决策(提取至 `plan_worker`)。
    // 消费方:prepare(`isolated`)/ resolve(`dispatch_model`)。
    let WorkerPlan {
        isolated,
        dispatch_model,
    } = plan_worker(db, input, def, force_readonly, parallel).await;

    // 阶段 C+D:worktree + guard + toolset + messages(提取至 `prepare_worker`)。
    // 早返(worktree 创建失败)经 `Err` 原样透传——`is_error` 恒为 true(AC7)。
    let WorkerPrep {
        worker_run_id,
        worker_branch,
        worker_worktree_opt,
        project_main_override,
        worker_read_guard,
        worker_tool_defs,
        worker_messages,
        resume_fallback_note,
    } = match prepare_worker(
        db,
        parent_session_id,
        current_ctx,
        app_data_dir,
        &project_id,
        &project_path,
        isolated,
        def,
        force_readonly,
        read_guard,
        task,
        memory_cache,
        input,
        workflow_ctx,
        subagent_name,
    )
    .await
    {
        Ok(p) => p,
        Err(early) => return early,
    };

    // 阶段 B2:model 解析(提取至 `resolve_worker`)。消费 `def.model` /
    // `dispatch_model`;产出 worker_provider / worker_ctx / worker_display
    // (backfill 已完成,此处不再变)。
    let WorkerModel {
        worker_provider,
        worker_ctx,
        worker_display,
        worker_provider_id,
    } = resolve_worker(
        db,
        provider,
        context_window,
        &catalog,
        parent_session_id,
        def,
        &dispatch_model,
    )
    .await;

    // Assemble the worker's system prompt — fully replaces the
    // parent's behavior_prompt + mode_prefix + base_prompt layers.
    // The assembled prompt is threaded as the 23rd
    // `system_prompt_override` argument to the nested
    // `run_chat_loop` call below (was previously dead code
    // discarded at this site; see `docs/review/b6-subagent-assessment.md`
    // §2 + the doc comment on `run_chat_loop.system_prompt_override`).

    // 阶段 E:run 注册 + sink(提取至 `register_run`)。无早返;insert /
    // set_worktree_path 失败均为 best-effort(warn + continue)。
    let RegisteredRun {
        worker_rid,
        worker_token,
        worker_run_id_opt,
        worker_sink,
    } = register_run(
        cancellations,
        db,
        parent_rid,
        parent_session_id,
        tool_use_id,
        subagent_name,
        task,
        &worker_run_id,
        &worker_worktree_opt,
        &worker_display,
        parent_token,
        worker_event_sink.clone(),
    )
    .await;

    // 阶段 F:drive worker(提取至 `drive_worker`)。嵌套 run_chat_loop 调用
    // 封装于此;`worker_sink` / `worker_rid` / `worker_run_id_opt` /
    // `worker_worktree_opt` 以 clone 传入,后续 collect/finalize 继续消费。
    drive_worker(
        db,
        parent_session_id,
        cancellations,
        _session_active_request,
        memory_cache,
        skill_cache,
        permission_asks,
        background_shells,
        app_data_dir,
        subagent_cache,
        parent_question_store,
        worker_event_sink.clone(),
        worker_tool_defs,
        worker_provider,
        worker_ctx,
        worker_provider_id,
        worker_rid.clone(),
        worker_messages,
        worker_sink.clone(),
        worker_read_guard,
        worker_token,
        worker_run_id_opt.clone(),
        worker_worktree_opt.clone(),
        project_main_override,
        def,
        task,
    )
    .await;

    // 阶段 G:收集 + 持久化(提取至 `collect_outcome`)。内部顺序固化(AC8):
    // status picker → transcript/messages 截断 → update_run_finished →
    // emit_subagent_finished;persist 为 best-effort。
    let outcome = collect_outcome(db, parent_session_id, &worker_sink, &worker_run_id_opt).await;

    // 阶段 H:收尾 + 格式化(提取至 `finalize_dispatch`)。cancel_parent /
    // partial_actions / worktree lifecycle / format + 3 个 trailing append。
    finalize_dispatch(
        db,
        parent_session_id,
        parent_token,
        &worker_sink,
        &worker_run_id_opt,
        &worker_run_id,
        &worker_worktree_opt,
        &worker_branch,
        &worker_display,
        &resume_fallback_note,
        outcome,
    )
    .await
}
// ---------------------------------------------------------------------------
// W1 (Workflow integration, Step 2.4 — 2026-07-08):
// pure workflow role-gate. Lives outside `run_subagent`
// so the gate logic is unit-testable without standing up
// the 25-arg signature.
//
// **Returns**: `Some(content)` when the dispatch must be
// denied (content is the tool_error body — the agent
// reads it and self-corrects on the next turn);
// `None` when the dispatch is allowed to proceed.
//
// **Side effect**: a single `tracing::warn!` per denial
// + per `force=true` bypass, so the audit log captures
// the overstep. No I/O, no LLM, no DB.
// ---------------------------------------------------------------------------
pub(crate) fn check_workflow_role_gate(
    workflow_ctx: Option<&WorkflowCtx>,
    subagent_name: &str,
    input: &serde_json::Value,
) -> Option<String> {
    let ctx = workflow_ctx?;
    // State derived from current_task.status; no
    // task → no gate (matches the breadcrumb-less
    // state at task bootstrap).
    let state = ctx.current_task.as_ref()?.status.as_str();

    // C5 (2026-07-28): resolve the state machine via the TASK's
    // owning plugin (`task_workflow_def`), not the session plugin.
    // A dev-created task keeps dev's role rules even when the
    // session switches to review mid-task — preventing the cross-
    // plugin dead-lock where review's `roles_by_state` has no entry
    // for dev's `planning` status (and vice versa).
    let task_def = &ctx.task_workflow_def;

    let allowed = crate::agent::workflow::allowed_roles(task_def, state)
        .iter()
        .any(|r| r == subagent_name);
    let forced = input
        .get("force")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if allowed {
        return None;
    }
    if forced {
        tracing::warn!(
            subagent = %subagent_name,
            state = %state,
            "run_subagent: role gate bypassed via force=true"
        );
        return None;
    }

    let allowed_in_state: Vec<String> =
        crate::agent::workflow::allowed_roles(task_def, state).to_vec();
    let allowed_str = if allowed_in_state.is_empty() {
        "(none)".to_string()
    } else {
        allowed_in_state.join(", ")
    };
    tracing::warn!(
        subagent = %subagent_name,
        state = %state,
        "run_subagent: role gate denied (workflow session)"
    );
    Some(format!(
        "Role gate denied: '{subagent}' is not allowed in state '{state}' \
         (allowed: {allowed_str}). Either transition to a state that \
         allows this role, or re-dispatch with force: true for a one-shot \
         override. Current breadcrumb: see messages[0].",
        subagent = subagent_name,
        state = state,
        allowed_str = allowed_str,
    ))
}
