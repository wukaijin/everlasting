# WORKFLOW-INTEGRATION 设计评审

> 评审对象: `docs/WORKFLOW-INTEGRATION.md`（2026-07-07 版本）
> 评审日期: 2026-07-07
> 评审方法: 逐节交叉验证 + 对照现有代码（`agent/subagent/dispatch.rs`、`agent/chat_loop.rs`、`agent/subagent/mod.rs`、`tools/update_checklist.rs`、`tools/ask_user_question.rs`、`memory/loader.rs`、`memory/memory_recall.rs`、`agent/question_store.rs`）
>
> 评审维度（优先级降序）:
> 1. 内部一致性 — Q1-Q9 决定在全文是否贯彻
> 2. 架构合理性 — engine/content 分离边界、WorkflowDef 接口形态
> 3. 可行性陷阱 — 对照现有 B6 / B12 / 注入 seam / 无 hook runner
> 4. 缺失与风险 — 错误处理、token 预算、Phase 间隐含依赖

---

## 1. 严重问题（阻塞实施，建议解决后再进 Phase 0）

### S1 · delegation 模板注入 agent 上下文的机制完全缺失

| 项 | 内容 |
|---|---|
| **位置** | §6.6（worker 上下文注入）、§5.4（WorkflowDef 接口）、§5.2（engine 能力）、§12（接入点） |
| **关联决定** | Q6 |

**问题**: 文档说"engine 把 task meta 占位符填进模板 → 主 LLM 读模板后写委托语"。但**从未指定模板文本如何到达 agent 上下文**。

现有代码仅有两条注入 seam:

| Seam | 频率 | 用途 |
|---|---|---|
| `build_instructions_blocks` → `messages[0]` | 每 session 一次 | 指令文件（cache breakpoint） |
| `inject_recall_into_turn` → `messages[0]` block 数组末尾 | 每 turn | memory recall（`cache_control: None`） |

delegation 模板是**角色相关的**（researcher / implementer / checker 各一个），agent 在 dispatch 某个角色时需要用到对应模板。但:
- breadcrumb 只含整体流程提示，不含逐角色 delegation 模板正文
- 没有第三条注入 seam 描述 template 如何进入 per-turn context
- §12 接入点表写"delegation 模板,engine 填 task meta 占位符 + 主 LLM 填委托细节"——**填完后放在哪？没说**

**建议**: 在 §6.6 加一小节"模板注入机制"，明确选一种方案:

| 方案 | 描述 | 改动 |
|---|---|---|
| **(a) 注入到 messages[0]** | engine 填好占位符 → 把文本 append 到 per-turn `messages[0]` 的 block 数组（复用 `inject_recall_into_turn` 模式，`cache_control: None`） | 1 条新 append 调用 |
| **(b) use_skill 级别** | 把每个 delegation 模板注册为可 `use_skill` 的资源，agent dispatch 前自行加载 | 需 skill loader 支持 workflow plugin 路径 |

**推荐 (a)** — 改动最小，且 delegation 模板本就是 engine 应注入的上下文，不应靠 agent 自觉加载。

---

### S2 · B12 loop-local checklist 与 workflow 需要的文件态 `checklist.md` 完全割裂

| 项 | 内容 |
|---|---|
| **位置** | §6.2（task 文件态记账，checklist.md 是文件）、§6.4（artifact 查阅）、§9 Phase 2（"checklist.md 推进:B12 升格读 checklist.md"） |
| **关联代码** | `tools/update_checklist.rs`、`chat_loop.rs:530` |

**问题**: 设计中最大的可行性断裂:

| 设计假设 | 现有代码事实 |
|---|---|
| agent 在 planning 写 `checklist.md`（文件） | B12 `update_checklist` 维护 `Arc<Mutex<Vec<ChecklistItem>>>`，**每 request 重建**，从不写文件 |
| implement 状态"按 checklist.md 逐项推进" | B12 注入的是 loop-local Vec，session 重开就消失 |
| Phase 2 "B12 升格读 checklist.md" | B12 的 `update_checklist` tool 不读文件，只做 Vec 原子替换 |

