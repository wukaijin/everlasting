# 调研:Codex CLI / Gemini CLI / opencode 压缩实现(2026-08-18)

*基于各仓库 main/dev 分支源码与官方文档。*

## 1. OpenAI Codex CLI(Rust,codex-rs)

- **模块**:`codex-rs/core/src/compact.rs`(本地压缩)、`compact_remote*.rs`(远端压缩)、`compact_token_budget.rs`(实验:跳过摘要直接换窗)。按 provider 能力选路,`Unsupported` 走本地 `SUMMARIZATION_PROMPT`。
- **触发**:配置 `model_auto_compact_token_limit`(token 数)+ `model_auto_compact_token_limit_scope`(`total` | `body_after_prefix`);阈值近似 ~90%(issue #9773:400k 窗约 244.8k 触发;#16068:400k 窗 360k)。检查时机在 **turn 边界**,不在 tool 输出中途(#16033)。
- **摘要 prompt 原文**(`prompts/templates/compact/prompt.md`):
  > "You are performing a CONTEXT CHECKPOINT COMPACTION. Create a handoff summary for another LLM that will resume the task. Include: Current progress and key decisions made / Important context, constraints, or user preferences / What remains to be done (clear next steps) / Any critical data, examples, or references needed to continue."
- **摘要前缀**(`summary_prefix.md`):"Another language model started to solve this problem and produced a summary of its thinking process…" —— **把摘要框定为"上一个模型的交接"**,要求续接而非复述,降低注入面。
- **保留策略**:重建历史 = 初始上下文(AGENTS.md/env 重注入)+ 摘要 + **近期用户消息 ≤ 20K**(从新到旧装满,非用户消息不逐字保留)。摘要 turn 以 `ContextCompaction` synthetic item 落 history,`history_version` 递增。
- **模型**:会话主模型(无廉价回退);`compact_prompt` 可覆写。

**借鉴点**:① "handoff to another LLM" 前缀话术;② 重建公式"重注入初始上下文 + 摘要 + ≤20k 近期用户消息(按用户轮选择,而非 assistant/tool 消息)";③ 阈值配置三件套 + 可换 prompt。

## 2. Google Gemini CLI(TypeScript)

- **触发**:`DEFAULT_COMPRESSION_TOKEN_THRESHOLD = 0.5`(旧版 0.7,版本差异以 main 为准);不做"丢旧 turn"式 windowing,超阈值直接压缩。
- **算法**:保留最近 30%、压缩前 70%;**切分点必须落在"无 function response 的 user message"边界**。喂 summarizer 时待压部分装得下就用原始历史,装不下用截断版。
- **摘要**:以 `<state_snapshot>` 为核心的 systemInstruction,要求先 scratchpad 推理再产出快照,**追加第二回合 "probe" 让模型自检遗漏后重新生成**(两段式)。摘要模型走专用 alias(`chat-compression-3-pro` 等,角色 `UTILITY_COMPRESSOR`,**可走廉价 utility 通道**)。
- **失败降级**:摘要失败 → `CONTENT_TRUNCATED`:不丢 turn,只把旧 tool output 截到最后 30 行并存临时文件引用(预算 50K);粘性 `hasFailedCompressionAttempt` 防反复失败。
- **持久化**:压缩结果只替换内存 API history、**不持久化**,resume 时被忽略(issue #20803 —— 反面教材,正是本项目要避开的坑)。
- `/compress`(别名 summarize/compact)手动触发,`force=true`。

**借鉴点**:① 生成 + probe 自检两段式摘要;② 优雅降级链(摘要失败 → 只截旧 tool output 而非丢 turn)+ 粘性失败标志;③ 摘要走专用 utility 模型角色,计费解耦。

## 3. opencode(sst,TS)

- **V1**:触发 `count >= usable(input)`(含 cache 读写);**分层**:先 prune 工具输出(倒序保留 40k 的 tool output,更旧的原地擦除标记 `time.compacted`)再全量摘要;尾部保留 `clamp(25% usable, 2k, 15k)`;摘要落库为**真实持久 assistant message**(`mode:"compaction", summary:true`)+ 配对 user message。
- **V2(现行)**:预检 `estimated tokens > context limit - max(requested output, buffer=20000)`;配置仅 `auto / keep.tokens=15000 / buffer=20000`。尾部 keep 区内**非逐字**(tool output 限 2000 chars)。摘要 ≤4096 输出 token、禁工具、会话自身模型;模板固定节:Objective / Important Details / Work State(Completed/Active/Blocked)/ Next Move / Relevant Files;**有前一份 summary 时做增量合并**(`<prior-summary>` + "conversation wins" 冲突规则)。
- **溢出兜底**:provider 报 context overflow → compact 后**重试该 step 一次**(即使 auto=false)。
- **持久化哲学**:持久 session 消息不删除 —— "落库无损、上下文有损"。完成后 checkpoint 作为历史上下文呈现(非新指令)。

**借鉴点**:① 预检估算 + 真实溢出错误"压缩重试一次"双保险;② 摘要增量合并规则(显式冲突裁决 "conversation wins"),多次压缩不滚雪球;③ 摘要作为普通持久消息落库、可审计;④ 尾部保留区内 tool output 照样截断,防"尾部免死"膨胀。

## 来源

- Codex:github.com/openai/codex — `codex-rs/core/src/compact.rs`、`core/src/tasks/compact.rs`、`prompts/templates/compact/{prompt,summary_prefix}.md`;issues #9773/#16033/#16068
- Gemini:github.com/google-gemini/gemini-cli — `packages/core/src/context/chatCompressionService.ts`、`docs/reference/configuration.md`;issues #12068/#20803
- opencode:github.com/sst/opencode(dev)— `packages/opencode/src/session/{compaction,overflow,summary}.ts`;opencode.ai/v2/docs/compaction
