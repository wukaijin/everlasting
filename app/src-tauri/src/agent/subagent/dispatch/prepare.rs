//! run_subagent 阶段 C+D:worktree + guard + toolset + messages(WorkerPrep + prepare_worker)。
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
// 阶段 C+D:worktree + guard + toolset + messages(拆分自 run_subagent 主体, 08-08-a-class-dispatch-split)
// ---------------------------------------------------------------------------

/// run_subagent 阶段 C+D 的输出:worker 运行前的一切准备产物。
pub(crate) struct WorkerPrep {
    /// `subagent_runs.id`(UUID,先于 insert 预生成以推导 worktree 路径)。
    pub(crate) worker_run_id: String,
    /// 隔离 worker 的 branch 名(`worker/<run_id>`)。
    pub(crate) worker_branch: String,
    /// 隔离激活时创建的 worker worktree 路径(Some);非隔离 None。
    pub(crate) worker_worktree_opt: Option<PathBuf>,
    /// 隔离 worker 的 project main 路径(权限 inside-check 锚点)。
    pub(crate) project_main_override: Option<PathBuf>,
    /// worker 的 ReadGuard(隔离 = 全新空 guard;非隔离 = 父 guard clone)。
    pub(crate) worker_read_guard: ReadGuard,
    /// worker 的 toolset(allowlist + 结构性禁项 strip + readonly 可选)。
    pub(crate) worker_tool_defs: Vec<ToolDef>,
    /// worker 的初始 messages([memory_blocks, delegation_task];resume 时 = 历史 + 本轮)。
    pub(crate) worker_messages: Vec<ChatMessage>,
    /// resume 降级为 fresh dispatch 时的说明行(Some);正常路径 None。
    pub(crate) resume_fallback_note: Option<String>,
}

