//! run_subagent 阶段 B1:isolation + dispatch_model 决策(WorkerPlan + plan_worker)。
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
// 阶段 B1:isolation + dispatch_model 决策(拆分自 run_subagent 主体, 08-08-a-class-dispatch-split)
// ---------------------------------------------------------------------------

/// run_subagent 阶段 B1 的输出:isolation + dispatch_model 决策结果。
pub(crate) struct WorkerPlan {
    /// 本 worker 是否在独立 worktree 运行(force_readonly > dispatch input > parallel+writable > frontmatter)。
    pub(crate) isolated: bool,
    /// 每-dispatch model 覆盖(id;display_name 已反向解析),None = 无覆盖。
    pub(crate) dispatch_model: Option<String>,
}

/// 阶段 B1(连续段):isolation 决策 + dispatch_model 候选解析。
/// 消费 `def.isolation` / `def` writable 判定;输出 `WorkerPlan` 供 prepare(消费
/// `isolated`)与 resolve(消费 `dispatch_model`)使用。无早返。
pub(crate) async fn plan_worker(
    db: &SqlitePool,
    input: &serde_json::Value,
    def: &SubagentDef,
    force_readonly: bool,
    parallel: bool,
) -> WorkerPlan {
    // L3b (2026-06-27): resolve the worktree-isolation decision.
    // Merge the per-agent frontmatter default (`def.isolation`) with
    // the per-dispatch `isolation` input the LLM may have supplied.
    // Precedence: dispatch input > frontmatter default > not isolated.
    // When isolated, the worker runs in its own git worktree
    // (`<app_data_dir>/worktrees/<project_uuid>/worker/<run_id>`)
    // on branch `worker/<run_id>`, based off the parent session's
    // current worktree HEAD. When not isolated, the worker reuses
    // the parent session's worktree (legacy behavior).
    //
    // **L3a backward-compat**: when `force_readonly=true` (the
    // L3a concurrent dispatch path; the only call site that ever
    // passed `true`), isolation was historically forced off — the
    // concurrent branch was scoped to read-only + shared cwd per
    // the L3a race-dissolution proof. Post-L3b PR2, the concurrent
    // branch no longer passes `true`; isolation now propagates
    // from `def.isolation` + `dispatch.isolation` even in the
    // concurrent path. The short-circuit is retained for
    // `force_readonly=true` so the L3a serial-only regression
    // (`l3a_single_dispatch_runs_serial_path_unchanged`) + any
    // future explicit read-only call site preserve the old
    // "read-only + shared cwd" semantics.
    let dispatch_isolation = input.get("isolation").and_then(|v| v.as_bool());
    // B6+ B (task 07-06-b6plus-b-dispatch-model-arg): per-dispatch
    // model override. The LLM path sends a display_name (the schema
    // `model` enum's values are display_names); the user `@@ --model=`
    // path sends an id (the frontend resolves display_name→id before
    // IPC). Both converge here: `resolve_model_by_name_or_id` accepts
    // either form. A miss (deleted model / typo / empty) → `None`,
    // which means "no dispatch override" — the dispatch then falls
    // through to `resolve_final_model` (DB > frontmatter > parent),
    // preserving A/C zero-regression. A `warn!` makes the silent
    // fallback visible in logs.
    let dispatch_model_raw: Option<&str> = input
        .get("model")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let dispatch_model: Option<String> = match dispatch_model_raw {
        Some(raw) => match resolve_model_by_name_or_id(db, raw).await {
            Ok(Some(id)) => Some(id),
            Ok(None) => {
                tracing::warn!(
                    input = raw,
                    "dispatch model not found (deleted / typo); ignoring, using agent default"
                );
                None
            }
            Err(e) => {
                tracing::warn!(
                    input = raw,
                    error = %e,
                    "dispatch model lookup failed; ignoring, using agent default"
                );
                None
            }
        },
        None => None,
    };
    let isolated = if force_readonly {
        // Serial-only switch; force isolation off so the read-only +
        // shared-cwd scope is preserved (L3a legacy compat).
        false
    } else if let Some(explicit) = dispatch_isolation {
        // Explicit per-dispatch input always wins — including
        // `isolation: false` to opt OUT of isolation even in a
        // concurrent batch (the caller then owns any write race).
        // Mirrors resolve_isolation's precedence (dispatch > default).
        explicit
    } else if parallel && worker_is_writable(def) {
        // B (2026-06-30): concurrent batch + writable worker + no
        // explicit input → default to isolated, so concurrent writes
        // land on separate `worker/<run_id>` branches. Replaces the
        // old "general-purpose defaults to isolated" safety argument
        // but, unlike a hard force, a caller can still opt out with
        // `isolation: false` (handled above). Read-only concurrent
        // workers (researcher) fall through to def default (shared) —
        // no write race, saves the checkout.
        true
    } else {
        resolve_isolation(def.isolation, dispatch_isolation)
    };

    WorkerPlan {
        isolated,
        dispatch_model,
    }
}
