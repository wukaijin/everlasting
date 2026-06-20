<!-- Latency tracking scenario. Moved from llm-contract.md 2026-06-21 (F5, 2026-06-11) -->

# Latency Tracking (F5, 2026-06-11)

> **Source**: extracted from `.trellis/spec/backend/llm-contract.md` §"Scenario: Latency Tracking" (2026-06-21 doc-trim task).
>
> **Cross-references**:
> - Main LLM contract: [llm-contract.md](./llm-contract.md)

## Scenario: Latency Tracking (F5, 2026-06-11)

> Per-message wall-clock timing for every LLM turn. Three
> measurements per turn (TTFB / gen / total) and per-tool
> duration for every tool invocation, all persisted to the
> DB so the user can see where their LLM time is going —
> both for the current turn (assistant bubble footer + Tool
> Call Card status row) and cumulatively per session
> (ChatPanel footer). Switching models, switching sessions,
> or restarting the app preserves the timing data.

### 1. Scope / Trigger

- Trigger: add a per-message latency breakdown (TTFB /
  generation / end-to-end) and a per-tool-call duration,
  display both in the chat UI, persist both to the DB.
- Why code-spec depth: mandatory — the new columns on
  `messages`, the new IPC commands, the timing
  measurement boundary (frontend `Date.now()` deltas
  around the SSE event stream), and the embed-in-JSON
  pattern for tool duration all cross multiple layers
  (Rust DB schema → Tauri IPC → Pinia store → Vue
  components) and are non-trivial to recover from
  without a code-spec. A change here cascades to
  rehydrate / latency rehydration / ToolCallCard
  rendering / ChatPanel footer.

### 2. Signatures

#### DB types (`app/src-tauri/src/db/types.rs`)

```rust
/// A message as stored in the DB. `content` is JSON (`Vec<ContentBlock>`).
pub struct MessageRow {
    pub id: i64,
    pub session_id: String,
    pub role: String,
    pub content: serde_json::Value,
    pub text: String,
    pub has_tool_calls: bool,
    pub has_tool_results: bool,
    pub created_at: String,
    pub seq: i64,
    pub metadata: Option<serde_json::Value>,
    /// F5: per-message latency breakdown. All three are
    /// `null` for pre-F5 rows; the `update_message_latency`
    /// IPC fires at `done` to populate them.
    pub ttfb_ms: Option<i64>,
    pub gen_ms: Option<i64>,
    pub total_ms: Option<i64>,
}

/// Three-field latency breakdown measured by the frontend
/// around the SSE event boundaries of one chat invocation.
/// All three fields are optional because the cancel / error
/// paths may only know the total (no `delta` was ever
/// received → no `ttfb_ms`).
#[derive(Debug, Clone, Copy, Default)]
pub struct MessageLatency {
    pub ttfb_ms: Option<i64>,
    pub gen_ms: Option<i64>,
    pub total_ms: Option<i64>,
}
```

#### DB schema (`migrations.rs` F5 ALTER)

```sql
ALTER TABLE messages ADD COLUMN ttfb_ms INTEGER;
ALTER TABLE messages ADD COLUMN gen_ms INTEGER;
ALTER TABLE messages ADD COLUMN total_ms INTEGER;
```

All three columns are **nullable** (no `DEFAULT`); a
pre-F5 session keeps NULL until its first LLM turn
post-upgrade, when `update_message_latency` initializes
them via the IPC. Tool duration follows R2 (embedded
in `messages.content` JSON; **0 schema change**).

#### DB functions (`app/src-tauri/src/db/sessions.rs`)

