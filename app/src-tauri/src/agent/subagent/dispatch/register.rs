//! run_subagent 阶段 E:run 注册 + sink(RegisteredRun + register_run)。
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
// 阶段 E:run 注册 + sink(拆分自 run_subagent 主体, 08-08-a-class-dispatch-split)
// ---------------------------------------------------------------------------

/// run_subagent 阶段 E 的输出:worker run 的注册产物 + buffer sink。
pub(crate) struct RegisteredRun {
    /// worker rid(`{parent_rid}-sub-{tool_use_id}`),注册进 `cancellations`。
    pub(crate) worker_rid: String,
    /// worker 的 cancellation token(父 token 的 child,parent cancel 传播)。
    pub(crate) worker_token: CancellationToken,
    /// `subagent_runs` 行 id(insert 失败 = None;后续 update/emit 均为 no-op)。
    pub(crate) worker_run_id_opt: Option<String>,
    /// worker 的 `SubagentBufferSink`(in-memory transcript + subagent:event 转发)。
    pub(crate) worker_sink: Arc<SubagentBufferSink>,
}

/// 阶段 E(连续段):worker_rid/token 构造 + `cancellations` 注册(不进
/// `session_active_request`)+ `insert_run_with_id`(best-effort)+
/// `set_worktree_path`(best-effort)+ `SubagentBufferSink::new_with_event_sink`。
///
/// 无早返:insert / set_worktree_path 失败均 warn + continue(worker 仍运行)。
/// 签名已按评审 P2-1 收窄——`tool_use_id` 仅用于 rid 构造;`worker_run_id` /
/// `worker_worktree_opt` 来自 `WorkerPrep`;`worker_display` 来自 `WorkerModel`。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn register_run(
    cancellations: &Arc<Mutex<std::collections::HashMap<String, CancellationToken>>>,
    db: &SqlitePool,
    parent_rid: &str,
    parent_session_id: &str,
    tool_use_id: &str,
    subagent_name: &str,
    task: &str,
    worker_run_id: &str,
    worker_worktree_opt: &Option<PathBuf>,
    worker_display: &Option<String>,
    parent_token: &CancellationToken,
    worker_event_sink: Arc<dyn SubagentEventSink>,
) -> RegisteredRun {
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
    let worker_rid = format!("{}-sub-{}", parent_rid, tool_use_id);
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
    //
    // L3b (2026-06-27): we pass the pre-generated `worker_run_id`
    // (computed above so the worktree path + branch name could be
    // derived from it BEFORE the insert). On success, the DB row's
    // id matches the worktree's branch name; on failure, the
    // worktree (if created) is orphaned and the post-loop cleanup
    // handles destruction via the `worker_worktree_opt` local
    // (independent of the DB row's existence).
    let worker_run_id_opt: Option<String> = match crate::db::subagent_runs::insert_run_with_id(
        db,
        worker_run_id,
        parent_session_id,
        &worker_rid,
        subagent_name,
        Some(task),
        // 2026-07-03 (task 07-03-subagent-per-agent-model-ui,
        // AC13): thread the worker's *actual* model display into
        // the row. `worker_display` is `Some(name)` on catalog
        // hit (i.e. the worker resolved a model override /
        // frontmatter), `None` on parent inheritance / catalog
        // miss. The frontend reads this for the card / drawer
        // model chip (AC14-15). The wire `[model: <name>]`
        // line in `format_dispatch_result_with_model` follows
        // the same `Option<String>` shape — when
        // `worker_display` is `None`, the line is omitted (no
        // redundant "inherited parent" line; the parent
        // fallback is implied).
        worker_display.as_deref(),
    )
    .await
    {
        Ok(()) => Some(worker_run_id.to_string()),
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

    // L3b (2026-06-27): if isolation is active + the DB row was
    // inserted, record the worktree path on the row. Best-effort
    // (warn+continue on failure — the path is a forward-compat
    // breadcrumb for PR3's merge/discard tool).
    if let (Some(_), Some(ref wt_path)) = (&worker_run_id_opt, worker_worktree_opt) {
        if let Err(e) = crate::db::subagent_runs::set_worktree_path(
            db,
            worker_run_id,
            Some(&wt_path.to_string_lossy()),
        )
        .await
        {
            tracing::warn!(
                worker_run_id = %worker_run_id,
                error = %e,
                "run_subagent: failed to record worker worktree_path (non-fatal)"
            );
        }
    }

    // SubagentBufferSink: records the worker's emits into an in-
    // memory transcript AND (PR2 hotfix) emits each event on the
    // `subagent:event` channel so the frontend `<SubagentDrawer>`
    // (PR3b) can stream the transcript live. Does NOT forward to
    // the parent sink — the parent's frontend only sees the
    // dispatch_subagent tool_call / tool_result pair; the worker's
    // stream stays isolated (Claude Code convention).
    //
    // P2.4 C5 (2026-07-22): the sink's event channel is the
    // injected `worker_event_sink` (Tauri `AppHandleSubagentSink` /
    // daemon `HttpSseSubagentSink` / test `ThreadLocalSubagentSink`)
    // — the old `app_handle: Option<AppHandle>` Some/None branching
    // (and its double-clone) is gone. See `run_chat_loop`'s
    // `worker_event_sink` param doc.
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
    // P2.4 C5 (2026-07-22): the worker's `SubagentBufferSink` is
    // now wired via the injected `SubagentEventSink`. The old
    // `app_handle` match (`new` / `new_without_app_handle` split)
    // is gone — one `new_with_event_sink` path now serves the
    // Tauri IPC sink, the daemon SSE sink, and the test ThreadLocal
    // collector uniformly.
    let worker_sink: Arc<SubagentBufferSink> = Arc::new(SubagentBufferSink::new_with_event_sink(
        event_run_id.clone(),
        parent_session_id.to_string(),
        worker_event_sink,
    ));

    RegisteredRun {
        worker_rid,
        worker_token,
        worker_run_id_opt,
        worker_sink,
    }
}
