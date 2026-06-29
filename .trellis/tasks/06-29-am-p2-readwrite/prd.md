# P2: 手工读写闭环 — session 开始召回 + remember tool + UI

> child `06-29-am-p2-readwrite` · parent [`06-29-autonomous-memory`](../06-29-autonomous-memory/prd.md) · 详见 [spike-007 §3/§4](../../../docs/spikes/007-agent-autonomous-memory-plan.md)
> 前置:P1(消费 `search_memories_fts` / `insert_memory` / `list_memories` / `delete_memory`)。**本阶段结束 = 最小可见价值**:手写一条记忆 → 新 session 看到注入。

## Goal

打通"手工写入 + 背景召回"的最小闭环,不依赖任何自动化(P3/P4):
- **写**:主 agent `remember` tool(也支持用户 UI 手写)→ `insert_memory`(candidate)
- **读**:session 开始用 FTS5 召回 preference/fact,注入 synthetic user message
- **UI**:MemoryPreview 扩展,查看/删除 runtime memories

## In Scope

- `tools/remember.rs` + 注册到 `builtin_tools()`(入参:title/content/tags/kind/scope;pitfall 的 trigger_key 可选)
- session 开始召回注入(接入点 A,`chat_loop.rs:537 build_instructions_blocks` 后):调 `search_memories_fts` → ephemeral ContentBlock,token ≤500,同分按 created_at 固定排序避击穿 cache
- system prompt"何时 remember"段(吸收 spike-005 §3):教 LLM 何时记 + 不记什么
- 前端:`stores/memory.ts` 加 `fetchMemories`/`deleteMemory`;`components/memory/MemoryPreview.vue` 扩展 runtime memories 列表 + 删除;Tauri commands `list_memories`/`delete_memory`(`commands/memory.rs`)
- 频率控制落地(remember tool 层,补 P1 留的接口位)

## Acceptance Criteria

- [ ] 手写/remember 写一条 preference → `list_memories` 能看到
- [ ] 新 session 用相关关键词 → FTS5 命中该条,注入 instruction blocks(日志/前端可确认)
- [ ] 召回 token ≤500;空结果不注入
- [ ] UI 能查看 + 删除 runtime memories;删除后不再被召回
- [ ] remember 写敏感内容被拒(P1 安全网)、超频被拒
- [ ] cargo test + vitest + pnpm build 全绿

## 代码现状(auto-context 探查,P1 后)

- **注入点 A**:`build_instructions_blocks()` 在 `memory/loader.rs:342`,返 `Vec<ContentBlock>`,`chat_loop.rs:537` 调用,注入 `messages[0]` synthetic user message 带 `cache_control:Ephemeral`。**P2 召回必须追加进同一 synthetic message**(不能另起 message,否则破坏 Anthropic cache breakpoint)。
- **project_id/scope**:`loaded_session.session.project_id`(`chat_loop.rs:376`),scope=`MemoryScope::Project`(`memories.rs:76`)。
- **tool 范式**:`tools/mod.rs:123 builtin_tools()` 返 `Vec<ToolDef{name,description,input_schema}>`,`execute_tool_inner` match name 分发(`tools/mod.rs:272`);模板 `use_skill.rs`/`update_checklist.rs`;Anthropic/OpenAI wire shape 在 provider 层处理(`llm/types.rs:200`)。
- **token 计算**:`memory/tokens.rs:50 count_tokens(text)->u32`(tiktoken cl100k_base),≤500 截断可直接用。
- **召回 query 来源**:`messages.iter().rev().find(role==User)` + `to_text()`(`chat_loop.rs:652` 已有 last_user_snapshot 范式)。
- **system prompt 组装**:`assemble_system_prompt(mode_prefix,base_prompt)`(`system_prompt.rs:103`)+ `build_system_prompt(...)`(`:56`)。"何时 remember"段加在 build_system_prompt 或独立 synthetic message。
- **前端现状**:`stores/memory.ts` 只管指令文件(layers/contentCache/loadForProject);`MemoryPreview.vue` 显示指令文件列表;`commands/memory.rs` 仅 3 命令(read_memory_layers/read_memory_content/open_memory_in_editor)。P2 加 runtime memories 列表+删除+fetchMemories/deleteMemory。
- **频率控制(修正)**:P1 `insert_memory`(`memories.rs:510`)**实际只有写入安全网,未留频率控制接口位**。spike-005 §4.3 规则"同 turn≤3 / 同 session≤50",P2 remember tool 自补(需在 ToolContext/session 缓存维护计数)。