**两个场景都断裂**:

1. **跨 session 续 task**: session 重开后, `update_checklist` 重建空 Vec —— agent 看不到上次的 checklist 进度
2. **implementer worker 看 checklist**: worker 的 `STRUCTURALLY_DISABLED` 包含 `update_checklist`（`subagent::tools_filter::STRUCTURALLY_DISABLED`），worker 根本拿不到 checklist 状态——除非靠 `read_file` 读 `checklist.md`，但 implement 期间的 item 状态变更（done / in_progress）不同步回文件

**建议**（三选一，需要作者拍板）:

| 方案 | 描述 | 优点 | 缺点 |
|---|---|---|---|
| **(a) B12 写透文件** | `update_checklist` 每次替换 Vec 时同步写 `checklist.md`（markdown checklist 格式） | B12 的"仅一个 in_progress"约束保留 | 需定义文件格式 + 写文件逻辑 |
| **(b) 放弃 B12，纯文件** | workflow session 内 agent 用 `read_file`/`write_file` 操作 checklist.md，B12 禁用 | 最简单 | 丢弃 B12 的结构化约束 |
| **(c) task.json 内嵌 items** | task.json 加 `items: [{title, status}]`，`update_checklist` 改写 task.json | 最干净，单一数据源 | task.json schema 膨胀 |

---

### S3 · "软→硬" 与 Q3 "协商档" 在全文中多处矛盾

| 项 | 内容 |
|---|---|
| **位置** | §5.2 第 3 点、§9 Phase 2 完成任务列表、§11.1 风险表第一行、§12 接入点"state 门控 dispatch"行 |
| **关联决定** | Q3 |

**问题**: Q3（§6.1）明确决定门控违反走**协商**（`ask_user_question` 申请切 task state），不是硬拒。但以下 4 处仍写"硬校验"或"软→硬":

| 位置 | 原文 | 冲突 |
|---|---|---|
| §5.2.3 | `校验(软→硬,follow-up)` | Q3 定了协商档，不是硬 |
| §9 Phase 2 | `state 门控 dispatch(软→硬)` | 同上 |
| §11.1 风险表 | `实效不足加硬校验(Phase 2)` | Q3 的 Phase 2 方案是协商不是硬校验 |
| §12 接入点 | `软(breadcrumb)先行;硬校验 follow-up` | 同上 |

如果意图是"角色门控用硬校验 + state 转移用协商"这种**两层门控**，文档没有在任何地方明确区分这两种门控。Q3 的例子"task 在 planning, agent 想写代码 → 弹窗确认进 implement"——agent 想 dispatch implementer（角色不允许）→ 弹窗协商 state 转移——这其实是一个机制同时处理了角色违规和 state 转移。

**建议**: 需要作者明确选一种，然后全文统一:

| 选项 | 描述 | 影响 |
|---|---|---|
| **两层门控** | 角色门控 = 硬拒（tool_error），state 转移 = 协商（ask_user_question） | 在 §6.1 加一段明确区分，保留 §5.2/§9/§12 的"软→硬"用于角色门控 |
| **统一协商** | 所有门控违规走协商（Q3 原意） | §5.2/§9/§11.1/§12 全部改为"协商档" |

---

## 2. 中等问题（应修不阻塞）

### M1 · WorkflowDef struct 缺少 `delegation_templates` 字段

**位置**: §5.4 的 Rust struct 定义

**问题**: struct 有 `name, description, states, initial, transitions, roles_by_state, breadcrumb`——缺 `delegation_templates: HashMap<String, String>`。§5.4 宣称"engine 全程只认 WorkflowDef + 这三个访问函数"，但 delegation templates 在 §6.6 和 §A.2 中是 workflow.json 的核心字段。

