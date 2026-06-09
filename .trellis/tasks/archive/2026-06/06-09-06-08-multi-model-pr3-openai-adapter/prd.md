# 06-08 multi-model PR3: OpenAI adapter + 跨协议

> Task: `06-09-06-08-multi-model-pr3-openai-adapter`
> Status: planning (brainstorm)
> Parent task: `06-08-multi-model-llm-provider-planning` (PR 切片 K1)
> 基线分支: `06-08-multi-model-llm-provider-planning-pr1-data-layer` (PR1 `f9c5648` + PR2 `0a787ef`)

## Goal

新增 `OpenAIProvider` 实现 `Provider` trait(Chat Completions streaming 协议),
引入 `WireMessage` 中间层做 provider-agnostic 互转,
实现跨协议 capability-aware 静默降级(parent PRD §Q5 H1 决议)。
Anthropic 路径行为跟 PR2 一致(继续跑通 218 个现有测试);
OpenAI 路径允许用户切到 `gpt-4o` / `gpt-4.1` 时真的发请求。

## What I already know(从代码 + parent PRD + PR2 spec 探查)

### 现状(代码层)

- **PR2 已落地**:
  - `Provider` trait + `ProviderCapabilities` + `ProviderProtocol` (provider/mod.rs:64-167)
  - `build_provider` 工厂:anthropic / openai NotImplemented / UnknownProtocol 三分支
  - `AnthropicProvider` impl 完整(anthropic.rs:549-578,搬自原 client.rs)
  - 私有 `LlmConfig`(provider::anthropic 模块私有,经 llm mod re-export)
  - 3 种 pre-flight 文案(api_key 空 Auth / model 找不到 InvalidRequest / provider 找不到 InvalidRequest)
- **`SseParser` 是通用的** (sse.rs):既能解析 `event: foo\ndata: {...}\n\n` (Anthropic) 也能解析 `data: {...}\n\n` (OpenAI,event_type 默认为空)。**PR3 复用 SseParser,不写新解析器**。
- **`ChatRequest` / `ChatEvent` / `ContentBlock` 仍是 Anthropic-shaped** (types.rs,640 行):thinking / redacted_thinking / tool_use / tool_result 都是 Anthropic 风格
- **`ContentBlock` 当前无 `Reasoning` 变体** — parent PRD 说"WireMessage 加 Reasoning block",但 ChatRequest/ChatEvent 是 Anthropic-shaped,**Reasoning 块应只存在于 WireMessage 内部层**,在 target provider 转换时映射到对应字段:
  - Anthropic 路径 → 转 `ContentBlock::Thinking` (若 supports_thinking=true) 或丢
  - OpenAI 路径 → 转 `OpenAIReasoningDelta` 直接转发(原生)
- **build_provider 工厂当前**:`openai` 协议分支返 `NotImplemented("openai")` — PR3 要替换为 `OpenAIProvider::new(...)` 构造
- **`resolve_chat_provider` 已在 spawn 闭包外**(lib.rs,PR2 实现)— PR3 不动 chat 命令入口,只换 dispatch 内部用的 provider 类型
- **PR1 seed 的 OpenAI models**:`gpt-4o` (supports_thinking=false, context_window=128000) + `gpt-4.1` (supports_thinking=false, context_window=1_000_000)

### 4 个 HACKING-llm 坑位置

- 全部保留在 Anthropic adapter 内部(PR2 已搬),PR3 不动
- OpenAI 协议差异(401 / 5xx / max_tokens / stream format)走 OpenAI adapter 自己的 `classify_error_response` 路径

### OpenAI Chat Completions 协议关键差异(从 parent PRD + WebFetch 推断)

