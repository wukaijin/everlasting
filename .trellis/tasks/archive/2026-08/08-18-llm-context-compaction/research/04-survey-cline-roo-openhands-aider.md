# 调研:Cline / Roo Code / OpenHands / Aider / Amp / Droid / 通用原则(2026-08-18)

*核心结论均对照 GitHub 源码或官方文档核实。*

## 1. Cline

- **触发**:`COMPACTION_TRIGGER_RATIO = 0.9`;目标 `DEFAULT_TARGET_RATIO = 0.7`(长会话 0.5);溢出报错时强制压缩并走**确定性 basic 策略**(不调 LLM —— 恢复不能赌再一次成功请求)。
- **摘要**:LLM 指令 "Summarize the provided coding session into a concise continuation note with detailed next steps",输出上限 4096 token;以 `role=user` 消息 `"Context summary:\n..."` 落回历史(metadata `kind=compaction_summary, displayRole=system`)。模板 8 段:Primary Request and Intent / Key Technical Concepts / Files and Code Sections / Problem Solving / Pending Tasks / Current Work / Optional Next Step / Most Recent User Intent。([源码](https://github.com/cline/cline/tree/main/sdk/packages/core/src/extensions/context)、[Zenn 解读](https://zenn.dev/gotalab/articles/4b74e6810db959))
- **逐字保留**:`DEFAULT_PRESERVE_RECENT_TOKENS = 20_000`;`findCutIndex` 保证**最近一条 typed user prompt 所在整轮绝不被摘要**(cut ≤ lastTurnStart)。
- **basic 策略(降级)**:被丢弃的工具活动折成 `<SYSTEM_NOTICE>`,保留最近 **3 条** assistant 文本逐字;旧 tool result / 文件内容截到 2000 字符(`...[truncated N chars]`);旧轮附件丢弃只留路径。

## 2. Roo Code

([源码](https://github.com/RooCodeInc/Roo-Code/tree/main/src/core/condense)、[文档](https://roocodeinc.github.io/Roo-Code/features/intelligent-context-condensing))

- 阈值百分比滑杆,**默认 100%**(社区常调 80%);窗口预留 30%(20% 输出 + 10% buffer)。
- 摘要 prompt 明示:"This summarization request is a SYSTEM OPERATION, not a user message… work continues seamlessly — as if it never happened";首条消息中的 `<command>` 块以 `<system-reminder>` 形式**跨压缩强制保留**。
- 特色:**folded file context** —— tree-sitter 只保留代码签名(默认 50k 字符)附在摘要后;为孤儿 tool_call 注入合成 tool_result;摘要 UI 可展开、checkpoint 保留原文可 rewind。
- 非 compact 截断:provider 报超限时**自动砍 25% 重试**。

## 3. OpenHands(software-agent-sdk)

([源码](https://github.com/OpenHands/software-agent-sdk/tree/main/openhands-sdk/openhands/sdk/context/condenser))

- **可插拔 condenser 架构**:每次 LLM 调用前把 event 流压成 `View`。Noop / RecentEvents(纯滑窗)/ **LLMSummarizingCondenser(默认,增量式:只总结本次被"遗忘"的事件,旧摘要作为输入继续滚动)** / pipeline 组合。
- 参数:`max_size=240` 事件(SDK 默认 80)、`keep_first=2`(头部事件永不压缩)、`minimum_progress=0.1`(一次至少压 10%,否则视为失败)、硬重置重试 5 次。
- 摘要模板:USER_CONTEXT / TASK_TRACKING(**必须保留 task ID 与状态**)/ COMPLETED / PENDING / CURRENT_STATE,代码任务追加 CODE_STATE / TESTS / CHANGES / DEPS / VERSION_CONTROL_STATUS。摘要用**独立 LLM**(可与主模型不同)。
- **落库**:event stream 写 `Condensation` 事件(`forgotten_event_ids` + `summary` + `summary_offset`);原始事件不删、可审计回放。

## 4. Aider

- 聊天历史超 `min(max_input_tokens/16, 1024..8192)` 时**后台线程**摘要;`ChatSummary` 递归:**尾部保留约一半预算逐字**,头部摘要;深度>3 则整体 summarize_all;先用 weak_model 再主模型。
- Repo map(tree-sitter + 图排序迭代逼近 `--map-tokens` 预算,默认 1024,无聊天文件时 ×8)承担"代码上下文"角色,聊天历史保持小。
- 手动治理:`/tokens` / `/drop` / `/add` / `/clear`。

## 5. Amp / Droid

- **Amp**:2025-10 [Handoff](https://ampcode.com/news/handoff) 一度**移除 /compact**(压缩有损、"stacking summary on top of summary"),改为生成目标 + starter prompt 开新线程;[Amp, Rebuilt](https://ampcode.com/news/neo) 后回归:90% 满自动压缩,handoff 移除。
- **Droid(Factory)**:公开配置 `compactionTokenLimit` / `compactionModel`("same" 或指定);算法未公开;changelog 提到 "Skills preserved after compaction"。

## 6. 通用参考

- **Manus 博客**([原文](https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus),已全文核对):① KV-cache 命中率是第一指标(前缀稳定、append-only、显式断点);② 工具 mask 不 remove;③ **文件系统即上下文,压缩必须可恢复**(留 URL/sandbox 路径);④ **recitation**:反复重写 todo.md 把全局计划推回注意力末端;⑤ 保留失败记录。注意:流传的 "~20% misalignment 预算""隐藏 scratchpad" **不在原文中,未核实**。
- **Anthropic** [Effective context engineering](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents):compaction + structured note-taking + just-in-time 检索 + sub-agent 隔离四件套。
- **Context-Folding**([arXiv 2510.11967](https://arxiv.org/abs/2510.11967)):子任务分支临时子轨迹、完成后"折叠"回紧凑形式。
- **跨工具对比**([badlogic gist](https://gist.github.com/badlogic/cd2ef65b0697c4dbe2d13fbecb0a0a5f))。

## 7. 跨工具共识与分歧

**行业标配(共识)**:
1. 满窗 80-100% 触发(turn 边界检查):Cline 0.9 / Amp 90% / Claude Code ~83.5%(buffer 式)/ Roo 默认 100%。
2. **"结构化 LLM 摘要 + 近期消息逐字保留"双层结构**;逐字区 ~20k token(Cline/Codex)或 30%(Gemini)或 keep_tokens 15k(opencode)或半预算(Aider)。
3. 摘要以普通 user 消息落回并**显式标注"系统操作/延续先前会话"**(Cline `kind=compaction_summary`、Roo "SYSTEM OPERATION"、Codex handoff 前缀)。
4. 摘要模板趋同:任务/决策/文件改动/错误/待办/当前工作/下一步。
5. **确定性溢出兜底**(摘要失败/超限时):Cline basic 策略、Gemini 截 tool output、Roo 砍 25% 重试、opencode compact-后-重试一次。
6. **原始历史另存不删**(checkpoint / event stream / "落库无损、上下文有损")。

**分歧大的**:
1. 触发度量:百分比 vs 事件数 vs 绝对 token buffer。
2. 压缩粒度:整段替换 vs **增量滚动合并**(OpenHands 只总结新遗忘事件;opencode V2 prior-summary + "conversation wins")。
3. 摘要模型:会话主模型(Claude Code/Codex/Roo/opencode)vs 独立/廉价(Gemini utility 通道 / OpenHands / Aider weak_model)。
4. 额外保留物:Roo 折叠代码签名、Cline 保 3 条 assistant 文本、Codex 只保用户消息轮、Aider 靠 repo map。
5. 哲学:Amp 曾主张"别压缩、开新线程接力"后又回归;Manus 主张外置可恢复。
