# A5+ LLM 网络健壮性(重试 / 退避 / SSE 断连边界)

> **source**:[`docs/research/llm-network-resilience-survey.md`](../../../docs/research/llm-network-resilience-survey.md)(本文档不重复协议事实 / 业界调研,只固化需求 / 约束 / 验收)
>
> 本 prd 只放 requirements / constraints / acceptance criteria。retry wrapper 落点、Full Jitter 公式、retry-after 解析、首字节边界判定、C1 取消与 sleep 交互的**精确设计**落 `design.md`;分步执行 + validation 命令 + 回滚点落 `implement.md`。

## Goal

在 provider 层补**网络重试 + 指数退避(Full Jitter)+ retry-after 解析**,堵长会话 5xx / 429 / 连接 reset / 流断连**整轮重来**的体验缺口(DESIGN §5.1 已留口;A5 错误契约 07-02 已落地)。

**MVP 走"首字节前重试"安全边界**(对齐 Claude Code "可见输出前重试" 标杆);但因 everlasting 的 tool 执行在 SSE 流结束之后(`chat_loop.rs:1490-1501` + `1655-1661`),即便不做"流中途重试"也**绝对安全**(tool 没跑,重发 = 继续即可),故 everlasting 的边界比 Claude Code 更彻底——**不需要 idempotency key / dedup 表**。

## 要解决的具体场景(Requirements)

### R1 · 可重试错误整轮重发(必须)

- **现状**:任何 `LlmError::Network` / `Server(5xx)` / `RateLimit(429)` → `ChatEvent::Error` → break → ERROR_MARKER → 用户手动重发(`chat_loop.rs:1592-1594`)。
- **要**:这三类错误在 retry 预算内**自动整轮重发** provider 请求(messages 不变,tool 必然未执行)。
- **复用**:`LlmError` 5 类 + `classify_error_response`(`error.rs:96-183`)已正确归类,**不改分类逻辑**。

### R2 · 不可重试错误立即失败(必须)

- **要**:`Auth`(401/403)/ `InvalidRequest`(4xx 非 429)**不重试**,直接 `ChatEvent::Error`(现状行为,不变)。
- **心智**:重试只对**瞬时**错误有意义;认证 / 参数错误重试 N 次还是错,白烧 token + 时间。

### R3 · SSE 断连边界:首字节前重试,首字节后保留 partial(核心安全决策)

- **现状**:SSE 流中途断连 → `Network("stream read: ...")` → break → **执行已收到的 partial tool_use** + ERROR_MARKER(`chat_loop.rs:1655-1661`)。无论断连点在首字节前还是后,行为相同。
- **要(MVP)**:
  - **首字节前断连**(还没 emit 任何 `ChatEvent` 到 sink,即连接建立后 / 第一个 SSE event 解析前断)→ **自动重试**(R1)。
  - **首字节后断连**(已 emit 过 `Start` / `Delta` / `ToolCall` 等)→ **不重试**,保留现状(执行 partial tool_use + ERROR_MARKER),让用户决定重发。
- **为什么这样安全**:research §5.3 确认 partial `ToolUse`(`content_block_stop` 未到达)**根本不会 emit**;而完整 `ToolCall` 已 emit 后的断连,tool 即将被执行,重发有重复执行风险(Cursor reconnect 后 tool call 2→11 膨胀就是反面教材)。首字节是天然分水岭。
- **边界 B(SSE 流中途也重试)= follow-up,不在 MVP**:需要追踪流内"是否已 emit ToolCall" + 处理前端已渲染 delta 的回滚,复杂度高;且 everlasting 不做也安全。

### R4 · Full Jitter 指数退避(必须)

- **要**:退避用 **Full Jitter**(AWS Architecture Blog 共识最优):`sleep = random(0, min(cap, base · 2ⁿ))`,n = 已重试次数。
- **参数(推荐,进 planning 可调)**:`base = 0.5s`,`cap = 30s`,`max_retries = 3`(总请求 ≤ 4 次)。
- **为什么不用纯指数退避**:research §5.3 / AWS blog —— 纯指数退避调用聚类、对服务端冲击大、总耗时也长,是公认输家。OpenAI / Anthropic SDK 的"0.75-1.0 弱抖动"也不是 Full Jitter;AWS SDK 才是。

### R5 · retry-after 优先解析(必须)