```rust
/// Update the three latency columns on an already-persisted
/// message row. The IPC looks up the row id by
/// `(session_id, seq)` first (via `find_message_id_by_seq`)
/// and updates by id. Each value is optional — a
/// `Some/None` mix is allowed (cancel / error paths).
pub async fn update_message_latency(
    pool: &SqlitePool,
    message_id: i64,
    latency: &MessageLatency,
) -> Result<(), sqlx::Error>;

/// Resolve `(session_id, seq)` to the auto-incrementing
/// row id. The frontend tracks the seq (the agent loop's
/// handle), not the id, so this is the IPC's lookup
/// bridge. Returns `None` if the pair is unknown (defensive
/// — the controller could in principle race the agent
/// loop's `persist_turn` if the user cancels mid-stream
/// and the cancel cleanup path persists at a later time).
pub async fn find_message_id_by_seq(
    pool: &SqlitePool,
    session_id: &str,
    seq: i64,
) -> Result<Option<i64>, sqlx::Error>;

/// Patch the `duration_ms` field onto a `tool_result`
/// content block embedded in `messages.content` JSON,
/// keyed by `tool_use_id`. Per R2 (ADR-lite decision 1),
/// the per-tool duration is embedded in the `tool_result`
/// block rather than a column — zero schema change for the
/// tool side. Returns `true` if a block was patched, `false`
/// if no matching block was found (defensive — see §4).
pub async fn record_tool_duration(
    pool: &SqlitePool,
    session_id: &str,
    tool_use_id: &str,
    duration_ms: i64,
) -> Result<bool, sqlx::Error>;
```

#### IPC commands (`app/src-tauri/src/commands/sessions.rs`)

| Command | Args (Rust) | Returns | Notes |
|---|---|---|---|
| `update_message_latency` | `session_id: String, seq: i64, ttfb_ms: Option<i64>, gen_ms: Option<i64>, total_ms: Option<i64>` | `Result<bool, String>` | Resolves `(session_id, seq)` to row id internally; returns `Ok(false)` if the seq isn't found. Fire-and-forget from the controller. |
| `record_tool_duration` | `session_id: String, tool_use_id: String, duration_ms: i64` | `Result<bool, String>` | Patches the `tool_result` block in `messages.content` JSON. `Ok(false)` = no matching block (defensive no-op). |

Both are fire-and-forget IPCs from the frontend
`streamController`; the agent loop itself does not call
them. A failure logs in the backend but doesn't surface
to the user — the in-memory value is what the UI shows.

#### Frontend payload (`app/src/stores/chat.ts`)

```typescript
/** F5: per-message latency breakdown measured by the
 *  frontend around the SSE event boundaries of one chat
 *  invocation. Mirrors the `MessageRow.ttfb_ms` / `gen_ms`
 *  / `total_ms` columns in the DB and the Rust
 *  `MessageLatency` struct. */
export interface LatencyInfo {
  ttfbMs?: number;
  genMs?: number;
  totalMs?: number;
}

export interface ToolResultInfo {
  toolUseId: string;
  content: string;
  isError: boolean;
  /** F5: per-tool wall-clock duration in ms. Embedded in
   *  the persisted `tool_result` block as `duration_ms`
   *  (per R2 / ADR-lite decision 1). The ToolCallCard
   *  displays "0.3s" next to the status text when set. */
  durationMs?: number;
}

export interface ChatMessage {
  // ... existing fields ...
  /** F5: per-message latency breakdown. Rehydrated from
   *  the `messages.ttfb_ms` / `gen_ms` / `total_ms` columns
   *  on session load; the controller populates it during
   *  streaming (via `Date.now()` deltas) and fires
   *  `update_message_latency` IPC at `done` to persist. */
  latency?: LatencyInfo;
  /** F5: the seq the agent loop assigned to this row. Used
   *  by the `update_message_latency` IPC to look up the
   *  SQLite id via `find_message_id_by_seq`. Set during
   *  rehydrate (from `messages.seq`). */
  seq?: number;
}
```

### 3. Contracts

#### Measurement boundary (frontend `Date.now()`)

Three timestamps are captured on the `RequestState`:

| Timestamp | When set | Source event |
|---|---|---|
| `sendAt` | `startRequest` | Send click |
| `firstDeltaAt` | First `delta` event of the chat | `ChatEvent::Delta` |
| `doneAt` | `done` / `error` event | `ChatEvent::Done` / `Error` |

The three millisecond values are derived at `done` /
`error` time:

