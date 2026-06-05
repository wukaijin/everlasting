# Research: Anthropic Extended Thinking API (2026 state) + GLM compatibility

- **Query**: 2026 状态下 Anthropic extended thinking API（adaptive vs fixed budget、signature、redacted、SSE 事件、max_tokens 约束、GLM 兼容层）
- **Scope**: external（Anthropic 官方文档 + 智谱 bigmodel 官方文档）
- **Date**: 2026-06-05
- **Primary sources**:
  - [Anthropic — Building with extended thinking](https://docs.claude.com/en/docs/build-with-claude/extended-thinking)
  - [Anthropic — Adaptive thinking](https://docs.claude.com/en/docs/build-with-claude/adaptive-thinking)
  - [Anthropic — Effort parameter](https://docs.claude.com/en/docs/build-with-claude/effort)
  - [智谱 — Claude API 兼容](https://docs.bigmodel.cn/cn/guide/develop/claude/introduction)
  - [智谱 — 深度思考](https://docs.bigmodel.cn/cn/guide/capabilities/thinking)
  - [智谱 — 思考模式](https://docs.bigmodel.cn/cn/guide/capabilities/thinking-mode)

---

## 1. Adaptive thinking（模型自决思考量）

**结论：是，2026 年 Anthropic 已正式上线 adaptive thinking，模型自主决定是否思考与思考多少，不再需要 `budget_tokens`。**

### 启用方式

请求体里设置 `thinking.type = "adaptive"`：

```json
{
  "model": "claude-opus-4-8",
  "max_tokens": 16000,
  "thinking": { "type": "adaptive" },
  "messages": [...]
}
```

可选地配合 `effort` 参数（`low` / `medium` / `high` / `xhigh` / `max`）作为"软引导"，告诉模型大致愿意花多少 token 思考。

### 支持矩阵（截至 2026-06）

| 模型 | adaptive | manual (budget_tokens) |
|---|---|---|
| Claude Opus 4.8 (`claude-opus-4-8`) | **唯一支持模式** | 400 错误（被拒） |
| Claude Opus 4.7 (`claude-opus-4-7`) | **唯一支持模式** | 400 错误（被拒） |
| Claude Mythos Preview (`claude-mythos-preview`) | 默认（`thinking` 字段省略即自动启用） | 仍接受 |
| Claude Opus 4.6 (`claude-opus-4-6`) | **推荐** | 已 deprecated，仍可用 |
| Claude Sonnet 4.6 (`claude-sonnet-4-6`) | **推荐** | 已 deprecated，仍可用 |
| Claude Sonnet 4.5 / Opus 4.5 及更早 | 不支持 | 必须用 manual |

### 默认行为

- adaptive 模式默认 `effort = "high"`，此时模型"几乎总是会思考"。降到 `low` / `medium` 后会对简单问题跳过思考。
- adaptive 模式自动启用 **interleaved thinking**（工具调用之间也能思考），不需要 beta header。
- 对 Mythos Preview，省略 `thinking` 字段 = adaptive 模式自动启用。
- 对 Opus 4.8 / 4.7，**必须显式写 `thinking: {type: "adaptive"}`**，否则不会思考。

来源：[Adaptive thinking](https://docs.claude.com/en/docs/build-with-claude/adaptive-thinking)

---

## 2. Fixed budget thinking（`type: "enabled"` + `budget_tokens`）

**结论：仍然支持，但 schema 没改，已在新模型上 deprecated。**

### 请求 schema（未变）

```json
{
  "thinking": {
    "type": "enabled",
    "budget_tokens": 10000,
    "display": "summarized"   // optional, 见 §3
  }
}
```

### 兼容性

- ✅ 完全支持：Claude Sonnet 4.5、Opus 4.5、所有 Haiku，以及更早的 Claude 4 模型
- ⚠️ Deprecated 但仍可用：Claude Opus 4.6、Sonnet 4.6（"will be removed in a future model release"）
- ❌ 400 错误：Claude Opus 4.8、Opus 4.7
- ✅ Mythos Preview 仍接受

### 参数约束

- `budget_tokens` **必须小于 `max_tokens`**（普通模式）；interleaved thinking 模式下可以超过，此时上限是整个 context window
- **最小预算 = 1,024 tokens**（官方建议起步）
- 大于 32k 的预算 Claude 可能不会用完
- 与 `temperature` / `top_k` / forced tool use（`tool_choice: any|tool`）不兼容
- 不能与 `max_tokens: 0`（cache 预热）同时用
- 不能 pre-fill assistant 响应

来源：[Building with extended thinking § How to use](https://docs.claude.com/en/docs/build-with-claude/extended-thinking#how-to-use-extended-thinking)、[§ Feature compatibility](https://docs.claude.com/en/docs/build-with-claude/extended-thinking#feature-compatibility)

---

## 3. Signature 处理与 `thinking` content block 字段

### 响应 content_block 形态

```json
{
  "type": "thinking",
  "thinking": "Let me analyze this step by step...",
  "signature": "WaUjzkypQ2mUEVM36O2TxuC06KN8xyfbJwyem2dw3URve/op91XWHOEBLLqIOMfFG/UvLEczmEsUjavL...."
}
```

- `thinking`：可读的**摘要文本**（不是完整思考；完整思考被加密在 `signature` 里）
- `signature`：不透明、加密、用于回填时让 API 验证"这块 thinking 真的是 Claude 自己生成的"

### `display` 字段（控制摘要可见性）

- `"summarized"`（默认 on Sonnet 4.6 / Opus 4.6 及更早 Claude 4）：`thinking` 字段含摘要文本
- `"omitted"`（默认 on Opus 4.8 / 4.7 / Mythos Preview）：`thinking` 字段为空字符串，只有 `signature` 携带加密完整思考。**主要好处是 streaming 时首个 text token 来得更快**（跳过 thinking deltas）。
- 与 `display` 值**无关**，`signature` 字段内容相同；两轮之间可以切换 `display`。
- 与 `type: "disabled"` 同时使用会报错。
- adaptive 模式下，如果模型跳过思考，**根本不产生 thinking block**，无论 `display` 设啥。

### 回填规则（PITFALLS）

1. **使用 tool 时必须回填**完整、未修改的 thinking blocks（最后一轮 assistant message 的）。
2. **顺序不能改**：连续的 thinking blocks 序列必须和原始响应一致。
3. **`display: "omitted"` 回填**：服务端会解密 `signature` 重建原始思考；你写在 `thinking` 字段里的任何文本会被**忽略**。
4. **跨平台兼容**：signature 在 Anthropic API / Bedrock / Vertex AI 之间互通。
5. **不要解析 `signature`**：它是 opaque field。
6. **跨模型类**：Opus 4.5+ / Sonnet 4.6+ 默认在多轮里**保留**所有先前 thinking blocks；更早的 Opus/Sonnet 与所有 Haiku 模型**自动剥离**先前 thinking。
7. **过滤陷阱**：如果你的代码用 `block.type == "thinking"` 过滤回填，**会漏掉 `redacted_thinking` 块**（见 §4），破坏多轮协议。

### 是否会过期？

文档没有提到 signature 有 TTL；它只是用来验证 + 解密重建思考。同一会话内反复回填 OK。**长时间运行的 thinking task 推荐用 1-hour cache 而不是默认 5-min cache**，因为 thinking 任务常超 5 分钟。

来源：[§ Thinking encryption](https://docs.claude.com/en/docs/build-with-claude/extended-thinking#thinking-encryption)、[§ Controlling thinking display](https://docs.claude.com/en/docs/build-with-claude/extended-thinking#controlling-thinking-display)、[§ Preserving thinking blocks](https://docs.claude.com/en/docs/build-with-claude/extended-thinking#preserving-thinking-blocks)

---

## 4. `redacted_thinking` 块

### 出现时机

API 在**部分思考内容触发安全策略**而被屏蔽时返回 `redacted_thinking`。这与 `display: "omitted"` 是**完全不同的概念** —— omitted 仍是 `type: "thinking"`、`thinking` 字段为空；redacted 是另一种 block type。

### 形态

```json
{
  "type": "redacted_thinking",
  "data": "..."
}
```

- `data` 字段：opaque、加密。**对用户完全不可见、不可解密。**
- 没有可读 summary，没有 signature 字段（加密数据本身在 `data` 里）。

### 回填规则

- 多轮 + 工具调用时**原封不动回填**（和 `signature` 一样），否则破坏推理流。
- 写过滤逻辑时**必须**同时匹配 `block.type == "thinking"` 和 `block.type == "redacted_thinking"`，否则会被静默丢弃。

来源：[§ Redacted thinking blocks](https://docs.claude.com/en/docs/build-with-claude/extended-thinking#redacted-thinking-blocks)

---

## 5. `max_tokens` 与 thinking 预算的关系

### 核心规则

- `max_tokens` 是**严格上限**，且**包含**思考 token（manual 模式下）。
- Manual 模式下 `budget_tokens < max_tokens` 是硬性校验，违反会失败（小心：用 interleaved thinking 时这条约束放宽到整个 context window）。
- 最小 `budget_tokens = 1024`，官方建议从这里起步。
- Adaptive 模式没有 `budget_tokens`，`max_tokens` 同时盖住 thinking + 文本输出；`effort` 仅是软引导。

### 超界行为（Claude 4.5+）

- 在 Claude 4.5 及更新模型上，如果 `input_tokens + max_tokens > context_window`，API **接受**请求，但生成时撞到 context window 上限会 `stop_reason: "model_context_window_exceeded"`。
- 旧模型上同样情况直接返回 validation error。

### 输出上限

- Mythos Preview / Opus 4.8 / Opus 4.7 / Opus 4.6：最高 128k 输出 token
- Sonnet 4.6 / Haiku 4.5：最高 64k
- Message Batches API + `output-300k-2026-03-24` beta header：可拉到 300k（仅 4.6 / 4.7 / 4.8 系列）

### 客户端 streaming 强制

- 官方 SDK 在 `max_tokens > 21,333` 时**强制要求** streaming，否则 SDK 报错（不是 API 限制，是客户端校验，防 HTTP 超时）。

来源：[§ Max tokens and context window size](https://docs.claude.com/en/docs/build-with-claude/extended-thinking#max-tokens-and-context-window-size-with-extended-thinking)、[§ Working with thinking budgets](https://docs.claude.com/en/docs/build-with-claude/extended-thinking#working-with-thinking-budgets)、[§ Performance considerations](https://docs.claude.com/en/docs/build-with-claude/extended-thinking#performance-considerations)

---

## 6. SSE 流式事件序列（thinking 模式）

### `content_block_start`

```
event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}
```

- `content_block.type` = `"thinking"`（或 `"redacted_thinking"`、`"text"`、`"tool_use"`）
- `thinking` 与 `signature` 初始都是空串

### `content_block_delta` — `thinking_delta`

```
event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"I need to find the GCD of 1071 and 462..."}}
```

- `delta.type` = `"thinking_delta"`，`delta.thinking` 是增量文本片段
- **`display: "omitted"` 时不会发出任何 `thinking_delta`**

### `content_block_delta` — `signature_delta`（关键！）

```
event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"EqQBCgIYAhIM1gbcDa9GJwZA2b3hGgxBdjrkzLoky3dl1pkiMOYds..."}}
```

- `delta.type` = `"signature_delta"`，**紧贴在 `content_block_stop` 之前**发送
- 完整 thinking 块通常是：`content_block_start` → 多个 `thinking_delta` → 1 个 `signature_delta` → `content_block_stop`
- `display: "omitted"` 时序列简化为：`content_block_start` → 1 个 `signature_delta` → `content_block_stop`（**直接拿到 signature，跳过 deltas**）

### 完整事件流示例（节选）

```
event: message_start
event: content_block_start  (index=0, type=thinking)
event: content_block_delta  (thinking_delta ×N)
event: content_block_delta  (signature_delta ×1)
event: content_block_stop   (index=0)
event: content_block_start  (index=1, type=text)
event: content_block_delta  (text_delta ×N)
event: content_block_stop   (index=1)
event: message_delta        (stop_reason)
event: message_stop
```

### Streaming 注意点

- 流可能 "chunky"：thinking 内容常以更大批次到达，文本可能更平滑。
- `usage.output_tokens_details.thinking_tokens` 字段**只在最后的 `message_delta` 事件上出现**（用于查询计费的原始思考 token 数）。

来源：[§ Streaming thinking](https://docs.claude.com/en/docs/build-with-claude/extended-thinking#streaming-thinking)

---

## 7. GLM（智谱 bigmodel.cn）兼容层

### Claude-compat 端点（项目当前用法）

GLM 提供 Anthropic-shape 端点：`https://open.bigmodel.cn/api/anthropic/v1/messages`，号称"无缝替换 Anthropic SDK"（只换 `base_url` 和 `api_key`）。但 **GLM 的 Claude API 兼容文档完全没有提到 thinking**（验证：`docs.bigmodel.cn/cn/guide/develop/claude/*` 路径下没有 thinking / thinking-mode 子页面，404）。

来源：[智谱 — Claude API 兼容](https://docs.bigmodel.cn/cn/guide/develop/claude/introduction)（无 thinking 章节）

### GLM 原生 thinking schema（在 OpenAI-compat 端点 `/api/paas/v4/chat/completions` 上）

GLM 自己定义的 thinking 启用方式**与 Anthropic schema 不同**：

```json
{
  "model": "glm-5.1",
  "thinking": {
    "type": "enabled"        // 或 "disabled"; 没有 "adaptive"
  }
}
```

差异点：
- **没有 `budget_tokens`**（也没有 `display` / `effort`）
- 默认行为：GLM-5.1 / GLM-5 / GLM-4.7 / GLM-4.5v 系列 `type: "enabled"` 是"强制思考"；其他模型是"自动判断"（这本身已经类似 adaptive，但 schema 上没有专门的 `adaptive` 标识）
- 响应里**思考内容放在 `reasoning_content` 字段**（OpenAI-style），不是 Anthropic 风格的 `thinking` content block + `signature`
- 多轮回填思考要用 `clear_thinking: false`（"Preserved Thinking"）放在 `thinking` 对象里
- "Turn-level thinking"（GLM-4.7 新能力）：会话内每轮独立开关思考，无需匹配前一轮模式

支持模型：GLM-5.1、GLM-5、GLM-5-Turbo、GLM-5V-Turbo、GLM-4.7、GLM-4.6、GLM-4.5

来源：[智谱 — 深度思考](https://docs.bigmodel.cn/cn/guide/capabilities/thinking)、[智谱 — 思考模式](https://docs.bigmodel.cn/cn/guide/capabilities/thinking-mode)

### 对本项目（Everlasting）的实际影响

| 维度 | Anthropic 原生 | GLM Claude-compat (`/api/anthropic/v1/messages`) |
|---|---|---|
| `thinking: {type: "adaptive"}` | ✅ Opus 4.8/4.7/Mythos 必需 | ⚠️ **未文档化**，需自行测试 |
| `thinking: {type: "enabled", budget_tokens: N}` | ✅ Sonnet 4.5/Opus 4.5 等支持 | ⚠️ **未文档化**，可能被忽略或报错 |
| 响应里 `type: "thinking"` block + `signature` | ✅ | ⚠️ **未明示**，GLM 原生格式是 `reasoning_content`，Claude-compat 端点是否转译未文档化 |
| `redacted_thinking` block | ✅ | ❌ 几乎肯定不支持 |
| SSE `thinking_delta` / `signature_delta` | ✅ | ⚠️ 未文档化 |

**结论**：GLM 的 Claude-compat 端点对 thinking 是"灰色地带"——SDK swap 工作，但 thinking 字段的转译行为**没有官方说明**。建议：
1. 真机测试：发一个带 `thinking: {type: "enabled", budget_tokens: 2048}` 的请求到 GLM Claude-compat 端点，观察是 (a) 返回 `thinking` block + signature、(b) 静默忽略、还是 (c) 报错。
2. 如果 (a) 不工作，回退到 GLM 原生 `/api/paas/v4/chat/completions` 端点，用 OpenAI schema 拿 `reasoning_content`。
3. 项目当前 LLM 客户端是手写 SSE（`llm/sse.rs`），方便加 GLM-specific 分支处理 `reasoning_content` 增量事件。

---

## 8. Best practices 2026

### 预算 sizing

- **manual 模式起步预算**：1,024（最小）→ 16,000+（复杂任务）→ 32,000+（深推理；超过此需 batch processing 避免网络超时）
- **adaptive 模式**：默认 `effort: high` 即可；对 Sonnet 4.6 实际显式设 `medium` 是推荐默认（"agentic coding / 工具密集"场景的最佳速度-质量平衡）；对 Opus 4.7 / 4.8 编码场景**起步 `xhigh`**，纯文字 / 评测后才降到 `high`

### Adaptive vs Fixed 选型

| 场景 | 推荐 |
|---|---|
| Opus 4.8 / 4.7 | **只能** adaptive |
| Opus 4.6 / Sonnet 4.6 | adaptive（manual 已 deprecated） |
| 需要"可预测延迟 / 精确成本"的批量任务 | manual + 固定 `budget_tokens` |
| Sonnet 4.5 / Opus 4.5 / Haiku 系列 | **只能** manual |
| 简单事实问答 / 翻译 / 分类 | `thinking: {type: "disabled"}` 或 adaptive + `effort: low` |

### 多轮对话里的 thinking

1. **整个 assistant turn 必须在同一种 thinking 模式下**——工具调用循环算同一 turn 的一部分，turn 内**不能切换 enabled/disabled**（中途切换会被 API 静默禁用 thinking）。
2. 在新的 user turn 开始时**可以**切换 thinking 模式；adaptive ↔ enabled/disabled 切换会使**消息缓存断点失效**（system prompt 与 tool defs 缓存不受影响）。
3. **始终回填**完整 thinking blocks（包括 `redacted_thinking`），即使你不展示给用户——API 会自动过滤、只计算用到的 thinking 块的 input tokens。
4. **Opus 4.5+ / Sonnet 4.6+** 默认在 context 里**保留**所有先前 thinking blocks（cache 友好，但占 context）。要清理用 [`clear_thinking_20251015` context-editing 策略](https://docs.claude.com/docs/en/build-with-claude/context-editing)。
5. **不要 pre-fill assistant 响应**（与 thinking 互斥）。
6. **`tool_choice` 限制**：thinking 启用时只能用 `auto` 或 `none`，`any` 和 `tool` 会报错。

### Streaming / latency

- 应用不需要展示思考内容时设 `display: "omitted"`，可大幅减少 first-text-token latency（但**成本一样**，仍按完整 thinking token 计费）
- `max_tokens > 21,333` 强制 streaming，避开 HTTP 超时
- 大预算（>32k）走 Message Batches API

### Token 监控

- 读 `usage.output_tokens_details.thinking_tokens` 拿到真实思考 token 数（≤ `output_tokens`）
- 计费按**完整原始思考**计，不是摘要

### Prompt-level 调控（adaptive 模式专属）

- System prompt 或 user message 末尾追加 `"Please think hard before responding."` 鼓励思考；`"Answer directly without deliberating."` 抑制思考。
- 措辞敏感，可能要换几种说法测试。

来源：[§ Best practices](https://docs.claude.com/en/docs/build-with-claude/extended-thinking#best-practices-and-considerations-for-extended-thinking)、[Adaptive § Tuning thinking behavior](https://docs.claude.com/en/docs/build-with-claude/adaptive-thinking#tuning-thinking-behavior)、[Effort § Recommended levels](https://docs.claude.com/en/docs/build-with-claude/effort)

---

## Caveats / Not Found

- **GLM Claude-compat 对 thinking 的转译行为**：智谱官方文档 `docs.bigmodel.cn/cn/guide/develop/claude/*` 没有任何 thinking 章节，`/cn/guide/develop/claude/thinking`、`/parameters`、`/feature` 均 404。**必须真机测试**才能确定 `thinking: {type: "enabled" | "adaptive"}` 字段在 GLM Claude-compat 端点上的行为。
- **Signature TTL**：Anthropic 文档未明示 signature 有过期时间，但跨长时间会话建议用 1-hour cache。
- **`output-300k-2026-03-24` beta header**：仅 Batches API 可用；同步 API 仍受模型固有输出上限约束。
- 文档中未明确给出 adaptive 模式下 `effort` 与实际 thinking token 数之间的精确换算表（"behavioral signal, not a strict token budget"）。
