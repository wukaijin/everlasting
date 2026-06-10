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

## Scenario: Token Usage Tracking (A4, 2026-06-10)

### 1. Scope / Trigger

- Trigger: the agent loop needs per-session token totals to drive the
 ChatInput hint area (Anthropic-style statusline: "current context
 usage, not cumulative session totals", but scoped to a single
 session). The data must round-trip the LLM → Rust → SQLite →
 Pinia → ChatInput.vue without the agent loop ever touching
 protocol-specific field names.
- Why code-spec depth: mandatory — the new `TokenUsage` struct is
 the cross-layer contract that touches `ChatEvent::Done`, the
 Anthropic SSE parser, the OpenAI SSE parser, the agent loop's
 accumulation write, the DB schema, and the frontend's
 `chatStore.tokenUsageBySession`. A change here cascades through
 every layer.

### 2. Signatures

#### Backend types (`app/src-tauri/src/llm/types.rs`)

```rust
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenUsage {
 pub input_tokens: u32,
 pub output_tokens: u32,
 pub cache_creation_input_tokens: u32,
 pub cache_read_input_tokens: u32,
}

pub enum ChatEvent {
 // ... existing variants ...
 Done {
 stop_reason: Option<String>,
 usage: Option<TokenUsage>, // <-- A4 field
 },
 // ... existing variants ...
}
```

#### DB schema (`migrations.rs` A4 ALTER)

```sql
ALTER TABLE sessions ADD COLUMN input_tokens_total INTEGER;
ALTER TABLE sessions ADD COLUMN output_tokens_total INTEGER;
ALTER TABLE sessions ADD COLUMN cache_creation_total INTEGER;
ALTER TABLE sessions ADD COLUMN cache_read_total INTEGER;
```

All four columns are **nullable** (no `DEFAULT`); a pre-A4 session
keeps NULL until its first LLM turn post-upgrade, when
`add_token_usage` initializes them from 0 (via `COALESCE(col, 0) + ?`).

#### DB function (`app/src-tauri/src/db/sessions.rs`)

```rust
pub async fn add_token_usage(
 pool: &SqlitePool,
 session_id: &str,
 usage: &TokenUsage,
) -> Result<(), sqlx::Error> {
 // Single UPDATE: 4 columns added in place, updated_at bumped.
}
```

#### Frontend payload (`app/src/stores/streamController.ts`)

```typescript
interface ChatEventPayload {
 request_id: string;
 kind: "start" | "delta" | "..." | "done" | "error";
 // ... existing fields ...
 usage?: { // <-- A4 field; only present on `done` events
 input_tokens: number;
 output_tokens: number;
 cache_creation_input_tokens: number;
 cache_read_input_tokens: number;
 };
}
```

### 3. Contracts

#### Wire format (snake_case, both layers)

```jsonc
// ChatEvent::Done { usage: Some(t) } on the chat-event channel:
{
 "kind": "done",
 "stop_reason": "end_turn",
 "usage": {
 "input_tokens": 1234,
 "output_tokens": 56,
 "cache_creation_input_tokens": 100,
 "cache_read_input_tokens": 200
 }
}

// ChatEvent::Done { usage: None } (cancel / error / network drop):
{
 "kind": "done",
 "stop_reason": "cancelled",
 "usage": null
}
```

