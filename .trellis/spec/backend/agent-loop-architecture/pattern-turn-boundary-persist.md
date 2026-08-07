## Pattern: Turn-boundary persist symmetry — error arm matches cancel arm (RULE-A-007, 2026-06-17)

**Problem**: When the LLM stream emits `ChatEvent::Error` mid-turn,
the agent loop's per-event arm emits the Error to the frontend
(already rendered as a terminal signal) and sets `had_error = true`.
Before RULE-A-007, the post-stream-loop code did `if had_error { return; }`
— bailing out **without** persisting any of the turn's accumulated
`text_parts` / `finalized_thinking` / `tool_calls`. The cancel path,
in contrast, flushed pending thinking, appended `CANCELLED_MARKER` to
the text, and called `persist_turn` so the partial turn survived in
the DB. This asymmetry meant: a user who watched a partial response
render live, then reloaded the session, would find the assistant turn
missing entirely (cancel preserved, error discarded).

**Solution**: The error arm now mirrors the cancel arm. Both paths:

1. Flush pending thinking into `finalized_thinking`.
2. Log an info-level `tracing` line (`cancelled — persisting partial turn`
   vs `errored — persisting partial turn`) so the cause is distinguishable.
3. Build the assistant blocks (`thinking` + `text` + `tool_use` +
   `redacted_thinking`) and append a sentinel marker to the text:
   - Cancel → `CANCELLED_MARKER` (`"[已停止]"`)
   - Error → `ERROR_MARKER` (`"[生成出错中断]"`, RULE-A-007 new constant)
   - Empty-text edge case: marker alone (symmetric branch in each arm).
4. `persist_turn` the partial row.
5. Emit `ChatEvent::TurnComplete { seq, ...latency }` so the frontend
   has the seq + latency breakdown for the partial row.

The two arms differ in **two** places only:

| Concern | Cancel path | Error path |
|---|---|---|
| Persist failure handling | log-only (no emit; the loop is about to emit terminal `Done{cancelled}`) | **log-only** (no emit; the per-event arm already emitted terminal `Error`. A second Error would be a conflicting double-terminal — RULE-A-007 decision B) |
| Terminal signal after persist | `Done { stop_reason: "cancelled", usage: None }` | (none — the pre-emit `Error` is the terminal; no follow-up `Done`) |

### Why error persist failure is log-only (RULE-A-007 decision B)

RULE-A-003 (2026-06-15) made **normal-path** persist failures emit a
typed `ChatEvent::Error{Server}` + abort (otherwise disk-full / DB-lock
contention would silently lose the user message). The error path is
**different**: the per-event arm at `ChatEvent::Error { .. }` already
emits the Error to the frontend before the persist attempt. Emitting
`emit_persist_failure` on top would produce two terminal events
(Error + Error), and the frontend's terminal handling would fire twice.

The cancel path's synthetic tool_result persist already uses log-only
for the same reason (its terminal `Done{cancelled}` is about to fire).
RULE-A-007 makes the error path's assistant-turn persist follow the
same log-only pattern, keeping the "exactly one terminal event per
request" invariant intact.

### Why error path still emits TurnComplete (RULE-A-007 decision C)

`TurnComplete` carries the partial turn's `seq` + latency breakdown.
The frontend uses it to (a) know which row to attach the latency to
and (b) trigger any per-turn UI updates. Without it, the error path's
partial row would be in the DB but the live-streaming UI wouldn't know
its seq until a reload. The pre-emit `Error` event and the
`TurnComplete` event are **not** in conflict — they carry disjoint
information (Error = "something broke"; TurnComplete = "this seq's
partial turn landed + here's the latency"). The controller routes
each event independently.

### Constants

Both markers live in `app/src-tauri/src/agent/helpers.rs` next to
each other:

```rust
pub const CANCELLED_MARKER: &str = "[已停止]";
pub const ERROR_MARKER: &str = "[生成出错中断]";
```

The bracketed-text style survives DOMPurify unchanged, is
locale-friendly, and renders inline in the bubble's markdown. The UI
does not need a special "interrupted" render branch — existing
markdown rendering handles both markers uniformly.

### When to apply this pattern

- Any new terminal path through `run_chat_loop` that has already
  accumulated partial content (text / thinking / tool_use) MUST
  persist the partial turn. The pattern: flush → marker → persist →
  TurnComplete. Bailing out with raw `return` before persist is the
  anti-pattern that RULE-A-007 removed.
- The persist failure handling on a terminal path is log-only
  (NEVER `emit_persist_failure`) — the terminal event was already
  emitted; a second one would conflict.

### When NOT to apply

- A terminal path that has accumulated **zero** content (no
  `text_parts`, no `finalized_thinking`, no `tool_calls`,
  no `redacted_thinking_data`) skips the persist entirely — the
  `if !assistant_blocks.is_empty()` guard handles this. The error
  path's `ErrThenEnd` (no preceding delta) still hits the persist
  branch because the `ERROR_MARKER` alone populates `full_text`.
  This is intentional — the user sees a visible "[生成出错中断]"
  marker explaining what happened, rather than a blank turn.

---
