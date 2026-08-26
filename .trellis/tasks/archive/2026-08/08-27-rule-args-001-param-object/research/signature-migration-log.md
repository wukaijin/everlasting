# 签名迁移实录（Step 1–5 生产侧，2026-08-27）

> 执行代理：trellis-implement（子代理）。基线 `6ce9ef4`，工作区保持脏、未 commit。
> 范围 = implement.md Step 1–5（仅生产侧）。Step 6 测试位点翻译由下一位代理接手。

## 0. 验证状态

- `cargo check -p everlasting --lib` ✅ 零错零警
- `cargo clippy -p everlasting --lib -- -D warnings` ✅ 通过
- `cargo fmt --check`（改动 .rs 全部）✅
- AC grep：`app/src-tauri/src/agent/chat_loop*` 内 `too_many_arguments` allow = **0**；全库剩余 **36** 处（46 − 10，全部为一期范围外）
- 无 `run_chat_loop_v2` / 旧签名 wrapper（防分叉条款满足）
- `cargo test --lib`：**预期不可编译**（70 处测试位参调用点待 Step 6），未做任何测试文件改动

## 1. 新类型骨架（`agent/chat_loop/suite.rs`，hub 已 `pub(crate) use suite::*;`）

### ChatLoopDeps（AppState 派生长寿命套件，derive Clone）

| 字段 | 类型 | ← 旧 run_chat_loop 位参 |
|---|---|---|
| db | SqlitePool | #9 db |
| cancellations | Arc<Mutex<HashMap<String,CancellationToken>>> | #10 |
| session_active_request | Arc<Mutex<HashMap<String,String>>> | #11 |
| read_guard | ReadGuard | #12 |
| memory_cache | Arc<MemoryCache> | #13 |
| skill_cache | Arc<SkillCache> | #14 |
| permission_asks | PermissionStore | #15 |
| token | CancellationToken | #16（每请求新建，不能从 AppState 派生——`from_app_state(state, token)` 显式带参） |
| background_shells | DefaultRegistry | #18 (L1a) |
| stub_loaded | Arc<StubRegistry> | #37 (C7D) |
| question_store | QuestionStore | #33 |
| subagent_cache | Arc<SubagentCache> | #27 (L3d) |

**构造面**：`ChatLoopDeps::from_app_state(&AppState, token)` + 命名模板
`From<ChatLoopDepsParts>`（12 具名字段 struct）+ `impl From<&QueueDriverDeps>`。
**无多参 fn 组装器** —— 原 from_parts(12 参) 会触发 clippy >7 警告且 AC3 要求
chat_loop* 目录内 allow 归零，故以具名字段 Parts literal 替代（新增字段时编译器
强制所有生产点对齐，防漂移能力等价）。

### ChatLoopRequest（单次请求值；不 derive Debug——含 dyn sink/provider）

| 字段 | ← 旧位参 |
|---|---|
| tool_defs | #1 |
| provider | #2 |
| context_window | #3 |
| provider_id | #4 (WP2) |
| rid | #5 |
| session_id | #6 |
| messages | #7（entry invariant 注释随迁；prepare 内 mem::take 移交） |
| sink | #8 |
| resend_seq | #17 (D3 PR3) |
| max_turns | #19 (B6) |
| workflow_ctx | #34 (W1，loop 内可变) |
| group_chat_state | #35 |
| current_speaker | #36 (TODO-A) |

### CallerRole（调用方角色旗标；dyn SubagentEventSink 无 Debug 故不 derive）

| 字段 | ← 旧位参 |
|---|---|
| is_worker | #22 (RULE-A-014) |
| skip_session_active | #20 (B6 PR1b/RULE-E-005) |
| skip_persist | #21 (RULE-A-015 16 写点门全文随迁字段 doc) |
| skip_cancellations | #38 (F1 双抑制) |
| worker_catalog | #23 (P2.4 C5) |
| worker_event_sink | #24 |
| system_prompt_override | #25 (B6 defect A) |
| worker_run_id | #26 (RULE-FrontSubagent-003) |
| run_grants | #28 (06-26 per-run grant, RULE-A-016) |
| worktree_override | #29 (L3b) |
| project_main_override | #30 (07-29) |
| app_data_dir | #31 (L3b pass-through) |
| forced_dispatch | #32（inventory 行锚点 ：539） |

