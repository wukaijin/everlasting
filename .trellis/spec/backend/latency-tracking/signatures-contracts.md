<!-- Moved from latency-tracking.md 2026-09-19 (doc-split) -->

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

