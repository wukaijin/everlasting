//! run_subagent 阶段 B2:worker model 解析(WorkerModel + resolve_worker)。
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
// 阶段 B2:worker model 解析(拆分自 run_subagent 主体, 08-08-a-class-dispatch-split)
// ---------------------------------------------------------------------------

/// run_subagent 阶段 B2 的输出:worker 的 provider / context_window / display。
pub(crate) struct WorkerModel {
    /// worker 实际使用的 provider(dispatch_model > DB > frontmatter > parent 解析后)。
    pub(crate) worker_provider: Arc<dyn Provider>,
    /// worker 的 context_window。
    pub(crate) worker_ctx: u32,
    /// worker 实际模型的 display_name(backfill 后;None = 父 session 亦无模型)。
    pub(crate) worker_display: Option<String>,
    /// 08-20-turn-usage-event-quota-view WP2: worker 实际模型的 provider
    /// 行 id(worker 自己模型 catalog 命中,或 inherit 时按父 session
    /// 模型回填 —— 与 display 同一块回填,同一 ModelRow)。落
    /// `turn_trace.provider_id` run 行归因。None = 两路皆失(聚合归
    /// unknown 桶)。
    pub(crate) worker_provider_id: Option<String>,
}

/// 阶段 B2(连续段):`resolve_final_model`(DB > frontmatter > parent)+
/// dispatch_model overlay(dispatch > DB > frontmatter > parent)+
/// `resolve_worker_provider` + worker_display backfill(父 session model_id →
/// display_name)。无早返。
pub(crate) async fn resolve_worker(
    db: &SqlitePool,
    provider: &Arc<dyn Provider>,
    context_window: u32,
    catalog: &Option<Arc<RwLock<ProviderCatalog>>>,
    parent_session_id: &str,
    def: &SubagentDef,
    dispatch_model: &Option<String>,
) -> WorkerModel {
    // task 07-03-subagent-frontmatter-model: resolve the worker's
    // provider / context_window / display from `def.model` via
    // [`resolve_worker_provider`] (the pure-over-(catalog, db) core,
    // unit-tested). We hold the catalog read lock here and pass
    // `&ProviderCatalog` in; `worker_display` threads to
    // `format_dispatch_result_with_model` so the parent LLM sees which
    // model the worker actually used.
    //
    // 2026-07-03 (task 07-03-subagent-per-agent-model-ui, 阶段 1):
    // the resolved model is `resolve_final_model` — DB override
    // (per-agent UI preference) > frontmatter `model:` (file
    // declaration) > parent. The two priority arms collapse into a
    // single `Option<model_id>` before reaching
    // `resolve_worker_provider`, which is **unchanged** (its 6
    // existing unit tests stay green; the priority change is
    // upstream of the resolver).
    //
    // 2026-07-07 (task 07-06-b6plus-b-dispatch-model-arg, B6+ B):
    // the per-dispatch override (`dispatch_model`, parsed above from
    // `input.model`) sits ABOVE `resolve_final_model` in the priority
    // chain: dispatch > DB > frontmatter > parent. The overlay is a
    // single `Option::or`, so `dispatch_model=None` (no per-dispatch
    // override) collapses to exactly the prior behavior (A/C
    // zero-regression). `dispatch_model` is always an id by the time
    // it reaches here (display_name reverse-lookup already happened
    // during parsing).
    let resolved_lower =
        match resolve_final_model(db, def.name.as_str(), def.model.as_deref()).await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    agent_name = %def.name,
                    error = %e,
                    "run_subagent: resolve_final_model failed; falling back to frontmatter-only"
                );
                def.model.clone()
            }
        };
    let final_model = dispatch_model.clone().or(resolved_lower);
    let cat_guard = match catalog {
        Some(c) => Some(c.read().await),
        None => None,
    };
    let (worker_provider, worker_ctx, mut worker_display, mut worker_provider_id): (
        Arc<dyn Provider>,
        u32,
        Option<String>,
        Option<String>,
    ) = resolve_worker_provider(
        final_model.as_deref(),
        provider,
        context_window,
        cat_guard.as_deref(),
        db,
    )
    .await;
    // 2026-07-29 (reviewer model-assignment gap): when `worker_display`
    // is `None` the worker is silently inheriting the PARENT session's
    // model. That's correct behavior, but recording NULL in
    // `subagent_runs.model_display` makes post-hoc DB inspection blind
    // to "which model actually ran" — e.g. a multi-model review where the
    // LLM forgot the `model` arg looks identical to a correct one in the
    // DB (both NULL), so "multi-model disagreement never materialized"
    // is undetectable after the fact (the exact failure in session
    // 6b313ce4: two reviewers, both NULL, both actually ran the parent
    // default model).
    //
    // Backfill the EFFECTIVE model: read the parent session's `model_id`,
    // resolve it to a display_name, and use it. `worker_display` stays
    // `None` only if the parent session itself has no model (degenerate);
    // the wire `[model: ...]` line + DB row then both reflect the actual
    // model the worker ran, instead of hiding it. Best-effort: a DB miss
    // logs at `warn!` and `worker_display` stays `None` (unchanged behavior).
    if worker_display.is_none() {
        match crate::db::sessions::load_session(db, parent_session_id).await {
            Ok(Some(s)) => {
                if let Some(mid) = s.session.model_id.as_deref().filter(|m| !m.is_empty()) {
                    match crate::db::models::get_model(db, mid).await {
                        Ok(Some(m)) => {
                            worker_display = Some(m.display_name);
                            // 08-20-turn-usage-event-quota-view WP2:inherit
                            // 场景的 provider 归因 —— 同一 ModelRow 顺手取
                            // (display 回填 = worker 实际跑的就是父模型,
                            // provider_id 同源即准确)。
                            if worker_provider_id.is_none() {
                                worker_provider_id = Some(m.provider_id);
                            }
                        }
                        Ok(None) => tracing::warn!(
                            parent_session_id = %parent_session_id,
                            model_id = %mid,
                            "run_subagent: parent session model_id not in models table; \
                             worker model_display stays None"
                        ),
                        Err(e) => tracing::warn!(
                            parent_session_id = %parent_session_id,
                            error = %e,
                            "run_subagent: get_model failed; worker model_display stays None"
                        ),
                    }
                }
            }
            Ok(None) => tracing::warn!(
                parent_session_id = %parent_session_id,
                "run_subagent: parent session not found; worker model_display stays None"
            ),
            Err(e) => tracing::warn!(
                parent_session_id = %parent_session_id,
                error = %e,
                "run_subagent: load_session failed; worker model_display stays None"
            ),
        }
    }

    WorkerModel {
        worker_provider,
        worker_ctx,
        worker_display,
        worker_provider_id,
    }
}
