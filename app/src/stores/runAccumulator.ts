// runAccumulator.ts — RunAccumulator + chat_event parsers for the
// subagentRuns store.
//
// This file is the "accumulator layer" of the subagentRuns store. It
// owns the per-runId `RunAccumulator` class (collapses the raw
// `chat_event` SSE chunk stream into Thinking / Text segments) plus
// the parsers it depends on (`parseTranscriptJson`, the `chat_event`
// field coercers, `buildSectionsFromRaw`).
//
// Why split (see PRD 06-23-06-23-split-subagent-runs):
//   `RunAccumulator` was a 318-line class + ~180 lines of supporting
//   parsers/helpers inside `subagentRuns.ts`. Moving it (with its
//   helpers) to a dedicated module gives the store a clean
//   type → accumulator → store dependency chain.
//
// ⚠️ Import-cycle note: `parseTranscriptJson` lives HERE (not in
//    `subagentRuns.ts`) because `RunAccumulator.rebuildFromCache`
//    depends on it. Keeping it in the store file would create a
//    store ↔ accumulator cycle. The store re-imports it from here.
//    Dependency direction (one-way): subagentRuns.ts → runAccumulator.ts
//    → subagentRuns.types.ts.

import { shallowRef, markRaw, type ShallowRef } from "vue";
import type {
  ChatEventInnerKind,
  TextSection,
  ThinkingSection,
  TranscriptEntry,
  TranscriptSection,
} from "./subagentRuns.types";

/** Parse `transcriptJson` (DB storage shape) into `TranscriptEntry[]`.
 *  Defensive: a missing or malformed JSON string yields `[]`. Uses
 *  `payload_json` (snake_case) — see Drift trap 2. Exported for the
 *  drawer + the vitest. */
export function parseTranscriptJson(
  raw: string | null | undefined,
): TranscriptEntry[] {
  if (!raw) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) return [];
  const out: TranscriptEntry[] = [];
  for (const entry of parsed) {
    if (!entry || typeof entry !== "object") continue;
    const e = entry as { kind?: unknown; payload_json?: unknown };
    if (typeof e.kind !== "string") continue;
    // Lenient kind coercion — mirrors `TranscriptKind` wire values.
    // 2026-06-22 (RULE-WorkerAsk-001): added `permission_ask_resolved`
    // so historical transcripts with the new resolve entries parse
    // correctly (older code would have dropped them, hiding the
    // outcome badge data from the drawer).
    if (
      e.kind !== "chat_event" &&
      e.kind !== "tool_call" &&
      e.kind !== "tool_result" &&
      e.kind !== "permission_ask" &&
      e.kind !== "permission_ask_resolved"
    ) {
      continue;
    }
    const payloadJson =
      e.payload_json && typeof e.payload_json === "object"
        ? (e.payload_json as Record<string, unknown>)
        : {};
    out.push({ kind: e.kind, payload_json: payloadJson });
  }
  return out;
}

// -----------------------------------------------------------------------
// RunAccumulator — per-runId collapse of chat_event SSE chunks
// -----------------------------------------------------------------------
//
// B6 redesign PR2 (2026-06-21). The worker emits one transcript
// entry per SSE chunk: `chat_event` (with inner kind `delta` /
// `thinking_delta` / `signature_delta` / `redacted_thinking_delta`
// / `done` / `error` / `start` / `tool_call` / `tool_result`),
// `tool_call` (independent), `tool_result` (independent),
// `permission_ask` (independent). The previous drawer showed all
// of these verbatim — including the verbose `chat_event` delta
// stream (PRD screenshots: 6963 chat_event entries hidden behind
// a toggle), which the main chat panel never exposes.
//
// The accumulator collapses the chat_event stream into two
// mutable segments (Thinking / Text) and passes tool_call /
// tool_result / permission_ask through unchanged. Live updates
// are O(1) per event (R20): each new event mutates the last
// segment's `text` / `chars` fields in place rather than
// rebuilding the array. The full transcript is rebuilt from
// scratch only on `rebuildFromCache` (called after worker
// finished → fetchRun sees the authoritative `transcriptJson`),
// per R22.
//
// Performance: the raw `chat_event` payload array is wrapped in
// `markRaw()` (R21) so Vue 3's reactivity proxy does not track
// it — saves 20000 proxy wrap operations on a 4MB JSON parse.

/** Type guard for the inner `kind` field of a `chat_event`
 *  payload. Returns the typed inner kind if the string is one
 *  of the known `ChatEvent` variants; otherwise `null`. Used
 *  by `RunAccumulator.routeChatEvent` to dispatch. */
