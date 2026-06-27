// subagentRuns.types.ts — Public type surface for the subagentRuns store.
//
// This file is the "types layer" of the subagentRuns store — the single
// source of truth for every type/interface/const the rest of the app
// imports (other than `useSubagentRunsStore` + `coerceStatus`, which stay
// in `subagentRuns.ts`, and the `RunAccumulator` class + its parsers,
// which live in `runAccumulator.ts`).
//
// Why split (see PRD 06-23-06-23-split-subagent-runs):
//   `subagentRuns.ts` was 1416 lines (Pinia store + ~324 lines of
//   type/interface declarations + the `SUBAGENT_EVENT_DEBOUNCE_MS`
//   const + a ~497-line `RunAccumulator` class with parser helpers).
//   The type declarations are pure compile-time; splitting them out
//   gives a clean "public contract" module mirroring `chat.types.ts`.
//
// Conventions locked (mirrors split-chat-types / chat.types.ts):
//   - MOVE: every `export type/interface` declaration + the
//     `SUBAGENT_EVENT_DEBOUNCE_MS` const that are part of the public API.
//   - KEEP in subagentRuns.ts: `coerceStatus` + `useSubagentRunsStore`.
//   - MOVE to runAccumulator.ts: `RunAccumulator` + `parseTranscriptJson`
//     + the chat_event parser helpers (parseTranscriptJson must follow
//     RunAccumulator — rebuildFromCache depends on it, else a
//     store↔accumulator import cycle).
//   - No behavior change — pure file/import reorganization.
//
// ⚠️ Cross-layer drift traps (see
//    `.trellis/spec/backend/subagent-runs-schema.md`):
//   1. `SubagentRunRow.status` is a raw `string` on the wire but
//      `SubagentRunSummary.status` is a typed `SubagentStatus` union.
//      Coerce via `coerceStatus` (in subagentRuns.ts) for display.
//   2. `TranscriptEntry.payload_json` is snake_case (Rust struct has NO
//      `rename_all`); the live `subagent:event` payload wraps the body as
//      camelCase `payload`. NEVER conflate them.
//   3. The raw `chat_event` payload carries a NESTED `ChatEvent` whose
//      `kind` discriminates the SSE subtype — see `ChatEventInnerKind`
//      below + `RunAccumulator.routeChatEvent` (in runAccumulator.ts).

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
 *  shape).
 *
 *  2026-06-22 (RULE-WorkerAsk-001): added `"permission_ask_resolved"`
 *  for the 5th Rust variant. The entry carries `{ rid, outcome }`
 *  where `outcome ∈ {"allow", "deny", "timeout", "cancel"}`. The
 *  drawer pairs this entry to the matching `permission_ask` entry
 *  by `rid` and surfaces the outcome as a badge on the historical
 *  card. Pre-this-task transcripts (no resolved entries) render the
 *  neutral ask card unchanged (backward compat). */
export type TranscriptKind =
  | "chat_event"
  | "tool_call"
  | "tool_result"
  | "permission_ask"
  | "permission_ask_resolved";

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
  /** 2026-06-22 (RULE-FrontSubagent-004): actual completed turn
   *  count the worker executed before reaching terminal state.
   *  Null on pre-PR2 rows (drawer degrades to wall-clock suffix
   *  for cancelled / incomplete). Cheap single-i64 column so it's
   *  included in the summary projection. */
  turnCount: number | null;
  /** L3b PR1 (2026-06-27): worker's isolated worktree path.
   *  Mirrors `SubagentRunRow.worktreePath`; included in the list
   *  projection so a future ToolCallCard chip can show "branch
   *  kept" without a per-run detail roundtrip. PR4 reads it from
   *  the row projection (drawer detail cache); the summary
   *  surface is for future use. */
  worktreePath: string | null;
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
  // 2026-06-22 (RULE-FrontSubagent-004): actual
  // completed turn count at worker exit. Null on
  // pre-PR2 rows; the drawer's statusDisplay degrades
  // to wall-clock suffix when null. Read by the
  // cancelled / incomplete branches only (completed
  // still uses wall-clock).
  turnCount: number | null;
  // L3b PR1 (2026-06-27): the worker's isolated
  // worktree path (`<app_data_dir>/worktrees/<project_uuid>/worker/<run_id>`),
  // set when the worker ran in isolation AND has
  // preserved changes (the destroy path clears it).
  // `null` for: non-isolated runs (researcher /
  // isolation=false), destroyed runs (sweep / merge /
  // discard). Drives the PR4 Merge / Discard button
  // visible condition (`status === 'completed' &&
  // worktreePath != null`).
  worktreePath: string | null;
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
 *  accumulator.
 *
 *  2026-06-22 (RULE-WorkerAsk-001): carries an optional `outcome`
 *  field populated by the pairing layer when a matching
 *  `PermissionAskResolved` section is found (matched by `rid`).
 *  When present, the historical card renders the outcome badge
 *  (✓ 已允许 / ✗ 已拒绝 / ⏱ 已超时 / ⊘ 已取消); when absent
 *  (no matching resolved entry — old transcript / live-pending),
 *  the card renders the neutral ask-context line. */