（inventory §1.1 的行号锚点逐条保留在各字段 doc comment 中；"28→29 trailing
expansion"/"29→30 arg" 等流水账按授权压缩。）

### TurnCarry（by-value 十人名单，design D2 字段逐一对应）

messages / seq / head_sha / system_prompt / permission_ctx / loop_window /
loop_hit_count / last_usage_terminal / workflow_ctx / summary_anchor
← drive.rs:253-262 十个 `let mut x = x;` 重绑定，1:1。

### 辅助小结构（同放 suite.rs）

- `TurnFrame<'a>`：LoopInit 派生常量引用（loaded_session/project/worktree_path/
  mode_prefix/model_briefs/session_mode/effective_is_worker/stub_on/budget_on/
  memory_token/digest_on/synthetic_prefix_len/compaction_on/system_token/
  at_files_token/at_file_spans/current_user_msg_idx/memory_catalog_blocks）
  ＋ 循环控制标量 `turn` / `force_compaction`（&mut 由轮间逻辑推进）。
  ⚠️ design 未点名此结构——十人名单之外的第三个"派生上下文"归属这里
  （"生命周期+所有权来源"原则的直接推论）。
- `TurnHot { current_ctx, last_cwd }`：cwd 状态对（shell cd 经 DispatchOutcome
  回写；inventory §4 "十人名单 + dispatch 期间的 cwd"的后半句实体化）。
- `DispatchCtx` / `FinalizeFrame<'a>` / `SummaryCompactionJob<'a>` /
  `TurnBudgetAsk<'a>` / `HardTurnsTerminal<'a>`：各函数非套件残余的具名包。
- `SuiteRefs` 初稿被弃用（最终形态直接传三套件引用，见 §2 函数表）。

## 2. 函数签名对照（旧→新）

| 函数 | 旧参数数 | 新签名 |
|---|---|---|
| run_chat_loop | 38 | `(mut request: ChatLoopRequest, deps: ChatLoopDeps, role: CallerRole)`（3） |
| prepare_loop_state | 19 | `(&mut ChatLoopRequest, &ChatLoopDeps, &CallerRole)`（3） |
| drive_turn | 49 | `(&req, &deps, &role, frame: &mut TurnFrame, carry: TurnCarry, hot: &TurnHot)`（6） |
| dispatch_tool_calls | 33 | `(&req, &deps, &role, ctx: DispatchCtx)`（4） |
| finalize_turn | 11 | `(&req, &deps, &role, fx: FinalizeFrame)`（4） |
| attempt_summary_compaction | 13 | `(&req, &deps, frame: &TurnFrame, job: SummaryCompactionJob)`（4） |
| ask_turn_limit_softcap | 13 | `(&req, &deps, &role, ask: TurnBudgetAsk)`（4） |
| emit_max_turns_terminal | 8 | `(&req, &deps, &role, tail: HardTurnsTerminal)`（4） |

全部 ≤7（clippy too_many_arguments 默认阈值），豁免删除有据。

### 函数体保真手法（design D3 "编译器驱动的改名"落地方式）

每个改造函数在入口做与旧位参**同名同形**的解包/重绑定块：

- run_chat_loop：owned 重绑定（克隆次数 ≡ 旧入口实参传递），此后 ~600 行主体
  与四个 helper 调用点外的正文**逐字节不动**；
- drive_turn：`let TurnCarry{..} = carry;` 直接还原十个 mut 名单 + frame Copy
  出 `turn`/`force_compaction` 别名 → 正文零改名；
- dispatch_tool_calls/finalize_turn/prep/attempt_compaction 同法。

run_chat_loop 内部实际改写的正文锚点仅 8 处（git diff hunk 可核）：
CancellationGuard 之前的 shadow 块注入、prepare 调用、frame/hot 构建 + 循环头
两标量更名、drive/dispatch/finalize/tail 四个调用点、softcap 内部 3 个终止臂
与 timeout 臂对 emit_max_turns_terminal 的新形参转发。

## 3. 生产调用点（全部迁移）

