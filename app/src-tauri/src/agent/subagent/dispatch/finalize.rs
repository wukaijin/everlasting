//! run_subagent 阶段 G+H:收集/持久化 + 收尾/格式化(WorkerOutcome + collect_outcome + finalize_dispatch)。
//!
//! 拆分自 `dispatch.rs`(08-08-a-class-dispatch-split);hub 全量 re-export 符号。

#![allow(unused_imports)]

use std::path::PathBuf;
use std::sync::Arc;

use sqlx::SqlitePool;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::agent::chat_loop::run_chat_loop;
use crate::agent::workflow::WorkflowCtx;
use crate::llm::{ChatMessage, Provider, ToolDef};
use crate::memory::MemoryCache;
use crate::skill::loader::SkillCache;
use crate::state::{ChatEventSink, ProviderCatalog};
use crate::tools::read_guard::ReadGuard;
use crate::tools::ToolContext;

use super::super::prep::{build_resume_messages, task_with_env_hint, worker_is_writable};
use super::super::resolve::{
    resolve_final_model, resolve_isolation, resolve_model_by_name_or_id, resolve_project_id,
    resolve_project_main_path, resolve_worker_provider,
};
use super::super::worktree::{create_worker_worktree, probe_worker_changes};
use super::super::{
    assemble_subagent_prompt, build_worker_messages, filter_tools_for_subagent,
    filter_tools_readonly, format_dispatch_result_with_model, format_final_text,
    summarize_worker_tool_actions, truncate_messages_for_persistence,
    truncate_transcript_for_persistence, MESSAGES_MAX_BYTES, TRANSCRIPT_MAX_BYTES,
};
use super::super::{
    LoadedSubagent, SubagentBufferSink, SubagentCache, SubagentDef, SubagentEventSink,
    SubagentStatus,
};

// ---------------------------------------------------------------------------
// 阶段 G+H:收集/持久化 + 收尾/格式化(拆分自 run_subagent 主体, 08-08-a-class-dispatch-split)
// ---------------------------------------------------------------------------

/// run_subagent 阶段 G 的输出:worker 退出后的核心结果。
pub(crate) struct WorkerOutcome {
    /// worker 的 final assistant text(summary)。
    pub(crate) worker_text: String,
    /// C2+ loop-terminated 信号(worker 被 harness 强制停止)。
    pub(crate) worker_loop_terminated: bool,
    /// 终态 status(Completed / Cancelled / Error / Incomplete)。
    pub(crate) status: SubagentStatus,
}

