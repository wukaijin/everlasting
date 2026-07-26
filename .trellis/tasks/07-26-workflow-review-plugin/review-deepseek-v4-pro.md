# Review: workflow review plugin epic

> 评审对象：`.trellis/tasks/07-26-workflow-review-plugin/` 下 4 个 PRD（父任务 + C1/C2/C3）
> 评审维度：整体合理性 / 任务拆分与依赖 / 关键决策风险 / 遗漏与盲点 / 可实施性

---

## 1. 整体合理性

**多模型评审 + 回环 + 可视化这个方向是对的**，但要先退一步问：这个问题值得用这么重的方案吗？

PRD 的核心价值主张是：不同模型审同一份 prd，主 LLM 综合修订，用户指挥收敛。这确实能发现单一模型漏掉的盲区——不同模型对同一段文字的理解偏差本身就是有价值的信息。**这个用例是 real 的。**

但有一个 PRD 没正面回答的问题：**reviewer 都是同一个 provider 的不同 model variant（Anthropic 的 Sonnet/Opus，OpenAI 的 GPT-4o/o1）**。如果所有模型来自同一个或两个 provider，独立性的边界在哪里？PRD 假设"不同模型 = 独立视角"，这在不跨 provider 时可能打了折扣。这不是设计错误，但值得在 reviewer.md 或 wf-overview 里提一句：**建议至少跨 2 个 provider 选 reviewer**，否则多样性打折。

**更简单的替代方案**：单轮 review——主 LLM 自己读一遍 prd 提意见，用户确认后修订。不需要 resume、不需要多模型、不需要矩阵视图。这显然更简单，但失去了"多视角对比"的核心价值。所以我倾向于认为多模型方案值得做，尤其是考虑到作者明确说过这个项目是**学习 harness 工程**——多模型编排本身就是值得学习的场景。

**结论**：整体合理，不否决。

---

## 2. 任务拆分与依赖

**拆分 C1 → C3 → C2 的依赖链逻辑上自洽，但 C1 作为硬前置是整个 epic 的最大风险点。**

具体分析：

- **C3 依赖 C1** 是合理的——reviewer 用 resume 续接确实需要引擎层支持。但如果 C1 延期或做不出来怎么办？目前没有 fallback。
- **C1 定位为"独立基建，dev 循环也受益"** —— 这个说法目前只是**潜力**，不是现实。dev 的 implement ↔ check 循环目前**并没有**使用 resume（也不存在 resume 机制），所以"dev 也受益"是一个未来的、未规划的使用场景。如果 review 是 C1 的唯一消费者，那 C1 就不是真正的"独立基建"，而是 review 的专用引擎改动。
- **建议**：C1 的 PRD 里增加一个 **fallback 路径**：如果 resume 做不出来（或为了加速交付），reviewer 首版可以**全新派（无 resume）**跑通，token 开销大但功能完整。这样 C3/C2 可以不阻塞在 C1 上。这是低风险的合理性检查——如果连"不 resume 也能跑通 review 流"都做不到，说明设计有问题。

**拆分粒度**：C3（资源包 + schema）+ C2（可视化）拆开是好的，因为 schema 是跨任务契约，必须先定稿。C3 PRD 的 R7 明确定义了 review-state.json schema，C2 PRD 的 R1 消费它——这个契约拆分清晰。

**结论**：依赖链合理但过于刚性。给 C1 加 fallback 路径（resume 可选，非硬依赖）会大幅降低 epic 风险。

---

## 3. 关键决策的风险

### 决策 6：产物 = prd 本身，砍掉 review.md

**这个设计我同意，但缺了一个重要的补充。**

"自产自销无意义"这个论点是对的——AI 写 review 报告给 AI 读，确实是循环引用。直接改 prd 更务实。

**但缺的是什么？** 用户需要知道"这次 review 发现了什么、改了什么"。review-state.json + 可视化矩阵部分弥补了"发现了什么"，但**"改了什么"完全没有记录**。prd 被主 LLM 修订后，改动散落在文件里，用户如果不逐行 diff，不知道哪些修订是因为 review 触发的，哪些是主 LLM 自己发挥的。

**建议**：不要完全砍掉，而是让 revising 主 LLM 在 prd 底部或 review-state.json 里追加一个简单的 **change log** 条目：「本轮修订：§2 澄清了范围边界（reviewer A 反馈）；§4 补充了错误处理流程（reviewer B 反馈）；...」。不需要独立文件，几行 bullet 就够。这样用户一眼看到"review 的价值兑现了什么"。

### 决策 11：可视化数据基础 = 层次 2

**这是整个 epic 里最危险的设计决策，但也是当前约束下最务实的。**