| 维度 | Anthropic | OpenAI Chat Completions |
|---|---|---|
| endpoint | `POST {base}/v1/messages` | `POST {base}/v1/chat/completions` |
| headers | `x-api-key` + `anthropic-version: 2023-06-01` | `Authorization: Bearer {api_key}` |
| system prompt | 顶层 `system` 字段 | 第一条 `role: "system"` message |
| tools 格式 | `[{name, description, input_schema}]` | `[{type: "function", function: {name, description, parameters}}]` |
| tool calls | content block `tool_use` (`input` 是 JSON object) | `tool_calls[]` (`arguments` 是 JSON string) |
| tool result | user message 里 `tool_result` block | 独立 `role: "tool"` message,内容是 string |
| stream event | `event: ...\ndata: {...}\n\n` 多 event 类型 | `data: {...}\n\n` 单一格式(event 在 `choices[].delta`) |
| text delta | `content_block_delta` 内的 `text_delta` | `choices[0].delta.content` |
| reasoning | `thinking_delta` 块 | `choices[0].delta.reasoning_content` (o1/o3 系列) |
| tool call delta | `input_json_delta` 累积 JSON | `tool_calls[].function.arguments` 累积 |
| finish | `message_delta.stop_reason` + `message_stop` | `choices[0].finish_reason` + `data: [DONE]` |
| max_tokens | 顶层 `max_tokens` | 顶层 `max_tokens`(兼容)或 `max_completion_tokens`(o1+) |
| thinking | 顶层 `thinking: {type, display, effort}` 块 | `reasoning_effort: "low"\|"medium"\|"high"` 顶层 |

## Assumptions(临时,待验证)

- 假设 1: WireMessage 是 **provider-agnostic 中间表示**,只存在于 `provider/wire.rs` 内部,不入 types.rs
- 假设 2: 降级时机 = **`provider.send` 内,转换 ChatRequest → WireRequest → provider-wire 之前 strip 一次**(整个 message history 一次性)
- 假设 3: 降级**不持久化** — 切回原 model 时 thinking 块仍存在(从 DB 读出的 ChatMessage blocks 完整保留)
- 假设 4: OpenAI 不支持的 `thinking_effort` 字段**不发送**(避免 400);若 model.supports_thinking=true 仍可发 Anthropic thinking 块
- 假设 5: OpenAI tool_calls 累积 JSON 解析需要类似 BlockState 的 state machine(per `tool_call_index` 跟踪),不复用 Anthropic 单一 tool_use 假设

## Open Questions(已收敛)

- [x] **Q1: WireMessage 引入范围** — **完整中间层** `wire.rs` 定义 `WireRequest` / `WireMessage` / `WireBlock` / `WireTool` / `WireCapabilities` / `strip_unsupported` / `chat_request_to_wire` / `wire_event_to_chat_event`(2026-06-09 决议)。Anthropic 和 OpenAI 都走 wire 层互转,代码统一,跨协议一致性最好。
- [x] **Q2: 降级粒度** — **`provider.send` 内 strip**(2026-06-09 决议)。流程:`ChatRequest → WireRequest → strip_unsupported(查 target_caps) → provider wire`。动态、每次重 strip(便宜),不持久化。
- [x] **Q3: Anthropic 是否走 wire 层** — **是**(默认)。`AnthropicProvider::send` 内部也走 `chat_request_to_wire → wire → anthropic-wire converter`,跟 OpenAI 对称。代价:多一次内存转换。收益:跨协议一致性 + 未来加 Gemini/Ollama 时无需重构。

## Requirements(2026-06-09 收敛)

### 1. WireMessage 中间层(`app/src-tauri/src/llm/provider/wire.rs` 新建)

```rust
//! Provider-agnostic wire representation. Both Anthropic and OpenAI
//! adapters convert from `ChatRequest`/`ChatEvent` (Anthropic-shaped)
//! to/from this intermediate form, then to/from the actual provider
//! wire format. The wire module is the single place that knows how
//! to map between protocols.

pub struct WireRequest {
    pub model: String,
    pub max_tokens: Option<u32>,
    pub system: Option<String>,
    pub messages: Vec<WireMessage>,
    pub tools: Vec<WireTool>,
    /// OpenAI-only. None = no reasoning effort requested.
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Clone)]
pub enum WireMessage {
    User { content: String },
    Assistant { blocks: Vec<WireBlock> },
    Tool { tool_call_id: String, content: String },
}

#[derive(Debug, Clone)]
pub enum WireBlock {
    Text { text: String },
    /// Provider-agnostic reasoning. Mapped to:
    /// - Anthropic thinking block (if target supports_thinking)
    /// - OpenAI reasoning_content field (if target is OpenAI)
    /// - dropped (if target supports neither)
    Reasoning { text: String },
    /// Tool call from the model. `input` is already parsed JSON.
    /// Mapped to Anthropic `tool_use` / OpenAI `tool_calls[].function`.
    ToolUse { id: String, name: String, input: serde_json::Value },
    /// Tool result (paired with ToolUse id). Common in both protocols.
    /// Mapped to Anthropic `tool_result` block / OpenAI `role: "tool"` message.
    ToolResult { tool_use_id: String, content: String, is_error: bool },
    /// Anthropic-specific opaque signature blob. Dropped on cross-protocol.
    Signature { data: String },
    /// Anthropic-specific redacted_thinking. Dropped on cross-protocol.
    RedactedThinking { data: String },
}

pub struct WireTool {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}

pub struct WireCapabilities {
    pub supports_thinking: bool,
    pub supports_tools: bool,
    pub supports_streaming: bool,
    pub supports_reasoning_effort: bool,  // OpenAI o1/o3 only
}
```