function chatEventInnerKind(
  payload: Record<string, unknown>,
): ChatEventInnerKind | null {
  const k = payload.kind;
  if (
    k === "start" ||
    k === "delta" ||
    k === "thinking_delta" ||
    k === "signature_delta" ||
    k === "redacted_thinking_delta" ||
    k === "tool_call" ||
    k === "tool_result" ||
    k === "done" ||
    k === "error"
  ) {
    return k;
  }
  return null;
}

/** Coerce a `chat_event` payload's `text` field to a string.
 *  Defensive: the inner `ChatEvent::Delta` / `ThinkingDelta`
 *  always carry a `text: string`, but the `delta` key may be
 *  absent on `done` / `error` / `start` events. Returns `""`
 *  for non-text events. */
function chatEventText(payload: Record<string, unknown>): string {
  const t = payload.text;
  return typeof t === "string" ? t : "";
}

/** Coerce a `chat_event` payload's `signature` field to a
 *  string. Defensive: `signature_delta` always carries a
 *  `signature: string`, but other events may not. Returns `""`
 *  for non-signature events. Kept exported (not used
 *  in this file) so the discriminator rules stay in one
 *  place; tests + future "raw signature inspection" flows
 *  can call it without re-implementing the string coercion. */
export function chatEventSignature(
  payload: Record<string, unknown>,
): string {
  const s = payload.signature;
  return typeof s === "string" ? s : "";
}

/** Per-runId accumulator. The store owns one instance per
 *  runId; the instance is created on first `feed` (live path)
 *  or `rebuildFromCache` (DB path) and is dropped from the
 *  store on `clearSession` / drawer close.
 *
 *  Live path (R20 / R22): `feed(entry)` is O(1) — appends to
 *  `rawEvents` (replacing the shallow ref's value) and mutates
 *  `thinkingSegment` / `textSegment` in place when the entry is
 *  a chat_event. Non-chat events append to `transcript.value`
 *  (the `TranscriptSection[]` shallow ref) as a new section.
 *
 *  Rebuild path (R22): `rebuildFromCache(transcriptJson,
 *  finalText)` parses the JSON string once via `JSON.parse`,
 *  walks the entries linearly, and rebuilds a fresh
 *  `transcript: TranscriptSection[]`. Used after the worker
 *  finishes — the live `transcript` is discarded and replaced.
 *
 *  **Why a class (vs a free function)?** The class holds the
 *  mutable per-runId state (`thinkingSegment` /
 *  `textSegment` / `transcript` shallowRef). A class keeps the
 *  state and the mutation logic co-located, and lets the store
 *  keep a `Map<runId, RunAccumulator>` cleanly. The class is
 *  intentionally NOT a Pinia reactive object — its `transcript`
 *  is a `shallowRef` (manually wired into the store's
 *  `liveSections` Map on flush) and its segment fields are
 *  plain class fields, so Vue's deep reactivity does not
 *  track them (per R21). */
export class RunAccumulator {
  /** Raw transcript events. Wrapped in `markRaw` so Vue's
   *  proxy does not touch it (R21). Replaced (not mutated)
   *  on every `feed` — a fresh array each call. The cost is
   *  one shallowRef `.value =` write, which triggers one
   *  component re-render downstream (the drawer's
   *  `transcript` computed). */
  private readonly rawEventsShallow: ShallowRef<TranscriptEntry[]>;

  /** Per-runId derived sections. The `shallowRef` wrapper
   *  means the .value's internal array structure is NOT
   *  tracked by Vue's reactivity — only the .value identity
   *  change is. The drawer reads `liveSections.get(runId)`
   *  (set by the store on flush), which IS a `reactive`
   *  Map entry — the Map-level reactivity is the trigger. */
  public readonly transcript: ShallowRef<TranscriptSection[]>;

  /** Open thinking segment, if any. Plain class field (NOT
   *  reactive). `append` mutates this in place when a
   *  `thinking_delta` SSE event arrives; the store does NOT
   *  see the per-event mutation. The segment "publishes"
   *  itself into `transcript.value` once it closes (i.e. on
   *  the matching `signature_delta`) — at which point the
   *  store's debounce flush picks up the new
   *  TranscriptSection. */
  private thinkingSegment: ThinkingSection | null = null;

  /** Open text segment, if any. Same in-place mutation
   *  pattern as `thinkingSegment`. The Text segment never
   *  gets a closing event (Anthropic text blocks have no
   *  signature); the worker exit hook (`subagent:finished`
   *  → fetchRun → `rebuildFromCache`) replaces the live
   *  transcript with the authoritative `final_text` from
   *  the DB. Until that point, the live Text segment
   *  carries whatever the LLM has streamed so far. */
  private textSegment: TextSection | null = null;