危险在于：主 LLM 是单点——它要读 N 份 reviewer 自由文本，提炼成结构化 JSON，写 review-state.json。如果主 LLM：
- 理解错了某个 reviewer 的 nuance → 误标 severity
- 漏了一个关键 finding → 矩阵里不显示
- 写了格式错误的 JSON → 前端崩溃

用户看到的是"干净的矩阵"，但可能已经是失真后的数据。

**但替代方案确实都更差**：
- 层次 1（纯软约束 reviewer 输出 JSON）——LLM 不严格遵守格式，做不到。
- 层次 3（引擎强制 JSON schema）——改动太大，Phase 2 合理。

**所以层次 2 是对的，但需要两个安全网**：

1. **矩阵每个 cell 必须带 "view raw review" 链接**，指向 reviewer 的原始 final_text（已存在 subagent_runs 表）。PRD C2 R3 说"单元格/finding 可展开看完整 issue 描述"，但这是展开**提炼后**的 finding，不是 reviewer 原文。用户需要能随时对比"主 LLM 提炼的" vs "reviewer 实际写的"。
2. **review-state.json 解析必须有健壮降级**——主 LLM 写了坏 JSON 时，视图不能白屏。需要在前端有个 try-catch + fallback UI（"review-state.json 解析失败，请查看原始 reviewer 输出"）。

C2 PRD 的 Note 说"数据源是主 LLM 提炼的 review-state.json，**不直接读 reviewer 的 final_text**"——**这个"不"太绝对了。应该改为"主要数据源是 review-state.json，但保留 reviewer final_text 作为验证路径。"**

### 决策 8：resume 硬前置

**这是最大的结构性风险。** 整个 epic 的成功与否绑在 C1 一个引擎特性上。如果 C1 遇到意外复杂度（PRD 里列的 open questions：transcript 重建、worktree 策略、跨 session resume——这些问题都没有 trivial 答案），epic 整个卡住。

**缓解方案**：reviewer 首版可以**不依赖 resume**，全新派发。事实上，reviewer 是只读角色，每次重读 prd 全文的成本是可接受的（prd 通常几千字，不是百万行代码库）。resume 是**优化**（省 token），不是**功能必需**。把 resume 从"硬前置"降级为"增强项"：
- C3 先做不带 resume 的 reviewer 派发（全新构造 messages）
- C1 做完后 C3 加 resume 参数
- C2/C3 的其他部分不阻塞

**现在 PRD 的立场是"resume 是 review 的硬依赖，拆为 C1 先做"——我建议改为"resume 是 review 的优化，C1 可并行或后做"。**

---

## 4. 遗漏与盲点

### 4.1 某个模型 API 失败/超时

**这是生产级多模型编排里最高频的失败模式，PRD 完全没有提。**

场景：reviewing 阶段并发派了 3 个 reviewer（Claude Sonnet、GPT-4o、Gemini），其中 GPT-4o 超时或返回 429 rate limit。主 LLM 怎么办？
- 不等 GPT-4o，拿 2/3 的结果继续？→ 矩阵少一列，用户会不会误解？
- 重试 GPT-4o？→ 等多久？review-state.json 怎么标注"这一轮 GPT-4o 缺失"？

**建议**：review-state.json schema 里 verdict 需要一个额外枚举值 `"error"` 或 `"unavailable"`，让矩阵能显示"该模型本轮未响应"。同时 wf-synthesize skill 需要指引主 LLM 处理部分失败（不等、标注缺失、让用户决定是否对该模型单独重试）。

### 4.2 reviewing ↔ revising 无限循环

PRD 用 `requires_user_confirm: true` 做兜底——每次回环都要用户确认。**这理论上能防止无限循环，但实践中不够。**

问题：如果 reviewer 每轮都发现新问题（哪怕是鸡毛蒜皮的），主 LLM 的 askUserQuestion 会持续问"是否再评"。用户可能不知道什么时候该停，因为 review-state.json 里每轮都有新 finding。**用户需要"该停了"的信号。**

**建议**：
1. wf-synthesize 要求主 LLM 给每轮一个 **convergence 评估**：本轮发现的问题相比上轮是递增还是递减？如果主 LLM 自己判断"本轮无实质性新发现"，应该在 askUserQuestion 时明确说"建议定稿，本轮无新增关键问题"。
2. review-state.json 的 round 条目加一个可选的 `convergence_note` 字段。
3. 硬上限：最多 N 轮（建议 5），超过后自动建议定稿。

### 4.3 Resume 续接中的 stale context 问题

**这是个 real 风险，PRD C1/C3 都没有充分讨论。**

场景：
- Round 1：reviewer 读了 `prd.md` v1，在回复中引用"§3 的 API 设计有问题，因为..."
- Revising：主 LLM 把 §3 重写了
- Round 2：reviewer resume 续接上轮对话，对话历史里有"§3 的 API 设计有问题..."这条消息。reviewer 可能基于旧引用做判断，即使主 LLM 注入了"修订了什么"的澄清。

