# workflow review plugin (epic — 父任务)

## Goal

为 everlasting 的 workflow 系统交付第二个内置 plugin —— `review`，提供**多模型评审流**：多个不同模型各自独立评审同一份需求/计划，revising 修订后回环重评，**过程中以专属可视化呈现每轮各模型发现**，由用户指挥主 LLM 是否继续收敛。与 `dev`（单模型实施流）互补。

**核心价值**：多模型评审 + 过程可视化 + 人工指挥收敛。review 不产最终报告（自产自销无意义），产物就是修订后的 prd/design 本身（dev 直接读）。

## 任务拆分（父任务 + 4 个子任务）

本 epic 拆为 4 个子任务，**有依赖顺序**（依赖写在各子任务 prd）：

| 子任务 | 内容 | 依赖 |
|---|---|---|
| **C0. TaskStatus 容纳自定义 state** | 引擎层：TaskStatus enum 加 Custom(String)，让 plugin 能定义自己的 state（review 的 intake/reviewing/revising/reported） | 无（架构级前置，最先做） |
| **C1. subagent resume** | 引擎层：subagent 续接机制（续上轮 worker 会话，省 token） | 无（独立基建，可与 C0 并行） |
| **C3. review plugin 资源包** | workflow.json + reviewer 角色 + 4 skill + builtin.rs 内置化 + dev skill 衔接指引；**定义并写入 review-state.json schema** | 依赖 **C0**（review 4 state 要能存）+ C1（reviewer resume） |
| **C2. review 可视化** | 前端：review 专属的「轮次 × 模型」矩阵视图，**读 review-state.json 渲染** | 依赖 **C0** + C1 + **C3 的 review-state.json schema**（schema 设计须先于 C2 实现） |

**执行顺序**：C0 + C1（可并行）→ C3（含 schema 设计）→ C2。C0 是 C3/C2 的硬前置（review 的 4 state 否则存不进 task.json.status，见 research/taskstatus-custom-state.md）。

**父任务职责**：拥有本需求集 + 跨子任务验收 + 最终集成验证（C0+C1+C2+C3 合起来跑通完整 review 流）。父任务本身不直接实施。

## Background（跨子任务共享的代码事实）

### 多模型基础设施（C3 用，均已就绪）

| 能力 | 位置 |
|---|---|
| subagent per-dispatch model override | `dispatch.rs:487/687` |
| `dispatch_subagent` 动态 model 枚举 | `chat_loop.rs:681` |
| 并发 dispatch（`DispatchBatch::Concurrent`） | `chat_loop.rs:4304` |
| task 跨 session/plugin 共享（`resolve_current_task`） | `inject.rs:229` |

### subagent 可视化基础设施（C2 用，部分就绪）

- ✅ `subagent_runs` 表已有 `final_text` + `model_display` + `transcript_json` + `status`（`db/migrations.rs:644`、`db/subagent_runs.rs:186/226`）。
- ✅ 前端 `SubagentDrawer` 已能展示每个 run（`app/src/components/chat/SubagentDrawer.vue`）。
- ❌ 缺：review 专属的「轮次 × 模型」矩阵对比视图（SubagentDrawer 是通用列表，不分组）。

### resume 机制（C1 要建的）

- ❌ **不存在**：`build_worker_messages`（定义在 `agent/subagent/mod.rs:642`，dispatch.rs:645 是调用点）每次 dispatch 全新构造 messages，无续接路径。
- C1 scope：持久化 worker messages + dispatch 续接 + worktree/隔离复用 + 边界处理。是独立基建，dev 的 implement ↔ check 循环也受益。
- ⚠️ **dispatch 路径零消费 `coordination` 字段**：`Coordination::SynthesisRound`（`def.rs:130`）类型层就绪，但 dispatch 只读 `roles_by_state`（`dispatch.rs:1632/1653` 的 `allowed_roles` 调用），不读 `coordination`。Pipeline 和 SynthesisRound 行为相同。注：这里说的「零消费」特指 coordination 字段，roles_by_state 是被消费的。

### SynthesisRound 纸面功能（C3 用 prompt 引导绕过）

- `Coordination::SynthesisRound` + `gather_strategy` 类型层就绪，dispatch 路径零消费（`dispatch.rs:1632/1653`）。
- C3 用 prompt 引导（B2-α）：review workflow.json 声明 `coordination: synthesis_round` 作提示，并行派 + 综合由主 LLM 在 skill 引导完成，不动引擎。引擎强制编排列 Phase 2。

## Decisions（brainstorm 收敛记录，跨子任务生效）