```typescript
const ttfbMs = firstDeltaAt !== null ? firstDeltaAt - sendAt : null;
const genMs = firstDeltaAt !== null ? doneAt - firstDeltaAt : null;
const totalMs = doneAt - sendAt;
```

`ttfbMs` and `genMs` are `null` when no `delta` event ever
arrived (e.g. the LLM returned `end_turn` immediately on
a no-op prompt — pathological but defensive). `totalMs`
is always set. The cancel path records `totalMs` only
(no `delta` arrived between cancel and `done`).

#### Tool duration measurement

Two timestamps are captured per tool on the
`RequestState.toolStartedAt: Map<tool_use_id, number>`:

| Timestamp | When set | Source event |
|---|---|---|
| `toolStartedAt.get(id)` | `tool:call` event | `ChatEvent::ToolCall` (or the `tool:call` channel event from the agent loop) |
| `now` | `tool:result` event | `tool:result` channel event |

The duration is `Date.now() - toolStartedAt.get(id)` at
`tool:result` time. The result is:
1. Patched onto the in-memory `toolResult.durationMs` (UI sees it immediately).
2. Sent to the `record_tool_duration` IPC (DB sees it
   on reload).
3. Embedded in the `tool_result` block as `duration_ms`
   (no schema change — see R2).

#### Persistence order (round-trip invariant)

1. Frontend controller's `done` handler computes the three
   latency values, writes them to `last.latency`
   (in-memory), and updates the per-session cumulative
   total via `accumulateLatency` (so the ChatPanel footer
   updates in the same tick).
2. The controller stashes the latency on
   `req.latencyPending` (the request state, in a
   separate `completedRequests` Map so the synchronous
   `finalizeRequest` cleanup doesn't drop it before the
   async IPC fires).
