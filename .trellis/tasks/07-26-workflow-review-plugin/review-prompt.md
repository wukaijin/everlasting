# 评审提示词：workflow review plugin epic

> 复制以下「--- 提示词正文 ---」之间的内容给评审 LLM。
> 评审 LLM 需要能读取本目录下的 PRD 文件（父任务 + C1/C2/C3）才能给出有效反馈。

--- 提示词正文 ---

我在为一个 AI 编程助手（everlasting）设计一个新功能，需要你帮我评审这套设计文档。请你先读取以下 4 个 PRD 文件，然后给出评审意见：

- `.trellis/tasks/07-26-workflow-review-plugin/prd.md`（父任务 / epic，含整体设计决策和任务拆分）
- `.trellis/tasks/07-26-subagent-resume/prd.md`（子任务 C1：subagent 续接机制）
- `.trellis/tasks/07-26-review-plugin-pack/prd.md`（子任务 C3：review plugin 资源包）
- `.trellis/tasks/07-26-review-viz/prd.md`（子任务 C2：review 可视化视图）

## 背景（你需要知道的上下文）

everlasting 是一个本地 AI 编程助手（Tauri + Rust 后端 + Vue 前端），有一个「workflow plugin」系统：每个 plugin 定义一套状态机 + 角色（sub-agent）+ 方法 skill，session 绑定一个 plugin，引导 AI 按流程干活。已有一个 `dev` plugin（planning → in_progress → done，单模型实施流）。

现在要做第二个 plugin `review`，核心是**多模型评审流**：让多个不同模型各自独立评审同一份需求/计划，主 LLM 综合修订后回环重评，过程中用可视化呈现各模型发现，由用户指挥收敛。review 主要用在 dev 之前评审需求/计划。

PRD 里的「Background」「代码事实」段落描述了现有架构（文件路径 + 行号锚点），这些都是我已经在代码里验证过的事实，你可以采信。「Decisions」段落是设计决策记录。

## 请你重点评审以下维度

1. **整体合理性**：多模型评审流 + reviewing↔revising 回环 + 过程可视化这个设计，是否真的解决了「需求评审」的问题？有没有更简单的替代方案被遗漏？

2. **任务拆分与依赖**：父任务把 epic 拆成 C1（resume 引擎）→ C3（资源包 + schema）→ C2（可视化）三个有依赖的子任务。这个拆分和依赖顺序合理吗？有没有该合并或该再拆的？resume 作为独立基建先做，是否过度（resume 只为 review 服务值得吗）？

3. **关键决策的风险**：特别注意这几个决策是否站得住——
   - 「产物 = prd 本身，砍掉 review.md」（决策 6）：评审不产报告，直接改 prd，会不会丢失「评审过程」的价值？
   - 「可视化数据基础 = 层次 2」（决策 11）：让主 LLM 把 N 份自由文本评审提炼成结构化 JSON 喂给前端，主 LLM 万一提炼错了/漏了，前端就显示错。这个单点失败风险可接受吗？
   - 「resume 硬前置」（决策 8）：review 强依赖一个还不存在的引擎特性，如果 resume 做不出来或延期，整个 epic 卡死。这个单点依赖风险如何缓解？

4. **遗漏与盲点**：有没有什么重要场景、边界条件、或失败模式，PRD 没考虑到？特别关注：
   - 多模型评审里，如果某个模型 API 失败/超时，回环怎么办？
   - reviewing↔revising 回环如果无限循环（每轮都「再评」），有没有兜底？
   - resume 续接上轮 worker 会话，但上轮的 prd 已经被 revising 改了，续接的 messages 里还残留「旧 prd 内容」，会不会误导 reviewer？

5. **可实施性**：作为 PRD（非 design），它的接受标准是否可测？有没有模糊到无法验收的条目？

请直接、批判性地给意见，不要客气。如果某个设计你觉得就是错的，请明确说「这里错了，理由是...」。如果你会怎么做，也请说。
