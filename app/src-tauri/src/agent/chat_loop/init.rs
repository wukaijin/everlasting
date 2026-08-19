//! run_chat_loop 初始化段(拆分自 chat_loop.rs,08-08-a-class-chat-loop-split)。
//!
//! `LoopInit` struct + `prepare_loop_state` 函数:hub 内 CancellationGuard 之后、
//! forced_dispatch 之前的 session/context/messages 准备。早返路径的 emit 留在
//! 函数内部,`return Err(())` 通知 hub 直接退出。hub 全量 re-export 符号。

#![allow(unused_imports)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sqlx::SqlitePool;
use tokio::sync::Mutex;

use crate::agent::helpers::emit_chat_event_via_sink;
use crate::agent::permissions::{self, PermissionContext};
use crate::llm::{ChatEvent, ChatMessage, LlmErrorCategory, MessageContent, Role};
use crate::memory::MemoryCache;
use crate::skill::loader::SkillCache;
use crate::state::ChatEventSink;
use crate::tools::ToolContext;

use super::{dd_guard_hit, emit_persist_failure, load_for_session};

/// Output of [`prepare_loop_state`]: everything the turn loop + forced_dispatch
/// path needs after session/context/messages preparation. Early-return paths
/// (session/project not found, invalid worktree, initial-user persist failure)
/// stay inside the prepare function and surface as `Err(())` — the hub returns.
///
/// Split off `run_chat_loop` (08-08-a-class-chat-loop-split, strategy C).
pub(crate) struct LoopInit {
    pub(crate) messages: Vec<ChatMessage>,
    pub(crate) seq: i64,
    pub(crate) loaded_session: crate::db::LoadedSession,
    pub(crate) project: crate::projects::ProjectRow,
    pub(crate) worktree_path: PathBuf,
    pub(crate) current_ctx: ToolContext,
    pub(crate) last_cwd: Option<PathBuf>,
    pub(crate) last_usage_terminal: Option<crate::llm::types::TokenUsage>,
    pub(crate) failure_tracker: Arc<Mutex<crate::agent::auto_reflect::FailureTracker>>,
    pub(crate) soft_blocked: Arc<Mutex<std::collections::HashSet<String>>>,
    pub(crate) session_mode: crate::db::Mode,
    pub(crate) effective_is_worker: bool,
    pub(crate) permission_ctx: PermissionContext,
    pub(crate) mode_prefix: &'static str,
    pub(crate) model_briefs: Vec<crate::agent::subagent::ModelBrief>,
    pub(crate) head_sha: String,
    pub(crate) system_prompt: String,
    /// memory-block-governance WP2 (2026-08-15): digest gate 结果
    /// (`开关 && !worker && !群聊`)。drive_turn 依此侧挂
    /// `load_memory_sections` 元工具 def(与注入 gate 同源)。
    pub(crate) digest_on: bool,
    /// memory-block-governance WP1 (2026-08-15): cl100k estimate
    /// of the memory instruction blocks actually injected into
    /// `messages` this request (banner + wrappers + layer bodies).
    /// Per-request constant — every turn_trace row of the request
    /// carries the same value via the Done-event upsert. `None`
    /// when no layers loaded (fresh install). Worker path note:
    /// workers inject via `subagent/prompt.rs` instead and never
    /// populate this (design §3.5a).
    pub(crate) memory_token: Option<u32>,
    /// unified-context-budget WP1 (2026-08-19): cl100k estimate of
    /// the system prompt body (send-side part) + the skill-listing
    /// synthetic message (attribution 口径: physically inside
    /// messages, attributed to the system slice, design §2).
    /// Per-request constant like memory_token; head_sha-driven
    /// per-turn system rebuilds change a few chars, absorbed by the
    /// estimate. Worker turns never reach the Done write point
    /// (skip_persist gate), same as memory_token.
    pub(crate) system_token: u32,
    /// unified-context-budget WP1 (2026-08-19): sum of the @-token
    /// injected-body token estimates across ALL user messages of the
    /// request (`inject_at_tokens` spans aggregate). Per-request
    /// constant; 0 when nothing was injected.
    pub(crate) at_files_token: u32,
    /// unified-context-budget WP1 (2026-08-19, prd D10): the
    /// same-request @文件 spans — ephemeral (never persisted; @文件
    /// are re-expanded per request so DB spans would be stale by
    /// construction). Threaded to `drive_turn` for the WP2 budget
    /// gate to consume within this request; PR1 only produces them
    /// (same reservation pattern as `synthetic_prefix_len` in C3
    /// PR1). APPEND-only turn assembly keeps the offsets stable
    /// (AC3).
    pub(crate) at_file_spans: Vec<crate::agent::at_file::AtFileSpan>,
    /// C3 摘要压缩 PR1 (08-18-llm-context-compaction):
    /// init 时水位替换命中则种子为水位摘要(`SummaryAnchor`,
    /// 纯摘要正文 + DB 行 seq),PR2 的 drive_turn 压缩路径经
    /// `DriveTurnOutcome` 循环内穿参更新(同 `loop_hit_count`
    /// 模式),供增量合并的 `<prior-summary>` 注入(评审 P1-1
    /// 修正:不按"摘要落在位置 2"猜)。`None` = 本请求无水位
    /// (未压缩过 / gate 关 / 对齐 fail-open)。
    pub(crate) summary_anchor: Option<crate::agent::compaction::SummaryAnchor>,
    /// C3 摘要压缩 PR1 预留(PR2 开始读取):init 完成全部合成插入
    /// (B5 memory 头对 ×2 + B4 skill listing ×1)后的合成头长度。
    /// PR2 的待压区与保留区都从这里起算,不把每请求重注入的
    /// skill listing 喂给摘要(design §4.1,评审 P2-2)。恒有值
    /// (无合成插入时为 0),与水位是否命中无关。
    pub(crate) synthetic_prefix_len: usize,
    /// C3 摘要压缩 PR2(08-18-llm-context-compaction):摘要压缩
    /// 总 gate(`llm_compaction_enabled && !worker && !群聊`,与
    /// 上面水位替换的 `compaction_on` 同源同值 —— 每 request 读
    /// 一次 config,穿 LoopInit 免去 drive 侧重复读库)。drive_turn
    /// 的 C3 块据此决定超线时先尝试摘要还是直走机械;熔断
    /// (`compaction_registry`)是 drive 侧的第二重独立 gate。
    pub(crate) compaction_on: bool,
}

