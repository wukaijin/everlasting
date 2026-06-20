# LLM API Contract —核心类型与思考契约

> **基线**:2026-06-10 commit `0f9a167` (8-PR5拆分后) + 2026-06-21 (doc-trim 拆 3 个 scenario)
> **来源**:从原 `llm-contract.md` (3149 行)拆出本文件 +4 个子文件 (`tool-contract.md` / `worktree-contract.md` / `multi-provider-contract.md` / `test-model-contract.md`)
> **同源文档**:
> - [llm-contract.md](./llm-contract.md) (本文) —核心类型 + Extended Thinking + 反模式汇总
> - [tool-contract.md](./tool-contract.md) —工具定义 + ReadGuard + shell spillover
> - [worktree-contract.md](./worktree-contract.md) — attach/detach/delete + cancel + system prompt
> - [multi-provider-contract.md](./multi-provider-contract.md) — Provider trait + catalog + Anthropic/OpenAI 分发
> - [test-model-contract.md](./test-model-contract.md) — `test_model` IPC
>
> **详细 scenario**(2026-06-21 doc-trim 拆出):
> - [latency-tracking.md](./latency-tracking.md) — F5(2026-06-11)+ Per-Turn Tracking follow-up
> - [token-usage-tracking.md](./token-usage-tracking.md) — A4(2026-06-10)
> - [permission-layer.md](./permission-layer.md) — A2 + B7(2026-06-13)⑨ 关 5-tier
>
> **何时读本文**:涉及 `ContentBlock` / `ChatMessage` / `ChatEvent` / Extended Thinking持久化 / 反模式排查时。

---

# LLM API Contract

> Anthropic Messages API contract enforced by the Rust agent core, with extended-thinking support.

---

## Overview

The LLM client (`app/src-tauri/src/llm/`) speaks the **Anthropic Messages API** schema
(`/v1/messages`, streaming via SSE) directly — not OpenAI, not a generic OpenAI-compat layer.
The `ChatRequest` / `ContentBlock` / `ChatEvent` types are aligned to the official schema
(serde tag `type` matches the wire string).

Two operator choices that look like "compat layer" are actually configuration knobs:

- **`ANTHROPIC_BASE_URL`** — a proxy or a self-hosted relay that follows the Anthropic
 schema. The current dev setup uses `<your-anthropic-compat-host>`'s Claude-compat endpoint; the
 payload is still Anthropic-shaped, not OpenAI-shaped.
- If at any point we switch to OpenAI-compat, the `reasoning_content` field replaces
 the `thinking` block entirely; that change would happen here, not in the UI.

For compatibility-layer caveats and what to test when the proxy changes, see
`docs/HACKING-llm.md`.

---

## Scenario: Extended Thinking Support (Step6)

###1. Scope / Trigger

- Trigger: Added `ContentBlock::Thinking` and `ContentBlock::RedactedThinking`
 to satisfy the cross-layer request/response contract for Anthropic extended thinking.
- Why code-spec depth: mandatory — the request body must include the right `thinking`
 shape, the response must be parsed without losing the `signature` blob, the signature
 must round-trip on subsequent turns or Anthropic returns `400`, and the SSE parser
 emits three new event variants that the frontend must handle in order.

###2. Signatures

#### Backend types (`app/src-tauri/src/llm/types.rs`)

```rust
pub struct LlmConfig {
 pub base_url: String,
 pub model: String,
 pub api_key: String,
 pub max_tokens: u32,
 pub thinking_effort: String, // "low" | "medium" | "high" | "xhigh" | "max"
}

pub struct ChatRequest {
 pub model: String,
 pub max_tokens: u32,
 pub system: Option<String>,
 pub messages: Vec<ChatMessage>,
 pub tools: Vec<ToolDef>,
 pub thinking: Option<ThinkingConfig>, // always Some(Adaptive{..}) in practice
}

#[serde(tag = "type", rename_all = "snake_case")]
pub enum ThinkingConfig {
 Adaptive { display: String, effort: String }, // display always "summarized"
}

#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
 Text { text: String },
 ToolUse { id: String, name: String, input: serde_json::Value },
 ToolResult { tool_use_id: String, content: String, is_error: bool },
 Thinking { thinking: String, signature: String },
 RedactedThinking { data: String },
}

#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatEvent {
 Start { request_id: String },
 Delta { text: String },
 ThinkingDelta { text: String },
 SignatureDelta { signature: String },
 RedactedThinkingDelta { data: String },
 ToolCall { id: String, name: String, input: serde_json::Value },
 ToolResult { tool_use_id: String, content: String, is_error: bool },
 Done { stop_reason: String, usage: serde_json::Value },
 Error { message: String, kind: LlmErrorKind },
}
```

