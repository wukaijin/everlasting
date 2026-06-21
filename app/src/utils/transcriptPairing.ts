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
// B6 redesign PR5 (2026-06-21): the drawer was rewritten from a
// time-ordered flat list into a 5-segment grouped view that reads
// `TranscriptSection[]` (post-accumulator). The legacy
// `pairTranscript` is preserved for backward compat (and any future
// raw-list consumer); PR5 adds `pairSections` + the
// `SectionToolPair` / `SectionPermissionAsk` / `SectionPendingCall`
// types as the new pairing layer over sections. The two functions
// share the 30s pending-timeout semantics so the visual transition
// from "running…" to "未完成" is identical.
//
// See `.trellis/tasks/06-21-redesign-subagent-drawer-entry-as-toolcard-style/prd.md`
// §"Technical Approach" for the design rationale and acceptance
// criteria.

import type { TranscriptEntry, TranscriptSection } from "../stores/subagentRuns";
import type { ToolCallInfo, ToolResultInfo } from "../stores/chat";

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

// =====================================================================
// B6 redesign PR5 (2026-06-21): section-level pairing layer
// =====================================================================
//
// The new drawer reads `TranscriptSection[]` from the store's
// `liveSections` accumulator (PR2 output). Each `ToolCallSection` +
// `ToolResultSection` pair must collapse into a single
// `DrawerToolCallCard`, mirroring the legacy `pairTranscript`
// semantics but consuming the post-accumulator section shape.
//
// The PR4 `DrawerToolCallCard` component accepts the canonical
// `ToolCallInfo` + `ToolResultInfo` types (same shape the main
// panel's `ToolCallCard` consumes). The pairing layer below maps
// the snake_case `payload_json` body to these canonical types —
// this is the load-bearing cross-layer conversion PR5 owns.
//
// `permission_ask` sections are NOT pairable; they pass through as
// `SectionPermissionAsk` and the drawer renders them as a static
// card (PR6 will add Allow/Deny buttons). `pending_call` (call
// without matching result) stays pending within the 30s window —
// matching the legacy `pairTranscript` invariant — and falls back
// to a "未完成" `SectionPendingCall` after the timeout elapses.
// However, because PR5's section-level view defaults to rendering
// a pending call as a "running…" `DrawerToolCallCard` (with
// `result === undefined`), the timeout flush is NOT visually
// load-bearing here (the running card already conveys "未完成").
// We still return the aged-out entries as `SectionPendingCall`
// with `timedOut: true` so PR6 / future polish can distinguish
// "still running" from "timed out without a result".

/** A `ToolCallSection` + matching `ToolResultSection` collapsed
 *  into a single card payload. `call` is the canonical
 *  `ToolCallInfo` (camelCase) consumed directly by
 *  `DrawerToolCallCard`. */
export interface SectionToolPair {
  kind: "paired";
  toolUseId: string;
  call: ToolCallInfo;
  result: ToolResultInfo;
}

/** A `ToolCallSection` whose matching `ToolResultSection` hasn't
 *  arrived yet. Rendered as a "running…" `DrawerToolCallCard`
 *  (no `result` prop). `timedOut` flips to `true` once the 30s
 *  window elapses (matches the legacy `pairTranscript` flush);
 *  PR5's running card renders identically for both states, but
 *  the flag is kept for future polish (e.g. PR6 / a "未完成"
 *  suffix in the card header). */
export interface SectionPendingCall {
  kind: "pending_call";
  toolUseId: string;
  call: ToolCallInfo;
  timedOut: boolean;
}

/** A `PermissionAskSection` pass-through. The drawer renders this
 *  via `DrawerPermissionAskCard` (a static historical-mode card,
 *  PR5 scope). PR6 will replace the static card with interactive
 *  Allow/Deny buttons wired to the `permission:response` IPC. */
export interface SectionPermissionAsk {
  kind: "permission_ask";
  payload_json: Record<string, unknown>;
}

/** Discriminated union returned by `pairSections`. The drawer's
 *  `DrawerSection(type="tools")` template branches on `kind`. */
export type SectionToolEntry = SectionToolPair | SectionPendingCall | SectionPermissionAsk;

/** Pull `tool_use_id` out of a section's `payload_json`. Defensive:
 *  missing or non-string field returns `undefined` so the caller
 *  falls back to a standalone render (no match). */
function readSectionToolUseId(p: Record<string, unknown>): string | undefined {
  const id = p?.tool_use_id;
  return typeof id === "string" && id.length > 0 ? id : undefined;
}

/** Map a `ToolCallSection.payload_json` (snake_case) to the
 *  canonical `ToolCallInfo` (camelCase). Defensive: missing
 *  `input` coerces to `{}` (the `DrawerToolCallCard`'s empty-input
 *  guard treats `{}` as "no input body"). Missing `name` coerces
 *  to `""` (the card header shows a placeholder). */
