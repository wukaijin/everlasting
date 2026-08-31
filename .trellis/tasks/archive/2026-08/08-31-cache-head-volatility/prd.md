# 提示词头部易变注入下沉:消除 OpenAI 路径全量缓存失效

## Goal

会话状态类易变内容不再位于 messages 头部,使 OpenAI 兼容路径(DeepSeek 等严格字节 0 前缀缓存、无 cache_control 断点)在状态迁移 / 新用户请求 / loop 提示时不再全量重付 28 万级 token 的 prefill。

## 问题与影响(取证见 `research/evidence-cache-head-volatility.md`)

session `d6728b3a`(08-31,deepseek-v4-flash 经 api.wukaijin.com)~110 轮缓存正常命中,但:

- **seq 435**(280,368 input / 678 output,first token 14.5s)cache_read=0:09:41:11 模型调 `request_task_state_transition`,**workflow breadcrumb 内容翻转**(task in_progress → planning/no-task),而 breadcrumb 每轮被 push 到 **messages[0] 块列表头部**([inject.rs:376](../../../app/src-tauri/src/agent/workflow/inject.rs))→ 字节 0 分叉 → 全量 miss;
- **seq 437**(281,165 / 257)cache_read=0:09:41:59 用户发第二条消息 "commit",**新 chat 请求 init 重读 instruction 文件**(AGENTS.md/CLAUDE.md,插在 messages[0..1],[init.rs:662-675](../../../app/src-tauri/src/agent/chat_loop/init.rs)),而 agent 在 loop 中期(09:12:56/09:13:31)自己改过这两个文件 → 头部合成块与已缓存前缀不同 → 全量 miss;
- seq 285 同为 0 命中,但归因存疑:同一时刻有 tools_count=0 的并发辅助请求(疑似 worker dispatch 前的 truncate_summary 摘要调用,无 tools、共享大段历史),疑似挤占上游缓存条目;loop-hint 已排除(注入在 result 消息块头 = 对话尾部,不破坏前缀,openai.rs 的 wire order-guard 注释可证);
- 次生影响:同机制下 **mid-session commit 会改 system prompt 的 head_sha**(RULE-A-005 每轮刷新)→ 下轮全量 miss(本 session 未触发,但同设计)。

Anthropic 路径有 cache_control 断点保护 instruction 块,头部变化代价小;OpenAI 兼容路径 `to_wire` 丢弃 cache_control,头部不可变是唯一杠杆。

## Requirements

- **R1 breadcrumb 下沉**:`append_workflow_breadcrumb` 的注入位置从 messages[0] 头部移到本轮对话尾部(逐轮追加、语义上是"当前状态提醒",天然属于尾部);worker 的 `append_delegation_template` 同理评估。
- **R2 loop-hint 维持现状**:已确认注入在对话尾部(result 消息块头),不是缓存杀手,不动。
- **R3 init 重读语义**:instruction 文件(AGENTS/CLAUDE × User/Project)在 **session 生命周期内冻结**(init 读一次),或改为尾部注入——二选一,由 design.md 评审定;默认倾向冻结(语义损失最小:agent 改 instruction 期望下个 session 生效,与本仓实际用法一致)。
- **R4 head_sha**:保留 RULE-A-005 的每轮刷新语义,但评估把 head_sha 从 system prompt 移到尾部状态块(与 breadcrumb 同位),消除 mid-session commit 的全量 miss。
- **R5 辅助调用缓存干扰调查**:确认 tools_count=0 并发请求的调用方(疑似 `truncate_summary`),评估其共享大段历史是否挤占主 loop 缓存条目;若确认,评估降低其 prompt 规模或与主 loop 隔离的方案。
- **R6 可观测性**:turn_trace 已有 per-turn cache_read;补一条 daemon 日志(或 trace 事件)在"头部注入内容变化"或"cache_read 相对上轮 input 骤降 >50%"时打点,便于回归验证与告警。
- 不破坏 Anthropic 路径现有 cache_control 布局;群聊/worker 的 S-B guard 语义(禁止头部插合成消息)保持。

## Acceptance Criteria

- [ ] 回归单测:同一会话序列下,状态迁移轮 / 新用户请求轮的 wire messages 前缀(头部合成块)字节不变(breadcrumb/hint 变化只出现在尾部)。
- [ ] 实测:`scripts/turn-smoke.sh` 或等效两轮连跑,第二轮 cache_read 不为 0(走真实 provider 时)。
- [ ] workflow 状态迁移 E2E(既有测试)不回归;群聊、worker dispatch 测试不回归。
- [ ] `cargo test -p everlasting --lib` 全绿。

## 非目标

- 上游聚合路由的缓存保留/节点亲和(154,112 部分命中回退问题,harness 不可控)。
- Anthropic 路径缓存策略调整。
