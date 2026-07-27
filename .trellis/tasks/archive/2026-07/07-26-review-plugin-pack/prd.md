# review plugin resource pack (C3)

> 父任务：`07-26-workflow-review-plugin`（review epic）
> 依赖：**C1（resume）**。本任务是 review plugin 的「资源包」层，消费 C1 的 resume 能力；同时**定义并写入 review-state.json schema**，是 C2（可视化）的前置契约（C2 读 C3 写的 schema，见父任务执行顺序）。

## Goal

交付 review plugin 的资源包：`workflow.json` + `reviewer` 角色 + 4 个 skill + `builtin.rs` 内置化 + dev skill 衔接指引。这是 review workflow 的「血肉」，定义状态机、角色、方法；它依赖 C1（reviewer 用 resume 续接），同时**定义并写入 review-state.json**（C2 视图的数据源，见 R7）。

**产物 = prd/design 本身**（reject review.md，见父任务决策 6）：review 不产最终报告，revising 主 LLM 直接改 prd，prd 即产物，dev 直接读。

## Background（代码事实，均就绪）

- 内置化机制：`07-09-workflow-builtin-plugin` 跑通的模式（`BUILTIN_PLUGIN_NAMES` + `builtin_workflow_json()` + `BuiltinPlugin` source）。C3 照搬，加 review 组。
- 多模型：per-dispatch model override（`dispatch.rs:487`）+ `dispatch_subagent` 动态 model 枚举（`chat_loop.rs:681`）。
- task 共享：`resolve_current_task`（`inject.rs:229`）按 project 扫，review/dev session 自动共享同一 task。
- 主 LLM 工具集：workflow_enabled 时不裁剪（`tools/mod.rs:223`），主 LLM 有写工具可在 revising 改 prd。

## Requirements

### R1. workflow.json（4 state 带回环）

```jsonc
{
  "name": "review",
  "description": "多模型评审流：评审需求/计划 → 修订 → 回环重评，过程可视化，用户指挥收敛",
  "states": ["intake", "reviewing", "revising", "reported"],
  "initial": "intake",
  "transitions": [
    {"from": "intake", "to": "reviewing", "requires_user_confirm": true},
    {"from": "reviewing", "to": "revising", "requires_user_confirm": true},
    {"from": "revising", "to": "reviewing", "requires_user_confirm": true},
    {"from": "revising", "to": "reported", "requires_user_confirm": true}
  ],
  "roles_by_state": {
    "intake": [],
    "reviewing": ["reviewer"],
    "revising": [],
    "reported": []
  },
  "coordination": "synthesis_round",
  "gather_strategy": {"reviewing": ["reviewer"]}
}
```
- `intake`/`revising`/`reported` 主 LLM 自己干（空 roles，无子代理）。
- `coordination: synthesis_round` 作 prompt 提示（B2-α，引擎不强制，由 skill 引导主 LLM 并发派）。

### R2. 角色（reviewer）

- 单 `reviewer.md`：只读工具集（read/grep/glob/web_fetch），不写文件；frontmatter `model:` 留空（per-dispatch override 主导）。
- **评审范围含项目代码**（评审 DeepSeek 4.6 采纳）：reviewer 不仅读 prd，有权读项目结构和关键代码做「设计 vs 实现一致性」检查 —— 纯读 prd 的 review 价值太低。reviewer.md 明确此范围。
- 输出结构约束（写进 reviewer.md prompt）：按维度分节 + 总体结论（通过/有条件通过/打回），便于主 LLM 综合时按维度横向对比。
- **stale context 提示**（评审议题 3 采纳）：reviewer.md system prompt 加「若上轮对话引用与当前文件内容矛盾，以当前文件为准」（针对 resume 续接的旧 prd 引用问题）。
- reviewer 用 C1 的 resume 续接上轮会话（C1 落地后），省 token。

### R3. skills（4 个）

- `wf-overview`：review 全貌 + 主 LLM orchestrator 职责（intake 准备 / reviewing 派 reviewer / revising 综合修订 / reported 收尾）+ 多模型评审心智。
- `wf-review-prep`（intake）：**发现可用模型**（主 LLM 无法直接调 `list_models` 内部 API，而是从 `dispatch_subagent` 工具的 `model` 参数 enum 看到所有已配置模型的 display_name，`chat_loop.rs:681` 已构建该动态 enum）+ askUserQuestion 用户多选（多选结果作为 reviewing 派 reviewer 时 per-dispatch model override 的依据）+ 推荐评审维度（按任务种类）+ askUserQuestion 确认增删维度。
- `wf-review-method`（reviewing）：**维度推荐器**（非写死）——按任务种类（新功能/重构/Bug/架构/文档）推荐基础维度组合 + 可选维度池（安全/性能/可读性/...）+ askUserQuestion 确认。维度定下后 N 个 reviewer 过同一套（多模型同题对比）。
- `wf-synthesize`（revising）：综合方法（按维度横向对比、标注分歧、提炼共识）+ **triage 决策**（每条 finding 标 adopt/reject/defer + 理由，reject 要对照已知约束——源于外部评审实践：评审者常缺决策上下文，其"合理建议"可能撞墙于项目已知约束，主 LLM 需带 brainstorm 上下文做判断，不能照单全收）+ 修订 prd 的指引 + askUserQuestion 问用户「再评一轮还是定稿」（含 convergence 评估，主动建议定稿）+ **产出 `<task>/review-state.json`**（C2 视图数据源，见 R7）。

### R4. builtin.rs 内置化

