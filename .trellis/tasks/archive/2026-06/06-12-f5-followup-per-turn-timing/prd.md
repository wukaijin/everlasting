# F5 follow-up: per-turn latency + thinking timing for multi-turn agent responses

## Background

F5 (LLM Latency Tracking, 2026-06-11) shipped with per-message latency + per-message
thinking duration, both persisted to the `messages` table (`ttfb_ms` / `gen_ms` /
`total_ms` / `thinking_ms`). The implementation tracks timing on the
`RequestState` (one per in-flight request) and fires `update_message_latency`
once at the end of the request, targeting the LAST assistant message.

For single-turn responses (no tool_use), the deferred `Done` event arrives after
the only assistant message is persisted, so the per-message columns get
written correctly.

For multi-turn responses (the agent loop calls the LLM multiple times because
the model emitted `tool_use` blocks in earlier turns), the agent loop persists
N assistant message rows — one per LLM call. The `update_message_latency` IPC
fires only for the last one (largest `seq`), so the first N-1 rows' columns
stay NULL. On rehydrate, the rehydrated messages carry
`thinkingDurationMs = undefined` and the ThinkingBlock header falls back to
`—`. The last message ends up with the FIRST thinking phase's duration (not
the last) because `RequestState.thinkingDurationMs` is closed on the first
non-thinking boundary and never re-opened.

## Symptom (verified via screenshot 2026-06-12)

User asked "node 版本是多少现在" on a Node repo. The model went through 3
turns (3 separate LLM calls) to answer: each turn did a `shell` tool_use to
inspect the project, then a final turn produced the text response.

Observed:

- 3 ThinkingBlocks rendered in the same response, one per persisted
  assistant message
- First 2 ThinkingBlocks: header `Thought for —`
- 3rd ThinkingBlock: header `Thought for 0.7s` (which is the FIRST thinking
  phase's duration, NOT the last)

Side effects:

- `MessageItem` per-message latency chip (footer): also `—` for the first 2
  messages
- `ChatInput` popover session-cumulative (`累计` / `轮次` / `平均`): correct
  (these accumulate across all turns via `accumulateLatency` / the
  per-session `Map<sessionId, number>`)

## Root cause

Two layers, both per-request:

1. **Agent loop is per-request, not per-turn** (`app/src-tauri/src/agent/chat.rs`).
   The inner LLM-stream loop processes each `ChatEvent::Done` (line 481) by
   setting `stop_reason` and breaking out of the inner loop — but does NOT
   emit the event to the frontend. The deferred `Done` is emitted ONLY at
   the very end of the agent loop (line 670, after `persist_turn` +
   `touch_session`), gated on `should_continue == false`. So a 3-turn
   request sees 1 `done` event in the frontend, not 3.

2. **Frontend `RequestState` is per-request, not per-turn**
   (`app/src/stores/streamController.ts:111`). `latencyPending` /
   `thinkingDurationMs` / `firstDeltaAt` are set once and never reset
   between turns. The first `delta` (text) or `tool:call` boundary closes
   the timer (lines 661, 856); subsequent phases find the duration already
   set (`=== null` check fails) and skip.

3. **`reloadAfterFinalize` re-attach writes to the largest-`seq` assistant
   only** (`app/src/stores/streamController.ts:1006-1013`). The seq-lookup
   iterates all assistant rows and picks the largest — i.e. the last one.
   The IPC payload carries the same `thinkingMs` / `ttfbMs` / `genMs` /
   `totalMs` for that one row.

## Fix scope (proposed)

| File | Change |
|---|---|
| `app/src-tauri/src/agent/chat.rs` | After each `persist_turn` (lines 600, 748), emit a per-turn `Done` event (or a new `TurnComplete` variant of `ChatEvent`) carrying the turn's `seq` and the turn-local `thinkingMs` / `ttfbMs` / `genMs` / `totalMs`. The deferred final `Done` stays for the cumulative total. |
| `app/src-tauri/src/llm/types.rs` | Either add fields to the existing `ChatEvent::Done` variant or add a new `ChatEvent::TurnComplete { seq, ttfb_ms, gen_ms, total_ms, thinking_ms }` variant. |
| `app/src-tauri/src/commands/sessions.rs` | If a new variant, route it in `emit_chat_event` (it just passes through). The existing `update_message_latency` IPC already takes `seq` as a parameter — no signature change. |
| `app/src/stores/streamController.ts` | Change `RequestState` from a single `latencyPending: { ttfbMs, genMs, totalMs } \| null` and `thinkingDurationMs: number \| null` to a `Map<seq, { ttfbMs, genMs, totalMs, thinkingMs }>`. The `done` handler writes by-`seq`. `handleToolCall` / `delta` close boundaries also key by `seq` (need a way to know which assistant row is currently receiving events — derive from the latest `delta`'s target message's seq, or add `seq` to each `ChatEvent` payload). |
| `app/src/stores/streamController.ts` (reloadAfterFinalize) | Iterate the `Map<seq, ...>` from `RequestState`, fire `update_message_latency` once per seq. Keep the in-memory re-attach (one write per seq on the rehydrated target). |
| `app/src/stores/streamController.test.ts` | New test: drive 3 turn boundaries (thinking → tool_call → tool_result → thinking → tool_call → tool_result → thinking → text) through `handleChatEvent` + `handleToolCall`, assert all 3 assistant messages end up with non-null `thinkingDurationMs` matching the expected per-phase durations. |
| `.trellis/spec/backend/llm-contract.md` | Update "Scenario: Latency Tracking" § to document per-turn timing; remove or downgrade the "Known Limitations" section once the fix lands. |
| `docs/IMPLEMENTATION.md` | Add an ADR-lite entry for the per-turn decision. |

## Estimation

- ~30-50 LOC across 4 files (Rust 1 + Vue 3-4)
- 1-2 new Rust tests (per-turn emit ordering) + 1 new vitest (multi-turn
  rehydrate)
- Migration: none (the columns already exist; just the write pattern changes)
- Backward compat: pre-fix rows still have NULL columns; rehydrate
  unchanged; only future writes use the new pattern

## Out of scope

- Per-message tool_use timing is already correct (lives in
  `messages.content` JSON, written by `record_tool_duration`).
- Token usage per turn is already correct (each `Done` event's `usage`
  payload is passed through; `add_token_usage` is per-session column-additive
  in `db::sessions`).
- The cumulative session stats (chat input popover) are correct as-is and
  don't need changes.

## Acceptance criteria

- A 3-turn agent response (with 2 tool_use) shows 3 ThinkingBlocks, each
  with a non-`—` duration header matching the actual time spent in THAT
  turn's thinking phase
- After page reload, the same 3 ThinkingBlocks still show their per-turn
  durations (DB round-trip works)
- The `MessageItem` per-message latency chip shows the correct per-turn
  `totalMs` for each of the 3 messages
- The `ChatInput` popover session-cumulative stays correct (no regression)
- All 89 existing vitest + 52 cargo db tests still pass; 1 new vitest test
  covers the multi-turn case
