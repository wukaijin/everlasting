// E2 (harness trace pipeline, 2026-07-14) — frontend types for the
// per-turn trace viewer.
//
// The trace viewer is a unified surface for **live** (in-flight
// ChatEvent payloads) and **回看** (turn_trace table rows) trace
// data. The two paths converge on the `TurnTrace` type defined
// here so the render layer can be a single code path.
//
// Wire shapes (cross-layer drift traps):
//
//   1. `TurnTraceRow` mirrors `db::trace::TurnTraceRow` (Rust,
//      `#[serde(rename_all = "camelCase")]`). The IPC field names
//      are `id` / `sessionId` / `seq` / `tokenUsageJson` /
//      `compactionJson` / `loopHintJson` / `breadcrumbJson` /
//      `createdAt`. The `*_json` columns are RAW JSON strings
//      — we parse them on load into the typed sub-objects below.
//
//   2. The 3 new `ChatEvent` variants (`ContextCompacted` /
//      `LoopHint` / `WorkflowBreadcrumb`) are emitted on the
//      `chat-event` channel with `#[serde(tag = "kind",
//      rename_all = "snake_case")]` — wire names `context_compacted`
//      / `loop_hint` / `workflow_breadcrumb`, with snake_case
//      fields (`tokens_before` / `tokens_after` / `dropped_count` /
//      `degradation` / `hit_count` / `verdict_kind` / `task_slug` /
//      `status` / `breadcrumb_text`). The streamController normalizes
//      these into the camelCase typed shapes below (see
//      `streamController.ts` handleChatEvent "context_compacted" /
//      "loop_hint" / "workflow_breadcrumb" cases).
//
// The 4 sub-objects are all `optional` on `TurnTrace` — the
// backend writes each dimension via a separate UPSERT
// (`upsert_turn_trace_token` / `_compaction` / `_loop_hint` /
// `_breadcrumb`), and they may arrive in any order (or not at
// all for a turn that didn't trigger that dimension).
//
// Cross-layer invariant: when a `ChatEvent::*` arm fires AND the
// matching UPSERT lands, the same `TurnTrace` slot ends up with
// the same `compaction` / `loopHint` / `breadcrumb` value
// (modulo the JSON-string round-trip for the storage path). The
// store's `applyEvent` is the single upsert point on the live
// path; `loadHistory` parses the DB rows for the 回看 path. Both
// paths write into the same `TurnTrace` shape, so the renderer
// is path-agnostic.

// ---------------------------------------------------------------------------
// 4 typed sub-objects (per-dimension trace payloads)
// ---------------------------------------------------------------------------

/** C3 context-compaction snapshot. Mirrors Rust
 *  `agent::trace::record_compaction`'s JSON payload. */
export interface CompactionPayload {
  tokens_before: number;
  tokens_after: number;
  dropped_count: number;
  /** `"none"` / `"no_candidates"` / `"still_over"`. The
   *  `still_over` value triggers the red "audit-item--critical"
   *  border on the TurnCard (per design — still_over = the
   *  turn is about to abort because compaction alone didn't
   *  bring the context under the threshold). */
  degradation: "none" | "no_candidates" | "still_over" | string;
}

/** C2 loop-detection soft hint. Mirrors Rust
 *  `agent::trace::record_loop_hint`'s JSON payload. The ≥3
 *  active-intervention path writes `loop_intervention` audit
 *  rows separately (NOT a `loop_hint`) — this only covers the
 *  pre-intervention 1-2 consecutive hits. */
export interface LoopHintPayload {
  hit_count: number;
  /** `"soft"` for `LoopVerdict::SoftLoop`, `"hard"` for
   *  `LoopVerdict::HardLoop`. Hard is a stronger signal but
   *  still below the intervention threshold (3+). */
  verdict_kind: "soft" | "hard" | string;
}

/** Workflow task breadcrumb snapshot. Mirrors Rust
 *  `agent::trace::record_breadcrumb`'s JSON payload. `task_slug`
 *  / `status` are `null` when there is no active workflow task
 *  (the bootstrap breadcrumb branch). */
export interface BreadcrumbPayload {
  task_slug: string | null;
  status: string | null;
  /** Full `<workflow-task-meta>` text block the agent loop
   *  injected into `messages[0]` this turn. The card renders
   *  this verbatim (or a single-line preview — implementation
   *  detail). */
  breadcrumb_text: string;
}

