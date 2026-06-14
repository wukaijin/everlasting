# thinking 块展示 + 持久化

## Goal

让 LLM 的扩展思考（Anthropic extended thinking，adaptive 模式）能在 UI 上展示给用户，同时完整存档到 DB（包含 `signature` / `redacted_thinking.data`），并能在 session 切换后正确回放。后续轮次 history 必须带回 thinking 块，否则 Anthropic API 返回 400。

## Decisions (ADR-lite)

### D1: 全局开启，无 per-session / per-request 开关

**Context**: MVP 简化 UX，所有 session 共享同一行为。
**Decision**: thinking 始终在请求里发，DB 不存 toggle 状态。env 只有一个 `LLM_THINKING_EFFORT` 控制 effort 级别（默认 `high`），无 kill switch。
**Consequences**: 简单；但如果实际 API 不支持 thinking，请求会 400（用户接受这个风险，先发出去试）。

### D2: UI 默认折叠

**Context**: thinking 经常比文字回复还长，全展开会撑高 chat 流。
**Decision**: 每条 assistant 消息里的 thinking 区默认折叠，header 类似 "💭 Thought for N tokens" 一行小条，点 `<details>` 展开看完整内容。折叠状态 in-memory（不存 DB），刷新会重置。
**Consequences**: 跟 Claude.ai / Claude Code 习惯一致；折叠状态不持久化。

### D3: Adaptive 模式（无 budget_tokens）

**Context**: 用户记得有 adaptive thinking，2026 年 Opus 4.7/4.8 已上线自适应，模型自决思考量。
**Decision**: 请求里发 `thinking: { type: "adaptive" }`，不传 `budget_tokens`。`display: "summarized"` 显式设以保证 thinking 文字流到 UI（不被 omitted 模式吞掉）。
**Consequences**: Opus 4.7/4.8/4.6 / Sonnet 4.6 全部支持；Sonnet 4.5 / Opus 4.5 / Haiku 等老模型不支持 adaptive，需用 manual 模式（但用户当前使用场景不在这些模型上）。

### D4: max_tokens 1024 → 16384，effort = "high"

**Context**: adaptive 模式无 budget，但 max_tokens 必须覆盖 thinking + 实际输出。1024 太低撞上限。effort = high 是 Opus 4.7+ 编码场景官方推荐。
**Decision**: `LLM_MAX_TOKENS` 默认值 1024 → 16384。`LLM_THINKING_EFFORT` 新增，默认 `high`。
**Consequences**: 简单任务浪费约 8k token 预算，但响应不会被截断；env 可覆盖。

### D5: 不需要 kill switch

**Context**: 用户接受 API 不支持 thinking 会 400 的风险(`<your-anthropic-compat-host>` proxy 应当兼容)。
**Decision**: 不加 `LLM_THINKING=off` 开关。代码里 thinking 始终发送，失败的话由用户改代码或换 API。
**Consequences**: MVP 最简单；生产环境真要关掉得改代码。

## Requirements

### 后端 (Rust)
- `ContentBlock` 枚举加 `Thinking { thinking: String, signature: String }` 和 `RedactedThinking { data: String }` 变体
- `ChatEvent` 枚举加 `ThinkingDelta { text: String }` 和 `SignatureDelta { signature: String }` 变体
- `ChatRequest` 加 `thinking: Option<ThinkingConfig>` 字段，`ThinkingConfig::Adaptive { display: String, effort: String }`（serde tag "type" = "adaptive"）
- `LlmConfig` 加 `thinking_effort: String` 字段，从 `LLM_THINKING_EFFORT` env 读，默认 "high"
- `LlmConfig::from_env()` 加 max_tokens 默认 1024 → 16384
- `BlockState` 加 `Thinking { json_buf: String }` 分支（SSE 流时 buffer signature）
- SSE parser 处理：
  - `content_block_start` block type = `"thinking"` / `"redacted_thinking"`
  - `content_block_delta` delta type = `"thinking_delta"` / `"signature_delta"`
  - `content_block_stop` 关闭 thinking block 时发 `SignatureDelta`（如果有 buffered signature）+ `EndThinking` 内部信号
- `MessageContent::to_text()` 不动（thinking 不算 "text"）
- 持久化：assistant blocks 里有 Thinking/RedactedThinking 时一并持久化到 DB
- 持久化时 `text` denormalized 字段**不包含** thinking 文字（保持现状）
- Agent loop 把 `ChatEvent::ThinkingDelta` 转发到前端 `chat-event` channel

