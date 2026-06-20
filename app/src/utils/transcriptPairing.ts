// Transcript pairing buffer — B6 PR3 redesign (2026-06-21).
//
// Purpose: pair adjacent `tool_call` and `tool_result` transcript
// entries by `tool_use_id` so the SubagentDrawer can render them as
// a single merged card (matching the main panel's `ToolCallCard`
// visual language). Without this layer, the drawer would show the
// call + result as two separate cards — visually noisy and
// inconsistent with the main panel.
//
// Layered on top of `TranscriptEntry` (from `stores/subagentRuns.ts`);
// the drawer's `transcript` computed reads the raw entries from
// `store.liveTranscript` / `run.transcriptJson`, then this module's
// `pairTranscript` produces a `BufferedTranscriptEntry[]` that the
// template iterates over.
//
// See `.trellis/tasks/06-21-redesign-subagent-drawer-entry-as-toolcard-style/prd.md`
// §"Technical Approach" for the design rationale and acceptance
// criteria.

import type { TranscriptEntry } from "../stores/subagentRuns";

// Re-export `TranscriptEntry` so the test file (and any other
// downstream consumer) can import it from a single place. The
// store remains the canonical source; this re-export exists
// only for the convenience of the pairing-layer test and
// future call sites that already pull from `transcriptPairing`.
export type { TranscriptEntry } from "../stores/subagentRuns";

/** A `tool_call` transcript entry waiting for its `tool_result`
 *  pair. Tracks the wall-clock `received_at` (ms since epoch) so
 *  the 30s timeout flush can decide when a pending call has
 *  "aged out" and should fall back to a standalone "未完成" card. */
export interface PendingCall {
  tool_use_id: string;
  call: TranscriptEntry;
  received_at: number;
}

/** How long a pending `tool_call` waits for its matching
 *  `tool_result` before the pairing layer flushes it as a
 *  standalone "未完成" card. 30s matches the worker's
 *  `max_turns: Some(20)` × per-turn latency budget (each tool
 *  typically returns in <10s; 30s is a generous bound for the
 *  the rare slow-network / large-output case). The PRD
 *  explicitly chose 30s (see prd.md §"Q1.1 = A"). */
export const PENDING_TIMEOUT_MS = 30_000;

/** Three-state union returned by `pairTranscript`. The drawer's
 *  template branches on `kind`:
 *
 *  - `paired` — call + result both arrived; rendered as a merged
 *    `.tool-card` (header shows tool name + status + duration).
 *  - `pending_call` — call arrived, result still pending (within
 *    30s); rendered as an amber-bordered card with a pulsing
 *    indicator.
 *  - `standalone` — chat_event / permission_ask / orphan call or
 *    result (no match). Each sub-kind has its own visual
 *    (chat_event: muted; permission_ask: amber; orphan call:
 *    "未完成" with timeout; orphan result: standard result card).
 */
export type BufferedTranscriptEntry =
  | {
      kind: "paired";
      tool_use_id: string;
      call: TranscriptEntry;
      result: TranscriptEntry;
    }
  | {
      kind: "pending_call";
      tool_use_id: string;
      call: TranscriptEntry;
    }
  | {
      kind: "standalone";
      entry: TranscriptEntry;
    };

/** Pull `tool_use_id` out of a transcript entry's `payload_json`.
 *  Defensive: missing or non-string field returns `undefined` so
 *  the caller falls back to a standalone render (no match). The
 *  pairing layer is read-only — it never mutates the entries. */
function readToolUseId(e: TranscriptEntry): string | undefined {
  const id = e.payload_json?.tool_use_id;
  return typeof id === "string" && id.length > 0 ? id : undefined;
}

/** Pair adjacent `tool_call` and `tool_result` transcript entries
 *  into merged cards. The pairing is order-preserving: a `paired`
 *  entry appears at the position of the matching `tool_result`
 *  (the call's position is absorbed into the result's card). A
 *  pending call that hasn't matched by the time `now - received_at
 *  >= PENDING_TIMEOUT_MS` falls back to `standalone` so the user
 *  sees a "未完成" card with a timeout hint.
 *
 *  This is a pure function — no I/O, no mutation of the input
 *  array. The `pendingFirstSeenAt` map is **mutated by the
 *  function** to track the wall-clock `received_at` of each
 *  pending call across invocations; the caller (the drawer)
 *  keeps a stable reference to the same Map instance so that
 *  re-invocations with advancing `now` can age out calls
 *  correctly.
 *
 *  The "received_at" is the timestamp at which a `tool_call` was
 *  FIRST seen (added to the map) — not the current `now`. This is
 *  the only way the 30s timeout can elapse between invocations:
 *  the caller re-invokes the function periodically with `now =
 *  Date.now()`, and the map's first-seen timestamps persist
 *  between calls. If we used `now` (the current invocation's
 *  timestamp) for `received_at`, every call would always be
 *  "just received" and would never time out.
 *
 *  Edge cases (locked by `transcriptPairing.test.ts`):
 *  - chat_event / permission_ask: always standalone (not
 *    pairable).
 *  - Orphan tool_result (no preceding tool_call): standalone,
 *    rendered as a regular result card.
 *  - Orphan tool_call that aged out past 30s: standalone
 *    "未完成" card. Pending calls within 30s stay as
 *    `pending_call`.
 *  - Two calls with the same `tool_use_id` (theoretical race
 *    where a tool_use is re-emitted): the second one overwrites
 *    the first in the pending map. The orphaned first call lands
 *    as a standalone entry on the next iteration. This is
 *    defensive — Anthropic never emits the same tool_use_id
 *    twice in one turn, but the buffer's per-id last-write-wins
 *    semantics keep us safe.
 */
