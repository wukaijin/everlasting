# 参数管道库存报告（RULE-ARGS-001 前期研究）

> 调研日期：2026-08-27，基线 commit `6ce9ef4`（工作区干净）。
> 结论均基于当日工作区真实代码；行号为当日实况，重构后自然漂移。

## 1. 三大签名全文

### 1.1 `run_chat_loop` — `app/src-tauri/src/agent/chat_loop.rs:319`

**实际 38 个参数**（journal 曾记 25）。`:318` 有 `#[allow(clippy::too_many_arguments)]`，返回 `()`。逐字摘录（省略号为参数间注释块，原文见行号）：

```rust
#[allow(clippy::too_many_arguments)]
pub async fn run_chat_loop(
    tool_defs: Vec<ToolDef>,                                            // :320
    provider: Arc<dyn Provider>,                                        // :321
    context_window: u32,                                                // :322
    provider_id: Option<String>,            // :328 (WP2 quota 归因)
    rid: String,                                                        // :329
    session_id: String,                                                 // :330
    messages: Vec<ChatMessage>,                                         // :331
    sink: Arc<dyn ChatEventSink>,                                       // :332
    db: SqlitePool,                                                     // :333
    cancellations: Arc<Mutex<HashMap<String, CancellationToken>>>,      // :334
    session_active_request: Arc<Mutex<HashMap<String, String>>>,        // :335
    read_guard: ReadGuard,                                              // :336
    memory_cache: Arc<MemoryCache>,                                     // :337
    skill_cache: Arc<SkillCache>,                                       // :338
    permission_asks: crate::agent::permissions::PermissionStore,        // :339
    token: CancellationToken,                                           // :340
    resend_seq: Option<i64>,                // :348 (D3 PR3 resend, best-effort)
    background_shells: crate::background_shell::DefaultRegistry,        // :356 (L1a)
    max_turns: Option<usize>,               // :365 (B6 per-invocation budget)
    skip_session_active: bool,              // :375 (B6/RULE-E-005 Drop skip)
    skip_persist: bool,                     // :387 (B6 全 DB 写抑制)
    is_worker: Option<bool>,                // :400 (RULE-A-014 第 21 参)
    worker_catalog: Option<Arc<RwLock<ProviderCatalog>>>,  // :415 (P2.4 C5 替代 app_handle)
    worker_event_sink: Arc<dyn SubagentEventSink>,          // :416
    system_prompt_override: Option<String>, // :437 (B6)
    worker_run_id: Option<String>,          // :457 (RULE-FrontSubagent-003)
    subagent_cache: Arc<crate::agent::subagent::SubagentCache>, // :473 (L3d)
    run_grants: Option<Arc<RunGrantCache>>, // :488 (06-26 per-run grant, RULE-A-016)
    worktree_override: Option<PathBuf>,     // :509 (L3b)
    project_main_override: Option<PathBuf>, // :517
    app_data_dir: PathBuf,                  // :530 (pass-through,循环体自身不读)
    forced_dispatch: Option<ForcedDispatch>, // :539
    question_store: QuestionStore,          // :557 ("28→29 trailing expansion")
    mut workflow_ctx: Option<WorkflowCtx>,  // :571 ("29→30 arg. Still appended at the tail")
    group_chat_state: Option<SharedTurnState>, // :578 (群聊)
    current_speaker: Option<String>,        // :591 (Phase 4 TODO-A)
    stub_loaded: Arc<StubRegistry>,         // :603
    skip_cancellations: bool,               // :613 (F1 双抑制 guard 尾部追加)
)
```

### 1.2 `drive_turn` — `app/src-tauri/src/agent/chat_loop/drive.rs:166`

**49 参**，返回 `Result<DriveTurnOutcome, ()>`。（debt 记录的 `drive.rs:82` 为旧行号。）摘录：

```rust
#[allow(clippy::too_many_arguments)]
pub(crate) async fn drive_turn(
    turn: usize, messages: Vec<ChatMessage>, seq: i64, head_sha: String,
    system_prompt: String, permission_ctx: PermissionContext,
    loop_window: VecDeque<loop_detection::ToolCall>, loop_hit_count: u32,
    last_usage_terminal: Option<TokenUsage>, workflow_ctx: Option<WorkflowCtx>,
    loaded_session: &LoadedSession, project: ProjectRow, worktree_path: PathBuf,
    last_cwd: &Option<PathBuf>, current_ctx: &ToolContext,
    mode_prefix: &'static str, model_briefs: Vec<ModelBrief>,
    session_mode: Mode, effective_is_worker: bool,
    system_prompt_override: &Option<String>, tool_defs: Vec<ToolDef>,
    subagent_cache: Arc<SubagentCache>, provider: Arc<dyn Provider>,
    context_window: u32, provider_id: Option<String>, rid: String,
    session_id: String, sink: Arc<dyn ChatEventSink>, db: SqlitePool,
    token: CancellationToken, background_shells: &DefaultRegistry,
    skip_persist: bool, worker_run_id: Option<&str>,
    current_speaker: &Option<String>, question_store: &QuestionStore,
    stub_on: bool, stub_loaded: &StubRegistry,
    memory_token: Option<u32>, digest_on: bool,
    summary_anchor: Option<SummaryAnchor>, synthetic_prefix_len: usize,
    compaction_on: bool, system_token: u32, at_files_token: u32,
    at_file_spans: Vec<AtFileSpan>, current_user_msg_idx: usize,
    memory_catalog_blocks: Option<Vec<ContentBlock>>, budget_on: bool,
    force_compaction: bool,
) -> Result<DriveTurnOutcome, ()>
```

