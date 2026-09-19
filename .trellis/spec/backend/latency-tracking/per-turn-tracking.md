<!-- Moved from latency-tracking.md 2026-09-19 (doc-split) -->

### Per-Turn Tracking (F5 follow-up, 2026-06-12)

F5 ships per-`RequestState` (one request = one LLM stream invocation), which works for the single-turn case but silently corrupts multi-turn agent responses: only the LAST assistant row's latency columns get written. This follow-up scopes the per-turn shape explicitly. The "Known Limitations: Per-turn latency only captured for the LAST assistant message" section that documented the bug has been **removed** — the bug is fixed by the changes below.

#### Wire format — new `ChatEvent::TurnComplete` variant

A new `ChatEvent` variant carries per-turn latency, emitted by the agent loop AFTER each `persist_turn` for an assistant row. Payload (mirrors `db::MessageLatency` plus the row's `seq`):

```rust
TurnComplete {
    seq: i64,                          // assistant row seq, written by persist_turn
    ttfb_ms: Option<i64>,              // first_delta_at - turn_send_at
    gen_ms: Option<i64>,               // done_at - first_delta_at
    total_ms: Option<i64>,             // done_at - turn_send_at
    thinking_ms: Option<i64>,          // turn_thinking_done - turn_thinking_start
}
```

`seq` is the per-turn row handle (assigned by the agent loop in `app/src-tauri/src/agent/chat.rs` from the per-session `next_seq` counter). The 4 ms fields are `Option` so a turn that never reached the relevant boundary (e.g. thinking-only turn cut by a `tool:call`) still serializes cleanly. Wire transport: same `chat-event` channel as every other variant (the Tauri `emit("chat-event", payload)` in `app/src-tauri/src/agent/helpers.rs` `emit_chat_event`), discriminator is `"kind": "turn_complete"`.

#### Backend — `persist_turn` writes the 4 columns in one INSERT

`app/src-tauri/src/agent/chat.rs` outer loop tracks 4 per-turn `Option<Instant>` locals per iteration:

- `turn_send_at` — set right before `provider.send(...)`
- `turn_first_delta_at` — set on the first `ChatEvent::Delta` for this turn
- `turn_thinking_start` — set on the first `ChatEvent::ThinkingDelta` for this turn
- `turn_thinking_done` — set on the first non-thinking boundary (text `Delta`, `ToolCall`, `Done`, or `Error`)

`ChatEvent::Start` no longer has the `if turn == 1` guard (`app/src-tauri/src/agent/chat.rs:422-425`) — every turn emits Start so the frontend can key its `latencyByTurn` per turn reliably.

At the `persist_turn` call site (line 600-607) the existing `latency: Option<&MessageLatency>` parameter is filled with `Some(&MessageLatency { ttfb_ms, gen_ms, total_ms, thinking_ms })` derived from the 4 Instants. The INSERT statement in `db::sessions::messages::persist_turn` already binds all 4 columns — F5 added `thinking_ms` on 2026-06-12. Per-turn rows therefore get all 4 columns populated atomically, no follow-up `UPDATE` needed for the common case.

Right after each successful `persist_turn` (assistant row), the loop emits `ChatEvent::TurnComplete { seq, ttfb_ms, gen_ms, total_ms, thinking_ms }`. Cancel-mid-turn and cancel-during-tool-exec paths also fire TurnComplete for whatever assistant row they persisted. The `MAX_TURNS = 20` safety net does NOT fire TurnComplete (it never persists).

The final `ChatEvent::Done` emit (line 660-679, gated on `!should_continue`) is unchanged — it still terminates the stream and carries `stop_reason` + `usage`. Per-turn and stream-terminating events are conceptually distinct; collapsing them would muddy the wire contract.

#### Frontend — `RequestState` keys per turn, not per request

`app/src/stores/streamController.ts` `RequestState` (lines 56-120) drops the per-request single-value fields:

- **Removed**: `latencyPending: { ttfbMs, genMs, totalMs } | null` (line 111)
- **Removed**: per-request single-value `thinkingDurationMs: number | null` (line 96)

…in favor of two new fields:

- `currentTurnIndex: number` — bumped in the `case "start"` arm of `handleChatEvent`'s switch (line 637-640)
- `latencyByTurn: Map<number, TurnLatency>` — keyed by `currentTurnIndex`, where `TurnLatency = { seq, ttfbMs, genMs, totalMs, thinkingMs }` mirrors the Rust `TurnComplete` payload

The 4 close-boundary sites that snapshot `thinkingDurationMs` (text `delta` line 661, `tool:call` line 856, `done` line 715, `error` line 805) keep their single-value `thinkingStartedAt` / `thinkingDurationMs` locals as PER-TURN timer state — they're reset on every `Start` event, not on `startRequest`. The new `case "turn_complete"` arm writes to `latencyByTurn.set(currentTurnIndex, ...)` AND in-place mutates the reactive placeholder's `latency` / `thinkingDurationMs` so `currentSessionLatencyTurns` (in `chat.ts`) updates in real-time per turn, with no reload.

`accumulateLatency(req.sessionId, totalMs)` moves from the `done` handler (line 743) to the `turn_complete` handler — same A4 `accumulateTokenUsage` per-done pattern, just one event earlier and fired N times per request instead of once.

#### Re-attach — fire N `update_message_latency` IPCs per request

`reloadAfterFinalize` (line 974-1113) iterates `req.latencyByTurn` and fires one `update_message_latency` IPC per entry, keyed by `lat.seq` (not by "max seq" of all assistant rows as in the F5 path). The in-place mutate loop is `m.seq === lat.seq` (per-turn) instead of "max seq" (per-request). `cancel` / `error` paths go through the same `reloadAfterFinalize` and naturally fire N IPCs for whatever turns had a `TurnComplete` arrive before the cancel/error.

`update_message_latency` IPC signature is unchanged (F5 + 2026-06-12 already takes `(sessionId, seq, ttfbMs, genMs, totalMs, thinkingMs)`). The 4-column `UPDATE` in `db::sessions::messages::update_message_latency` is also unchanged — it's just called N times instead of once.