- **要**:服务端给的 retry-after 优先于 Full Jitter,但**封顶 60s**(对齐 Anthropic / OpenAI SDK 源码 `_calculate_retry_timeout`)。
- **覆盖三家格式**:
  - Anthropic `retry-after`(秒整数)+ `retry-after-ms`(毫秒,非标但 SDK 解析)
  - OpenAI `x-ratelimit-reset-requests` / `x-ratelimit-reset-tokens`(Go duration 字符串,如 `6m0s` / `1s` / `500ms`)—— OpenAI 几乎不发标准 `retry-after`,这是双 provider 必经路径,**MVP 必须解析**。
- **>60s 的 retry-after**:忽略,回落 Full Jitter(对齐 SDK;防服务端给 120s cooldown 导致死等)。

### R6 · 硬上限熔断(必须,吸取 OpenCode 教训)

- **要**:双限熔断 —— ① `max_retries` 次数上限(推荐 3);② **总时间预算上限**(推荐 60s,含所有 sleep)。任一触达即停止重试,落 ERROR_MARKER。
- **反面教材**:OpenCode `SessionRetry.policy()` 对 429/529 无限重试无熔断,session 死几小时(research §4.2)。**everlasting 必须有硬上限**。

### R7 · 重试 sleep 期间响应 C1 取消(必须)

- **现状**:C1 取消在 SSE 消费阶段响应(`chat_loop.rs:1417-1421` biased select)。
- **要**:重试的退避 sleep 期间,用户取消应**立即响应**,不等满 sleep。实现走 `tokio::select! { _ = token.cancelled() => cancel, _ = sleep => retry }`。
- **心智**:重试不能绑架用户——sleep 30s 时用户想停,必须能停。

### R8 · 前端可见重试事件(必须,否则用户以为卡死)

- **要**:每次进入重试前,emit 一个结构化 `chat-event` 给前端(新子类型,如 `{ type: "retrying", attempt: 2, max_attempts: 3, wait_ms: 2000, reason: "5xx server error" }`)。前端显示"↩ 重试中 2/3,2s 后重发…(原因)"。
- **为什么必须**:退避 sleep 几秒到 30s,前端若无任何信号,用户会以为 agent 卡死 / 重复点发送。

### R9 · 既有不变量保持(回归)

引入重试不能破坏:
- **A4 token 统计 OVERWRITE**:重试成功后只计一次(`db/sessions.rs:397-419`)。
- **C3 压缩幂等**:同一 messages 重复压缩结果不变,重试不误触发。
- **tool 执行时序**:tool 仍在 SSE 流结束后统一执行(`chat_loop.rs:1490-1501`)。
- **LlmError 5 类 + classify**:不改分类逻辑,只在其上加 retryable 判定。
- **C1 取消语义**:取消仍是用户主动(不触发 `had_error`),网络错误仍是系统失败(`had_error`)。

## Constraints(不得违反的不变量)

1. **Provider trait 签名不动**(`llm/provider/mod.rs:66-99` `send() -> Stream`)—— retry 走**外层 wrapper**,Provider 实现专注协议转换,不感知 retry。
2. **tool 执行时序不动** —— 这是"首字节前重试绝对安全"的根基。retry 只发生在 provider.send 阶段,绝不延伸到 tool 执行阶段。
3. **LlmError 5 类不删 / 分类逻辑不动** —— 复用 `classify_error_response`,只加 `is_retryable(&self) -> bool`。**边界细化(见 design §3.6)**:`RateLimit` / `Server` 变体加 `headers: HeaderMap` 字段(供 retry-after 解析),5 类**名称与分类逻辑**不变;`Auth` / `InvalidRequest` / `Network` 不带 header。
4. **C1 取消优先级不动** —— `token.cancelled` 在 biased select 第一位,重试 sleep 也必须让位(R7)。
5. **SSE 状态机不改** —— `sse.rs` 的 `reset()` 仍 `dead_code`,MVP 不依赖续传。
6. **不引入重试框架依赖**(`tower` / `reqwest-retry` / `tokio-retry`)—— 自研 retry loop,契合项目"自研 SSE / 自研 Provider trait"风格。
7. **不改 DB schema** —— 无新表 / 新列(retry 状态是 loop-local,不持久化)。
8. **AuditKind 不加新类** —— 重试事件走 `chat-event` 前端通道,不入审计表(它是 transient UX 信号,不是 security-relevant 事件)。

## Acceptance Criteria(回归测试矩阵)