### 2. 降级函数(`app/src-tauri/src/llm/provider/wire.rs`)

```rust
/// Strip blocks the target protocol can't represent. Pure function — no IO.
/// Called inside `provider.send` right before wire conversion.
pub fn strip_unsupported(
    messages: Vec<WireMessage>,
    target_caps: &WireCapabilities,
) -> Vec<WireMessage>;
```

降级规则(parent PRD §Q5 D5):
- `WireBlock::Reasoning`:
  - target supports_thinking → 保留
  - target supports_reasoning_effort (OpenAI) → 保留
  - 其他 → 丢
- `WireBlock::Signature` / `WireBlock::RedactedThinking`:
  - target 是 Anthropic → 保留(thinking 块需要)
  - target 是 OpenAI → 丢(opaque blob 不可转)
- `WireBlock::ToolUse` / `ToolResult`: 全部保留(两边都支持)
- `WireBlock::Text`: 全部保留

### 3. ChatRequest ↔ WireRequest 转换

```rust
// 入参: Anthropic-shaped ChatRequest → WireRequest (带 reasoning_effort 来自 model.thinking_effort)
pub fn chat_request_to_wire(
    req: ChatRequest,
    system: Option<String>,
    target_caps: &WireCapabilities,
) -> WireRequest;

// 出参: Anthropic-shaped ChatEvent 序列(从 WireEvent 转换)
//      (Stream<WireEvent> 仍存在,AnthropicProvider 跟 OpenAIProvider 都返回 Stream<WireEvent>,
//       顶层 Provider::send 负责把 Stream<WireEvent> → Stream<ChatEvent>)
```

### 4. OpenAIProvider impl

- 路径: `app/src-tauri/src/llm/provider/openai.rs` 新建
- `OpenAIConfig` struct: `base_url`, `api_key`, `model`, `max_tokens` (from model_row, default 16384)
- 构造: `OpenAIProvider::new(OpenAIConfig) -> Self`
- 协议层:
  - `POST {base}/v1/chat/completions` + `Authorization: Bearer {api_key}` headers
  - 请求体: OpenAI Chat Completions schema(`messages` 数组含 system 头,`tools` 数组 `function` 包裹)
  - SSE 流:复用 `SseParser`,空 event 名时按 data-only 模式解析
  - BlockState 风格:跟踪 `tool_call_index → {id, name, args_buf}`,多个 tool call 并行 streaming
  - `Reasoning` 流:直接转发为 `ChatEvent::ThinkingDelta` (因为 ChatEvent 已经是 Anthropic-shaped)
  - `Done`:`choices[0].finish_reason` 映射到 `stop_reason`
  - 错误:复用 `classify_error_response` 5 类错误模式,401/429/4xx/5xx 各自映射
- `impl Provider for OpenAIProvider`: `send` 方法先做 `chat_request_to_wire` → `strip_unsupported` → 转 OpenAI wire → SSE 解析 → `Stream<ChatEvent>`
- 测试:7+ 个 mock 单测覆盖 OpenAI 协议各分支(text / tool_calls / reasoning_content / finish_reason / 4 类错误 / 空响应)

### 5. build_provider 工厂更新

- `openai` 协议分支:`Ok(Box::new(OpenAIProvider::new(OpenAIConfig {...})))`
- `max_tokens` fallback: `model_row.max_tokens.unwrap_or(16384)`
- `thinking_effort`:OpenAI 不发(默认 None),`reasoning_effort` 由 `model_row.thinking_effort` 派生

