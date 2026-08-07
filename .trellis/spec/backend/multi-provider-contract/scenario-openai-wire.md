## Future Work (Deferred from PR2 → resolved in PR3)

| Item | Why deferred / resolved |
|------|-------------------------|
| OpenAI adapter | **Resolved in PR3** — `app/src-tauri/src/llm/provider/openai.rs` ships `OpenAIProvider::new` + `impl Provider`; see the PR3 section below. |
| Cross-protocol `WireMessage` intermediate type | **Resolved in PR3** — `app/src-tauri/src/llm/provider/wire/`(08-07 拆分为 types/to_wire/from_wire)ships `WireRequest` / `WireMessage` / `WireBlock` / `WireCapabilities` + `chat_request_to_wire` + `strip_unsupported` + `wire_messages_to_chat_messages`. |
| `ProviderCapabilities`-gated dispatch | Anthropic + OpenAI both support system + tools + streaming; capability gating is a no-op until a future protocol (Gemini? Ollama?) diverges. PR3 also adds the model-level `WireCapabilities` struct used at the wire layer for the cross-protocol strip pass. |
| Provider-level API key redaction (so `api_key` is never logged even in `tracing::debug!`) | Deferred. The PR3 `OpenAIProvider::send` info log does not include the key (only `url`, `model`, `tools_count`, `has_system`); the same is true for the PR2 `chat_stream_with_tools`. Explicit redaction is a defensive layer a future PR should add. |

---

## Scenario: OpenAI Chat Completions adapter + cross-protocol WireMessage (PR3 of06-08-multi-model-llm-provider-planning)

###1. Scope / Trigger

