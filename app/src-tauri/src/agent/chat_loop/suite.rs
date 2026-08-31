//! Agent-loop parameter suites（RULE-ARGS-001 一期，2026-08-27）。
//!
//! 把旧 `run_chat_loop` 的 38 个线性裸参按「生命周期 + 所有权来源」
//! 收敛为四个领域对象：
//!
//! | 对象 | 生命周期 | 来源 |
//! |---|---|---|
//! | [`ChatLoopDeps`] | 整个 loop ≥ 跨请求 | `AppState` 派生（`ChatLoopDeps::from_app_state`） |
//! | [`ChatLoopRequest`] | 单次请求 | 入口逐值构建 |
//! | [`CallerRole`] | 单次请求·常量 | 调用方身份决定 |
//! | [`TurnCarry`] | 单 turn 可变状态 | LoopInit 种子 → 循环体 mutation → `DriveTurnOutcome` 回传 |
//!
//! 命名比照库内先例：入口聚合用 `XxxDeps`（照
//! `agent::chat::QueueDriverDeps`）；随 request 穿线的域上下文用
//! `XxxCtx`/`XxxRole` 惯例；每 turn by-value 状态包沿
//! `LoopInit`/`DriveTurnOutcome` 管道惯例。本迁移是编译器驱动的改名，
//! 不是行为改写（design D3）：所有 `Arc/Mutex` 克隆次数、调用次序、锁
//! 获取点保持逐一对应。
//!
//! 尾部追加约定已随本次迁移终止：历史上每个 feature 往签名尾部追加一截
//! （"28→29 trailing expansion" 等），新增字段今后直接加在对应 struct
//! 上，由编译器强制全量迁移（RULE-A-006 保证生产可见）。

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use sqlx::SqlitePool;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::agent::loop_detection;
use crate::agent::permissions::PermissionContext;
use crate::agent::subagent::SubagentEventSink;
use crate::llm::{ChatMessage, ContentBlock, ToolDef};
use crate::memory::MemoryCache;
use crate::skill::loader::SkillCache;
use crate::state::{AppState, ChatEventSink, ProviderCatalog};
use crate::tools::read_guard::ReadGuard;
use crate::tools::ToolContext;

