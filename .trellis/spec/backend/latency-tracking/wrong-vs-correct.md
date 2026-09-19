<!-- Moved from latency-tracking.md 2026-09-19 (doc-split) -->

### 7. Wrong vs Correct

#### Wrong: timing on the backend (Instant::now / SystemTime)

```rust
// BAD — Rust side timing. Requires plumbing the
// SystemTime through every layer (agent loop → DB →
// IPC). Duplicates what the frontend already measures.
let start = SystemTime::now();
// ... do work ...
let elapsed = start.elapsed().unwrap();
db::update_message_latency(pool, message_id, &MessageLatency {
    ttfb_ms: Some(elapsed.as_millis() as i64 - 400),
    // ...
});
```

The backend already has timing (it logs the
`request_id` and `done` event latency in
`tracing::info!`). But the *measurement boundary* the
user cares about is "send click → first delta on
screen", which the frontend can measure more
accurately (the network round-trip from the
`tauri::async_runtime::spawn` IPC is the user's
perceived latency). The backend would over-count the
spawn overhead and miss the client-side render.

#### Correct: timing on the frontend (`Date.now()`)

```typescript
// GOOD — frontend deltas, persisted via IPC.
const sendAt = Date.now();
// First `delta`:
const firstDeltaAt = Date.now();
// `done`:
const doneAt = Date.now();
const ttfbMs = firstDeltaAt - sendAt;
const totalMs = doneAt - sendAt;
```

Single source of truth, same as A4 token usage. The
backend's role is persistence (the IPC + DB write), not
measurement.

#### Wrong: tool duration as a new table

```sql
-- BAD — adds a `tool_durations` table with a foreign key
-- to messages, a composite key on (session_id, tool_use_id).
CREATE TABLE tool_durations (
  session_id TEXT NOT NULL,
  tool_use_id TEXT NOT NULL,
  duration_ms INTEGER,
  PRIMARY KEY (session_id, tool_use_id)
);
```

Adds migration churn (a new table + indexes + an
on-delete-cascade constraint), rehydrate path complexity
(a third join in `load_session`), and complicates the
2013 orphan-repair logic (a duration for an orphan
tool_use needs to be cleaned up too). For one number
per tool, the cost outweighs the benefit.

#### Correct: tool duration embedded in `messages.content` JSON

```rust
// GOOD — single object mutation on the existing
// `content` JSON. The block is already there (it carries
// `tool_use_id` and `content`); adding one field is
// one INSERT-or-UPDATE.
obj.insert("duration_ms".to_string(), serde_json::Value::Number(duration_ms.into()));
```

Zero schema change. Rehydrate reads `duration_ms` off
the same block it's already walking. The 2013
orphan-repair flow is unaffected.

#### Wrong: latency columns on a separate `latencies` table

```sql
-- BAD — separate table for the three columns, joined by
-- message id. Rehydrate now has two round trips + a
-- join; the in-memory representation needs a second
-- Map.
CREATE TABLE message_latencies (
  message_id INTEGER PRIMARY KEY,
  ttfb_ms INTEGER,
  gen_ms INTEGER,
  total_ms INTEGER
);
```

Same as the tool-duration anti-pattern, but worse:
the three columns are **per-message** (not per-tool),
and every `load_session` rehydrate would need the
join. The columns are part of the message metadata;
they belong on the same row.

#### Correct: nullable INTEGER columns on `messages`

```sql
-- GOOD — three nullable columns on the existing
-- `messages` row. `load_session`'s SELECT picks them up
-- for free. NULL semantics align with the in-memory
-- `latency?: LatencyInfo` (absent = no timing).
ALTER TABLE messages ADD COLUMN ttfb_ms INTEGER;
ALTER TABLE messages ADD COLUMN gen_ms INTEGER;
ALTER TABLE messages ADD COLUMN total_ms INTEGER;
```

Pre-F5 rows keep NULL → the rehydrate path's
`hasLatency` check correctly omits the in-memory
`latency` field, and the UI shows "—". Post-F5 rows
have all three values set by the IPC.

#### Wrong: include timing in the wire payload

```typescript
// BAD — wire payload grows by 3 numbers per message.
// Doubles the LLM-side per-turn token usage payload
// (Anthropic charges input tokens, so we pay for this
// twice on the cache hit + the inbound round).
const payload = {
  role: "assistant",
  content: [...],
  // NO! timing is per-message UI metadata, not LLM state
  latency: { ttfbMs: 400, genMs: 2800, totalMs: 3200 },
};
```

Anthropic charges for input tokens including cached
content. Embedding latency in the wire payload would
add 30-50 tokens per turn that Anthropic re-parses on
every rehydrate (cache_control: ephemeral caches the
instructions, not the assistant turns).

#### Correct: timing stays in the DB / in-memory

```typescript
// GOOD — the in-memory `ChatMessage` carries the
// `latency` field for the UI, but the outbound wire
// payload (built by `toPayloadContent` in chat.ts) does
// NOT emit it. The DB column is for rehydrate on next
// session load, not for LLM round-trip.
```

The wire payload stays the same (4 fields: text /
thinking / tool_use / tool_result). The DB column is
for rehydrate. The in-memory field is for the UI. Three
disjoint concerns, three disjoint storage paths.