  constructor() {
    this.rawEventsShallow = shallowRef<TranscriptEntry[]>(markRaw([]));
    this.transcript = shallowRef<TranscriptSection[]>(markRaw([]));
  }

  /** Read-only access to the raw events. Used by the test
   *  suite to assert that chat_event routing did not drop
   *  any entries. The store also reads this on
   *  `rebuildFromCache` for migration from live to cache. */
  get rawEvents(): readonly TranscriptEntry[] {
    return this.rawEventsShallow.value;
  }

  /** Append a live event. O(1) for chat_event deltas (R20);
   *  O(1) for pass-through kinds (tool_call / tool_result /
   *  permission_ask — single push to `transcript.value`).
   *  The shallowRef `.value =` assignment is a single write
   *  to the reactive system; downstream re-render happens
   *  on the debounce flush, not here.
   *
   *  **Live path does NOT accumulate `rawEvents`** (R20 — the
   *  array-spread on every event was O(N) per event, which
   *  is O(N²) cumulative over N events; for a 20k-event busy
   *  worker that hit ~1300ms, violating the R20 ceiling).
   *  `rawEvents` is only populated by `rebuildFromCache`
   *  (one JSON.parse + one linear walk — the 13ms-per-20k
   *  cold-start path, which is the AC ceiling) and is read
   *  by tests for round-trip verification. The live `feed`
   *  path mutates segments in place; raw events are not
   *  retained until the worker exits and the DB-cached
   *  authoritative transcript is rebuilt. */
  feed(entry: TranscriptEntry): void {
    if (entry.kind === "chat_event") {
      this.routeChatEvent(entry.payload_json);
      return;
    }
    if (entry.kind === "tool_call") {
      this.appendSection({ kind: "ToolCall", payload_json: entry.payload_json });
      return;
    }
    if (entry.kind === "tool_result") {
      this.appendSection({ kind: "ToolResult", payload_json: entry.payload_json });
      return;
    }
    if (entry.kind === "permission_ask") {
      this.appendSection({
        kind: "PermissionAsk",
        payload_json: entry.payload_json,
      });
      return;
    }
    if (entry.kind === "permission_ask_resolved") {
      // 2026-06-22 (RULE-WorkerAsk-001): the resolve outcome of a
      // worker's PermissionAsk. Pass-through as a
      // `PermissionAskResolvedSection` so the pairing layer
      // (`pairSections`) can match it by `rid` to the corresponding
      // `PermissionAskSection` and attach an `outcome` for the
      // historical card's outcome badge. The drawer's Tools
      // segment template does NOT render this section directly —
      // `pairSections` consumes + drops it after pairing.
      this.appendSection({
        kind: "PermissionAskResolved",
        payload_json: entry.payload_json,
      });
      return;
    }
    // Unknown kind — defensive no-op. The parse layer has
    // already filtered unknown kinds out of transcriptJson,
    // but a malformed live event (e.g. an upstream regression)
    // should not crash the store.
  }

  /** Append a `TranscriptSection` to `transcript.value`. The
   *  new array identity triggers the store's debounce flush
   *  to write into `liveSections` (R20 / R22). The
   *  markRaw wrap is redundant for arrays-of-objects (Vue's
   *  shallowRef already skips the inner proxy), but kept
   *  for symmetry with the `rawEvents` field. */
  private appendSection(section: TranscriptSection): void {
    this.transcript.value = markRaw([...this.transcript.value, section]);
  }

