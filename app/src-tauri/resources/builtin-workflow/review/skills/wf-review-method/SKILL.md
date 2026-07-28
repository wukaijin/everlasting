---
name: wf-review-method
description: reviewing 阶段方法:维度推荐器细则 + 并发派 reviewer 的执行指引
allowed-tools: []
---

# 评审执行(reviewing)

reviewing 阶段你要按 intake 确认的维度,并发派 N 个 reviewer(各不同模型)。本 skill 是维度推荐器的细则 + 派 reviewer 的执行指引。

## 1. 复核维度(承接 intake)

intake 已确认维度集(见 wf-review-prep)。进入 reviewing 时快速复核一遍,若用户在 intake 漏选了关键维度(如 Bug 任务没选「根因分析」),可在这里 askUserQuestion 补充。

**维度是软约束**:reviewer 遵守得越好,主 LLM 综合越准;但 reviewer 是自由 markdown 输出,主 LLM 兜底提炼。

## 2. 维度推荐器(参考表)

若 intake 没走完整维度确认(如直接进 reviewing),按任务种类重新推荐(详见 wf-review-prep §4 表)。维度定下后,**N 个 reviewer 过同一套维度**(多模型同题对比,价值在分歧)。

## 3. 并发派 reviewer

对用户多选的每个模型,用 `dispatch_subagent` 派一个 reviewer:

- `subagent` 参数 = `reviewer`(review plugin 的唯一角色;它会在 dispatch_subagent 的 subagent enum 里出现 —— 若 enum 里没有 `reviewer`,说明 plugin 层 agent 未加载,这是引擎 bug,请报告用户而非手动 edit 配置)
- `model` 参数 = 该模型的 display_name(per-dispatch override,reviewer.md 的 `model:` 留空就是为此)
- `resume_from` = 上一轮该模型的 run_id(**回环续接**,省 token;首轮无 resume_from)

**并发**:多个 dispatch_subagent 调用尽量并发(在同一 turn 内连续发起),不要串行等。

## 4. delegation 模板填入

dispatch 时 delegation 模板(plugin 配置)会自动填入 `{title}` / `{summary}` / `{relevant_specs}`。模板告诉你「告诉 reviewer 要评审什么、不该做什么」。你可在 worker instruction 里补充本次评审的具体维度(让 reviewer 知道评什么)。

## 5. 某模型失败的处理

若某模型的 dispatch 失败(超时 / error):
- **不阻塞**其他模型 —— 标注该模型缺失
- 在 revising 写 review-state.json 时,该模型的 status 标 `error`(对齐 DB subagent_runs.status 的 CHECK 约束;详见 wf-synthesize 的 status 枚举 running/completed/cancelled/error/incomplete)
- 用户可在后续轮次重试该模型

## 6. 等全部返回后转 revising

所有 reviewer 返回(或失败)后,用 ask_user_question 申请转 revising(用户确认才推进)。

reviewer 输出是自由 markdown(按维度分节 + severity + location + 建议),你进入 revising 后用 wf-synthesize 综合。

## 约束

- reviewing 不修订 prd —— 修订是 revising 阶段的事
- reviewer 是只读角色,你不替它评审(你的职责是派它 + 等结果)
- 同一套维度过所有模型(同题对比,否则无法横向对比)
