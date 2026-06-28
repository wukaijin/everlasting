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

## Technical Approach(方向,实现细节实施时定)

- 注入位置:checklist injection 之后、provider.send 之前(吸收 spike-006 §4.4);ephemeral 不持久化到 messages
- remember tool 权限:silent allow(写自主记忆不拦,安全网兜底)——与 epic 决策"全自主写"一致,**不走 Tier4 ask**
- FTS5 召回的 query 构造:用最近 user message 文本(实施时定具体取哪段)
- 前端复用现有 `components/memory/` 脚手架

## Out of Scope

- 工具执行前召回(→ P3)
- 事件驱动自动写入(→ P4)
- 状态机自动晋升 + 卫生 job(→ P5;remember 写入固定 candidate,不自动升)
- pitfall 类召回(P3 才接 trigger_key;P2 session 开始召回只取 preference/fact)

## 关联

- epic:[`06-29-autonomous-memory/prd.md`](../06-29-autonomous-memory/prd.md) · spike-007 §3/§4 · FTS5+前端吸收自 spike-006