**建议**: WorkflowDef 加 `delegation_templates` 字段，三条访问函数扩展为四条:
```rust
fn delegation_template_for(def: &WorkflowDef, role: &str) -> Option<&str>;
```
§5.4 更新"三个访问函数"→"四个访问函数"。

---

### M2 · hook 触发路径: 谁调用 `set_task_state`？

**位置**: §8（Q9 Rust 固定 hook）、§6.7（沉淀闭环）

**问题**: Q9 决定 hook 嵌在 `set_task_state` 里，但**没有指定谁调用 `set_task_state`**:

| 调用者 | 可靠性 | 风险 |
|---|---|---|
| agent 通过 tool 调用 | agent 可能忘记调用 → 沉淀闭环断 | 违背 Q9"不靠 agent 自觉"的初衷 |
| user 确认门自动调 | 可靠，不依赖 agent | 需 engine 在确认门 resolve 后主动写 task.json |

当前 `ask_user_question` 是**纯问答**——resolve 后只返回 answer JSON，不触发任何副作用（`tools/ask_user_question.rs`、`question_store.rs::resolve`）。

**建议**: 选方案 2（user 确认门自动调）。在 Phase 0 实现时，`ask_user_question` 的 resolve handler 在返回确认答案后调用 `set_task_state(task, new_state)`，触发 hook。在 §8 补充调用链描述。

---

### M3 · 门控执行点（dispatch 拦截点）未定义

**位置**: §5.2.3、§6.1 Q3

**问题**: 文档描述了门控的**策略**（breadcrumb / 协商 / 硬拒），但未描述**执行点**。

现有 `dispatch_subagent` 的拦截路径: `chat_loop.rs:3283`（`if name == "dispatch_subagent"` → `run_subagent(...)`），中间只有 `permissions::check`（5-tier 权限），无 role/state gate。

两个候选执行点:
- **(A)** tool 参数解析后、`run_subagent` 调用前（`chat_loop.rs:3283` 附近）——不需要改 `run_subagent` 签名
- **(B)** `run_subagent` 内部（`dispatch.rs:335` 附近）——需要传入当前 state 参数

**建议**: 选 **(A)**。在 §5.2.3 补充:"门控执行点在 `chat_loop.rs` 的 `dispatch_subagent` 拦截处（`run_subagent` 调用前），检查 `roles_by_state[当前 state]` 是否包含目标 role。不允许时触发协商流程。"

---

### M4 · wf-* skill 存放位置与 Phase 1/2 顺序矛盾

**位置**: §6.3、§12、§9 Phase 1

**问题**: 鸡生蛋蛋生鸡:

| 阶段 | wf-* skill 应该放哪 | 现实 |
|---|---|---|
| Phase 1（skill 规范包） | `.everlasting/workflow/dev/skills/`（plugin 目录） | Phase 2 才做 plugin 外置，Phase 1 时此目录和 skill loader 的 plugin 解析层都不存在 |
| Phase 2（plugin 外置） | 同上 | 目录有了，但 Phase 1 的 skills 需要已经可用 |

**建议**（三选一，需要作者拍板）:

| 方案 | 描述 |
|---|---|
| **(a)** Phase 1 暂放全局 `.everlasting/skills/`，Phase 2 迁移（加迁移 note） |
| **(b)** Phase 1 就把 plugin skills 加载路径做出来（`skill/loader.rs` 加 workflow plugin 解析层），即使 workflow.json 还是硬编码 |
| **(c)** 合并 Phase 1 + Phase 2（一起做） |

---

### M5 · review 流协调模型字段在 WorkflowDef 中无位置

**位置**: §4.3、§5.4 影响分析表

**问题**: 文档说"引擎把任何一个写死，另一个就塞不进去"（§4.3）。但当前 `WorkflowDef` 只有 pipeline 隐式协调模型（`roles_by_state` 表达"谁在哪个 state 被允许"）。review 流的多轮 fan-out/gather 无法用 `roles_by_state` 表达。