3. `reloadAfterFinalize` runs as part of `finalizeRequest`.
   It loads the messages from DB (which gives us the
   assistant row's `seq`), fires the
   `update_message_latency` IPC with the seq, and
   drops the `completedRequests` entry.

The agent loop emits `done` AFTER `persist_turn` returns
(`agent::chat::chat` lines around the `Done` event
emission), so the seq is stable by the time the
controller's reload sees it. A cancel path persists a
synthetic assistant turn (the same seq-based pipeline),
so the IPC also works for cancelled turns.

#### Wire format

```typescript
// invoke("update_message_latency", { ... })
{
  sessionId: "...",
  seq: 3,
  ttfbMs: 420,    // number | null
  genMs: 2100,    // number | null
  totalMs: 3200,  // number (always)
}

// invoke("record_tool_duration", { ... })
{
  sessionId: "...",
  toolUseId: "toolu_abc",
  durationMs: 250,
}

// IPC return value
true   // patched / found
false  // seq unknown / no matching tool_result block
```

The `false` return is a defensive no-op, NOT an error.
The frontend treats it as a benign outcome (logged but
not surfaced).

#### Tool duration embed-in-JSON (R2)

The `record_tool_duration` IPC patches the `tool_result`
block in `messages.content` JSON:

```jsonc
// Before patch
{
  "type": "tool_result",
  "tool_use_id": "toolu_abc",
  "content": "...",
  "is_error": false
}

// After patch
{
  "type": "tool_result",
  "tool_use_id": "toolu_abc",
  "content": "...",
  "is_error": false,
  "duration_ms": 250
}
```

The patch is a single object mutation; the rest of the
`content` array (other blocks in the same message) and
other message columns are untouched. The
`rehydrateMessages` path reads `duration_ms` off the
block on session load — the value is available in the
ToolCallCard immediately on reload, without any extra
IPC.

### 4. Validation & Error Matrix

| Condition | Result |
|---|---|
| `update_message_latency` called for a `(session_id, seq)` pair with no matching row | IPC returns `Ok(false)` (no error); frontend logs |
| `record_tool_duration` called for a `tool_use_id` not in any persisted `tool_result` block | IPC returns `Ok(false)` (no error); frontend logs |
| `record_tool_duration` called for a `tool_use_id` only present in a tool_result block that's already been patched | Idempotent: the patch overwrites the same value |
| `record_tool_duration` for a tool result block on an assistant row (orphan-repair or synthetic-on-cancel) | The patch lands on the matching block; the block's `tool_use_id` is the discriminator |
| Cancel mid-stream (no `delta` arrived) | `totalMs` recorded, `ttfbMs` / `genMs` are `null`, UI shows "—" for them in the hover tooltip |
| Error mid-stream (network / API error) | Same as cancel: `totalMs` recorded, others `null` |
| User clock change causes `Date.now() - start` to go negative | Rehydrate clamps `duration_ms` to 0 (defensive — see `rehydrateMessages` clamp logic) |
| Pre-F5 session loaded | All three columns are `NULL` on the message rows; UI shows "—" with no tooltip; `latency` is `undefined` on the in-memory `ChatMessage` |
| Brand-new session (no LLM turn yet) | `sessionTotalLatencyMs.get(sid)` is `undefined`; UI shows "—" with the "升级前未统计" tooltip |
| Page reload after N turns | `load_session` returns rows with `ttfb_ms` / `gen_ms` / `total_ms` set; `rehydrateMessages` rebuilds `latency` per message; `ensureLoaded` sums `totalMs` over assistant rows and calls `accumulateLatency` (single seed call); subsequent turns add on top |
| Session switch mid-stream | The in-flight request keeps running on the backend; the controller's listener routes events to the matching `request_id` regardless of the user's current view. When the user returns, the cumulative is up-to-date |

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

### 6. Tests Required

#### Backend (`cargo test`)

| Test | Asserts |
|---|---|
| `persist_turn_with_latency_writes_three_columns` | `persist_turn` with `Some(&MessageLatency)` writes all three INTEGER columns |
| `persist_turn_with_no_latency_leaves_columns_null` | `persist_turn` with `None` keeps all three columns NULL (tool_result rows, pre-F5 callers) |
| `update_message_latency_patches_columns_by_id` | The IPC's backend function writes the three columns when given an id |
| `update_message_latency_accepts_partial_payload` | Cancel / error paths with `ttfb_ms = None`, `gen_ms = None` work; NULLs are written, not 0 |
| `find_message_id_by_seq_returns_none_for_unknown_pair` | Defensive: a race between controller IPC and agent loop persist returns `None` |
| `record_tool_duration_patches_matching_tool_result_block` | The function finds the matching `tool_use_id` in `messages.content` JSON, writes `duration_ms` on the block, leaves the rest of the array untouched |
| `record_tool_duration_returns_false_when_no_block_matches` | A `tool_use_id` not in any persisted block returns `Ok(false)`, no error |
| `record_tool_duration_handles_text_only_message_without_error` | A text-only message has no `tool_result` blocks; the function returns `Ok(false)` cleanly |

#### Frontend (`pnpm test`)

| Test | Asserts |
|---|---|
| `rehydrateMessages — F5 latency rehydration > populates the latency triple on an assistant message that has all three values` | `rehydrateMessages` builds `latency: { ttfbMs, genMs, totalMs }` for a fully-set row |
| `rehydrateMessages — F5 latency rehydration > omits latency when all three columns are NULL (pre-F5 rows)` | The `hasLatency` check correctly omits the field |
| `rehydrateMessages — F5 latency rehydration > includes only the non-NULL fields in a partial-latency row` | Cancel-path latency with only `totalMs` set is rendered with just the total |
| `rehydrateMessages — F5 per-tool duration rehydration > reads duration_ms off a persisted tool_result block` | The merge step + the per-block read both surface `durationMs` |
| `rehydrateMessages — F5 per-tool duration rehydration > leaves durationMs undefined when the field is missing (pre-F5 rows)` | Pre-F5 blocks render no time |
| `rehydrateMessages — F5 per-tool duration rehydration > rounds fractional durationMs to an integer` | Defensive round |
| `rehydrateMessages — F5 per-tool duration rehydration > clamps negative durationMs to 0 (defensive against clock skew)` | Pathological user clock change doesn't break the UI |
| `abbreviateDuration — formats sub-second durations with one decimal` | `0.4s`, `1.0s`, `999ms → 1.0s` |
| `abbreviateDuration — formats sub-minute durations with one decimal` | `1.0s`, `3.2s`, `59.9s` |
| `abbreviateDuration — switches to 'Xm Ys' format past 60 seconds` | `1m 0s`, `1m 23s`, `12m 4s` |
| `abbreviateDuration — formats the seconds portion with one decimal only when fractional` | `1m 30s` not `1m 30.0s`; `1m 30.5s` for fractional |
| `abbreviateDuration — clamps negative inputs to 0.0s` | Defensive against clock skew |
| `abbreviateDuration — clamps NaN / Infinity to 0.0s` | Defensive against buggy upstreams |

#### Existing 2013 / A4 invariants (must continue to pass)

| Test | Asserts |
|---|---|
| `finalizeRequest (06-08-06-08 step-4 follow-up — 2013 wire invariant) > evicts the in-memory message buffer and unloads from DB cache` | `pinnedSessions` is cleared on `finalizeRequest` (the synchronous part of the F5 contract) |
| `finalizeRequest (06-08-06-08 step-4 follow-up — 2013 wire invariant) > invalidates the chat store's diff cache for the same session` | `invalidateDiff` still runs (paired invariant) |
| `finalizeRequest (06-08-06-08 step-4 follow-up — 2013 wire invariant) > both actions fire on the same finalizeRequest call (paired invariant)` | `pinnedSessions` clear + `diffCache` clear happen in the same synchronous tick |

The 2013 tests' buffer-clear assertions were updated
in F5 (they no longer assert `messagesBySession` is
cleared synchronously — that's now `reloadAfterFinalize`'s
async job). The synchronous contract that
`finalizeRequest` owns is the `pinnedSessions` /
`activeRequests` cleanup.

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

### Design Decisions

#### Decision: Tool duration embedded in `tool_result` JSON (R2, locked 2026-06-11)

**Context**: The original F5 spec (in the
`06-11-session-loading` archive task) assumed a
separate `tool_results` table. The actual DB schema
embeds `tool_result` blocks in `messages.content`
JSON.

**Decision**: Tool-use-id-scoped `durationMs` is
written onto the `tool_result` block in
`messages.content` JSON. The backend uses
`serde_json::Value::pointer_mut` (via the
`record_tool_duration` function) to patch the
matching block. New IPC `record_tool_duration(session_id,
tool_use_id, duration_ms)`.

**Consequences**:
- Zero schema change for the tool side; only the 3
  column ALTERs for R3 (TTFB / gen / total).
- Rehydrate path is zero-modification (the function
  already walks the `content` array, picking up
  `duration_ms` along the way).
- Trade-off: `content` JSON gains one field per
  tool_result block. ~25 bytes per tool call —
  negligible.
- The 2013 orphan-repair flow is unaffected
  (orphan-repaired `tool_result` blocks are
  identical to live ones; the IPC patches them by
  `tool_use_id` lookup).

#### Decision: Frontend `Date.now()` timing (ADR-lite, locked 2026-06-11)

**Context**: A4 token usage is also frontend-computed;
`test_provider` has `latencyMs` but that's a single
HTTP probe (not the per-message timing the user
wants to see).

**Decision**: F5 measures everything on the
frontend. R1's three values (TTFB / gen / total) and
R2's per-tool duration are all `Date.now()` deltas.
The backend's only role is persistence (the
`update_message_latency` and `record_tool_duration`
IPCs) — no `Instant::now()` / `SystemTime` calls in
the agent loop or the SSE parser.