| 调用点 | 改造 |
|---|---|
| chat_inner 经典分支 | `ChatLoopDeps::from_app_state(state, token)` 统一构造于 spawn 前；闭包顶解构还原同名局部；else 臂 Parts 重装 deps + CallerRole/ChatLoopRequest 显式构建。**三处生产拼装点共用同一构造面** |
| chat_inner 签名 | 9 参收敛为 `(state, ChatEntry)` 二元（ChatEntry 为 transport 入口载荷具名包）；Tauri `chat` 命令与 daemon/routes/agent.rs 两 caller 同步 |
| run_queue_driver 每轮 | `ChatLoopDeps::from(&deps)` + Role(双抑制 guard)/Request(reload 后历史)；QueueDriverDeps 本身未动一个字段 |
| run_group_chat_loop ×2（moderator/participant） | 最小适配：Parts 构建（空 stub registry 占位语义照旧）+ Request/Role；**23 参外层签名一行未动** |
| subagent/dispatch/drive.rs drive_worker 嵌套递归 | 从现有入参机械映射（38 位参 → 三套件字段逐一对照，Box::pin 保留）；drive_worker 自身签名与其余 dispatch 子系统未动（二期） |

## 4. 删除的 allow 清单核对（应删 10，实删 10）

| 文件:原行 | 附着目标 |
|---|---|
| chat_loop.rs:148 | LlmRetrySink 孤儿残留（或有的第 4 处，inventory §5 计入 chat_loop.rs×4） |
| chat_loop.rs:318 | run_chat_loop(38) |
| chat_loop.rs:1281 | emit_max_turns_terminal(8) |
| chat_loop.rs:1350 | ask_turn_limit_softcap(13) |
| drive.rs:165 | drive_turn(49) |
| drive.rs:2499 | attempt_summary_compaction(13) |
| tools.rs:46 | dispatch_tool_calls(33) |
| tools.rs:1778 | finalize_turn(11) |
| init.rs:120 | prepare_loop_state(19) |
| chat.rs:166 | chat_inner(9)（连带前置 "// See DEBT.md RULE-ARGS-001" 注释行） |

未动（超范围）：group_chat_loop.rs:209（23 参签名一期不收缩）、subagent/dispatch/
drive.rs:53（drive_worker）、以及其余 34 处非 chat-loop 家族。
注意 chat.rs:145 还有一个附着在 `enum ChatAcceptance` 上的孤儿 allow —— 与
本任务改动无关且不在计数口径内，为保 blame 干净未触碰（留给后续小额清理）。

## 5. 与 design.md 的偏离及理由

1. **CallerRole 内 group-chat 双件未独立小簇**：workflow_ctx/group_chat_state/
   current_speaker/design 表原列于 request/extras 混合态；终版把 workflow 三件套
   归 ChatLoopRequest（它是 per-request 域上下文而非调用方身份），Role 仅留
   身份/隔离/skip 族。design §总体形态括注已授权此类微调。
2. **TurnFrame/TurnHot 两个新小结构**（design 只点名 TurnCarry）：49→6 参在
   ≤8 且 ≤7（clippy）双约束下的必然产物；内容物严格取自 LoopInit 出参与
   fn-scope cwd 对，无状态发明。
3. **from_parts(fn) → From<ChatLoopDepsParts>**：AC3 要求 chat_loop* 内该
   allow 归零，12 参组装器必然违例；Parts 具名字段 literal 是防漂移等价解。
4. **SuiteRefs 弃用**：中途设计，后被"直接传 (&req,&deps,&role)"替代（参数
   数已达标，少一层间接）。
5. **克隆次数的净变化**（均为套件化机械产物，行为不变）：
   - 减少：drive/dispatch 每 turn 的 project/worktree_path/model_briefs/
     at_file_spans/memory_catalog_blocks/tool_defs/tool_defs×site 等 **约每 turn
     省 5~7 次 Vec/Arc clone**（改为借用/frame Copy）；
   - 增加：run_chat_loop 入口一次性 shadow 块（resend_seq/current_speaker/
     provider_id/stub_loaded/is_worker 等 ~6 个轻量 copy/clone ≡ 旧槽位克隆）、
     workflow_ctx 每请求 +1 次 WorkflowCtx clone（原为 move）、prep 内
     worktree/project_main override 各 +1 次 PathBuf clone（role 借用不可移出）。
   - 权属注释：正文标注于各重绑定块头部。
6. **dispatch 并发臂内 workflow_ctx 连续两次 `.clone()`（tools.rs 原样怪状，
   评审 P 复制粘贴痕）**：保持原样未顺手去重（硬约束 4）。

## 6. 行为保真复核记录（人工 diff 审查结论）