- 新增 `app/src-tauri/resources/builtin-workflow/review/`（workflow.json + agents/reviewer.md + 4 skill）。
- `builtin.rs`：追加 `BUILTIN_REVIEW_WORKFLOW_JSON` + `BUILTIN_REVIEW_SKILLS` + `BUILTIN_REVIEW_AGENTS` 常量组；`BUILTIN_PLUGIN_NAMES` 追加 `"review"`；`builtin_workflow_json` 加 `"review"` 分支。
- 项目示例 `.everlasting/workflow/review/` 同步一份（同 dev 约定，人工同步）。

### R5. dev skill 衔接指引

- 改 dev 的 `wf-brainstorm` / `wf-overview`：加「prd 可能已被 review session 修订过，planning 注意读最新 prd」。
- （注：因砍了 review.md，指引从「读 review.md」改为「注意 prd 已被 review 修订」。）

### R6. breadcrumb

- 每个 state 的 breadcrumb 文本（参考 dev 风格）：intake（选模型+确认范围+维度）、reviewing（并发派 reviewer）、revising（综合+修订 prd+问用户循环）、reported（收尾，prd 已就绪供 dev）。

### R7. review-state.json（C2 视图数据源，跨任务契约）

- wf-synthesize 在 revising 写 `<task>/review-state.json`，结构化呈现历轮各模型评审（层次 2 决策）。
- **schema（采纳评审 MiniMax §6.1 核心字段）**：
  ```jsonc
  {
    "schema_version": "1.0",
    "task_id": "<task slug>",
    "current_round": 2,
    "rounds": [
      {
        "round": 1,
        "dimensions": ["清晰度", "范围边界", "可行性"],  // per-round（采纳 MiniMax 4.8）
        "models_present": ["model-id-a", "model-id-b"],   // 本轮参与模型（采纳 DeepSeek 4.4）
        "models": {
          "<model_id 稳定 id>": {                          // key 用 model_id 非 display_name（采纳 MiniMax 4.12）
            "model_display": "claude-sonnet-4",
            "run_id": "<subagent_runs.id>",
            "status": "completed",                        // running/completed/cancelled/error/incomplete（对齐 DB subagent_runs.status CHECK 约束，评审回流）
            "verdict": "revise",                          // pass/pass_with_minor/revise/reject
            "findings": [
              {
                "finding_id": "r1-m-modela-1",            // 稳定 id（采纳 MiniMax 6.1）
                "dimension": "清晰度",
                "severity": "high",                       // critical/high/medium/low/info
                "issue": "...",
                "suggestion": "...",
                "location": "prd.md§2",
                "source_run_id": "<subagent_runs.id>",     // 跳转原始 final_text（采纳议题2）
                "triage": {                                // 主 LLM 的 triage 决策（源于外部评审实践）
                  "decision": "adopt",                     // adopt/reject/defer
                  "reason": "命中范围边界缺失,需补"
                }
              }
            ]
          }
        },
        "change_log": ["§2 澄清范围边界（reviewer A 反馈）", "§4 补错误处理（reviewer B）"],  // 本轮修订摘要（采纳 DeepSeek 决策6）
        "convergence_note": "本轮无新增关键问题，建议定稿"  // 收敛信号（采纳议题4软引导）
      }
    ]
  }
  ```
- 每轮 revising 由主 LLM 重写整个文件，`rounds` 数组累积历轮（不丢失历史）。
- **写文件原子化**（采纳遗漏点4）：tmp + rename，防前端撕裂读。
- 这是 C2 视图的数据源 —— C3 是写入方，C2 是读取方，schema 是跨任务契约，design 阶段两边对齐。

## Acceptance Criteria

- [ ] review plugin 在 PluginSelect 可见可切，breadcrumb 显示 review 流程。
- [ ] intake：主 LLM 枚举模型 + askUserQuestion 多选 + 推荐维度并确认。
- [ ] reviewing：主 LLM 并发派多模型 reviewer（resume 续接，依赖 C1），产出带 `[model: xxx]` 标签。
- [ ] reviewing：C2 矩阵视图能基于 review-state.json 渲染（依赖 C2 实现，但本任务保证 schema 契约）。
- [ ] revising：主 LLM 综合 + 修订 prd（有写工具）+ 写 `<task>/review-state.json`（C2 数据源）+ askUserQuestion 问用户循环。
- [ ] 回环：用户「再评」回 reviewing（reviewer resume 续接），「定稿」进 reported。
- [ ] reported：prd 已修订就绪，dev session 读同一 prd（task 共享）。
- [ ] `builtin.rs` 单测：`BUILTIN_REVIEW_WORKFLOW_JSON` 过 `validate()`；skills/agents body 非空且 frontmatter 含 `name`。
- [ ] 回归：dev plugin + 内置化机制 + resume（C1）+ 可视化（C2）全绿；`cargo test --lib` + `cargo clippy --lib --tests` 通过。

## Open Questions（design.md 需解决）

1. reviewer.md 的输出结构细则（维度小节格式、评分粒度）——注意：reviewer 输出是自由 markdown，主 LLM 在 revising 提炼成结构化数据，所以 reviewer 不必严格遵守，但要有足够信息让主 LLM 提炼。
2. review-state.json schema 细则（与 C2 对齐）：severity 枚举值、location 格式、dimensions 如何随用户增删演化。
3. reviewer resume 续接时，主 LLM 注入的「现状/变化/目的」澄清文本模板（依赖 C1 的 resume API 形态）。

## Notes

- 依赖 C1（reviewer resume 续接形态）。C1 定稿前本任务的 R2/R7 部分细节无法最终确定。
- **不依赖 C2**：相反，C2 依赖本任务的 review-state.json schema（R7）。执行顺序 C1 → C3 → C2。
- review-state.json 是 C3 写、C2 读的跨任务契约（R7），design 阶段对齐 schema。
- 产物是 prd/design 本身，不产 review.md（父任务决策 6）。