**Consequences**:
- Consistency with A4 (single source of truth = the
  frontend).
- No new system-clock coupling between Rust and
  TypeScript; the agent loop stays timing-agnostic.
- Known limitation: a user who changes their system
  clock mid-stream will see weird numbers (negative
  TTFB, etc.). The rehydrate path clamps to 0
  (defensive). Same trade-off as A4 — acceptable.
- The `request_id`-based event routing in the
  controller means a single request's timing stays
  coherent even when the user switches sessions
  mid-stream.

#### Decision: Per-message IPC + cumulative in-memory (matches A4)

**Context**: A4's token usage has per-session
cumulative (`sessions.*_total` columns +
`tokenUsageBySession` map). F5 follows the same
shape for the latency cumulative (in-memory
`sessionTotalLatencyMs` map, no schema column for
the cumulative).

**Decision**: The cumulative is a frontend-only
projection. The DB stores the per-message
`ttfb_ms` / `gen_ms` / `total_ms` columns; the
`sessionTotalLatencyMs` map is `Σ totalMs WHERE
role = 'assistant' AND totalMs IS NOT NULL`,
rehydrated on session load.

**Consequences**:
- Reload after N turns shows the cumulative
  immediately (seeded from the messages in
  `ensureLoaded`).
- No `sessions.total_latency_ms` column needed
  (the SUM is trivial; the DB doesn't have to keep
  the running total).
