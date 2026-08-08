# WORKFLOW-INTEGRATION 二轮评审

> 评审对象：`docs/WORKFLOW-INTEGRATION.md`（2026-07-07 后版本，二轮）
> 前一轮：`docs/WORKFLOW-INTEGRATION-REVIEW.md`（S1/S2/S3 + M1-M7 + 8 个小问题 + 12 个认可点）
> 评审日期：2026-07-08
> 评审方法：复核前一轮修复 + 二次交叉验证（对照 `chat_loop.rs`、`memory_recall.rs`、`tools/update_checklist.rs`、`agent/question_store.rs`、`commands/question.rs`、`agent/subagent/{mod.rs,dispatch.rs}`）
>
> 评审维度（与一轮同）：
> 1. 内部一致性 — Q1-Q9 决定 + 前一轮 10 处修复是否在全文贯彻
> 2. 架构合理性 — engine/content 分离边界、WorkflowDef 接口形态
> 3. 可行性陷阱 — 对照现有 B6 / B12 / 注入 seam / 无 hook runner
> 4. 缺失与风险 — 错误处理、token 预算、Phase 间隐含依赖

---

## 前一轮修复确认

10 处修复均到位，无明显回退或减弱：

| 一轮编号 | 一轮问题 | 二轮状态 |
|---|---|---|
| S1 | delegation 模板注入机制完全缺失 | §6.6.1 修复：engine 走 `inject_recall_into_turn` 同款 seam，append messages[0] block（`cache_control: None`） |
| S2 | B12 loop-local checklist 与文件态割裂 | §6.2 修复：选 (c) task.json items 内嵌，单一数据源，B12 coerce 保留 |
| S3 | Q3 "软→硬" 与 "协商档" 多处矛盾 | §5.2 / §6.1 / §9 / §11.1 / §12 全文统一为"统一协商档" |
| M1 | WorkflowDef 缺 `delegation_templates` | §5.4 修复：加字段 + 4 访问函数，`delegation_template_for` |
| M2 | hook 触发路径未指定 | §8 + §14 修复：选 user 确认门自动调，对标 `resolve_mode_change` pattern |
| M3 | 门控执行点未定义 | §6.6.2 修复：拦截点 A = `chat_loop.rs` dispatch 处，`run_subagent` 前 |
| M4 | wf-* skill 存放 / Phase 1-2 顺序矛盾 | §6.3 + §9.5 修复：M4 决定选 (b) Phase 1 直接做 plugin skill loader |
| M5 | review 协调模型无字段位置 | §5.4 修复：`coordination: String`，默认 `"pipeline"`，review 用 `"round-robin"`/`"fan-out-gather"` |
| M6 | 空/损坏 workflow.json 回退策略缺失 | §5.4 修复：validate + fallback 三步 |
| M7 | token 预算无具体估算 | §11.1 修复：估算表 + 结论"常驻 ~450 tokens + dispatch turn +200" |

但**一轮修复不到位 + 新问题**仍残留，见下。

---

## 1. 严重问题（阻塞实施）

### S-A · `run_subagent` 在 `chat_loop.rs` 有 **3 处** 调用点，§6.6.2 只锁了 1 处门控

**位置**：§6.6.2（§5.2.3、§9 步 2.4、§12 接入点）都引用"chat_loop.rs dispatch 拦截处（`run_subagent` 调用前）"

**问题**：核对 `chat_loop.rs`，`run_subagent` 实际有 3 个调用点：

| 行 | 路径 | 用途 |
|---|---|---|
| `chat_loop.rs:1000` | parent-spawn explicit | 单元测试 / 内部显式派 |
| `chat_loop.rs:2937` | L3b PR2 并发 dispatch（`FuturesUnordered` batch） | 读集并发 path，已落地 |
| `chat_loop.rs:3286` | 串行 path（§6.6.2 标的） | 当前 MVP 多数 turn |

§6.6.2 / §A.1 反复强调"chat_loop.rs dispatch 拦截处（`run_subagent` 调用前）"为唯一门控点——**真正的并发 dispatch path 在 2937 附近**完全没被覆盖。§4.2 评审流 Q8 走 A 路径（round-robin fan-out）本质就是 L3b 现成能力的应用，**门控只在 3286 一处 = 2937 处漏判 + 1000 处漏判**。

**建议**：gate 下沉到 `subagent/dispatch.rs` 内部（`run_subagent` 签名追加 `current_role: &str` + `current_state: &WorkflowState` 参数，参照 §5.4 Q2 "扩展不是重构"的精神只增不改）；三处调用点传同一个元组即可。§6.6.2 / §9 步 2.4 / §12 接入点表统一改"执行点：`run_subagent` 内部统一拦截"。