#### Frontend payload (`app/src/stores/chat.ts`)

```typescript
type ContentBlockPayload =
 | { type: "text"; text: string }
 | { type: "tool_use"; id: string; name: string; input: unknown }
 | { type: "tool_result"; tool_use_id: string; content: string; is_error: boolean }
 | { type: "thinking"; thinking: string; signature: string }
 | { type: "redacted_thinking"; data: string };

// ChatMessage.thinkingBlocks is in-memory only; persisted as
// ContentBlock::Thinking { thinking, signature } rows in the DB.
type ThinkingBlockInfo = { thinking: string; signature: string };
```

###3. Contracts

#### Request (always sent)

```json
{
 "model": "<from env LLM_MODEL>",
 "max_tokens":16384,
 "system": "<system prompt or omitted>",
 "messages": [ ... ],
 "tools": [ ... ],
 "thinking": { "type": "adaptive", "display": "summarized", "effort": "high" }
}
```

- `thinking` is **always present** in the request body. There is no kill switch.
 - If the upstream model does not support adaptive thinking, the call returns `400`.
 This is an accepted operational risk (see ADR D5 in the task PRD).
- `thinking.display` is **always `"summarized"`** — explicit, not omitted.
 - On Opus4.7+ the default `display` is `"omitted"`, which suppresses `thinking_delta`
 SSE events and breaks the UI.
- `thinking.effort` is sourced from `LLM_THINKING_EFFORT` (default `"high"`).
 - Valid values: `low` / `medium` / `high` / `xhigh` / `max` (Anthropic schema).
 - Invalid values pass through unchanged; the upstream API will reject them.
- `max_tokens` default is `16384` (was `1024` in step2; bumped in step6 because
 thinking tokens count against the same budget as the actual answer).

#### Response (SSE event sequence)

```
content_block_start { index:0, content_block: { type: "thinking", thinking: "" } }
content_block_delta { index:0, delta: { type: "thinking_delta", thinking: "..." } }
content_block_delta { index:0, delta: { type: "signature_delta", signature: "..." } }
content_block_stop { index:0 }
content_block_start { index:1, content_block: { type: "text", text: "" } }
content_block_delta { index:1, delta: { type: "text_delta", text: "..." } }
content_block_stop { index:1 }
message_delta { delta: { stop_reason: "end_turn" } }
message_stop
```

Block types observed in step6: `text`, `tool_use`, `thinking`, `redacted_thinking`.
Delta types observed: `text_delta`, `input_json_delta`, `thinking_delta`, `signature_delta`.

#### Environment keys

| Key | Required | Default | Notes |
|-----|----------|---------|-------|
| `ANTHROPIC_API_KEY` (or `ANTHROPIC_AUTH_TOKEN`) | yes | — | The dev setup uses `<your-anthropic-compat-host>` proxy tokens; `ANTHROPIC_AUTH_TOKEN` is the legacy alias. |
| `ANTHROPIC_BASE_URL` | no | `https://api.anthropic.com` | Trailing `/v1/messages` is appended by `LlmConfig::endpoint()`. |
| `LLM_MODEL` | no | `GLM-4.7` | |
| `LLM_MAX_TOKENS` | no | `16384` | Was `1024` before step6. |
| `LLM_THINKING_EFFORT` | no | `high` | Adaptive thinking effort. |