- `!skip_persist` 写点计数 OLD=NEW 逐文件相等（hub 6/init 3/tools 13/drive 29）；
  gate 表达式文本无一改动，diff 中出现的都是位参槽位移除与缩进漂移。
- `stop_reason:` 终态发射点计数 OLD=NEW（hub 5/drive 9/tools 1）。
- CancellationGuard 构造语句仍为函数体第一句（shadow 之后），字段表达式与
  旧一致；F1 双抑制经 role.skip_session_active/skip_cancellations。
- messages[0] cache_control 断点顺序：init.rs B5/B4 插入代码 0 改动。
- P3/P4 seam（tools.rs parallel/serial recall+reflect 挂载点）：0 改动。
- D-D 入口守卫（dd_guard_hit/user_message_matches）：0 改动。
- RULE-A-014 链路：role.is_worker（init.rs:145 取别名）→ effective_is_worker
  unwrap_or(false)（:378）→ PermissionContext.is_worker（:383）→ 既有测试逻辑
  一行未动（tests 尚未编译通过系 Step 6 范畴）。
- R3 终态 Done usage 去重路径（sink 侧守卫）：消费端 0 改动。

## 7. 给 Step 6 测试翻译代理的操作要点

1. **fixture 形状（对齐 inventory §2.3 四种 harness）**：
   - `basic.harness`（tests_agent_loop 42 处）：默认 =
     `ChatLoopDeps::from_app_state(&state_like, token)` 做不到——测试无 AppState；
     请给 tests_common 提供 `TestFixture::default(deps_core...)`：
     - deps 用 in-memory pool + 真 Arc registry 组 Parts（比照 QueueDriverDeps
       在 tests_message_queue.rs:91 的直构习惯）；
     - Request 默认 `{tool_defs: builtin_tools(), provider: MockProvider(Arc),
       context_window: 8k~64k, provider_id: None, rid/session_id: uuid,
       messages: vec![user_msg], sink: MockEmitter(Arc), resend_seq: None,
       max_turns: None, workflow_ctx: None, group_chat_state: None,
       current_speaker: None}`；
     - Role 默认全集 = 旧行展开（is_worker Some(false)、skip×3 false、
       worker 双件 None/ThreadLocalSubagentSink、四 override None、
       app_data_dir 空路径、forced_dispatch None）。
     worker 测试改 `.with_role(|r| r.is_worker=Some(true).skip_persist=true…)`
     或提供 `worker_role()` 变体（对照 prep/dispatch 测试）。
   - queue-driver 两处直构 QueueDriverDeps 不动（其字段面未变）。
   - compaction_summary / softcap / turn_checkpoint 的本地 `run_loop(...)` 包装器：
     包装器内部一次翻译到 fixture，外部测试零感知。
   - tests_group_chat(2)：需置 group_chat_state=Some(SharedTurnState) +
     current_speaker=speaker + max_turns（1/20）分支。
2. `is_worker` 相关断言若失败先查 Role 是否忘带 `Some(false)` 显式值（生产契约）。
3. 注意软卡 env 钩子（EVERLASTING_SOFTCAP_*）测试与 Timeout 分支语义未变，
   fixture 不要吞 env 设置顺序。

## 8. 遗留（下一步代理须知）

- Step 6：70 处测试调用点 + 上述包装器；预计 todo: tests_common.rs 现成宿主。
- Step 7：spec 更新 signature-run-chat-loop.md（演进表补 38 参终点 +
  parameter-object 章节；本文档 §1/§2 可作为素材源）。
- DEBT.md RULE-ARGS-001 条目删除请在 AC 全门（全绿 lib 测试）通过后进行。

## Step 6 测试位点翻译记录（2026-08-27）

> 执行代理：trellis-implement（子代理）。延续 §0 状态：工作区脏、未 commit；
> 生产代码零改动（本步仅动 cfg(test) 侧 + tests_common.rs）。

### 1. fixture（tests_common.rs，比照 harness 既有自由函数习惯）

三个缺省组装器替代 design D4 的 builder-object 形态（与 `make_harness` /
`test_messages` 同风格；§7 的「default 展开 + 差异覆盖」要点逐条落实）：

- `chat_loop_request(tool_defs, provider, context_window, rid: String,
  session_id, messages, sink)` —— 旧位参 #1–#7 阅读序；#4 provider_id 由
  构造器代置 None。**rid 收具体 String 而非 `impl Into<String>`**：调用点
  实参已是 `.into()`/`format!` 产物，泛型边界会与之打成 E0283 推理循环。
