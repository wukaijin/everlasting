# Error Handling

> How errors are handled in this project.

---

## Overview

<!--
Document your project's error handling conventions here.

Questions to answer:
- What error types do you define?
- How are errors propagated?
- How are errors logged?
- How are errors returned to clients?
-->

(To be filled by the team)

---

## Error Types

<!-- Custom error classes/types -->

(To be filled by the team)

---

## Error Handling Patterns

<!-- Try-catch patterns, error propagation -->

(To be filled by the team)

---

## API Error Responses

<!-- Standard error response format -->

The agent classifies LLM-side failures into `LlmErrorKind` (`llm/error.rs`):

- `Auth` — bad or missing API key, 401/403 from upstream.
- `RateLimit` — 429.
- `Server` — 5xx.
- `Network` — connection / timeout.
- `Protocol` — 4xx other than 401/403/429, including 400 from a malformed request
  body. **This is the bucket that catches the thinking-related failures below.**

### Anthropic 400 from extended-thinking contract violations

The `Protocol` kind covers failures caused by our own payload. Three patterns
all surface as a 400 with a message like `"messages.0.content.0.signature: Field required"`:

| Cause | Fix |
|-------|-----|
| Thinking block omitted from history on round-trip | The block is mandatory on the next turn after a thinking turn. Rehydrate must include all thinking blocks with their full `signature`. |
| `signature` lost, truncated, or mutated | Store verbatim; emit verbatim. The signature is opaque. |
| Thinking block positioned after `tool_use` (or anywhere other than the head of the assistant message) | `toPayloadContent` must put thinking blocks first. |

See `backend/llm-contract.md` §4 Validation & Error Matrix for the full list.

---

## Common Mistakes

<!-- Error handling mistakes your team has made -->

### Mistake: dropping the `signature` to "save space"

The `signature` on a `ContentBlock::Thinking` is a cryptographic anchor for
Anthropic. Drop it and the next turn 400s. The DB stores it in full; the
rehydrate path emits it in full. There is no compression, no truncation, no
"redact for privacy" — the field is opaque and the only safe behavior is
verbatim round-trip.

### Mistake: emitting `signature_delta` per SSE event

`signature_delta` is buffered in `BlockState::Thinking { signature_buf }` and
emitted as a single `ChatEvent::SignatureDelta` on `content_block_stop`.
Per-event emit was the step 6 v1 implementation; the check phase caught it
because Anthropic might split the signature across N events in a future
schema, and a per-event emit would scatter chunks across N thinking blocks.
See `backend/llm-contract.md` §7 Wrong vs Correct.