export interface PermissionAskSection {
  kind: "PermissionAsk";
  payload_json: Record<string, unknown>;
  /** Resolve outcome surfaced by the pairing layer. One of
   *  `"allow"` / `"deny"` / `"timeout"` / `"cancel"` or
   *  `undefined` (no matching resolved entry — pre-this-task
   *  transcript or live-pending ask). */
  outcome?: PermissionAskOutcome;
}

/** Resolve outcome wire string for a worker's `PermissionAsk`.
 *  Mirrors the Rust `ask_path` worker branch's four-state
 *  outcome (DEBT-locked). */
export type PermissionAskOutcome = "allow" | "deny" | "timeout" | "cancel";

/** Pass-through for `permission_ask_resolved`. Consumed by the
 *  pairing layer (`pairSections`) to attach an `outcome` to the
 *  matching `PermissionAskSection`. Never rendered as a standalone
 *  card (the drawer drops it from the visible list after
 *  pairing). */
export interface PermissionAskResolvedSection {
  kind: "PermissionAskResolved";
  payload_json: Record<string, unknown>;
}

/** Discriminated union for the drawer's section list. The
 *  drawer's `liveSections` computed iterates over
 *  `TranscriptSection[]` and branches on `kind` to choose
 *  the right `DrawerSection` slot.
 *
 *  2026-06-22 (RULE-WorkerAsk-001): added `PermissionAskResolved`
 *  variant — carried through the accumulator so the pairing layer
 *  can attach an `outcome` to the matching `PermissionAsk` card.
 *  The drawer's `DrawerSection(type="tools")` template does NOT
 *  render this section directly; `pairSections` consumes it and
 *  drops it from the visible list after pairing. */
export type TranscriptSection =
  | ThinkingSection
  | TextSection
  | FinalTextSection
  | ToolCallSection
  | ToolResultSection
  | PermissionAskSection
  | PermissionAskResolvedSection;

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
// L3b PR4 (2026-06-27): merge / discard worker branch UI types
// -----------------------------------------------------------------------

/** Discriminated result of `mergeWorker(runId)`.
 *
 *  The backend `merge_worker_run` IPC returns `Result<String,
 *  String>` — the success arm carries a human-readable status
 *  string, the error arm carries a plain-English error
 *  (`"worker run not found"`, `"merge conflict: [...]"`, etc.).
 *  The store discriminates the conflict case (parse the conflict
 *  file list from the error string) so the drawer can render an
 *  inline read-only file list with a "resolve via git CLI" hint.
 *
 *  Conflict string format (from
 *  `app/src-tauri/src/tools/merge_worker.rs::do_merge_blocking`):
 *    `"merge conflict: [<file1>, <file2>, ...]. The worker branch
 *    'worker/<run_id>' and parent branch 'session/<id>' both
 *    modified these files. Resolve manually, then call
 *    merge_worker again (or discard_worker to drop the changes)."`
 *  The store's `parseConflictFiles` regex extracts the bracketed
 *  comma-separated file list. */
export type MergeResult =
  | { kind: "success" }
  | { kind: "conflict"; files: string[] }
  | { kind: "error"; message: string };

/** Discriminated result of `discardWorker(runId)`. The backend
 *  has no conflict path — discard is a destructive delete with
 *  no preconditions beyond `worktree_path IS NOT NULL`, so the
 *  result is either success or a generic error. */
export type DiscardResult =
  | { kind: "success" }
  | { kind: "error"; message: string };

/** Per-run spinner state for the merge / discard buttons. Stored
 *  in a `Map<runId, MergeState>` so multiple drawers (different
 *  runs) don't block each other. `kind` distinguishes which
 *  action is in flight so the drawer can render the spinner on
 *  the right button.
 *
 *  `null` = no action in flight for this run (button enabled). */
export type MergeState = { kind: "merge" | "discard"; loading: true };

/** Conflict file list extracted from a `merge_worker_run` Err
 *  string. Exported as a standalone helper so the unit tests can
 *  exercise the parser directly without going through the store
 *  (the parser is a pure string transform).
 *
 *  Returns `null` when the input doesn't match the conflict
 *  message format (caller should treat the whole string as a
 *  generic error). The bracketed list uses `, ` (comma+space) as
 *  the separator — the backend `conflicts.join(", ")` format. */
export function parseConflictFiles(errStr: string): string[] | null {
  // Match "merge conflict: [<files>]" prefix + the bracketed list.
  // The backend ALWAYS emits this exact prefix on conflict; a
  // generic error (e.g. "worker run not found") doesn't match.
  const m = errStr.match(/^merge conflict: \[([^\]]*)\]/);
  if (!m) return null;
  const inner = m[1].trim();
  if (inner.length === 0) return [];
  // Backend joins with ", "; split defensively on the same
  // separator (a file path with a literal ", " is vanishingly
  // rare — git paths with spaces would still split, but the user
  // reading the file list can still recognize them).
  return inner.split(", ").map((s) => s.trim()).filter((s) => s.length > 0);
}
