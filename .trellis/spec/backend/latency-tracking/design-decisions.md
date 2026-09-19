<!-- Moved from latency-tracking.md 2026-09-19 (doc-split) -->

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

