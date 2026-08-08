//! run_subagent 阶段 A:解析 + 校验(ParsedDispatch + parse_dispatch)。
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
use super::check_workflow_role_gate;

// ---------------------------------------------------------------------------
// 阶段 A:解析 + 校验(拆分自 run_subagent 主体, 08-08-a-class-dispatch-split)
// ---------------------------------------------------------------------------

/// run_subagent 阶段 A 的输出:解析 + 校验后的 dispatch 输入。
/// 字段在 run_subagent 主体中解构回同名局部变量,后续阶段零改动消费。
pub(crate) struct ParsedDispatch {
    /// LLM-supplied subagent name(原始字符串,未解析;`loaded.def.name` 是解析后的)。
    pub(crate) subagent_name: String,
    /// LLM-supplied delegation task(原始字符串)。
    pub(crate) task: String,
    /// 父 session 的 project_id(worker 共享父 memory 槽位 + worktree 定位)。
    pub(crate) project_id: String,
    /// 父 session 的 project_path(SubagentCache 作用域 key)。
    pub(crate) project_path: String,
    /// 解析后的 SubagentDef(三层缓存合并,含 workflow 层)。
    pub(crate) loaded: LoadedSubagent,
}

/// 阶段 A(L218–321):解析 LLM-supplied 参数 + project 解析 + cache lookup
/// (workflow/legacy 分支)+ unknown-name hint + 空 task 校验 + workflow role gate。
///
/// 早返(Err)不变量(AC7):unknown-name / 空 task / role gate 拒绝的 `is_error` 恒为
/// `true`;`cancel_parent` 与 `exit_code` 沿正常路径初值。
pub(crate) async fn parse_dispatch(
    db: &SqlitePool,
    parent_session_id: &str,
    current_ctx: &ToolContext,
    subagent_cache: &Arc<SubagentCache>,
    workflow_ctx: Option<&WorkflowCtx>,
    input: &serde_json::Value,
) -> Result<ParsedDispatch, (String, bool, bool, Option<i32>)> {
    // Parse the LLM-supplied { subagent, task } arguments.
    let subagent_name = input.get("subagent").and_then(|v| v.as_str()).unwrap_or("");
    let task = input.get("task").and_then(|v| v.as_str()).unwrap_or("");

    // Resolve the parent session's project_id + path so the worker
    // reads the same memory cache slots the parent uses. The
    // `project_path` is also the key the `SubagentCache` uses to
    // scope its `<project>/.everlasting/agents/` dir.
    let project_id = resolve_project_id(db, parent_session_id).await;
    let project_path = current_ctx.worktree_path.to_string_lossy().to_string();

    // L3d (2026-06-25): resolve the SubagentDef via the cache
    // (builtin + user + project merged with project > user >
    // builtin precedence). Replaces the static `lookup_subagent`.
    // Unknown name → error tool_result (keeps the
    // tool_use/tool_result pairing invariant).
    //
    // W1 (Workflow integration, Step 2.7 — 2026-07-09):
    // workflow sessions consult the *workflow-aware* lookup so the
    // plugin `.everlasting/workflow/<wf>/agents/` layer
    // (Step 2.3, highest precedence) is honored. Before this the
    // dispatch path always used the legacy `lookup`, so a plugin's
    // `researcher.md` / `implementer.md` / `checker.md` was never
    // loaded — the role-gate (Step 2.4) would correctly *allow* the
    // role but the worker fell back to the builtin/project/user body.
    // Non-workflow callers (`workflow_ctx = None`) keep the legacy
    // path byte-for-byte.
    let wf_name = workflow_ctx.map(|c| c.workflow_def.name.as_str());
    let Some(loaded) = (match wf_name {
        Some(wf) => {
            subagent_cache
                .lookup_with_workflow(&project_path, Some(wf), subagent_name)
                .await
        }
        None => subagent_cache.lookup(&project_path, subagent_name).await,
    }) else {
        // Build a friendly "available" hint by re-listing (cheap;
        // the cache is mtime-fenced so this is a HashMap lookup
        // when nothing changed since the dispatch_def was built).
        // Same workflow/legacy split as the lookup above so the
        // hint reflects the plugin layer the caller is dispatching in.
        let available: Vec<String> = match wf_name {
            Some(wf) => {
                subagent_cache
                    .list_with_workflow(&project_path, Some(wf))
                    .await
            }
            None => subagent_cache.list(&project_path).await,
        }
        .into_iter()
        .map(|l| l.def.name)
        .collect();
        let content = format!(
            "Unknown subagent '{}'. Available: {}.",
            subagent_name,
            if available.is_empty() {
                "(none)".to_string()
            } else {
                available.join(", ")
            }
        );
        return Err((content, true, false, None));
    };
    if task.trim().is_empty() {
        let content = "Missing or empty 'task' parameter. The delegation task must be a                        non-empty string."
            .to_string();
        return Err((content, true, false, None));
    }

    // W1 (Workflow integration, Step 2.4 — 2026-07-08):
    // workflow state-machine gate. Pure check extracted
    // into `check_workflow_role_gate` (see below) so the
    // logic is unit-testable without standing up the full
    // `run_subagent` signature.
    //
    // **Why a gate**: the workflow session is a guided
    // state machine — the agent SHOULD follow the
    // breadcrumb (planning → implement → check → done)
    // and dispatch the role that matches the current
    // state. Without a gate, an agent in `planning` could
    // dispatch `implementer` early and skip the research
    // step, breaking the workflow contract.
    //
    // **One-shot bypass via `force: true`**: the LLM (or
    // a power-user via the dispatch tool UI) can pass
    // `force: true` to override the gate for a single
    // dispatch — useful when the user explicitly wants to
    // run a researcher task while in `implement` (e.g.
    // "go back and re-research this decision"). The
    // bypass is one-shot (no persistence) and logs a
    // `warn!` so the audit log captures the overstep.
    //
    // **Non-workflow callers / no active task**:
    // `workflow_ctx = None` OR `current_task = None`
    // short-circuits the gate — same as pre-Step-2.4
    // behavior (legacy dispatch shape preserved
    // end-to-end).
    if let Some(denial) = check_workflow_role_gate(workflow_ctx, subagent_name, input) {
        return Err((denial, true, false, None));
    }

    Ok(ParsedDispatch {
        // 原代码中 `subagent_name` / `task` 是借用 input 的 `&str`;字段 owned 化
        // 后,run_subagent 解构处 shadow 回 `&str` 保持后续消费零改动。
        subagent_name: subagent_name.to_string(),
        task: task.to_string(),
        project_id,
        project_path,
        loaded,
    })
}