### S-B · §6.6.1 注入路径假设"workflow session 必有 user instruction message"，但 fallback 分支会破坏 prompt cache

**位置**：§6.6.1（"复用 `inject_recall_into_turn` 同款 seam"）、§10.7（"注入一律 append messages[0]"）

**问题**：`memory_recall::inject_recall_into_turn`（memory_recall.rs:259-278）有两个分支：

- 有 `messages[0]` user instruction message → append block（常驻 seam，cache 安全）
- 无 → **prepend 一个新建 synthetic user message，破坏 prompt cache breakpoint**

workflow session 默认开 → CLAUDE.md 等指令文件经 B5 自动加载 → 实际有 user instruction，**走了分支 A**。但文档没说这个分支条件；实施时若有人误以为"workflow session 不一定有指令文件"（`build_instructions_blocks` 在无 CLAUDE.md 时行为如何文档未交代），fallback 分支 prepend 会破坏 cache。

**建议**：§6.6.1 加一行前置约束："本机制依赖 `messages[0]` 存在 user instruction message（workflow session 默认满足：B5 指令文件固定加载）；无指令文件时需另开注入机制（不在本文档范围）"。§10.7 加注"prepend fallback 分支会破坏 cache，本工作流不允许触发 fallback"。

---

## 2. 中等问题（应修不阻塞）

### M-A · §8 hook 触发路径中"resolve handler 区分 state-transition"的传递链路未闭合

**位置**：§8（Q9 Rust 固定 hook）、§3.1（"状态转移：复用 ask_user_question"）、§10.6

**问题**：`commands/question.rs:92` 的 `resolve_tool_question` 当前签名 `(session_id, tool_use_id, answer, cancelled)`，**无任何字段标识"这是 state-transition 申请 vs 普通询问"**；`question_store.rs::resolve`（line 387）仅 oneshot 送答案，无任何副作用。文档 §8 把"engine 在 resolve handler 里返回确认答案后主动调 `set_task_state`"当作自然延伸，但：

- 这条要求 `resolve_tool_question` **必须能区分**"答了普通 ask_user_question"vs"答了 state-transition 申请"
- 这两种在现有系统是同一种 tool，调用形态相同——形态本身无法区分

而 `resolve_mode_change`（commands/question.rs:137-176）是**新增 IPC + 在 apply mode + resolve oneshot 同一处收口**（line 140-150 设计注释明说"apply mode BEFORE resolve"），这条路径就是文档想要的范本。但文档说成"状态转移确认门复用 ask_user_question"（§3.1 第 7 项、§10.6），跟实际最干净的实现（reference `request_mode_change` 的双 IPC pattern）矛盾。

**建议**：§8 改成"状态转移确认门**新开 IPC `resolve_task_state_transition`**（对标 `resolve_mode_change`），在内部 apply `set_task_state` + resolve oneshot"。"`ask_user_question` 复用" 仅作为"state 转移不在场时，用户表达确认意图"的 fallback 备选，不作主线 hook 触发路径。

### M-B · workflow.json schema 缺 `{relevant_specs}` 占位符，让 implementer 不知读哪些 spec

**位置**：§5.3（workflow.json schema 草案）、§6.4、§A.4（wf-before-dev body）、§A.2（dev plugin 完整示例）

**问题**：§A.4 `wf-before-dev` 写"把相关 spec 引用塞进 delegation message（让 implementer 也读）"；§A.2 delegation_templates 又确实不提供该占位符（只有 `{title}`/`{summary}`/`{state}` 三占位）。结果：

- 主 LLM 派 implementer 时，delegation 模板只告诉 worker"实现 item X"
- worker 不知道项目里有哪些 spec 必读——违反"角色化，不靠自觉"原则
- 跟 §6.7 沉淀闭环（"下次 implement → wf-before-dev → 读 spec → 按规范写"）的承诺也对不上

**建议**：`delegation_templates` schema 占位符扩到 `{title}`/`{summary}`/`{state}`/`{relevant_specs}`；engine 注入时由 `wf-before-dev` skill body 给出的 spec 候选列表，按 task.summary FTS5 过滤后填（无匹配则留 "(auto-detect via wf-before-dev)"）。§5.3 schema 段同步更新。

### M-C · `coordination` 字段语义与 §4.2 回合制 A 实际需求错位

**位置**：§5.4（`coordination: String` 字段）、§4.2 回合制 A 描述、§M5 修复决定