- `chat_loop_deps(&TestHarness)` —— harness 字段逐一 clone 组 Parts
  （= §7 "in-memory pool + 真 Arc registry"），token 缺省
  `CancellationToken::new()`，取消类测试以 `deps.token = ..` 具名覆盖。
- `parent_role(&TestHarness)` —— parent 全集展开（is_worker Some(false)、
  skip×3 false、ThreadLocalSubagentSink、四 override None、app_data_dir 取
  harness tempdir）。worker 形态全部由调用点具名覆盖。

调用点统一形状 = 表达式位的 `{ let mut x = ctor(); x.f = v; x }` 差异块，
嵌套位（plan_mode 的 timeout 包装）同样合法。24 个文件扩展既有
`use super::tests_common::{…}` 导入；tests_subagent/mod.rs 手工补一行
（它只 import 模块本体）。

### 2. 位点计数（实测 vs inventory §2.3 图谱）

| 文件 | 本次 | inventory §2.3 |
|---|---|---|
| tests_agent_loop/ 合计 | **48**（basic 8、error_path 6、stub 6、resilience/error_persist 各5、notifications 4、parallel_dispatch/checklist 各3、recall/softcap 各2、budget/compaction_summary/turn_checkpoint/turn_usage_event 各1） | 42 |
| tests_subagent/ 合计 | **15**（dispatch_main 6、persist_audit_token 4、system_prompt_override 2、forced_dispatch/mod/plan_mode 各1） | "20+" |
| 散布 | **7**（c2plus 2、group_chat 2、ask_user_question/request_mode_change/sse 各1） | ≈8（差额同上） |
| **总计** | **70**（编译器 E0061 清单逐一穷尽，非抽样） | 70 |

图谱的分文件拆分是 Step 1 前快照的近似（总数一致）；以本次
`cargo check --tests` E0061 全清单为准。`tests_message_queue.rs` 两处
QueueDriverDeps 直构零改动（字段面未变，符合预期）；handoff/
manual_compaction/l3a/l3b 等不经 run_chat_loop 入口的文件零波及。

包装器处置：
- compaction_summary.rs `run_loop`（7 参）/ turn_checkpoint.rs `run_loop`
  （5 参）：内部翻译到 fixture 后删 `#[allow(clippy::too_many_arguments)]`
  （已无必要）；doc 里"36 参对齐"措辞同步改掉。
- softcap.rs `run_loop`（9 参）：**保留 allow**——外部签名未收缩，豁免仍必要。
- tests_subagent/mod.rs `run_loop`（6 参）顺带翻译（不在指令三包装器名单内，
  但它是 mod.rs:37 的宿主）；无 allow 需处理。

### 3. 两可判断点及取舍

1. **Boilerplate 注释的处理**：~60 个位点重复粘贴的 RULE-A-014/B6 PR 系列
   出处注释未在调用点复制保留——其原文已在 suite.rs 对应字段 doc comment
   （migration log §1/§4 核实），fixture 头注也写明默认像契约。**真正
   site-specific 的语义注释逐一手工回植**：c2plus worker 位点的 tool_defs
   理由块、system_prompt_override 位点的"is_worker=Some(false) 但 override
   才是 worker 性"整段、turn_checkpoint 的"checkpoints ACTIVE"、
   forced_dispatch 的 QuestionStore 签名必填说明、group_chat 双守卫位点的
   `Some(1) // single turn` 与 guard skip 行内注释（附于赋值行尾）。全量删除
   清单经 git diff 审过一遍（见过程 audit 输出）。
2. **app_data_dir 缺省取 harness tempdir** 而非空路径：旧行即传
   `h.app_data_dir.clone()`，loop 体虽不读、但 dispatch interceptor 会——
   取等价值避免任何行为面变化。
3. **自由函数而非 builder object**：design D4 的
   `ChatLoopTestFixture::default().with(...)` 被落成三个组装器 +
   字段赋值差异块。理由：harness 惯例即自由函数；且差异字段直接具名赋值
   比 with 链更接近旧调用点的读法、diff 噪声最小。
4. plan_mode 位点包在 `tokio::time::timeout(...)` 内 → 采用表达式位块的
   统一形状后无需提升语句，结构一行未动。

### 4. 最终验证输出（原文）

全量 `cargo test -p everlasting --lib`（满载并行，多线程默认）：