注：`drive.rs:201-206` 注释自我认知"已知 ~45 参债务,不为此小额任务重构"。
**跨 turn 可变状态的直接证据**：函数体开头（`drive.rs:253-263`）把前 10 个参数逐个 `let mut` 重绑定 —— `messages, seq, head_sha, system_prompt, permission_ctx, loop_window, loop_hit_count, last_usage_terminal, workflow_ctx, summary_anchor`。

### 1.3 `finalize_turn` — `app/src-tauri/src/agent/chat_loop/tools.rs:1779`

**11 参**，返回 `Result<(), ()>`：

```rust
#[allow(clippy::too_many_arguments)]
pub(crate) async fn finalize_turn(
    mut result_blocks: Vec<ContentBlock>, loop_hint: &Option<String>,
    cancelled: bool, skip_persist: bool, db: &SqlitePool,
    sink: &Arc<dyn ChatEventSink>, rid: &str, session_id: &str, seq: i64,
    messages: &mut Vec<ChatMessage>, last_cwd: &Option<PathBuf>,
) -> Result<(), ()>
```

### 1.4 同区域其它 ≥10 参函数

| 函数 | 位置 | 参数数 | 备注 |
|---|---|---|---|
| `dispatch_tool_calls` | `chat_loop/tools.rs:47` | **33** | 返回 `DispatchOutcome`(:39)；与 run_chat_loop 共享约 20 个同名参数 |
| `prepare_loop_state` | `chat_loop/init.rs:121` | **19** | 返回 `LoopInit` |
| `run_group_chat_loop` | `group_chat_loop.rs:210` | **23** | 二期候选 |
| `ask_turn_limit_softcap` | `chat_loop.rs:1351` | 13 | 一期收编 |
| `attempt_summary_compaction` | `chat_loop/drive.rs:2500` | 13 | 一期收编 |
| `emit_max_turns_terminal` | `chat_loop.rs:1282` | 8 | clippy 阈值边缘，豁免注释已指回 DEBT |

## 2. 调用图

### 2.1 `run_chat_loop` 生产调用点（仅 4 处）

| 调用点 | 链路 |
|---|---|
| `agent/chat.rs:637` | `chat_inner` 直发分支 |
| `agent/chat.rs:1072` | `run_queue_driver(deps)` 每续轮 |
| `group_chat_loop.rs:302`/:506 | moderator / participant（上游仅 chat.rs:575） |
| `subagent/dispatch/drive.rs:114` | worker 嵌套递归（`run_subagent → drive_worker`） |

最外层入口（双传输已收敛单一函数）：
- Tauri 命令 `agent::chat::chat`（chat.rs:67，注册 lib.rs:220）→ :104 `chat_inner`
- daemon HTTP `daemon/routes/agent.rs:59` → 同一 `chat_inner`
- **session restore 无独立入口**

### 2.2 内部传递链

`run_chat_loop` = `:653 prepare_loop_state` → `:1027 drive_turn` → `:1133 dispatch_tool_calls`（其中 `dispatch_subagent` 拦截回到 2.1 递归）→ `:1180 finalize_turn`。`drive_turn` / `finalize_turn` 生产代码各只有这 1 个调用点。

### 2.3 测试侧波及面

`run_chat_loop(` 测试位参调用点共 **70 处**（全部按序传参）：tests_agent_loop/ 42、tests_subagent/ 20+、tests_sse / tests_group_chat(2) / tests_c2plus(2) / tests_ask_user_question / tests_request_mode_change 散布。间接波及：`tests_message_queue.rs:91/119` 直接构造 `QueueDriverDeps`；`compaction_summary / softcap / turn_checkpoint` 各有本地 `run_loop(...)` 包装器（持豁免标注）。

## 3. 参数聚类（38 参去向草案）