/// AppState 派生的长寿命套件（design §总体形态表）。
///
/// 字段与 `chat_inner` 既有手工克隆块一一对应；`run_queue_driver` /
/// `run_group_chat_loop` 也必须经同一条构造路径
/// （[`ChatLoopDeps::from_parts`] / [`ChatLoopDeps::from_app_state`]）
/// 组装，杜绝三处分头拼装漂移。
#[derive(Clone)]
pub(crate) struct ChatLoopDeps {
    /// SQLite 连接池（廉价 clone）。所有 DB 写点的 RULE-A-015 门语义
    /// 不变——写点自身在 loop 体内逐一保持原位。
    pub(crate) db: SqlitePool,
    /// 活跃请求取消令牌表（rid → token），AppState 持有。入口 pre-flight
    /// 注册、本 loop 的 RAII CancellationGuard 在每条退出路径清理。
    pub(crate) cancellations: Arc<Mutex<HashMap<String, CancellationToken>>>,
    /// session → rid 活跃请求映射（RULE-E-005 破坏性命令的取消钩子）。
    pub(crate) session_active_request: Arc<Mutex<HashMap<String, String>>>,
    /// 读守卫（tools/read_guard）。
    pub(crate) read_guard: ReadGuard,
    pub(crate) memory_cache: Arc<MemoryCache>,
    pub(crate) skill_cache: Arc<SkillCache>,
    /// Tier 3 权限询问的 oneshot 注册表（permissions::PermissionStore）。
    pub(crate) permission_asks: crate::agent::permissions::PermissionStore,
    /// 每请求新建的取消令牌（入口 pre-flight 注册进 cancellations，
    /// loop 内 CancellationGuard / 各 tokio::select! 消费）。不能从
    /// AppState 派生，故列为 ChatLoopDepsParts 的显式字段。
    pub(crate) token: CancellationToken,
    // --- L1a (2026-06-19): cross-request background-shell registry.
    // Threaded into the per-turn `ToolContext` so the 3 L1a tools
    // (`run_background_shell` / `shell_status` / `shell_kill`) can call
    // into it. The agent loop itself reads it once per turn (after C3
    // compaction, before `provider.send`) to drain pending completion
    // notifications and inject them as user-role messages.
    //
    // （原文档来自 run_chat_loop 旧第 17 参，RULE-ARGS-001 迁移入位。）
    pub(crate) background_shells: crate::background_shell::DefaultRegistry,
    // D (2026-08-14, `08-14-c7d-tools-stub-registration`): the session →
    // loaded-set stub registry（渐进式披露 D 的粘性 loaded-set）.
    // `drive_turn` 的第 4 环 stubify 读它(候选未 loaded → stub,
    // 已 loaded → 全量);`chat_loop/tools.rs` 的 `load_tool_schemas` /
    // 直呼自愈拦截写它。跨 request 存活(registry 挂 `AppState`,AC4 粘性)。
    //
    // 生产(chat.rs / group_chat_loop.rs)传 `state.stub_loaded`;worker
    // 嵌套调用(`run_subagent` → `dispatch/drive.rs`)传每次新建的空
    // registry — worker 永不 stub(gate `!effective_is_worker`),
    // registry 只是占位,不会被读写。
    pub(crate) stub_loaded: std::sync::Arc<crate::tools::stub::StubRegistry>,
    // 2026-06-30 (`ask_user_question` task): parallel QuestionStore for
    // the blocking reverse-question tool. Threads through so the loop's
    // `tool_name == "ask_user_question"` interception can call
    // `execute_blocking(input, session_id, tool_use_id, &question_store,
    // &sink, &token)`. Production sources it from
    // `AppState.question_store.clone()`; tests pass a fresh registry.
    // Worker nested calls carry the parent's — but since
    // `ask_user_question` is in `STRUCTURALLY_DISABLED`, a worker never
    // reaches the intercept, so the store is unused on the worker path.
    pub(crate) question_store: crate::agent::question_store::QuestionStore,
    // L3d (2026-06-25): the process-wide subagent cache. Used by the
    // loop's per-turn tool list construction to append the dynamic
    // `dispatch_subagent` ToolDef via
    // `definition_with_cache(&subagent_cache, project_path)`, and by
    // `run_subagent` to look up the dispatched subagent across builtin +
    // user + project layers. Threaded here rather than read off
    // `AppState` mid-loop for uniform treatment of every `Arc<...>`
    // handle (keeps test + production paths shape-identical). Read-through
    // + mtime-fenced so editing a `.md` is picked up next chat turn.
    pub(crate) subagent_cache: Arc<crate::agent::subagent::SubagentCache>,
}

/// [`ChatLoopDeps`] 的命名组装模板：所有生产构造点
/// （chat_inner 经典路重建、`run_queue_driver` 转正、群聊两调用点、
/// worker 嵌套）都经 `From<ChatLoopDepsParts>` 字段逐一具名拼装 ——
/// 不设多参 fn 组装器（避免 `too_many_arguments` 豁免回潮），新增字段
/// 时由编译器强制全部生产点对齐，等效防漂移。
pub(crate) struct ChatLoopDepsParts {
    pub(crate) db: SqlitePool,
    pub(crate) cancellations: Arc<Mutex<HashMap<String, CancellationToken>>>,
    pub(crate) session_active_request: Arc<Mutex<HashMap<String, String>>>,
    pub(crate) read_guard: ReadGuard,
    pub(crate) memory_cache: Arc<MemoryCache>,
    pub(crate) skill_cache: Arc<SkillCache>,
    pub(crate) permission_asks: crate::agent::permissions::PermissionStore,
    pub(crate) token: CancellationToken,
    pub(crate) background_shells: crate::background_shell::DefaultRegistry,
    pub(crate) stub_loaded: std::sync::Arc<crate::tools::stub::StubRegistry>,
    pub(crate) question_store: crate::agent::question_store::QuestionStore,
    pub(crate) subagent_cache: Arc<crate::agent::subagent::SubagentCache>,
}

