# Scenario: Extended Thinking Support (Step6)

> **Source**: extracted from `llm-contract.md` §"Scenario: Extended Thinking Support (Step6)"
> (2026-08-10 doc-split task)。Extended Thinking 完整契约(7 段:Scope /
> Signatures / Contracts / Validation / Cases / Tests / Wrong-vs-Correct)。

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
 "model": "<from DB catalog models.model_name>",
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
- `thinking.effort` is sourced from the DB catalog (`models.thinking_effort`, default `"high"`).
 - Valid values: `low` / `medium` / `high` / `xhigh` / `max` (Anthropic schema).
 - Invalid values pass through unchanged; the upstream API will reject them.
- `max_tokens` default is `16384` (was `1024` in step2; bumped in step6 because
 thinking tokens count against the same budget as the actual answer). Sourced from
 `models.max_tokens` when set.

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

**项目不读任何 LLM 相关 env 变量。** provider / model / api_key / base_url / max_tokens / thinking_effort 全部来自 DB catalog(`providers` / `models` / `app_config` 表),由 UI Settings 配置。历史上曾有 `ANTHROPIC_API_KEY` / `ANTHROPIC_BASE_URL` / `LLM_MODEL` / `LLM_MAX_TOKENS` / `LLM_THINKING_EFFORT` 等 env 兜底路径(`LlmConfig::from_env`),在 multi-model catalog 架构稳定后已移除。

`ANTHROPIC_API_KEY` 仍作为**敏感变量名**出现在 `tools/shell.rs` 的 shell 命令环境变量脱敏清单里(执行 shell 命令前擦除),与 LLM 配置无关。

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
| `default_model_id` unset or model row missing at chat time | `PreFlightError::NoModel` → chat emits `ChatEvent::Error` with "没有可用 model,请到 Settings选 default model". App still launches (no env to check at startup). |
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
| `thinking_config_is_adaptive_summarized_with_configured_effort` | `LlmConfig` (constructed directly) always sets `display: "summarized"` and threads the configured `effort`. |
| ~~`unconfigured_has_empty_thinking_effort`~~ | **Removed** — `LlmConfig::unconfigured()` was deleted along with the env fallback path. |

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