§5.4 影响分析表承认:"评审流加入(实时群聊 B) | 加协调模型字段 + engine 新能力 | ⚠️ 仅此情况需 engine 扩展"。

**建议**: 在 WorkflowDef 中预留 `coordination: String`（默认 `"pipeline"`，review 用 `"round-robin"` 或 `"fan-out-gather"`）。当前 engine 只认 `"pipeline"`，未来加其他模式的分发逻辑。这样 review 加入时 **WorkflowDef 接口不用改**（只加 engine 内部的分发分支），真正"扩展不是重构"。

---

### M6 · 空/损坏 workflow.json 的回退策略未定义

**位置**: §5.4 "fallback 策略"段

**问题**: 只说了"项目无 `.everlasting/workflow/` 时用默认"——没覆盖:
- workflow.json 存在但 JSON 解析失败（malformed）
- workflow.json 存在但缺必填字段（如 `states` 为空数组）
- `roles_by_state` 引用了未定义的 state

**建议**: 加验证 + fallback 策略:
```
1. 读 workflow.json → serde_json 解析失败 → log warn → 回退 default_workflow()
2. 解析成功 → validate(states 非空, initial ∈ states, transitions 引用已定义 states,
   roles_by_state keys ⊆ states) → 失败则 log warn + 回退 default
3. delegation_templates / breadcrumb 键缺失 → 对应角色/state 用空字符串（warn），不阻塞加载
```

---

### M7 · token 预算风险无具体估算

**位置**: §11.1 风险表第 5 行

**问题**: 标记为风险但无数字。看一眼 §A.2 的 breadcrumb 长度——每个 state 约 200-400 中文字符（~300-600 tokens）。加上 task.json metadata（~50 tokens）、memory recall（已有 `RECALL_TOKEN_BUDGET` 约束）、checklist（可变）。单 turn 额外注入可达 **500-1000+ tokens**。

**建议**: 在 §11.1 加估算表:
```
| 注入项 | per-turn 估算 | cache 命中时成本 |
|---|---|---|
| breadcrumb (state) | ~400 tokens | 0 (append, cache_control: None) |
| task.json metadata | ~50 tokens | 0 (在 messages[0] block append) |
| delegation template (dispatch 时) | ~200 tokens | 0 (同上) |
| memory recall | ≤RECALL_TOKEN_BUDGET | 0 (已有预算约束) |
```

---

## 3. 小问题 / 笔误

1. **§5.4** — `Transition` struct 未定义。`transitions` 声明为 `Vec<Transition>`，注释说 `{from, to, requires_user_confirm}`，但 Transition 本身没有出现在文档中。建议补单行 `struct Transition { from: String, to: String, requires_user_confirm: bool }`。

2. **§A.3 implementer.md** — `tools: []` 语义含糊。`[]` 在现有 `general-purpose` 中表示"全集减 STRUCTURALLY_DISABLED"。implementer 和 general-purpose 的 `tools: []` 效果完全相同——仅靠 system_prompt 区分角色。建议 implementer 显式列出工具白名单，或在 body 加注"tools 全集（engine 自动剥离 dispatch_subagent 等）"。

3. **§A.4 wf-overview + §6.3 skill 表** — "去平台 hook 依赖" 措辞矛盾。§6.3 说 wf-* skills "借鉴 Trellis,去平台 hook 依赖,纯描述性"，但 §A.4 body 写"(done 的沉淀由 Rust 固定 hook 触发,保证不漏)"——这**依赖 Rust hook**，不是"去 hook 依赖"。建议改为"去 Trellis Python hook 依赖，换成 Everlasting Rust hook 触发"。

4. **§4.1 流程图** — "agent 自动起 task" 触发条件不明确。是 workflow session 的第一个 user message 就起？还是 agent 判断需要 task 时才起？§6.2 说"读目录找 status≠done 的自然续上"，但**首次起 task 的触发点**未定义。建议在 §6.1 或 §6.2 加触发规则。