```
test agent::tests_subagent::plan_mode::agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied ... FAILED
failures:
test result: FAILED. 1996 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 111.67s
```

唯一失败项与 [baseline-notes.md](./baseline-notes.md) 记录的两条满载抖动
计时测试之一同名（基线固化时它在两次满载 run 中同样红）；单独重跑裁决：

```
test agent::tests_subagent::plan_mode::agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1997 filtered out; finished in 0.62s

test daemon::server::tests::serve_daemon_keeps_serving_without_signal_past_grace_window ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1997 filtered out; finished in 5.99s
```

两条均单独绿 → 按 baseline-notes 判定规则不视为行为回归（逻辑/时限未动）。

`cargo clippy -p everlasting --lib -- -D warnings`：

```
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 11.55s
```

其余门：改动文件 `cargo fmt --check` ✅；tests 编译 0 warning；
测试域 `too_many_arguments` 仅剩 softcap.rs:225 一处（9 参包装器必要豁免）；
chat_loop* 家族 allow 维持 0。AC grep 口径不变。

## trellis-check 复核记录（2026-08-27）

> 执行代理：trellis-check（子代理）。基线 HEAD=`6ce9ef4`，审计对象 = 工作区全部未提交改动
> （Step 1–5 生产侧 + Step 6 测试翻译 + Step 7 spec 文档）。全程未 commit。

### Verdict

**可安全合入** —— 发现并修复 P0 级行为漂移一处（F-1），其余四个透镜零 finding；
修复后五道门独立复验全绿。

### Finding 清单

| # | 严重度 | 位置 | 描述 | 处置 |
|---|---|---|---|---|
| F-1 | **P0（行为回归，违反 R5/RULE 保真）→ 已修复** | `chat_loop/tools.rs::dispatch_tool_calls` 重绑定表 | 新代码把旧调用点传入的 `&workflow_ctx`（run_chat_loop 函数域活绑定）误映射为 `&request.workflow_ctx`（入口快照）。该字段唯一的行为敏感消费者是 W1 Step 2.4 角色门 `check_workflow_role_gate`（`dispatch.rs:411`，读 `ctx.current_task.status`）——而 `current_task` 正是 drive_turn 轮顶从盘上刷新、经 `DriveTurnOutcome` 回写函数域绑定的那份拷贝（drive.rs 原 :918-940 注释即契约）。多轮工作流 session 中 task.json 在 loop 入口后发生任何变更（用户改任务、上轮 request_task_state_transition 异步 apply、非隔离 worker 写盘），角色门将以过期状态做 deny/allow 判定 → 行为变化。全量测试未抓住的原因：role-gate 只有纯函数单测，无多轮 workflow-session 集成断言（观察项 O-1）。 | **已修**：`DispatchCtx` 加生命周期 `'a` 与字段 `workflow_ctx: &'a Option<WorkflowCtx>`（suite.rs），`dispatch_tool_calls(ctx: DispatchCtx<'_>)` 解构取得活引用，`chat_loop.rs:814` 构造点传 `workflow_ctx: &workflow_ctx`（LoopInit 回流后的每-turn 最新值）。修复不新增克隆、签名仍为 4 形参（spec 表无需改动）；备选方案（写回 request.workflow_ctx 或加第 5 形参）因保真度/表驱动漂移被否。逐字对齐旧行为：旧 dispatch 实参正是同一绑定在 write-back 之后的引用。 |
| F-2 | P3（文档一致性）→ 已修复 | `.trellis/reviews/DEBT.md:61` | P2 段头计数 `[3 items]` 与实际条目（RULE-SHELL-001 + RULE-FE-001）及「优先级分布」表（P2=2 / Total=11）不一致。 | 改为 `[2 items]`。 |
| O-1 | 观察项（不阻塞） | `agent/subagent/tests_dispatch.rs` gate_* 单测 | workflow 角色门只有纯函数级覆盖，缺"多轮 loop 中 task.json 变更后下一 turn 门判定刷新"的集成断言——正是 F-1 能漏过全量绿的原因。 | 不在本任务扩scope修测；候选登记后续小额任务（结合 baseline-notes 已挂的 plan_mode 断言文案失实一起清理亦可）。 |

确认现状（按授权不动）：`chat.rs:147` 枚举上的孤儿 allow、softcap.rs 测试包装器 allow（9 参仍必要）、plan_mode 断言文案与 2s 实际预算不符（baseline-notes 已留痕）。