export function pairTranscript(
  entries: readonly TranscriptEntry[],
  now: number,
  pendingFirstSeenAt: Map<string, number>,
): BufferedTranscriptEntry[] {
  const pending = new Map<string, PendingCall>();
  const out: BufferedTranscriptEntry[] = [];

  for (const e of entries) {
    if (e.kind === "tool_call") {
      const id = readToolUseId(e);
      if (id === undefined) {
        // Defensive: a tool_call without a `tool_use_id` field
        // is unusual (the backend injects it in
        // `SubagentBufferSink::emit_tool_call`), but if a
        // pre-redesign row sneaks through (e.g. a transcript
        // persisted before the backend PR landed), render it
        // standalone rather than dropping it.
        out.push({ kind: "standalone", entry: e });
        continue;
      }
      // Record the first-seen timestamp ONLY on first sight.
      // On subsequent invocations the existing entry stays
      // intact, so the call can age out across re-invocations
      // (the drawer's `setInterval` re-calls every 5s with
      // advancing `now`).
      if (!pendingFirstSeenAt.has(id)) {
        pendingFirstSeenAt.set(id, now);
      }
      pending.set(id, {
        tool_use_id: id,
        call: e,
        received_at: pendingFirstSeenAt.get(id)!,
      });
    } else if (e.kind === "tool_result") {
      const id = readToolUseId(e);
      if (id === undefined) {
        // Same defensive fallback for tool_result without
        // tool_use_id (orphaned pre-redesign row). The
        // `duration_ms` field is also missing on pre-redesign
        // rows; the `ToolOutputBody` treats `durationMs ===
        // undefined` as "omit duration chip" (per its file
        // header), so rendering is a no-op visual regression
        // — exactly the right behavior for legacy rows.
        out.push({ kind: "standalone", entry: e });
        continue;
      }
      const p = pending.get(id);
      if (p) {
        out.push({ kind: "paired", tool_use_id: id, call: p.call, result: e });
        pending.delete(id);
        // Clean up the first-seen map on a successful pair so
        // a future re-emit of the same id (shouldn't happen,
        // but defensive) restarts the 30s window.
        pendingFirstSeenAt.delete(id);
      } else {
        // Orphan tool_result — the call was lost (IPC drop,
        // 4 MiB transcript truncation, etc.). Surface as a
        // standalone card; the user sees a regular result
        // card with no preceding call.
        out.push({ kind: "standalone", entry: e });
      }
    } else {
      // chat_event / permission_ask: always standalone.
      out.push({ kind: "standalone", entry: e });
    }
  }

  // Flush remaining pending calls. Within the timeout window they
  // stay as `pending_call` (so the UI can show an "in flight"
  // indicator); past the window they fall back to `standalone`
  // with the "未完成" hint. The `received_at` is the first-seen
  // timestamp stored in `pendingFirstSeenAt`; we use that value
  // here (NOT `now`) so the age-out reflects actual elapsed
  // wall-clock time across invocations.
  //
  // B6 PR3 check-phase fix (2026-06-21): we do NOT delete the
  // entry from `pendingFirstSeenAt` on the timeout flush.
  // Previously the delete + re-set on the next call reset the
  // timer to "now", so the standalone → pending_call transition
  // would flicker every 30s (standalone for one tick, then
  // back to pending_call, then standalone again 30s later).
  // The same `tool_use_id` is never re-emitted by Anthropic in
  // practice, so a stale entry is bounded by the number of
  // distinct tool_use_ids ever seen in this app session —
  // trivial memory cost. If a re-emit ever does happen, the
  // existing entry's `received_at` is kept (the original "first
  // seen" timestamp) so the timeout keeps ticking correctly.
  for (const p of pending.values()) {
    if (now - p.received_at >= PENDING_TIMEOUT_MS) {
      out.push({ kind: "standalone", entry: p.call });
    } else {
      out.push({ kind: "pending_call", tool_use_id: p.tool_use_id, call: p.call });
    }
  }

  return out;
}

/** Convenience: did the entry's `tool_result` report
 *  `is_error === true`? Returns `false` for non-`tool_result`
 *  entries (no concept of "error"). Defensive: missing /
 *  non-boolean `is_error` defaults to `false` (matches the
 *  Rust `ToolResultPayload::is_error: bool` default). */
export function isErrorResult(e: TranscriptEntry): boolean {
  if (e.kind !== "tool_result") return false;
  return e.payload_json?.is_error === true;
}