function toToolCallInfo(p: Record<string, unknown>): ToolCallInfo {
  const id = readSectionToolUseId(p) ?? "";
  const name = typeof p.name === "string" ? p.name : "";
  const rawInput = p.input;
  const input =
    rawInput && typeof rawInput === "object" && !Array.isArray(rawInput)
      ? (rawInput as Record<string, unknown>)
      : {};
  return { id, name, input };
}

/** Map a `ToolResultSection.payload_json` (snake_case) to the
 *  canonical `ToolResultInfo` (camelCase). Defensive: missing
 *  `is_error` defaults to `false` (matches the Rust
 *  `ToolResultPayload::is_error: bool` default). Missing /
 *  non-finite `duration_ms` returns `undefined` (the
 *  `DrawerToolCallCard`'s `durationLabel` treats `undefined` as
 *  "omit duration chip"). */
function toToolResultInfo(
  toolUseId: string,
  p: Record<string, unknown>,
): ToolResultInfo {
  const content = typeof p.content === "string" ? p.content : "";
  const isError = p.is_error === true;
  const d = p.duration_ms;
  const durationMs =
    typeof d === "number" && Number.isFinite(d) && d >= 0 ? d : undefined;
  return { toolUseId, content, isError, durationMs };
}

/** Pair adjacent `ToolCallSection` + `ToolResultSection` entries
 *  into merged `SectionToolPair` payloads. Same 30s pending-timeout
 *  semantics as the legacy `pairTranscript`: a call that hasn't
 *  matched by `now - received_at >= PENDING_TIMEOUT_MS` flips to
 *  `SectionPendingCall.timedOut = true`. `permission_ask` sections
 *  pass through as `SectionPermissionAsk`.
 *
 *  This is a pure function — no I/O, no mutation of the input
 *  array. `pendingFirstSeenAt` is mutated by the function to track
 *  first-seen timestamps across invocations (same contract as
 *  `pairTranscript`): the caller keeps a stable Map reference so
 *  advancing `now` can age out pending calls correctly.
 *
 *  Edge cases:
 *    - Orphan `ToolResultSection` (no preceding call): the legacy
 *      `pairTranscript` would render this as a standalone result
 *      card. PR5's section view drops orphan results (they should
 *      not appear in practice — every result has a matching call
 *      in the same worker turn). If you need to surface orphans,
 *      fall back to `pairTranscript` on the raw entry list.
 *    - `ToolCallSection` without `tool_use_id`: dropped (defensive
 *      — the backend always injects the id; a pre-redesign row
 *      without it is treated as corrupt).
 *    - `ThinkingSection` / `TextSection` / `FinalTextSection`:
 *      skipped (they belong to the thinking / reply segments, not
 *      the tools segment).
 */
export function pairSections(
  sections: readonly TranscriptSection[],
  now: number,
  pendingFirstSeenAt: Map<string, number>,
): SectionToolEntry[] {
  const pending = new Map<string, { call: ToolCallInfo; receivedAt: number }>();
  const out: SectionToolEntry[] = [];

  for (const s of sections) {
    if (s.kind === "ToolCall") {
      const id = readSectionToolUseId(s.payload_json);
      if (id === undefined) continue;
      if (!pendingFirstSeenAt.has(id)) {
        pendingFirstSeenAt.set(id, now);
      }
      pending.set(id, {
        call: toToolCallInfo(s.payload_json),
        receivedAt: pendingFirstSeenAt.get(id)!,
      });
      continue;
    }
    if (s.kind === "ToolResult") {
      const id = readSectionToolUseId(s.payload_json);
      if (id === undefined) continue;
      const p = pending.get(id);
      if (p) {
        out.push({
          kind: "paired",
          toolUseId: id,
          call: p.call,
          result: toToolResultInfo(id, s.payload_json),
        });
        pending.delete(id);
        pendingFirstSeenAt.delete(id);
      }
      // Orphan result (no preceding call): silently drop. See
      // docstring for rationale.
      continue;
    }
    if (s.kind === "PermissionAsk") {
      out.push({ kind: "permission_ask", payload_json: s.payload_json });
      continue;
    }
    // ThinkingSection / TextSection / FinalTextSection: skipped.
    // The drawer routes those to the thinking / reply segments,
    // not the tools segment.
  }

  // Flush remaining pending calls. Within the timeout window they
  // stay as `SectionPendingCall` (timedOut=false); past the window
  // they flip to timedOut=true. PR5 renders both states identically
  // (a running `DrawerToolCallCard`), but the flag is preserved
  // for future polish.
  for (const [id, p] of pending) {
    const elapsed = now - p.receivedAt;
    const timedOut = elapsed >= PENDING_TIMEOUT_MS;
    out.push({
      kind: "pending_call",
      toolUseId: id,
      call: p.call,
      timedOut,
    });
  }

  return out;
}