impl From<ChatLoopDepsParts> for ChatLoopDeps {
    fn from(p: ChatLoopDepsParts) -> Self {
        Self {
            db: p.db,
            cancellations: p.cancellations,
            session_active_request: p.session_active_request,
            read_guard: p.read_guard,
            memory_cache: p.memory_cache,
            skill_cache: p.skill_cache,
            permission_asks: p.permission_asks,
            token: p.token,
            background_shells: p.background_shells,
            stub_loaded: p.stub_loaded,
            question_store: p.question_store,
            subagent_cache: p.subagent_cache,
        }
    }
}

impl ChatLoopDeps {
    /// 统一组装路径之一：从 [`AppState`] 解包（`token` 除外——取消令牌
    /// 是每请求新建并由入口注册进 `cancellations` 的，必须由调用方传入；
    /// 见 chat.rs pre-flight 序列）。其余生产点（队列驱动器转正 / 群聊 /
    /// worker 嵌套 / spawn 内经典路重建）一律走
    /// `ChatLoopDeps::from(ChatLoopDepsParts { .. })` 具名拼装。
    pub(crate) fn from_app_state(state: &AppState, token: CancellationToken) -> Self {
        Self::from(ChatLoopDepsParts {
            db: state.db.clone(),
            cancellations: state.cancellations.clone(),
            session_active_request: state.session_active_request.clone(),
            read_guard: state.read_guard.clone(),
            memory_cache: state.memory_cache.clone(),
            skill_cache: state.skill_cache.clone(),
            permission_asks: state.permission_asks.clone(),
            token,
            background_shells: state.background_shells.clone(),
            stub_loaded: state.stub_loaded.clone(),
            question_store: state.question_store.clone(),
            subagent_cache: state.subagent_cache.clone(),
        })
    }
}

