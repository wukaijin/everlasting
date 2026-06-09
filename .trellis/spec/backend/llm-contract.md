# LLM API Contract —核心类型与思考契约

> **基线**:2026-06-10 commit `0f9a167` (8-PR5拆分后)
> **来源**:从原 `llm-contract.md` (3149 行)拆出本文件 +4 个子文件 (`tool-contract.md` / `worktree-contract.md` / `multi-provider-contract.md` / `test-model-contract.md`)
> **同源文档**:
> - [llm-contract.md](./llm-contract.md) (本文) —核心类型 + Extended Thinking + 反模式汇总
> - [tool-contract.md](./tool-contract.md) —工具定义 + ReadGuard + shell spillover
> - [worktree-contract.md](./worktree-contract.md) — attach/detach/delete + cancel + system prompt
> - [multi-provider-contract.md](./multi-provider-contract.md) — Provider trait + catalog + Anthropic/OpenAI 分发
> - [test-model-contract.md](./test-model-contract.md) — `test_model` IPC
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
 schema. The current dev setup uses `wukaijin.com`'s Claude-compat endpoint; the
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
| `ANTHROPIC_API_KEY` (or `ANTHROPIC_AUTH_TOKEN`) | yes | — | The dev setup uses `wukaijin.com` proxy tokens; `ANTHROPIC_AUTH_TOKEN` is the legacy alias. |
| `ANTHROPIC_BASE_URL` | no | `https://api.anthropic.com` | Trailing `/v1/messages` is appended by `LlmConfig::endpoint()`. |
| `LLM_MODEL` | no | `GLM-4.7` | |
| `LLM_MAX_TOKENS` | no | `16384` | Was `1024` before step6. |
| `LLM_THINKING_EFFORT` | no | `high` | Adaptive thinking effort. |

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

---

## Common Mistakes

### Mistake: Treating `redacted_thinking` as a partial `thinking`

The `data` field of `redacted_thinking` is opaque — Anthropic redacts it for
safety reasons. Don't try to "decode" it, don't append it to the visible
thinking text, don't mutate it. Store and forward verbatim. The UI shows a
"🔒 redacted" placeholder.

### Mistake: Dropping the signature to "save space"

The `signature` is a cryptographic anchor — Anthropic uses it to verify the
thinking block was generated by the same session. Drop it, and the next turn
400s. The DB stores it in full; the rehydrate path emits it in full.

### Mistake: Ordering thinking blocks after tool_use in history

Anthropic requires thinking blocks at the head of an assistant message. If
`toPayloadContent` appends thinking at the tail (or interleaves it with
tool_use), the next turn400s. The fix lives in `toPayloadContent`; the
ordering is tested in the frontend type-check.

---

## Anti-Patterns

- **Don't** add a per-session or per-request thinking toggle. (MVP simplification;
 see PRD D1.)
- **Don't** add an `LLM_THINKING=off` env var. (PRD D5; if the upstream is broken,
 fix the upstream or change code.)
- **Don't** parse `signature` client-side. It is opaque; the proxy might transform
 it, but the wire format is "string of unknown base64-like data."
- **Don't** coalesce multiple thinking blocks into one on rehydrate. The
 interleaving is meaningful when it happens (rare), and Anthropic expects
 the same order on round-trip.

---

## Future Work (Deferred from Step6)

| Item | Why deferred |
|------|-------------|
| Parse `usage.output_tokens_details.thinking_tokens` from `message_delta` | UI uses `length /4` estimate; real count would require a new `ChatEvent` variant + plumbing. |
| Preserve interleaved `thinking → redacted_thinking` order | Redacted is rare and interleaved is even rarer; current code appends redacted to the tail. |
| Coalesce `text → text` into one block | Multiple text blocks in one turn are coalesced; interleaved `thinking → text → thinking → text` loses the second text's position. |
| Fix pre-existing `tool_result` in `assistant` role | From step3a; out of step6 scope. Follow-up task. |
| OpenAI-compat `reasoning_content` fallback | Different wire format entirely; would require its own `ContentBlock` variant + parser. (Cross-protocol handled by Wire layer — see multi-provider-contract.md.) |