/// 阶段 G:收集 worker 结果 + 持久化到 `subagent_runs`。
///
/// 内部顺序固化(AC8):status picker → transcript/messages 截断 →
/// `update_run_finished` → `emit_subagent_finished`,与现状一致。
/// persist 为 best-effort:DB 失败 warn + continue(不 mask worker 实际结果,
/// 不违反 RULE-A-007 tool_use/tool_result 配对不变量)。
pub(crate) async fn collect_outcome(
    db: &SqlitePool,
    parent_session_id: &str,
    worker_sink: &Arc<SubagentBufferSink>,
    worker_run_id_opt: &Option<String>,
) -> WorkerOutcome {
    // Drain the worker's accumulated state.
    //
    // 2026-06-21 (R2): the status picker now distinguishes
    // `max_turns` (soft-terminal, worker burned its 200-turn
    // budget without cleanly finishing) from `end_turn` /
    // `tool_use` (clean completion). The `was_incomplete` flag
    // is set by the sink's `Done{max_turns}` arm; the
    // `was_cancelled` flag is set by the `Done{cancelled}` arm;
    // `had_error` is set by the `Error` arm. The three are
    // mutually exclusive in practice (the agent loop's max_turns
    // branch is reached only when no cancel or error fired).
    let worker_text = worker_sink.final_text();
    // C2+ (2026-07-05, task `07-05-c2-loop-active-intervention` PR3):
    // capture the worker's loop-terminated signal BEFORE the status
    // picker so we can route it to `Incomplete` (the worker did not
    // cleanly finish — it was force-stopped by the harness mid-loop
    // after consecutive loop-detection hits ≥ 3). Kept as a separate
    // bool from the status so the `format_dispatch_result_with_model`
    // call + the loop-terminated line append below can read it
    // without re-parsing the status enum.
    let worker_loop_terminated = worker_sink.was_loop_terminated();
    let status = if worker_sink.was_cancelled() {
        SubagentStatus::Cancelled
    } else if worker_sink.had_error() {
        SubagentStatus::Error
    } else if worker_sink.was_incomplete() {
        // 2026-06-21 (R2): the `max_turns` soft-terminal path
        // is its own status (NOT `Completed` — the worker did
        // not cleanly finish). The DB `incomplete` row is the
        // signal for "useful partial output, did not exhaust
        // the task"; the `[status: incomplete]\n<partial>\n
        // [INCOMPLETE_MARKER]` wire shape makes it visible in
        // the parent's tool_result.
        SubagentStatus::Incomplete
    } else if worker_loop_terminated {
        // C2+ (PR3): the worker's loop-detection state machine
        // hit the consecutive-hits threshold and the worker
        // path took the direct-break short-circuit (no
        // `QuestionStore` round-trip, no audit row — R5).
        // Treat as `Incomplete`: the worker did useful partial
        // work but was force-stopped before completing the
        // task. The `[loop terminated: ...]` line appended
        // below carries the harness signal to the parent LLM.
        SubagentStatus::Incomplete
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
    // **Worker token isolation** (2026-06-26 reversal of
    // RULE-A-015/PR2a): the parent's `sessions.last_*` snapshot
    // is NOT updated by the worker. `update_last_turn_usage` is
    // back inside the `!skip_persist` gate at `chat_loop.rs`, so
    // worker turns (which run with `skip_persist=true`) don't
    // touch the parent's snapshot. The sink's per-turn
    // accumulator is the ONLY path by which the worker's
    // `TokenUsage` reaches disk — `cumulative_usage()` produces
    // the worker-run-level total for `token_usage_json`, written
    // here. Worker token usage is visible to the parent only via
    // `<SubagentDrawer>`.
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
            truncate_transcript_for_persistence(transcript_snapshot, TRANSCRIPT_MAX_BYTES);
        // C1 (07-26-subagent-resume): snapshot the worker's final
        // messages (captured by `record_worker_messages` on the chat
        // loop's normal completion path) and truncate for persistence.
        // Empty for cancel/error/incomplete exits (those skip the
        // snapshot call) → messages_json persists "[]" and resume
        // falls back to fresh dispatch. Over-cap runs also collapse
        // to empty + truncated=1 (partial history is unsafe to resume).
        let worker_messages = worker_sink.worker_messages();
        let (truncated_messages, messages_truncated) =
            truncate_messages_for_persistence(worker_messages, MESSAGES_MAX_BYTES);
        let cumulative_usage = worker_sink.cumulative_usage();
        let finished_at = chrono::Utc::now().to_rfc3339();
        let status_db = match status {
            SubagentStatus::Completed => crate::db::subagent_runs::SubagentStatusDb::Completed,
            SubagentStatus::Cancelled => crate::db::subagent_runs::SubagentStatusDb::Cancelled,
            SubagentStatus::Error => crate::db::subagent_runs::SubagentStatusDb::Error,
            // 2026-06-21 (R2): max_turns soft-terminal. The DB
            // CHECK constraint was widened to include
            // `'incomplete'` by the
            // `widen_subagent_runs_status_check_for_incomplete`
            // migration; the `Incomplete` variant was added to
            // both `agent::subagent::SubagentStatus` and
            // `db::subagent_runs::SubagentStatusDb` in lockstep.
            SubagentStatus::Incomplete => crate::db::subagent_runs::SubagentStatusDb::Incomplete,
        };
        match crate::db::subagent_runs::update_run_finished(
            db,
            worker_run_id,
            status_db,
            &finished_at,
            &worker_text,
            // B6 redesign PR1 (2026-06-21): the prefix-stripped
            // final text that the drawer renders in its Reply
            // segment. `summary` carries the same string for
            // backward compat (the legacy wire field); `final_text`
            // is the new consumer-facing field. Both land in
            // distinct DB columns so legacy `summary` consumers
            // (e.g. PR3 list-view summaries) keep working unchanged.
            &format_final_text(status, &worker_text),
            &cumulative_usage,
            &truncated_transcript,
            transcript_truncated,
            // 2026-06-22 (RULE-FrontSubagent-004): thread the actual
            // completed turn count so the drawer's `statusDisplay`
            // can render "stopped at turn N" / "incomplete at turn N"
            // for the cancelled / incomplete terminal states.
            // Completed runs also carry the count (harmless; the
            // drawer only reads it for cancelled + incomplete).
            // The counter is the sink's REAL per-turn Done count
            // (synthetic cancelled / max_turns terminals don't
            // increment — see `SubagentBufferSink::turns_completed`).
            Some(worker_sink.turns_completed() as i64),
            // C1 (07-26-subagent-resume): messages payload for resume.
            &truncated_messages,
            messages_truncated,
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
                // transport-abstraction 2026-07-20 (P1.3):
                // route the `subagent:finished` IPC emit through
                // the `SubagentEventSink` trait. The trait
                // handles the `app.emit` (production) /
                // no-op (test) split internally — no
                // `Option<AppHandle>` branching here.
                worker_sink.emit_subagent_finished(
                    worker_run_id,
                    parent_session_id,
                    status_db.as_str(),
                    &finished_at,
                );
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

    WorkerOutcome {
        worker_text,
        worker_loop_terminated,
        status,
    }
}

/// 阶段 H:收尾 + 格式化,返回 `(content, is_error, cancel_parent, exit_code)`。
///
/// 职责:cancel_parent 检测(父 token 直接判定)+ partial_actions 汇总 +
/// worktree change-detection + lifecycle(auto-commit / destroy / 清 DB 列)+
/// `format_dispatch_result_with_model` + loop-terminated / changes /
/// resume-fallback 三个 trailing append(顺序与现状一致)。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn finalize_dispatch(
    db: &SqlitePool,
    parent_session_id: &str,
    parent_token: &CancellationToken,
    worker_sink: &Arc<SubagentBufferSink>,
    worker_run_id_opt: &Option<String>,
    worker_run_id: &str,
    worker_worktree_opt: &Option<PathBuf>,
    worker_branch: &str,
    worker_display: &Option<String>,
    resume_fallback_note: &Option<String>,
    outcome: WorkerOutcome,
) -> (String, bool, bool, Option<i32>) {
    let WorkerOutcome {
        worker_text,
        worker_loop_terminated,
        status,
    } = outcome;

    // Detect parent-driven cancel: the parent token fired while the
    // worker was running. The worker's own cancel_done event may
    // NOT have fired if the cancel arrived after the worker loop
    // already returned (e.g. worker finished turn 1 cleanly, then
    // parent cancel propagated before turn 2's select! polled).
    // The child_token relationship makes the worker_token fire when
    // the parent fires; check parent_token directly so the caller's
    // serial loop flips its `cancelled` flag and drives the existing
    // cancel path (matches the user's Stop intent).
    let cancel_parent = parent_token.is_cancelled() && status == SubagentStatus::Cancelled;

    // RULE-BackSubagent-001 (PR2): for non-completed terminal states,
    // summarize the worker's executed tool_calls so the parent LLM can
    // do compensatory repair (skip already-landed writes, retry failed
    // tools). Completed gets `None`; an empty summary (worker executed
    // no tool_calls before exiting) also gets `None` so no empty
    // "Worker partial actions:" header lands in the tool_result.
    let partial_actions = if matches!(status, SubagentStatus::Completed) {
        None
    } else {
        let summary = summarize_worker_tool_actions(&worker_sink.transcript_snapshot());
        if summary.is_empty() {
            None
        } else {
            Some(summary)
        }
    };

    // L3b (2026-06-27): worktree change-detection + lifecycle.
    //
    // When the worker ran in an isolated worktree (`worker_worktree_opt`
    // is Some), we probe the worktree for changes vs its base commit
    // after the worker exits:
    //   - **No changes** → destroy the worktree immediately (the
    //     branch carries nothing useful). Clear `subagent_runs.worktree_path`.
    //   - **Has changes** → preserve the worktree + branch; the diff
    //     summary is appended to the dispatch_subagent tool_result
    //     (below) so the parent LLM knows where the worker's edits
    //     live ("changes left on branch worker/<run_id>"). A future
    //     PR3 `merge_worker` / `discard_worker` tool acts on the
    //     preserved branch.
    //
    // The change-detection + destroy/preserve decision happens
    // REGARDLESS of terminal status (completed / cancelled / error /
    // incomplete) — per the PRD's Edge Cases: "worker 取消 → 按正常
    // 完成处理 (有 changes 保留 branch, 无 destroy)". A cancelled
    // worker that landed partial writes still has useful artifacts
    // worth preserving for inspection.
    let mut worker_changes_summary: Option<String> = None;
    if let Some(wt_path) = worker_worktree_opt.as_ref() {
        let changes = probe_worker_changes(wt_path, worker_run_id);
        if changes.has_changes {
            // A (auto-commit, 2026-06-30): commit the worker's
            // working-tree changes onto `worker/<run_id>` so the branch
            // tip advances past the base. `probe_worker_changes` diffs
            // the working tree while `do_merge_blocking` merges branch
            // tips — without this, a worker that never commits leaves
            // worker_tip == parent_tip and merge_worker hits the
            // is_ancestor == short-circuit (merge_worker.rs:651) →
            // "merged fast-forward" with zero changes actually merged
            // (silent false-success). Failure is non-fatal: warn +
            // preserve the worktree anyway; merge degrades to legacy.
            if let Err(e) = crate::git::worktree::commit_worker_changes(wt_path, worker_run_id) {
                tracing::warn!(
                    worker_run_id = %worker_run_id,
                    worktree = %wt_path.display(),
                    error = %e,
                    "run_subagent: auto-commit of worker changes failed; preserving worktree (merge may degrade to legacy behavior)"
                );
            }
            // Preserve the worktree + branch. The DB row's
            // `worktree_path` column was already set to `wt_path`
            // above (after `insert_run`); leave it as-is.
            worker_changes_summary = Some(format!(
                "Worker changes left on branch `{}` (worktree at `{}`). \
                 Use `git diff` in that worktree to review, or merge/discard \
                 via a future tool.\n\n{}",
                worker_branch,
                wt_path.display(),
                changes.summary
            ));
        } else {
            // No changes — destroy the worktree + branch. Best-effort
            // (a destroy failure leaves a stale worktree; a future
            // sweep would clean it up — out of scope for PR1).
            let project_main_path = resolve_project_main_path(db, parent_session_id).await;
            if !project_main_path.is_empty() {
                if let Err(e) = crate::git::worktree::destroy_worker(
                    std::path::Path::new(&project_main_path),
                    wt_path,
                    worker_run_id,
                ) {
                    tracing::warn!(
                        worker_run_id = %worker_run_id,
                        worktree = %wt_path.display(),
                        error = %e,
                        "run_subagent: destroy_worker failed on no-changes exit (non-fatal; stale worktree left behind)"
                    );
                }
            }
            // Clear the DB column (best-effort).
            if worker_run_id_opt.is_some() {
                if let Err(e) =
                    crate::db::subagent_runs::set_worktree_path(db, worker_run_id, None).await
                {
                    tracing::warn!(
                        worker_run_id = %worker_run_id,
                        error = %e,
                        "run_subagent: failed to clear worktree_path after destroy (non-fatal)"
                    );
                }
            }
        }
    }

    let (content, is_error) = format_dispatch_result_with_model(
        status,
        &worker_text,
        partial_actions.as_deref(),
        worker_display.as_deref(),
    );

    // C2+ (2026-07-05, task `07-05-c2-loop-active-intervention` PR3):
    // when the worker exited via the C2+ direct-break short-circuit
    // (loop detection fired 3 turns in a row), append a
    // harness-generated line to the dispatch_result content so the
    // parent LLM sees the loop-termination signal and can decide
    // whether to retry / change strategy / accept (R5). The line is
    // appended AFTER `format_dispatch_result_with_model` so the
    // existing `[status: incomplete]` prefix + partial-actions
    // section stay unchanged; the loop-terminated line is a new
    // trailing signal.
    //
    // Why a trailing line (vs a new `SubagentStatus::LoopTerminated`
    // variant): adding a 5th status would ripple through
    // `format_final_text` / `format_dispatch_result` /
    // `SubagentStatusDb` (DB CHECK constraint + migration) /
    // frontend drawer status pill. C2+ R5 explicitly defers that
    // (worker runs have their own transcript; the parent only needs
    // to know the worker was force-stopped). The trailing-line
    // approach matches the existing `worker_changes_summary`
    // pattern below (also a trailing append on a non-clean exit).
    let content = if worker_loop_terminated {
        format!(
            "{}\n\n{}",
            content, "[loop terminated: worker 因循环重复操作被自动终止，未完成全部步骤]"
        )
    } else {
        content
    };

    // L3b (2026-06-27): append the worker-changes summary to the
    // tool_result content when the worker left changes on its branch.
    // The summary tells the parent LLM where to find the worker's
    // edits (branch name + worktree path + diff file list). We
    // append AFTER `format_dispatch_result` so the existing
    // `[status: ...]` prefix + partial-actions section stay
    // unchanged; the changes summary is a new trailing section.
    let content = if let Some(summary) = worker_changes_summary {
        format!("{}\n\n{}", content, summary)
    } else {
        content
    };
    // C1 (07-26-subagent-resume): when the caller asked to resume a
    // prior run but the resume path fell back to a fresh dispatch
    // (run missing / truncated / cross-session / still-running),
    // surface a trailing `[resume: fallback, reason: <code>]` line so
    // the parent LLM knows the worker did NOT continue the prior
    // conversation. Appended last so all other trailing sections
    // (loop-terminated, changes summary) stay in their existing order.
    let content = if let Some(note) = resume_fallback_note {
        format!("{}\n\n{}", content, note)
    } else {
        content
    };
    (content, is_error, cancel_parent, None)
}
