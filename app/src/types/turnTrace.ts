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
  /** 08-18-llm-context-compaction PR2:压缩路径
   *  (`"none"` / `"summary"` / `"mechanical"`)。旧回看行(PRPC2
   *  之前)没有该字段 —— 消费侧须按 `"none"` 兜底。 */
  method?: "none" | "summary" | "mechanical" | string;
  /** PR2:摘要 LLM 调用的 usage(仅 method=summary 的行有;live
   *  wire 不带,只有 DB 回看路径的 compaction_json 里有)。 */
  summary_usage?: TokenUsagePayload;
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
  /** 08-20-worker-turn-trace-persist: run 维度 —— `''` = 主 loop 行,
   *  worker 行 = `subagent_runs.id`。`list_turn_traces` 只回主行,
   *  worker 行走 `list_worker_turn_traces`。optional 而非必填:
   *  回滚兼容(新前端 + 旧后端 payload 无此字段)+ 既有 fixture
   *  零改动;消费点以 `?? ''` 容错(store 归一化时兜底)。 */
  runId?: string;
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
  /** unified-context-budget WP1 (2026-08-19): @文件切片 —— 全部
   *  user message 的 @-token 注入正文 est 之和(文本 + 降级占位;
   *  @图走 imagesToken 不在此列)。Mirrors Rust
   *  `TurnTraceRow.at_files_token` → `atFilesToken`。实发口径
   *  (prd D9)。`null` for pre-column rows / worker turns / 零注入
   *  requests。 */
  atFilesToken: number | null;
  /** unified-context-budget WP1 (2026-08-19): system 切片 = system
   *  prompt 本体(发送部件)+ skill listing 合成消息(messages 内
   *  归因口径)。Mirrors Rust `TurnTraceRow.system_token` →
   *  `systemToken`。`null` for pre-column rows and worker turns. */
  systemToken: number | null;
  /** unified-context-budget WP1 (2026-08-19): 请求时
   *  `context_window` 快照 —— 预算行分母(NOT a token slice)。
   *  Mirrors Rust `TurnTraceRow.context_window` → `contextWindow`。
   *  `null` for pre-column rows(card 回退 200_000)。 */
  contextWindow: number | null;
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
  /** unified-context-budget WP1 (2026-08-19): @文件切片(全部 user
   *  message 的注入正文 est 之和;实发口径,裁剪后 = 预裁 − freed)。
   *  Render treatment mirrors the img cell(gated on > 0)。*/
  atFilesToken?: number;
  /** unified-context-budget WP1 (2026-08-19): system 切片(system
   *  prompt 本体 + skill listing 归因)。Render treatment mirrors the
   *  tools/mem cells(列存在即展示)。*/
  systemToken?: number;
  /** unified-context-budget WP1 (2026-08-19): 请求时 context_window
   *  快照 —— contextUtilPct 的分母(per-model;旧行回退 200_000)。 */
  contextWindow?: number;
  /** unified-context-budget WP2 (2026-08-19): 关卡⑤硬卡的裁剪观察
   *  (live 路径经 ChatEvent::BudgetTrim 归一化写入;回看路径无此维
   *  —— 持久化记录在 context_budget_trim 审计行,不在 turn_trace)。 */
  budgetTrim?: {
    freed_tokens: number;
    post_total: number;
    window: number;
  };
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
  /** 08-18 PR2:压缩路径(`"none"` / `"summary"` / `"mechanical"`)。
   *  optional —— 旧后端 wire 无此字段,消费侧(store/live 归一化)
   *  须按 `"none"` 兜底。 */
  method?: string;
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

/** Mirror of Rust `ChatEvent::BudgetTrim` payload
 *  (unified-context-budget WP2, snake_case wire)。 */
export interface BudgetTrimEvent {
  kind: "budget_trim";
  request_id: string;
  seq: number;
  freed_tokens: number;
  post_total: number;
  window: number;
}

/** 08-20-turn-usage-event-quota-view WP1: per-turn token 观察(live
 * 路径)。Mirrors Rust `ChatEvent::TurnUsage`(snake_case wire)。切片
 * 字段 `number | null`(Rust `Option<u32>` 序列化为 null;worker 行按
 * NULL 列契约带 null —— 但 worker 事件不冒泡到主 chat,主 loop 行
 * 全切片非 null)。store 侧归一化 null → undefined(TurnTrace 的
 * "undefined = never written" 语义)。 */
export interface TurnUsageEvent {
  kind: "turn_usage";
  request_id: string;
  seq: number;
  /** `''` = 主 loop 行(worker 事件被 sink 跳过,不到达前端;字段
   *  保留与 wire 形状对齐)。 */
  run_id: string;
  usage: TokenUsagePayload;
  tools_token?: number | null;
  memory_token?: number | null;
  images_token?: number | null;
  at_files_token?: number | null;
  system_token?: number | null;
  context_window: number;
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
  if (row.atFilesToken != null) {
    out.atFilesToken = row.atFilesToken;
  }
  if (row.systemToken != null) {
    out.systemToken = row.systemToken;
  }
  if (row.contextWindow != null) {
    out.contextWindow = row.contextWindow;
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
