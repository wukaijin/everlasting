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

## Agent Loop Error Paths — terminal event + persist invariants

The agent loop's `run_chat_loop` (see `backend/agent-loop-architecture.md`)
has three terminal paths that exit the per-turn stream loop early:
**normal Done**, **cancel**, and **error**. Each has its own
terminal-event + persist contract.

### Path: `ChatEvent::Error` mid-turn (RULE-A-007, 2026-06-17)

When the LLM stream emits `ChatEvent::Error`, the per-event arm:

1. Emits the `Error` to the frontend immediately (this is the
   terminal signal — the controller treats it as end-of-stream).
2. Sets `had_error = true` and breaks out of the stream loop.

After the stream loop, the agent loop **persists the partial turn**
symmetric with the cancel path (RULE-A-007 fix; previously the error
arm did `if had_error { return; }` and dropped all accumulated
content):

1. Flushes pending thinking into `finalized_thinking`.
2. Builds assistant blocks (`thinking` + `text` + `tool_use` +
   `redacted_thinking`).
3. Appends `ERROR_MARKER` (`"[生成出错中断]"`) to the text —
   symmetric to the cancel path's `CANCELLED_MARKER`. Empty-text
   edge case: marker alone.
4. `persist_turn` the partial row.
5. Emits `ChatEvent::TurnComplete { seq, ...latency }` so the
   frontend has the partial row's seq + latency (RULE-A-007
   decision C). This **coexists** with the pre-emit `Error` event
   — they carry disjoint information and the controller routes
   each independently.
6. Persists cwd + touches the session, then returns. The error
   path does NOT emit a follow-up `Done` event (the pre-emit
   `Error` is the terminal; emitting `Done` would conflict).

### Persist failure on the error path is log-only (RULE-A-007 decision B)

RULE-A-003 (2026-06-15) made **normal-path** persist failures
emit a typed `ChatEvent::Error{Server}` + abort (so disk-full /
DB-lock contention doesn't silently swallow the user message).
The error path is **different**: the per-event arm already emitted
the terminal `Error`. Calling `emit_persist_failure` on top would
produce two terminal events (Error + Error) and the frontend's
terminal handling would fire twice.

The error path therefore follows the **same log-only pattern** the
cancel path uses for its synthetic tool_result persist (cancel's
terminal `Done{cancelled}` is about to fire, so an Error there
would also be a double-terminal). The "exactly one terminal event
per request" invariant stays intact.

| Persist site | Failure handling | Why |
|---|---|---|
| Initial user message (normal path) | `emit_persist_failure` + return | First persist; no terminal yet — Error becomes the terminal (RULE-A-003) |
| Assistant turn (normal Done path) | `emit_persist_failure` + return | Mid-request; no terminal yet — Error becomes the terminal (RULE-A-003) |
| Tool_result turn (normal path) | `emit_persist_failure` + return | Mid-request; no terminal yet — Error becomes the terminal (RULE-A-003) |
| Cancel's synthetic tool_result persist | `tracing::error!` log-only | Cancel's terminal `Done{cancelled}` is about to fire — double-terminal hazard |
| Cancel's cancelled tool_result persist | `tracing::error!` log-only | Same as above |
| **Error path's assistant partial persist** | **`tracing::error!` log-only** | **Per-event arm already emitted terminal `Error` — double-terminal hazard (RULE-A-007 decision B)** |

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
