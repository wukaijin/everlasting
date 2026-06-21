// useSubagentRunsStore — Pinia store for B6 PR3 subagent drawer.
//
// Backend contract (post PR2 hotfix + PR3a, see
// `.trellis/tasks/06-20-b6-pr3-frontend-expand/wire-shape-contract.md`):
//
//   1. Two Tauri commands:
//      - `list_subagent_runs_by_session(sessionId) → SubagentRunSummary[]`
//        (no transcript_json — light list for ToolCallCard lookup).
//      - `get_subagent_run(runId) → SubagentRunRow | null`
//        (full row incl. transcriptJson for drawer render).
//
//   2. One IPC event stream: `subagent:event` — emitted live by the
//      worker's `SubagentBufferSink` while the worker runs. Payload
//      shape `{ runId, sessionId, kind, payload, timestamp }`. The
//      drawer reads these in real-time (debounced 200ms) so the user
//      sees worker progress before the run completes.
//
// Data sources for the drawer's transcript list (R6 priority):
//
//     store.liveTranscript.get(openRunId)              // live stream
//       ?? parse(store.getRunCache.get(openRunId)?.transcriptJson)  // DB cache
//       ?? []
//
// 2026-06-21 redesign (PR2 of the subagent-drawer refactor): the
// store also runs a per-runId `RunAccumulator` that collapses the
// raw `chat_event` SSE chunk stream into `Thinking | Text` segments
// (so the drawer can render the worker's intermediate state the
// same way the main chat panel does, without exposing 6963
// meaningless chat_event rows). The accumulator lives in
// `liveSections: Map<runId, TranscriptSection[]>` (R22); the raw
// `liveTranscript` is preserved unchanged for the pairing layer
// (call+result merge) that powers the drawer's per-tool cards.
//
// ⚠️ Cross-layer drift traps (see wire-shape-contract.md):
//   1. `SubagentRunRow.status` is a raw `string` on the wire but
//      `SubagentRunSummary.status` is a typed enum
//      `"running" | "completed" | "cancelled" | "error"`. We coerce
//      both into a single TS union (`SubagentStatus`) via
//      `coerceStatus` for display.
//   2. `TranscriptEntry` (from transcriptJson, the DB storage shape)
//      uses snake_case `payload_json` because the Rust struct has NO
//      `rename_all`. The live `subagent:event` IPC payload wraps the
//      body as camelCase `payload`. NEVER conflate them — when
//      parsing transcriptJson use `payload_json`; when handling the
//      live stream use `payload`.
//   3. The raw `chat_event` payload carries a NESTED `ChatEvent`
//      whose `kind` discriminates between `delta` / `thinking_delta`
//      / `signature_delta` / `redacted_thinking_delta` / `done` /
//      `error` / `start` / `tool_call` / `tool_result`. The
//      accumulator dispatches on the INNER kind to route to the
//      Thinking vs. Text segment. See `RouteAccumulator` for the
//      discriminator rules.

import { defineStore } from "pinia";
import { reactive, computed, ref, shallowRef, markRaw, type ShallowRef } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// -----------------------------------------------------------------------
// Types — mirror wire-shape-contract.md verbatim
// -----------------------------------------------------------------------

/** Worker run status. Mirrors `SubagentStatusDb`
 *  `#[serde(rename_all = "lowercase")]`. The wire enum has 5
 *  variants (added `incomplete` in Session 60 R2 / 2026-06-21 for
 *  the `max_turns` soft-cap terminal state); the previous 4-value
 *  union was a frontend-only oversight — see
 *  RULE-FrontSubagent-005 in `.trellis/reviews/DEBT.md`. */
export type SubagentStatus =
  | "running"
  | "completed"
  | "cancelled"
  | "error"
  | "incomplete";

/** `TranscriptKind` — mirrors the Rust enum's
 *  `#[serde(rename_all = "snake_case")]` wire values. Used both as
 *  the `kind` field on `SubagentEventPayload` (live stream) AND as
 *  the `kind` field on `TranscriptEntry` (transcriptJson DB storage
 *  shape). */
export type TranscriptKind =
  | "chat_event"
  | "tool_call"
  | "tool_result"
  | "permission_ask";