### 核对矩阵摘要（五透镜）

- **L1 参数映射完备性 ✅**：38 位参 ↔ 三套件字段在 4 个生产调用点逐一人工对照（chat_inner 经典分支 / 队列驱动器 / 群聊 moderator+participant / worker 嵌套递归），每个簇 ≥3 参数抽查，值语义零出入（含 queue driver 双抑制 true/true、群聊 Some(1)/Some(20)/resend=None/moderator speaker、worker 的 is_worker=Some(true)+skip_persist=true+skip_session_active=true+skip_cancellations=false+空 stub registry）。helper 函数（prep/drive/dispatch/finalize/compaction/softcap/terminal）重绑定表逐行核对；`request./deps./role.` 在 chat_loop.rs 全文仅出现于 shadow 块与 helper 头部重绑定（grep 验证），无隐式中途读取。
- **L2 行为保真 ✅（含 F-1 修复）**：RULE-A-014 链 init.rs:145→:378→:383 一眼可循，worker 递归位参 Some(true) 原样；`!skip_persist` 门计数 OLD=NEW 独立复验 hub 6/6、init 3/3、tools 13/13、drive 29/29；终态 stop_reason 计数 OLD=NEW hub5/drive9/tools1，max_turns 门拓扑与 HEAD 逐字节一致；CancellationGuard 构造保持在 shadow 块（纯 clone 无 await 无早退）之后原位、双抑制/skip 字段原表达式；messages mem::take 移交与 LoopInit 回流等价旧的按值移交（Err 早退两侧同为丢弃）；D-D 守卫/init B4·B5 cache_control/P3·P4 seam 区域 diff 零 hunk；事件发射次序零漂移。diff 中非翻译性质改动定性：克隆→借用换血（frame/借用形参，§5 已记录）、tokio::select! 体缩进重排（rustfmt 不解析 select! 宏体，仅缩进逐臂核对等价）——均良性。
- **L3 测试面完整性 ✅**：70 位点 + 3 包装器 diff 全量扫描：被删非注释行的全集 = 位参值 + import 行；数字承载行逐一配对（200_000×63≡63、Some(1)×3、Some(2)→`request.max_turns = Some(2)`、20_000/1000/各 rid/mock/emitter 均 added 侧重现）；assertion/timeout/call_count 零变动；plan_mode 2s 包裹结构原样（RULE-A-014 位参翻译核对无一错位）；site-specific 注释抽查回植成立（error_path.rs:141 等），boilerplate 弃置符合 §Step6 记录的授权取舍。
- **L4 范围纪律 ✅**：QueueDriverDeps 字段面零改动（仅新增 `From<&QueueDriverDeps>`）；drive_worker 及 dispatch 子系统签名零改动（仅其内部递归调用点翻译）；run_group_chat_loop 外层 23 参原样；allow 计数账目自洽（46→34 = 生产 −10 + 测试包装器 −2）；DEBT/spec 改动均在任务口径内（F-2 为口径内笔误修正）。
- **L5 AC 独立复核 ✅**（见下方原文末行）。

### 验证输出（末行原文）

```
cargo clippy -p everlasting --lib -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 16.01s

cargo test -p everlasting-remote
test result: ok. 89 passed; 0 failed; 0 ignored; 0 measured; 90 filtered out; finished in 0.91s

cargo test -p everlasting --lib           （F-1 修复后全量）
test result: FAILED. 1996 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 100.10s
  ↳ 唯一失败 = agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied
    （baseline-notes 两条满载抖动计时测试之一，基线 run#1/#2 同名红过）
单独重跑裁决：
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1997 filtered out; finished in 0.59s
daemon::server::tests::serve_daemon_keeps_serving_without_signal_past_grace_window：
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1997 filtered out; finished in 5.61s

grep '#[allow(clippy::too_many_arguments)]' app/src-tauri/src/agent/chat_loop*  → 0 处
grep -rn 'run_chat_loop_v2' app/src-tauri/src/                                    → 0 处
全库 allow 余量 = 34（46 − 生产10 − 测试包装器2）
```

AC1/AC3/AC4/AC5 直接通过；AC2 按 baseline-notes 判定协议通过（两条抖动项单跑绿，
除其外三轮无其他失败名）。`cargo fmt --check` 于全部 Rust 改动后复跑通过。
