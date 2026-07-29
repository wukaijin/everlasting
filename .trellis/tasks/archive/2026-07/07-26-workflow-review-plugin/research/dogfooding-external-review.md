# Dogfooding 笔记：用外部 review 验证 review plugin 设计

> 日期：2026-07-26
> 性质：设计验证证据（非纸面推理，来自真实经历）
> 关联：本笔记的结论已回流到父任务 Out of Scope + C3 R3/R7 + C2 R3

## 背景

review epic 的 PRD（父任务 + C1/C2/C3）完成后，把评审提示词 + 4 个 PRD 交给两个外部 LLM（MiniMax-M3、DeepSeek-v4-pro）评审，回收意见后由主 LLM（我）做 triage。

关键点：**这一轮 review 本身就是 review plugin 想做的事——评审一个 task 的产出物（PRD）→ 综合修订 → 收敛**。但我们跑在 EverLasting 脚手架**之外**（手动复制 prompt、手动回收、手动 triage）。等于用"人肉版"跑了一遍 review 流程，切身体验了设计的对与缺。

## 真实痛点（按时间顺序）

### 痛点 1：评审对象是散落的 4 个文件，不是 reviewable unit
评审 prompt 列了 4 个 PRD 路径让评审 LLM 自己读 + 拼。MiniMax 花了 §0 一整节核对跨文件一致性——这本不该是评审者负担。

### 痛点 2：上下文交接靠 prompt 文字，极易丢
prompt 里埋了"代码事实可采信""重点评这 5 维度"，但项目背景（模型配置、学习目的、brainstorm 决策）没给足。DeepSeek 花整段讨论"是否跨 provider"，MiniMax §0 核对行号——都是因为信息不全只能假设。

### 痛点 3：两份意见高度重叠但格式不同，比对费劲
MiniMax 用风险矩阵 + schema 草案，DeepSeek 用散文。两份在 resume 硬前置 / 层次2单点失败上独立收敛（高信号），但要逐条人工比对才发现。

### 痛点 4：triage 要带原始上下文，评审者缺了就撞墙
做 adopt/reject 时最大依据是 brainstorm 时用户原话（"resume 必须先做""省 token""学习 harness"）。这些评审 LLM 没有——所以它们建议"降级 resume"在它们的信息边界内合理，但撞墙于已知约束。**这暴露：评审者缺决策上下文，"合理建议"可能无效。**

### 痛点 5：triage 回流 PRD 是手工的、易漏
采纳项分散改到 4 个 PRD ~10 处，每处要记"这条对应哪个 PRD 哪段"。漏改一处就 triage 与 PRD 不一致。

### 痛点 6：无"评审→修订→再评审"闭环
这轮跑完 triage 落档。但若评审者想看"反对理由是否成立"，得重读全部文件——变更没结构化呈现。review plugin 设计的 reviewing↔revising 回环，在这轮外部 review 里**根本跑不起来**（无 resume/可视化/review-state.json）。

## 对 review plugin 设计的启示（已回流）

### 设计对了的（经隐性检验）
- 以 task 为评审单位 ✓
- 回环收敛 ✓
- 可视化对比 ✓
- change_log 字段 ✓（DeepSeek 建议加，正好补这轮"改了什么无记录"的缺口）

### 设计缺了的（已补充，低成本）
1. **wf-synthesize 显式建模 triage**（不只综合，要决策：adopt/reject + 理由 + 对照约束）—— 痛点 4 的直接产物。评审者缺上下文是常态，主 LLM 的 triage 必须带 brainstorm 上下文判断。
2. **review-state.json finding 加 triage 字段**—— 让收敛过程可追溯（这轮我就缺这个：哪条意见导致哪处改动，要翻 review-triage.md 对照）。
3. **父任务 Out of Scope 明确"跨 session/人机混合评审不在 MVP"**—— 这轮正是人机混合（我=主 LLM，外部=reviewer），验证了该场景真实，但 MVP 聚焦 AI 内部闭环。

### 反思到的局限（不改，记边界）
review plugin MVP 覆盖"AI 内部多模型评审"（主 LLM 派 subagent），不覆盖"跨 session / 人机混合"（这轮的模式）。这是 scope 取舍，MVP 聚焦前者合理，后者列 Phase 2。

## 价值

这轮最大的收获不是评审意见本身（虽然也很有价值），而是**用脚手架外的方式跑了脚手架内想做的事，切身感受到设计的对与缺**。这种 dogfooding 比纯纸面推理有说服力得多——痛点是真实的，启示是具体的。

后续若做"跨 session/人机混合评审"（Phase 2），本笔记的痛点 1/2/5/6 是直接的需求来源。