/** Per-turn token usage. Mirrors the Rust `TokenUsage` struct
 *  (5-field shape, `#[serde(rename_all = "snake_case")]` is NOT
 *  applied so the field names stay snake_case on the wire —
 *  same convention as `subagent_runs.token_usage_json`). */
export interface TokenUsagePayload {
  input_tokens: number;
  output_tokens: number;
  cache_creation_input_tokens: number;
  cache_read_input_tokens: number;
  /** Cross-provider-normalized "total input for this turn".
   *  Anthropic: input + cache_creation + cache_read; OpenAI:
   *  prompt_tokens. `0` for legacy rows that pre-date the
   *  2026-06-26 normalized-field fix. */
  context_input_tokens: number;
}

// ---------------------------------------------------------------------------
// IPC row type — mirrors `db::trace::TurnTraceRow` (camelCase)
// ---------------------------------------------------------------------------

/** Raw `turn_trace` row returned by the `list_turn_traces` IPC.
 *  The `*_json` fields are RAW JSON strings (or `null` if that
 *  dimension was never written for this turn). The store
 *  parses them on load via `parseTurnTraceRow`. */
export interface TurnTraceRow {
  id: number;
  sessionId: string;
  seq: number;
  tokenUsageJson: string | null;
  compactionJson: string | null;
  loopHintJson: string | null;
  breadcrumbJson: string | null;
  /** C7 (2026-08-14): cl100k estimate of the serialized `tools[]`
   *  array for this turn. Mirrors Rust `TurnTraceRow.tools_token`
   *  (`#[serde(rename_all = "camelCase")]` → `toolsToken`). `null`
   *  for rows written before the column existed or turns that
   *  skipped the estimate. NOT part of the cache-rate — a
   *  separately-measured slice already inside
   *  `context_input_tokens` (see design §R1). */
  toolsToken: number | null;
  /** memory-block-governance WP1 (2026-08-15): cl100k estimate of
   *  the memory instruction blocks injected this request (banner +
   *  wrappers + layer bodies). Mirrors Rust
   *  `TurnTraceRow.memory_token` → `memoryToken`. Same slice
   *  semantics as `toolsToken`; a per-request constant across the
   *  request's turn rows. `null` for pre-column rows and worker
   *  turns (worker injection lives in subagent/prompt.rs). */
  memoryToken: number | null;
  /** B1 (2026-08-16) image-multimodal R6: sum of the request's
   *  image-block token estimates — current-turn pastes PLUS rebuilt
   *  history images (history images are re-sent every request, so
   *  counting only new images would systematically understate).
   *  Per-image estimate is `(w×h)/750` computed at attach time.
   *  Mirrors Rust `TurnTraceRow.images_token` → `imagesToken`.
   *  `null` for pre-column rows; `0` for image-less turns. */
  imagesToken: number | null;
  createdAt: string;
}

// ---------------------------------------------------------------------------
// Unified in-memory shape (single source of truth for live + 回看)
// ---------------------------------------------------------------------------

/** The unified per-turn trace record consumed by the renderer.
 *  `currentSessionTraces: Map<seq, TurnTrace>` keyed by the
 *  per-session turn seq (the same seq that `messages.seq` uses
 *  — the trace viewer's timeline axis is `seq ASC`). */
export interface TurnTrace {
  id: number;
  sessionId: string;
  seq: number;
  createdAt: string;
  /** `undefined` = never written for this turn (no `Done` event
   *  yet for live, or the DB row's `token_usage_json` is null
   *  for 回看). The card renders this column as "—" when
   *  absent. */
  tokenUsage?: TokenUsagePayload;
  compaction?: CompactionPayload;
  loopHint?: LoopHintPayload;
  breadcrumb?: BreadcrumbPayload;
  /** C7 (2026-08-14): per-turn estimated token cost of the
   *  serialized `tools[]` array (cl100k of the post-filter
   *  ToolDef JSON). `undefined` when the backend wrote no value
   *  (pre-column rows / skipped estimate). The card renders it
   *  as a `tools` legend cell plus its share of context_input. */
  toolsToken?: number;
  /** memory-block-governance WP1 (2026-08-15): cl100k estimate of
   *  the memory instruction blocks injected this request. Same
   *  render treatment as `toolsToken` (`mem` legend cell +
   *  share-of-context tooltip); per-request constant. */
  memoryToken?: number;
  /** B1 (2026-08-16) image-multimodal R6: sum of the request's
   *  image-block token estimates (current-turn + rebuilt history;
   *  `(w×h)/750` per image). Render treatment mirrors the tools /
   *  memory cells (`img` legend cell + share-of-context tooltip),
   *  gated on `> 0` so image-less turns don't render a noise cell. */
  imagesToken?: number;
  /** Audit events whose `turnSeq === this.seq` (populated only
   *  on the 回看 path — the live path doesn't carry audit
   *  events; the audit row store handles those). The card
   *  renders `tool_executed` rows as a per-turn tool-call
   *  sub-list. */
  auditEvents?: import("../utils/audit").AuditEventRow[];
}