**问题在于**：LLM 不一定能完美区分"对话历史里的旧引用"和"当前文件的实际内容"。尤其当 reviewer 是 resume 续接时，它的 context 同时包含旧消息 + 主 LLM 的澄清文本 + 新读的文件内容——这三者可能矛盾。

**建议**：
1. C1 的 resume 机制应该支持**截断或标记过期消息**——不一定是引擎功能，可以在 resume 注入的澄清文本里明确说"上轮对话中涉及以下已修改章节的讨论可能已过时：§3, §5"。
2. C3 的 wf-synthesize skill 应该在修订后生成一个 **change summary** 注入到下一轮 reviewer resume 的澄清文本中。
3. Reviewer 的 system prompt 应该有一条："如果上轮对话中的引用与当前文件内容矛盾，以当前文件为准。"

### 4.4 用户中途换 reviewer 模型

场景：Round 1 用了 Claude + GPT-4o，Round 2 用户想换成 Claude + Gemini。review-state.json 的 models 维度如何处理？
- Round 1 有 GPT-4o 列，Round 2 没有 → 矩阵出现不规则列
- 新模型在 Round 1 没有数据 → 无法 resume（无上轮会话）

**建议**：review-state.json 的 rounds 数组加一个 `models_present: string[]` 字段标明本轮实际参与的模型，矩阵按**所有出现过的模型的并集**渲染列，缺席的轮次 cell 显示"未参与"。

### 4.5 review-state.json 写入失败

主 LLM 写 `<task>/review-state.json` 时：
- 目录不存在？
- 权限不够？
- JSON 格式错误（LLM 输出了 trailing comma 或注释）？

**建议**：
1. wf-synthesize skill 明确告诉主 LLM 写文件前先确保目录存在（用 `ls` 或直接写）。
2. 前端读 review-state.json 时必须有 try-catch，解析失败显示"review 数据暂不可用"而不是白屏。
3. C3 考虑用 `write_file` 工具写 JSON（已有），但主 LLM 需要知道这是机器读取的文件，不能加 markdown 包裹或注释。

### 4.6 评审范围——reviewer 该读什么？

PRD 说 reviewer 工具集是只读（read/grep/glob/web_fetch），但**没明确 reviewer 应该读什么文件**。是只读 `prd.md`？还是连同项目代码一起读？如果 reviewer 不能读代码，它的评审只能停留在"文档写得好不好"，不能判断"设计是否可行"——后者才是更有价值的评审。

**建议**：reviewer.md 的 prompt 里明确评审范围——至少让 reviewer 有权读项目结构和关键代码文件，做"设计 vs 实现一致性"检查。

---

## 5. 可实施性（Acceptance Criteria 可测性）

整体 AC 是 concrete 的，但有几个模糊点：

| 位置 | AC | 问题 |
|---|---|---|
| C3 | "reviewer 用 resume 续接 + 多模型" | 这是集成行为，不是可独立验证的单元 AC。需要拆成：① dispatch_subagent 传入 resume_from 参数 ② worker messages 包含上轮历史 ③ 多模型并发派。 |
| C2 | "视图按轮次 × 模型正确渲染矩阵（verdict + findings 数）" | "正确"的定义是什么？需要一个具体的测试 fixture（已知的 review-state.json 内容 + 期望的渲染结果）。 |
| 父任务 | "完整 review 流跑通" | 需要定义一个最小可验收的 smoke test 场景（1 个 task + 2 个模型 + 2 轮 + 收敛到 reported）。 |
| C2 | "前端测试覆盖" | 需要明确覆盖哪些：组件渲染？JSON 解析？维度对比逻辑？state 门控？建议列具体测试文件。 |

---

## 6. 如果你问我怎么做

1. **把 resume 从 C1 硬前置降级为并行增强**：C3 先做"全新派 reviewer"，功能完整跑通后，C1 再加 resume 省 token。这样 epic 不卡在引擎改动上。
2. **review-state.json 加 `convergence_note` + `models_present` 字段**，解决收敛信号和模型变更两个盲点。
3. **C2 矩阵每个 cell 必须能链接到 reviewer 原始 final_text**（已存在 subagent_runs 表，零额外成本）——这是层次 2 决策的安全网。
4. **reviewer.md 给 reviewer 读代码的权限**——纯读 prd 的 review 价值太低。
5. **加一个简单的 change log 到修订流程**——不建新文件，在 review-state.json 或 prd 末尾追加本轮改动摘要。

**总结**：设计方向正确，细节扎实（代码事实都有验证），但 **resume 硬前置是整个 epic 的阿喀琉斯之踵**，以及 **层次 2 的单点失败风险需要 reviewer final_text 作为 fallback 验证路径**。这两个点如果不调整，epic 的风险曲线会很陡。
