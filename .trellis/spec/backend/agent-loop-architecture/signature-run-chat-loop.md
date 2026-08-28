## Signature: `run_chat_loop`

**Location**: `app/src-tauri/src/agent/chat_loop.rs:313`；套件对象定义在
`app/src-tauri/src/agent/chat_loop/suite.rs`（经 `chat_loop.rs:82` 的
`pub(crate) use suite::*;` 出口对内可见）。

```rust
pub async fn run_chat_loop(
    mut request: ChatLoopRequest,
    deps: ChatLoopDeps,
    role: CallerRole,
) { ... }
```

2026-08-27 起（RULE-ARGS-001 一期），旧 38 个线性裸参收敛为三个领域对象。
按「生命周期 + 所有权来源」归类：

| 对象 | 角色 | 归入内容（字段全文见 `suite.rs` 各 doc comment） |
|---|---|---|
| `ChatLoopRequest` | 单次请求值，入口逐值构建 | tool_defs / provider / context_window / provider_id / rid / session_id / **messages** / sink / resend_seq / max_turns / workflow_ctx / group_chat_state / current_speaker / drained(队列驱动器本轮 drained 全量,RULE-QUEUE-001 后替代原 `origin` 尾条字段;非驱动器路径恒空) |
| `ChatLoopDeps` | AppState 派生长寿命套件 | db / cancellations / session_active_request / read_guard / memory_cache / skill_cache / permission_asks / token / background_shells / stub_loaded / question_store / subagent_cache |
| `CallerRole` | 调用方角色旗标（单请求常量） | is_worker / skip_session_active / skip_persist / skip_cancellations / worker_catalog / worker_event_sink / system_prompt_override / worker_run_id / run_grants / worktree_override / project_main_override / app_data_dir / forced_dispatch |
| `TurnCarry` + `TurnFrame<'a>` / `TurnHot` | 每-turn 可变状态与派生上下文（by-value / 借用，不进 deps） | TurnCarry = drive.rs 十人名单（messages / seq / head_sha / system_prompt / permission_ctx / loop_window / loop_hit_count / last_usage_terminal / workflow_ctx / summary_anchor）；TurnFrame = LoopInit 派生常量引用 + `turn`/`force_compaction` 控制标量；TurnHot = cwd 对 |

每个字段的原始位参编号（`#22 (RULE-A-014)` 式锚点）与历史决策注释已随迁到
`suite.rs` 字段 doc comment —— 查"这个参数当年为什么加"看那里，不在本文。

链上函数同批收编（签名变更由编译器强制全量迁移调用点，RULE-A-006 同款语义）：

| 函数 | 旧参 | 新签名（参量主体为三套件引用 `&req/&deps/&role`） | 位置 |
|---|---|---|---|
| `run_chat_loop` | 38 | `(mut request, deps, role)`（3） | `chat_loop.rs:313` |
| `prepare_loop_state` | 19 | `(&mut request, &deps, &role)`（3） | `chat_loop/init.rs:126` |
| `drive_turn` | 49 | `(&req, &deps, &role, frame: &mut TurnFrame, carry: TurnCarry, hot: &TurnHot)`（6） | `chat_loop/drive.rs:173` |
| `dispatch_tool_calls` | 33 | `(&req, &deps, &role, ctx: DispatchCtx)`（4） | `chat_loop/tools.rs:52` |
| `finalize_turn` | 11 | `(&req, &deps, &role, fx: FinalizeFrame)`（4） | `chat_loop/tools.rs:1787` |
| `attempt_summary_compaction` | 13 | `(&req, &deps, frame, job: SummaryCompactionJob)`（4） | `chat_loop/drive.rs:2482` |
| `ask_turn_limit_softcap` | 13 | `(&req, &deps, &role, ask: TurnBudgetAsk)`（4） | `chat_loop.rs:1020` |
| `emit_max_turns_terminal` | 8 | `(&req, &deps, &role, tail: HardTurnsTerminal)`（4） | `chat_loop.rs:944` |