1. **review 形态**：独立前置 session 的评审 plugin，与 dev 平级。dev 前开 review session 评审需求/计划，通过后开 dev session 实施。
2. **SynthesisRound**：prompt 引导（B2-α），零引擎改动；强制编排列 Phase 2。
3. **状态机（4 state 带回环）**：`intake → reviewing → revising → reported`，`reviewing ↔ revising` 可回环（用户确认驱动）。引擎合法性已验证（validate `def.rs:548` 不禁回边、空 roles 合法）。
4. **reviewer 角色**：单 `reviewer.md`，per-dispatch model override 实现多模型；frontmatter `model:` 留空；工具集只读；输出按维度分节 + 总体结论。
5. **主 LLM 角色**：orchestrator + synthesizer + 修订者。intake 编排准备，reviewing 并发派 reviewer，revising 综合 + 修订 prd/design（主 LLM 有写工具）+ askUserQuestion 问用户继续与否，reported 收尾。
6. **产物 = prd/design 本身**（reject review.md）：review 不产最终报告（自产自销无意义），revising 直接改 prd，prd 即产物，dev 直接读。砍掉 review.md 设计。
7. **不强制 commit 版本追踪**：prd 改动是否 commit 交给用户工程习惯，review plugin 不强制。
8. **resume 硬前置**（C1）：循环里 reviewer 续接上轮会话省 token，是 review 的硬依赖。resume 是独立基建（dev 循环也受益），拆为子任务 C1 先做。
9. **专属可视化**（C2）：review 过程中按「轮次 × 模型」矩阵呈现各 reviewer 发现，用户据此指挥主 LLM。复用 subagent_runs 数据，新建前端组件。
10. **dev skill 衔接指引**：本 epic 一并改 dev 的 wf-brainstorm/wf-overview 加「prd 可能已经被 review 修订过，planning 注意读最新版」（注意：因砍了 review.md，指引从「读 review.md」改为「注意 prd 已被 review 修订」）。
11. **可视化数据基础 = 层次 2**（prompt 约束 + 主 LLM 提炼）：现有 subagent 无强格式约束机制（`assemble_subagent_prompt` 仅原样传 system prompt），reviewer 输出是自由 markdown 不可直接机器解析。故 C2 视图**不读 reviewer 原始 final_text**，而是读**主 LLM 在 revising 提炼的结构化数据** `<task>/review-state.json`（C3 的 wf-synthesize 写入，C2 读取，schema 跨任务契约）。主 LLM 做最难的「自然语言→结构化对比」转换，前端只渲染。排除层次 1（纯软约束做不到对比）和层次 3（强制 JSON 引擎改动过度，另立 task）。

## Acceptance Criteria（父任务 — 跨子任务集成）

- [ ] C1 落地：subagent resume 机制可用（含单测 + dev 循环回归不破坏）。
- [ ] C2 落地：review 专属视图能按轮次 × 模型呈现各 reviewer 发现（含前端测试）。
- [ ] C3 落地：review plugin 在 PluginSelect 可见可切；intake/reviewing/revising/reported 四 state 跑通；reviewer 用 resume 续接 + 多模型；builtin.rs 内置化完成。
- [ ] **集成**：完整 review 流跑通——intake 选模型 → reviewing 多模型评审（reviewing/revising/reported 三 state 视图可见各模型发现，见 C2 R5）→ revising 主 LLM 修订 prd + 写 review-state.json + askUserQuestion → 用户选「再评」回 reviewing（reviewer resume 续接）→ 收敛后 reported。
- [ ] 回归：dev plugin + 现有 subagent 机制（含 resume 改动）全绿；`cargo test --lib` + `cargo clippy --lib --tests` 通过。

## Out of Scope（整个 epic）

- SynthesisRound 引擎强制编排（Rust fan-out/gather）—— Phase 2。
- review 的第 2 用途（dev 之后查缺补漏）—— Phase 2。
- **跨 session / 人机混合评审**：review plugin MVP 覆盖的是「AI 内部多模型评审」（主 LLM 派多模型 subagent）；不覆盖评审者来自另一个 session、外部工具、或人（如把 PRD 贴给外部 LLM 评审再回收意见）。这轮 epic 设计本身就经历了人机混合评审（PRD 交给 MiniMax/DeepSeek 外部评审），验证了该场景真实存在，但 MVP 聚焦 AI 内部闭环，混合评审列 Phase 2。
- `.trellis/scripts/common/task_context.py` 语法错误 —— trellis 上游 bug（`chore: update trellis` a9b0812 引入），brainstorm 期间已临时修复并 commit（`fix/trellis-task-context-fstring` 分支，commit a0afeb1），非本 epic 交付物。

## Open Questions

（brainstorm 已收敛，无遗留 —— 任务拆分见上方「任务拆分」表，各子任务 prd 承载细节）