### 前端 (TS / Vue)
- `ChatMessage` 加 `thinking?: string` 和 `thinkingSignature?: string` 字段（in-memory 状态）
- `ContentBlockFromDb` 加 `thinking` / `signature` / `data` 字段
- `ContentBlockPayload` 加对应的 union 变体
- `ChatEventPayload.kind` 加 `"thinking_delta"` / `"signature_delta"`
- `handleChatEvent` 处理 thinking_delta（append to `last.thinking`）和 signature_delta（设 `last.thinkingSignature`）
- `rehydrateMessages` 从 `m.content` blocks 解析出 thinking + signature
- `toPayloadContent` 把 thinking blocks 带过去给 LLM（assistant messages 必须有完整 signature）
- `ChatWindow.vue`:
  - assistant message 在 tool cards 上面、bubble 上面加一个 thinking 区（`<details>` 默认折叠）
  - header 显示 "💭 Thought for N tokens"（用 `thinking.length / 4` 估算 token 数）
  - 展开看 `thinking` 文字
  - rehydrated 消息也正确显示

### GLM 兼容层
- 不做特殊处理：项目用 Anthropic schema 发请求，thinking 字段按 Anthropic 规范发
- HACKING-llm.md 加一条 note: "GLM Claude-compat 端点对 thinking 字段的转译行为未官方文档化，需真机测试；如有问题改用 OpenAI-compat 端点 + `reasoning_content`"

## Acceptance Criteria

- [ ] 给 Claude Opus 4.7 / 4.6 / Sonnet 4.6 发请求，UI 看到 thinking 摘要 + 文字回复
- [ ] 切 session 再回来，thinking 内容正确还原（DB 持久化 OK）
- [ ] 切 session 后再发一条消息，LLM 不报 400（signature 带回 OK）
- [ ] 工具调用 + thinking 同时启用：tool_use 之前的 thinking 完整回传，agent loop 不 400
- [ ] redacted_thinking 块不丢失（在多轮里正确回传）
- [ ] 折叠状态 UI 正常工作，点开看 thinking 全文
- [ ] `cargo test` 全过；新增 thinking 相关单测
- [ ] `pnpm build` 通过
- [ ] `pnpm tauri dev` 起来手测：观察 thinking 流式 + 折叠 + 切 session 行为

## Definition of Done

- 后端：types.rs / client.rs / lib.rs 改完
- 前端：chat.ts / ChatWindow.vue 改完
- 单测：`ContentBlock` / `ChatMessage` 的 thinking 序列化 round-trip
- HACKING-llm.md 加 GLM 兼容层 note
- 42 个旧测试 + 新增测试全过
- `pnpm build` 通过

## Out of Scope (MVP)

- per-session thinking toggle（DB metadata 不加）
- per-request thinking toggle（前端不暴露）
- thinking kill switch（env LLM_THINKING=off 不做）
- effort 动态调整（固定 env）
- thinking 内容搜索 / 导出
- 折叠状态持久化
- 多个 Provider 适配（只走 Anthropic schema，GLM 兼容层如不支持 fallback 暂时无解）
- 2026 年 GLM 改用 `reasoning_content` 路径

## Technical Notes

### 关键文件
- `app/src-tauri/src/llm/types.rs` — ContentBlock, ChatRequest, ChatEvent 改造
- `app/src-tauri/src/llm/client.rs` — BlockState, SSE parser, ChatRequest 构造
- `app/src-tauri/src/lib.rs` — agent loop event 转发
- `app/src-tauri/src/db.rs` — persist_turn 不需改（JSON content 自然支持）
- `app/src/stores/chat.ts` — types, events, rehydrate, toPayloadContent
- `app/src/components/ChatWindow.vue` — thinking 区块
- `docs/HACKING-llm.md` — GLM 兼容层 note

### 关键约束（来自研究）
- `signature` 必须原样回传、保持顺序、不可解析
- `redacted_thinking.data` 必须原样回传
- `display: "omitted"` 时**不流 thinking_delta**，UI 看不到文字 → 我们强制 `summarized`
- max_tokens 必须 ≥ 实际输出，撞上限会 `stop_reason: "max_tokens"`
- Anthropic 端 `effort` 跟实际 thinking token 数不是精确换算（"behavioral signal"）
- 客户端 `max_tokens > 21,333` 官方 SDK 强制 streaming（reqwest 流式本来就开了，无影响）

### 研究引用
- [`research/anthropic-thinking-api.md`](research/anthropic-thinking-api.md) — 完整 Anthropic thinking API 2026 现状