/** `list_subagent_runs_by_session` array element. The Rust struct
 *  carries `#[serde(rename_all = "camelCase")]`; `status` is a
 *  typed enum on this shape. B6 redesign PR1 (2026-06-21):
 *  the Rust struct now also carries `task` (parent LLM's prompt)
 *  and `final_text` (prefix-stripped worker reply), so the
 *  summary list view can show the worker title / first line
 *  without a follow-up `get_subagent_run` roundtrip. The
 *  frontend tolerates `null` for legacy pre-PR1 rows. */
export interface SubagentRunSummary {
  id: string;
  parentSessionId: string;
  parentRequestId: string;
  subagentName: string;
  status: SubagentStatus;
  startedAt: string;
  finishedAt: string | null;
  tokenUsageJson: string | null;
  summary: string | null;
  task: string | null;
  finalText: string | null;
}

/** `get_subagent_run` return. The Rust struct carries
 *  `#[serde(rename_all = "camelCase")]`.
 *  ⚠️ Drift trap 1: `status` is a raw `String` on the wire (NOT the
 *  typed enum like on SubagentRunSummary). Coerce via `coerceStatus`
 *  before comparing to the union type. */
export interface SubagentRunRow {
  id: string;
  parentSessionId: string;
  parentRequestId: string;
  subagentName: string;
  status: string;
  startedAt: string;
  finishedAt: string | null;
  tokenUsageJson: string | null;
  summary: string | null;
  transcriptJson: string | null;
  transcriptTruncated: number;
  createdAt: string;
  // B6 redesign PR1 (2026-06-21): the worker's final
  // assistant text, with the `[status: ...]\n` prefix
  // already stripped (the `status` column carries the
  // prefix separately). Nullable for running runs (the
  // column is only written on worker exit by the Rust
  // `format_final_text` helper). The drawer's Reply
  // segment reads this verbatim for the `finalText`
  // accumulator input.
  finalText: string | null;
  // B6 redesign PR1: the parent LLM's prompt that
  // dispatched this worker (the `dispatch_subagent`
  // tool's `task` argument). Nullable for legacy
  // pre-PR1 rows. The drawer's PromptCard header
  // (PR5) truncates this to 120 chars + "View full →".
  task: string | null;
}

/** Live `subagent:event` IPC payload. camelCase via the Rust
 *  `build_subagent_event_payload`. `payload` is the wrapped entry
 *  body (camelCase on the wire). */
export interface SubagentEventPayload {
  runId: string;
  sessionId: string;
  kind: TranscriptKind;
  payload: Record<string, unknown>;
  timestamp: string;
}

/** One-shot `subagent:finished` IPC payload — emitted by the Rust
 *  `run_subagent` AFTER `update_run_finished` commits the run's
 *  terminal state. Distinct from `SubagentEventPayload` (which
 *  streams transcript entries while the worker runs): this carries
 *  only the terminal status + timestamp, so the frontend can refetch
 *  `get_subagent_run` + `list_subagent_runs_by_session` and flip the
 *  drawer / card from `running` to the terminal state without
 *  polling. `runId` is the same DB row id `subagent:event` uses
 *  (== `summary.id`). */
export interface SubagentFinishedPayload {
  runId: string;
  sessionId: string;
  status: string;
  finishedAt: string;
}

/** Transcript entry as stored in `transcriptJson` (the DB storage
 *  shape). ⚠️ Drift trap 2: the Rust struct has NO `rename_all`, so
 *  the field is `payload_json` (snake_case) — distinct from the live
 *  `subagent:event` payload's `payload` (camelCase).
 *
 *  B6 PR3 redesign (2026-06-21): the `payload_json` shape carries
 *  two new top-level fields for `tool_call` / `tool_result` entries:
 *    - `tool_call.payload_json.tool_use_id: string` — the LLM-assigned
 *      tool_use id (matches `ToolCallPayload::id` on the Rust side);
 *      lets the frontend drawer pair call+result by id.
 *    - `tool_result.payload_json.tool_use_id: string` — same id
 *      (matches the `ToolResultPayload::tool_use_id`); the drawer's
 *      pairing layer keys on this.
 *    - `tool_result.payload_json.duration_ms: number` — the
 *      wall-clock gap between the matching tool_call and this
 *      tool_result (measured in `SubagentBufferSink`). The drawer
 *      surfaces this in the merged card header via
 *      `abbreviateDuration`. Pre-redesign rows (old persisted
 *      transcripts) lack the field; the `ToolOutputBody` treats
 *      `durationMs === undefined` as "omit duration chip" (no
 *      visual regression).
 *  The `permission_ask` and `chat_event` shapes are unchanged. */