**问题**：M5 决定里 `coordination` 字段 review 用 `"round-robin"` / `"fan-out-gather"`。但 §4.2 A 路径实质是"按轮 gather 多 reviewer 结果 → 喂回再 dispatch"，**和"round-robin"字面意思（轮流）无关**。A 路径本质需求是"dispatch 后必须 gather-reduce 再决定下一轮"，pipeline 模型表达不出来。M5 修复形式上到位但语义不贴。

**建议**：改 `coordination: "pipeline" | "synthesis_round"`（后者 = 每轮必须 gather 后再 dispatch），再加 `gather_strategy: HashMap<state, Vec<Role>>`（每 state 收集哪些角色结果）。比 `coordination: String` 更接近 A 路径真实需要，也避免 pipeline 写出"虽然 coordination=pipeline 但实际每轮还是 gather"的别扭代码。

### M-D · §A.3 implementer.md `tools: []` 与 general-purpose 角色无法在 toolset 上区分

**位置**：§A.3 implementer.md frontmatter（"tools: []   # 空数组 = 全集"）

**问题**：`subagent::filter_tools_for_subagent`：`tools: []` 表示"全集"（再减 STRUCTURALLY_DISABLED）。`researcher` 是白名单（只读 5 tool），`checker` 是白名单（只读 + shell），**唯独 implementer 是全集 = 跟 general-purpose 同 toolset，只能靠 system_prompt 区分角色**。一轮评审小问题 2 已记，但二轮看 implementer.md 改成了加注释（"engine 自动剥离 STRUCTURALLY_DISABLED"），并未真正解决 frontmatter 不可区分角色的问题。这违背 B6 引入 SubagentDef 的初衷——角色 = tools + body 不能光靠 body。

**建议**：frontmatter 给 implementer 列显式白名单（全部 builtin 共 19 个工具名），把"禁用 dispatch_subagent 等结构化禁用"对标到 STRUCTURALLY_DISABLED（不可由 frontmatter 重开启）。或者为 SubagentDef 加 `tools_negation: Vec<String>` 字段，空白名单 + 减号表达"全集去掉 X"。让"角色 = 工具集 + body"，而非"全 tool + 纯靠 prompt"。

### M-E · §6.6.1 delegation 模板"仅 dispatch 时填充" 与 §10.7 "append messages[0]" 表意不清

**位置**：§6.6.1、§10.7

**问题**：§10.7 说"per-turn 都 append messages[0] block（各自内容不同），不写持久化"；§6.6.1 delegation 模板是"dispatch 该 turn 才追加"。两段读起来像"总是常驻"，但实际是"大多数 turn 不追加，仅 dispatch 当 turn 追加"。文档应明确：

- 大多数 turn：breadcrumb + task meta 追加（per-turn）
- dispatch turn：再加 delegation 模板 block（per-turn, 当 turn 出现 dispatch_subagent tool_use）

**建议**：在 §6.6.1 段头就写"注入时机 = 当 turn 出现 dispatch_subagent tool_use 时，engine 在 run_subagent 拦截点解析 role，取模板填占位符后 append messages[0]；非 dispatch turn 不追加"。

---

## 3. 小问题 / 笔误

1. **§5.2.1 / §6.4** "task.json metadata + summary → append `messages[0]`（常驻）" 措辞不准确。`memory_recall.rs:255-257` 注释明确 "Mutates `turn_messages` (the request clone), NOT the persisted `messages`"——这是 per-turn 注入。"常驻"会跟"持久化同步"混淆。改为 "per-turn，持久化不动"。

2. **§5.4 fallback 段**：Phase 0 → 2 数据源演化路径需明示优先级：Phase 0 builtin = `default_workflow()` 常量（写死在 engine）；Phase 2 起 builtin = "项目无 .everlasting/workflow/ 时的最后兜底 `default_workflow()`"，而"项目 .everlasting/workflow/dev/workflow.json"覆盖。当前文档写法把两种 builtin 混说。

3. **§11.1 token 表** breadcrumb 估 "~400 tokens"。§A.2 中文 breadcrumb 实测 250-500 字 ≈ ~600-1200 tokens，下限偏低。建议改 "~300-700 tokens，按 §A.2 实测"。

4. **§14** 已决策项用 ✅ 标，Q8（评审流 A/B）还是 ❓ 延迟。建议加"立项 trigger：dev plugin 跑通 1 个完整 task + 沉淀 spec 起见过收益"，给延迟定条件。

5. **§5.3 schema 与 §A.2 实际 schema 占位符不一致**：§5.3 schema 段 delegation_templates 示例写法粗略，§A.2 researcher 模板里 `{summary}` 占位符在 §5.3 schema 解释段未列占位符全集。两段应明确列出占位符集合 `{title}` / `{summary}` / `{state}` / `{relevant_specs}`（见 M-B）。