## 已定决策(brainstorm)

1. **candidate 召回**:P2 session 开始召回 status IN (candidate,active,verified)。保持 remember→candidate 入口不变,P5 状态机落地后收紧到 active/verified。(用户确认)
2. **token≤500 截断优先级**:按 created_at 降序(新优先)。P2 记忆均 candidate、hit_count=0,新记忆更能反映当前上下文;截断到 token 预算用尽为止。
3. **频率控制**:采纳 spike-005 §4.3 —— 同 turn ≤3 次、同 session ≤50 条。remember tool 层维护计数(ToolContext/session 缓存,无状态 tool call 的解法见风险点 5)。
4. **"何时 remember"引导**:采纳 spike-005 §3,加进 system prompt(`build_system_prompt`),实施时定具体措辞。
5. **session 开始召回 scope**:搜两层(scope=None → user + 当前 project),与 `search_memories_fts` 第三语义一致;user scope 记忆跨 project 召回(全局经验,符合 epic"user/project 两级先")。
6. **召回 query 构造**:每 turn 召回,query = 最近 1 条 user message(`messages.iter().rev().find(role==User).to_text()`)。追加在 instructions blocks 之后(同一 synthetic message),prefix 不变 → Anthropic cache 不受影响;FTS5 查询毫秒级,每 turn 一次可接受。

## Open Questions

(全部已解决,见上方「已定决策」)

## Technical Approach(方向,实现细节实施时定)

- 注入位置:checklist injection 之后、provider.send 之前(吸收 spike-006 §4.4);ephemeral 不持久化到 messages
- remember tool 权限:silent allow(写自主记忆不拦,安全网兜底)——与 epic 决策"全自主写"一致,**不走 Tier4 ask**
- FTS5 召回的 query 构造:用最近 user message 文本(实施时定具体取哪段)
- 前端复用现有 `components/memory/` 脚手架

## Implementation Plan (small PRs)

- **PR1 · 写路径**:`tools/remember.rs`(definition + execute→`insert_memory` candidate + 频率控制同 turn≤3/session≤50,ToolContext/session 计数)+ 注册 `builtin_tools()` + `execute_tool` 分发 + system prompt"何时 remember"引导段(`build_system_prompt`)。单测:写入 roundtrip / 安全网拒绝 / 超频拒绝 / scope-project_id 校验。
- **PR2 · 读路径**:`chat_loop.rs` 在 `build_instructions_blocks` 后追加召回 block(`search_memories_fts` scope=None 两层 / status IN candidate,active,verified / query=最近 user / created_at 降序 / token≤500 `count_tokens` 截断 / 追加进同一 synthetic message 保 cache)+ 召回命中 `bump_hit_count`。单测:注入内容 / token 截断 / 空结果不注入 / cache_control 位置。
- **PR3 · 前端 UI**:`commands/memory.rs` 加 `list_memories`/`delete_memory` Tauri commands(project 隔离权限检查);`stores/memory.ts` 加 `fetchMemories`/`deleteMemory` + runtime memories 状态;`MemoryPreview.vue` 扩展 runtime memories 列表 + 删除。vitest。
- **PR4 · 收尾**:`cargo test --lib` + `cargo check` + `vitest` + `pnpm build` 全绿;remember/召回函数转 production caller 后清理 dead_code。

## Decision (ADR-lite)

**Context**: spike-007 设计 session 开始召回只取 status IN (active,verified) 以避免未验证记忆污染召回。但 P2 无晋升机制(active 来自 P4 旁路事件,状态机自动晋升在 P5),P2 remember 写入固定 candidate → 召回排除 candidate 则 P2 手写记忆新 session 永远召回不到,核心 AC 不成立。
**Decision**: P2 session 开始召回 status IN (candidate,active,verified)。
**Consequences**: P2 闭环成立;candidate 噪音风险低(P2 记忆均为用户显式/agent remember,可信度高于 P4 旁路自动写入);P5 状态机落地后收紧到 active/verified,candidate 仅作写入入口状态保留。

## Out of Scope

- 工具执行前召回(→ P3)
- 事件驱动自动写入(→ P4)
- 状态机自动晋升 + 卫生 job(→ P5;remember 写入固定 candidate,不自动升)
- pitfall 类召回(P3 才接 trigger_key;P2 session 开始召回只取 preference/fact)

## 关联

- epic:[`06-29-autonomous-memory/prd.md`](../06-29-autonomous-memory/prd.md) · spike-007 §3/§4 · FTS5+前端吸收自 spike-006