The IPC field is **snake_case** (the existing `kind` discriminator
and the existing `stop_reason` are both snake_case; mixing styles
here would break the `parse_*` symmetry on the TS side). Field
names mirror Rust's `TokenUsage` 1:1 — no `camelCase` rewrite on
the boundary (the outer `ChatEventPayload` is camelCase via
`#[serde(rename_all = "camelCase")]` on the Rust side at the
struct level, but the inner `usage` JSON object keeps snake_case
to match the user's request). **See "Wrong vs Correct" §7 for the
rationale.**

#### Anthropic protocol mapping

The Anthropic SSE `message_delta` event carries:

```jsonc
{
 "type": "message_delta",
 "delta": { "stop_reason": "end_turn" },
 "usage": {
 "input_tokens": 1234,
 "output_tokens": 56,
 "cache_creation_input_tokens": 100,
 "cache_read_input_tokens": 200
 }
}
```

The Anthropic adapter's `parse_anthropic_usage(usage_value)`
function (in `provider/anthropic.rs`) reads all four fields
verbatim. `usage` is **cumulative per turn** — the first
`message_delta` event for a turn typically reports `output_tokens:
1`; later ones carry the cumulative value. The adapter keeps the
**last seen** payload in a `let mut usage: Option<TokenUsage> = None`
local and yields it on the terminal `Done` event. A `usage: {}`
or all-zero payload is treated as `None` ("no usage") to skip the
agent loop's SQL write.

Some Anthropic-compatible proxies also attach `usage` to the
`message_start` event (an initial baseline). The adapter reads
this as the first non-null `usage` and lets subsequent
`message_delta.usage` overwrite it.

#### OpenAI protocol mapping

The OpenAI Chat Completions final chunk (when
`stream_options.include_usage: true` is set on the request body)
carries:

```jsonc
{
 "usage": {
 "prompt_tokens": 200,
 "completion_tokens": 30,
 "total_tokens": 230,
 "prompt_tokens_details": { "cached_tokens": 50 }
 }
}
```

The OpenAI adapter's `parse_openai_usage(usage_value)` function
(in `provider/openai.rs`) normalizes:

- `prompt_tokens` → `input_tokens`
- `completion_tokens` → `output_tokens`
- `prompt_tokens_details.cached_tokens` → `cache_read_input_tokens`
- `cache_creation_input_tokens` → 0 (no OpenAI equivalent today)

The adapter requires `stream_options: { include_usage: true }` on
the **request body** (set in `build_http_body`). Without this,
OpenAI omits the `usage` field on all chunks and the agent loop
has no per-turn token counts.

#### Agent loop accumulation (R2)

The agent loop's `ChatEvent::Done` handler in
`app/src-tauri/src/agent/chat.rs`:

```rust
ChatEvent::Done { stop_reason: sr, usage } => {
 stop_reason = sr.clone();
 if let Some(t) = usage {
 if let Err(e) = crate::db::add_token_usage(&db, &session_id, t).await {
 tracing::warn!(error = %e, "failed to accumulate token usage");
 }
 } else {
 tracing::info!("skipping token accumulation (no usage in Done)");
 }
}
```

The single SQL `UPDATE col = col + ?` is column-additive, so
multi-turn sessions converge on the cumulative total. The
`COALESCE(col, 0) + ?` pattern handles pre-A4 rows (NULL treated
as 0).

#### Frontend accumulation (`chat.ts` + `streamController.ts`)

`streamController.handleChatEvent("done")` calls
`useChatStore().accumulateTokenUsage(sid, event.usage)`. The chat
store's `tokenUsageBySession: reactive(Map)` holds the running
totals. `currentSessionTokenUsage` is a `computed` that the
ChatInput hint reads.

The map is also **seeded** from `SessionSummary` /
`load_session` results, so a page reload shows the cumulative
value (not "—") immediately.

#### Color thresholds (UI)

| Percentage of `context_window` | Color | CSS class |
|--------------------------------|-------|-----------|
| 0-49% | green (`#4ade80`) | `chat-input__token-usage--ok` |
| 50-74% | amber (`#fbbf24`) | `chat-input__token-usage--warn` |
| 75%+ | red (`var(--color-tool-error)`) | `chat-input__token-usage--alert` |

The 50% / 75% thresholds are the same as Anthropic's statusline
recommendation. The CSS uses `var()` for the red (a project
token) and direct hex for green / amber (Tailwind 400-family
colors, not in the design token system per
`.trellis/spec/frontend/design-tokens.md` "Don't add a new
`--color-*` token for a one-off use" rule).

### 4. Validation & Error Matrix

| Condition | Result |
|-----------|--------|
| `ANTHROPIC_API_KEY` missing at startup | LLM stream never opens; the chat command returns pre-flight `ChatEvent::Error`. No usage data is ever written. |
| Anthropic `message_delta` event has no `usage` field | `parse_anthropic_usage` returns `None`; the `usage` local stays `None`; agent loop's `if let Some(t) = usage` skips the write. |
| Anthropic `usage` is all-zero | `parse_anthropic_usage` returns `None` (deliberate — see §5 Base case). Agent loop skips the write. |
| Anthropic `usage` is `{}` (empty object) | `parse_anthropic_usage` returns `None`. |
| OpenAI request body missing `stream_options` | The OpenAI server omits `usage` on all chunks. `parse_openai_usage` returns `None` for every chunk. Agent loop skips the write. (The `build_http_body_includes_stream_options_for_usage` test asserts the field is always present.) |
| OpenAI `usage` chunk has `prompt_tokens_details: {}` | Defensive path: `parse_openai_usage` reads `cached_tokens` as missing → 0. The other three fields parse normally. |
| OpenAI `usage` is all-zero | `parse_openai_usage` returns `None`. Same deliberate contract as Anthropic. |
| Cancel mid-stream (user hits Stop) | `ChatEvent::Done { usage: None, stop_reason: "cancelled" }`. Agent loop skips the write, `tracing::info!` records the skip. |
| Network error mid-stream | `ChatEvent::Error { category: Network }`. The agent loop's `if had_error { return }` short-circuits before any `Done` is processed — the `usage` write is naturally skipped. |
| `add_token_usage` on missing session id | `UPDATE` matches 0 rows. `sqlx::Error` is not raised. `tracing::info!` would log success, but the write is a no-op. |
| `add_token_usage` on a session where columns are NULL | `COALESCE(col, 0) + ?` evaluates `col` to 0, writes `?`. Subsequent reads show `Some(value)`. |
| Session switch mid-stream (user views a different session) | The stream keeps running on the controller's `request_id`; the `done` event routes by `request_id` to the originating session, updates `tokenUsageBySession` for that session (not the user's current view). When the user returns to the streamed session, the `currentSessionTokenUsage` computed re-evaluates and shows the updated total. |
| Page reload after N turns | `list_sessions` returns `SessionSummary` with `input_tokens_total` etc. (not NULL). `onProjectChange` seeds the in-memory Map. The hint area shows the cumulative value on first paint. |
| Pre-A4 session (columns NULL) | UI renders "—" with the "升级前未统计" tooltip. The first post-upgrade turn starts the counters from 0. |

### 5. Good / Base / Bad Cases

#### Good: Anthropic happy path

1. User opens a session, sends a question, hits Send.
2. `chat` command resolves the catalog (Anthropic + claude-sonnet-4-5), builds `AnthropicProvider`.
3. `AnthropicProvider::send` streams:
 - `message_start { ... usage: { input_tokens: 0, output_tokens: 0, ... } }` (initial baseline; the `usage` local in the adapter is set to `Some(TokenUsage { 0,0,0,0 })` but the all-zero case is mapped to `None` first... actually re-read: the baseline sets `usage` only if it's currently `None`, but the all-zero check is in the inner `parse_anthropic_usage`. Net: a baseline `usage: { 0,0,0,0 }` returns `None`, so the `usage` local stays `None` and the next `message_delta` overwrites it. See `parse_anthropic_usage_zero_returns_none` test.)
 - `content_block_start` / `delta`s (text + tool_use + thinking).
 - `message_delta { delta: { stop_reason: "end_turn" }, usage: { input_tokens: 1234, output_tokens: 56, cache_creation_input_tokens: 100, cache_read_input_tokens: 200 } }` — the `usage` local is overwritten with the non-zero value.
 - `message_stop`.
4. The stream yields `ChatEvent::Done { stop_reason: Some("end_turn"), usage: Some(TokenUsage { 1234, 56, 100, 200 }) }`.
5. The agent loop's `if let Some(t) = usage { add_token_usage(...) }` runs the SQL UPDATE.
6. The frontend's `streamController.handleChatEvent("done")` sees the `usage` field and calls `useChatStore().accumulateTokenUsage(sid, t)`. The `tokenUsageBySession` map updates; `currentSessionTokenUsage` re-evaluates.
7. The ChatInput hint area re-renders: `1.2K · 1% / 200K` (assuming 1234 tokens is ~0.6% of 200K context_window). The color is `ok` (green).

#### Good: OpenAI happy path

1. Same flow, but the `chat` command's `resolve_chat_provider` returns an `OpenAIProvider` (the user has switched the default to a gpt-4o model).
2. The OpenAI adapter's `build_http_body` includes `"stream_options": { "include_usage": true }`.
3. The SSE stream emits normal text deltas, then a final chunk with `usage: { prompt_tokens: 200, completion_tokens: 30, total_tokens: 230, prompt_tokens_details: { cached_tokens: 50 } }` and no `choices`.
4. The adapter's `parse_openai_usage` normalizes: `input_tokens: 200, output_tokens: 30, cache_read_input_tokens: 50, cache_creation_input_tokens: 0`.
5. The agent loop + frontend flow identically to the Anthropic path.

#### Base: cancel mid-stream

1. User sends a question; LLM starts streaming.
2. User hits Stop. The cancellation token fires; the agent loop's `tokio::select!` notices on the next event boundary.
3. The agent loop bails out, persists whatever's been collected so far, and yields `ChatEvent::Done { stop_reason: Some("cancelled"), usage: None }`.
4. The frontend's `done` handler sees `usage` is undefined; `accumulateTokenUsage` is not called.
5. The SQL write is skipped. The session's column totals reflect all PRE-cancel turns (i.e. the cancel did NOT roll anything back). This is the correct behavior — a user who cancelled 3 turns in still has those 3 turns' usage on the dashboard.

#### Bad: OpenAI stream without `include_usage`

1. (Anti-pattern, NOT the implementation) The `build_http_body` does not set `stream_options.include_usage: true`.
2. OpenAI omits the `usage` field on every chunk.
3. `parse_openai_usage` returns `None` for every chunk. The adapter's `usage` local stays `None`.
4. The agent loop's `if let Some(t) = usage` is never true. No SQL write happens.
5. The user opens the app, sends a message, gets a response, and the ChatInput hint shows "—" (or the pre-A4 value). They report "token usage tracking doesn't work for OpenAI".

**Fix**: `build_http_body` must always set
`"stream_options": { "include_usage": true }`. The
`build_http_body_includes_stream_options_for_usage` test asserts
this is the case.

#### Bad: persistent strip of token usage

1. (Anti-pattern, NOT the implementation) The agent loop "saves tokens" by writing only `output_tokens` to the DB and discarding the other three.
2. The UI shows only output. The user has no visibility into the cache hit rate, the context pressure, or the cumulative input growth.
3. **Fix (A4 doesn't do this)**: the 4 columns are persisted verbatim. Future PRs (B6 subagent token quotas, A5 $ cost conversion) can read any of the four fields independently.

### 6. Tests Required

#### Backend (cargo test)

**`llm::types` (4 new tests)**

- `token_usage_serializes_with_snake_case_fields`
- `token_usage_default_is_all_zero`
- `token_usage_add_assign_saturates_at_u32_max`
- `chat_event_done_carries_usage_payload`
- `chat_event_done_with_none_usage_emits_null`

**`llm::provider::anthropic` (4 new tests)**

- `parse_anthropic_usage_full_payload`
- `parse_anthropic_usage_minimal_payload`
- `parse_anthropic_usage_zero_returns_none`
- `parse_anthropic_usage_empty_object_returns_none`

**`llm::provider::openai` (6 new tests)**

- `build_http_body_includes_stream_options_for_usage`
- `parse_openai_usage_full_payload`
- `parse_openai_usage_minimal_payload`
- `parse_openai_usage_no_usage_key_returns_none`
- `parse_openai_usage_zero_returns_none`
- `parse_openai_usage_empty_prompt_tokens_details`

**`db::sessions` (4 new tests, in `db::tests`)**

- `add_token_usage_first_turn_initializes_columns`
- `add_token_usage_accumulates_across_turns`
- `add_token_usage_on_missing_session_is_noop`
- `list_sessions_includes_token_columns`

Total A4 tests: **18 new cargo tests** (rounds up to 285 from 281;
the implementation also has a few additional tests for chat_event
done serialization edge cases).

#### Frontend

- `pnpm build` (vue-tsc strict) must pass.
- Manual smoke test (acceptance A2 from the parent PRD):
 1. `cd app && pnpm tauri dev`
 2. Open a session, send a question, click Send.
 3. Observe the ChatInput hint area shows "X · Y% / 200K" (e.g. "1.2K · 1% / 200K"), green color (under 50%).
 4. After 4-5 turns, observe the percentage climbs. Watch the color shift to yellow at 50%, red at 75%.
 5. Hover the chip, observe the tooltip shows the four counters (input / cache_read / cache_creation / output).
 6. Open Settings, delete the model's `api_key` (or the model entirely). Send a message — observe the pre-flight error and "token usage not accumulating" (the agent loop never reached `add_token_usage`).
 7. Page reload. Observe the hint area still shows the cumulative value (seeded from `list_sessions`).

### 7. Wrong vs Correct

#### Wrong: snake_case in the inner `usage` object, but camelCase outer

```rust
// BAD — mixed naming convention
#[derive(Serialize)]
#[serde(rename_all = "camelCase")] // applied at the struct level
pub struct TokenUsagePayload {
 pub input_tokens: u32, // → "inputTokens" on the wire
 pub output_tokens: u32, // → "outputTokens"
 // ...
}

// Resulting IPC payload:
{
 "kind": "done",
 "stopReason": "end_turn",
 "usage": { "inputTokens": 1234, "outputTokens": 56, ... }
}
```

The frontend's TypeScript interface then has to mix `stopReason` (camelCase
outer) with `inputTokens` (camelCase inner usage) — a 2-style
"polyglot" payload that's hard to grep, hard to map to Anthropic
or OpenAI field names, and breaks the user's mental model of
"snake_case from the LLM, snake_case on the wire".

#### Correct: snake_case throughout the inner object

```rust
// GOOD — match the rest of the wire payload's snake_case
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenUsage {
 pub input_tokens: u32,
 pub output_tokens: u32,
 pub cache_creation_input_tokens: u32,
 pub cache_read_input_tokens: u32,
}

// Resulting IPC payload (mixed at the outer/inner boundary;
// outer is the parent `ChatEventPayload` which has its own
// serde rename; inner is the raw Rust struct field names):
{
 "kind": "done",
 "stopReason": "end_turn", // outer ChatEventPayload camelCase
 "usage": {
 "input_tokens": 1234, // inner TokenUsage snake_case
 "output_tokens": 56,
 "cache_creation_input_tokens": 100,
 "cache_read_input_tokens": 200
 }
}
```

The frontend reads `event.usage.input_tokens` (matching the Rust
struct's field name verbatim). Anthropic / OpenAI API
documentation (which uses snake_case for the same fields) reads
1:1 with the IPC payload, making cross-referencing easy.

#### Wrong: agent loop branches on protocol

```rust
// BAD — agent loop checks Anthropic vs OpenAI to read the
// usage field
match event {
 ChatEvent::Done { stop_reason, usage } => {
 // ... accumulates ...
 // But what if some future protocol's `Done` carries a
 // different shape? The agent loop would need a new match arm.
 }
}
```

The Provider abstraction is leaky if the agent loop has to know
which protocol emitted the event.

#### Correct: provider-normalized payload

```rust
// GOOD — provider adapter normalizes; agent loop sees
// protocol-agnostic 4 fields
// In AnthropicProvider::parse_anthropic_usage:
let u = TokenUsage {
 input_tokens: v.get("input_tokens")...as u32,
 output_tokens: v.get("output_tokens")...as u32,
 cache_creation_input_tokens: v.get("cache_creation_input_tokens")...as u32,
 cache_read_input_tokens: v.get("cache_read_input_tokens")...as u32,
};
// In OpenAIProvider::parse_openai_usage:
let u = TokenUsage {
 input_tokens: prompt,
 output_tokens: completion,
 cache_creation_input_tokens: 0, // no OpenAI equivalent
 cache_read_input_tokens: cached,
};
// Both yield:
// ChatEvent::Done { stop_reason, usage: Some(TokenUsage { 4 fields }) }
// Agent loop is protocol-agnostic.
```

Future protocols (Gemini, Ollama) plug in by writing their own
`parse_<protocol>_usage` function. The agent loop and the DB
schema don't change.

#### Wrong: persist the cumulative usage in the message row

```rust
// BAD — every assistant turn stores its own usage in the
// messages table; per-session total becomes a SUM() query on
// every read
INSERT INTO messages (..., input_tokens, output_tokens, ...)
```

This requires a `messages` schema change (out of scope per the
PRD's "排除" list), and the per-turn granularity is overkill
for the A4 hint (the PRD explicitly defers per-turn to "后续
C3 / B6 阶段").

#### Correct: per-session cumulative on the sessions row

```rust
// GOOD — single SQL UPDATE on the sessions row, additive
// on the existing column values
UPDATE sessions
 SET input_tokens_total = COALESCE(input_tokens_total, 0) + ?,
 output_tokens_total = COALESCE(output_tokens_total, 0) + ?,
 cache_creation_total = COALESCE(cache_creation_total, 0) + ?,
 cache_read_total = COALESCE(cache_read_total, 0) + ?,
 updated_at = ?
 WHERE id = ?
```

The hint area reads the cumulative value with no aggregation.
Future C3 / B6 work can ALTER `messages` to add per-turn columns
without changing the A4 schema.

### Design Decisions

#### Decision: Anthropic also goes through the wire layer's usage normalization

**Context**: The A4 scope is "per-session token accumulation +
ChatInput hint". The cross-protocol question is: does
Anthropic's `message_delta.usage` parsing live in the
`AnthropicProvider` (and the agent loop) or in a shared
helper?

**Decision**: Provider-private parsing. `parse_anthropic_usage`
is a private free function in `provider/anthropic.rs`;
`parse_openai_usage` is private in `provider/openai.rs`. The
wire layer (`provider/wire.rs`) is unchanged — usage is not a
block-level concept, it's a stream-level one, and the wire
layer deals with `ContentBlock` round-trips not SSE-side
metadata.

**Consequences**:
- ✅ Anthropic and OpenAI each handle their own protocol's
 quirks (Anthropic's `message_delta` vs OpenAI's
 `data-only` SSE; Anthropic's `cache_*` fields vs OpenAI's
 `prompt_tokens_details.cached_tokens`).
- ✅ The wire layer is not contaminated with token-usage
 types — it stays focused on block-level cross-protocol
 conversion.
- ⚠️ A future "Gemini usage" function would be a 3rd private
 helper in `provider/gemini.rs`. The pattern is clear, the
 cost is one duplicated `Option`-handling decision per
 protocol.

#### Decision: in-memory accumulation on the frontend, not in the DB schema

**Context**: The frontend (Pinia store) and the backend (SQLite)
both need to know the cumulative per-session totals. Where does
the running total live?

**Decision**: The DB stores the per-session cumulative
(`sessions.*_total` columns, updated on every `Done` event). The
frontend also has a `tokenUsageBySession` Map that's seeded
from `list_sessions` and incremented on every `done` event with
a `usage` payload. The DB is the source of truth; the frontend
Map is a projection of the DB for live updates.

**Consequences**:
- ✅ A page reload shows the right number (seeded from DB).
- ✅ No need for a separate `token-usage` IPC command (the
 chat command's `done` event carries everything the
 frontend needs for live updates; the SessionSummary carries
 the historical total).
- ✅ The frontend can show a per-session usage chip in the
 sidebar (not in this PRD's scope, but the data is there
 in `SessionSummary` for a future PR to wire up).
- ⚠️ A reload mid-stream would show the pre-stream value
 (the in-flight `done` event hasn't fired yet). This is
 acceptable — the streaming session is ephemeral.

#### Decision: percentage denominator is the default model's `context_window`, not the session's `model_id`

**Context**: A session can override its model via
`sessions.model_id` (per-session model override). The model's
`context_window` varies (Sonnet 200K, Haiku 200K, GPT-4o
128K). The percentage denominator should match the model that's
actually being called.

**Decision**: The percentage uses
`modelsStore.defaultModel.contextWindow`. Reasoning:
- The chat command's `resolve_chat_provider` already
 resolves a session override to a specific model; the
 frontend doesn't easily track "which model this session
 last used" without another IPC.
- The visual stability is better (the denominator doesn't
 flicker when the user opens a session whose last-used
 model had a different window than the current default).
- The PRD explicitly scopes "current context usage, not
 cumulative" — the *current* default's window is the most
 useful visual baseline.

**Consequences**:
- ✅ The percentage always shows "X% / 200K" (or whatever
 the current default is) — stable across sessions.
- ⚠️ A session mid-flight on a per-session override with a
 smaller context_window (e.g. 128K) would show a
 *under*-estimated percentage. Future PR can thread the
 effective model's window through the IPC if this becomes
 a real problem.

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
