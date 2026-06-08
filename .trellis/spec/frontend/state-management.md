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

---

## Stream Controller Pattern (added 2026-06-07, PR 06-07-6-ui-bug-markdown-sse)

For anything related to **in-flight SSE streams from the Rust agent loop**, the
single source of truth is `useStreamControllerStore()` in
`app/src/stores/streamController.ts` — NOT `useChatStore()`. The chat store
is a thin facade that projects controller state for the UI to read.

### Why a separate store

The old design put messages, `streamingSessionId`, `currentRequestId`, and
the SSE listener all inside `useChatStore()`. That broke the moment a user
switched sessions mid-stream: the listener filtered events by
`currentSessionId`, so `done` events for the now-non-current stream were
dropped, leaving the red dot, the "stop" button, and `sending` all stuck.
The streaming message itself was also lost when `switchSession` rehydrated
the new session's messages and overwrote `messages.value`.

### The split

| Concern | Owns | API |
|---|---|---|
| Per-session message buffer | **streamController** | `messagesBySession: Map<sessionId, ChatMessage[]>` (LRU 20) |
| Active in-flight requests | **streamController** | `activeRequests: Map<requestId, RequestState>` |
| SSE listener registration | **streamController** (singleton) | `start()` / `stop()` in `App.vue` lifecycle |
| `streamingSessionIds` / `streamingProjectIds` | **streamController** | `ComputedRef<Set<string>>` — UI subscribes |
| Sessions list, currentSessionId, currentCwd | **chatStore** (UI state) | `sessions`, `currentSessionId`, `currentCwd`, `simplifiedCwd` |
| Session CRUD (`createNewSession`, `switchSession`, `deleteSession`) | **chatStore** (delegates to controller) | wires UI → controller's `ensureLoaded` / `evict` |
| Wire-format history construction (`toPayloadContent`) | **chatStore** (the only place that needs `ChatMessage` for outbound) | passed to `controller.startRequest` |

### Rules of thumb for new code

- **Never** register an SSE listener outside `streamController.start()`. One
  global listener routes by `request_id` (not by current session) — that's
  the whole point.
- **Never** mutate `messagesBySession` directly from a component. Use
  `chatStore.send()` (which calls `controller.startRequest` and pushes
  user/assistant placeholders into the correct session's array) or
  `controller.ensureLoaded(id)` for the DB-backed load path.
- **Pin streaming sessions in the LRU.** `controller.startRequest` calls
  `pinnedSessions.add(sessionId)`; `finalizeRequest` removes it. Don't
  hand-evict a session whose `activeRequests` map has an entry for it.
- **`isCurrentSessionStreaming` is per-session.** Use it for the chat
  input's stop button. Use `streamController.streamingSessionIds` directly
  for the sidebar (PR4) so non-current sessions can show their own
  streaming indicator.

### Reactive bridge caveat

`streamController.streamingSessionIds` is a `ComputedRef<Set<string>>`.
Pinia auto-unwraps refs on store-proxy access, so components read it as a
plain `Set<string>` (no `.value`). The `Set` itself is recomputed on every
`activeRequests` mutation; the `v-if` binding on the session card flips
automatically. No manual triggers needed.

### Worktree transition invalidation (added 2026-06-08, step 4 follow-up)

After any worktree state change (`attachWorktree` / `detachWorktree` /
`deleteWorktree`), the chat store calls `controller.refresh(sessionId)` to
evict the cached messages and reload from DB. This is the only way the LLM's
next `send()` payload can include the freshly-injected `[worktree event]`
system event.

```typescript
// app/src/stores/chat.ts (excerpt)
async function attachWorktree(sessionId: string) {
  // ... Tauri invoke ...
  controller.refresh(sessionId);  // ← mandatory, do not skip
}
```

`controller.refresh` is a thin wrapper around `ensureLoaded` that first
evicts the LRU entry and then re-loads from DB. Without it:

- The cached `messagesBySession.get(sessionId)` is stale.
- The next `send()` builds `toPayloadContent` from the cache.
- The LLM's payload omits the system event, and the model reasons on
  the old worktree state.

Backend stores the system event in the same `messages` table; the
frontend does not need a special branch in the wire event handler.
The system event renders as a regular user-role message with the
`[worktree event]` prefix.

### Send completion invalidation (added 2026-06-08, step 4 follow-up — 2013 wire invariant)

Every `send()` must end with `controller.finalizeRequest` clearing the
in-memory `messagesBySession` entry **and** the chat store's
`diffCache` entry for the same session. The cleanest place to do
both is the controller's own `finalizeRequest` (the function the
`done` / `error` / catch-error paths all route through) — pairing
`evict(sessionId)` with `useChatStore().invalidateDiff(sessionId)`
in one call.

```typescript
// app/src/stores/streamController.ts (excerpt)
function finalizeRequest(requestId: string, sessionId: string, _errored: boolean): void {
  activeRequests.delete(requestId);
  pinnedSessions.delete(sessionId);
  evict(sessionId);
  useChatStore().invalidateDiff(sessionId);  // ← paired, mandatory
}
```

Why both:

- **In-memory `messagesBySession`** is the *streaming-accumulation*
  shape — a single `assistantMsg` placeholder that absorbed every
  `delta` / `tool_call` / `tool_result` / `thinking_delta` event
  across all turns of the `chat` invocation. The DB stores one
  assistant message per agent-loop turn (per turn the Rust side
  persists in `lib.rs:chat`). If we leave the cache after a
  successful send, the next `ensureLoaded` for that session takes
  the in-memory fast path and the wire-format history sent to the
  LLM has an assistant turn whose `tool_use` block is followed by
  a user-text message (the next typed prompt) with no `tool_result`
  in between — Anthropic Messages API returns 2013 ("tool call
  result does not follow tool call"). Evicting forces the next
  `ensureLoaded` to re-read from DB, where the per-turn split
  shape puts the `tool_result` in the correct following
  user-role message.
- **Chat store `diffCache`** holds the worktree diff result so the
  chip's "diff (N)" counter doesn't re-fetch on every render. A
  `git commit` run inside the worktree during the just-finished
  send should drop the counter immediately; without invalidation
  the chip keeps showing the pre-send snapshot until the next
  `attachWorktree` / `detachWorktree` / `deleteWorktree` happens to
  trigger an `invalidateDiff` (coincidentally — that's why the
  earlier 2026-06-07 step-4 follow-up worked for the first send
  but regressed on the second).

A failure mode that this invariant prevents: a refactor that splits
`finalizeRequest` into "evict" and "invalidate" helpers but only
calls one of them would silently break one of the two bugs above.
The two actions are paired, not independent — the
`streamController.test.ts` `finalizeRequest` describe block has a
`both actions fire on the same finalizeRequest call (paired
invariant)` test that locks this.