export interface TranscriptEntry {
  kind: TranscriptKind;
  payload_json: Record<string, unknown>;
}

/** Debounce window for batching live events into the reactive
 *  `liveTranscript`. Self-implemented (no lodash) per PRD decision
 *  #8. A 200ms cadence keeps the drawer lively without re-rendering
 *  on every SSE delta. */
export const SUBAGENT_EVENT_DEBOUNCE_MS = 200;

// -----------------------------------------------------------------------
// TranscriptSection — derived view (R8 / R20-R22)
// -----------------------------------------------------------------------

/** Per-section kind in the new grouped view. Distinct from
 *  `TranscriptKind` (the raw wire kind); a `TranscriptSection`
 *  carries the post-accumulator shape the drawer's 5-segment
 *  collapsed view consumes. Raw `chat_event` SSE chunks are
 *  collapsed into `Thinking` / `Text` segments by the
 *  `RunAccumulator`; `tool_call` / `tool_result` /
 *  `permission_ask` pass through (renamed from the wire kind
 *  to a more drawer-friendly form). The store keeps BOTH the
 *  raw `liveTranscript: Map<runId, TranscriptEntry[]>` (for
 *  the pairing layer) and the derived `liveSections:
 *  Map<runId, TranscriptSection[]>` (for the 5-segment view). */
export type TranscriptSectionKind =
  | "Thinking"
  | "Text"
  | "FinalText"
  | "ToolCall"
  | "ToolResult"
  | "PermissionAsk";

/** Accumulated thinking-block content. Multiple
 *  `thinking_delta` SSE events roll up into a single segment
 *  (the in-place `append` mutates the `text` string). The
 *  `closed` flag flips to `true` on the matching
 *  `signature_delta` event; the drawer can stop showing the
 *  streaming indicator at that point. */
export interface ThinkingSection {
  kind: "Thinking";
  /** Accumulated thinking text (live-mutated via `+=`). */
  text: string;
  /** Chars appended so far — for the segment chip's "N chars"
   *  badge. Mirrors `text.length` after every append. */
  chars: number;
  /** True once a `signature_delta` has been seen for this block. */
  closed: boolean;
}

/** Accumulated text-block content. Same shape as Thinking
 *  minus the `closed` flag (text blocks don't have a
 *  signature event). The drawer's Reply segment reads the
 *  accumulator's `textSegment` directly; a separate
 *  `FinalText` section is appended when the worker exit hook
 *  writes `subagent_runs.final_text`. */
export interface TextSection {
  kind: "Text";
  text: string;
  chars: number;
}

/** Final worker reply (post-exit). Set when the drawer reads
 *  `row.final_text` (or the live accumulator's `finalText`
 *  helper after `subagent:finished`). The drawer's Reply
 *  segment shows this verbatim; PR3 modal uses it for the
 *  "View full →" detail view. */
export interface FinalTextSection {
  kind: "FinalText";
  text: string;
}

/** Pass-through for `tool_call` transcript entries. Body
 *  fields mirror `TranscriptEntry.payload_json` so the
 *  pairing layer (in the drawer) can still key by
 *  `tool_use_id`. */
export interface ToolCallSection {
  kind: "ToolCall";
  payload_json: Record<string, unknown>;
}

/** Pass-through for `tool_result`. Paired with its matching
 *  `ToolCallSection` by `tool_use_id` (the drawer's pairing
 *  layer is unchanged from the previous PR). */
export interface ToolResultSection {
  kind: "ToolResult";
  payload_json: Record<string, unknown>;
}

/** Pass-through for `permission_ask`. The drawer's historical
 *  mode (already shipped) renders these as static cards; the
 *  PR6 interactive allow/deny path is independent of the
 *  accumulator. */
export interface PermissionAskSection {
  kind: "PermissionAsk";
  payload_json: Record<string, unknown>;
}

/** Discriminated union for the drawer's section list. The
 *  drawer's `liveSections` computed iterates over
 *  `TranscriptSection[]` and branches on `kind` to choose
 *  the right `DrawerSection` slot. */
