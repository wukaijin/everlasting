# memory 指令块窗口治理(度量 + 分级注入 digest)

> 状态:PRD 定稿(2026-08-15,基于 design.md 方案 A)。规划期调研见 `research/code-map-20260815.md`。

## Goal

把 memory 指令块(CLAUDE.md/AGENTS.md 4 层)纳入与 messages / tools[] 并列的上下文窗口治理:先落度量(turn_trace.memory_token),再以"分级注入 + 按需拉取"(AGENTS.md 全量、CLAUDE.md 目录 digest + `load_memory_sections` 工具)把首轮注入的 memory token 从 ~7-8k 压到 ≤2.5k,agent 行为不回归,prompt cache 断点与 session 内前缀稳定不劣化。

## Background

- 数据(2026-08-14 C7 live 烟测):首轮 context_input=17602,tools=6773(38.5%),memory ~7-8k(≈42%)。C7D 落地(08-14,`bcf4187`)后 tools 降至 3677(26%),**memory 反超为首轮窗口最大单项**。
- 本机实测(08-15,code-map §4):repo CLAUDE.md 27,810B(~7k tok)是大头;repo AGENTS.md 4,677B(~1.3k);user CLAUDE.md 36B;user AGENTS.md 缺失。
- 现状:`build_instructions_blocks` 把 4 层全量拼进会话头部合成 user 消息(banner 块带唯一 cache 断点);session 内内容固定(mtime fence)→ 前缀天然稳定。
- 与 tools 治理差异:memory 是行为指导,不能机械压缩;选"目录 → 按需全文"(同构 C7D 渐进披露),不选相关性动态裁剪(信号弱 + 破前缀稳定,详见 design §2 选型表)。

## Requirements

### WP1 度量先行(R1,复刻 C7 R1 模式,独立成 PR)

- `turn_trace` 加 `memory_token INTEGER` 列(schema.rs 建表 + `add_turn_trace_column_if_missing` 幂等 backfill);`db/trace.rs` 行类型/list 查询/`upsert_turn_trace_token` 扩参。
- init.rs 注入处对实际注入 blocks 跑 cl100k 存 `LoopInit`,drive.rs Done 事件与 tools_token 同点落库(非致命 warn 口径)。**度量只覆盖 init.rs 路径**(worker 的独立注入点 `subagent/prompt.rs:63` 不动,其 turn_trace 行该列留 null,与 C7D worker 豁免对称——见 design §3.5a)。
- 前端 `turnTrace.ts` + `TurnCard.vue` memory cell;占比 = memory_token / context_input,**不 double-count**。
- `turn-smoke.sh` 输出 memory_token / 占比 → 拿全量基线。

### WP2 分级注入 digest(方案 A,详见 design.md §3)

- tier 规则:AGENTS.md 层 Full;CLAUDE.md 层 Digest(fence-aware 切节 + 目录 = 节标题 + ≤120 chars 首句);层 tokens ≤600 的小文件豁免(直接 Full)。
- `load_memory_sections` 元工具:参数 `sections: string[]`(`CLAUDE.md` / `CLAUDE.md#节标题` / `["all"]`),返回节原文普通文本;不进 `builtin_tools()`(侧挂 append,同 `load_tool_schemas_def` 先例);未知节名报错附可用节列表;read-only silent-allow。
- 粘性:`MemoryDigestRegistry`(session_id → loaded 节集),已加载节并入下个 request 的该层注入;`delete_session` 清空。
- 开关 `memory_digest_enabled`(缺省 on,best-effort 读);gate = 开关 && !worker && !群聊(与 C7D 豁免口径一致)。
- 目录块尾附一行调用指引(同 C7D stub description 先例)。

### WP3 验证与沉淀

- 真机行为验证(AC3)+ cache 率对比(AC4)。
- spec 沉淀:`memory.md` 加 digest/tier Scenario 段(或 Decision 注记);`token-usage-tracking.md` 加 memory_token 口径(与 tools_token 同款 no-double-count 说明);BACKLOG §3.1 进展 + ROADMAP §1.2 行。

## Constraints

- 不变量(违者即错):I1 banner 块唯一 cache 断点;I2 session 内注入内容固定(粘性重算仅由"文件 mtime 变"或"拉取节"触发);I3 `load_for_session` 恒返 4 元素、banner-first 块序、`<primary>`/`<reference>` 包裹语义不变。
- 不动:autonomous-memory 体系、`read_memory_*` IPC、灰阶无关;digest 纯机械生成,**禁止 LLM 生成摘要**(非确定性破 I2)。
- 群聊 / worker v1 豁免(行为与现状逐字节一致);多 provider 无关(cl100k 近似口径声明沿用)。
- 新工具/hex/常量遵循既有 spec 惯例;memory 文件内容本身不动(用户资产)。

## Acceptance Criteria

- [x] AC1 基线:turn-smoke 实测(digest-off)memory_token=**10124** / 占首轮 **72%**(高于 08-14 估算 ~7-8k:cl100k 对 CJK 实际计数 + wrappers)——`research/live-smoke-20260815.md`。
- [x] AC2 收益:digest-on 首轮(无拉取)memory_token=**2080 ≤ 2500**(-79.5%);首轮 context 14079→7421(**-47%**);CLAUDE.md 单层降幅 >90%。
- [x] AC3 行为不回归:**代理验证通过**——只存在于 CLAUDE.md 的问题(Tech Stack 节)模型主动 `load_memory_sections` 拉全文并逐条遵循(thinking 明确识别 digest 形态);可由 AGENTS.md 回答的问题不拉节直接答、无编造。边界:改代码/跑命令型多轮任务的规范遵循观察留给日常真机使用(机制侧已闭环)。
- [x] AC4 cache 不劣化:digest-on 双轮第二轮 cache 率 **99.8%**(7494/7510)vs digest-off 99.7%(13952/13991)——不劣化;拉取当轮一次性 miss 由 design §3.4 论证(粘性后重新稳定)。
- [x] AC5 开关:digest-off 与 legacy 注入**逐字节一致**(单测 `digest_off_is_byte_identical_to_legacy` 锁);worker 路径 `subagent/prompt.rs` 零改动(继续调 legacy 入口);群聊经 init 路径被 gate 短路。
- [x] AC6 测试:`cargo test -p everlasting --lib` **1722 通过**(1 挂为预存 flake `agent_loop_dispatch_subagent_guard…`,干净 main 上同挂,临时 worktree 复现,非本任务回归);`pnpm test` **1039 全绿**(65 文件,含 memory cell 新测试)+ `pnpm build`(vue-tsc)通过;clippy 仅 1 条预存警告(08-10 blame,trace.rs 空串比较,非本任务)。

## Notes

- PR 划分与执行清单见 `implement.md`;技术选型与 cache 影响分析见 `design.md`;行号基线见 `research/code-map-20260815.md`(实现前重扫)。
- 拒绝方案记录(防翻烧饼):相关性动态裁剪(信号弱/破 I2)、trigger_key 工具前召回(对象混淆)、手动瘦身文件(不治本)——见 design §2。