- Trigger: PR2 shipped the catalog + `Provider` trait dispatch
 with a real Anthropic adapter and a stub `OpenAI` branch.
 PR3 closes the loop: implement `OpenAIProvider` (Chat
 Completions streaming), introduce a `WireMessage`
 intermediate layer so both providers share a single
 cross-protocol conversion + strip path, and lock the
 cross-protocol degradation rules (parent PRD §Q5 H1
 decision: "switching model silently drops the wire
 blocks the new model can't represent").
- Why code-spec depth: mandatory — the wire layer is the
 single place that knows how to map between
 Anthropic-shaped `ChatRequest` / `ChatEvent` and the
 provider-specific wire payloads. A bug here cascades to
 every future protocol (Gemini, Ollama, …).

###2. Signatures

#### Wire layer (`app/src-tauri/src/llm/provider/wire/` — new,08-07 拆分为 types/to_wire/from_wire)

```rust
pub struct WireRequest {
 pub model: String,
 pub max_tokens: Option<u32>,
 pub system: Option<String>,
 pub messages: Vec<WireMessage>,
 pub tools: Vec<WireTool>,
 pub reasoning_effort: Option<String>, // OpenAI o1/o3
}

pub enum WireMessage {
 User { content: String },
 Assistant { blocks: Vec<WireBlock> },
 Tool { tool_call_id: String, content: String },
}

pub enum WireBlock {
 Text { text: String },
 Reasoning { text: String }, // Anthropic thinking / OpenAI reasoning_content
 Signature { data: String }, // Anthropic-only
 RedactedThinking { data: String }, // Anthropic-only
 ToolUse { id: String, name: String, input: serde_json::Value },
}

pub struct WireTool {
 pub name: String,
 pub description: Option<String>,
 pub input_schema: serde_json::Value,
}

pub struct WireCapabilities {
 pub supports_thinking: bool,
 pub supports_reasoning_effort: bool,
 pub supports_thinking_signatures: bool,
}

pub fn chat_request_to_wire(req: ChatRequest, system: Option<String>) -> WireRequest;
pub fn strip_unsupported(messages: Vec<WireMessage>, caps: &WireCapabilities) -> Vec<WireMessage>;
pub fn wire_messages_to_chat_messages(messages: Vec<WireMessage>) -> Vec<ChatMessage>;
```

#### `OpenAIConfig` (`app/src-tauri/src/llm/provider/openai.rs` — new)

```rust
pub struct OpenAIConfig {
 pub base_url: String,
 pub model: String,
 pub api_key: String,
 pub max_tokens: u32,
 pub reasoning_effort: Option<String>, // from ModelRow.thinking_effort
}

pub struct OpenAIProvider { config: OpenAIConfig }
impl OpenAIProvider { pub fn new(config: OpenAIConfig) -> Self; }
impl Provider for OpenAIProvider { ... }
```

###3. Contracts

#### Protocol differences (the only spec table PR3 needs)

| Concern | Anthropic (PR2) | OpenAI (PR3) |
|---------|-----------------|---------------|
| URL | `provider.base_url + "/v1/messages"` | `provider.base_url + "/chat/completions"` (base_url MUST include `/v1`) |
| Auth | `x-api-key: <key>` + `anthropic-version:2023-06-01` | `Authorization: Bearer <key>` |
| System prompt | top-level `system` field | first `role: "system"` message |
| Tools | `[ToolDef]` (Anthropic) | `[{type: "function", function: {name, description, parameters}}]` |
| Tool call | `tool_use` block in `content[]` | `tool_calls[]` array of `{index, id, function: {name, arguments: "<json-string>"}}` |
| Tool result | `role: "user"` + `tool_result` block | independent `role: "tool"` message + `tool_call_id` |
| Text delta | `content_block_delta.text_delta` | `choices[0].delta.content` |
| Reasoning | `thinking_delta` block (Anthropic SSE) | `choices[0].delta.reasoning_content` (OpenAI o1/o3) |
| Finish | `message_delta.stop_reason` + `message_stop` | `choices[0].finish_reason` + `data: [DONE]` |
| Error body | `{"error": {"type": "<class>", "message": "..."}}` | `{"error": {"code": "<class>", "message": "..."}}` |
| Stream format | `event: ...\ndata: {...}\n\n` (typed) | `data: {...}\n\n` (data-only) |

**`base_url` convention is per-protocol, NOT symmetric:**

| Protocol | Seed base_url (PR1) | What adapter appends | Final URL |
|----------|---------------------|----------------------|-----------|
| Anthropic | `https://api.anthropic.com` (no `/v1`) | `/v1/messages` | `https://api.anthropic.com/v1/messages` |
| OpenAI | `https://api.openai.com/v1` (with `/v1`) | `/chat/completions` | `https://api.openai.com/v1/chat/completions` |

This **mismatch is intentional**: Anthropic historically exposes `https://<host>` as the protocol root and the API lives at `/v1/messages`; OpenAI exposes `https://<host>/v1` as the versioned root and the completion endpoint is `/chat/completions` (the `/v1` is part of the host path, not an endpoint component). The convention matches `test_model` / `test_provider` in `lib.rs` (which both pass `provider.base_url` straight through and append the path component).

**BUG FIX (06-09-fix-session):** prior to this fix, `OpenAIConfig::endpoint()` appended `/v1/chat/completions` (matching the table row above the table's convention was already documented as `+ "/v1/chat/completions"`). Against any realistic `base_url` that already included `/v1` (the seed and every real OpenAI-compatible proxy like `https://api.deepseek.com/v1`), this produced `/v1/v1/chat/completions` and the upstream404'd with `path not found: /v1/v1/chat/completions`. The SSE parser never saw a stream, the agent loop emitted `ChatEvent::Error`, and `streamController.finalizeRequest` evicted the in-memory cache (per the8509bff2013-wire-invariant fix) so the UI landed on the empty state — exactly the "新 session发送消息，闪一下变空" symptom. The fix updates the table + code + spec in lockstep.

#### `strip_unsupported` decision matrix

| `WireBlock` variant | `supports_thinking` | `supports_reasoning_effort` | `supports_thinking_signatures` | Outcome |
|---------------------|---------------------|-----------------------------|----------------------------------|---------|
| `Text` | * | * | * | keep |
| `ToolUse` | * | * | * | keep |
| `Reasoning` | true | * | * | keep → Anthropic thinking block |
| `Reasoning` | false | true | * | keep → OpenAI `reasoning_content` stream |
| `Reasoning` | false | false | * | **drop** |
| `Signature` | * | * | true | keep |
| `Signature` | * | * | false | **drop** |
| `RedactedThinking` | * | * | true | keep |
| `RedactedThinking` | * | * | false | **drop** |

`User` and `Tool` messages are passed through unchanged.

#### `OpenAIProvider::send` flow

```text
ChatRequest --(chat_request_to_wire)--> WireRequest
 |
 v
 (strip_unsupported, openai caps)
 |
 v
 WireRequest
 |
 v
 (build_http_body: openai-shape)
 |
 v
 POST {base}/chat/completions
 |
 v
 SSE stream (data-only)
 |
 v
 choices[0].delta.{content, reasoning_content, tool_calls}
 choices[0].finish_reason + data: [DONE]
 |
 v
 Stream<ChatEvent>
```

#### `AnthropicProvider::send` (PR3 cross-protocol symmetry)

The Anthropic adapter also runs the request through the wire
layer (decision D1 — symmetry). The flow:

```text
ChatRequest --(chat_request_to_wire)--> WireRequest
 |
 v
 (strip_unsupported, anthropic caps — no-op)
 |
 v
 WireRequest
 |
 v
 (wire_messages_to_chat_messages) -> ChatRequest
 |
 v
 chat_stream_with_tools(req) // unchanged SSE parser
```

`strip_unsupported` is a no-op when caps say "support
everything" (Anthropic target on Anthropic source). The
inverse function reconstitutes the Anthropic-shaped
`ChatRequest` the legacy SSE parser understands, so the
rest of the call chain is byte-for-byte the same as
pre-PR3.

#### Error classification (PR3 extension)

`classify_error_response` in `error.rs` now reads BOTH
`error.type` (Anthropic / GLM convention) and `error.code`
(OpenAI convention). The keyword-match logic picks the
field that contains a classification keyword
(`authentication` / `new_api_error` / `invalid_api_key` for
Auth, `rate_limit` for RateLimit, `invalid_request` for
InvalidRequest). If neither field carries a useful
keyword, the function falls back to the HTTP status
(5xx → Server,4xx → InvalidRequest). Net effect: same5
`LlmError` categories, both protocols.

#### DB / persistence (PR3 doesn't change)

- `providers.protocol = "openai"` is now a real dispatch
 path; the existing catalog + seed code (PR1) is
 unchanged.
- `ModelRow.thinking_effort` is dual-purpose: it
 configures `Anthropic.adaptive.effort` (PR2) and the
 top-level `reasoning_effort` field on OpenAI requests
 (PR3). `None` means "do not emit" on either side.
- The wire strip is **in-memory only**. The DB stores the
 full assistant turn (text + thinking + signature +
 redacted_thinking) regardless of which model the user
 switches to. Strip only affects what goes on the wire
 this turn.

###4. Validation & Error Matrix

| Condition | Result |
|-----------|--------|
| `OpenAIProvider::send` on an empty `messages` | Body has `messages: []` (valid) |
| `messages` contains a `Thinking` block, target is OpenAI | Strip keeps as `[reasoning] <text>` in content (cross-protocol history marker) |
| `messages` contains a `Signature` block, target is OpenAI | Strip drops the block entirely |
| `messages` contains a `RedactedThinking` block, target is OpenAI | Strip drops the block |
| OpenAI401 with `error.code = "invalid_api_key"` | `LlmError::Auth` |
| OpenAI429 with `error.code = "rate_limit_exceeded"` | `LlmError::RateLimit` |
| OpenAI400 with `error.code = "invalid_request_error"` | `LlmError::InvalidRequest` |
| OpenAI5xx | `LlmError::Server` |
| `delta.tool_calls[i]` with no `function.name` (defensive) | Buffer accumulates; emit-time check returns `None` and skips the broken tool call with a `tracing::warn!` |
| `delta.tool_calls[i].function.arguments` is partial JSON | Buffer accumulates; emit-time parse falls back to `{}` on parse error |
| `data: [DONE]` arrives with in-flight tool_call buffers | Defensive: emit any unfinished tool calls before terminating the stream |
| `AnthropicProvider::send` with `signature` block in history | Strip keeps it; the inverse `wire_messages_to_chat_messages` reconstitutes the `Thinking` block with the signature intact (no Anthropic round-trip regression) |

###5. Good / Base / Bad Cases

#### Good: OpenAI gpt-4o happy path

1. User opens Settings, adds an OpenAI provider with their
 `sk-...` key, adds a `gpt-4o` model under it, sets it as
 the default.
2. User opens a session, types a question, clicks Send.
3. `resolve_chat_provider` resolves the gpt-4o
 `ModelWithProvider` and the OpenAI provider row.
4. `build_provider` constructs an `OpenAIProvider` with
 `max_tokens =16384`, `reasoning_effort = None`.
5. `OpenAIProvider::send` runs `chat_request_to_wire` →
 `strip_unsupported` (drops any prior thinking /
 signature blocks silently) → `build_http_body` →
 `POST https://api.openai.com/v1/chat/completions` with
 `Authorization: Bearer ...`.
6. SSE stream: text deltas arrive as `ChatEvent::Delta`,
 tool_calls arrive fully-assembled as `ChatEvent::ToolCall`,
 finish_reason arrives as `ChatEvent::Done { stop_reason:
 "end_turn" }` (normalized from OpenAI's `"stop"`).
7. The chat command's agent loop continues identically to
 the Anthropic path — same `ChatEvent` stream, same
 persistence, same tool envelope.

#### Good: switch from Claude to gpt-4o mid-session

1. User has an active session on `claude-sonnet-4-5`; the
 assistant's last turn emitted a `thinking` block with a
 signature.
2. User opens Settings, sets `gpt-4o` as the default model.
3. User sends a follow-up message in the same session.
4. `resolve_chat_provider` returns the OpenAI provider.
5. The history that goes on the wire includes the
 `Thinking` block from the prior Anthropic turn. The
 `OpenAIProvider::send` runs `strip_unsupported` with
 OpenAI caps (`supports_thinking = false`,
 `supports_reasoning_effort = true` since the gpt-4o
 model row has no `thinking_effort` set → `false` in
 practice): the `Signature` block is dropped
 (opaque — not mappable), the `Reasoning` block is
 dropped (no reasoning target). The `Text` block is
 kept.
6. The DB still has the full Thinking + Signature
 blocks; only the wire payload is degraded.

#### Base: OpenAI401 with new-style error body

1. User's OpenAI key is invalid; first request returns:
```json
{ "error": { "code": "invalid_api_key", "message": "Incorrect API key provided", "type": "error" } }
```
2. `classify_error_response(401, body)` reads `error.type`
 first (literal `"error"`) — no keyword match. Then
 reads `error.code` (`"invalid_api_key"`) — matches
 `invalid_api_key` → `LlmError::Auth`.
3. The chat command emits
 `ChatEvent::Error { message: "API key 无效或已过期...",
 category: Auth }`.

#### Bad: stripping on the wrong side

1. (Pre-PR3 v1 implementation) `strip_unsupported` lived
 inside `OpenAIProvider::send` and ran on
 `Vec<ContentBlock>` (Anthropic-shaped) instead of
 `Vec<WireBlock>`.
2. Switching from Anthropic to OpenAI would attempt to
 strip `ContentBlock::Signature` directly — but the
 function signature expected `WireBlock::Signature`,
 producing a confusing type error on the first attempt
 to compile the adapter.
3. Fix: `strip_unsupported` lives in the wire module and
 takes `Vec<WireMessage>` + `&WireCapabilities`. Both
 providers call it. The Anthropic provider's call is
 observably a no-op for Anthropic→Anthropic, but the
 code path is the same as OpenAI's.

#### Bad: persistent strip on the wrong default

1. (Anti-pattern, NOT the implementation) Strip once on
 model switch, persist the stripped form to the DB.
2. User switches back to a thinking-capable model: the
 thinking blocks are GONE from history; the LLM has no
 memory of its prior reasoning.
3. Fix (PR3 doesn't do this): strip is in-memory only;
 the DB stores the full turn. The DB shape is
 independent of the active default model.

###6. Tests Required

#### Wire layer (`wire::tests`)

| Test | Asserts |
|------|---------|
| `caps_anthropic_with_thinking_signatures_supported` | All3 caps true for Anthropic + thinking-effort-set model |
| `caps_openai_drops_signatures_even_with_effort` | OpenAI: `supports_thinking_signatures` is false |
| `caps_no_effort_disables_reasoning_effort` | `reasoning_effort = None` → `supports_reasoning_effort = false` |
| `chat_request_to_wire_preserves_system_and_tools` | System + tools come through unchanged |
| `chat_request_to_wire_lifts_tool_results_out_of_user_message` | A `role: "user"` with N `tool_result` blocks + interleaved text fans out to N+1 wire messages in order |
| `chat_request_to_wire_thinking_block_splits_reasoning_and_signature` | Anthropic `Thinking { thinking, signature }` → `Reasoning { text }` (signature split out for independent strip) |
| `strip_drops_signature_when_target_cant_carry_it` | OpenAI target: Signature dropped, Reasoning kept if `reasoning_effort = true` |
| `strip_drops_reasoning_when_target_has_no_thinking_or_reasoning` | gpt-4o (no thinking, no reasoning effort) → Reasoning dropped |
| `strip_keeps_tool_use_and_text_always` | Worst-case caps: ToolUse + Text survive |
| `strip_drops_redacted_thinking_on_cross_protocol` | OpenAI target: RedactedThinking dropped |
| `strip_preserves_user_and_tool_messages_unchanged` | User / Tool messages flow through unchanged |
| `strip_keeps_signature_for_anthropic_target` | Anthropic→Anthropic: signature survives strip |
| `wire_block_text_to_chat_event_delta` | `Text` → `ChatEvent::Delta` |
| `wire_block_reasoning_to_chat_event_thinking_delta` | `Reasoning` → `ChatEvent::ThinkingDelta` |
| `wire_block_tool_use_to_chat_event_tool_call` | `ToolUse` → `ChatEvent::ToolCall { id, name, input }` |
| `wire_block_redacted_thinking_to_chat_event_redacted_delta` | `RedactedThinking` → `ChatEvent::RedactedThinkingDelta` |

#### OpenAI adapter (`openai::tests`)

| Test | Asserts |
|------|---------|
| `endpoint_trims_trailing_slash` | `base_url = "https://x.com/v1/"` → endpoint has no double slash |
| `endpoint_uses_provided_base_url` | Custom proxy base URL works |
| `endpoint_does_not_double_prefix_v1_when_base_url_includes_v1` | `base_url = "https://api.openai.com/v1"` and `base_url = "https://api.deepseek.com/v1"` both produce `<base>/chat/completions` (no `/v1/v1/`) |
| `openai_provider_reports_openai_capabilities_and_protocol` | `protocol() == Openai`, all3 caps true |
| `openai_provider_is_send_sync` | `Send + Sync` (compile-time) |
| `build_http_body_system_prompt_becomes_first_message` | `system: Some(s)` → first `role: "system"` message |
| `build_http_body_no_system_prompt_omits_system_message` | `system: None` → no system message |
| `build_http_body_tools_wrapped_in_function_envelope` | `WireTool` → `[{type: "function", function: {…}}]` |
| `build_http_body_tool_results_become_role_tool_messages` | `WireMessage::Tool` → `role: "tool"` with `tool_call_id` + `content` |
| `build_http_body_assistant_message_carries_text_and_tool_calls` | `WireMessage::Assistant` → `{role: "assistant", content, tool_calls[]}` |
| `build_http_body_omits_tools_field_when_empty` | No `tools: []` (absent) |
| `build_http_body_sets_model_and_max_tokens_from_config` | `model` + `max_tokens` come from `OpenAIConfig` |
| `openai_strip_drops_thinking_signature_from_anthropic_history` | Cross-protocol strip integration with wire layer |
| `openai_401_classified_as_auth` | OpenAI `error.code = "invalid_api_key"` → `LlmError::Auth` |
| `openai_429_classified_as_rate_limit` | OpenAI `error.code = "rate_limit_exceeded"` → `LlmError::RateLimit` |
| `openai_400_with_invalid_request_code_is_invalid` | OpenAI `error.code = "invalid_request_error"` → `LlmError::InvalidRequest` |
| `openai_500_classified_as_server` | OpenAI5xx → `LlmError::Server` |
| `build_tool_call_event_parses_accumulated_arguments_json` | Tool-call buffer with complete JSON → `ChatEvent::ToolCall` with parsed `input` |
| `build_tool_call_event_handles_partial_arguments` | Concatenated fragments → valid JSON parsed at emit time |
| `build_tool_call_event_returns_none_without_name` | Defensive: missing `function.name` → drop |
| `build_tool_call_event_empty_args_buf_yields_empty_object` | Defensive: no arguments → `{}` |
| `wire_block_to_chat_event_text_path` | (sanity, same as wire test) |
| `wire_block_to_chat_event_reasoning_path` | (sanity, same as wire test) |

#### Factory / dispatch (`provider::tests` —1 changed,1 new)

- `build_provider_openai_returns_openai_provider` (CHANGED from
 `build_provider_openai_returns_not_implemented`): PR2's
 stub is now a real dispatch. Same test name with new
 assertion shape.

PR3 net new tests:16 wire +14 OpenAI =**30+ new tests**.
Pre-PR3 baseline:218. Post-PR3 target:**248+ tests** (the
implementation may land a few more in the
`AnthropicProvider::send` integration path).

#### Frontend

- `pnpm build` (vue-tsc strict) must pass. PR3 does NOT
 change the frontend; `get_llm_config` IPC's wire shape
 (`{model, baseUrl, configured}`) is preserved. The
 catalog's existing display still works (no
 protocol-aware UI yet — that's PR4).
- Manual smoke test (acceptance A2 from the parent PRD):
1. `cd app && pnpm tauri dev`
2. Open Settings, see the2 seeded providers +4
 seeded models (2 Anthropic,2 OpenAI per the PR1
 seed). Default is `claude-sonnet-4-5`.
3. Open a session, type a question, click Send.
4. Observe the LLM responds — same wire behavior as
 pre-PR3 (Anthropic path).
5. In Settings, switch the default to `gpt-4o` (or
 any OpenAI model with a valid key).
6. Open a NEW session (the previous session's history
 is still in the DB; new sessions auto-pick the new
 default). Type a question, click Send.
7. Observe the LLM responds via OpenAI Chat
 Completions. The cross-protocol strip in
 `OpenAIProvider::send` silently drops any
 `Signature` blocks from the prior Anthropic turns
 — the wire payload omits them; the DB still has
 them.

###7. Wrong vs Correct

#### Wrong: openai branch of `build_provider` returns a stub

```rust
// BAD — pre-PR3 stub
"openai" => Err(ProviderBuildError::NotImplemented("openai")),
```

User picks a gpt-4o model in Settings; chat command's
pre-flight returns `PreFlightError::BuildFailed(NotImplemented)`
which renders as "无法构造 LLM provider: provider protocol
'openai' is not implemented yet". The user has no way to
actually use the model they configured.

#### Correct: openai branch constructs an `OpenAIProvider`

```rust
// GOOD — PR3 dispatch
"openai" => {
 let max_tokens = model_row.max_tokens.unwrap_or(16384);
 let reasoning_effort = model_row.thinking_effort.clone();
 let config = openai::OpenAIConfig {
 base_url: provider_row.base_url.clone(),
 model: model_row.model_name.clone(),
 api_key: provider_row.api_key.clone(),
 max_tokens,
 reasoning_effort,
 };
 Ok(Box::new(OpenAIProvider::new(config)))
}
```

The `reasoning_effort` value is plumbed from
`ModelRow.thinking_effort` so o1/o3 users get the
correct effort level on every request.

#### Wrong: strip on the wrong layer

```rust
// BAD — strip on Anthropic-shaped ContentBlock
fn strip_for_openai(blocks: &mut Vec<ContentBlock>) {
 blocks.retain(|b| !matches!(b, ContentBlock::Thinking { .. }));
 // ...
}
```

This couples the strip logic to the Anthropic-shaped
types. Future protocols (Gemini, Ollama) would each need
their own strip function. Cross-protocol history
(Signature / RedactedThinking) cannot be expressed.

#### Correct: strip on the wire layer

```rust
// GOOD — provider-agnostic
fn strip_unsupported(messages: Vec<WireMessage>, caps: &WireCapabilities) -> Vec<WireMessage> {
 messages.into_iter()
 .filter_map(|m| match m {
 WireMessage::User { content } => Some(WireMessage::User { content }),
 // ... etc
 WireMessage::Assistant { blocks } => {
 let filtered: Vec<WireBlock> = blocks.into_iter()
 .filter(|b| block_supported(b, caps))
 .collect();
 Some(WireMessage::Assistant { blocks: filtered })
 }
 })
 .collect()
}
```

Single function. Driven by `WireCapabilities`. Both
providers call it. Future protocols plug in by writing
their own provider-wire converter and the strip pass
auto-adapts.

#### Wrong: `classify_error_response` only matches `error.type`

```rust
// BAD — pre-PR3 Anthropic-only
if keyword.contains("authentication") || keyword.contains("new_api_error") {
 Auth
} else if keyword.contains("rate_limit") {
 RateLimit
} // ...
```

OpenAI401 returns
`{"error": {"code": "invalid_api_key", "message": "..."}}`.
`error.type` is a literal `"error"`. The keyword match
fails. Status is401 (4xx) → falls through to
`InvalidRequest`. The user sees "请求无效: Incorrect API
key provided" instead of "API key 无效或已过期...".

#### Correct: read both `error.type` and `error.code`

```rust
// GOOD — PR3 extended classifier
let err_type = parsed.error.as_ref().and_then(|e| e.r#type.clone());
let err_code = parsed.error.as_ref().and_then(|e| e.code.clone());
// Pick the field that contains a classification keyword
let mut chosen: Option<String> = None;
for cand in [&err_type, &err_code, &top_type] {
 let s = keyword_in(cand);
 if has_keyword(&s) {
 chosen = Some(s);
 break;
 }
}
```

The first field with a useful keyword wins. If neither
field has one, the function falls back to status code.
Pre-PR3 Anthropic / GLM tests still pass (they use
`error.type` and the keyword match still finds it).

#### Wrong: persist the strip result

```rust
// BAD — strip + persist + lose the original
let stripped = strip_unsupported(messages, &caps);
db::update_messages(session_id, &stripped).await?; // ❌
```

The DB now lacks the thinking / signature blocks.
Switching back to a thinking-capable model: the LLM has
no memory of its prior reasoning. Recovery requires
re-running the model on the original turn (lossy and
expensive).

#### Correct: strip in-memory only

```rust
// GOOD — wire-only strip
let wire = chat_request_to_wire(req, system);
let wire = WireRequest {
 messages: strip_unsupported(wire.messages, &caps),
 ..wire
};
// DB write path uses the original `req.messages` —
// untouched by strip.
```

The DB is the source of truth for conversation history.
The wire payload is a per-turn projection of the DB
state onto the target protocol's capabilities. Switch
back, the projection includes the blocks again.

### Design Decisions

#### Decision: Anthropic also goes through the wire layer (D1, locked2026-06-09)

**Context**: The PR3 PRD considered two architectures:
(a) Anthropic stays on its pre-PR3 code path
(verbatim-move to a new `AnthropicProvider`), OpenAI
plugs in via a separate `chat_openai_stream_with_tools`
function — minimal disruption to PR2's tests. (b) Both
providers go through a shared wire layer — symmetric
architecture, but Anthropic's `chat_stream_with_tools`
has to be refactored to consume a `ChatRequest` (the
wire layer's inverse output) rather than the legacy
`system + messages + tools` parameters.

**Decision**: (b). The wire layer's inverse
(`wire_messages_to_chat_messages`) reconstructs the
Anthropic-shaped `ChatRequest` the legacy SSE parser
consumes, so the rest of `chat_stream_with_tools` is
unchanged. The cost is one extra in-memory conversion
per `send` call. The benefit is architectural symmetry
— future protocols (Gemini, Ollama) plug in with no
refactor to the existing providers.

**Consequences**:
- ✅ Cross-protocol consistency: both providers go
 through the same `chat_request_to_wire → strip →
 provider-wire-converter` flow.
- ✅ The strip pass is exercised on the Anthropic
 path too, catching bugs early.
- ⚠️ Slight code overhead: the Anthropic `send`
 method is ~20 lines longer.
- ⚠️ The4 PR2 `anthropic::tests::*` tests still pass
 verbatim because the wire round-trip preserves the
 same field set on the Anthropic→Anthropic path.

#### Decision: In-memory strip, no persistence (D2, locked2026-06-09)

**Context**: The parent PRD §Q5 H1 decided on "silent
degradation" when switching models. PR3 implements this
by stripping blocks the new model can't carry from the
wire payload. The question is whether the strip result
is persisted.

**Decision**: Strip in-memory only. The DB stores the
full turn (text + thinking + signature + redacted).
The wire payload is a per-turn projection.

**Consequences**:
- ✅ Switching back to a thinking-capable model
 restores the thinking display.
- ✅ No DB schema change.
- ✅ No migration risk on existing sessions.
- ⚠️ A cross-protocol session always has a "stripped"
 view on the wire for non-thinking targets — the
 model's prior reasoning is invisible to it. This is
 the documented trade-off (parent PRD §Q5).

### Future Work (Deferred from PR3)

| Item | Why deferred |
|------|-------------|
| `ProviderCapabilities`-gated dispatch | All3 protocols (Anthropic, OpenAI, future Gemini) support system + tools + streaming. Capability gating is a no-op until a protocol diverges. |
| Provider-level API key redaction (so `api_key` is never logged even in `tracing::debug!`) | The `info!` logs in `chat_stream_with_tools` and `OpenAIProvider::send` don't include the key today; explicit redaction is a defensive layer PR4+ should add. |
| OpenAI `max_completion_tokens` (o1+ specific) | The OpenAI API uses `max_tokens` for non-o1 models and `max_completion_tokens` for o1+. PR3 uses `max_tokens`; future PR may branch on `model_name` to pick the right field. |
| OpenAI `parallel_tool_calls: true` | PR3 emits multiple `tool_calls` from a single assistant turn (matches the OpenAI streaming semantics), but the request body doesn't set `parallel_tool_calls: true` explicitly. The default is `true` on most models, so this is a no-op today. |
| Gemini / Ollama adapters | Each will plug in via the wire layer — write a new `provider::gemini` module, implement `Provider`, register in `build_provider`. The wire layer is the cross-protocol contract that doesn't need to change. |