### 6. AnthropicProvider 不变

- PR2 行为完全保留
- 唯一改动:`impl Provider for AnthropicProvider::send` 内部**也走 wire 层**(chat_request_to_wire → anthropic wire converter)?? 或保持原样
- **决策**(待 Q3 决):Anthropic 是否走 wire 层

### 7. 错误处理

- OpenAI 401 → `LlmError::Auth`
- OpenAI 429 → `LlmError::RateLimit`
- OpenAI 400 (含 `invalid_request_error` code) → `LlmError::InvalidRequest`
- OpenAI 5xx / 502/503 → `LlmError::Server`
- 网络错误 → `LlmError::Network`
- 复用 `classify_error_response` 函数,扩展支持 OpenAI 错误 body 格式(`{error: {message, type, code}}`)

## Acceptance Criteria

### WireMessage + 降级

- [ ] `app/src-tauri/src/llm/provider/wire.rs` 定义 `WireRequest` / `WireMessage` / `WireBlock` / `WireTool` / `WireCapabilities` / `strip_unsupported`
- [ ] `strip_unsupported` 单元测试 6+ case:Anthropic→OpenAI drop signature/redacted_thinking,OpenAI→Anthropic drop reasoning, supports_thinking=false drop thinking, supports_reasoning_effort 保留, ToolUse/ToolResult 全部保留
- [ ] `chat_request_to_wire` 单元测试 4+ case:含 thinking 块 / 含 redacted_thinking / 含 tool_use / 含 system
- [ ] `wire_event_to_chat_event` 单元测试 4+ case:Reasoning → ThinkingDelta, ToolUse → ToolCall, Done → Done

### OpenAIProvider

- [ ] `app/src-tauri/src/llm/provider/openai.rs` 实现 `Provider` for `OpenAIProvider`
- [ ] Chat Completions streaming 协议正确(text / tool_calls / reasoning_content / finish_reason / data: [DONE])
- [ ] BlockState 风格 per `tool_call_index` state machine(并行多 tool call)
- [ ] 错误分类:5 类 LlmError 全覆盖
- [ ] 单元测试 7+ 个 mock 单测
- [ ] `build_provider` 的 `openai` 协议分支返 `Ok(Box::new(OpenAIProvider::new(...)))`

### 行为完全不变(Anthropic 路径)

- [ ] 218 个 PR2 cargo test 仍全过(0 改)
- [ ] Anthropic 请求 URL/headers/thinking 字段 4 块 1:1 不变
- [ ] `get_llm_config` IPC shape 保持
- [ ] 3 种 pre-flight 文案不变

### 跨协议切换

- [ ] 切到 gpt-4o (supports_thinking=false):若历史有 thinking 块,被 strip 丢(不发到 OpenAI)
- [ ] 切到 claude-sonnet-4-5 (supports_thinking=true):OpenAI reasoning_content 块被 strip 丢
- [ ] 切到 gpt-4o:signature / redacted_thinking 块被 strip 丢
- [ ] strip 不持久化(下次切回原 model,thinking 块仍在 DB)

### 验证

- [ ] `cargo test --lib` 全过(目标 230+ pass)
- [ ] `pnpm build` 通过
- [ ] `trellis-check` PASS verdict
- [ ] spec section 加到 `.trellis/spec/backend/llm-contract.md` "Scenario: OpenAI adapter + 跨协议 dispatch (PR3)"
- [ ] `docs/HACKING-llm.md` 加 "OpenAI 协议差异" 章节
- [ ] `docs/IMPLEMENTATION.md` §2.7 + §3 状态更新
- [ ] `docs/BACKLOG.md` §4 v3+ 多角色 bind default model 改成引用本任务

## Definition of Done

- [ ] Tests added/updated(目标 +12 个新测试:6 strip + 4 wire conv + 7 OpenAI mock)
- [ ] `cargo check` + `cargo test --lib` + `pnpm build` 全 pass,0 warning
- [ ] spec 加 PR3 section
- [ ] docs 更新 3 处(IMPLEMENTATION / HACKING-llm / BACKLOG)
- [ ] trellis-check 通过
- [ ] commit message:`feat(llm): PR3 OpenAI adapter + 跨协议 WireMessage`

