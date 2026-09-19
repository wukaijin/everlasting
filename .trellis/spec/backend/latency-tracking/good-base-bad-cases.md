<!-- Moved from latency-tracking.md 2026-09-19 (doc-split) -->

### 5. Good / Base / Bad Cases

#### Good: Anthropic happy path

1. User sends "explain closure"; controller records
   `sendAt = T0`.
2. Backend streams:
   - `start` event
   - `delta` × N: controller records `firstDeltaAt = T0+0.4s`
     on the first one.
   - `done` with `stop_reason: "end_turn"`, no usage.
3. Controller's `done` handler:
   - `doneAt = T0+3.2s`
   - `ttfbMs = 400`, `genMs = 2800`, `totalMs = 3200`
   - In-memory: `last.latency = { ttfbMs: 400, genMs: 2800, totalMs: 3200 }`
   - Cumulative: `sessionTotalLatencyMs.get(sid) += 3200`
   - Stash: `req.latencyPending = { ttfbMs: 400, genMs: 2800, totalMs: 3200 }`
4. `reloadAfterFinalize` fires (synchronous cleanup
   happened already):
   - `load_session` returns the assistant row with `seq = 3`.
   - IPC: `update_message_latency({ sessionId, seq: 3, ttfbMs: 400, genMs: 2800, totalMs: 3200 })`.
5. UI: assistant bubble shows "3.2s" at the bottom-right;
   hover tooltip shows three lines.

#### Good: OpenAI same flow

The timing is protocol-agnostic — Anthropic and OpenAI
both emit a `delta` event on the first content byte, and
both emit a `done` (or equivalent) at the end. The TTFB
/ gen / total math is identical. The cross-protocol
strip doesn't affect the chat-event channel.

#### Good: per-tool duration

1. User asks the LLM to read a file; the LLM emits
   `tool_use` with `id: "toolu_abc"`.
2. Controller's `tool:call` handler:
   `req.toolStartedAt.set("toolu_abc", T0)`.
3. Backend executes the tool and emits `tool:result` at
   `T0+0.35s`.
4. Controller's `tool:result` handler:
   - `durationMs = 350`
   - In-memory: `last.toolResults.push({ ..., durationMs: 350 })`
   - IPC: `record_tool_duration({ sessionId, toolUseId: "toolu_abc", durationMs: 350 })`.
5. The backend patches the `tool_result` block in
   `messages.content` JSON.
6. UI: ToolCallCard shows "0.4s" next to the status text.

#### Base: cancel during TTFB

1. User sends "explain closure"; controller records
   `sendAt = T0`.
2. The backend hasn't started streaming yet (slow proxy).
3. User hits Stop; the agent loop bails out, persists
   the partial turn (with `usage: None`), emits `done`
   with `stop_reason: "cancelled"`.
4. Controller's `done` handler:
   - `firstDeltaAt = null` (no `delta` arrived)
   - `ttfbMs = null`, `genMs = null`, `totalMs = doneAt - sendAt` (e.g. 8000)
   - In-memory: `last.latency = { totalMs: 8000 }`
   - Cumulative: adds 8000.
   - IPC: `update_message_latency({ ..., ttfbMs: null, genMs: null, totalMs: 8000 })`.
5. UI: assistant bubble shows "8.0s" at the bottom-right;
   hover tooltip shows only the "端到端: 8.0s" line (the
   TTFB / 生成 rows are hidden because the values are
   `null`).

#### Base: pre-F5 session on first load

1. User has a session from before the F5 migration; the
   three columns are all `NULL` on the assistant rows.
2. `load_session` returns the messages with all three
   latency fields `null`.
3. `rehydrateMessages`: the `hasLatency` check fails for
   every row → no `latency` is attached to any
   `ChatMessage`.
4. `ensureLoaded`: no assistant row has
   `latency.totalMs` set → no `accumulateLatency` seed.
5. UI: assistant bubbles show "—" (no chip); the
   ChatPanel footer shows "—" with the
   "升级前未统计" tooltip.

#### Bad: stripping timing on cross-protocol switch

1. User has an active session on `claude-sonnet-4-5`;
   assistant turns have `ttfb_ms` / `gen_ms` / `total_ms`
   in the DB.
2. User switches the default model to `gpt-4o` and
   sends a new message.
3. (Anti-pattern) The new LLM call's history is filtered
   to drop the timing columns; the in-memory
   `ChatMessage` loses its `latency`.
4. UI: the assistant bubble no longer shows the
   "3.2s" chip, even though the data is in the DB.
5. **Fix (F5 doesn't do this)**: F5 timing is purely a
   display concern; the timing columns live on the
   message row, not in the wire payload. They survive
   model switches naturally. The wire layer's cross-
   protocol strip is for `tool_use` / `thinking` blocks
   — `ttfb_ms` / `gen_ms` / `total_ms` are per-message
   metadata, not blocks, so they're untouched.

#### Bad: per-tool duration lost on assistant row only

1. The agent loop emits `tool_use`, the tool runs, the
   `tool:result` event fires, the controller patches the
   `tool_result` block in memory and fires the
   `record_tool_duration` IPC.
2. (Pre-fix anti-pattern) The IPC was hard-coded to look
   only at user-role `tool_result` blocks (because the
   rehydrate merge step moves them onto the assistant
   message AFTER the IPC fires).
3. Result: the timing patch lands on a user-role row's
   `content` JSON, but the UI reads from the assistant
   message's `toolResults` (the merged view), which has
   no `durationMs` because the patch went to the wrong
   row.
4. **Fix (F5's `record_tool_duration` walks ALL rows)**:
   the function searches every `tool_result` block in
   the session (user-role and assistant-role rows) for
   the matching `tool_use_id`. Whichever row holds the
   block gets the patch. The 2013 orphan-repair row is
   also covered (its `tool_result` blocks are valid
   candidates for the patch).

