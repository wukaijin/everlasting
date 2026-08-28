//! run_subagent 阶段 F:drive worker(drive_worker,嵌套 run_chat_loop 调用封装)。
//!
//! 拆分自 `dispatch.rs`(08-08-a-class-dispatch-split);hub 全量 re-export 符号。

#![allow(unused_imports)]

use std::path::PathBuf;
use std::sync::Arc;

use sqlx::SqlitePool;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::agent::chat_loop::{
    run_chat_loop, CallerRole, ChatLoopDeps, ChatLoopDepsParts, ChatLoopRequest,
};
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
use super::SUBAGENT_MAX_TURNS;

// ---------------------------------------------------------------------------
// 阶段 F:drive worker(拆分自 run_subagent 主体, 08-08-a-class-dispatch-split)
// ---------------------------------------------------------------------------

/// 阶段 F:驱动 worker 的嵌套 `run_chat_loop` 调用。
///
/// 参数较多(design §2 已知债务):这是 `run_chat_loop` 位置参数调用点的直接
/// 映射,命名化后已优于散落位置参数;本任务不为它进一步结构化。
///
/// 内部职责:worker_sink → `dyn ChatEventSink`、per-run grant cache 构造、
/// `Box::pin(run_chat_loop(...))`(24+ 位置参数,worker 隔离标志 + system_prompt
/// 覆盖 + worktree 覆盖等)。无返回、无早返。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn drive_worker(
    db: &SqlitePool,
    parent_session_id: &str,
    cancellations: &Arc<Mutex<std::collections::HashMap<String, CancellationToken>>>,
    session_active_request: &Arc<Mutex<std::collections::HashMap<String, String>>>,
    memory_cache: &Arc<MemoryCache>,
    skill_cache: &Arc<SkillCache>,
    permission_asks: &crate::agent::permissions::PermissionStore,
    background_shells: &crate::background_shell::DefaultRegistry,
    app_data_dir: &std::path::Path,
    subagent_cache: &Arc<SubagentCache>,
    parent_question_store: &crate::agent::question_store::QuestionStore,
    worker_event_sink: Arc<dyn SubagentEventSink>,
    worker_tool_defs: Vec<ToolDef>,
    worker_provider: Arc<dyn Provider>,
    worker_ctx: u32,
    // 08-20-turn-usage-event-quota-view WP2:worker 模型的 provider 行 id
    // (WorkerModel.worker_provider_id),落 turn_trace run 行归因。
    worker_provider_id: Option<String>,
    worker_rid: String,
    worker_messages: Vec<ChatMessage>,
    worker_sink: Arc<SubagentBufferSink>,
    worker_read_guard: ReadGuard,
    worker_token: CancellationToken,
    worker_run_id_opt: Option<String>,
    worker_worktree_opt: Option<PathBuf>,
    project_main_override: Option<PathBuf>,
    def: &SubagentDef,
    task: &str,
) {
    // Nested run_chat_loop. The worker reuses the parent's
    // session_id for DB linkage (its turns land in the same
    // `messages` table), but:
    //   - `skip_session_active: true` so the guard's Drop does NOT
    //     evict the parent's session_active_request entry.
    //   - `max_turns: Some(SUBAGENT_MAX_TURNS)` to bound the worker's
    //     turn budget.
    //   - The worker_token is the parent_token's child, so a user
    //     Stop that reaches the parent also fires the worker
    //     (cancel propagation).
    //
    // Boxed: `run_subagent` → `run_chat_loop` → `run_subagent`
    // (worker dispatches its own subagent? No — workers have
    // `dispatch_subagent` stripped from their tools, so the
    // recursion is bounded at depth 1). Still, the async-fn
    // recursion is statically unbounded (the compiler cannot prove
    // the depth-1 invariant), so `Box::pin` breaks the size-
    // infinite Future chain. The cost is one heap allocation per
    // worker dispatch — negligible relative to the LLM round-trip.
    //
    // 2026-06-26 (task 06-26-subagent-per-run-grant): construct a
    // fresh per-run grant cache for THIS worker. The Arc dies with
    // this `run_chat_loop` call (no shared state across workers,
    // no leakage to the parent session). L3a concurrent dispatch
    // → each worker's `run_subagent` constructs its own Arc →
    // isolated caches (a grant on one worker's `cargo` does not
    // authorize another worker's `cargo`).
    let worker_sink_dyn: Arc<dyn ChatEventSink> = worker_sink.clone();
    let run_grants =
        std::sync::Arc::new(crate::agent::permissions::run_grant::RunGrantCache::new());
    // RULE-ARGS-001：worker 嵌套调用改走三套件。历史位参注记的承载处
    // 迁入 suite.rs 对应字段文档，此处保留关键语义锚点：
    // - is_worker=Some(true)（RULE-A-014 显式传播链；Tier 4 ask 走
    //   worker-owned interactive round-trip，2026-06-22 起）；
    // - skip_persist=true（B6 PR1b：worker 中间轮只进内存/transcript，
    //   避免与父 loop 撞 (session_id, seq) UNIQUE）；skip_session_active=true
    //   （RULE-E-005：不逐父 slot）；skip_cancellations=false；
    // - max_turns=Some(SUBAGENT_MAX_TURNS)；worker_token 为父 token 子
    //   令牌（取消传播）；
    // - system_prompt_override=Some(assemble_subagent_prompt(def, task))
    //   （B6 review defect A 修复：SubagentDef.system_prompt 不再死代码）；
    // - run_grants=Some(fresh Arc)（RULE-A-016：per-worker grant 隔离，
    //   随本次 run_chat_loop 结束消亡）；
    // - worktree_override/project_main_override 透传隔离工作树（L3b/07-29）；
    // - worker_catalog=None（worker 不再 dispatch；模型解析已在上游完成）、
    //   空 stub registry（worker 永不 stub）、question_store 透父 handle
    //   （ask_user_question 在 STRUCTURALLY_DISABLED，永不触达）。
    Box::pin(run_chat_loop(
        ChatLoopRequest {
            tool_defs: worker_tool_defs,
            provider: worker_provider.clone(),
            context_window: worker_ctx,
            provider_id: worker_provider_id,
            rid: worker_rid.clone(),
            session_id: parent_session_id.to_string(),
            messages: worker_messages,
            sink: worker_sink_dyn,
            resend_seq: None,
            max_turns: Some(SUBAGENT_MAX_TURNS),
            workflow_ctx: None,
            group_chat_state: None,
            current_speaker: None,
            // worker 路径不经队列驱动器(drained 载体只在驱动器路径)。
            drained: Vec::new(),
        },
        ChatLoopDeps::from(ChatLoopDepsParts {
            db: db.clone(),
            cancellations: cancellations.clone(),
            session_active_request: session_active_request.clone(),
            read_guard: worker_read_guard.clone(),
            memory_cache: memory_cache.clone(),
            skill_cache: skill_cache.clone(),
            permission_asks: permission_asks.clone(),
            token: worker_token,
            background_shells: background_shells.clone(),
            stub_loaded: std::sync::Arc::new(crate::tools::stub::StubRegistry::new()),
            question_store: parent_question_store.clone(),
            subagent_cache: subagent_cache.clone(),
        }),
        CallerRole {
            is_worker: Some(true),
            skip_session_active: true,
            skip_persist: true,
            skip_cancellations: false,
            worker_catalog: None,
            worker_event_sink: worker_event_sink.clone(),
            system_prompt_override: Some(assemble_subagent_prompt(def, task)),
            worker_run_id: worker_run_id_opt.clone(),
            run_grants: Some(run_grants),
            worktree_override: worker_worktree_opt.clone(),
            project_main_override: project_main_override.clone(),
            app_data_dir: app_data_dir.to_path_buf(),
            forced_dispatch: None,
        },
    ))
    .await;
}