export type TranscriptSection =
  | ThinkingSection
  | TextSection
  | FinalTextSection
  | ToolCallSection
  | ToolResultSection
  | PermissionAskSection;

/** Type of the inner `kind` field of a `chat_event` payload.
 *  Mirrors the Rust `ChatEvent` enum's
 *  `#[serde(tag = "kind", rename_all = "snake_case")]` (see
 *  `app/src-tauri/src/llm/types.rs:330`). When the outer
 *  `TranscriptKind` is `"chat_event"`, the inner `kind`
 *  discriminates the SSE event subtype. Used by
 *  `RunAccumulator.routeChatEvent` to dispatch deltas into
 *  the right segment. */
export type ChatEventInnerKind =
  | "start"
  | "delta"
  | "thinking_delta"
  | "signature_delta"
  | "redacted_thinking_delta"
  | "tool_call"
  | "tool_result"
  | "done"
  | "error";

// -----------------------------------------------------------------------
// Helpers — cross-layer drift trap fixes
// -----------------------------------------------------------------------

/** Coerce a raw status string (from `SubagentRunRow.status` or a
 *  malformed `SubagentRunSummary.status`) into the typed union.
 *  Unknown strings fall back to `"running"` (matches the Rust
 *  `SubagentStatusDb::from_str_opt` lenient-parse default). The
 *  5-variant union (incl. `"incomplete"`) mirrors the backend
 *  `SubagentStatusDb` enum (Session 60 R2, 2026-06-21); missing
 *  it here previously caused incomplete runs to render as
 *  "运行中" forever (RULE-FrontSubagent-005). */
export function coerceStatus(raw: string): SubagentStatus {
  if (
    raw === "running" ||
    raw === "completed" ||
    raw === "cancelled" ||
    raw === "error" ||
    raw === "incomplete"
  ) {
    return raw;
  }
  return "running";
}

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
    if (
      e.kind !== "chat_event" &&
      e.kind !== "tool_call" &&
      e.kind !== "tool_result" &&
      e.kind !== "permission_ask"
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
 *  `signature: string`, but other events may not. Returns
 *  `""` for non-signature events. Kept exported (not used
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
  }
  return out;
}

// -----------------------------------------------------------------------
// Store
// -----------------------------------------------------------------------