  /** Dispatch a `chat_event` payload to the Thinking or
   *  Text segment based on the inner `kind` field. The
   *  `text` / `signature` / `redacted_thinking.data` body
   *  fields roll up into the active segment. `start` /
   *  `done` / `error` are terminal signals that do not
   *  contribute text — they are dropped from the
   *  accumulator (the worker's exit status is tracked
   *  separately via `subagent:finished` + `coerceStatus`).
   *
   *  **Why a switch on inner kind**: Anthropic's
   *  `ChatEvent` carries an inner `kind` discriminator
   *  (`delta` / `thinking_delta` / ...). The PRD's R8
   *  explicitly requires routing `chat_event` into the
   *  right segment based on this discriminator. We default
   *  unknown / non-text inner kinds to `Text` (the most
   *  common case), but in practice the only inner kinds
   *  carrying `text` are `delta` (text block) and
   *  `thinking_delta` (thinking block). */
  private routeChatEvent(payload: Record<string, unknown>): void {
    const inner = chatEventInnerKind(payload);
    if (inner === null) return;
    switch (inner) {
      case "thinking_delta": {
        const text = chatEventText(payload);
        if (text.length === 0) return;
        if (this.thinkingSegment === null) {
          this.thinkingSegment = {
            kind: "Thinking",
            text,
            chars: text.length,
            closed: false,
          };
          this.appendSection(this.thinkingSegment);
        } else {
          this.thinkingSegment.text += text;
          this.thinkingSegment.chars += text.length;
        }
        return;
      }
      case "signature_delta": {
        // Close the active thinking block. The signature
        // itself is opaque (Anthropic blob); we don't
        // surface it to the drawer. Just flip the flag.
        if (this.thinkingSegment !== null) {
          this.thinkingSegment.closed = true;
        }
        return;
      }
      case "redacted_thinking_delta": {
        // Treat as a thinking-flavored event but DO NOT
        // expose the `data` field (Anthropic encrypts
        // redacted content — the UI shows "🔒 1 redacted").
        // We do not start a new segment here; the existing
        // thinking segment (if any) gets a marker note so
        // the drawer's chip can show "1 redacted" if it
        // wants. For PR2 the marker is stored as a
        // leading-line prefix on `text`; the drawer renders
        // it via a regex match in PR3.
        if (this.thinkingSegment === null) {
          this.thinkingSegment = {
            kind: "Thinking",
            text: "[🔒 1 redacted]",
            chars: "[🔒 1 redacted]".length,
            closed: true,
          };
          this.appendSection(this.thinkingSegment);
        } else {
          this.thinkingSegment.text += "\n[🔒 1 redacted]";
          this.thinkingSegment.chars = this.thinkingSegment.text.length;
          this.thinkingSegment.closed = true;
        }
        return;
      }
      case "delta": {
        const text = chatEventText(payload);
        if (text.length === 0) return;
        if (this.textSegment === null) {
          this.textSegment = {
            kind: "Text",
            text,
            chars: text.length,
          };
          this.appendSection(this.textSegment);
        } else {
          this.textSegment.text += text;
          this.textSegment.chars += text.length;
        }
        return;
      }
      case "start":
      case "done":
      case "error":
      case "tool_call":
      case "tool_result":
        // Terminal / pass-through signals. Not text — drop.
        return;
    }
  }

  /** Discard live state and rebuild from the DB-cached
   *  authoritative `transcriptJson`. Called by the store
   *  after `fetchRun` resolves post-`subagent:finished`
   *  (R22). The live Thinking / Text segments are dropped —
   *  whatever the LLM streamed is now superseded by the
   *  server's persisted transcript.
   *
   *  **Why replace (vs append)**: the live `transcript`
   *  may have missed events that the server persisted
   *  (network drop, debounce flush lag, etc.). Replacing
   *  is the only way to guarantee consistency with the
   *  authoritative source. The `finalText` is appended as
   *  a `FinalText` section so the drawer's Reply segment
   *  can render the prefix-stripped worker reply.
   *
   *  Performance: for 20000 events, this is one
   *  `JSON.parse` + one linear walk + one array
   *  allocation. Empirically <500ms (PRD R22 + AC).
   *  The 20000-event benchmark test in
   *  `subagentRuns.test.ts` asserts this. */
  rebuildFromCache(
    transcriptJson: string | null,
    finalText: string | null,
  ): void {
    // Reset segments + transcript. The previous
    // `transcript.value` is GC'd.
    this.thinkingSegment = null;
    this.textSegment = null;
    const raw = transcriptJson ? parseTranscriptJson(transcriptJson) : [];
    this.rawEventsShallow.value = markRaw(raw);
    // Build the derived sections linearly. We do NOT
    // reuse the live `feed` path here — that path mutates
    // segments in place; the rebuild path needs to start
    // from scratch and finish with a single fresh
    // `transcript.value`. The `buildSectionsFromRaw`
    // helper does the linear walk.
    const sections = buildSectionsFromRaw(raw);
    if (finalText !== null && finalText.length > 0) {
      sections.push({ kind: "FinalText", text: finalText });
    }
    this.transcript.value = markRaw(sections);
  }