## Decision(ADR-lite)

### D1: 完整 WireMessage 中间层(2026-06-09)

**Context**: PR3 引入跨协议能力,parent PRD §"PR3 — OpenAI adapter + 跨协议" 提"中间用 provider-agnostic `WireMessage` 表示"。可选项:完整中间层 vs 仅互转函数。

**Decision**: 完整中间层 `wire.rs` 定义 `WireRequest` / `WireMessage` / `WireBlock` / `WireTool` / `WireCapabilities` / `strip_unsupported` / `chat_request_to_wire` / `wire_event_to_chat_event`。Anthropic 和 OpenAI 都走 wire 层互转。

**Consequences**:
- ✅ 跨协议一致性最好(Anthropic 跟 OpenAI 走对称路径)
- ✅ 未来加 Gemini / Ollama 时,只需新加 provider wire converter,wire 层不动
- ✅ 降级规则在 wire 层一处定义
- ⚠️ Anthropic 路径多一次内存转换(便宜,可忽略)
- ⚠️ wire 层是 1 个新文件 + ~200 行(Anthropic 跟 OpenAI 都要写 wire converter)

### D2: 降级在 `provider.send` 内 strip(2026-06-09)

**Context**: parent PRD §Q5 H1 决议"静默降级",但未指定 strip 时机。可选项:send 内 / 切 model 时持久化 / 每 turn。

**Decision**: `provider.send()` 内部 strip,流程 `ChatRequest → WireRequest → strip_unsupported(查 target_caps) → provider wire`。每次重 strip(便宜,纯函数 in-memory),不持久化降级结果。

**Consequences**:
- ✅ 实现简单,无 DB schema 变化
- ✅ 不持久化 → 切回原 model 时 thinking 块仍可读(从 DB 完整 ChatMessage blocks 读)
- ✅ target_caps 来自 `ModelRow.capabilities` 实时读,跟 catalog 同步
- ⚠️ 每次 send 都重 strip 整个 history(纯函数 <1ms,可忽略)
- ⚠️ 跨 provider 切的 session,前几 turn 的 assistant 回复会"丢失 thinking 显示"(在 UI 那一瞬),但切回后恢复

## Out of Scope(明确不做)

- ❌ Ollama / Gemini / Mistral / Cohere
- ❌ Provider API 自动发现 `/v1/models`
- ❌ 自动按 cost / latency 选 provider
- ❌ 多 agent 编排
- ❌ UI 改 model(PR4)
- ❌ rig-core 迁移
- ❌ 降级结果持久化(strip 只在内存)
- ❌ reasoning_effort 之外的 OpenAI o1 专属参数(`max_completion_tokens` 留未来)

## Technical Notes

### 关键文件

| 文件 | 改动 |
|---|---|
| `app/src-tauri/src/llm/provider/openai.rs` | 新建:OpenAIProvider impl |
| `app/src-tauri/src/llm/provider/wire.rs` | 新建:WireMessage + strip_unsupported + 互转函数 |
| `app/src-tauri/src/llm/provider/mod.rs` | 改:`build_provider` openai 分支 + 工厂加 WireCapabilities |
| `app/src-tauri/src/llm/provider/anthropic.rs` | 不动(或最小动:走 wire 层?)— **Q3 决** |
| `.trellis/spec/backend/llm-contract.md` | 加 PR3 section |
| `docs/HACKING-llm.md` | 加 OpenAI 差异章节 |
| `docs/IMPLEMENTATION.md` | 状态更新 |
| `docs/BACKLOG.md` §4 | v3+ 引用本任务 |

### 关联决策

- 引用 parent PRD §D2(自研 Provider trait)
- 引用 parent PRD §D5(静默降级,parent PRD §Q5 H1)
- 引用 PR2 PRD(Provider trait shape / factory)

### Anti-patterns(避免)

- ❌ 引入 rig-core
- ❌ 引入新 crate(serde_json / reqwest / futures_util 已够)
- ❌ 改 `ChatRequest` / `ChatEvent` / `ContentBlock`(向后兼容)
- ❌ 动前端(PR2 行为不变;PR3 不新增 UI)
- ❌ 持久化降级结果(只 strip in-memory)
- ❌ 改 Anthropic 行为(只动工厂,Anthropic path 1:1 保留)