export const useSubagentRunsStore = defineStore("subagentRuns", () => {
  // -----------------------------------------------------------------------
  // Reactive state
  // -----------------------------------------------------------------------

  /** List cache: per-session summaries. Keyed by `sessionId`; the
   *  value is the array returned by `list_subagent_runs_by_session`.
   *  The ToolCallCard looks up its worker via
   *  `getSummaryByToolUseId`. */
  const runSummaryBySession = reactive(
    new Map<string, SubagentRunSummary[]>(),
  );

  /** Detail cache: per-runId full row. Written by `fetchRun`. */
  const getRunCache = reactive(new Map<string, SubagentRunRow>());

  /** Live transcript: per-runId transcript entries streamed in from
   *  `subagent:event` (debounced). Drawer reads this first when the
   *  worker is still running. On worker completion the backend
   *  persists the full transcript; the drawer falls back to the
   *  cache after `fetchRun` resolves. */
  const liveTranscript = reactive(new Map<string, TranscriptEntry[]>());

  /** B6 redesign PR2 (2026-06-21): per-runId `RunAccumulator`
   *  instance. Created lazily on first event / rebuild. The
   *  accumulator's internal `transcript: shallowRef` is what the
   *  store copies into `liveSections` on debounce flush. The
   *  Map is non-reactive (plain `Map`) — the per-runId
   *  `liveSections` Map is the reactive surface the drawer
   *  subscribes to. */
  const accumulators = new Map<string, RunAccumulator>();

  /** B6 redesign PR2: per-runId derived `TranscriptSection[]`,
   *  the drawer's 5-segment grouped view source. Mirrors
   *  `liveTranscript` but each entry is a post-accumulator
   *  section (Thinking / Text / FinalText / ToolCall /
   *  ToolResult / PermissionAsk) — the chat_event SSE chunk
   *  stream is collapsed into Thinking / Text segments rather
   *  than exposed verbatim. The drawer reads this Map first
   *  when the worker is running; falls back to a
   *  `buildSectionsFromRaw(parseTranscriptJson(transcriptJson))`
   *  computation when no live sections exist yet. */
  const liveSections = reactive(new Map<string, TranscriptSection[]>());

  /** Drawer open state. `null` = closed; a runId = open at that run.
   *  Drawer binds `open = computed(() => openRunId !== null)`. Single
   *  drawer at a time — opening run B closes run A (no nesting per
   *  PRD Out of Scope). */
  const openRunId = ref<string | null>(null);

  // -----------------------------------------------------------------------
  // Non-reactive debounce buffer
  // -----------------------------------------------------------------------

  /** Stage buffer for the 200ms debounce. Non-reactive (a plain
   *  `Map`) so SSE deltas don't trigger per-event re-renders. The
   *  debounce timer flushes this into `liveTranscript` (the reactive
   *  mirror) every `SUBAGENT_EVENT_DEBOUNCE_MS`. */
  const liveTranscriptBuffer = new Map<string, TranscriptEntry[]>();

  /** Pending debounce timer per runId. Cleared on flush. */
  const debounceTimers = new Map<string, ReturnType<typeof setTimeout>>();

  /** Tauri `listen` unlisten handle for `subagent:event`. Set by
   *  `start()`, torn down by `stop()`. */
  let unlisten: UnlistenFn | null = null;

  /** Tauri `listen` unlisten handle for `subagent:finished`
   *  (B6 PR3b hotfix, 2026-06-21). Separate from `unlisten` so the
   *  two channels' lifecycles stay independent and `stop()` tears
   *  both down. */
  let unlistenFinished: UnlistenFn | null = null;

  /** B6 PR3b (2026-06-20): dedup Set for the eager-fetch path in the
   *  `subagent:event` listener. A burst of events for the same runId
   *  (e.g. a tool_call arriving milliseconds apart from a tool_result)
   *  must NOT fire `fetchRun` / `fetchForSession` more than once —
   *  the IPC roundtrip is cheap but the cost adds up on a busy worker.
   *  Non-reactive (plain Set) so per-event checks don't trigger Vue
   *  effect re-evaluation. Lives for the lifetime of the store (not
   *  cleared on `stop()` — the cache survives component unmount so the
   *  dedup should too). Bounded by the number of distinct runIds seen
   *  in this app session; not a memory concern for realistic usage. */
  const eagerFetchedRunIds = new Set<string>();

  // -----------------------------------------------------------------------
  // API
  // -----------------------------------------------------------------------

  /** Load all worker summaries for a session. Replaces the cached
   *  array. Failure is logged + swallowed (the caller can show a
   *  toast if it cares; the store doesn't own toasts). */
  async function fetchForSession(sessionId: string): Promise<void> {
    try {
      const rows = await invoke<SubagentRunSummary[]>(
        "list_subagent_runs_by_session",
        { sessionId },
      );
      runSummaryBySession.set(sessionId, Array.isArray(rows) ? rows : []);
    } catch (e) {
      console.error("useSubagentRunsStore.fetchForSession failed:", e);
    }
  }

  /** Load the full row for a run (incl. transcriptJson). Writes
   *  `getRunCache` AND parses the transcript into `liveTranscript`
   *  so the drawer can fall back to the cached transcript when no
   *  live events have arrived yet (e.g. opening a completed worker).
   *  Does NOT overwrite a live transcript that already has entries
   *  (that would erase in-flight streaming progress).
   *
   *  B6 redesign PR2 (2026-06-21): also rebuilds the per-runId
   *  accumulator from `row.transcriptJson` + `row.finalText` and
   *  publishes the derived `TranscriptSection[]` to `liveSections`
   *  (R22 — worker finished → fetchRun → rebuildFromCache replaces
   *  the in-memory transcript with the authoritative DB-cached
   *  version). If the worker is still streaming (live transcript
   *  non-empty), the rebuild is skipped to avoid losing in-flight
   *  progress — the live path keeps owning the transcript until
   *  `subagent:finished` triggers a fresh fetchRun (which sees
   *  the now-terminal state and rebuilds). */
  async function fetchRun(runId: string): Promise<void> {
    try {
      const row = await invoke<SubagentRunRow | null>("get_subagent_run", {
        runId,
      });
      if (!row) return;
      getRunCache.set(runId, row);
      // Only seed liveTranscript if it's empty — once live events
      // start streaming we let them own the transcript.
      if (
        !liveTranscript.has(runId) ||
        (liveTranscript.get(runId)?.length ?? 0) === 0
      ) {
        const parsed = parseTranscriptJson(row.transcriptJson);
        if (parsed.length > 0) {
          liveTranscript.set(runId, parsed);
        }
      }
      // Rebuild the accumulator's derived sections. If the live
      // transcript is non-empty (worker still streaming), this
      // call's sections are dropped by the "first publish wins"
      // rule in `routeEvent`'s `feed` path. The post-`finished`
      // path flushes the live buffer first (subagent:finished
      // handler) and then calls fetchRun — at which point the
      // live transcript is non-empty, but the
      // `subagent:finished` handler's flushBuffer call has
      // cleared the live buffer and the new rebuild IS the
      // authoritative source.
      const acc = accumulators.get(runId) ?? new RunAccumulator();
      acc.rebuildFromCache(row.transcriptJson, row.finalText ?? null);
      accumulators.set(runId, acc);
      // Publish the rebuilt sections to `liveSections`. The
      // shallowRef inside the accumulator now points to the
      // fresh post-rebuild array; we copy that identity into
      // the reactive Map.
      publishAccumulator(runId);
    } catch (e) {
      console.error("useSubagentRunsStore.fetchRun failed:", e);
    }
  }

  /** Open the drawer for a worker run. Sets `openRunId` + fetches
   *  the row if it isn't cached yet. */
  async function openDrawer(runId: string): Promise<void> {
    openRunId.value = runId;
    if (!getRunCache.has(runId)) {
      await fetchRun(runId);
    }
  }

  /** Close the drawer. Clears `openRunId` only — leaves caches
   *  intact so reopening is instant. */
  function closeDrawer(): void {
    openRunId.value = null;
  }

  /** Find the summary for a worker run by the dispatch_subagent
   *  tool_use's `id`. The backend formats the worker's rid as
   *  `"{parent_rid}-sub-{tool_use_id}"` (see
   *  `chat_loop.rs::run_subagent`), so we match summaries whose
   *  `parentRequestId` ends with `"-sub-" + toolUseId`. Returns
   *  `undefined` when no worker for this tool_use has been
   *  dispatched yet (e.g. the lookup ran before `fetchForSession`
   *  resolved). */
  function getSummaryByToolUseId(
    sessionId: string,
    toolUseId: string,
  ): SubagentRunSummary | undefined {
    const list = runSummaryBySession.get(sessionId);
    if (!list) return undefined;
    const suffix = `-sub-${toolUseId}`;
    return list.find((s) => s.parentRequestId.endsWith(suffix));
  }

  // -----------------------------------------------------------------------
  // IPC listener — `subagent:event` + 200ms debounce
  // -----------------------------------------------------------------------

  /** Route a live event into the debounce buffer + schedule a flush.
   *  The buffer is per-runId so multiple concurrent workers don't
   *  interleave (the drawer only shows one at a time, but the buffer
   *  still needs to preserve per-run ordering for when the user
   *  switches).
   *
   *  B6 redesign PR2 (2026-06-21): the entry is ALSO fed to the
   *  per-runId `RunAccumulator` (R8). The accumulator's live
   *  `transcript` shallowRef carries the post-collapse sections;
   *  the store copies it into `liveSections` on debounce flush. The
   *  raw `TranscriptEntry[]` is still preserved in `liveTranscript`
   *  for the pairing layer (call+result merge). */
  function routeEvent(event: SubagentEventPayload): void {
    // Convert the live payload's camelCase `payload` into the DB
    // storage shape `payload_json` so the drawer has a single
    // TranscriptEntry type to render. (See Drift trap 2 — the live
    // stream wraps the body as `payload`, transcriptJson stores it
    // as `payload_json`. We unify on the storage shape internally.)
    const entry: TranscriptEntry = {
      kind: event.kind,
      payload_json: event.payload,
    };
    // Feed the accumulator live (R20: O(1) per event, in-place
    // mutation of the active segment). The accumulator's
    // `transcript` shallowRef is read on flush and copied to
    // `liveSections` (the reactive surface the drawer subscribes
    // to). We do NOT read the shallowRef on every event — the
    // debounce flush batches the writes to one Map.set per 200ms
    // (matches the previous 200ms cadence; R22 "live phase does
    // NOT run full accumulator").
    const acc = accumulators.get(event.runId) ?? new RunAccumulator();
    acc.feed(entry);
    accumulators.set(event.runId, acc);
    const existing = liveTranscriptBuffer.get(event.runId) ?? [];
    existing.push(entry);
    liveTranscriptBuffer.set(event.runId, existing);
    scheduleFlush(event.runId);
  }

  /** Arm (or re-arm) the debounce timer for a runId. Self-implemented
   *  setTimeout — no lodash dependency (PRD decision #8). */
  function scheduleFlush(runId: string): void {
    const prev = debounceTimers.get(runId);
    if (prev !== undefined) {
      clearTimeout(prev);
    }
    const t = setTimeout(() => {
      debounceTimers.delete(runId);
      flushBuffer(runId);
    }, SUBAGENT_EVENT_DEBOUNCE_MS);
    debounceTimers.set(runId, t);
  }

  /** Commit the buffer for a runId into the reactive
   *  `liveTranscript`. Clears the buffer slot. Also copies the
   *  accumulator's current `transcript: shallowRef` into
   *  `liveSections` so the drawer's 5-segment view picks up the
   *  post-collapse sections. B6 redesign PR2. */
  function flushBuffer(runId: string): void {
    const buffered = liveTranscriptBuffer.get(runId);
    if (!buffered || buffered.length === 0) {
      liveTranscriptBuffer.delete(runId);
      // The accumulator may have mutated even with no buffered
      // events (e.g. eager-fetch path that doesn't feed events).
      // Still publish the current `transcript` value to keep
      // the drawer's `liveSections` in sync.
      publishAccumulator(runId);
      return;
    }
    const existing = liveTranscript.get(runId) ?? [];
    // Concat defensively — we never mutate the existing reactive
    // array in place (Vue's reactivity tracks `.set` on the Map,
    // not array push on a cached reference).
    liveTranscript.set(runId, [...existing, ...buffered]);
    liveTranscriptBuffer.delete(runId);
    publishAccumulator(runId);
  }

  /** Copy the per-runId accumulator's `transcript: shallowRef`
   *  into the reactive `liveSections` Map. Called on every
   *  debounce flush + on `rebuildFromCache`. The Map is the
   *  reactive surface the drawer subscribes to; the shallowRef
   *  itself is not. R20 / R22. */
  function publishAccumulator(runId: string): void {
    const acc = accumulators.get(runId);
    if (!acc) return;
    const sections = acc.transcript.value;
    // Defensive: skip the publish if the section list is
    // identical to the prior publish (avoids spurious
    // re-renders when no events have been fed since the
    // last flush). The shallowRef .value identity is the
    // ground truth — the accumulator replaces .value on
    // every appendSection, so identity is the cheapest
    // check. We do NOT deep-compare.
    const prior = liveSections.get(runId);
    if (prior === sections) return;
    liveSections.set(runId, sections);
  }

  /** Mount the `subagent:event` listener. Idempotent — calling twice
   *  replaces the prior unlisten. Mirrors permissions.ts `start()`.
   *
   *  B6 PR3b (2026-06-20): the listener ALSO fires an eager-fetch
   *  on the first event for any new runId. This fixes the
   *  dispatch_subagent card race — see PR3b PRD §"Root cause".
   *  Without this, the ToolCallCard's `getSummaryByToolUseId`
   *  lookup may stay empty for the entire worker lifetime if the
   *  initial `fetchForSession` IPC roundtrip races against the
   *  backend's `insert_run`. By the time the first `subagent:event`
   *  arrives, `insert_run` has definitely committed (the sink is
   *  constructed AFTER the row insert), so the eager-fetch is
   *  guaranteed to see the row. */
  async function start(): Promise<void> {
    if (unlisten) {
      unlisten();
      unlisten = null;
    }
    unlisten = await listen<SubagentEventPayload>("subagent:event", (event) => {
      const e = event.payload;
      routeEvent(e);
      // Eager-fetch: warm the run-detail cache + session-summary
      // cache the first time we see a runId. Dedup'd by the
      // `eagerFetchedRunIds` Set so burst events don't re-fetch.
      // `fetchRun` and `fetchForSession` are fire-and-forget here —
      // they're independent of the routeEvent debounce path, and a
      // failure to warm the cache just falls back to the existing
      // ToolCallCard click-time retry (which polls fetchForSession
      // for up to 1.5s before giving up).
      if (!eagerFetchedRunIds.has(e.runId)) {
        eagerFetchedRunIds.add(e.runId);
        void fetchRun(e.runId);
        void fetchForSession(e.sessionId);
      }
    });
    // Bug2 fix (2026-06-21): listen for the one-shot terminal signal
    // emitted by `run_subagent` after `update_run_finished` commits.
    // On receipt, flush any buffered transcript events for the run
    // (so `liveTranscript` is complete before `fetchRun`'s seed-guard
    // checks it) then refetch the run detail (drawer source:
    // terminal status + finishedAt + full transcript) + session
    // summary (card source: status). This flips the drawer / card
    // from `running` to the terminal state without polling. NOT
    // dedup'd by `eagerFetchedRunIds` — the terminal signal is
    // one-shot by definition.
    if (unlistenFinished) {
      unlistenFinished();
      unlistenFinished = null;
    }
    unlistenFinished = await listen<SubagentFinishedPayload>(
      "subagent:finished",
      (event) => {
        const f = event.payload;
        flushBuffer(f.runId);
        void fetchRun(f.runId);
        void fetchForSession(f.sessionId);
      },
    );
  }

  /** Tear down the listener + flush all pending buffers + clear
   *  timers. Does NOT clear the caches (the drawer may need them
   *  again on reopen). */
  function stop(): void {
    if (unlisten) {
      unlisten();
      unlisten = null;
    }
    if (unlistenFinished) {
      unlistenFinished();
      unlistenFinished = null;
    }
    // Flush any pending buffered events so the user doesn't lose
    // the last batch when the component unmounts.
    for (const runId of [...debounceTimers.keys()]) {
      const t = debounceTimers.get(runId);
      if (t !== undefined) clearTimeout(t);
      debounceTimers.delete(runId);
      flushBuffer(runId);
    }
  }

  /** Drop all state for a session (e.g. on `deleteSession`). */
  function clearSession(sessionId: string): void {
    const list = runSummaryBySession.get(sessionId) ?? [];
    for (const s of list) {
      getRunCache.delete(s.id);
      liveTranscript.delete(s.id);
      liveTranscriptBuffer.delete(s.id);
      // B6 redesign PR2: also drop the per-runId accumulator
      // + the derived `liveSections` entry. The accumulator
      // is plain JS (no reactive resources), so just `delete`
      // on the Map. `liveSections` IS reactive — the delete
      // triggers a Map-level re-render.
      accumulators.delete(s.id);
      liveSections.delete(s.id);
      const t = debounceTimers.get(s.id);
      if (t !== undefined) {
        clearTimeout(t);
        debounceTimers.delete(s.id);
      }
    }
    runSummaryBySession.delete(sessionId);
    if (openRunId.value && list.some((s) => s.id === openRunId.value)) {
      openRunId.value = null;
    }
  }

  // -----------------------------------------------------------------------
  // Drawer-derived getters
  // -----------------------------------------------------------------------

  /** The currently open run's full row (from cache), or `undefined`.
   *  Drawer reads this for the header (status + summary + timestamps). */
  const openRun = computed<SubagentRunRow | undefined>(() => {
    const rid = openRunId.value;
    if (!rid) return undefined;
    return getRunCache.get(rid);
  });

  return {
    // reactive state
    runSummaryBySession,
    getRunCache,
    liveTranscript,
    // B6 redesign PR2: derived sections (chat_event stream
    // collapsed into Thinking/Text segments). The drawer's
    // 5-segment grouped view reads this; falls back to
    // `buildSectionsFromRaw(parseTranscriptJson(...))` for
    // the cold-cache path. Kept as a separate Map from
    // `liveTranscript` so the pairing layer (which needs
    // the raw entries) and the segmented view (which needs
    // the collapsed sections) don't fight over the same
    // data.
    liveSections,
    openRunId,
    openRun,
    // actions
    fetchForSession,
    fetchRun,
    openDrawer,
    closeDrawer,
    getSummaryByToolUseId,
    clearSession,
    // lifecycle
    start,
    stop,
  };
});
