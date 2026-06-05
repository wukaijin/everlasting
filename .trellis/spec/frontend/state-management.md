# State Management

> How state is managed in this project.

---

## Overview

<!--
Document your project's state management conventions here.

Questions to answer:
- What state management solution do you use?
- How is local vs global state decided?
- How do you handle server state?
- What are the patterns for derived state?
-->

The chat store is a Pinia store (`app/src/stores/chat.ts`) backed by Tauri IPC
events from the Rust agent core. The state model is: **the wire-format blocks
the agent sends are the source of truth for what's persisted, but the
in-memory representation is denormalized for rendering and event handling.**

---

## State Categories

<!-- Local state, global state, server state, URL state -->

`ChatMessage` carries three categories of state:

1. **Visible fields** — `role`, `text: string[]`, `toolUses`, `toolResults`,
   `isStreaming`. These render directly into the bubble.
2. **Thinking fields** — `thinkingBlocks: ThinkingBlockInfo[]` and
   `redactedThinkingData: string[]`. In-memory only. The visible UI is a
   `<details>` element that defaults to collapsed; on session reload these
   fields are rehydrated from DB and rendered the same way.
3. **Collapse / scroll state** — in-memory only, intentionally NOT persisted.
   Reload resets; the spec is to behave like Claude.ai / Claude Code.

---

## When to Use Global State

<!-- Criteria for promoting state to global -->

Any field that survives a session reload goes in the DB. The DB schema is
documented in `app/src-tauri/src/db.rs`; the round-trip contract is
`MessageContent::Blocks` (a `Vec<ContentBlock>`).

Three persistence rules that came out of step 6:

- **Thinking text + signature go in the DB.** `ContentBlock::Thinking` and
  `ContentBlock::RedactedThinking` are first-class block types and survive
  rehydrate losslessly.
- **The denormalized `text` column does NOT contain thinking text.**
  `MessageContent::to_text()` (Rust) and the corresponding rehydrate logic
  (TS) keep the two streams separate; the bubble shows only the actual reply.
- **Thinking blocks come first in the outbound payload.**
  `toPayloadContent` emits `thinking` / `redacted_thinking` blocks at the
  head of the assistant message — Anthropic requires this order on
  round-trip; emitting them anywhere else → 400. See
  `backend/llm-contract.md` §4 for the full constraint list.

---

## Server State

<!-- How server data is cached and synchronized -->

Event handling contract (from step 6):

| Wire event (`kind`) | Handler | Effect on `ChatMessage` |
|---------------------|---------|-------------------------|
| `"thinking_delta"` | append to `currentThinkingBlock(m).thinking` | UI `<details>` re-renders with new text. |
| `"signature_delta"` | set `currentThinkingBlock(m).signature` | Block is now "closed" — the next `text_delta` or `tool_call` flushes it. |
| `"redacted_thinking_delta"` | append to `m.redactedThinkingData` | UI shows "🔒 N redacted" placeholder. |

The `currentThinkingBlock(m)` helper finds or creates the last open
`thinkingBlocks[i]` so the frontend can handle multiple interleaved thinking
blocks without scattering chunks. It exists because the wire format
`thinking_delta → signature_delta → thinking_delta → signature_delta` (rare
but possible) is the canonical "two thinking blocks in one turn" pattern.

---

## Common Mistakes

<!-- State management mistakes your team has made -->

### Mistake: appending thinking text to the bubble

Thinking is rendered in a separate `<details>` block ABOVE the bubble. Do not
inline it; do not append to `m.text`. `MessageContent::to_text()` (Rust) and
the TS rehydrate path both keep these streams separate.

### Mistake: dropping the signature on rehydrate

`ContentBlock::Thinking { thinking, signature }` → both fields must land in
`m.thinkingBlocks[i]`. If `signature` is empty after rehydrate, the next turn
will 400. The check phase of step 6 added a unit test for the round-trip; any
future change to the DB schema must keep this invariant.

### Mistake: emitting thinking blocks after tool_use in `toPayloadContent`

The outbound payload order is: `thinking → redacted_thinking → text → tool_use → tool_result`.
Anthropic validates this order strictly. See
`backend/llm-contract.md` §4 / §7 for the constraint and a Wrong/Correct pair.