5. **§5.4 影响分析表** — "评审流加入(回合制 A)…零改动" 前提未决。Q8 未决议走 A 还是 B。建议改为"若选回合制 A 则零改动;若选 B 则需 engine 扩展（Q8 待决）"。

6. **§12 接入点** — "task artifact 读写…零改动" 过于乐观。`write_file` 写 `task.json` 可能产出损坏的 JSON（agent 手写）。建议至少加 `task_json_validate` 或提供专用 tool（只允许改特定字段），而非裸 `write_file`。

7. **§A.3 checker.md** — `shell` tool 的安全面需要明确。checker tools 包含 `shell`（可执行任意命令），但能力描述说"只读 + 可跑测试"。建议在 body 加"只用 shell 跑 lint/test 命令，不修改源码文件"的硬约束。

8. **§2.1 表** — 引用了 `.trellis/spec/` 路径（如 `.trellis/spec/backend/agent-loop-architecture.md`）。如果 `.trellis/spec/` 未来和 `.everlasting/spec/` 分家，这些引用可能失效。建议加注"当前引用基于过渡期两目录共存"。

---

## 4. 认可的设计点

以下设计点经过评审确认合理，值得保留（避免后续修改时误伤）:

1. **Engine/content 分离边界清晰** — Rust 只做机制（加载/注入/门控/转移/IO），内容完全文件态，plugin 可移植
2. **Phase 0 预留 WorkflowDef + 访问函数接口** — 把"后续改动是扩展不是重构"落到实处
3. **主角是机制，不是 task** — 用户不操作 task，task 是 agent 的记账副产物，避免了悬空的 task UI 抽象
4. **session 不绑 task，task 纯文件态跨 session** — 靠目录结构天然交接，零 DB 成本
5. **workflow 与 Mode 正交** — 两个独立旋钮，不互相干扰
6. **实施阶段不进 state 枚举** — LLM 拆 checklist，避免 state 爆炸
7. **append 到 messages[0] 保护 cache breakpoint** — 正确复用已验证的硬规则
8. **Q3 协商档（非硬拒）** — agent autonomy 和 user control 之间有合理折中
9. **Q6 delegation 模板三层分工** — plugin 给框架 + task meta 给上下文 + 主 LLM 填细节，清晰实用
10. **Q9 Rust 固定 hook 而非脚本 runner** — 避免安全面和过度工程化
11. **附录 A 完整实施参考物料** — 降低 Phase 0-1 启动成本
12. **借鉴 Trellis 的可定制性但不照搬** — 有自己的判断（workflow vs task 中心化、JSON vs markdown 配置）

---

## 5. 需要作者拍板的决定

以下问题标注了 ❓ 或存在隐含歧义，需要作者明确选择:

| # | 问题 | 选项 | 关联严重问题 |
|---|---|---|---|
| 1 | **B12 ↔ checklist.md 同步方案** | (a) B12 写透文件 / (b) 放弃 B12 走纯文件 / (c) task.json 内嵌 items | S2 |
| 2 | **门控分层** | 角色门控硬拒 + state 转移协商，还是所有违规统一走协商？ | S3 |
| 3 | **hook 触发路径** | `set_task_state` 由 agent tool 调用，还是 user 确认门自动调？ | M2 |
| 4 | **Phase 1 wf-* skill 存放** | (a) 暂放全局等 Phase 2 迁移 / (b) Phase 1 直接做 plugin skill loader / (c) 合并 Phase 1+2 | M4 |

---

> 评审结论: 文档整体架构方向正确（engine/content 分离、Phase 0 预留接口、协商门控、Rust 固定 hook），附录 A 的实施参考物料充分。3 个严重问题集中在**注入机制未指定**（S1）、**B12↔checklist.md 割裂**（S2）、**门控表述不一致**（S3），均可在进入 Phase 0 前修正。4 个待拍板决定建议在修改文档时一并解决。
