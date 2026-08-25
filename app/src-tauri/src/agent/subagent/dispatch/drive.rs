//! run_subagent 阶段 F:drive worker(drive_worker,嵌套 run_chat_loop 调用封装)。
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
    Box::pin(run_chat_loop(
        worker_tool_defs,
        worker_provider.clone(),
        worker_ctx,
        worker_provider_id,
        worker_rid.clone(),
        parent_session_id.to_string(),
        worker_messages,
        worker_sink_dyn,
        db.clone(),
        cancellations.clone(),
        session_active_request.clone(),
        // L3b (2026-06-27): pass the (possibly reset) worker
        // ReadGuard. When isolated, this is a fresh empty guard
        // (the worker starts in a new checkout with no inherited
        // reads); when not isolated, it's a clone of the parent's
        // guard (legacy behavior).
        worker_read_guard.clone(),
        memory_cache.clone(),
        skill_cache.clone(),
        permission_asks.clone(),
        worker_token,
        None,
        background_shells.clone(),
        Some(SUBAGENT_MAX_TURNS),
        // B6 PR1b review #2: worker path — skip_session_active = true
        // so the worker's guard Drop does not evict the parent's
        // session_active_request[parent_session_id] entry.
        true,
        // B6 PR1b: worker path — skip_persist = true so the worker's
        // intermediate turns stay in-memory only. The
        // SubagentBufferSink captures them; PR2 will persist the
        // transcript into `subagent_runs`. Without this, the worker
        // would race the parent's persist_turn calls on the same
        // `(session_id, seq)` key (UNIQUE collision).
        true,
        // B6 PR2b (RULE-A-014, 2026-06-20): worker path — is_worker
        // = Some(true) so the nested run_chat_loop builds a
        // PermissionContext with is_worker: true. Pre-2026-06-22
        // (RULE-FrontSubagent-003) this collapsed Tier 4
        // ask_path / ask_shell to Decision::Deny (the worker had no
        // UI sink — a permission:ask would hang forever on the
        // oneshot); since 2026-06-22 worker asks route through the
        // `WorkerAskBanner` round-trip (see permission-layer.md §5b
        // — biased select over parent cancel / 120s timeout /
        // oneshot response). The `is_worker` flag now mainly scopes
        // the ask's internal session key (`"worker:{run_id}"`) and
        // stops a worker `AllowAlways` from persisting into the
        // parent's `session_tool_permissions` (cross-privilege
        // boundary). Pre-PR2b the worker path constructed
        // `_worker_permission_ctx` here but never threaded it into
        // the nested call, so the override was unreachable on the
        // worker's actual permission checks.
        Some(true),
        // P2.4 C5 (2026-07-22): the worker's nested `run_chat_loop`
        // receives the injected `worker_event_sink` + `catalog`
        // (replacing the forwarded `app_handle`). The worker itself
        // never dispatches a subagent (`dispatch_subagent` is
        // stripped from its toolset), so `catalog = None` is honest
        // here — the param is a dead carry-through on the worker
        // path (model resolution happened above via the sibling
        // `catalog` arg + `resolve_worker_provider`). The sink is
        // cloned (not moved) so the post-loop `subagent:finished`
        // emit below can still use it.
        None,
        worker_event_sink.clone(),
        // 2026-06-21 fix (B6 review defect A): thread the
        // worker's `SubagentDef.system_prompt` (built via
        // `assemble_subagent_prompt` above) as the 23rd
        // `system_prompt_override` parameter. When `Some`, the
        // nested `run_chat_loop` uses this string directly and
        // skips the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` step. Pre-fix the worker was getting the
        // parent's system prompt (the worker's own prompt was
        // dead code), causing prompt / permission contradictions
        // (worker told "you can write" but `is_worker=true` made
        // Tier 4 ask_path collapse to Deny pre-2026-06-22).
        Some(assemble_subagent_prompt(def, task)),
        // 2026-06-22 (RULE-FrontSubagent-003 fix): thread the
        // worker's `subagent_runs.id` (DB row UUID) so the
        // nested run_chat_loop can build the worker-owned
        // permission session id and propagate `worker_run_id`
        // into `PermissionAskPayload.worker_run_id`. `None` when
        // `insert_run` failed (no DB row → no drawer can open →
        // worker ask interactive would have nothing to route to;
        // the ask_path worker branch will fall back to a logging
        // sentinel via the unwrap_or_else in permissions::ask_path,
        // but the practical case is "spawn failed" — the parent
        // gets an Error tool_result anyway).
        worker_run_id_opt.clone(),
        // L3d (2026-06-25): thread the subagent cache so the
        // worker's own per-turn tool list construction can append
        // the dynamic `dispatch_subagent` ToolDef (the worker's
        // `filter_tools_for_subagent` then strips it via
        // `STRUCTURALLY_DISABLED`, preventing nesting). Also
        // powers any future sub-subagent dispatch (also structurally
        // disabled in MVP). The cache is shared (Arc clone), so the
        // worker sees the same mtime-fenced view as the parent.
        subagent_cache.clone(),
        // 2026-06-26 (task 06-26-subagent-per-run-grant): per-run
        // grant cache for this worker. `Some(Arc<...>)` threads
        // the cache into the worker's `PermissionContext.run_grants`
        // so `check.rs` Tier 4 can consult it before falling through
        // to `ask_path`, and the worker's `AllowAlways` arm in
        // `ask_path` can write to it. Dies with this `run_chat_loop`
        // call — no persistence to `session_tool_permissions`.
        Some(run_grants),
        // L3b (2026-06-27): the worker's isolated worktree path.
        // When `Some(path)`, the nested `run_chat_loop` uses `path`
        // as the worker's worktree root (redirecting the worker's
        // tools into the isolated checkout) INSTEAD of the parent
        // session's worktree_path. When `None`, the loop builds the
        // worktree_path from the session row (legacy shared-cwd
        // behavior).
        worker_worktree_opt.clone(),
        // project_main_override (2026-07-29): the worker's original
        // project main repo path when isolated, threaded into the nested
        // loop's `PermissionContext.project_main_path`. See the
        // `project_main_override` local above.
        project_main_override.clone(),
        // L3b (2026-06-27): thread the app_data_dir so the worker's
        // own (structurally-disabled) dispatch_subagent interceptor
        // would have it — in practice the worker never dispatches
        // a sub-subagent (STRUCTURALLY_DISABLED), so this is purely
        // for signature uniformity. We pass the same path the parent
        // passed us.
        app_data_dir.to_path_buf(),
        // explicit-agent-dispatch (2026-06-30): worker path —
        // no forced dispatch on the nested call. User-forced
        // dispatch is a parent-LLM bypass; only the parent chat
        // command honors `@@` prefixes.
        None,
        // 2026-06-30 (`ask_user_question` task): the worker's
        // toolset strips `ask_user_question` via
        // `STRUCTURALLY_DISABLED`, so the worker never reaches
        // the `chat_loop.rs` interception branch and this store
        // is unused on the worker path. We still pass it (the
        // parameter is non-optional) so the signature stays
        // shape-identical with the parent — pass the parent's
        // handle cloned, defensively. If a future change ever
        // re-enables the tool for workers, the same store is
        // already wired in (no further changes needed).
        parent_question_store.clone(),
        // W1 (Workflow integration, Phase 0 Step 0.5
        // — 2026-07-08): worker nested call passes `None`
        // — the worker focuses on its dispatched task, NOT
        // the parent session's workflow state. Workflow
        // breadcrumbs stay parent-scoped.
        None,
        // Group chat (07-29-group-chat): worker nested call passes
        // `None` — group-chat turn-taking is an outer-loop concern
        // (`run_group_chat_loop`), not a worker concern. The
        // nominate/end tools are also stripped from the worker's
        // toolset (only surfaced via builtin_tools to the moderator's
        // loop), so the interception never fires here.
        None,
        // Group chat (07-29-group-chat, Phase 4 TODO-A): worker
        // nested call passes `None` — workers don't carry a
        // speaker (they ARE the assistant's inner sub-task, not a
        // distinct participant in the outer session's transcript).
        // Additionally, the worker path above sets `skip_persist =
        // true`, so the carried value would never reach the DB
        // anyway. Pass `None` for symmetry with the parent's
        // classic-chat call site.
        None,
        // D (2026-08-14, `08-14-c7d-tools-stub-registration`):
        // worker 嵌套调用传新建空 registry — worker 永不 stub
        // (stubify/append gate `!effective_is_worker` 挡掉,拦截
        // gate `permission_ctx.is_worker` 挡掉),registry 不会被
        // 读写,只作签名占位。
        std::sync::Arc::new(crate::tools::stub::StubRegistry::new()),
        // F1 queue driver (2026-08-25): guard-owned cleanup — this call
        // site is single-shot per invocation (speaker / worker), not a
        // continuation round; keep the guard as sole owner.
        false,
    ))
    .await;
}