#### Via-Relay (wukaijin.com / DeepSeek) thinking contract

When the Anthropic `/v1/messages` endpoint is fronted by the **wukaijin.com**
relay (upstream `deepseek-v4-flash`, and any other relay that streams thinking
text WITHOUT a real `signature_delta`), the relay enforces a **stricter**
contract than native Anthropic. On every assistant message echoed back in a
later turn, BOTH are required:

- `content[].thinking` blocks MUST be present — **even with an empty
  `signature`** (the relay does NOT cryptographically verify signatures).
- a top-level `reasoning_content` string field (sibling of `content`) MUST be
  present, carrying the concatenated `thinking` text of that message.

Drop either → `400`. Verified by V1/V2/V3 probe experiments against the real
relay (task `06-21-fix-deepseek-relay-thinking-block-drop-causing-turn-2-400`):

| assistant shape returned on the next turn | relay response |
|---|---|
| `content[].thinking` blocks **dropped** | 400 `content[].thinking in the thinking mode must be passed back` |
| blocks kept, **NO** `reasoning_content` | 400 `reasoning_content in the thinking mode must be passed back` |
| blocks kept **WITH** `reasoning_content` | **200 ✅** |

`apply_deepseek_reasoning_fix` (`anthropic.rs`) enforces this on the send path,
**unconditionally for every Anthropic request** (harmless to native Claude,
which has non-empty signatures and ignores the unknown field): it keeps every
`thinking` block verbatim and lifts a `reasoning_content` field from ALL
thinking blocks (joined by `\n`), guarded so a thinking-less assistant message
(text + tool_use only) gets no field.

> **Why the relay streams empty signatures**: in **streaming** mode the
> wukaijin relay does NOT emit `signature_delta`, so the persisted
> `ContentBlock::Thinking` ends up with `signature: ""`. (Non-streaming
> responses carry a placeholder uuid.) The fix MUST treat empty-signature
> blocks as keepable — dropping them was the 06-20 regression root cause.