  /** Set the `FinalText` section without touching the
   *  live transcript (used by the store when the worker
   *  finishes mid-flight and we want to surface the
   *  authoritative `final_text` without discarding the
   *  live `Text` segment yet). The store clears the live
   *  Text segment AFTER calling this so the drawer's
   *  Reply segment reads `FinalText` only.
   *
   *  **Implementation note**: in practice, the store
   *  always calls `rebuildFromCache` after
   *  `subagent:finished` (R22), so this helper is unused
   *  in the production path. Kept for tests + future
   *  "live append final_text" flows (YAGNI for now). */
  appendFinalText(text: string): void {
    if (text.length === 0) return;
    // Remove any prior FinalText section (re-entry case).
    // The cast is needed because TypeScript narrows the
    // filter callback's `kind` to the remaining union and
    // refuses to push a `FinalText` back into a
    // `NonFinalTextSection[]` array.
    const prior = this.transcript.value;
    const filtered: TranscriptSection[] = prior.filter(
      (s) => s.kind !== "FinalText",
    );
    filtered.push({ kind: "FinalText", text });
    this.transcript.value = markRaw(filtered);
  }

  /** Drop the live `Text` segment. Called by the store
   *  after `subagent:finished` → `rebuildFromCache` if
   *  the worker has streamed a partial Text segment that
   *  is now superseded by `final_text`. Unused in the
   *  current `rebuildFromCache` flow (which replaces
   *  the whole transcript), but kept for symmetry with
   *  `appendFinalText`. */
  closeTextSegment(): void {
    this.textSegment = null;
  }
}

/** Pure helper: walk the raw `TranscriptEntry[]` once and
 *  produce a `TranscriptSection[]`. Used by
 *  `RunAccumulator.rebuildFromCache` AND by the store's
 *  `rebuildSectionFromCache` (so the logic stays in one
 *  place). Kept as a free function (not a method) so the
 *  store can call it directly without instantiating an
 *  accumulator on cold cache fetch.
 *
 *  Implementation: same `routeChatEvent` rules, but
 *  starting from scratch. A `ThinkingSection` / `TextSection`
 *  is added to the output on the FIRST delta; subsequent
 *  deltas mutate the same object in place (R20). The
 *  output array's identity is stable across mutating
 *  deltas — only the inner segment's `text` / `chars`
 *  fields change. This is the load-bearing perf trick:
 *  20000 deltas → ~3 array pushes (one per segment
 *  start) + 19997 in-place string appends. No array
 *  reallocation per event. */
function buildSectionsFromRaw(
  raw: readonly TranscriptEntry[],
): TranscriptSection[] {
  const out: TranscriptSection[] = [];
  let thinking: ThinkingSection | null = null;
  let text: TextSection | null = null;
  for (const entry of raw) {
    if (entry.kind === "chat_event") {
      const inner = chatEventInnerKind(entry.payload_json);
      if (inner === null) continue;
      switch (inner) {
        case "thinking_delta": {
          const t = chatEventText(entry.payload_json);
          if (t.length === 0) continue;
          if (thinking === null) {
            thinking = {
              kind: "Thinking",
              text: t,
              chars: t.length,
              closed: false,
            };
            out.push(thinking);
          } else {
            thinking.text += t;
            thinking.chars += t.length;
          }
          continue;
        }
        case "signature_delta": {
          if (thinking !== null) thinking.closed = true;
          continue;
        }
        case "redacted_thinking_delta": {
          if (thinking === null) {
            thinking = {
              kind: "Thinking",
              text: "[🔒 1 redacted]",
              chars: "[🔒 1 redacted]".length,
              closed: true,
            };
            out.push(thinking);
          } else {
            thinking.text += "\n[🔒 1 redacted]";
            thinking.chars = thinking.text.length;
            thinking.closed = true;
          }
          continue;
        }
        case "delta": {
          const t = chatEventText(entry.payload_json);
          if (t.length === 0) continue;
          if (text === null) {
            text = { kind: "Text", text: t, chars: t.length };
            out.push(text);
          } else {
            text.text += t;
            text.chars += t.length;
          }
          continue;
        }
        case "start":
        case "done":
        case "error":
        case "tool_call":
        case "tool_result":
          continue;
      }
      continue;
    }
    if (entry.kind === "tool_call") {
      out.push({ kind: "ToolCall", payload_json: entry.payload_json });
      continue;
    }
    if (entry.kind === "tool_result") {
      out.push({ kind: "ToolResult", payload_json: entry.payload_json });
      continue;
    }
    if (entry.kind === "permission_ask") {
      out.push({ kind: "PermissionAsk", payload_json: entry.payload_json });
      continue;
    }
    if (entry.kind === "permission_ask_resolved") {
      // 2026-06-22 (RULE-WorkerAsk-001): pass-through for the
      // pairing layer. Same shape as the live `feed` path — the
      // drawer's Tools template does NOT render this section.
      out.push({
        kind: "PermissionAskResolved",
        payload_json: entry.payload_json,
      });
      continue;
    }
  }
  return out;
}