最外层入口同步收敛：`agent/chat.rs::chat_inner` 9 参 → `(state: &Arc<AppState>,
entry: ChatEntry)` 二元（`ChatEntry` 为 transport 入口载荷具名包），Tauri `chat`
命令与 daemon `routes/agent.rs` 两个 caller 共用。本次迁移共移除 10 处
`#[allow(clippy::too_many_arguments)]`（本家族 8 函数 + chat_inner +
LlmRetrySink 孤儿），AC 口径下 `chat_loop*` 范围内该豁免为 0。

> **Warning — Entry invariant: "the tail user message is a fresh, not-yet-persisted send".**
>
> `run_chat_loop`'s entry user-message persist site
> (`chat_loop.rs`, "Persist the most recent user message before the agent
> loop runs") **unconditionally re-persists** `messages`'s tail user-role
> message. This relies on an implicit invariant: that message is a fresh
> frontend send, not an already-persisted DB row.
>
> **Violating it is the root cause of the group-chat 400 death-loop**
> (`08-04-group-chat-orchestration-rewrite`): `messages` has
> `UNIQUE(session_id, seq)` but `run_chat_loop` recomputes `seq = max+1`
> on every entry, so re-persisting a reloaded `tool_result` (role=user)
> writes a NEW row with the same content — no UNIQUE collision → a
> duplicate `tool_result` with no matching `tool_calls` → OpenAI 400 /
> Anthropic 2013 on every subsequent request. DB forensics in
> `.trellis/tasks/08-04-group-chat-orchestration-rewrite/research/db-evidence.md`
> show the same `tool_use_id` accumulating 30+ rows.
>
> **Guard (D-D, 08-04 rewrite)** — the persist site skips re-writing when
> ALL of:
> 1. `group_chat_state.is_some()` (a group-chat speaker — ordinary chat
>    short-circuits, byte-identical behavior);
> 2. the tail user message content-matches **any** user-role row in
>    `loaded_session.messages` (`user_message_matches`: `tool_result`
>    by `tool_use_id`, plain text by byte equality).
>
> Do NOT restore a `messages.len() == loaded_session.messages.len()`
> length criterion (the pre-08-04 heuristic, D-F): it is always false
> after the memory/skill injection pass inserts synthetic user rows into
> `messages`, and it misfires on filtered participant views (fewer rows
> than the DB). Known cosmetic boundary (documented in the guard
> comment): a human re-sending the EXACT same text in a group chat is
> judged already-persisted and skipped — one text row lost, no tool-pair
> breakage, no 400.

**套件化后的携带路径（不变量本体不受影响）**：`messages` 现随
`ChatLoopRequest.messages` 进门（doc comment 内嵌本不变量摘要），由
`prepare_loop_state` 以 `mem::take` 移交、加工后经 `LoopInit.messages`
回流、此后逐 turn 在 `TurnCarry.messages` 里 by-value 穿线。D-D 入口守卫
（dd_guard_hit / user_message_matches）原样活在 init 段——判据仍读
request 侧 messages 与 `loaded_session`，未随字段搬家而变化。

### 现行契约：三套件对象模型与扩展规则（RULE-ARGS-001）

取代旧文"Do not refactor this into a struct"的方向性警告——parameter object
已是既成形态，今后加 feature 的规矩如下：

1. **新状态进对应 struct 的具名新增字段，不往函数签名尾部追加裸参。**
   尾部追加约定（演化表里 "28→29 trailing expansion" 一类流水账）已随本次
   迁移终止。归属判据沿用上表的「生命周期 + 所有权来源」：
   - AppState 派生的长寿命资源 → `ChatLoopDeps`；
   - 单次请求值（含 per-request 域上下文 workflow 三件套）→ `ChatLoopRequest`；
   - 调用方身份决定的隔离 / skip / override 族 → `CallerRole`。
     其中 **`is_worker` 的显式传播链以该字段为载体**：
     `role.is_worker → prepare_loop_state 的 effective_is_worker →
     PermissionContext`（RULE-A-014 保真约束，回归测试
     `agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied`
     断言 <15s 退出）。勿把这条链隐式化或改从环境/session 推断；
   - 每-turn 可变状态走 `TurnCarry` / `TurnHot` by-value（沿
     LoopInit → DriveTurnOutcome 管道惯例），**不进 deps**。
2. **构造路径统一**：能从 `AppState` 取到的资源先扩
   `ChatLoopDeps::from_app_state(&AppState, token)`
   （`suite.rs:155`；`token` 是每请求新建并注册进 `cancellations` 的，
   故显式带参而非从 state 派生），再扩消费者。非 AppState 来源用
   `From<ChatLoopDepsParts>` 具名字段 literal 拼装——刻意不设多参 fn
   组装器，新增 deps 字段时编译器强制所有生产构造点对齐（防漂移能力
   等价于旧位参全量迁移）。既有转换实现：`From<&QueueDriverDeps>`
   （`chat.rs:917`）。
3. **命名惯例**：入口聚合套件 `XxxDeps`（比照先例 `QueueDriverDeps`）；
   IPC 入口一次解析随 request 穿线的域上下文 `XxxCtx`（比照
   `WorkflowCtx` / `GroupChatCtx`）；每 turn 状态包沿 by-value 出入参
   包命名。**禁 `RunChatLoopArgs` 式命名**（既不表生命周期也不表来源）。
4. **回归护栏保持强制门槛**：凡改动本签族（含任一套件增删字段），
   全量 `cargo test -p everlasting --lib` + 
   `cargo clippy -p everlasting --lib -- -D warnings` 仍是变更时的必过门
   （CI 已 `-D warnings`）。这条是旧警告的存活部分——结构化不减测试义务。

### （历史，已封档）Why 23 parameters — and how the era ended

以下论证保留为历史记录，回答"为什么曾经容忍位参堆叠"。写作当时
（2026-06，23 参形态）成立：

> The parameters look excessive, but they are the **exact set of state pieces
> the agent loop body needs**, and grouping them into a config struct would:
>
> 1. Hide the dependency surface (a struct named `RunChatLoopArgs` would tempt
>    callers to add fields that are *only* test-internal)
> 2. Add a layer of indirection without adding safety (Rust's borrow checker
>    already enforces "use what you need")
> 3. Obscure the 1:1 correspondence between production and test call sites
>    (integration tests pass them in the same order, with the same types)
>
> `#[allow(clippy::too_many_arguments)]` was accepted as the deliberate cost
> of keeping the dependency surface explicit.

**终点标注（2026-08-27）**：该警告的前提被自身增长推翻——23 → 38 参的
持续尾部追加（见下表 + 表后明细）叠加测试侧 70 处位参调用点，"依赖面
显式"已成负债。RULE-ARGS-001 以 parameter object 收编终结了这个时代：
三套件按生命周期拆分后，上述三条反对理由分别由新机制替代——

- 反对理由 1（test-only 字段藏进 struct）→ 由「CallerRole 只收调用方
  身份族」的字段归属纪律约束；
- 反对理由 2（间接层无安全增益）→ 反转：`Parts` 具名字段 literal 让编译器
  承担了原本靠人眼维护的对齐检查；
- 反对理由 3（生产/测试调用点 1:1 对应）→ 未丢失：三套件两侧同构构建
  （见下方 call-site parity 节），且 RULE-A-006 的编译器强制迁移语义不变。

原句 "Do not refactor this into a struct without re-running all integration
tests + cargo check" 就此退役——重构已发生，其保留义务并入现行契约第 4 条
的回归护栏。

#### Evolution log (parameter count grew with new features)

| Date | Count | PR / task | New param | Why |
|---|---|---|---|---|
| 2026-06-15 | 14 | `06-15-unify-chat-loop-dispatch` (RULE-A-006 closure) | — | baseline after production migrated through `run_chat_loop` |
| 2026-06-17 | 15 | D3 PR3 | `resend_seq: Option<i64>` | resend audit row at user-message persist site |
| 2026-06-19 | 17 | L1a | `background_shells: DefaultRegistry` | cross-request registry threaded into `ToolContext` + per-turn notification drain |
| 2026-06-19 | 18 | B6 PR1a | `max_turns: Option<usize>` | worker turn cap; production + tests pass `None` |
| 2026-06-19 | 19 | B6 PR1b | `skip_session_active: bool` | worker guard Drop skips `session_active_request.remove` |
| 2026-06-19 | 20 | B6 PR1b | `skip_persist: bool` | persist-site gates inside the function body (PR1 spec: 18 sites; PR2a actual: 16 — see RULE-A-015) |
| 2026-06-20 | 21 | B6 PR2b (RULE-A-014) | `is_worker: Option<bool>` | thread `is_worker` to nested `run_chat_loop` so Tier 4 `ask_path` / `ask_shell` collapses to `Deny` on the worker path (workers have no UI sink) |
| 2026-06-20 | 22 | B6 PR3 (PR2 hotfix) | `app_handle: Option<tauri::AppHandle>` | thread the parent's `AppHandle` through so `run_subagent` can wire the worker's `SubagentBufferSink` with a live `subagent:event` IPC emit path (live transcript streaming for the PR3b `<SubagentDrawer>`); tests pass `None`. *后注：P2.4 C5（07-22）以此参换血为 transport 无关的 `worker_catalog` + `worker_event_sink` 双件* |
| 2026-06-21 | 23 | `06-21-fix-worker-system-prompt-dead-code` (B6 review defect A) | `system_prompt_override: Option<String>` | thread the worker's `SubagentDef.system_prompt` through as the override; production + tests pass `None` |
| 2026-06-30 | 24 | `06-30-explicit-agent-dispatch` | `forced_dispatch: Option<ForcedDispatch>` | user `@@<agent> <task>` prefix → turn-1 short-circuit bypasses `provider.stream` |
| 2026-07-03 | 25→26 | `07-03-subagent-per-agent-model-ui` / `07-06-b6plus-b-dispatch-model-arg` | (no new `run_chat_loop` param) | per-agent model priority chain lives upstream in `run_subagent`（`resolve_final_model` / dispatch overlay），`run_chat_loop` 签名不动 |

*（表止于此快照。25→38 的后续追加未逐条入表，明细与行号锚点见
`.trellis/tasks/08-27-rule-args-001-param-object/research/run-chat-loop-signature-inventory.md`
§1.1：worker_run_id(06-22)/subagent_cache(L3d)/run_grants(06-26)/
worktree+project_main_override+app_data_dir(L3b·07-29)/question_store/
workflow_ctx(W1·07-08)/group_chat_state/current_speaker(TODO-A)/
provider_id(WP2·08-20)/stub_loaded(C7D)/skip_cancellations(F1·08-25)。）*

**表终点：38 参时代于 2026-08-27 被 parameter object 收编终结**
（RULE-ARGS-001 一期，旧 38 参 → `(request, deps, role)` 三元，家族函数
49/33/19/11/13 同批塌缩）。本演化史封档；此后的形态演进走上文
「现行契约」的新增字段规矩，不再有参数计数这一说。

### Production + test call-site parity（套件化后形态）

- **生产构造点共用同一构造面**：
  - `chat_inner` 经典分支：spawn 前 `ChatLoopDeps::from_app_state(state, token.clone())`
    统一构建 deps（`chat.rs:546`）；闭包顶解构还原同名局部；
  - 队列驱动器每续轮：`ChatLoopDeps::from(&deps)`（`From<&QueueDriverDeps>`
    转正），Role 为双抑制 guard（skip_session_active + skip_cancellations
    双 true），Request 用 reload 后历史重建；
  - `run_group_chat_loop` ×2（moderator/participant）：`ChatLoopDepsParts`
    具名拼装 + Request/Role 显式构建（外层 23 参签名一期未收缩，二期候选）;
  - worker 嵌套递归（`subagent/dispatch/drive.rs::drive_worker`）：从入参
    机械映射到三套件字段（`Box::pin` 递归保留；dispatch 子系统其余签名
    二期再收编）。
- **Role 默认值契约**（两侧一致）：production / 测试的非 worker 调用点
  显式 `is_worker: Some(false)`（把 production-style default 摆在调用点，
  RULE-A-014 契约）、三个 skip 位 `false`；worker 路径才置
  `Some(true)` + `skip_persist` 等。迁移后新增字段同样适用"默认值显式
  写在调用点"纪律。
- **测试侧**：沿用与生产相同的具名拼装路径构建三套件（deps 走 Parts
  直构 + in-memory pool 与真 Arc registry；request 含 MockProvider /
  MockEmitter），不加 `#[cfg(test)]` 分叉、不设新旧双签名并存路径
  （debt-linkage.md 防分叉条款继续有效）。

---
