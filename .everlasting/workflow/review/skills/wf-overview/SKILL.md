---
name: wf-overview
description: review workflow 全貌说明。进 review workflow 时加载,建立"多模型评审流、4 state 回环、主 LLM 是 orchestrator"的全局意识
allowed-tools: []
---

# Review Workflow 全貌

本 skill 是 `review` workflow(多模型评审流)的完整说明。当你处于 workflow session 且 breadcrumb 显示 review workflow 时,读这个 skill 理解整个流程。

## 整体流程(4 个 state,带回环)

```
intake → reviewing ⇄ revising → reported
 准备     并发派 reviewer   综合+修订   收尾
```

- `intake` → `reviewing`:用户确认后进入评审
- `reviewing` → `revising`:reviewer 全部返回后,用户确认进入综合修订
- `revising` → `reviewing`(**回环**):用户选择「再评一轮」时回到 reviewing(reviewer 用 `resume_from` 续接上轮)
- `revising` → `reported`:用户选择「定稿」时收尾

每个 state 转移都需要**用户确认**(你用 ask_user_question 申请,用户同意才推进)。回环是 review workflow 的核心 —— 通过多轮评审收敛 prd,而非一次性。

**产物 = prd/design 本身**:review 不产最终报告,revising 阶段主 LLM 直接修订 prd(主 LLM 在 workflow_enabled 时有写工具),prd 即产物,dev session 读同一 prd 实施。

## 主 LLM 的 orchestrator 职责

你不是被动的执行者,你是评审流程的指挥。每个 state 你的核心动作:

| state | 你做什么 |
|---|---|
| intake | 读 current_task 理解评审对象;发现可用模型(从 dispatch_subagent 的 model enum);askUserQuestion 让用户多选评审模型;按任务种类推荐评审维度并确认。详见 `wf-review-prep` |
| reviewing | 按确认的维度,并发派 N 个 reviewer(各不同模型,用 dispatch_subagent 的 `model` 参数 + `resume_from` 续接上轮)。等全部返回。详见 `wf-review-method` |
| revising | 综合 N 份评审;triage(adopt/reject/defer 每条 finding);修订 prd/design;写 `<task>/review-state.json`;askUserQuestion 问用户「再评一轮还是定稿」。详见 `wf-synthesize` |
| reported | prd 已修订就绪,告知用户可切 dev session 实施 |

## 多模型评审心智

review workflow 的价值在**分歧** —— N 个不同模型同题评审,差异处往往是被单个模型漏掉的问题。

- 派 reviewer 时**跨 provider**(如果用户选了不同 provider 的模型),避免同 provider 模型盲区重叠
- 同一套维度过所有模型(同题对比),维度由 intake 确认
- reviewer 是**只读**角色(读 prd/design + 项目代码做「设计 vs 实现一致性」检查),不写文件
- reviewer 的 `model:` 留空,由 dispatch_subagent 的 `model` 参数(per-dispatch override)主导 —— 这是多模型的核心机制
- 某模型失败时标注缺失,不阻塞其他模型(写 review-state.json 的 status=error)

## task 共享机制(衔接 dev)

review 和 dev session 自动共享同一 `current_task`(`resolve_current_task` 按 project 扫)。prd 是衔接物:

- review revising 阶段修订 prd
- review reported 后,用户切 dev session
- dev session 读**同一份 prd** 实施(无需手动拷贝)

所以 **prd 是单一真源**,review 改它,dev 读它。

## 门控:违反流程时怎么办

若你想做当前 state 不允许的事(如 intake 想直接改 prd),不要硬闯:
- 用 ask_user_question 跟用户协商
- 用户同意 → 推进 state 继续
- 用户拒绝 → 回 breadcrumb 提示,继续当前 state 该做的事

这是**协商档**(同 dev workflow 约定)—— 流程有默认,但允许例外(用户背书)。

## 何时 use_skill 哪个

| skill | 何时 |
|---|---|
| wf-overview(本 skill) | 进 review workflow 时,或忘了流程时自查 |
| wf-review-prep | intake(模型发现 + 维度推荐) |
| wf-review-method | reviewing(维度推荐器细则,派 reviewer 前复核维度) |
| wf-synthesize | revising(综合 + triage + 修订 prd + 写 review-state.json) |