| 簇 | 参数（行号） | 性质 |
|---|---|---|
| **Provider/LLM 会话** | `provider`(:321) `context_window`(:322) `provider_id`(:328) `tool_defs`(:320) | provider 长命 Arc；余为每请求值 |
| **取消信号** | `token`(:340) `cancellations`(:334) `session_active_request`(:335) `skip_session_active`(:375) `skip_cancellations`(:613) | 前三是 AppState Arc 克隆，跨请求可变，同时透传 dispatch_tool_calls(:63-64)；两个 skip 是调用方角色常量 |
| **cache** | `memory_cache`(:337) `skill_cache`(:338) | AppState 长命，loop 内只读 |
| **Subagent 套件（14 参最大簇）** | `max_turns`(:365) `is_worker`(:400) `worker_catalog`(:415) `worker_event_sink`(:416) `system_prompt_override`(:437) `worker_run_id`(:457) `subagent_cache`(:473) `run_grants`(:488) `worktree_override`(:509) `project_main_override`(:517) `app_data_dir`(:530) `forced_dispatch`(:539) `skip_persist`(:387) (`current_speaker`(:591) 亦可独属群聊) | 除 subagent_cache 进程单例外几乎全 `Option` 区分 parent/worker 角色 |
| **持久化** | `db`(:333) `skip_persist`(:387) | pool 廉价克隆；skip 受 RULE-A-015（16 写点位） |
| **身份标识** | `rid`(:329) `session_id`(:330) | 每请求新建 |
| **event sink** | `sink`(:332) | 每请求构建（Tauri IPC / SSE / MockEmitter） |
| **权限** | `permission_asks`(:339) `read_guard`(:336) | AppState 来源 |
| **散件** | `messages`(:331) `resend_seq`(:348) `background_shells`(:356) `question_store`(:557) `workflow_ctx`(:571，唯一显式 mut) `group_chat_state`(:578) `current_speaker`(:591) `stub_loaded`(:603) | 后四 store 类均为进程/AppState 级 |

**可变性结论**：签名级跨 turn 可变只有 `mut workflow_ctx` + 值消耗的 `messages`；真正逐 turn 突变全部走 `LoopInit` 出参 → drive_turn 值参 → `DriveTurnOutcome` 回传（外加 dispatch 期间 `current_ctx.cwd`/`last_cwd` 经 `DispatchOutcome` 带回，tools.rs:39-44、1731-1734）。其余参数在整个循环生命周期只读 —— 理想的 env/suite 分组。

## 4. 所有权与生命周期

**单一 owner = AppState**：生产链路在 `chat.rs:214-223` 起连续解包克隆：

```rust
let db = state.db.clone();
let catalog = state.catalog.clone();
let cancellations = state.cancellations.clone();
let session_active_request = state.session_active_request.clone();
// ... memory_cache / skill_cache / permission_asks / read_guard ...
```

队列驱动器已把同一形状搬进 `QueueDriverDeps`（chat.rs:979-1010）；测试 harness 再次镜像（basic.rs:44-51）。**三个代码位从同一 owner 取同一子集 → struct 天然可聚合。**

**派生而非传入**：`worktree_path/project/loaded_session/permission_ctx/system_prompt/seq` 等由 `prepare_loop_state` 从 DB 行派生（init.rs:145-229），打包在 `LoopInit`（init.rs:31-112）。身份/标识与派生上下文应分开建模。

**每 turn 必变子集**（drive.rs:253-262 的 let mut 名单）：`messages, seq, head_sha, system_prompt, permission_ctx, loop_window, loop_hit_count, last_usage_terminal, workflow_ctx, summary_anchor` + dispatch 期间的 cwd。适合独立小包，不进 Deps。

## 5. too_many_arguments 分布（实测 46 处，DEBT 记 43）

| 计数 | 文件 |
|---|---|
| 4 | commands/providers.rs（CRUD 四连，纯样板） |
| 4 | agent/chat_loop.rs |
| 3 | agent/trace.rs（record_* 三兄弟） |
| 2 | db/models.rs、chat_loop/tools.rs、chat_loop/drive.rs、agent/chat.rs、agent/budget.rs |
| 1 × 25 文件 | 见仓库 grep（tools/{request_task_state_transition,request_mode_change,ask_user_question}、llm/retry、db/trace、db/subagent_runs、db/sessions/session_crud、db/memories/lifecycle、daemon/tunnel/{dispatcher,client}、tests_agent_loop/{turn_checkpoint,softcap,compaction_summary}、subagent/prep、subagent/dispatch/{register,prepare,finalize,drive}、subagent/dispatch、permissions/check/permission、permissions/audit、permissions/ask、group_chat_loop、chat_loop/init、auto_reflect） |

一期范围（chat_loop.rs/tools.rs/drive.rs/init.rs）合计 **9 处**。CI 门只查 `--lib`，`#[cfg(test)]` 不入 gate（quality-guidelines.md 明示）。

## 6. 库内 parameter object 先例