/// 单次请求值（design §总体形态表）。入口（`chat_inner` / 队列驱动器 /
/// 群聊编排 / worker 嵌套）逐值构建。
pub(crate) struct ChatLoopRequest {
    // D3 PR3 (2026-06-17): resend context. When `Some(seq)`, the
    // user-message persist site writes a `resend_message` audit row
    // pointing at the original user message's seq. `None` for normal
    // first-time sends. Best-effort.
    pub(crate) resend_seq: Option<i64>,
    // B6 Subagent (2026-06-19, review #4): per-invocation turn budget.
    // `None` falls back to the global `MAX_TURNS` (50) — preserves
    // RULE-A-006 single-source-of-truth semantics for the production
    // path. The worker agent path passes `Some(SUBAGENT_MAX_TURNS)`; the
    // group-chat moderator passes `Some(1)`, participants `Some(20)`.
    // C3 compaction and the max_turns terminal event both honor this
    // limit identically to the const case.
    pub(crate) max_turns: Option<usize>,
    pub(crate) tool_defs: Vec<ToolDef>,
    pub(crate) provider: Arc<dyn crate::llm::Provider>,
    pub(crate) context_window: u32,
    // 08-20-turn-usage-event-quota-view WP2: 解析模型的 provider 行 id,
    // 落 `turn_trace.provider_id`(5h 窗口配额聚合分组键)。`None` =
    // catalog miss 等极端路径(落 NULL,聚合归 unknown 桶)。主路来自
    // `ResolvedChatProviderWrapper.provider_id`,worker 路来自
    // `resolve_worker_provider` 扩展返回值。
    pub(crate) provider_id: Option<String>,
    pub(crate) rid: String,
    pub(crate) session_id: String,
    /// Entry invariant（signature-run-chat-loop.md Warning）：尾条 user
    /// message 必须「新鲜未落盘」。D-D 守卫在 init 段负责跳过重持久化。
    /// **禁止**恢复 `messages.len()` 判据（08-04 群聊 400 死循环根因）。
    pub(crate) messages: Vec<ChatMessage>,
    pub(crate) sink: Arc<dyn ChatEventSink>,
    // W1 (Workflow integration, Phase 0 Step 0.5 — 2026-07-08):
    // per-session workflow context. `None` for non-workflow sessions.
    // When `Some(ctx)`, the state breadcrumb + current-task metadata
    // are appended to the request TAIL on every turn (D1,
    // 08-31-cache-head-volatility — was `messages[0]`). Loop 内可变
    // （current_task 每 turn 从盘上刷新），经 TurnCarry/Outcome 穿线。
    pub(crate) workflow_ctx: Option<crate::agent::workflow::WorkflowCtx>,
    // Group chat (07-29-group-chat): shared turn state for the
    // `nominate_speaker` / `end_discussion` interception. `None` for
    // classic-chat + worker paths.
    pub(crate) group_chat_state: Option<crate::tools::nominate_speaker::SharedTurnState>,
    // Group chat (07-29-group-chat, Phase 4 TODO-A): per-turn speaker.
    // `None` for normal chat / subagent paths; carried into the assistant
    // persist site so messages store the originating speaker. Read-only —
    // never affects tool routing or wire shape.
    pub(crate) current_speaker: Option<String>,
    // F1 队列驱动器本轮的 drained 全量(RULE-QUEUE-001 根治,2026-08-29,
    // `08-29-rule-queue-001-multi-drain-persist`)。驱动器是唯一非空调用点;
    // 其余(classic / 群聊 / worker)恒空 vec。
    //
    // 消费两端(均在 init.rs):
    // - 非尾条:persist 循环逐条补写 —— 旧世界非尾条无 DB 行,reload 后
    //   从时间线消失(LLM 单次看到、历史不落库);带 origin / 附件的行
    //   另写 metadata 信封(`scheduled` / `attachments`)。
    // - 尾条:origin 派生自 `drained.last()`,供 persist 门控放宽 + 信封
    //   `scheduled` 键(F2 `08-28-f2-scheduled-tasks` design §4.1 契约
    //   延续;原独立 `origin: Option<TaskOrigin>` 字段已并入此处)。
    //
    // 不变量:`drained` 非空 ⇒ 尾条 == `messages` 尾条 user(驱动器
    // append 顺序保证),且 `group_chat_state` 恒 None(队列只在 classic
    // 单聊装配)。载体的 payload 必须在队列项上:忙时 fire 的条目由另一个
    // 请求的驱动器在 round>0 消费,请求级上下文在 round>0 一律丢弃。
    pub(crate) drained: Vec<crate::agent::message_queue::QueuedMessage>,
}