`cargo test`(带 `PKG_CONFIG_PATH`)全绿,且覆盖以下矩阵(落地为 `llm/retry.rs` 内联 `#[cfg(test)]` + 扩展 `MockProvider` + agent loop 集成测试):

- [x] **5xx 重试成功**:`MockProvider` 前 2 次返 `Server(503)`,第 3 次成功 → 最终成功,前端收到 2 条 retrying 事件。
- [x] **429 + retry-after**:返 `RateLimit` + `retry-after: 5` → sleep ≈ 5s(非 Full Jitter)后重试。
- [x] **429 + OpenAI x-ratelimit-reset-requests**:header `x-ratelimit-reset-requests: 6m0s` → cap 60s,sleep 60s(封顶)后重试。
- [x] **网络超时 / connect reset(connect 阶段)** → 重试。
- [x] **SSE 首字节前断连**(`Stream` 第一项即 `Err(Network)`,未 emit 任何 ChatEvent)→ 重试。
- [x] **SSE 首字节后断连**(已 emit `Start` / `Delta`,第 N 项 `Err`)→ **不重试**,ERROR_MARKER + 执行 partial tool(现状)。
- [x] **401/403(Auth)** → 不重试,立即 ERROR_MARKER。
- [x] **400/422(InvalidRequest)** → 不重试,立即 ERROR_MARKER。
- [x] **max_retries 耗尽**:连续 `Server(503)` 4 次 → 第 4 次失败落 ERROR_MARKER(不无限重试)。
- [x] **总时间预算熔断**:构造 sleep 累计 >60s 的序列 → 触达预算即停止(不等满 max_retries)。
- [x] **Full Jitter 区间**:`sleep ∈ [0, min(cap, base·2ⁿ)]`(统计多次落区间内,不下溢 / 不溢出)。
- [x] **retry-after 封顶 60s**:`retry-after: 120` → 忽略,回落 Full Jitter。
- [x] **C1 取消在 sleep 中**:重试 sleep 30s 时取消 → 立即响应(< 1s),`cancelled = true` + CANCELLED_MARKER(不触发 had_error)。
- [x] **前端 retrying 事件**:每次重试 emit 一条 `{kind: "retrying", attempt, max_attempts, wait_ms, reason}`(注:wire 字段实为 `kind` 非 `type`,对齐 `ChatEvent` 既有 `#[serde(tag = "kind")]` 标签;`type` 是 prd 早期设想)。
- [x] **token 统计不重复(R9)**:重试成功后,`update_last_turn_usage` 只计一次最终成功请求(OVERWRITE)。
- [x] **既有不变量回归**:现有 `chat_loop` / `provider` / `sse` / `error` 测试全绿(LlmError 分类 / SSE 状态机 / C1 取消 / C3 压缩 / A4 统计)。

## 非目标

- **SSE 流中途(首字节后)断连自动重试**(边界 B,复杂度高,follow-up)。
- **SSE 真正断点续传**(协议不支持,research §2)。
- **idempotency key / dedup 表**(tool 执行时序分离,research §5.4,不需要)。
- **per-provider 自定义 retry 策略**(MVP 全局统一够用)。
- **per-turn retry 可观测 viewer**(归第三档 E2,本任务只 emit 基本重试事件)。
- **连接池 / TCP keepalive 调优**(正交,可顺手配 `pool_max_idle_per_host` 但非本任务核心)。
- **改 DB schema / 加 AuditKind**(constraint 7/8)。

## Notes

- **调研走** `docs/research/llm-network-resilience-survey.md`(协议事实 + 业界标杆 + 反面教材 + 本仓库现状)。
- **待决策项**(进 planning grill,见 survey §7 + design §8):max_retries 3 vs 5 / cap 8s vs 30s / LlmError 变体加 headers 字段是否可接受 / rand 依赖选型 / 重试事件是否入审计 / 前端 retrying 视觉。
- **风险面**:① Full Jitter 的 `random` 在测试中如何 mock(注入 seed 或抽象 `Rng` trait);② retry wrapper 落点(design §1 已决策"外层 wrapper + 独立 retry.rs 模块");③ 前端 retrying 事件与现有 `chat-event` delta 流的时序(不能污染正在重试的那条 assistant message)。
- **完成后**:AC 全绿 + spec(`tool-contract.md` / `agent-loop-architecture.md` / `llm-contract.md` 加 retry 段)+ ROADMAP §1.2 + IMPLEMENTATION §4 ADR → archive。