6. **§2.1 表头注释** 写"未来 `.everlasting/spec/`（Q7）沉淀起来后，新 spec 进 `.everlasting/spec/`，现有引用逐步迁移"——过于乐观。§6.7 Q7 决定里已经明确"过渡期两份共存"，表头注释应改"过渡期两份共存（各自管各自职责）"。

7. **§A.3 checker.md** tools: `[read_file, grep, glob, list_dir, shell]`——shell 拿到时本身不安全，frontmatter 只列名不列"允许的命令前缀"。建议在 implementer / checker frontmatter 加可选字段 `shell_allow_prefixes: ["cargo ", "pnpm ", "pytest"]`，engine 在 §6.6.2 dispatch 校验时除 state×role 外再叠一层"shell 命令前缀"，避免 checker 实现 rm 或 sed 改源码。

---

## 4. 认可的设计点（一轮延续 + 二轮新增）

1. **§5.4 Phase 0 留 `WorkflowDef` + 4 访问函数（M1 修复到位）** —— 后续变更是扩展不是重构。
2. **§10.7 三条注入一律 append messages[0]、不写持久化** —— 跟 `memory_recall.rs:255-258` 一致，守 cache breakpoint。
3. **§6.3 Q4 不自动 inject skill body + §6.6.1 delegation 模板 engine 注入，二者区分清晰** —— 前者按需知识（B4 三层渐进披露），后者 dispatch 基础设施。
4. **§6.2 S2(c) task.json items 内嵌** —— B12 loop-local Vec → 写 task.json.items，跨 session 续 task 天然修复，coerce 逻辑保留。
5. **§8 hook 在 `set_task_state` 内 match 分支，不引入 hook runner** —— 对标 `resolve_mode_change` 在 IPC 内 apply + resolve 的现有 pattern，杠杆合适。
6. **§6.1 Q3 + S3 统一协商档** —— 全文贯彻，无 §5.2.3 / §9 / §11.1 / §12 中"硬拒"残留矛盾。
7. **§5.4 M6 validate + fallback 段** —— 显式三步（serde 失败 / validate 失败 / 字段缺失降级），覆盖原评审没覆盖的边缘 case。
8. **§11.1 M7 token 估算表** —— 给出数量级，不算完美但给了决策依据。
9. **§3.2 不做 task UI / DB 表** —— "task = 副产物"贯彻到边界。
10. **§4.3 plugin 化价值论证** —— 评审流 A/B 跟 dev 的根本差异性论证清晰，engine 不能写死。
11. **§6.4 artifact 查阅（prd 全文给 path，不放常驻）** —— 跟 `RECALL_TOKEN_BUDGET` 思维一致。
12. **§9.5 分步小步 + 验证手段 + 风险最高步标注** —— 实施落地可操作性强，小步风险分散。
13. **二轮新增认可 · §13.1 §9.5 步 2.4 / 2.5 / 2.6 可并行的标注** —— 明确点出三步都只依赖 2.3，实际工程里能省工。

---

## 5. 需要作者拍板

| # | 问题 | 选项 | 关联严重/中等问题 |
|---|---|---|---|
| 1 | **状态转移确认门 IPC 形态** | (i) 新增 `resolve_task_state_transition` 对标 `resolve_mode_change` / (ii) 在 `ask_user_question` schema 加 `purpose` 字段分支 | M-A / S-A（注 S-A 跟 M-A 是同根问题） |
| 2 | **门控拦截点位置** | (i) `run_subagent` 内部统一拦截（三处调用点都过）/ (ii) 三处调用点各放一份 gate 代码 | S-A |
| 3 | **`coordination` 字段语义重定义** | (i) 改为 `synthesis_round` 枚举 + `gather_strategy` map / (ii) 保留字符串但文档强化 A 路径描述 | M-C |
| 4 | **WorkflowDef 是否扩 `{relevant_specs}` 占位符** | (i) 扩 schema + engine 注入逻辑 / (ii) 留给主 LLM 自查 spec 目录塞 delegation | M-B |

> 评审结论：文档整体架构方向正确（engine/content 分离、Phase 0 预留接口、协商门控、Rust 固定 hook），附录 A 实施参考物料充分。前一轮 10 处修复均到位，但**二轮新增 2 个严重问题**集中在 (1) `run_subagent` 三处调用点的门控覆盖率、(2) 注入 seam 假设 user instruction 必须存在的隐含约束。4 个中等问题集中在 hook 触发链路、schema 占位符、协调模型语义、角色区分度。Phase 0 进入前建议解决 2 个严重问题 + 4 个拍板决定。