/// run_chat_loop 初始化段(L591–1262):CancellationGuard 构造之后、
/// forced_dispatch 之前的全部 session/context/messages 准备。早返路径
/// (session/project not found、worktree invalid、initial-user persist 失败)
/// 的 emit 原样留在本函数内部,`return Err(())` 通知 hub 直接退出。
///
/// Split off `run_chat_loop` (08-08-a-class-chat-loop-split, strategy C).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn prepare_loop_state(
    db: SqlitePool,
    sink: Arc<dyn ChatEventSink>,
    rid: String,
    session_id: String,
    messages: Vec<ChatMessage>,
    memory_cache: Arc<MemoryCache>,
    skill_cache: Arc<SkillCache>,
    worktree_override: Option<PathBuf>,
    project_main_override: Option<PathBuf>,
    background_shells: crate::background_shell::DefaultRegistry,
    app_data_dir: PathBuf,
    workflow_ctx: &Option<crate::agent::workflow::WorkflowCtx>,
    is_worker: Option<bool>,
    worker_run_id: Option<String>,
    run_grants: Option<std::sync::Arc<crate::agent::permissions::RunGrantCache>>,
    system_prompt_override: Option<String>,
    skip_persist: bool,
    resend_seq: Option<i64>,
    group_chat_state: &Option<crate::tools::nominate_speaker::SharedTurnState>,
) -> Result<LoopInit, ()> {
    let mut messages = messages;

    // Start seq from the highest existing seq in this session + 1.
    let loaded_session = match crate::db::load_session(&db, &session_id).await {
        Ok(Some(loaded)) => loaded,
        Ok(None) => {
            tracing::warn!(session_id = %session_id, "session not found");
            sink.emit_chat_event(&crate::state::ChatEventPayload {
                request_id: rid.clone(),
                event: ChatEvent::Error {
                    message: format!("session {} not found", session_id),
                    category: LlmErrorCategory::InvalidRequest,
                },
            });
            return Err(());
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to load session");
            return Err(());
        }
    };
    let next_seq = loaded_session
        .messages
        .iter()
        .map(|m| m.seq)
        .max()
        .map(|s| s + 1)
        .unwrap_or(0);
    let mut seq = next_seq;

    // The agent loop uses a directory-bound worktree + cwd. The
    // test setup creates a project whose `path` we use directly
    // (no worktree); we read it from the session's project.
    let project = match crate::db::get_project(&db, &loaded_session.session.project_id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            sink.emit_chat_event(&crate::state::ChatEventPayload {
                request_id: rid.clone(),
                event: ChatEvent::Error {
                    message: format!(
                        "project {} not found for this session",
                        loaded_session.session.project_id
                    ),
                    category: LlmErrorCategory::InvalidRequest,
                },
            });
            return Err(());
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to load project");
            return Err(());
        }
    };
    let session_root_raw = loaded_session
        .session
        .worktree_path
        .clone()
        .unwrap_or_else(|| project.path.clone());
    // L3b (2026-06-27): when `worktree_override` is `Some(path)`,
    // use the override INSTEAD of the session row's worktree_path.
    // The override is the worker's isolated git worktree (created
    // by `git::worktree::create_worker`); the session row's
    // worktree_path is the PARENT session's worktree, which is the
    // root cause of worker reuse of the parent's checkout. The
    // override path is already asserted to be inside the project
    // root by `run_subagent` before being passed here, so the
    // `assert_within_root` call below still passes (we use the
    // override as both the path AND the canonicalization target).
    let session_root_raw = worktree_override
        .clone()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or(session_root_raw);
    let worktree_path = match crate::projects::boundary::assert_within_root(
        std::path::Path::new(&session_root_raw),
        std::path::Path::new(&session_root_raw),
    ) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(session_id = %session_id, error = %e, "session root invalid");
            sink.emit_chat_event(&crate::state::ChatEventPayload {
                request_id: rid.clone(),
                event: ChatEvent::Error {
                    message: format!("session root is invalid: {}", e),
                    category: LlmErrorCategory::InvalidRequest,
                },
            });
            return Err(());
        }
    };
    // L3b (2026-06-27): a worker worktree is a fresh checkout with
    // no `current_cwd` history — the worker starts at the worktree
    // root, NOT the parent session's `current_cwd` (which would
    // point at a path inside the parent's checkout, not the
    // worker's). The override path wins; non-override path keeps
    // the legacy behavior (read `current_cwd` from the session row).
    let session_cwd_raw =
        if worktree_override.is_some() || loaded_session.session.current_cwd.is_empty() {
            worktree_path.to_string_lossy().to_string()
        } else {
            loaded_session.session.current_cwd.clone()
        };
    let session_cwd = match crate::projects::boundary::assert_within_root(
        &worktree_path,
        std::path::Path::new(&session_cwd_raw),
    ) {
        Ok(p) => p,
        Err(_) => worktree_path.clone(),
    };
    // project_main_path (2026-07-29): the inside-check anchor for the
    // permission layer. For a non-isolated worker / parent session,
    // `worktree_path` IS the project root → fall back to it. For an
    // isolated worker, `worktree_path` is its checkout subtree and the
    // real project root comes via `project_main_override` (set by
    // `run_subagent` to the project's main repo path). Canonicalize the
    // override to match `is_within_root`'s lexical expectation; if it's
    // missing/invalid (tests, degenerate cases) fall back to worktree_path
    // so behavior matches the old code rather than panicking.
    let project_main_path = match &project_main_override {
        Some(p) if !p.as_os_str().is_empty() => {
            crate::projects::boundary::resolve_path(&p.to_string_lossy(), &worktree_path)
        }
        _ => worktree_path.clone(),
    };
    let turn_ctx = ToolContext {
        worktree_path: worktree_path.clone(),
        cwd: session_cwd.clone(),
        // B12 (2026-06-19): per-request checklist handle. Constructed
        // fresh for each `run_chat_loop` call so a new user message
        // (or D3 resend fork) starts with an empty list. The handle
        // is threaded through `ToolContext` so `update_checklist::execute`
        // can atomically mutate it; the same handle is read every turn
        // to build the ephemeral injection block (see `inject_checklist`
        // below).
        checklist: crate::tools::update_checklist::new_handle(),
        // L1a (2026-06-19): cross-request background-shell registry.
        // Pulled from `AppState` (which owns the single in-memory
        // impl); tools consume it from `ToolContext` so the registry
        // isn't plumbed through every tool signature.
        background_shells: background_shells.clone(),
        // L3b PR3 (2026-06-27): DB pool for the `merge_worker` /
        // `discard_worker` tools. These tools read the
        // `subagent_runs` row to find the worker worktree path +
        // the parent project root, then call libgit2 to merge /
        // destroy. The pool is `Clone` (Arc-internal) so the
        // per-turn `ToolContext::clone()` pattern is unaffected.
        db: db.clone(),
        // P2 (2026-06-29): the session's `projects.id` UUID. The
        // `remember` tool binds project-scope memories to this id;
        // the session-start recall filters by the same id. Worker
        // subagents reuse the parent's project (their worktree is a
        // checkout OF the parent's project), so the worker path
        // also carries the parent's project_id.
        project_id: project.id.clone(),
        // 06-30 follow-up: pass through the app-global data
        // directory so tool-layer helpers that need to construct
        // absolute paths (e.g. lazy auto-attach on the
        // merge_worker path) can read it from `ToolContext`
        // without changing every tool-execute signature. The
        // value here is identical to `state.app_data_dir` — we
        // clone the `app_data_dir` parameter that's already in
        // scope on the chat_loop function.
        data_dir: app_data_dir.clone(),
        // Step 1.5 (07-08-workflow-integration): propagate the
        // active plugin name so `tools::use_skill` can load
        // plugin-layer skills (e.g. `wf-overview`). `None` for
        // non-workflow sessions — the loader treats that as
        // "no plugin layer, fall through to project/user".
        // `workflow_ctx` is already in scope (declared on the
        // `run_chat_loop` signature at line 406) and is
        // populated by `lib.rs::chat` from the session's
        // `workflow_enabled` toggle + the active plugin.
        workflow_name: workflow_ctx.as_ref().map(|c| c.workflow_def.name.clone()),
    };
    let current_ctx = turn_ctx;
    let last_cwd: Option<PathBuf> = None;
    // 2026-06-21 (R3): the per-turn `last_usage` is re-declared
    // at the top of each iteration of the `for turn in 1..=turn_limit`
    // loop, so the synthetic `max_turns` terminal site
    // (chat_loop.rs:1797-1820) cannot read it directly. Track
    // the most recent value here at the function scope so the
    // terminal site can forward it to the sink (and the sink
    // can route it into `cumulative_usage()` exactly once, via
    // the R3 stop_reason guard). Pre-R3 the synthetic terminal
    // hard-coded `usage: None`, which produced the
    // `subagent_runs.token_usage_json == 0` regression on
    // `max_turns` exits (c27f3fd7 worker run).
    let last_usage_terminal: Option<crate::llm::types::TokenUsage> = None;

    // P4 (2026-06-29, 06-29-am-p4-event-reflect): per-session
    // failure tracker. Created once at the top of `run_chat_loop`
    // and shared (via `Arc`) across the two tool-emit sites —
    // the parallel-batch L2 path's `FuturesUnordered` task AND
    // the serial path's `for (id, name, input) in &tool_calls`
    // loop both feed outcomes into the same tracker. When the
    // "≥2 consecutive failures → success" pattern lands for
    // a tool, the tracker fires a fire-and-forget LLM reflection
    // that produces a `kind=pitfall, status=active` row in
    // `autonomous_memories` — which the P3 pre-tool recall
    // surfaces on the next session (or even later in the same
    // session if a worker re-tries the same operation). v1
    // accepts session-boundary reset (no cross-session carry
    // of "tools that were flaky yesterday"; spike-007 §10
    // extension point).
    let failure_tracker = Arc::new(Mutex::new(crate::agent::auto_reflect::FailureTracker::new()));

    // P5 (2026-06-29, 06-29-am-p5-quality): session-scoped soft-block
    //记账. When a verified pitfall soft-blocks a tool_use, its
    // memory_id lands here; the next hit on the same pitfall degrades
    // to Footnote + normal execution (the dead-loop guard, design D1).
    // Same lifecycle as `failure_tracker` (loop-local, dropped on
    // exit) — no cross-session carry.
    let soft_blocked: Arc<Mutex<std::collections::HashSet<String>>> = Arc::default();

    let session_mode = loaded_session.session.mode;
    // B6 PR2b (RULE-A-014, 2026-06-20): the `is_worker` parameter
    // (added as the 21st arg) threads the worker path's
    // `PermissionContext.is_worker = true` override into the loop
    // body. The 2026-06-22 fix (RULE-FrontSubagent-003) further
    // added `worker_run_id` (the 24th arg) so `ask_path` can route
    // worker asks via a worker-owned permission session id
    // (`"worker:<worker_run_id>"`) + propagate `worker_run_id`
    // into the IPC payload for frontend routing. Pre-fix (PR2b)
    // the worker path collapsed Tier 4 ask_path → Deny (no UI
    // sink — would hang on oneshot); post-fix the worker enters
    // the interactive round-trip and waits for the user. Yolo
    // mode still bypasses the whole Tier 4 above (in `check`),
    // so a worker under Yolo never reaches `ask_path`.
    let effective_is_worker = is_worker.unwrap_or(false);
    let permission_ctx = PermissionContext {
        session_id: session_id.clone(),
        mode: session_mode,
        cwd: session_cwd.clone(),
        is_worker: effective_is_worker,
        // 2026-06-22 (RULE-FrontSubagent-003 fix): carry the
        // worker_run_id through so `ask_path` can build the
        // worker-owned permission session id and propagate the
        // worker_run_id into `PermissionAskPayload.worker_run_id`.
        // `None` for the parent path (production chat + tests);
        // `Some(...)` for the worker nested call. The
        // `effective_is_worker` gate above is the actual
        // "is this a worker?" predicate — this field is just
        // the routing key, used only when `effective_is_worker`
        // is true.
        worker_run_id: worker_run_id.clone(),
        // 2026-06-26 (task 06-26-subagent-per-run-grant): per-run
        // in-memory grant cache. `None` for the parent path
        // (production chat + tests) — the Tier 4 grant-check
        // branches in `check.rs` skip the cache lookup entirely
        // when this is `None`. `Some(Arc<...>)` for the worker
        // path — the Arc is constructed fresh in `run_subagent`
        // per worker, so concurrent workers have isolated caches.
        run_grants: run_grants.clone(),
        // read-side boundary decouple (2026-07-01): deny-list/allow-list
        // 的"项目外"判定锚点(项目根). 见 PermissionContext.worktree_path doc.
        worktree_path: worktree_path.clone(),
        // 2026-07-29: inside-check anchor (项目根). 隔离 worker 的
        // worktree_path 指向 checkout 子树,不能作锚点 —— 用真实项目根.
        project_main_path: project_main_path.clone(),
        // E2 trace: per-turn seq, updated at the top of each turn
        // before the tool-execution phase. None at construction
        // (pre-turn-loop); the turn loop sets Some(seq) per turn.
        turn_seq: None,
    };
    let mode_prefix = permissions::mode_system_prefix(session_mode);

    // B6+ B (task 07-06-b6plus-b-dispatch-model-arg): snapshot the
    // model list once per `run_chat_loop` invocation to build the
    // dynamic `model` enum on the `dispatch_subagent` tool schema
    // (display_name values — the system prompt does not list models,
    // so the enum is the LLM's only discovery channel). Placed here
    // (after `effective_is_worker` is known and `db` is in scope)
    // but still OUTSIDE the turn loop (1511), so it runs once per
    // chat invocation regardless of turn count. The worker path
    // snapshots too (harmless — `definition_with_cache` is gated on
    // `effective_is_worker == false` below, so the worker never
    // consumes the snapshot). Models change at low frequency; CRUD
    // during a session is reflected next session and covered by the
    // catalog-miss fallback in `resolve_worker_provider`.
    let model_briefs: Vec<crate::agent::subagent::ModelBrief> =
        match crate::db::list_models(&db).await {
            Ok(rows) => rows
                .into_iter()
                .map(|mwp| crate::agent::subagent::ModelBrief {
                    id: mwp.model.id,
                    display_name: mwp.model.display_name,
                })
                .collect(),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "list_models snapshot failed; dispatch_subagent `model` enum will be empty"
                );
                vec![]
            }
        };

    // B5 memory is empty in tests (no memory files written to the
    // temp project dir). Skip the synthetic user/assistant
    // inserts when `load_for_session` returns no layers.
    let memory_layers = load_for_session(&memory_cache, &project.id, &project.path).await;
    // memory-block-governance WP2 (2026-08-15): digest gate — 开关
    // (best-effort 缺省 on,`"false"` 才关,fail-open 同 `tools_stub_enabled`)
    // && 非 worker && 非群聊(与 C7D gate 同款豁免口径;worker 注入走
    // `subagent/prompt.rs` 本就不经过这里,群聊参与者在 init 路径上被
    // 短路)。已加载节从进程级 registry 现取(mtime fence 保证层内容
    // 新鲜,registry 只管粘性)。
    let is_group_chat = loaded_session.session.session_type == crate::db::SessionType::GroupChat;
    let digest_on = match crate::db::config::get_config_value(&db, "memory_digest_enabled").await {
        Ok(Some(v)) => v != "false",
        _ => true,
    } && !effective_is_worker
        && !is_group_chat;
    // C3 摘要压缩 PR1 (08-18-llm-context-compaction): 水位替换 gate,
    // 口径照抄上面的 digest gate —— 开关(best-effort 缺省 on,
    // `"false"` 才关,fail-open 同 `tools_stub_enabled`)&& !worker
    // && !群聊(`session_type` 判定,worker/群聊路径不进这条替换:
    // worker 有 200 turn + resume 兜,群聊有 30 轮编排上限)。
    // 开关关 = 水位替换停用,完全回到 main 行为(design §10 回滚点)。
    let compaction_on =
        match crate::db::config::get_config_value(&db, "llm_compaction_enabled").await {
            Ok(Some(v)) => v != "false",
            _ => true,
        } && !effective_is_worker
            && !is_group_chat;
    // C3 水位替换:对 raw wire 历史操作,**必须发生在下方 B5 memory
    // 头对 insert(0/1) 与 B4 skill listing 插入之前** —— 头对照常落
    // 在替换后列表的 0-1 位,摘要消息在合成头之后(位置 ≥ 2,memory
    // cache 断点不 bust,design §3)。DB 是 SoT:wire 层 ChatMessage
    // 无 metadata 字段,前端无法告知 kind;`loaded_session.messages`
    // 在上面已现成加载,零额外查询。算法与对齐防御见
    // `agent::compaction` 模块文档(依赖前端 reloadAfterFinalize
    // 保证 wire 含摘要行 —— 评审 P1-3)。
    let mut summary_anchor: Option<crate::agent::compaction::SummaryAnchor> = None;
    if compaction_on {
        match crate::agent::compaction::apply_compaction_watermark(
            messages,
            &loaded_session.messages,
        ) {
            crate::agent::compaction::WatermarkResult::Applied {
                messages: folded,
                anchor,
            } => {
                // PR1:摘要消息直接用 DB 行 content(纯摘要,无前缀
                // 话术 —— 前缀在 PR2 的 in-context 构建时拼接,不落库,
                // 评审 P1-2)。`summary_anchor` 是 PR2 增量合并的种子
                // (`DriveTurnOutcome` 循环内穿参,design §4.2)。
                messages = folded;
                summary_anchor = Some(anchor);
            }
            crate::agent::compaction::WatermarkResult::Miss {
                messages: original,
                reason,
            } => {
                // 对齐失败 ≠ 哑失败(评审 P1-3):warn 可观测后
                // fail-open 回全量历史 = main 行为。本 PR 只用
                // tracing 落 watermark_miss;DB trace 扩展属 PR2。
                if let crate::agent::compaction::MissReason::AlignmentFailed { summary_seq } =
                    reason
                {
                    tracing::warn!(
                        session_id = %session_id,
                        summary_seq,
                        "watermark_miss: compaction summary row present but wire history does not align; fail-open to full history"
                    );
                }
                messages = original;
            }
        }
    }
    let instructions_blocks = if digest_on {
        let loaded_sections = crate::memory::digest::registry().get(&session_id).await;
        crate::memory::loader::build_instructions_blocks_with_digest(
            &memory_layers,
            true,
            &loaded_sections,
        )
    } else {
        crate::memory::loader::build_instructions_blocks(&memory_layers)
    };
    let has_memory = !instructions_blocks.is_empty();
    // memory-block-governance WP1 (2026-08-15): cl100k estimate of
    // the blocks actually injected below — banner + wrappers +
    // bodies, i.e. slightly above the banner's per-file token sums
    // (wrapper overhead). Computed once per request; threaded to
    // the Done-event `upsert_turn_trace_token` via `LoopInit` /
    // `drive_turn` (same write point as `tools_token`).
    let memory_token = if instructions_blocks.is_empty() {
        None
    } else {
        let joined = instructions_blocks
            .iter()
            .filter_map(|b| match b {
                crate::llm::types::ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        Some(crate::memory::tokens::count_tokens(&joined).await)
    };
    if !instructions_blocks.is_empty() {
        messages.insert(
            0,
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(instructions_blocks),
                speaker: None,
                attachments: None,
            },
        );
        messages.insert(
            1,
            ChatMessage {
                role: Role::Assistant,
                content: MessageContent::Text(
                    "Understood. I will follow these instructions throughout our session."
                        .to_string(),
                ),
                speaker: None,
                attachments: None,
            },
        );
    }

    // B4 skill listing (L0): an independent synthetic user message,
    // decoupled from the memory instructions cache window so skill
    // add/remove does not bust the memory cache breakpoint (PR2
    // brainstorm Q1 decision). Empty when no skill files exist —
    // skipped, symmetric to the memory `instructions_blocks.is_empty()`
    // guard above.
    //
    // Uses `worktree_path` (not `project.path`) so the L0 listing
    // resolves from the same dir the `use_skill` L1 activation
    // (`tools/use_skill.rs`, via `ctx.worktree_path`) consults —
    // otherwise a worktree-attached session would list skills from
    // the main project root but resolve them from the worktree,
    // turning a matching listing into a "not found" on L1.
    // (`worktree_path` already went through `assert_within_root`
    // canonicalize above, so symlinks are resolved consistently on
    // both sides; `SkillCache` keys by the path string, so the L0
    // + L1 cache slots line up.)
    //
    // Step 1.1 (07-08-workflow-integration): when the session is a
    // workflow session, also consult the plugin layer (highest
    // precedence: plugin > project > user). The plugin layer reads
    // `<project>/.everlasting/workflow/<name>/skills/` and silently
    // falls through to project / user when the plugin dir is absent
    // — non-workflow callers get the old project-overrides-user
    // behavior byte-identical (see skill::loader::merge_skill_layers).
    let skill_listing_path = worktree_path.to_string_lossy().to_string();
    let skill_wf_name = workflow_ctx
        .as_ref()
        .map(|ctx| ctx.workflow_def.name.clone());
    let skill_infos = crate::skill::loader::list_skill_infos_with_workflow(
        &skill_cache,
        Some(&skill_listing_path),
        skill_wf_name.as_deref(),
    )
    .await;
    let skill_blocks = crate::skill::loader::build_skill_listing_block(&skill_infos);
    // C3 PR1 预留:bool 先行捕获(skill_blocks 会被 move 进下面的
    // insert),`synthetic_prefix_len` 依赖它。
    let has_skill_listing = !skill_blocks.is_empty();
    // unified-context-budget WP1 (2026-08-19): skill listing 的归因
    // 切片计数 —— 物理上它随下面的 insert 进 messages(messages 部件
    // 估算已含),这里只取归因值汇入 `system_token`(design §2:system
    // 切片 = system prompt 本体 + skill listing 归因)。必须在 move 前
    // 从 `skill_blocks` 取文本。
    let skill_listing_token = if skill_blocks.is_empty() {
        0
    } else {
        let joined = skill_blocks
            .iter()
            .filter_map(|b| match b {
                crate::llm::types::ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        crate::memory::tokens::count_tokens(&joined).await
    };
    if !skill_blocks.is_empty() {
        // Insert after the memory user/assistant pair (pos 2) when
        // memory is present, else at the head (pos 0).
        let skill_pos = if has_memory { 2 } else { 0 };
        messages.insert(
            skill_pos,
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(skill_blocks),
                speaker: None,
                attachments: None,
            },
        );
    }
    // C3 摘要压缩 PR1 预留 (08-18-llm-context-compaction): init 完成
    // 全部合成插入(B5 头对 insert(0/1) + B4 skill listing)后的
    // 合成头长度 —— PR2 的待压区/保留区从这里起算(design §4.1)。
    // 三种布局:memory+skills → 3 / 仅 memory → 2 / 仅 skills → 1 /
    // 都无 → 0。摘要消息(若水位替换发生)落在合成头之后,位置随
    // 布局漂移,这正是 PR2 用 `SummaryAnchor` 而非位置猜测的原因
    // (评审 P1-1)。
    let synthetic_prefix_len =
        (if has_memory { 2 } else { 0 }) + (if has_skill_listing { 1 } else { 0 });

    // P2 RULE-A-005 (2026-06-24, fix 1 of 3 P2 open rules):
    // `head_sha` is now MUTABLE and refreshed at the start of every
    // turn (before `provider.send`) so the LLM sees the current HEAD
    // after a mid-session commit. Pre-fix: `head_sha` was a one-shot
    // `let` at chat_loop.rs:492 — the 50-turn loop sent a stale SHA
    // for every turn after turn 1, drifting the LLM's mental model of
    // the repo state. The cost is one extra `lookup_head_sha` (libgit2
    // `Repository::open` + `head().peel_to_commit()`) per turn —
    // negligible relative to LLM network latency.
    //
    // Cache-correctness (RULE-A-005 invariant, verified in
    // prd §6.1): the head_sha field lives inside `build_system_prompt`
    // output, which is fed into the provider's **system** role string.
    // The 4 instruction files (User/Project × CLAUDE.md/AGENTS.md)
    // are injected as a SEPARATE user-role synthetic message via
    // `memory::loader::build_instructions_blocks` and carry their own
    // `cache_control: Ephemeral` breakpoint — independent of the
    // system role. So a per-turn system-prompt mutation does NOT
    // bust the memory cache. The 4 instruction blocks stay cache-hot
    // across the 50-turn loop.
    let head_sha = crate::agent::system_prompt::lookup_head_sha(&worktree_path);
    // The 2026-06-21 B6 review defect A fix (the worker's
    // `SubagentDef.system_prompt` override via the 23rd parameter)
    // short-circuits below — when `Some(p)`, the worker uses `p`
    // directly and never calls `assemble_system_prompt` or
    // `build_system_prompt`. The production + 35 test path passes
    // `None`, so this branch runs on every parent turn.
    let system_prompt = match system_prompt_override {
        Some(ref p) => p.clone(),
        None => {
            let base_prompt = crate::agent::system_prompt::build_system_prompt(
                &loaded_session.session,
                &project,
                &worktree_path,
                &head_sha,
            );
            crate::agent::system_prompt::assemble_system_prompt(mode_prefix, &base_prompt)
        }
    };
    // unified-context-budget WP1 (2026-08-19): system 切片 = system
    // prompt 本体(发送部件)+ skill listing(归因,上面已计)。请求
    // 常量:head_sha 每轮重建只动几个字符,absorbed(见 LoopInit 字段
    // 注释)。注意与 drive_turn 每轮的 overhead 估算区分 —— 那是
    // `count_tokens(当前 system_prompt)`(不含 skill listing,它在
    // messages 里),两个口径各自独立(prd D8)。
    let system_token =
        crate::memory::tokens::count_tokens(&system_prompt).await + skill_listing_token;

    // Persist the most recent user message before the agent loop runs.
    //
    // B2 PR3 (2026-06-17): also snap the original (pre-inject)
    // content for the `persist_turn` call below. PR2 stores the
    // raw `@relpath` text as source of truth; PR3 adds the
    // injection manifest to `messages.metadata` so the frontend
    // hint row survives session reload. We keep BOTH the
    // original content (DB `content` + `text` columns) and the
    // manifest (DB `metadata` JSON) — the user sees the
    // original `@relpath` in the bubble and the hint row below
    // it; a reload reads both back.
    //
    // We capture the seq now (before persist) so the
    // `ChatEvent::FileInjections` event below can identify the
    // user row to the frontend (the controller's user-message
    // keys on reload are `${sid}-${seq}`, so `message_seq`
    // round-trips through the DB and matches the rehydrated
    // key).
    //
    // Group chat (08-04 dedup rewrite, design.md D-D): detect when the
    // tail user message is ALREADY in the DB (happens in group_chat's
    // per-speaker loop, where `reload_messages` feeds an already-
    // persisted tool_result — stored as role=user — back in as the
    // tail). Blindly re-persisting it wrote a duplicate tool_result row
    // with no matching tool_calls, which OpenAI rejects with HTTP 400
    // "Messages with role 'tool' must be a response to a preceding
    // message with 'tool_calls'", erroring every subsequent turn. When
    // the tail user message is already in the DB, skip the persist +
    // resend audit + seq bump, and point `user_seq` at the existing row
    // so the FileInjections metadata update below still targets the
    // right row. Normal single-agent chat is unaffected: its tail is a
    // freshly-composed user message that does NOT match any DB row, so
    // this guard returns false and the original path runs verbatim.
    //
    // The guard replaces the pre-08-04 heuristic (design D-F): the old
    // version required `messages.len() == loaded_session.messages.len()`,
    // which is always false once the memory/skill injection pass above
    // inserts synthetic user rows into `messages` — and it was NOT
    // scoped to `group_chat_state`, so it could never fire at all. The
    // new version (a) requires `group_chat_state.is_some()` (a group-
    // chat speaker — ordinary chat never enters), and (b) content-matches
    // the tail user message against ANY user-role row in the loaded
    // session (not just the tail / not length-based), so a filtered
    // participant view (fewer rows than the DB) is still matched
    // correctly.
    //
    // Known cosmetic boundary (design §5): a human re-sending the EXACT
    // same text in a group chat (cancel + resend identical text) is
    // judged already-persisted and skipped → the transcript loses one
    // text row. Harmless: it does not break tool pairing and never
    // triggers a 400.
    //
    // P0-1/P0-3 extension (08-07-group-chat-role-history-isolation):
    // `role_history` rewrite products — other speakers' utterances
    // rewritten as `role:user` + `speaker` — would ALSO be judged "new
    // human messages" here (their original DB rows are assistant, so
    // `user_message_matches` can never hit) and re-persisted every
    // round. `dd_guard_hit` treats any tail user row with
    // `speaker.is_some()` as already persisted (safety: in the
    // group-chat codebase only rewrite products carry speaker on a
    // user row), anchoring `seq` on the tail-most user row, and its
    // `None` snapshot disables the at_file injection pass below for
    // those rows.
    let (last_user_snapshot, last_user_seq) =
        if let Some(last_user) = messages.iter().rev().find(|m| m.role == Role::User) {
            let msg = last_user.clone();
            // P0-1 + P0-3 (08-07-group-chat-role-history-isolation):
            // the guard now also treats a tail user row carrying
            // `speaker` as already persisted — such a row can ONLY be
            // a `role_history` rewrite product (an other-speaker
            // utterance rewritten as user; the original DB row is
            // assistant, so content-matching can never hit). Without
            // this, every rewrite row would be judged a "new human
            // message" and re-persisted every round → DB pollution +
            // frontend ghost rows. The seq anchors on the tail-most
            // user row; the snapshot is None so at_file injection
            // below is skipped (P0-3).
            let already_in_db = dd_guard_hit(
                skip_persist,
                group_chat_state.as_ref(),
                &loaded_session.messages,
                &msg,
            );
            if let Some(hit) = already_in_db {
                // Already persisted (group_chat reload path): do not
                // write a new row, do not bump seq. `user_seq` points
                // at the existing row so FileInjections metadata
                // update + memory injection target the right message.
                // P0-3: a speaker-carrying hit (rewrite product) returns
                // `None` for the snapshot → the at_file injection
                // condition below stays false (no manifest write to the
                // wrong seq row, no misplaced FileInjections event).
                let user_seq = hit.seq;
                (hit.snapshot, user_seq)
            } else {
                // B6 PR1b: in the worker path, skip ALL DB writes (see
                // `skip_persist` docstring at the function head). The
                // worker still bumps the in-memory `seq` and pushes into
                // `messages` so the agent loop stays coherent, but it
                // NEVER writes to the parent's `messages` table (the
                // SubagentBufferSink captures the transcript for PR2).
                //
                // RULE-A-003 (2026-06-15): if the very first user message
                // can't be persisted, abort with a visible Error —
                // continuing would let the LLM answer a message the DB
                // never recorded, so the next session reload is blank.
                if !skip_persist {
                    if let Err(e) = crate::db::persist_turn(
                        &db,
                        &session_id,
                        msg.role,
                        &msg.content,
                        seq,
                        None,
                        msg.speaker.as_deref(),
                    )
                    .await
                    {
                        emit_persist_failure(&sink, &rid, &e);
                        return Err(());
                    }
                }
                // D3 PR3 (2026-06-17): if the user hit Resend (instead of
                // Edit), the frontend passed `resend_seq` through the chat
                // IPC. Fire the `resend_message` audit row pointing at
                // the original user message's seq (the one the user clicked
                // Resend on). Best-effort: a failure is logged + swallowed
                // — audit loss is acceptable here because the user has
                // already seen the visual confirmation (the new assistant
                // turn is about to stream). The `content_text_preview`
                // comes from the ORIGINAL message's content (truncated to
                // 80 chars inside the helper), not the new send's text —
                // they're identical because Resend re-fires the same
                // prompt, but we use the ORIGINAL seq to keep the audit
                // link obvious ("you re-ran this row at T").
                //
                // Sits AFTER persist_turn so the audit row's payload can
                // safely reference `seq` (the original row's seq — the
                // user message we just persisted is a NEW row with seq=N+1,
                // not the one being re-run). The `resend_seq` is the seq
                // of the ORIGINAL user message; the new send uses seq=N+1.
                if let Some(original_seq) = resend_seq {
                    // B6 PR1b: skip audit writes in the worker path (see
                    // `skip_persist` docstring). The resend audit is
                    // user-message scope; workers don't observe user
                    // resends.
                    if !skip_persist {
                        // Derive a short text preview from the original
                        // message's content. `MessageContent` carries
                        // `to_text()` which concatenates all text blocks
                        // (mirrors the `text` column write). We use the
                        // in-memory `msg` (which equals what just got
                        // persisted) — same text, same preview budget.
                        let preview = msg.content.to_text();
                        if let Err(e) = crate::agent::permissions::record_message_resend_audit(
                            &db,
                            &session_id,
                            original_seq,
                            &preview,
                            None,
                        )
                        .await
                        {
                            tracing::warn!(
                                    error = %e,
                                    request_id = %rid,
                                    session_id = %session_id,
                                    original_seq = original_seq,
                                    "chat_loop: record_message_resend_audit failed (non-fatal)"
                            );
                        }
                    }
                }
                // B2 PR3: snap the seq for the FileInjections event;
                // the original (un-injected) content stays in the
                // `messages` vec at this point because the inject
                // pass below mutates the in-memory copy in place —
                // but the DB row is already locked to the original.
                let user_seq = seq;
                seq += 1;
                (Some(msg.content), user_seq)
            }
        } else {
            (None, -1)
        };

    // B2 PR2: expand `@relpath` tokens in user messages into file
    // content (text) or placeholder (image/PDF/Office/binary). Runs
    // AFTER the user message is persisted (DB keeps the original
    // `@relpath` as source of truth) and BEFORE the turn loop, so C3
    // compaction + `provider.send` see the expanded content. A reloaded
    // session re-expands against the current file contents.
    //
    // B2 PR3 (2026-06-17): the function now also returns the
    // per-token injection manifest for the LAST user text message.
    // We (a) persist the manifest as `messages.metadata` on the user
    // row (update, not insert — the row was just written above with
    // `None` metadata), and (b) push a `ChatEvent::FileInjections`
    // event so the live-streaming user message's hint row appears
    // before the assistant starts.
    let (last_user_after_inject, injections, at_file_spans) =
        crate::agent::at_file::inject_at_tokens(&mut messages, &current_ctx, &session_id).await;
    // unified-context-budget WP1 (2026-08-19): @文件切片聚合(全部
    // user message 的注入正文 est 之和)。@图不在内 —— 其 token 走
    // attachments 的 tokens_est / images_token 列,不重复计
    // (TurnTraceRow.at_files_token 注释同款口径)。
    let at_files_token = at_file_spans.iter().map(|s| s.tokens).sum::<u32>();
    // B1 (2026-08-16): attach the message-level attachment refs
    // (current turn's paste uploads + history refs riding the
    // frontend payload / reload) as `ImageRef` blocks on the
    // in-memory request copy. Runs after the @-token pass above —
    // @-injected images already appended their own ImageRef blocks
    // inside `inject_at_tokens`; this pass covers the rest. The DB
    // row was already persisted text-only (source of truth intact).
    let _attached_images = crate::attachments::attach_images(&mut messages);
    // B1: the metadata manifest carries BOTH the @-token records and
    // the message's attachment refs (paste path). Merge the
    // @-injected images into the attachments list so the frontend
    // renders one thumbnail row from one key.
    let last_user_attachments: Vec<crate::llm::types::AttachmentRef> = {
        let mut refs = messages
            .iter()
            .rev()
            .find(|m| m.role == crate::llm::types::Role::User)
            .and_then(|m| m.attachments.clone())
            .unwrap_or_default();
        for r in &injections {
            if let crate::agent::at_file::InjectionAction::InjectedImage {
                file,
                media_type,
                tokens_est,
            } = &r.action
            {
                refs.push(crate::llm::types::AttachmentRef {
                    file: file.clone(),
                    media_type: media_type.clone(),
                    source: "at_file".to_string(),
                    tokens_est: *tokens_est,
                });
            }
        }
        refs
    };
    let has_attachments = !last_user_attachments.is_empty();
    if (!injections.is_empty() || has_attachments) && last_user_snapshot.is_some() {
        // Update the user row with the injection manifest as
        // metadata. The `update_message_metadata` IPC at the
        // SQL layer (added in this PR — see `db::sessions.rs`)
        // is the single write path; using a fresh SQL UPDATE
        // here keeps the contract that `messages.metadata` is
        // only ever set by the agent loop.
        //
        // B2 PR3 (bug fix 2026-06-17): wrap the manifest in
        // an object envelope `{"injections": [...]}` so the
        // frontend rehydrate path can read it back via
        // `m.metadata.injections` (see
        // `streamController.ts::rehydrateMessages`). The
        // previous form (`serde_json::to_value(&injections)`)
        // serialized the `Vec<InjectionRecord>` directly as a
        // top-level JSON array, which the rehydrate path's
        // `meta.injections` lookup treated as undefined and
        // silently dropped every entry. The envelope leaves
        // room for future metadata fields (latency, tags,
        // links) without another rehydrate-path migration.
        //
        // B1 (2026-08-16): `attachments` joins the envelope (same
        // single-write-path contract); emitted only when non-empty
        // so legacy rows / text turns stay byte-identical.
        let meta = if has_attachments {
            serde_json::json!({
                "injections": &injections,
                "attachments": &last_user_attachments,
            })
        } else {
            serde_json::json!({ "injections": &injections })
        };
        // B6 PR1b: skip the metadata UPDATE in worker mode (the
        // user row is the parent's, not the worker's).
        if !skip_persist {
            if let Err(e) =
                crate::db::update_message_metadata(&db, &session_id, last_user_seq, &meta).await
            {
                tracing::warn!(
                        request_id = %rid,
                        session_id = %session_id,
                        message_seq = last_user_seq,
                        error = %e,
                        "agent loop: failed to persist injection manifest as messages.metadata (non-fatal)"
                );
            }
        }
        // Live-push the manifest to the frontend. The
        // controller's `handleChatEvent("file_injections")`
        // case patches the user message's `injections` array
        // by `request_id` + `message_seq`.
        emit_chat_event_via_sink(
            &sink,
            &rid,
            &ChatEvent::FileInjections {
                request_id: rid.clone(),
                message_seq: last_user_seq,
                injections: injections.clone(),
            },
        );
    }
    // Silence the unused warning on `last_user_after_inject` —
    // we keep the in-place expansion in `messages` but the
    // returned clone is not needed (the chat loop iterates
    // `messages` directly downstream).
    let _ = last_user_after_inject;
    Ok(LoopInit {
        messages,
        seq,
        loaded_session,
        project,
        worktree_path,
        current_ctx,
        last_cwd,
        last_usage_terminal,
        failure_tracker,
        soft_blocked,
        session_mode,
        effective_is_worker,
        permission_ctx,
        mode_prefix,
        model_briefs,
        head_sha,
        system_prompt,
        memory_token,
        digest_on,
        summary_anchor,
        synthetic_prefix_len,
        compaction_on,
        system_token,
        at_files_token,
        at_file_spans,
    })
}