// ---------------------------------------------------------------------------
// 3 new ChatEvent typed payloads (live path — snake_case wire)
// ---------------------------------------------------------------------------

/** Mirror of Rust `ChatEvent::ContextCompacted` payload. The
 *  streamController's `case "context_compacted"` arm normalizes
 *  this into `TurnTrace.compaction` (camelCase typed). */
export interface ContextCompactedEvent {
  kind: "context_compacted";
  request_id: string;
  seq: number;
  tokens_before: number;
  tokens_after: number;
  dropped_count: number;
  degradation: string;
}

/** Mirror of Rust `ChatEvent::LoopHint` payload. */
export interface LoopHintEvent {
  kind: "loop_hint";
  request_id: string;
  seq: number;
  hit_count: number;
  verdict_kind: string;
}

/** Mirror of Rust `ChatEvent::WorkflowBreadcrumb` payload. */
export interface WorkflowBreadcrumbEvent {
  kind: "workflow_breadcrumb";
  request_id: string;
  seq: number;
  task_slug: string | null;
  status: string | null;
  breadcrumb_text: string;
}

// ---------------------------------------------------------------------------
// Parsers (turn_trace row → TurnTrace)
// ---------------------------------------------------------------------------

/** Parse a `turn_trace` row's `*_json` fields into typed
 *  sub-objects. Tolerant: malformed JSON / non-object / missing
 *  fields fall back to the row being present but the sub-object
 *  absent (so the card renders the column as "—" instead of
 *  crashing). */
export function parseTurnTraceRow(row: TurnTraceRow): TurnTrace {
  const out: TurnTrace = {
    id: row.id,
    sessionId: row.sessionId,
    seq: row.seq,
    createdAt: row.createdAt,
  };
  if (row.tokenUsageJson) {
    out.tokenUsage = parseTokenUsage(row.tokenUsageJson);
  }
  if (row.compactionJson) {
    out.compaction = parseJsonField<CompactionPayload>(
      row.compactionJson,
    );
  }
  if (row.loopHintJson) {
    out.loopHint = parseJsonField<LoopHintPayload>(row.loopHintJson);
  }
  if (row.breadcrumbJson) {
    out.breadcrumb = parseJsonField<BreadcrumbPayload>(
      row.breadcrumbJson,
    );
  }
  if (row.toolsToken != null) {
    out.toolsToken = row.toolsToken;
  }
  if (row.memoryToken != null) {
    out.memoryToken = row.memoryToken;
  }
  if (row.imagesToken != null) {
    out.imagesToken = row.imagesToken;
  }
  return out;
}

function parseTokenUsage(json: string): TokenUsagePayload | undefined {
  const o = parseJsonField<Record<string, unknown>>(json);
  if (!o) return undefined;
  const num = (k: string): number => {
    const v = o[k];
    return typeof v === "number" && Number.isFinite(v) ? v : 0;
  };
  return {
    input_tokens: num("input_tokens"),
    output_tokens: num("output_tokens"),
    cache_creation_input_tokens: num("cache_creation_input_tokens"),
    cache_read_input_tokens: num("cache_read_input_tokens"),
    context_input_tokens: num("context_input_tokens"),
  };
}

function parseJsonField<T>(json: string): T | undefined {
  try {
    const parsed: unknown = JSON.parse(json);
    if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
      return undefined;
    }
    return parsed as T;
  } catch {
    return undefined;
  }
}