/// 阶段 C+D(连续段):worker_run_id/branch 预生成 + worktree 创建(失败 fail
/// dispatch)+ project_main_override + ReadGuard 重置 + toolset 过滤 + messages
/// 构建(resume 分支)+ delegation template 注入。
///
/// 早返(Err)不变量(AC7):worktree 创建失败,`is_error` 恒为 `true`。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn prepare_worker(
    db: &SqlitePool,
    parent_session_id: &str,
    current_ctx: &ToolContext,
    app_data_dir: &std::path::Path,
    project_id: &str,
    project_path: &str,
    isolated: bool,
    def: &SubagentDef,
    force_readonly: bool,
    read_guard: &ReadGuard,
    task: &str,
    memory_cache: &Arc<MemoryCache>,
    input: &serde_json::Value,
    workflow_ctx: Option<&WorkflowCtx>,
    subagent_name: &str,
) -> Result<WorkerPrep, (String, bool, bool, Option<i32>)> {
    // The worker_run_id is the `subagent_runs.id` we'll insert below
    // (a UUID). We need it BEFORE the insert to compute the worktree
    // path (the branch name + on-disk dir are derived from it). So
    // we pre-generate the UUID here and pass it into `insert_run`'s
    // slot. This is a small departure from the existing flow (which
    // let `insert_run` generate the id), but it keeps the worktree
    // path + DB row id in lockstep.
    let worker_run_id = uuid::Uuid::new_v4().to_string();
    let worker_branch = crate::git::worktree::worker_branch_name(&worker_run_id);

    // Compute the worker worktree path + create the worktree when
    // isolated. On any failure we FAIL the dispatch (return an error
    // tool_result) — per the PRD's Edge Cases: "worktree 创建失败 →
    // fail dispatch,不降级到不隔离" (avoids silent behavior
    // inconsistency where the LLM thinks isolation is active but
    // it isn't).
    //
    // `worker_worktree_opt` carries the path (Some) when isolation
    // is active + the worktree was created successfully. It's the
    // value threaded into `run_chat_loop`'s `worktree_override`
    // parameter (Some) below, and the value written to
    // `subagent_runs.worktree_path`.
    let worker_worktree_opt: Option<PathBuf> = if isolated {
        match create_worker_worktree(
            db,
            parent_session_id,
            project_id,
            &worker_run_id,
            app_data_dir,
            &current_ctx.worktree_path,
        )
        .await
        {
            Ok(path) => Some(path),
            Err(e) => {
                tracing::warn!(
                    parent_session_id = %parent_session_id,
                    worker_run_id = %worker_run_id,
                    error = %e,
                    "run_subagent: worker worktree creation failed; failing dispatch (no fallback to non-isolated)"
                );
                let content = format!(
                    "[status: error]\nFailed to create isolated worker worktree on branch \
                     `worker/{}`: {}. The dispatch was aborted — the worker did not run. \
                     Either retry without isolation, or resolve the underlying git error.",
                    worker_run_id, e
                );
                return Err((content, true, false, None));
            }
        }
    } else {
        None
    };

    // project_main_override (2026-07-29): for an isolated worker,
    // resolve the ORIGINAL project main repo path so the nested
    // `run_chat_loop` can anchor the permission layer's inside-check on
    // the project root (not the worker's checkout subtree — see
    // PermissionContext.project_main_path). Non-isolated workers / paths
    // pass `None` and let `run_chat_loop` fall back to `worktree_path`
    // (which is the project root for them). Reuse the same
    // session→project→project.path resolution as `create_worker_worktree`.
    let project_main_override: Option<PathBuf> = if isolated {
        let main = resolve_project_main_path(db, parent_session_id).await;
        if main.is_empty() {
            tracing::warn!(
                parent_session_id = %parent_session_id,
                worker_run_id = %worker_run_id,
                "run_subagent: failed to resolve project main path for worker; \
                 permission inside-check will anchor on the worktree (may cause \
                 spurious permission prompts on project-root reads)"
            );
            None
        } else {
            Some(PathBuf::from(main))
        }
    } else {
        None
    };

    // L3b (2026-06-27): when isolated, RESET the ReadGuard for the
    // worker. The worker starts in a fresh checkout with no
    // inherited "already-read" file set — if we passed the parent's
    // ReadGuard through, the worker's edit_file would pass the
    // verify_read check for files the parent read (in a DIFFERENT
    // checkout), then fail at verify_fresh (the file doesn't exist
    // in the worker's tree). A fresh empty ReadGuard forces the
    // worker to read files in its own tree before editing.
    //
    // We construct a fresh guard and swap it in for the nested
    // run_chat_loop call; the parent's guard is borrowed (`&`) and
    // untouched. The fresh guard dies with this run_subagent call
    // (no shared state to clean up — ReadGuard is per-session and
    // the worker has no session of its own).
    let worker_read_guard: ReadGuard = if isolated {
        ReadGuard::new()
    } else {
        // Non-isolated: clone the parent's guard (legacy behavior).
        // The clone is cheap (Arc inside).
        read_guard.clone()
    };

    // Build the worker's toolset (allowlist + structural-disabled
    // strip). The worker's run_chat_loop call gets this filtered
    // Vec; the parent's tool_defs is unaffected.
    //
    // L3d (2026-06-25): we clone the resolved `def` (the cache
    // returns an owned `LoadedSubagent`) so the worker's filter
    // can consume it. `filter_tools_for_subagent` takes `&SubagentDef`
    // so we just borrow.
    let worker_tool_defs = filter_tools_for_subagent(crate::tools::builtin_tools(), def);
    // L3a (2026-06-24): concurrent dispatch branch forces the
    // worker's toolset down to read-only tools. The serial path
    // passes `force_readonly = false` so `general-purpose` in
    // the serial path keeps its full write/shell/web toolset
    // (gated by `is_worker: true` at the ⑨ permission layer).
    // For `researcher` this is a no-op (its allowlist is already
    // exactly the 4 read-only tools).
    let worker_tool_defs = if force_readonly {
        filter_tools_readonly(worker_tool_defs)
    } else {
        worker_tool_defs
    };

    // Build the worker's messages: [memory_blocks (cache_control),
    // delegation_task]. The task is APPENDed (prompt-cache invariant
    // — see PRD §Decisions 6 + research §10.5). `project_id` +
    // `project_path` were resolved above (before the cache lookup,
    // since the cache scopes its `<project>/.everlasting/agents/`
    // dir by `project_path`).
    // C (2026-06-30): append an isolation environment hint to the
    // delegation task when the worker runs isolated (shared: raw task).
    let final_task = task_with_env_hint(task, isolated, &worker_run_id);

    // C1 (07-26-subagent-resume): branch on `resume_from`. When the
    // caller asks to resume a prior run, the worker's initial
    // messages = the prior run's persisted history + a clarification
    // user message + this round's task. We BYPASS `build_worker_messages`
    // on the resume path (the history already carries the prior
    // memory snapshot — re-injecting would duplicate; design §2
    // trade-off: mid-run memory edits don't apply to resumed runs).
    // Any validation failure (run missing / truncated / cross-session
    // / still-running) falls back to a fresh dispatch and surfaces a
    // `[resume: fallback, reason: <code>]` line in the tool_result so
    // the parent LLM knows the delegation was NOT a continuation.
    // No `resume_from` → the original `build_worker_messages` path
    // (zero regression — existing callers never set the field).
    let resume_from = input.get("resume_from").and_then(|v| v.as_str());
    let (mut worker_messages, resume_fallback_note) = if let Some(run_id) = resume_from {
        build_resume_messages(
            db,
            parent_session_id,
            run_id,
            &final_task,
            input,
            memory_cache,
            project_id,
            project_path,
        )
        .await
    } else {
        (
            build_worker_messages(memory_cache, project_id, project_path, &final_task).await,
            None,
        )
    };

    // W1 (Workflow integration, Step 2.5 — 2026-07-08):
    // append the filled delegation template to
    // `worker_messages[0]`'s block list. Only fires when:
    // - workflow session (`workflow_ctx.is_some()`)
    // - plugin defines a template for the dispatched role
    //
    // The helper handles both guards (`append_delegation_template`
    // returns `false` on `None` template; the S-B guard inside
    // catches the not-a-user-Blocks-message case). On
    // non-workflow callers OR no plugin template → no-op
    // (legacy behavior preserved).
    //
    // **Per M-E**: this is a dispatch-turn-only injection.
    // The parent's chat_loop's messages[0] is untouched; we
    // only mutate the worker's messages[0]. The worker
    // sees the template as part of its initial context.
    let filled = workflow_ctx.and_then(|ctx| {
        crate::agent::workflow::compute_delegation_template(ctx, project_path, subagent_name)
    });
    crate::agent::workflow::append_delegation_template(&mut worker_messages, filled);

    Ok(WorkerPrep {
        worker_run_id,
        worker_branch,
        worker_worktree_opt,
        project_main_override,
        worker_read_guard,
        worker_tool_defs,
        worker_messages,
        resume_fallback_note,
    })
}
