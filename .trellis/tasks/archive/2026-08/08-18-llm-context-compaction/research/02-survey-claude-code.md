# 调研:Claude Code 上下文压缩机制(2026-08-18)

*来源:泄露源码分析(openedclaude)、官方文档、逆向仓库(Yuyz0112)、GitHub issues 交叉验证。*

## 1. Auto-compact 触发

- **阈值是 token buffer 而非固定百分比**:`getAutoCompactThreshold` = 有效窗口 − `AUTOCOMPACT_BUFFER_TOKENS`(13,000);有效窗口 = context window − min(maxOutputTokens, 20,000)。200K 模型 → 约 180K 有效 → 约 167K 触发 ≈ 83.5%。([openedclaude 架构分析](https://github.com/openedclaude/claude-reviews-claude/blob/main/architecture/11-compact-system.md))
- **历史演变**:早期 ~95%(issue #6123 报为 bug);2025 年中降到 ~77-78%;2026 年初悄然上调到 ~83.5%(社区推测与 v2.1.21 修复有关,未官宣)。"92%" 未见实锤出处。([claudefa.st](https://claudefa.st/blog/guide/mechanics/context-buffer-management)、[hyperdev](https://hyperdev.matsuoka.com/p/how-claude-code-got-better-by-protecting))
- **无迟滞/防抖,但有三级状态机**:warning(有效窗口−20K)→ auto-compact(−13K)→ blocking(−3K,提示手动 compact);触发时机在**每次 API 请求前(turn 边界)**。
- 可配置:`/autocompact [auto|<tokens>]`、`--autocompact` flag、env 覆盖;buffer 本身硬编码([#15435](https://github.com/anthropics/claude-code/issues/15435) 请求可配置被拒)。
- **熔断**:`MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES = 3`,连续 3 次压缩失败后本 session 放弃 auto(源码注释:曾有 1,279 个 session 各 50+ 次连续失败,浪费约 25 万 API calls/天)。

## 2. Microcompact vs Full compact(实为多层)

- **Microcompact**:官宣"自动清理旧 tool calls 以延后 /compact"。`microCompact.ts`(531 行)**每轮运行**,只手术式清旧 tool result,不动对话结构,零 LLM 调用。可清:FileRead/Bash/Grep/Glob/WebSearch/WebFetch/FileEdit/FileWrite;**Agent(task)结果不清**;保留最近 5 条 tool result,旧的替换为 `[Old tool result content cleared]`。暖缓存走 API `cache_edits` 按 tool_use_id 服务端删除以保 prompt cache。([Threads 官宣](https://www.threads.com/@boris_cherny/post/DM8tYAJz6gN)、[barazany.dev](https://barazany.dev/blog/claude-codes-compaction-engine))
- 泄露分析称共 8 种模式:microcompact、HISTORY_SNIP(滑窗截断)、session memory compact(无 LLM)、full compact(LLM 摘要)、emergency truncation 等。([Medium 泄露分析](https://medium.com/data-science-collective/inside-claude-codes-leak-8-compaction-modes-3-memory-tiers-44-flags-anthropic-never-talked-c9740c501e63))
- **Full compact**:auto-compact 阈值或手动 `/compact` 触发,1 次 LLM 调用。

## 3. 结构化摘要 Prompt(实为 9 段)

逆向仓库原文核实([Yuyz0112/claude-code-reverse](https://github.com/Yuyz0112/claude-code-reverse/blob/main/results/prompts/compact.prompt.md)、[Reddit 泄露帖](https://www.reddit.com/r/ClaudeAI/comments/1jr52qj/here_is_claude_codes_compact_prompt/)):

1. Primary Request and Intent
2. **Key** Technical Concepts
3. Files and Code Sections
4. Errors and fixes
5. Problem Solving
6. **All user messages**(标注 "critical for understanding the users' feedback",**逐字**)
7. Pending Tasks
8. Current Work
9. **Optional Next Step**(要求 "include direct quotes from the most recent conversation" 防任务漂移)

流程:先在 `<analysis>` 标签内按时间线推理,再输出 `<summary>`;`formatCompactSummary()` 剥掉 analysis 只回填 summary。

**回填形式:以 user message 插入**,开头 "This session is being continued from a previous conversation..."(issue #8094 证实);autonomous 场景附指示"继续工作、无需确认摘要"。([barazany.dev](https://barazany.dev/blog/claude-codes-compaction-engine))

## 4. 保留策略(逐字/重注入,不进摘要)

- **磁盘重注入**:system prompt 不变;CLAUDE.md / auto memory 压缩后从磁盘重新注入;丢失 path-scoped 规则(读到匹配文件才恢复)。([hidekazu-konishi](https://hidekazu-konishi.com/entry/claude_code_compaction_and_long_session_guide.html))
- **文件重注入**:最近读过的 5 个文件,总预算 50K、单文件 5K,并**伪造 read 工具对**以通过 read-before-write 校验;skills 重注入(总 25K);plan/todo 状态恢复。([openedclaude](https://github.com/openedclaude/claude-reviews-claude/blob/main/architecture/11-compact-system.md)、[labuladong 复刻](https://labuladong.online/en/ai-coding/coding-agent/compact/))
- 摘要内逐字:全部用户消息 + 最近对话直接引语。

## 5. /compact 手动命令

可带定向 instructions(`/compact focus on the API changes`);不受 3 次失败熔断限制;`/rewind` 另有部分压缩。([官方 commands](https://code.claude.com/docs/en/commands))

## 6. 持久化与 resume

摘要作为 synthetic user message 写入 session transcript(JSONL);`/resume` 恢复压缩后历史(压缩状态保留),属"有损起点";旧 checkpoint 因索引指向已删历史被丢弃。已知 bug #8094:resume 列表摘要显示占位文本。

## 7. 摘要模型

**默认主模型**(同会话 model/system prompt/消息前缀,为命中 prompt cache —— 曾试别的方案致 98% cache miss);社区 `compactModel` 设置有 bug(#16065)。"专门小模型"未核实。([barazany.dev](https://barazany.dev/blog/claude-codes-compaction-engine)、[#16065](https://github.com/anthropics/claude-code/issues/16065))

## 8. /context 记账

resident layer(system prompt、CLAUDE.md、MEMORY.md、skill 描述、工具定义)/ tool results / 对话轮次分类明细;压缩调用本身开销约 15-20K tokens。

## 9. 已知缺陷 / 社区批评

- **指令遵从度断崖**:压缩前 100% 遵守的项目指令压缩后 100% 违反(#9796);忘记 Skills(#13919)、忘记所在 repo 重复犯错(#10960)。([golev 汇总](https://golev.com/post/claude-saves-tokens-forgets-everything/))
- **触发类 bug**:102% 无限压缩循环(#3274)、100% 不触发(#66144、#63015)。
- **摘要的再摘要**逐轮劣化;"重 recency 轻 importance"。([golev](https://golev.com/post/claude-saves-tokens-forgets-everything/))

## 对本项目的可借鉴点

1. **分层压缩,先确定性后 LLM**:microcompact(旧 tool output 占位符化,保对话结构、保 prompt cache)+ 满阈值才 LLM 全量摘要。
2. **阈值用 token buffer 表达**(window − min(maxOutput,20K) − 13K,给压缩调用自身留空间)+ 三级水位 + 连续失败熔断,避免 102% 死循环。
3. **结构化摘要模板 + 双逐字锚点**("All User Messages" + 最近对话直接引语)是对抗任务漂移的关键;摘要以 user message 回填并指示"继续工作不复述"。
4. **resident 层磁盘重注入**:CLAUDE.md/skills/todo 不进摘要,压缩后从磁盘重读 —— 本项目 memory 块天然每请求重注入,已等价。
5. **规避已知坑**:摘要的再摘要逐轮劣化 → 增量合并或落盘重读;高优先级约束放 resident 层而非依赖摘要保真。