/// 调用方角色旗标（design 表第三行；暂名 CallerRole）。由调用方身份决定
/// 的单请求常量：worker 子代理旗标群 + skip 三兄弟 + 各 override。
pub(crate) struct CallerRole {
    // B6 Subagent PR2b (2026-06-20, RULE-A-014): when `Some(true)`, the
    // `PermissionContext` built inside this loop carries `is_worker:
    // true`, which gates `ask_path` into the worker's interactive
    // round-trip branch. `None` falls back to the session-row mode's
    // natural default (production = `false`). The worker path passes
    // `Some(true)`; production + tests pass `Some(false)` to make the
    // production-style default explicit at the call site.
    //
    // **显式传播链（RULE-A-014 保真）：role.is_worker →
    // prepare_loop_state 的 effective_is_worker → PermissionContext。
    // 回归测试 agent_loop_dispatch_subagent_general_purpose_plan_mode_
    // write_denied 断言该链路 <15s 退出。**
    pub(crate) is_worker: Option<bool>,
    // B6 Subagent (2026-06-19, PR1b review #2): when `true`, the
    // per-invocation `CancellationGuard`'s Drop skips the
    // `session_active_request.remove(&session_id)` step. Workers reuse
    // the parent's `session_id` but their rid must NOT own the session's
    // "active request" slot (RULE-E-005).
    pub(crate) skip_session_active: bool,
    // B6 Subagent (PR1b): when `true`, the loop skips ALL DB writes
    // (`persist_turn` / `update_message_metadata` / `touch_session` /
    // `update_last_turn_usage` / `record_*_audit`; 16 write sites — see
    // RULE-A-015 in debt-linkage.md). The worker path uses this so its
    // intermediate turns stay in-memory only.
    //
    // **PR2a correction (2026-06-20, RULE-A-015)**: two sites were fixed
    // as over-broad: (a) `add_token_usage` was pulled OUT then REVERSED
    // back IN on 2026-06-26 (snapshot model — worker token lives in
    // `subagent_runs.token_usage_json` only); (b) the terminal `Done`
    // emit stays OUTSIDE the gate permanently (the `SubagentBufferSink`
    // is both consumer and transcript source). These gate-shape
    // contracts travel with every write site and are unchanged by this
    // migration.
    pub(crate) skip_persist: bool,
    // F1 消息队列(2026-08-25):`true` 时入口 CancellationGuard 的 Drop 连
    // `cancellations.remove(&rid)` 一起跳过(与 `skip_session_active`
    // 独立)。队列驱动器对每一轮内层调用传双 true——rid 与 session slot
    // 必须跨轮存活,否则轮间隙并发入队会误判"闲";驱动器自身在最终退出
    // 时清两个 map。其余全部调用点传 `false`。
    pub(crate) skip_cancellations: bool,
    // P2.4 C5 (2026-07-22): the worker dispatch context, replacing the
    // old `app_handle: Option<AppHandle>` param. Transport-agnostic:
    //   - `worker_catalog`: `Some(catalog.clone())` in production (Tauri
    //     + daemon); `None` in tests.
    //   - `worker_event_sink`: `AppHandleSubagentSink` (Tauri IPC),
    //     `HttpSseSubagentSink` (daemon SSE), `ThreadLocalSubagentSink`
    //     (tests).
    // The agent loop body itself does NOT use either — only
    // `run_subagent` does, when constructing the worker sink.
    pub(crate) worker_catalog: Option<Arc<RwLock<ProviderCatalog>>>,
    pub(crate) worker_event_sink: Arc<dyn SubagentEventSink>,
    // 2026-06-21 fix (B6 review defect A): the worker system prompt
    // override. When `Some(p)`, the loop uses `p` directly (skipping
    // `assemble_system_prompt`); when `None`, builds from project +
    // session row. `run_subagent` passes
    // `Some(assemble_subagent_prompt(def, &task))`; production passes
    // `None`.
    pub(crate) system_prompt_override: Option<String>,
    // 2026-06-22 (RULE-FrontSubagent-003 fix): the worker's
    // `subagent_runs.id` (DB row UUID). Threads into `PermissionContext`
    // so `ask_path` can build the worker-owned permission session id
    // (`"worker:<worker_run_id>"`) and populate
    // `PermissionAskPayload.worker_run_id` for frontend routing. The
    // companion `is_worker: Some(true)` gates the branch — this field is
    // the routing key.
    pub(crate) worker_run_id: Option<String>,
    // 2026-06-26 (task `06-26-subagent-per-run-grant`): per-run in-memory
    // grant cache for worker subagents. `Some(Arc<...>)` on the worker
    // path (fresh Arc per worker); `None` on the parent path.
    // RULE-A-016 isolation: worker grants must not cross the privilege
    // boundary into the parent session's grant table.
    pub(crate) run_grants: Option<std::sync::Arc<crate::agent::permissions::RunGrantCache>>,
    // L3b (2026-06-27): worker worktree isolation override. When
    // `Some(path)`, the loop uses it as the worker's worktree root
    // INSTEAD of the session row's `worktree_path` (root cause of worker
    // reuse of parent checkout); also seeds the worker's cwd.
    // Mirrors the `system_prompt_override` pattern.
    pub(crate) worktree_override: Option<PathBuf>,
    // project_main_override (2026-07-29): the worker's ORIGINAL project
    // main repo path when `worktree_override` is `Some`. Threads into
    // `PermissionContext.project_main_path` so the inside-check anchors
    // on the project root, NOT the worker's own checkout subtree.
    pub(crate) project_main_override: Option<PathBuf>,
    // L3b (2026-06-27): the app's data directory, threaded so the
    // dispatch_subagent interceptor can compute the worker worktree path
    // when isolation is active. Pass-through — the loop body itself does
    // NOT read it; only `run_subagent` does.
    pub(crate) app_data_dir: PathBuf,
    // explicit-agent-dispatch (2026-06-30): when `Some(fd)`, the loop's
    // turn-1 prefix short-circuits the LLM — synthesizes a
    // `dispatch_subagent` tool_use from `fd` and calls `run_subagent`
    // directly (NO `provider.stream`). `None` = normal LLM-driven loop.
    pub(crate) forced_dispatch: Option<crate::agent::subagent::ForcedDispatch>,
}