> **LESSON — attribute relay/API behavior by probe experiment, NOT by
> inference from production symptoms.** The 06-20 fix dropped empty-signature
> blocks based on an unverified theory ("empty sig inflates the relay's
> accumulated-state count"); real-relay V1/V2/V3 probing proved the theory
> wrong and the drop produced the 06-21 turn-2 400. Run `/trellis:break-loop`
> for the full analysis.

> **ROOT FIX (06-21): route DeepSeek via OpenAI protocol, not Anthropic.**
> The Via-Relay anthropic path above is **fundamentally unreliable** — the relay's
> Anthropic→DeepSeek thinking translation is non-deterministic (same payload 400s
> on one call, 200s on the next) and no client-side `thinking`/`reasoning_content`
> shaping reliably satisfies it (V1 drop-block → `thinking must be passed back`;
> V2 keep+field → `reasoning_content must be passed back`). The root fix: configure
> `deepseek-v4-flash` on an **OpenAI-protocol** provider (wukaijin exposes
> `/v1/chat/completions` too; DeepSeek is natively OpenAI). Then `reasoning_content`
> is a native field — **no translation layer**.
>
> `OpenAIProvider` contract (`openai.rs` RULE-D-006, **gated to reasoning models**
> via `reasoning_effort.is_some() || is_o1_family(&model)`):
> - assistant carrying `Reasoning` blocks → lift to top-level `reasoning_content`
>   field (joined `\n`), NOT prepended into `content` text.
> - text-only assistant (worker memory ack, plain reply) → `reasoning_content: "none"`
>   (DeepSeek v4 requires non-empty; `""` is rejected by the strict AstrBot-PR-7823
>   contract though wukaijin today tolerates it — `"none"` is the safe choice).
> - non-reasoning OpenAI models (gpt-4o / gpt-4.1) → field **absent** (vanilla
>   OpenAI shape; `reasoning_content` is provider-specific and reserved-ish on
>   some proxies, so don't pollute non-reasoning requests).
>
> `reasoning_effort`: DeepSeek's own enum is `{low, medium, high, xhigh, max}`
> (superset of OpenAI o1's `{low, medium, high}`); it rejects `minimal`.
> `ModelRow.thinking_effort` uses the same vocabulary, so OpenAIProvider passes it
> through verbatim — no per-model suppression needed.
>
> `apply_deepseek_reasoning_fix` (anthropic.rs) is **retained** for native Claude
> and other anthropic-relayed models, but is a no-op for DeepSeek once DeepSeek is
> on the OpenAI protocol.

###4. Validation & Error Matrix

| Condition | Result |
|-----------|--------|
| `ANTHROPIC_API_KEY` missing at startup | `LlmConfig::unconfigured()` — `api_key: ""`, app still launches so UI shows a helpful error. |
| `LLM_MAX_TOKENS` is not a number | Falls back to default `16384`. |
| `LLM_THINKING_EFFORT` is unrecognized | Sent verbatim; upstream may400. |
| Upstream rejects `thinking: { type: "adaptive" }` | Anthropic returns400. Switch base_url or downgrade to manual mode (out of MVP scope). |
| `signature` is lost on round-trip (e.g. dropped during rehydrate) | Anthropic returns400 on the next turn. **Hard rule: `signature` must round-trip verbatim.** |
| `redacted_thinking.data` is mutated or truncated | Anthropic returns400 on the next turn. Opaque — store as-is. |
| `thinking` block appears after a `tool_use` block in history | Anthropic rejects the order. The rehydrate path emits thinking blocks FIRST. |
| `content_block_start` for `thinking` arrives with non-empty `thinking`/`signature` fields | Treated as the initial buffer content (defensive — Anthropic today sends empty). |
| Via wukaijin relay: assistant message echoed back WITHOUT `content[].thinking` blocks (e.g. empty-signature blocks dropped) | 400 `content[].thinking in the thinking mode must be passed back`. Fix: `apply_deepseek_reasoning_fix` keeps ALL thinking blocks (empty-signature OK). |
| Via wukaijin relay: assistant message echoed back WITH thinking blocks but NO top-level `reasoning_content` | 400 `reasoning_content in the thinking mode must be passed back`. Fix: `apply_deepseek_reasoning_fix` lifts `reasoning_content` from all thinking blocks. |

###5. Good / Base / Bad Cases

#### Good: streaming + persistence + round-trip

1. Model emits `thinking_delta` × N, then `signature_delta` ×1, then `content_block_stop`.
2. Backend buffers the signature; emits `ThinkingDelta` per `thinking_delta` event;
 emits a single `SignatureDelta` on `content_block_stop`.
3. Agent loop finalizes a `ContentBlock::Thinking { thinking, signature }` at the
 turn boundary; persists to DB; emits to frontend.
4. Frontend rehydrates on next session load; `toPayloadContent` puts the thinking
 block first in the assistant message.
5. Next request to Anthropic carries the full signature; no400.

#### Base: redacted_thinking

1. Safety filter triggers; Anthropic emits `content_block_start { type: "redacted_thinking", data: "..." }`
 followed immediately by `content_block_stop` (no streaming deltas).
2. Backend buffers `data`; emits a single `RedactedThinkingDelta` on stop.
3. Agent loop finalizes a `ContentBlock::RedactedThinking { data }`.
4. Frontend renders a "🔒 N redacted thinking block(s)" placeholder; the data is
 never displayed.

#### Bad: per-event signature emit

1. (The original step6 implementation emitted `SignatureDelta` per `signature_delta`
 event instead of buffering until `content_block_stop`.)
2. If Anthropic ever splits the signature across N `signature_delta` events (defensive),
 the frontend opens N empty-text thinking blocks; the DB stores N partial-signature
 blocks; the next turn's history is malformed → Anthropic returns400.
3. Fix: buffer in `BlockState::Thinking { signature_buf }`; emit once on stop.

#### Bad: thinking block re-emitted as second text block

1. UI's `MessageContent::to_text()` accidentally includes thinking text in the
 denormalized `text` column.
2. Rehydrate reads both the `text` block AND the thinking text into the bubble.
3. The user sees duplicated content; on the next turn the model is confused.

###6. Tests Required

The step6 PR added15 unit tests; the following are mandatory for any future
change to this area.

#### Backend (`cargo test`)

| Test | Asserts |
|------|---------|
| `thinking_block_serializes_to_anthropic_schema` | `ContentBlock::Thinking` → `{"type":"thinking","thinking":"...","signature":"..."}`. |
| `thinking_block_deserializes_from_anthropic_schema` | The reverse round-trip. |
| `redacted_thinking_block_serializes_to_anthropic_schema` | `type: "redacted_thinking"` with only the `data` field. |
| `redacted_thinking_block_deserializes_from_anthropic_schema` | Reverse. |
| `chat_message_round_trip_with_thinking_blocks` | Thinking blocks survive `to_json` / `from_json` losslessly. |
| `chat_message_round_trip_with_redacted_thinking` | Same for redacted. |
| `message_content_to_text_excludes_thinking` | `MessageContent::to_text()` does NOT include thinking text in the denormalized string. |
| `chat_event_thinking_delta_serializes_with_snake_case_kind` | The wire `kind` is `"thinking_delta"`. |
| `chat_event_signature_delta_serializes_with_snake_case_kind` | `"signature_delta"`. |
| `chat_event_redacted_thinking_delta_serializes_with_snake_case_kind` | `"redacted_thinking_delta"`. |
| `chat_request_thinking_omitted_when_none` | `Option<ThinkingConfig>` uses `skip_serializing_if` (or equivalent) when `None`. |
| `chat_request_thinking_adaptive_serializes_correctly` | Output: `{"type":"adaptive","display":"summarized","effort":"high"}`. |
| `default_max_tokens_is_16384_not_1024` | Env-less default is `16384`. |
| `thinking_config_is_adaptive_summarized_with_configured_effort` | `LlmConfig::from_env()` honors `LLM_THINKING_EFFORT` and always sets `display: "summarized"`. |
| `unconfigured_has_empty_thinking_effort` | `LlmConfig::unconfigured()` has `thinking_effort: ""` (does not panic). |

Total backend suite:57 tests pass as of step6.

#### Frontend

- `pnpm build` (vue-tsc strict) must pass. The thinking-related types live in
 `app/src/stores/chat.ts`; any field added there must be type-checked end-to-end.
- Manual smoke test (acceptance A9): `cd app && pnpm tauri dev`, observe
 thinking stream + `<details>` collapse + session switch round-trip.

###7. Wrong vs Correct

#### Wrong: drop empty-signature thinking blocks for a relay model

```rust
// BAD (06-20 regression) — the wukaijin relay requires content[].thinking
// blocks AND reasoning_content TOGETHER; dropping the block →
// 400 "content[].thinking in the thinking mode must be passed back".
arr.retain(|block| {
    if block.get("type").and_then(|t| t.as_str()) == Some("thinking") {
        !block.get("signature").and_then(|s| s.as_str()).unwrap_or("").is_empty()
    } else {
        true
    }
});
```

#### Correct: keep every thinking block + lift `reasoning_content`

```rust
// Keep ALL thinking blocks (empty-signature OK — the relay does NOT verify
// signatures) and lift reasoning_content from every one of them.
let mut reasoning_buf = String::new();
for block in arr.iter() {
    if block.get("type").and_then(|t| t.as_str()) == Some("thinking") {
        if let Some(text) = block.get("thinking").and_then(|t| t.as_str()) {
            if !reasoning_buf.is_empty() {
                reasoning_buf.push('\n');
            }
            reasoning_buf.push_str(text);
        }
    }
}
if !reasoning_buf.is_empty() {
    msg["reasoning_content"] = serde_json::Value::String(reasoning_buf);
}
```

#### Wrong: emit `SignatureDelta` per `signature_delta` SSE event

```rust
// BAD — emit immediately on each delta
"signature_delta" => {
 let sig = delta.get("signature").and_then(|s| s.as_str()).unwrap_or("").to_string();
 yield Ok(ChatEvent::SignatureDelta { signature: sig });
 // signature_buf is dead code
}
```

If Anthropic ever splits the signature across events, the frontend opens N
thinking blocks, the DB stores N partial signatures, and the next turn400s.

#### Correct: buffer and emit on `content_block_stop`

```rust
// GOOD — buffer, then emit once on stop
"signature_delta" => {
 if let BlockState::Thinking { signature_buf, .. } = &mut block_state {
 signature_buf.push_str(delta.get("signature")...);
 }
 // no event emitted here
}
// ...
BlockState::Thinking { signature_buf, .. } if !signature_buf.is_empty() => {
 yield Ok(ChatEvent::SignatureDelta { signature: std::mem::take(signature_buf) });
}
```

The buffered signature is the full assembled blob, ready for the DB and the
next-turn payload.

#### Wrong: thinking block emitted after tool_use in history

```typescript
// BAD — toPayloadContent appends thinking at the tail
function toPayloadContent(m: ChatMessage): ContentBlockPayload[] {
 return [
 ...m.toolUses.map(...),
 ...m.thinkingBlocks.map(t => ({ type: "thinking", ...t })),
 ];
}
```

Anthropic requires thinking blocks at the head of an assistant message; the
next turn400s.

#### Correct: thinking blocks first

```typescript
// GOOD — thinking blocks first, then tool_use / text
function toPayloadContent(m: ChatMessage): ContentBlockPayload[] {
 return [
 ...m.thinkingBlocks.map(t => ({ type: "thinking", thinking: t.thinking, signature: t.signature })),
 ...m.text.map(t => ({ type: "text", text: t })),
 ...m.toolUses.map(...),
 ];
}
```

---

## Decision: Always send `thinking`, no per-session / per-request toggle

**Context**: MVP UX. Adding a toggle would expand the settings surface and the
DB schema.

**Decision**: `thinking` is always in the request body. The only knob is
`LLM_THINKING_EFFORT` env, applied globally.

**Consequences**: Simple. If the upstream model does not support adaptive
thinking the call400s — accepted as an operational risk.

## Decision: `display: "summarized"` is explicit, never omitted

**Context**: Opus4.7+ defaults to `display: "omitted"`, which suppresses
`thinking_delta` SSE events and breaks the UI's streaming label.

**Decision**: `ThinkingConfig::Adaptive { display: "summarized", effort }` is
hard-coded in `LlmConfig::thinking_config()`.

**Consequences**: Streamed thinking is always visible. (Trade ~1-2 ms per
response for guaranteed streaming.)

## Decision: `max_tokens` default1024 →16384

**Context**: Thinking tokens count against the same budget as the actual answer.
1024 was too low — non-trivial turns would hit `stop_reason: "max_tokens"`.

**Decision**: `DEFAULT_MAX_TOKENS =16384`.

**Consequences**: Cheap requests waste ~8k of budget, but no truncation on
real workloads. Env override available.

## Gotcha: tool_use ↔ tool_result Pair Atomicity (C3, 2026-06-12)

**Rule**: Any code path that truncates / compacts / splits the `messages` array
(e.g. C3 `compact_messages`) MUST treat an `assistant(tool_use)` + the immediately
following `user(tool_result)` as **one atomic unit**. Either both stay in history
or both are dropped. Never split them.

**Why**: Anthropic returns `400 invalid_request_error` on the next turn if
history has an `assistant(tool_use)` block whose `tool_use_id` has no matching
`tool_result` (orphan request) or a `user(tool_result)` whose `tool_use_id`
has no matching `tool_use` (orphan result). The error does NOT name the
problem — the agent loop sees a generic 400 and retries, which 400s again.

**When this bites**:
- C3 context compression (the obvious case — dropping old turns)
- Any future "summarize old messages" feature
- Any future "sliding window context" feature
- Edge case in `compact_messages`: when a pair straddles the **protected tail**
 boundary (current user message is protected), the algorithm must recognize
 `messages[len-2] = assistant(tool_use)` + `messages[len-1] = user(tool_result)`
 and treat them as a single protected unit (not as separate droppable turns).

**Test coverage** (in `agent/context.rs`):
- `case_3_tool_use_tool_result_pair_intact_or_dropped_together`
- `regression_pair_at_tail_split_under_pressure` (C3 PR1 regression)

**Related**:
- Thinking blocks have a similar atomicity requirement (see Validation & Error
 Matrix row "`thinking` block appears after a `tool_use` block in history") —
 the assistant turn is the atomic unit for thinking, while the pair is the
 atomic unit for tool_use.

---

## Scenario: DeepSeek-Via-Anthropic-Relay thinking block fix (RULE-D-003, 2026-06-20)

###1. Scope / Trigger

- Trigger: DeepSeek-v4 thinking mode (`deepseek-v4-flash` / `deepseek-v4-pro` / `deepseek-reasoner` alias) 经中转站（如 wukaijin.com — `https://api.wukaijin.com`）以 Anthropic Messages API 协议 (`POST /v1/messages`) 访问时，多轮对话第二轮起持续 400：
  ```
  {"error":{"type":"invalid_request_error","message":"Error from provider (DeepSeek): The `reasoning_content` in the thinking mode must be passed back to the API. (request id: ...)"}}
  ```
- Why code-spec depth: mandatory — 中转站 thin passthrough Anthropic schema 到 DeepSeek V4 后端，DeepSeek V4 thinking mode 契约要求 assistant message 带顶层 `reasoning_content` 字段；Anthropic 标准 `thinking` block + `signature` 单独不够（cumul. state 校验失败）。

###2. Root Cause

- 中转站对 assistant message 中的 thinking block 做累积状态校验（threshold 不稳定，与 thinking block 数量 / token / UUID signature 数量 / cache 状态综合相关），校验失败时报 "reasoning_content must be passed back"。
- Anthropic SSE 解析侧：中转站用 **UUID v4 字符串**（如 `c556ef17-b531-4366-9477-ebc7bdc29b9b`）作 thinking block `signature` 字段，不是 Anthropic 原生 base64 加密 blob。empty signature (`""`) 是 SSE `signature_delta` 事件未到达时的 fallback。
- `wukaijin.com` 对 empty-signature 块**不**做 reasoning_content 校验（DB 反推：`e9bf6c07` turn 0-1 empty sig 仍 work），对非空 UUID sig 块**做**校验（`0a8cc2f0` turn 1 empty + turn 2 UUID → turn 3 400）。

###3. Fix Contract

实现位于 `app/src-tauri/src/llm/provider/anthropic.rs::apply_deepseek_reasoning_fix`，pub(crate) 纯函数：

```rust
pub(crate) fn apply_deepseek_reasoning_fix(req: &ChatRequest) -> serde_json::Value
```

对每条 `role == "assistant"` 消息（`content` 必须是 block 数组形式；string content 直接跳过）：

1. **Filter (B)** — 从 `content[]` 移除所有 `{"type":"thinking","signature":""}` 块（含 signature 字段缺失 — `unwrap_or("")` 统一视为空）。保留 text / tool_use / thinking-with-non-empty-signature / redacted_thinking 块。块顺序保留（tool_use / text 交错不变）。
2. **Inject (A)** — 收集所有**保留下来**的 thinking block 的 `thinking` 文本，多块用 `\n` 拼接；若结果非空，在 message 顶层加 `reasoning_content: String` 字段（与 `content` 同级，Anthropic 协议非标扩展）。全空时**不**加 `reasoning_content: ""` 字段（避免 relay sentinel mismatch）。

`apply_deepseek_reasoning_fix` 在 `AnthropicProvider::send` 末尾调用，输出 `body: serde_json::Value` 传入 `chat_stream_with_tools(config, body)` 替代 `req: ChatRequest`。`chat_stream_with_tools` 签名从 `(config, req: ChatRequest)` 改为 `(config, body: serde_json::Value)`，HTTP POST 从 `.json(&req)` 改为 `.body(body.to_string())`。

###4. Anthropic 原生路径兼容性

- Anthropic 标准 `/v1/messages` API 接受未知字段（serde 默认行为），`reasoning_content` 字段被忽略，extended thinking 行为 1:1 等价。
- 顶层 `thinking: adaptive` 字段保留不动（Claude extended thinking 必需 — D 方案/FT-D-001 跟进）。
- `OpenAIProvider` 路径完全未触碰（R4）— 验证 `cargo test --lib openai::` 35 passed。
- 顶层 `tracing::info!` log 字段（`model` / `tools_count` / `has_system`）从 `body` JSON 提取，log 内容与 pre-fix 等价。

###5. Evidence (DB 反推)

4 个 DeepSeek-via-wukaijin session 对比（`session_audit_events` + `messages.created_at` + Anthropic SSE 解析 `signature` 字段值）：

| Session | mode | turn 0 | turn 1 | turn 2 | turn 3 | turn 4 | turn 5 | 结果 |
|---|---|---|---|---|---|---|---|---|
| 0a8cc2f0 | yolo | empty | UUID | **400** | - | - | - | ❌ |
| 053ae61e | yolo | UUID | UUID | UUID | UUID | **400** | - | ❌ |
| 11cefabc | edit | empty | empty | UUID | empty | UUID | **400** | ❌ |
| e9bf6c07 | yolo | empty | empty | UUID | UUID | UUID | UUID→empty | ✅ |

empty sig + UUID sig 混合 / 全 UUID 都可能 400；具体 threshold 不稳定。修复通过**降低触发因子**（减少 thinking block 数量 + 显式提供 `reasoning_content` 字段）规避 400。

`AuditKind` 14 variant 全部是 permission / mode / tool / edit 类，**没有 LlmError / NetworkError / ProviderError**（`app/src-tauri/src/agent/permissions/mod.rs:152`）—— LLM 错误**只**走 `tracing::warn!`（`anthropic.rs:262`），不进 `session_audit_events`。LLM 错误时间定位**只能靠** `messages.created_at` 的 `ERROR_MARKER`（`app/src-tauri/src/agent/helpers.rs:307` = `"[生成出错中断]"`）。

###6. Tests Required

`app/src-tauri/src/llm/provider/anthropic.rs` `#[cfg(test)] mod tests` 末尾 7 个新单测（`deepseek_reasoning_fix_*` 前缀）：

- `removes_empty_sig_thinking_blocks` — empty 块被移除，text/tool_use 保留
- `omits_reasoning_content_when_all_empty` — 全 empty 时**不**加 `reasoning_content` 字段
- `keeps_nonempty_sig_and_adds_reasoning_content` — 单 UUID 块加 `reasoning_content`
- `concatenates_multiple_nonempty_blocks` — 多块用 `\n` 拼接
- `skips_user_messages` — user 消息完全不动（R4）
- `no_thinking_blocks_no_reasoning_content` — 纯 text + tool_use 不加 `reasoning_content`
- `preserves_top_level_thinking_field` — 顶层 `thinking: adaptive` 字段保留

`cargo test --lib` → **739 passed; 0 failed**（anthropic 18 + openai 35 + wire 20 + 其他 666；OpenAI 路径完全未触碰，符合 R4）。

###7. Out of Scope (Follow-up Tasks)

- **FT-D-001**: 调查 Anthropic 顶层 `thinking` 字段对 DeepSeek V4 后端的影响（D 方案 — 移除顶层 `thinking: adaptive` 是否会改变 400 行为；需要更直接 evidence 才能动 Claude extended thinking 路径）
- **FT-D-002**: 调查 wukaijin.com 400 threshold 的精确机制（DB 4 session 对比表明 threshold 不稳定，需要按 relay 分类的实测数据）
- **FT-D-003**: 评估是否需要按 relay 自动分发 capability（heuristic 或新 ModelRow 字段 `disable_reasoning_content_inject`），让 strict Anthropic relay 不接收 `reasoning_content` 顶层字段