1. **`QueueDriverDeps`**（chat.rs:979，消费 `run_queue_driver(deps)` @:1034，测试直构 @tests_message_queue.rs:91）—— 最佳范本；`XxxDeps` 是现成的入口聚合命名。
2. **`LoopInit`**（init.rs:31，出参包，chat_loop.rs:684 解构消费）—— 单次产物包先例。
3. **`WorkflowCtx`**（workflow/inject.rs:86）/ **`GroupChatCtx`**（group_chat.rs:63，"Built once at IPC entry … consumed by run_group_chat_loop"）—— `XxxCtx` 后缀 = IPC 入口一次解析随 request 穿线的域上下文。
4. 反例参照：`ToolContext`（tools/mod.rs:369）、`PermissionContext`（permissions/types.rs:144）已是 struct 形态的每 turn 工具面上下文，可作新成员二次聚合。

**命名建议**：`ChatLoopDeps` 承载长寿命 suite（比照 QueueDriverDeps）；每 turn 状态包沿 LoopInit/DriveTurnOutcome by-value 惯例；避免 `RunChatLoopArgs` 式命名。

## 7. 近期相关提交

- `e630b72`（08-26）ci(rust)： clippy gate `-D warnings` + 2 处显式豁免（chat_inner / emit_max_turns_terminal）并显式登记 RULE-ARGS-001 —— 冻结现状待 epic，与本重构同向。
- `92a480b`（08-25）feat(queue) F1-A：尾部追加 `skip_cancellations`；上游建 `QueueDriverDeps` —— 开始承认 bundling。
- 8 月下旬 budget/trace 系列（ca675a1/325ad19/5703783/38c8e63/ae2f3b4）—— drive_turn ~40→49 的来源，"每 feature 追加一段"即债务成因本身。
- `d71d296`/`ae7381e`（08-26 p2-debt-cleanup）只清 db/loader，未触 chat_loop —— 设计无冲突。

## 8. spec 约定（.trellis/spec/backend/agent-loop-architecture*）

注意：目录**没有 index.md**；总入口是平级的 `.trellis/spec/backend/agent-loop-architecture.md`（:71 Part Index 列 18 篇）。

- **架构契约**：run_chat_loop 是 production/tests 单一事实源；`messages[0]` 注入顺序硬约束（指令块带 `cache_control: Ephemeral` 在前、recall 块不带在后，否则缓存断点漂移 5-10× 成本）；P3/P4 seam 挂载点不得移动。
- **signature-run-chat-loop.md**：快照停在 23 参时代，但有**至今未撤销的方向性警告**——"Do not refactor this into a struct without re-running all integration tests + cargo check"；:167 起有按日增参历史表（止于 2026-07-07 的 25 项）。重构必须正面回应（全量回归兑现）并同步此文演进表。
- **Entry invariant**（:112 起）：尾条 user message 必须"新鲜未落盘"；禁止恢复 `messages.len()` 判据（08-04 群聊 400 死循环根因）。
- **R3 终态语义**：合成终态 `Done{usage}` 只允许 sink 侧去重推进一次。
- **debt-linkage.md**：
  - RULE-A-006（closed）：production/tests 同穿 run_chat_loop，签名变化由编译器强制全量迁移；
  - **RULE-A-014**（closed 06-20，事故）：worker 嵌套漏穿 is_worker → Plan 模式 Tier 4 ask 永久挂死；修复即 `is_worker: Option<bool>` 第 21 参 + `effective_is_worker` 收敛；回归测试 `agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied` 断言 15s 内退出。**parameter object 必须保住这条显式传播路径与回归测试。**
  - RULE-A-015：skip_persist 门不得罩终态 Done emit；
  - RULE-A-016：worker grant/审计隔离；
  - **防分叉条款**（:66-70）：禁"v1 + v2 并存（哪怕 for tests）"——parameter object 属改签名而非 fork，勿采用新旧共存过渡方案。

## 附：设计直接推论

1. 38 参中约 24 参可归入 AppState 派生的长寿命 suite（QueueDriverDeps 已验证）；每 turn 状态只有 drive.rs:253-262 的 10 人名单（+cwd），独立小包。
2. 尾部追加约定 + 70 个位参测试调用点：迁移要么一次性机械替换、要么测试侧先建构造 helper（tests_common.rs 现成宿主，cfg(test) 不入 clippy gate）。
3. `dispatch_tool_calls`（33）与 run_chat_loop 共享约 20 同名参数，一处结构两端受益；worker 链（dispatch/* 6 处 allow）镜像主循环，天然二期受益者。
4. spec 的"不要 config struct"警告基于 23 参时代；现为 38 参 + 70 测试点，前提已被恶化推翻，但仍须以完整回归跑通作为验收，并在 signature-run-chat-loop.md 销账。