/// 跨 turn 可变状态包（design D2）：drive.rs 十人 `let mut` 名单
/// （messages/seq/head_sha/system_prompt/permission_ctx/loop_window/
/// loop_hit_count/last_usage_terminal/workflow_ctx/summary_anchor）。
/// by-value 进 drive_turn、经 `DriveTurnOutcome` by-value 出——完全继承
/// 现行 LoopInit → outcome 管道。
pub(crate) struct TurnCarry {
    pub(crate) messages: Vec<ChatMessage>,
    pub(crate) seq: i64,
    pub(crate) head_sha: String,
    pub(crate) system_prompt: String,
    pub(crate) permission_ctx: PermissionContext,
    pub(crate) loop_window: VecDeque<loop_detection::ToolCall>,
    pub(crate) loop_hit_count: u32,
    pub(crate) last_usage_terminal: Option<crate::llm::types::TokenUsage>,
    pub(crate) workflow_ctx: Option<crate::agent::workflow::WorkflowCtx>,
    pub(crate) summary_anchor: Option<crate::agent::compaction::SummaryAnchor>,
}

/// 每轮常量帧：prepare_loop_state 派生、整个请求生命周期内不变的量
/// （引用借用；避免旧调用点每轮 `project.clone()` / `model_briefs.clone()`
/// 等按值搬运）+ 两个循环控制标量（由 run_chat_loop 的轮间逻辑推进）。
pub(crate) struct TurnFrame<'a> {
    pub(crate) loaded_session: &'a crate::db::LoadedSession,
    pub(crate) project: &'a crate::projects::ProjectRow,
    pub(crate) worktree_path: &'a PathBuf,
    pub(crate) mode_prefix: &'static str,
    pub(crate) model_briefs: &'a [crate::agent::subagent::ModelBrief],
    pub(crate) session_mode: crate::db::Mode,
    /// 由 role.is_worker 解出（prepare_loop_state 内 unwrap_or(false)），
    /// RULE-A-014 传播链的中间态。
    pub(crate) effective_is_worker: bool,
    /// 每 request 读一次的 config 开关（run_chat_loop 体内读取，fail-open）。
    pub(crate) stub_on: bool,
    pub(crate) budget_on: bool,
    pub(crate) memory_token: Option<u32>,
    pub(crate) digest_on: bool,
    pub(crate) synthetic_prefix_len: usize,
    pub(crate) compaction_on: bool,
    pub(crate) system_token: u32,
    pub(crate) at_files_token: u32,
    pub(crate) at_file_spans: &'a [crate::agent::at_file::AtFileSpan],
    pub(crate) current_user_msg_idx: usize,
    pub(crate) memory_catalog_blocks: &'a Option<Vec<ContentBlock>>,
    /// 轮计数（loop 每轮 +1）。旧函数体局部变量 `turn`。
    pub(crate) turn: usize,
    /// softcap「压缩后续跑」的一次性 force 标志；drive_turn 消费后由
    /// 调用方复位 false。
    pub(crate) force_compaction: bool,
}

/// 循环域 cwd 状态对：shell 类工具可改 cwd，经 `DispatchOutcome` 回写。
/// 与 TurnCarry 平行的第二个跨 turn 可变集合（inventory §4"十人名单 +
/// dispatch 期间的 cwd"）。
pub(crate) struct TurnHot {
    pub(crate) current_ctx: ToolContext,
    pub(crate) last_cwd: Option<PathBuf>,
}