- The cumulative updates synchronously on `done`
  (the chat store's `accumulateLatency` runs in the
  same tick as the in-memory message update), so
  the ChatPanel footer reflects the new value
  without an extra IPC.

#### Decision: 1 PR all-in (locked 2026-06-11)

**Context**: R1-R8 are tightly coupled (timing
measurement → in-memory mutation → IPC fire →
DB column write → rehydrate path → UI rendering).
A 2-PR split would have an un-runnable middle state.

**Decision**: 1 PR for the whole F5. Same pattern as
A4 (LLM usage parsing + DB schema + agent loop + UI
+ spec + decision log).

**Consequences**: 8-12 file diff (Rust 4-5 + Vue
3-4 + spec 1 + docs 1). Review difficulty rises;
commit message must list all touched concerns.

### Future Work (Deferred from F5)

| Item | Why deferred |
|---|---|
| P50 / P95 latency stats per session | Out of scope (PRD OOS #1). The user wants to *see* their LLM calls, not analyze them statistically. |
| Historical trend chart across sessions | Out of scope (PRD OOS #2). |
| CSV / JSON export of latency data | Out of scope (PRD OOS #3). |
| Backend-precise timing (Rust `Instant::now()`) | PRD ADR-lite decision 2. |
| Token rate (tokens/second) | Out of scope (PRD OOS #5). |
| Per-model / per-provider latency breakdown | Out of scope (PRD OOS #6). |
| Cross-session global cumulative | Out of scope (PRD OOS #7). |
| Per-session latency in SessionList sidebar row | Out of scope (PRD OOS #8). |
| Persist the cumulative total in a `sessions.total_latency_ms` column | Not needed — the SUM is trivial; in-memory is fine. |
| `update_message_latency` for tool_result rows | Tool-result rows have no per-message latency triple (per-tool duration lives in the JSON, not the columns). The IPC is only called for assistant turns. |
| LLM-claimed TTFB (parse from `usage.creation_time` if exposed) | Anthropic doesn't expose this on `message_delta`; the frontend's `Date.now()` is the only source. |

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

At the `persist_turn` call site (line 600-607) the existing `latency: Option<&MessageLatency>` parameter is filled with `Some(&MessageLatency { ttfb_ms, gen_ms, total_ms, thinking_ms })` derived from the 4 Instants. The INSERT statement (`app/src-tauri/src/db/sessions.rs:565-595`) already binds all 4 columns — F5 added `thinking_ms` on 2026-06-12. Per-turn rows therefore get all 4 columns populated atomically, no follow-up `UPDATE` needed for the common case.

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

`update_message_latency` IPC signature is unchanged (F5 + 2026-06-12 already takes `(sessionId, seq, ttfbMs, genMs, totalMs, thinkingMs)`). The 4-column `UPDATE` in `app/src-tauri/src/db/sessions.rs:662-677` is also unchanged — it's just called N times instead of once.
