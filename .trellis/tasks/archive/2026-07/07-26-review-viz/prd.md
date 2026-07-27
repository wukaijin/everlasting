# review visualization view (C2)

> 父任务：`07-26-workflow-review-plugin`（review epic）
> 依赖：**C1（subagent resume）** + **C3 的 review-state.json schema**（schema 设计须先于本任务实现，见父任务执行顺序）。

## Goal

为 review workflow 新建**专属可视化视图**：在 `reviewing ↔ revising` 循环过程中，按「**轮次 × 模型**」矩阵直观呈现每轮各 reviewer（不同模型）发现的问题，让用户据此指挥主 LLM 是否继续修订和重评。

**动机**：review 的价值在过程（每轮多模型发现 + 收敛），不在最终报告。用户要在过程中实时看到「第 1 轮：模型A发现X/模型B发现Y；第 2 轮：...」的对比，才能判断修订是否有效、是否继续。通用 SubagentDrawer 是扁平列表，不满足「按轮次分组 + 跨模型对比」。

**数据基础（层次 2 决策）**：reviewer 输出是自由 markdown（现有 subagent 无强格式约束机制，`assemble_subagent_prompt` 仅原样传 system prompt），不可直接机器解析。因此视图**不读 reviewer 原始 final_text**，而是读**主 LLM 在 revising 提炼的结构化数据**（`<task>/review-state.json`）。主 LLM 做最难的「自然语言→结构化对比」转换，前端只渲染。

## Background（代码事实）

- ✅ `subagent_runs` 表已有 `final_text`（reviewer 完整产出）+ `model_display`（模型名）+ `transcript_json` + `status` + `parent_session_id` + `started_at`（`db/migrations.rs:644/598`、`db/subagent_runs.rs:186/226`）。
- ✅ 前端 `SubagentDrawer.vue` 已展示单个 run（`app/src/components/chat/SubagentDrawer.vue`）。
- ✅ `useSubagentRunsStore` 已存在（`SubagentDrawer.vue:61`）。
- ⚠️ 缺：review 专属的轮次 × 模型矩阵视图；缺：把多个 reviewer run 归属到「同一轮」的字段（需 C1 的 resume 句柄或新增轮次标识）。

## Requirements

### R1. 数据源：review-state.json（主 LLM 提炼的结构化数据）

- 视图数据源 = `<task>/review-state.json`，由主 LLM 在 revising 写入（C3 的 wf-synthesize skill 职责）。
- 结构化数据形态（design.md 定细则）：
  ```jsonc
  {
    "rounds": [
      {
        "round": 1,
        "models": {
          "claude-sonnet-4": {
            "verdict": "revise",           // pass / revise / reject
            "findings": [
              {"dimension": "清晰度", "severity": "high", "issue": "...", "location": "prd.md§2"}
            ]
          },
          "gpt-4o": { "verdict": "...", "findings": [...] }
        }
      }
    ],
    "dimensions": ["清晰度", "范围边界", "可行性", ...]  // 本轮确认的评审维度
  }
  ```
- 每轮 revising 由主 LLM 重写整个 review-state.json，`rounds` 数组累积历轮（不丢失历史）；revising 完成即文件就绪，视图可读。
- 前端轮询/监听该文件变化（参考现有 subagent_runs 的前端刷新机制）。

### R2. 轮次归属

- 轮次由 review-state.json 的 `rounds[].round` 显式标注（主 LLM 写入时确定），不依赖 C1 resume 链推断。
- 这降低了 R1 对 C1 的耦合：轮次是数据层标识，与 resume 实现形态解耦。

### R3. 矩阵视图（前端新组件）

- 视图形态：
  - **主视图**：行 = 轮次，列 = 模型（按所有出现过的模型并集渲染，缺席 cell 显「未参与」，采纳 DeepSeek 4.4），单元格 = verdict + findings 数（点击展开 findings 列表）。失败模型列灰显 + tooltip「此模型本轮未完成」（采纳议题4）。
  - **维度对比视图**（核心价值）：选定一个维度（如「清晰度」），横向呈现各模型在该维度的发现 + severity，直观对比分歧。
- **finding 可跳转原始 final_text**（采纳议题2 安全网）：每条 finding 带 `source_run_id`，点击跳转该 reviewer 的原始 final_text（subagent_runs 已持久化），让用户对比「主 LLM 提炼的」vs「reviewer 原话」。
- **triage 决策可视化**（源于外部评审实践）：每条 finding 显示主 LLM 的 adopt/reject/defer + reason，让用户一眼看到「哪些被采纳、哪些被拒及为什么」——收敛过程可追溯，而非黑箱。
- 实时更新：review-state.json 每轮重写后（rounds 累积），视图增量呈现新一轮行。
- **解析降级**（采纳遗漏点4）：前端读 review-state.json 必须 try-catch，解析失败（坏 JSON / 字段缺失）进入错误态「review 数据暂不可用，请查看原始 reviewer 输出」+ 链接 SubagentDrawer，不白屏。

### R4. 指挥交互

- 视图旁引导用户回到 chat 用 askUserQuestion 指挥（revising 的主 LLM 已发 askUserQuestion 问「再评/定稿」）。
- design.md 定：视图是否额外带「继续/定稿」按钮直接触发回环，还是纯展示 + 指引回 chat（倾向后者，避免与 askUserQuestion 重复触发逻辑）。

### R5. 触发条件

- 视图仅在 review workflow session 的 reviewing/revising/reported state 出现，且 review-state.json 存在时渲染。
- 其他 plugin/state 不显示，避免干扰 dev 等。

## Acceptance Criteria

- [ ] review session 的 reviewing/revising/reported state 出现专属矩阵视图（review-state.json 存在时）。
- [ ] 视图读 `<task>/review-state.json`，按轮次 × 模型正确渲染矩阵（verdict + findings 数）。
- [ ] 维度对比视图：选定维度横向呈现各模型发现 + severity。
- [ ] finding 可展开看完整 issue + location。
- [ ] review-state.json 每轮重写后（rounds 累积）视图增量更新。
- [ ] 非 review session / 无 review-state.json 时不显示该视图（回归 dev 不受影响）。
- [ ] 前端测试覆盖（组件渲染 + review-state.json 解析 + 维度对比 + state 门控）。

## Open Questions（design.md 需解决）

1. R3 视图位置：chat 内嵌面板 / 独立抽屉 / Tab（参考现有 SubagentDrawer / ModeSelect 的 popover 模式）。
2. R4 视图纯展示 vs 带指挥按钮（倾向纯展示 + 指引回 chat）。
3. review-state.json 的前端监听机制（轮询 vs IPC 推送，参考 subagent_runs 现有刷新路径）。
4. finding 的 location 字段能否支持点击跳转 prd 对应位置（依赖 prd 内定位锚点是否存在）。

## Notes

- **主要数据源**是主 LLM 提炼的 review-state.json（层次 2 决策），但**保留 reviewer final_text 作为验证/fallback 路径**（采纳评审议题2）—— 矩阵每条 finding 可跳转原始 final_text；解析失败时降级到 SubagentDrawer。
- 轮次归属由 review-state.json 显式标注（R2），与 C1 resume 实现形态解耦。
- **执行顺序**：C1 → C3（含 schema 设计）→ C2。本任务在 C3 的 review-state.json schema 定稿后才能实现（父任务执行顺序）。
- C2 的 review-state.json schema 需与 C3 的 wf-synthesize（写入方）约定一致 —— 跨任务契约，design 阶段对齐。