/// 轮数软卡询问的标量包（原 ask_turn_limit_softcap 尾部散参）。
pub(crate) struct TurnBudgetAsk<'a> {
    pub(crate) turn: usize,
    pub(crate) turns_budget: usize,
    pub(crate) compaction_on: bool,
    pub(crate) seq: i64,
    /// 终态 usage 快照（R3；Copy 值传递与旧位参一致）。
    pub(crate) last_usage_terminal: Option<crate::llm::types::TokenUsage>,
    pub(crate) last_cwd: Option<&'a std::path::Path>,
}

/// 硬轮数终态（max_turns 分支 / softcap 各终止臂共用）的标量包。
pub(crate) struct HardTurnsTerminal<'a> {
    pub(crate) budget: usize,
    pub(crate) last_usage_terminal: Option<crate::llm::types::TokenUsage>,
    pub(crate) last_cwd: Option<&'a std::path::Path>,
}

/// dispatch_tool_calls 的非套件残余（turn/fn-scope 状态 + LoopInit 派生
/// 常量）。套件可达成员一律走三套件引用参数，不在此重复。
/// （例外：[`DispatchCtx::workflow_ctx`] 以引用穿透 —— 见字段 doc。）
pub(crate) struct DispatchCtx<'a> {
    pub(crate) tool_calls: Vec<(String, String, serde_json::Value)>,
    pub(crate) permission_ctx: PermissionContext,
    /// 函数域工具上下文（shell 类工具可能改 cwd），出参回传。
    pub(crate) current_ctx: ToolContext,
    pub(crate) last_cwd: Option<PathBuf>,
    pub(crate) cancelled: bool,
    pub(crate) session_mode: crate::db::Mode,
    pub(crate) failure_tracker: Arc<Mutex<crate::agent::auto_reflect::FailureTracker>>,
    pub(crate) soft_blocked: Arc<Mutex<std::collections::HashSet<String>>>,
    pub(crate) seq: i64,
    pub(crate) stub_on: bool,
    /// W1 Step 2.4：dispatch 期间的工作流角色门
    /// （`check_workflow_role_gate` 读 `current_task.status`）输入。
    /// **必须是 run_chat_loop 函数域绑定的活引用**（drive_turn 轮顶把
    /// on-disk 最新 task 刷新进这份拷贝、经 DriveTurnOutcome 回写），
    /// 不能读 `request.workflow_ctx` —— 那是入口快照，loop 内永不更新。
    pub(crate) workflow_ctx: &'a Option<crate::agent::workflow::WorkflowCtx>,
}

/// finalize_turn 的非套件残余（≤6 参收编）。
pub(crate) struct FinalizeFrame<'a> {
    pub(crate) result_blocks: Vec<ContentBlock>,
    pub(crate) loop_hint: &'a Option<String>,
    pub(crate) cancelled: bool,
    pub(crate) seq: i64,
    pub(crate) messages: &'a mut Vec<ChatMessage>,
    pub(crate) last_cwd: &'a Option<PathBuf>,
}

/// attempt_summary_compaction 的一次压缩作业参数（C3 PR2 摘要旁路）。
pub(crate) struct SummaryCompactionJob<'a> {
    /// 待压消息切片（borrow 自 carry.messages）。
    pub(crate) messages: &'a [ChatMessage],
    /// DB 权威行（对齐计算用；frame.loaded_session.messages）。
    pub(crate) db_rows: &'a [crate::db::MessageRow],
    /// 上次压缩的水位锚点（增量合并 prior）。
    pub(crate) prior: Option<crate::agent::compaction::SummaryAnchor>,
    /// 待压区结束下标（compute_preservation_region 输出）。
    pub(crate) cut: usize,
    pub(crate) seq: i64,
    pub(crate) tokens_before: u32,
    /// metadata `trigger` 标注："auto" / "softcap"。
    pub(crate) trigger_label: &'static str,
}
