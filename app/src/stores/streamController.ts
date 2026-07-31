// streamController — single source of truth for in-flight chat
// streams and per-session message buffers.
//
// Why this exists:
//   The previous `chat.ts` store held `messages.value` for the
//   *current* session only. Switching sessions reloaded from DB
//   and overwrote the in-memory array — which lost the in-flight
//   streaming message and stranded state (sending flag, red dot,
//   cancel button). This controller fixes that by owning the
//   message buffer for all visited sessions (with an LRU bound
//   so memory doesn't grow unbounded) and by keeping the SSE
//   listener logic out of the per-session event filter that was
//   dropping `done` events for non-current sessions.
//
// Architecture (per the PRD for 06-07-6-ui-bug-markdown-sse):
//   - `messagesBySession`: Map<sessionId, ChatMessage[]>, the
//     unique source of truth for the messages the UI renders.
//   - `activeRequests`: Map<requestId, RequestState>, tracks
//     which streams are in flight. Per-session independent —
//     multiple sessions can stream concurrently.
//   - `streamingSessionIds` / `streamingProjectIds`: reactive
//     Sets derived from `activeRequests`, for UI subscription
//     (project tab red dots, session card streaming indicators).
//   - One global SSE listener; events route by `request_id` to
//     the matching active request, NOT by current session.
//   - Pinned LRU: a session with an active stream is pinned and
//     cannot be evicted by the LRU. The streaming message would
//     otherwise be lost mid-request.
//
// Public API (consumed by `useChatStore` in chat.ts):
//   - `getMessages(sessionId)` — reactive read, touches LRU
//   - `ensureLoaded(sessionId)` — DB read if not cached
//   - `evict(sessionId)` — explicit removal (e.g. on delete)
//   - `startRequest({ sessionId, projectId, text, history })`
//   - `cancel(requestId)`
//   - `start()` / `stop()` — listener lifecycle
//
// This file is the PR2 scaffold. The wiring into chat.ts and the
// UI consumers (SessionList, ProjectTabs) lands in PR3 + PR4.

import { defineStore } from "pinia";
import { computed, markRaw, reactive, ref, type ComputedRef } from "vue";
import { transport, type UnlistenFn } from "../transport";
import { extractErrorMessage } from "../utils/useErrorBus";

import { useChatStore } from "./chat";
import type {
  ChatMessage,
  ContentBlockView,
  ErrorCategory,
  InjectionEntry,
} from "./chat.types";
import {
  useChecklistStore,
  CHECKLIST_TOOL_NAME,
} from "./checklist";
import { useMemoryStore } from "./memory";
import { useProjectsStore } from "./projects";
import {
  useReviewStateStore,
  matchesReviewStatePath,
} from "./reviewState";
import { useTraceStore } from "./traceStore";
import {
  useQuestionCardsStore,
} from "./questionCards";
import {
  GET_PENDING_INTERACTION_CMD,
  MODE_CHANGE_EVENT,
  TASK_STATE_TRANSITION_EVENT,
  TOOL_QUESTION_EVENT,
  type ModeChangePayload,
  type PendingInteraction,
  type TaskStateTransitionPayload,
  type ToolQuestionPayload,
} from "./questionCards.types";

/** Upper bound on number of sessions whose messages are kept
 *  in memory. Pinned (in-flight streaming) sessions are not
 *  counted against this limit — they can keep the cache
 *  temporarily over budget. 20 is a guess based on the typical
 *  developer usage: a couple of active projects × ~5 recent
 *  sessions per project. Tweak as needed. */
const CACHE_SIZE = 20;

interface RequestState {
  requestId: string;
  sessionId: string;
  projectId: string;
  userMsgId: string;
  assistantMsgId: string;
  // Captured at send time so the wire-format history matches
  // what `chat.ts` constructed (preserves thinking blocks,
  // tool_use blocks, and tool_result blocks verbatim — the
  // Anthropic API 400s if any of those are missing or rewritten).
  history: unknown[];
  // F5 (LLM Latency Tracking): wall-clock timestamps for the
  // three latencies. `sendAt` is set on `startRequest`; the
  // first `delta` event sets `firstDeltaAt`; the `done` event
  // reads `Date.now()` for `doneAt`. The three millisecond
  // values are computed in the `done` handler and stashed on
  // `latencyPending`; `reloadAfterFinalize` reads the stashed
  // value once the assistant row's seq is known and fires the
  // `update_message_latency` IPC.
  sendAt: number;
  firstDeltaAt: number | null;
  // F5: per-tool timing keyed by tool_use_id. Set on
  // `tool:call` (in `handleToolCall`), read on `tool:result`
  // (in `handleToolResult`) to compute `durationMs`. The
  // result is patched onto the in-memory `toolResult` and
  // sent to the `record_tool_duration` IPC to update the
  // `messages.content` JSON.
  toolStartedAt: Map<string, number>;
  // F5 follow-up per-turn: per-turn latency accumulator.
  // Keyed by `currentTurnIndex` (0-based, incremented in
  // the `case "start"` arm). Populated by the
  // `case "turn_complete"` arm with the 4 ms values the
  // agent loop emits right after `persist_turn`. The
  // `seq` field on each entry is the assistant row's seq
  // (assigned by the agent loop in the per-session
  // `next_seq` counter) — used by `reloadAfterFinalize` to
  // fire one `update_message_latency` IPC per entry (N per
  // request, not 1). Replaces the F5 single-value
  // `latencyPending` which only ever held the LAST turn's
  // data and forced N-1 rows to stay NULL on multi-turn
  // responses.
  currentTurnIndex: number;
  latencyByTurn: Map<number, TurnLatency>;
  // 交错思考(实时态): 正在累积、尚未 flush 进 contentBlocks 的文本。
  // 思考与文本交错时,文本按段累积到 `pendingTimelineText`,遇到
  // thinking/tool/redacted 边界时 flush 成一个 text 块进 contentBlocks
  // (镜像后端 chat_loop.rs 的 pending_text / flush_pending_text)。
  // 这样实时流式态也能让 MessageItem 的 renderTimeline 按真实流序
  // 交错渲染 thinking + text(而非 reload 后才有交错)。
  pendingTimelineText: string | null;
  // 交错思考(实时态): contentBlocks 里"当前正在累积的 thinking 块"的
  // 索引。thinking_delta/signature_delta 就地 mutate 这个块(而非 buffer 在
  // pending 里),这样 contentBlocks 从第一个思考 token 就非空(useTimeline
  // 立刻为 true,不闪回顶部 ThinkingBlock)。遇非思考边界(delta/tool/redacted/
  // done)时置 null(seal),下次 thinking_delta 开新块。
  // 块边界 = "thinking ↔ 非 thinking 切换",**不依赖 signature** —— 完全
  // 镜像后端 chat_loop.rs 的 flush_pending_thinking(在 Delta/ToolCall 边界
  // flush,signature 只附属累积)。这样实时态与 reload 后拆分一致:相邻
  // thinking(think1→sig1→think2→sig2,无中间文本)合并成一个块,和后端
  // 落库/rehydrate 一致。OpenAI(无 signature)同理单块。
  activeThinkingIdx: number | null;
  // F5: per-request error flag. The cancel / network-drop
  // path also persists a partial turn (with `usage: None`),
  // so the seq-lookup is still meaningful — the errored
  // turn just has its latency recorded without a usage.
  // The flag is consulted by `reloadAfterFinalize` to
  // decide whether to pass the latency through to the IPC
  // (it always does — totalMs is meaningful even for
  // errored turns).
}

/** F5 follow-up per-turn: a single assistant row's latency
 *  quadruple, with the row's `seq` for IPC routing.
 *  Mirrors the Rust `ChatEvent::TurnComplete` payload. */
interface TurnLatency {
  seq: number;
  ttfbMs: number | null;
  genMs: number | null;
  totalMs: number | null;
  thinkingMs: number | null;
}

interface ChatEventPayload {
  request_id: string;
  kind:
    | "start"
    | "delta"
    | "thinking_delta"
    | "signature_delta"
    | "redacted_thinking_delta"
    | "done"
    | "turn_complete"
    | "error"
    // A5+ (2026-07-04, R8): transient retry notice. Emitted by
    // `LlmRetrySink` BEFORE each backoff sleep so the frontend
    // can show "↩ 重试中 2/3, 2s …" instead of looking frozen.
    // The handler attaches the payload to the in-flight
    // assistant placeholder's `retrying` field (NOT into the
    // `messages` array — it must not pollute the persisted DB
    // shape); the next `start` / `delta` / `done` clears it.
    | "retrying"
    // B2 PR3: per-user-turn `@relpath` injection manifest,
    // emitted ONCE per user turn (right after `inject_at_tokens`
    // runs on the last user message). Mirrors Rust
    // `ChatEvent::FileInjections { request_id, message_seq,
    // injections }`. The controller's `case "file_injections"`
    // arm patches the matching user message's `injections`
    // array by `request_id` + `message_seq`. The `injections`
    // shape is the wire-format tagged union — see
    // `InjectionEntry.action` for the `kind` discriminator
    // rules.
    | "file_injections"
    // 07-06 (am-observability-panel R2b/A7): read-only recall
    // notice. Emitted by the agent loop at the FTS recall site
    // (turn start) and at each pitfall-recall site (on tool-call
    // dispatch). The controller does NOT attach this to the
    // message buffer (it must not pollute the persisted shape);
    // it routes the hits into `useMemoryStore().pushRecallHits`
    // so the ChatPanel recall chip renders "本次召回 N 条".
    // Worker sinks never emit on `chat-event` (AC7), so a hit
    // arriving here is always from the main chat loop.
    | "recall"
    // E2 (harness trace pipeline, 2026-07-14): the 3 new
    // always-on trace variants from `ChatEvent::ContextCompacted`
    // / `ChatEvent::LoopHint` / `ChatEvent::WorkflowBreadcrumb`
    // (Rust `llm/types.rs:485-514`). These ride the `chat-event`
    // channel; the controller's `handleChatEvent` cases route
    // them into `useTraceStore().applyEvent`. Field names are
    // snake_case on the wire (per the `ChatEvent` enum's
    // `rename_all = "snake_case"`); the trace store normalizes
    // to camelCase typed sub-objects on the live path.
    | "context_compacted"
    | "loop_hint"
    | "workflow_breadcrumb";
  text?: string;
  signature?: string;
  data?: string;
  stop_reason?: string;
  message?: string;
  category?: ErrorCategory;
  /** A4 (Token Usage Tracking): the per-turn token usage report
   *  from the LLM. `undefined` on every non-Done event, and on
   *  Done events where the provider did not report usage
   *  (cancel / error / network drop). Schema mirrors Rust
   *  `llm::types::TokenUsage`. */
  usage?: TokenUsagePayload;
  // F5 follow-up per-turn: only present when `kind === "turn_complete"`.
  // Mirrors Rust `ChatEvent::TurnComplete` payload. `seq` is the
  // assistant row's seq (assigned by the agent loop in the
  // per-session `next_seq` counter); the 4 ms fields are
  // `Option<i64>` server-side and `number | null` here, with
  // `null` for turns that never reached the corresponding
  // boundary (e.g. a turn that emitted tool_call straight from
  // thinking_delta with no text delta has `ttfb_ms: null` and
  // `gen_ms: null` but `total_ms` and `thinking_ms` set).
  seq?: number;
  ttfb_ms?: number | null;
  gen_ms?: number | null;
  total_ms?: number | null;
  thinking_ms?: number | null;
  // B2 PR3: only present when `kind === "file_injections"`.
  // `message_seq` is the seq the agent loop assigned to the
  // user row (the per-session `next_seq` counter) — used to
  // locate the user message on the controller side without
  // a per-request `userMsgId` plumbing. The rehydrate path
  // also uses seq as the message key (the user message's
  // `id` is `${sid}-${seq}`), so this value round-trips
  // through the DB and matches the rehydrated key.
  message_seq?: number;
  injections?: InjectionEntry[];
  // A5+ (2026-07-04): only present when `kind === "retrying"`.
  // Mirrors Rust `ChatEvent::Retrying`. The handler attaches
  // the four fields to the in-flight assistant placeholder's
  // transient `retrying` field for the MessageItem to render.
  attempt?: number;
  max_attempts?: number;
  wait_ms?: number;
  reason?: string;
  // 07-06 (am-observability-panel A7): only present when
  // `kind === "recall"`. Mirrors Rust `ChatEvent::Recall { hits }`.
  // The handler routes the array into
  // `useMemoryStore().pushRecallHits(sessionId, hits)`. snake_case
  // fields match the Rust `RecallHit` (no `rename_all`) — consistent
  // with `Retrying` / `FileInjections` nested payloads.
  hits?: RecallHitWire[];
  // E2 (harness trace pipeline, 2026-07-14): only present when
  // `kind === "context_compacted"`. Mirrors Rust
  // `ChatEvent::ContextCompacted`. The handler routes into
  // `useTraceStore().applyEvent` (live path).
  tokens_before?: number;
  tokens_after?: number;
  dropped_count?: number;
  degradation?: string;
  // E2: only present when `kind === "loop_hint"`. Mirrors
  // Rust `ChatEvent::LoopHint`.
  hit_count?: number;
  verdict_kind?: string;
  // E2: only present when `kind === "workflow_breadcrumb"`.
  // Mirrors Rust `ChatEvent::WorkflowBreadcrumb`. The
  // `task_slug` / `status` are `null` on the wire when
  // there's no active workflow task.
  task_slug?: string | null;
  status?: string | null;
  breadcrumb_text?: string;
}

/** 07-06 (am-observability-panel A7): the wire shape of a single
 *  element in `ChatEvent::Recall { hits }`. Mirrors the Rust
 *  `RecallHit` struct (snake_case, no `rename_all`). Local to this
 *  file (not re-exported) because the memory store owns the
 *  canonical frontend `RecallHit` type — this is just the raw wire
 *  payload the controller validates before handing off. */
interface RecallHitWire {
  memory_id: number;
  title: string;
  kind: string;
  source: "fts" | "pitfall";
}

/** A4: 4-field token usage payload from the LLM. Mirrors Rust
 *  `llm::types::TokenUsage` (snake_case to match the existing
 *  IPC convention — see backend/llm-contract.md "Scenario: Token
 *  Usage Tracking" §3). The frontend reads this in the `done`
 *  event handler to update the per-session totals displayed in
 *  the ChatInput hint. */
interface TokenUsagePayload {
  input_tokens: number;
  output_tokens: number;
  cache_creation_input_tokens: number;
  cache_read_input_tokens: number;
  /**
   * 2026-06-26 snapshot fix: cross-provider-normalized total input
   * for this request (Anthropic: input+cc+cr; OpenAI: prompt_tokens).
   * The canonical "% of context_window" numerator. Optional for
   * backward-compat with older backends that pre-date the field —
   * `setLastTurnUsage` falls back to the sum of the 4 fields.
   */
  context_input_tokens?: number;
}

interface ToolCallPayload {
  request_id: string;
  id: string;
  name: string;
  input: Record<string, unknown>;
}

interface ToolResultPayload {
  request_id: string;
  tool_use_id: string;
  content: string;
  is_error: boolean;
}

interface LoadedMessage {
  id: number;
  session_id: string;
  role: "user" | "assistant";
  content: unknown;
  text: string;
  has_tool_calls: boolean;
  has_tool_results: boolean;
  created_at: string;
  seq: number;
  /** F5 (LLM Latency Tracking): per-message latency breakdown.
   *  All three are `null` for pre-F5 rows. Rehydrated into
   *  the assistant message's `latency` field; the
   *  `MessageItem` footer renders `totalMs` and the hover
   *  tooltip shows the three lines. */
  ttfb_ms: number | null;
  gen_ms: number | null;
  total_ms: number | null;
  /** F5 follow-up: thinking-phase wall-clock duration in ms.
   *  `null` for messages that never entered the thinking
   *  phase AND for pre-F5-follow-up rows. Rehydrated into
   *  the assistant message's `thinkingDurationMs` field;
   *  the `ThinkingBlock` header renders it as
   *  "Thought for X.Xs" (replacing the previous "X tokens"
   *  estimate). Persisted by `update_message_latency`'s
   *  new 4th-column UPDATE — same IPC, one extra bind. */
  thinking_ms: number | null;
  /** Group chat (07-29-group-chat, Phase 4 TODO-B/D3): the
   *  originating speaker for this message. Round-tripped
   *  from the Rust `MessageRow.speaker` column. `undefined`
   *  / `null` for classic chat / subagent / review messages
   *  (the pre-Phase 4 default + unchanged). For group-chat
   *  sessions, set to the fixed identifier `"moderator"` for
   *  moderator turns or the participant's `name` for
   *  participant turns. Rehydrated into the
   *  `ChatMessage.speaker` field; the `MessageItem` renders
   *  a chip + accent color when present. Optional in the
   *  type so pre-Phase-4 test fixtures without `speaker` still
   *  typecheck — the rehydrate path treats `undefined` as
   *  `null` (no chip). */
  speaker?: string | null;
  /** B2 PR3: optional per-user-turn injection manifest
   *  JSON, written by the agent loop's `update_message_metadata`
   *  SQL after `inject_at_tokens` produces the list. `null`
   *  for non-user rows AND for user rows without
   *  `@relpath` tokens. The rehydrate path parses this
   *  into `ChatMessage.injections` so the hint row
   *  survives a session reload. The shape is the same
   *  wire-format tagged-union as the live `FileInjections`
   *  event — see `InjectionEntry` / `InjectionRecord`.
   *  Optional in the type so existing test fixtures that
   *  don't model metadata still typecheck; the production
   *  IPC always sends `metadata` (NULL for non-user rows
   *  per `db::MessageRow::metadata`). */
  metadata?: unknown;
}

interface LoadedSession {
  session: {
    id: string;
    title: string;
    created_at: string;
    updated_at: string;
    model: string;
    project_id: string;
    current_cwd: string;
    /** Step 4 follow-up: tri-state worktree state. The `none`
     *  default lets pre-follow-up + post-follow-up sessions
     *  load identically; the UI uses this to render the
     *  three-state worktree chip in ChatPanel. */
    worktree_state: "none" | "active" | "detached";
    worktree_path: string | null;
    last_worktree_path: string | null;
    /** PR4 of multi-model: per-session model override. `null`
     *  means the session uses the global default model. The
     *  ModelSelect popover in the chat input reads/writes this
     *  via the `update_session_model_id` IPC. */
    model_id: string | null;
    /** A4 (Token Usage Tracking): per-session cumulative
     *  token totals. `null` for pre-A4 sessions (the columns
     *  are nullable; a legacy session's first post-upgrade
     *  turn starts the counter from 0). The frontend uses
     *  these to render the ChatInput hint area's
     *  "14.2K · 7% / 200K" line. */
    input_tokens_total: number | null;
    output_tokens_total: number | null;
    cache_creation_total: number | null;
    cache_read_total: number | null;
    /** Group chat (07-29-group-chat, Phase 4 TODO-D1):
     *  session type discriminator. `"chat"` is the default
     *  (existing classic-chat + review + subagent sessions);
     *  `"group_chat"` is the new free-form multi-LLM
     *  session type. Backed by the `sessions.session_type`
     *  column (Phase 1 migration, default `"chat"`). Drives
     *  the chat entry-point branch in `chat_inner` (classic
     *  vs. `run_group_chat_loop`) + the frontend `SessionList`
     *  type badge + the `GroupChatConfigModal` access path. */
    session_type: "chat" | "group_chat";
    /** Group chat (07-29-group-chat, Phase 4 TODO-D1):
     *  per-session free-form JSON metadata. Classic chat
     *  sessions have `null` (no metadata). Group-chat
     *  sessions store `{participants: ParticipantConfig[]}`.
     *  Optional in the type so existing test fixtures that
     *  don't model group chat still typecheck; the production
     *  IPC always sends `metadata` (NULL for non-group-chat
     *  rows per `db::SessionRow::metadata`). */
    metadata?: Record<string, any> | null;
  };
  messages: LoadedMessage[];
}

const genId = () =>
  Math.random().toString(36).slice(2) + Date.now().toString(36);

// --- Module-level listener state ---------------------------------------
// One global listener for the whole app, owned by the controller.
// Lifted out of the store setup so it persists across HMR
// re-instantiations of the Pinia store (otherwise the listener
// is registered twice after a hot reload and events double-fire).
let unlistenChat: UnlistenFn | null = null;
let unlistenTC: UnlistenFn | null = null;
let unlistenTR: UnlistenFn | null = null;
let unlistenTQ: UnlistenFn | null = null;
let unlistenMC: UnlistenFn | null = null;
let unlistenTST: UnlistenFn | null = null;
let listenerWired = false;

// --- Wire-format rehydration ------------------------------------------
// Lifted from chat.ts so the controller can own message shape
// without depending on chat.ts (which will in turn import the
// controller). Identical logic — kept here to break the cycle.
//
// Exported (re-exported as a named binding below) so the
// `streamController.test.ts` file can call it directly. The
// public Pinia store API does not re-export this function;
// callers should go through `ensureLoaded`.
export function rehydrateMessages(loaded: LoadedMessage[]): ChatMessage[] {
  const out: ChatMessage[] = loaded.map((m) => {
    const blocks = Array.isArray(m.content) ? (m.content as Array<Record<string, unknown>>) : [];
    const toolCalls: ChatMessage["toolCalls"] = [];
    const toolResults: ChatMessage["toolResults"] = [];
    const thinkingBlocks: ChatMessage["thinkingBlocks"] = [];
    const redactedThinkingData: string[] = [];
    // 交错思考: 按 DB content 数组原序构建 `contentBlocks`,供
    // `MessageRunGroup` 做流式渲染。与上面的分桶数组共享同一个遍历
    // 循环(避免二次遍历),每个有效分支同时 push 到分桶 + contentBlocks。
    // 注意 `continue`(坏块跳过)也会跳过 contentBlocks,保持一致。
    const contentBlocks: ChatMessage["contentBlocks"] = [];
    for (const b of blocks) {
      if (!b || typeof b.type !== "string") continue;
      if (b.type === "thinking") {
        thinkingBlocks.push({
          text: (b.thinking as string) ?? "",
          signature: (b.signature as string) ?? "",
        });
        contentBlocks.push({
          kind: "thinking",
          text: (b.thinking as string) ?? "",
          signature: (b.signature as string) ?? "",
        });
      } else if (b.type === "redacted_thinking" && typeof b.data === "string") {
        redactedThinkingData.push(b.data);
        contentBlocks.push({ kind: "redacted_thinking", data: b.data });
      } else if (
        b.type === "tool_use" &&
        typeof b.id === "string" &&
        typeof b.name === "string"
      ) {
        toolCalls.push({ id: b.id, name: b.name, input: (b.input as Record<string, unknown>) ?? {} });
        contentBlocks.push({
          kind: "tool_use",
          id: b.id,
          name: b.name,
          input: (b.input as Record<string, unknown>) ?? {},
        });
      } else if (b.type === "tool_result" && typeof b.tool_use_id === "string") {
        // F5: per-tool duration is embedded in the tool_result
        // block as `duration_ms` (per R2 / ADR-lite decision 1).
        // Read it here so the ToolCallCard can display "0.3s"
        // on reload. Pre-F5 blocks (no `duration_ms` field) leave
        // it `undefined` → the card renders nothing.
        const durationRaw = b.duration_ms;
        const durationMs =
          typeof durationRaw === "number" && Number.isFinite(durationRaw)
            ? Math.max(0, Math.round(durationRaw))
            : undefined;
        toolResults.push({
          toolUseId: b.tool_use_id,
          content: (b.content as string) ?? "",
          isError: !!b.is_error,
          ...(durationMs !== undefined ? { durationMs } : {}),
        });
        contentBlocks.push({
          kind: "tool_result",
          toolUseId: b.tool_use_id,
          content: (b.content as string) ?? "",
          isError: !!b.is_error,
          ...(durationMs !== undefined ? { durationMs } : {}),
        });
      } else if (b.type === "text" && typeof b.text === "string") {
        // 交错思考: text 块也透传(后端流序落库后,一个 turn 可能有
        // 多个 text 块——思考夹在两段文本之间时)。
        contentBlocks.push({ kind: "text", text: b.text });
      }
    }
    const msg: ChatMessage = {
      id: `${m.session_id}-${m.seq}`,
      role: m.role,
      content: m.text,
    };
    if (toolCalls.length) msg.toolCalls = toolCalls;
    if (toolResults.length) msg.toolResults = toolResults;
    if (thinkingBlocks.length) msg.thinkingBlocks = thinkingBlocks;
    if (redactedThinkingData.length) msg.redactedThinkingData = redactedThinkingData;
    // 交错思考: 仅当有透传块时挂上(避免给纯文本 user 消息挂空数组)。
    if (contentBlocks.length) msg.contentBlocks = contentBlocks;
    // F5: per-message latency. All three fields are nullable
    // in the DB; only the assistant rows that ran an LLM turn
    // will have non-null values. We attach `latency` only when
    // at least one field is present, so the UI can use the
    // presence-check (`m.latency && m.latency.totalMs`) to
    // distinguish "—" from "0.0s" (which is a real value
    // — extremely fast local proxy).
    const hasLatency =
      m.ttfb_ms !== null ||
      m.gen_ms !== null ||
      m.total_ms !== null;
    if (hasLatency) {
      msg.latency = {
        ...(m.ttfb_ms !== null ? { ttfbMs: m.ttfb_ms } : {}),
        ...(m.gen_ms !== null ? { genMs: m.gen_ms } : {}),
        ...(m.total_ms !== null ? { totalMs: m.total_ms } : {}),
      };
    }
    // F5 follow-up: thinking-phase wall-clock. Mirrors the
    // `latency` triple's "only set if at least one field is
    // present" rule — the ThinkingBlock header uses the
    // `thinkingDurationMs !== undefined` presence check to
    // distinguish "—" from "0.0s" (a real, extremely fast
    // local-proxy value). Pre-F5-follow-up rows have the
    // column NULL and fall through to undefined, which the
    // UI renders as "—" — the same fallback the in-memory
    // path used before this persistence work.
    if (m.thinking_ms !== null) {
      msg.thinkingDurationMs = m.thinking_ms;
    }
    // B2 PR3: parse the `metadata` JSON into the
    // `injections` field. The agent loop wrote the
    // per-user-turn injection manifest here via
    // `update_message_metadata` (see
    // `db::sessions::update_message_metadata`); a
    // `null` / missing / non-array metadata is the
    // "no @relpath tokens" case and is rendered
    // as no hint row. The `action` object's shape
    // is the same wire-format tagged union as
    // the live `FileInjections` event — we
    // narrow with the same `kind` discriminator.
    if (m.metadata !== null && m.metadata !== undefined) {
      const meta = m.metadata as { injections?: unknown };
      if (Array.isArray(meta.injections)) {
        // Defensive: skip entries that don't have
        // the {path, action} shape — DB writes can
        // outlive the schema. Real entries are
        // typed via `InjectionEntry`; we just
        // assign the parsed array directly.
        const entries: InjectionEntry[] = [];
        for (const r of meta.injections) {
          if (
            r &&
            typeof r === "object" &&
            typeof (r as { path?: unknown }).path === "string" &&
            (r as { action?: unknown }).action &&
            typeof (r as { action?: { kind?: unknown } }).action?.kind ===
              "string"
          ) {
            entries.push(r as InjectionEntry);
          }
        }
        if (entries.length > 0) {
          msg.injections = entries;
        }
      }
      // D3 PR3 (2026-06-17): also surface the raw metadata
      // object on the in-memory message so MessageItem can
      // render the "(edited)" label when `metadata.edited_at`
      // is present. The shape is loosely typed (Record<string,
      // unknown>) so future metadata fields don't require
      // touching this rehydrate site. We attach the parsed
      // object verbatim — the same JSON the agent loop
      // persisted via `edit_user_message` (see
      // `.trellis/spec/backend/database-guidelines.md`
      // "Pattern: `edit_user_message`" — `metadata` shape is
      // `{ edited_at, original_content? }`).
      msg.metadata = meta;
    }
    // The `seq` is plumbed through for the F5
    // `update_message_latency` IPC. The streaming path tracks
    // it on `RequestState` instead (the seq is the agent
    // loop's handle, not the controller's).
    msg.seq = m.seq;
    // Group chat (07-29-group-chat, Phase 4 TODO-D3): pass the
    // speaker through verbatim. `m.speaker` is `undefined` /
    // `null` for classic chat / subagent / review rows
    // (pre-Phase 4 behavior) and rehydrates into `msg.speaker`
    // as undefined — the UI's `v-if="message.speaker"` chip
    // condition naturally skips these. For group-chat rows,
    // carries the moderator's `"moderator"` identifier or the
    // participant's user-visible name; the `MessageItem`
    // renders the corresponding chip + accent color.
    if (m.speaker) {
      msg.speaker = m.speaker;
    }
    return msg;
  });
  // Merge user-message tool_results into the previous assistant
  // message for the UI's "done / running" lookup (see chat.ts for
  // the long version of this comment).
  for (let i = 0; i < out.length; i++) {
    const m = out[i];
    if (m.role !== "user" || !m.toolResults?.length) continue;
    for (let j = i - 1; j >= 0; j--) {
      if (out[j].role === "assistant") {
        if (!out[j].toolResults) out[j].toolResults = [];
        out[j].toolResults!.push(...m.toolResults!);
        break;
      }
    }
  }
  // BUG FIX (2013 tool_use orphan, frontend rehydrate side): the
  // backend's `chat` command used to (pre-fix) return on cancel
  // *after* persisting the assistant turn with `tool_use` blocks
  // but *before* persisting the corresponding `user(tool_result)`
  // turn. The DB ended up with an orphan `tool_use` and the next
  // `send()` built a history where `tool_use` had no follow-up
  // `tool_result` — Anthropic API 2013 ("tool call result does
  // not follow tool call"). The backend now persists a synthetic
  // `tool_result` on cancel (see `build_synthetic_tool_result_message`
  // in `app/src-tauri/src/lib.rs`), so *new* orphans stop
  // appearing. This step repairs **historical** orphans sitting
  // in the DB from before that fix.
  //
  // We splice in a synthetic user-role message with one
  // `tool_result` block per orphan `tool_use` id, immediately
  // after the orphan assistant. The merge step above does NOT
  // cover this case: it only moves `tool_result` data from a
  // user message that already has it onto the *preceding*
  // assistant. An orphan `tool_use` is the inverse — an
  // assistant `tool_use` with no following user `tool_result`
  // at all.
  //
  // Reverse scan so the splice-in's index shift doesn't
  // affect the next iteration (splicing at `i + 1` shifts
  // `i + 1` to `i + 2`, but the loop is going down so we
  // won't visit `i + 2` again).
  for (let i = out.length - 1; i >= 0; i--) {
    const m = out[i];
    if (m.role !== "assistant" || !m.toolCalls?.length) continue;
    // Set of `tool_use_id`s already paired with a `tool_result`,
    // either by the merge step (results copied onto this
    // assistant from a later user message) or by the *next*
    // message in the post-merge array carrying its own
    // `toolResults`. Both sources are checked because the
    // merge step *copies* (does not move) toolResults, so
    // a user message that the merge step drained for a
    // *different* preceding assistant can still have its
    // own (now-empty after merge) toolResults field — but
    // for our purposes the post-merge view of the assistant
    // plus the immediate next message's toolResults covers
    // every "did the wire get a result" question.
    const coveredIds = new Set<string>();
    for (const tr of m.toolResults ?? []) coveredIds.add(tr.toolUseId);
    const next = i + 1 < out.length ? out[i + 1] : null;
    if (next && next.role === "user") {
      for (const tr of next.toolResults ?? []) coveredIds.add(tr.toolUseId);
    }
    const orphanCalls = m.toolCalls.filter((tc) => !coveredIds.has(tc.id));
    if (orphanCalls.length === 0) continue;
    const syntheticMsg: ChatMessage = {
      // Distinct id so subsequent `send()`s that build a fresh
      // `userMsg` / `assistantMsg` placeholder don't collide
      // with this synthetic. The `id` is internal to the
      // store / `controller` filter logic — it never reaches
      // the LLM wire.
      id: `${m.id}-orphan-repair`,
      role: "user",
      content: "",
      toolResults: orphanCalls.map((tc) => ({
        toolUseId: tc.id,
        // Same wording as `build_synthetic_tool_result_message`
        // in `lib.rs` so the LLM sees a consistent shape on
        // the live-cancel and the historical-repair paths.
        // English + tool name (per PRD ADR-lite decision).
        content: `Tool execution was interrupted: the user stopped the request or the session was cancelled before the tool could run. The tool ${tc.name} did not run.`,
        isError: true,
      })),
    };
    out.splice(i + 1, 0, syntheticMsg);
    // Mirror the merge step's UI-grouping behavior: push the
    // synthetic toolResults onto the assistant message so the
    // UI's "tool just finished" lookup on the assistant
    // message surface the synthetic results too. Mirrors
    // `out[j].toolResults!.push(...m.toolResults!)` in the
    // merge loop above.
    if (!m.toolResults) m.toolResults = [];
    m.toolResults.push(
      ...syntheticMsg.toolResults!.map((tr) => ({
        toolUseId: tr.toolUseId,
        content: tr.content,
        isError: tr.isError,
      })),
    );
  }
  // After the merge step, the four "deep payload" arrays on every
  // message (toolCalls / toolResults / thinkingBlocks /
  // redactedThinkingData) are immutable for the lifetime of this
  // message — they were built from the DB once, and nothing in
  // this store will ever push into them again. Mark them raw so
  // the reactive Map's deep-proxy does not wrap them (and the
  // ToolCallInfo / ThinkingBlockInfo items inside them) on every
  // access. For a 5000-message session this is the difference
  // between ~10k proxy operations at first render and zero.
  //
  // We do NOT markRaw the message itself, the `content` string, or
  // the `streaming` / `error` fields — those are the per-message
  // mutables that still need reactive updates (see the streaming
  // path below for the parallel markRaw that fires when a fresh
  // message's stream ends).
  for (const m of out) {
    if (m.toolCalls) markRaw(m.toolCalls);
    if (m.toolResults) markRaw(m.toolResults);
    if (m.thinkingBlocks) markRaw(m.thinkingBlocks);
    if (m.redactedThinkingData) markRaw(m.redactedThinkingData);
    // B2 PR3: `injections` is also immutable post-rehydrate
    // — the live `FileInjections` event patches the
    // user message *during* the request, not after a
    // reload. Marking it raw skips the deep proxy wrap
    // for the array and its entries (the cost is small
    // per turn but adds up for sessions with many
    // @file mentions across many turns).
    if (m.injections) markRaw(m.injections);
  }
  return out;
}

/** Pure decision + message builder for the cross-session pending
 *  toast (exported top-level so the Q3 gate "current session → no
 *  toast" can be unit-tested without spinning up the store /
 *  Tauri listener). Returns `null` when `sessionId ===
 *  currentSessionId` (the inline card is already visible, so
 *  toasting would only disrupt the active session), otherwise a
 *  `{ message, sessionId }` toast payload. Title resolution: a
 *  cross-project target is absent from `sessions` (which hold only
 *  the current project's rows), so it falls back to the generic
 *  "另一项目的会话" label — no session→project mapping (Q4). */
export function buildPendingNotification(
  sessionId: string,
  kind: "mode_change" | "question",
  currentSessionId: string | null,
  sessions: { id: string; title: string }[],
  targetMode?: "edit" | "plan" | "yolo",
): { message: string; sessionId: string } | null {
  if (sessionId === currentSessionId) return null;
  const title = sessions.find((s) => s.id === sessionId)?.title;
  const who = title ? `「${title}」` : "另一项目的会话";
  const message =
    kind === "mode_change"
      ? `${who} 申请切换到 ${targetMode} 模式`
      : `${who} 有问题等你回答`;
  return { message, sessionId };
}

export const useStreamControllerStore = defineStore("streamController", () => {
  // ---------------------------------------------------------------------
  // State
  // ---------------------------------------------------------------------

  // The unique source of truth for in-memory messages. Outer Map
  // is a Vue `reactive` proxy so `set` / `delete` trigger UI
  // updates. Inner arrays and ChatMessage objects are also
  // reactive (Vue's reactive is deep), so `last.content += text`
  // in a delta handler triggers the bubble re-render.
  const messagesBySession = reactive(new Map<string, ChatMessage[]>());
  // Set of session IDs that have an active in-flight request.
  // Pinned in the LRU sense — cannot be evicted while streaming.
  const pinnedSessions = new Set<string>();
  // Tracks whether each session has been loaded from DB at least
  // once this app session. Used by `ensureLoaded` to skip the
  // IPC round-trip on subsequent accesses.
  const loadedFromDb = new Set<string>();

  // Active in-flight requests, keyed by request_id (so events
  // can route to the right session without scanning). Each
  // request is for exactly one session.
  const activeRequests = reactive(new Map<string, RequestState>());

  // F5: "just-completed" requests, keyed by request_id. The
  // request entry is moved here from `activeRequests` when
  // `finalizeRequest` runs, so the post-`done` cleanup is
  // synchronous (the existing test suite asserts immediate
  // state cleanup — see `finalizeRequest` paired-invariant
  // test) but the request state itself stays accessible to
  // `reloadAfterFinalize` for the latency IPC fire. The Map
  // is deleted on the next user-visible `finalizeRequest` /
  // stream start / session switch to bound memory. The two
  // Maps together implement "drop the public route, keep
  // the IPC payload".
  const completedRequests = new Map<string, RequestState>();

  const listenerReady = ref(false);

  // ---------------------------------------------------------------------
  // Derived reactive state for UI subscribers
  // ---------------------------------------------------------------------

  /** Sessions that currently have an in-flight stream. The
   *  `SessionList` component subscribes to this Set and renders
   *  a streaming indicator on the matching cards. */
  const streamingSessionIds = computed<Set<string>>(() => {
    const s = new Set<string>();
    for (const r of activeRequests.values()) {
      s.add(r.sessionId);
    }
    return s;
  });

  /** Projects that currently have at least one in-flight stream.
   *  Used by the project tab to render the red dot. Per-session
   *  independence means a single project can have multiple
   *  simultaneous streams (e.g. two sessions both active in the
   *  same project) — the dot stays on until all of them end. */
  const streamingProjectIds = computed<Set<string>>(() => {
    const s = new Set<string>();
    for (const r of activeRequests.values()) {
      s.add(r.projectId);
    }
    return s;
  });

  // ---------------------------------------------------------------------
  // Internal helpers
  // ---------------------------------------------------------------------

  /** Append an entry to the LRU, evicting the LRU non-pinned
   *  entry if over capacity. `reactive(Map)` tracks `set` /
   *  `delete` for us, so we just mutate it directly.
   *
   *  F5 (LLM Latency Tracking) follow-up: the array is
   *  wrapped in `reactive()` on insertion. Vue 3's
   *  `reactive(new Map())` does NOT auto-wrap stored values
   *  (native Map uses internal slots, not property access,
   *  so the outer Map's proxy can't intercept them) — see
   *  https://vuejs.org/api/reactivity-core.html#reactive.
   *  Without this wrap, the array and its items stay as
   *  plain objects, and a per-item mutation like
   *  `last.latency = { totalMs, ... }` (in the `done`
   *  handler) or `target.latency = { totalMs, ... }` (in
   *  `reloadAfterFinalize`) writes through a plain object
   *  with no proxy in the way — Vue's effect tracker never
   *  sees the change, and the `currentSessionLatencyTurns`
   *  computed in chat.ts (which iterates the array and
   *  reads `m.latency`) never re-evaluates. Symptom: the
   *  cumulative chip in the ChatInput popover showed
   *  "累计 10.1s" but "轮次 0" because `accumulateLatency`
   *  fires the *outer* Map's set trap (which IS tracked)
   *  while per-message `latency` assignment does not.
   *
   *  Wrapping here is safe for both code paths:
   *  - `ensureLoaded` / `reloadAfterFinalize` call us with
   *    a fresh `rehydrateMessages(loaded.messages)` array
   *    of plain objects; `reactive()` deep-wraps them.
   *  - The streaming path's `msgs.push(userMsg, assistantMsg)`
   *    (in chat.ts) mutates the wrapped array; the new
   *    items get wrapped on the proxy's set trap.
   *  - `markRaw`d nested fields (toolCalls / toolResults /
   *    thinkingBlocks / redactedThinkingData) skip the
   *    wrap, preserving the existing memory-shape contract.
   *
   *  Cost: one `reactive()` call per putMessages (cheap —
   *  Vue 3 wraps lazily on property access). */
  function putMessages(
    sessionId: string,
    messages: ChatMessage[],
    pinned: boolean,
  ): void {
    const had = messagesBySession.has(sessionId);
    if (had) {
      // Touch: move to MRU by delete + set so the Map's iteration
      // order (and thus the eviction order in `evictIfNeeded`)
      // reflects the new recency.
      messagesBySession.delete(sessionId);
    }
    messagesBySession.set(sessionId, reactive(messages));
    if (pinned) pinnedSessions.add(sessionId);
    evictIfNeeded();
  }

  /** Drop the LRU non-pinned entry if the cache is over budget.
   *  Walks insertion order from the oldest; pinned entries are
   *  skipped (so an over-budget cache that is fully pinned is
   *  tolerated — streaming sessions are sacred). */
  function evictIfNeeded(): void {
    if (messagesBySession.size <= CACHE_SIZE) return;
    for (const [key] of messagesBySession) {
      if (pinnedSessions.has(key)) continue;
      messagesBySession.delete(key);
      return;
    }
  }

  // --- 交错思考(实时态 contentBlocks 维护) -----------------------------
  // 实时流式时同步构建 `last.contentBlocks`,让 MessageItem 的
  // renderTimeline 按真实流序交错渲染 thinking + text(而非 reload 后
  // 才有),且与 reload 后 rehydrate 的形态**逐块一致**。
  //
  // 设计(镜像后端 chat_loop.rs 的 ordered_blocks):
  //   - thinking: 就地 mutate —— 第一个 thinking_delta 在 contentBlocks push
  //     一个空 thinking 块并记其索引为 `activeThinkingIdx`;后续 thinking_delta
  //     /signature_delta 直接 append 到该块。这样 contentBlocks 从第一个
  //     思考 token 就非空(useTimeline 立刻 true,不闪回顶部 ThinkingBlock)。
  //   - 块边界 = "thinking ↔ 非 thinking(text/tool/redacted)切换",**不依赖
  //     signature** —— 与后端 flush_pending_thinking 一致(Delta/ToolCall 时
  //     flush,signature 只附属)。相邻 thinking(think1→sig1→think2,无中间
  //     文本)合并成一块,和后端落库/rehydrate 一致。
  //   - text: 按段累积到 pendingTimelineText,遇边界 flush(避免碎片)。
  //   - tool_use/tool_result/redacted 作为独立块按到达顺序 push。
  function ensureContentBlocks(m: ChatMessage): ContentBlockView[] {
    if (!m.contentBlocks) m.contentBlocks = [];
    return m.contentBlocks;
  }

  /** Seal 当前正在累积的 thinking 块(若有): 清空 activeThinkingIdx,使下次
   *  thinking_delta 开新块。在非思考边界(delta/tool/redacted/done/error)
   *  调用。不删除块 —— 块已在 contentBlocks 里就地累积完毕。 */
  function sealActiveThinking(req: RequestState): void {
    req.activeThinkingIdx = null;
  }

  /** 把累积中的 pending text flush 成一个 text 块进 contentBlocks。
   *  在遇到 thinking/tool/redacted 边界时调用,保持"文本与思考交错"的
   *  真实流序。空文本不 push(避免空块)。 */
  function flushPendingTimelineText(req: RequestState, m: ChatMessage): void {
    if (req.pendingTimelineText !== null && req.pendingTimelineText !== "") {
      ensureContentBlocks(m).push({ kind: "text", text: req.pendingTimelineText });
    }
    req.pendingTimelineText = null;
  }

  // ---------------------------------------------------------------------
  // Event handlers (one global listener; routes by request_id)
  // ---------------------------------------------------------------------

  function handleChatEvent(event: ChatEventPayload): void {
    const req = activeRequests.get(event.request_id);
    if (!req) return; // event for unknown / already-finished request — drop
    const msgs = messagesBySession.get(req.sessionId);
    if (!msgs) return; // session was evicted mid-stream — shouldn't happen because pinned, but guard
    const last = msgs[msgs.length - 1];
    if (!last || last.role !== "assistant") return;

    switch (event.kind) {
      case "start":
        // F5 follow-up: every turn emits Start now (the agent
        // loop drops the `if turn == 1` guard on
        // `agent/chat.rs:422-425`). Each Start is the boundary
        // between turns — increment `currentTurnIndex` so the
        // next `TurnComplete` writes to the right slot in
        // `latencyByTurn`. The first Start moves -1 → 0 (turn 0
        // starts). The 4 per-turn close boundaries below
        // (`delta` / `tool:call` / `done` / `error`) reset
        // `thinkingDurationMs`-equivalent state on each new
        // turn — see the `RequestState` comment for the full
        // close-trigger list.
        req.currentTurnIndex++;
        last.streaming = true;
        last.error = undefined;
        // 交错思考(实时态): 新 turn 边界 —— seal 上一 turn 可能仍活动的
        // thinking 块 + flush 残留 text(若上一 turn 以纯思考结束,无后续
        // delta/tool 触发 seal)。防止下一 turn 的 thinking_delta 复用
        // 已失效的 activeThinkingIdx(数组可能已被 reloadAfterFinalize
        // 替换,索引越界)。
        sealActiveThinking(req);
        flushPendingTimelineText(req, last);
        // A5+ (2026-07-04): a fresh turn started — the prior
        // retry notice (if any) is stale. Clear it so the
        // MessageItem row disappears the moment the stream
        // resumes after a successful retry.
        last.retrying = undefined;
        break;
      case "delta":
        // 交错思考(实时态): 文本到达 = 思考块边界,seal 活动 thinking 块
        // (思考在文本之前;块边界 = thinking↔非thinking 切换,镜像后端
        // flush_pending_thinking 在 Delta 时 flush)。
        sealActiveThinking(req);
        // F5: capture the first-delta timestamp exactly once,
        // on the very first `delta` event. Subsequent deltas
        // see `firstDeltaAt` already set and skip the write.
        // The TTFB is computed in the `done` handler as
        // `firstDeltaAt - sendAt`.
        if (event.text) last.content += event.text;
        // 交错思考(实时态): 文本按段累积到 pendingTimelineText,
        // 等 thinking/tool/redacted 边界再 flush 成 text 块进
        // contentBlocks(避免每段 delta 一个碎片块)。
        if (event.text) {
          req.pendingTimelineText = (req.pendingTimelineText ?? "") + event.text;
        }
        // A5+ (2026-07-04): real content arrived — the retry
        // succeeded. Clear the transient notice so the
        // MessageItem row disappears (the bubble takes over).
        if (last.retrying) last.retrying = undefined;
        if (req.firstDeltaAt === null) {
          req.firstDeltaAt = Date.now();
        }
        // F5 follow-up per-turn: the close-boundary timer
        // snapshot (`req.thinkingDurationMs = ...`) is gone.
        // The placeholder's `last.thinkingDurationMs` is
        // set by the `case "turn_complete"` handler with
        // the backend's per-turn `thinking_ms` value
        // (computed from the agent loop's per-turn
        // `turn_thinking_done - turn_thinking_start`
        // `Instant` pair). A `text delta` no longer needs
        // to close the thinking timer on the frontend —
        // the backend's `ChatEvent::Delta` arm closes it
        // there.
        break;
      case "thinking_delta":
        // 交错思考(实时态): 一个 thinking 块开始前,先把之前累积的
        // 文本 flush 成 text 块(思考夹在文本之间时,前段文本应排在思考前)。
        flushPendingTimelineText(req, last);
        // 就地 mutate contentBlocks 里的活动 thinking 块(若有),否则
        // 开新块。contentBlocks 从第一个思考 token 就非空 → useTimeline
        // 立刻 true,不闪回顶部 ThinkingBlock(消除 "· N blocks" 计数闪现)。
        // 块边界 = 非思考事件 seal(activeThinkingIdx=null),不依赖 signature
        // —— 与后端 flush_pending_thinking 一致,实时态/reload 拆分相同。
        if (event.text) {
          const blocks = ensureContentBlocks(last);
          if (req.activeThinkingIdx === null) {
            blocks.push({ kind: "thinking", text: "", signature: "" });
            req.activeThinkingIdx = blocks.length - 1;
          }
          const tb = blocks[req.activeThinkingIdx];
          if (tb.kind === "thinking") tb.text += event.text;
          // 注: 不再双写 thinkingBlocks —— timeline 路径(useTimeline=true)
          // 只读 contentBlocks,thinkingBlocks 双写会导致两个 store 漂移。
        }
        // F5 follow-up per-turn: the `req.thinkingStartedAt =
        // Date.now()` start-of-thinking stamp is gone. The
        // backend `ChatEvent::ThinkingDelta` arm opens its
        // per-turn `turn_thinking_start: Option<Instant>`
        // there; the corresponding `turn_thinking_done` is
        // set on the first non-thinking boundary. The
        // duration comes back through `ChatEvent::TurnComplete`
        // and is written to `last.thinkingDurationMs` by the
        // `case "turn_complete"` handler. The frontend no
        // longer maintains per-turn wall-clock state for
        // thinking (it has the agent loop for that now).
        break;
      case "signature_delta":
        // signature 附加到活动 thinking 块(若有)。不作为 push/seal 时机
        // —— signature 是 Anthropic 专有,OpenAI 不发;块边界由非思考事件
        // 决定(见 thinking_delta 的 sealActiveThinking 调用点)。
        if (event.signature) {
          const blocks = ensureContentBlocks(last);
          if (req.activeThinkingIdx === null) {
            blocks.push({ kind: "thinking", text: "", signature: "" });
            req.activeThinkingIdx = blocks.length - 1;
          }
          const tb = blocks[req.activeThinkingIdx];
          if (tb.kind === "thinking") tb.signature += event.signature;
        }
        break;
      case "redacted_thinking_delta":
        if (event.data) {
          if (!last.redactedThinkingData) last.redactedThinkingData = [];
          last.redactedThinkingData.push(event.data);
          // 交错思考(实时态): redacted 按到达顺序进 contentBlocks。
          // 先 seal 活动 thinking + flush pending text,保持流序。
          sealActiveThinking(req);
          flushPendingTimelineText(req, last);
          ensureContentBlocks(last).push({ kind: "redacted_thinking", data: event.data });
        }
        break;
      case "turn_complete": {
        // F5 follow-up per-turn: the agent loop emits this
        // event right after `persist_turn` for the assistant
        // row. The `seq` field is the assistant row's seq
        // (assigned by the agent loop in the per-session
        // `next_seq` counter) — used by `reloadAfterFinalize`
        // to fire the `update_message_latency` IPC for THIS
        // turn's row (N per request, not 1 as the F5
        // "max seq" path did). The 4 ms fields are
        // `Option<i64>` server-side; `null` for turns that
        // never reached the corresponding boundary.
        // The `{ }` wraps the case body so the `const`
        // declarations don't leak into the next `case`
        // block (TypeScript "Cannot redeclare
        // block-scoped variable" if a sibling case
        // happens to define `turnLatency` etc.).
        if (typeof event.seq !== "number") {
          // Defensive — the Rust side always sends `seq`
          // for `turn_complete`, but if a future wire
          // change drops it, drop the event rather than
          // corrupt the Map.
          break;
        }
        const turnLatency: TurnLatency = {
          seq: event.seq,
          ttfbMs: event.ttfb_ms ?? null,
          genMs: event.gen_ms ?? null,
          totalMs: event.total_ms ?? null,
          thinkingMs: event.thinking_ms ?? null,
        };
        req.latencyByTurn.set(req.currentTurnIndex, turnLatency);
        // In-place mutate the streaming placeholder's
        // `latency` / `thinkingDurationMs` for instant UI
        // feedback (no reload needed). The `reactive(Map)`
        // set trap in `chat.ts` `putMessages` (F5 commit
        // `74e43e4` fix) fires `currentSessionLatencyTurns`
        // computed re-eval, so the ChatInput popover
        // updates in real-time per turn.
        if (
          turnLatency.ttfbMs !== null ||
          turnLatency.genMs !== null ||
          turnLatency.totalMs !== null
        ) {
          last.latency = {
            ...(turnLatency.ttfbMs !== null
              ? { ttfbMs: turnLatency.ttfbMs }
              : {}),
            ...(turnLatency.genMs !== null
              ? { genMs: turnLatency.genMs }
              : {}),
            ...(turnLatency.totalMs !== null
              ? { totalMs: turnLatency.totalMs }
              : {}),
          };
        }
        if (turnLatency.thinkingMs !== null) {
          last.thinkingDurationMs = turnLatency.thinkingMs;
        }
        // Per-session cumulative: each turn contributes
        // its own `totalMs`. Matches the A4
        // `accumulateTokenUsage` per-done pattern. The
        // `done` handler no longer fires
        // `accumulateLatency` — `TurnComplete` is the
        // single source of per-turn latency.
        if (turnLatency.totalMs !== null) {
          useChatStore().accumulateLatency(
            req.sessionId,
            turnLatency.totalMs,
          );
        }
        break;
      }
      case "done":
        // F5 follow-up per-turn: the `done` handler is the
        // stream-termination signal. It does NOT compute or
        // write per-turn latency values — those are the
        // `turn_complete` handler's job (it ran earlier for
        // every persisted turn and wrote `last.latency` /
        // `last.thinkingDurationMs` from the backend's
        // per-turn `Instant`-derived values). The
        // `done` handler's responsibilities:
        //
        // - Set `last.streaming = false` to extinguish the
        //   blinking ▍ cursor in MessageItem.vue.
        // - markRaw the four deep-payload arrays (they're
        //   done mutating).
        // - Fire `accumulateTokenUsage` (A4 — the per-turn
        //   usage report rides the `done` payload).
        // - Reset `forceFollowActive` (F2).
        // - `finalizeRequest` (moves the request from
        //   `activeRequests` to `completedRequests` and
        //   kicks off the async `reloadAfterFinalize` →
        //   re-reads DB, re-attaches per-turn latency from
        //   `req.latencyByTurn` to the rehydrated messages,
        //   and fires N `update_message_latency` IPCs).
        // F5 follow-up per-turn: the `last.latency` write
        // is GONE (was here as the F5 "last turn fast
        // path"). Reason: `turn_complete` already wrote
        // `last.latency` with the backend's per-turn
        // values (computed from `agent/chat.rs`
        // per-turn `Instant` timestamps, NOT the
        // frontend's `Date.now()`). The `done` handler's
        // `ttfbMs = firstDeltaAt - sendAt` would OVERWRITE
        // the backend-precise value with a frontend-DOM
        // wall-clock measurement (a different time base).
        // For multi-turn responses this is critical: the
        // LAST turn's `last.latency` must match what
        // `TurnComplete` emitted, not a stale
        // `firstDeltaAt` from the first turn (the
        // `firstDeltaAt` field is per-request, not
        // per-turn — its value is set on the first delta
        // of the request and never reset between turns,
        // so the `done`-handler computation drifts from
        // the actual final turn's TTFB).
        //
        // `accumulateTokenUsage` (A4) still fires from
        // `done` — that path reads the per-turn usage
        // from the `done` payload, NOT from a request-
        // level Map, so it's per-turn correct.

        // F5 follow-up per-turn: `accumulateLatency` is
        // fired by the `case "turn_complete"` handler (one
        // fire per turn) — NOT by `done` (which would
        // double-count for multi-turn responses and miss
        // turns that errored before reaching `done`).
        // The single source for per-turn latency is
        // `TurnComplete`; `done` is the stream-termination
        // signal only.
        //
        // `last.latency` / `last.thinkingDurationMs` are
        // NOT touched here (turn_complete already wrote
        // them). The error path (no turn_complete) writes
        // them in its own case below.
        //
        // `reloadAfterFinalize` later re-attaches
        // per-turn from `req.latencyByTurn` onto the
        // rehydrated (per-turn split) messages.
        //
        // The `req.latencyPending` stash is gone — the
        // `reloadAfterFinalize` reads `req.latencyByTurn`
        // directly.
        // CRITICAL (PR3 self-check fix): the old chat.ts handler
        // set `last.streaming = false` here, which extinguishes the
        // blinking ▍ cursor in MessageItem.vue (rendered under
        // `v-if="message.streaming"`) and lets the markdown
        // pipeline `flush()` the final frame (watch on streaming
        // in MessageItem.vue). Forgetting it leaves the cursor
        // blinking forever after the stream completes — a
        // regression that violates AC6.3 ("streaming=false,光标消失").
        last.streaming = false;
        // A5+ (2026-07-04): terminal event — the retry row (if
        // somehow still set, e.g. the last attempt succeeded but
        // no `delta` ever arrived — a pure tool_use turn) MUST
        // clear so the chip doesn't linger post-stream.
        last.retrying = undefined;
        // 交错思考(实时态): stream 结束兜底 —— seal 活动 thinking + flush
        // 残留的 pending text。thinking 块已就地累积在 contentBlocks,无需
        // 再 push;text 段(text 之后直接 done)靠这里 flush。
        sealActiveThinking(req);
        flushPendingTimelineText(req, last);
        // Stream is over — the four deep-payload arrays stop
        // mutating. markRaw them now so future reads (and the
        // rehydrate path on session reload) skip the reactive
        // proxy. This pairs with the markRaw in rehydrateMessages;
        // together they cover both "loaded from DB" and
        // "just-finished streaming" code paths.
        if (last.toolCalls) markRaw(last.toolCalls);
        if (last.toolResults) markRaw(last.toolResults);
        if (last.thinkingBlocks) markRaw(last.thinkingBlocks);
        if (last.redactedThinkingData) markRaw(last.redactedThinkingData);
        // 交错思考: contentBlocks 也已构造完毕,markRaw 同理。
        if (last.contentBlocks) markRaw(last.contentBlocks);
        // 2026-06-26 snapshot fix: per-turn usage report arrives
        // on the `done` event. Hand the payload off to the chat
        // store which owns the per-session LAST-TURN snapshot
        // (rendered by ChatInput.vue's hint area). Snapshot
        // semantics: OVERWRITE (not accumulate).
        if (event.usage) {
          // 2026-06-26 snapshot fix: build a complete
          // `SessionTokenUsage`. `context_input_tokens` is
          // optional on the wire (legacy backends) — fall back to
          // `input + cache_creation + cache_read` (the Anthropic
          // normalization). For OpenAI sources the backend already
          // sends the field (= prompt_tokens), so the fallback is
          // only hit on the legacy wire shape.
          const u = event.usage;
          useChatStore().setLastTurnUsage(req.sessionId, {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            cache_creation_input_tokens: u.cache_creation_input_tokens,
            cache_read_input_tokens: u.cache_read_input_tokens,
            context_input_tokens:
              u.context_input_tokens ??
              u.input_tokens +
                u.cache_creation_input_tokens +
                u.cache_read_input_tokens,
          });
        }
        // F2: reset force-follow mode when the stream finishes.
        useChatStore().forceFollowActive = false;
        finalizeRequest(req.requestId, req.sessionId, false);
        break;
      case "error":
        last.streaming = false;
        last.error = {
          message: event.message ?? "未知错误",
          category: event.category ?? "server",
        };
        // A5+ (2026-07-04): terminal event — clear the retry
        // chip on terminal failure too (otherwise the row
        // would linger on the bubble even though `last.error`
        // is now showing).
        last.retrying = undefined;
        // F5: error path. The `totalMs` is still recorded
        // (user wants to see "在 X 秒时断了"), but `ttfbMs`
        // and `genMs` may be `null` (no delta arrived).
        {
          const doneAt = Date.now();
          const sendAt = req.sendAt;
          const firstDeltaAt = req.firstDeltaAt;
          const ttfbMs = firstDeltaAt !== null ? firstDeltaAt - sendAt : null;
          const genMs =
            firstDeltaAt !== null ? doneAt - firstDeltaAt : null;
          const totalMs = doneAt - sendAt;
          last.latency = {
            ...(ttfbMs !== null ? { ttfbMs } : {}),
            ...(genMs !== null ? { genMs } : {}),
            totalMs,
          };
          // F5 follow-up: error path also closes the
          // thinking timer if it's still open (e.g. the
          // F5 follow-up per-turn: the close-thinking-timer
          // branch and the `last.thinkingDurationMs` write
          // are gone. The error path never fires
          // `TurnComplete` (the agent loop bails out of
          // the outer loop on `had_error` and never reaches
          // `persist_turn`), so the placeholder's
          // `thinkingDurationMs` stays `undefined` for
          // errored turns — the ThinkingBlock header
          // falls back to "—". This is the correct
          // semantic: a turn that errored before
          // persisting has no per-turn thinking duration
          // to record.

          // Per-session cumulative: error turns also count
          // toward the displayed total (the user can see
          // "I spent 5s on this prompt and it errored out").
          // The F5 follow-up `turn_complete` path does NOT
          // fire on error (the agent loop bails out of the
          // outer loop on `had_error` and never reaches
          // `persist_turn`, so no `TurnComplete` is emitted) —
          // the error handler is the single fire for an
          // errored turn. Matches the A4 `accumulateTokenUsage`
          // per-done pattern.
          useChatStore().accumulateLatency(req.sessionId, totalMs);
          // F5 follow-up: the `req.latencyPending` stash is
          // gone. `reloadAfterFinalize` reads from
          // `req.latencyByTurn` (or, for the no-turn case
          // where the request errored before reaching
          // `TurnComplete`, fires no IPC — there's no
          // persisted assistant row to UPDATE). The error
          // path's in-memory `last.latency` /
          // `last.thinkingDurationMs` writes above are the
          // only place these values live.
        }
        // Same post-stream markRaw — the error case is terminal
        // just like `done`, the arrays won't grow further.
        sealActiveThinking(req);
        flushPendingTimelineText(req, last);
        if (last.toolCalls) markRaw(last.toolCalls);
        if (last.toolResults) markRaw(last.toolResults);
        if (last.thinkingBlocks) markRaw(last.thinkingBlocks);
        if (last.redactedThinkingData) markRaw(last.redactedThinkingData);
        if (last.contentBlocks) markRaw(last.contentBlocks);
        // F2: reset force-follow on error too.
        useChatStore().forceFollowActive = false;
        finalizeRequest(req.requestId, req.sessionId, true);
        break;
      case "file_injections": {
        // B2 PR3: the agent loop emitted a per-user-turn
        // `@relpath` injection manifest. Patch the
        // matching user message's `injections` array so
        // the hint row under the user bubble renders
        // immediately (without waiting for the assistant
        // response to surface it). The lookup is by
        // `request_id` (active request guard — the
        // outer `activeRequests.get(event.request_id)`
        // above already filtered to the right request)
        // + `message_seq` (the seq the agent loop
        // assigned to the user row in its per-session
        // `next_seq` counter). `msgs.find` walks the
        // full buffer; in practice the user row sits
        // 1-2 slots before the assistant placeholder,
        // so the linear scan is cheap (1-3 items).
        if (
          typeof event.message_seq !== "number" ||
          !Array.isArray(event.injections)
        ) {
          // Defensive: the Rust side always sends
          // both fields for `file_injections`, but if
          // a future wire change drops them, drop the
          // event rather than corrupt the buffer.
          break;
        }
        const targetSeq = event.message_seq;
        const target = msgs.find(
          (m) => m.role === "user" && m.seq === targetSeq,
        );
        if (target) {
          target.injections = event.injections;
        }
        // else: could happen if the user navigated
        // away before the agent loop got to emit the
        // event — the session is pinned during the
        // request so the buffer survives, but
        // `rehydrateMessages` rebuilds it post-`done`
        // from the DB and picks up the manifest from
        // `messages.metadata` there.
        break;
      }
      case "retrying": {
        // A5+ (2026-07-04, R8): the agent loop's `LlmRetrySink`
        // emitted this notice before the next backoff sleep. The
        // user sees "↩ 重试中 N/M, Ts 后重发… (reason)" instead of a
        // multi-second frozen-looking stream. Attach to the
        // in-flight assistant placeholder's transient `retrying`
        // field — the MessageItem renders the row above the
        // bubble. The field is cleared by the next `start` /
        // `delta` / `done` / `error` event (the retry either
        // succeeds and resumes the stream, fails terminally, or
        // the user cancels).
        if (
          typeof event.attempt !== "number" ||
          typeof event.max_attempts !== "number" ||
          typeof event.wait_ms !== "number" ||
          typeof event.reason !== "string"
        ) {
          // Defensive — the Rust side always sends all four
          // fields for `retrying`, but a future wire change
          // dropping one would crash the renderer on `.toFixed`
          // etc. Drop the event rather than risk it.
          break;
        }
        last.retrying = {
          attempt: event.attempt,
          maxAttempts: event.max_attempts,
          waitMs: event.wait_ms,
          reason: event.reason,
        };
        break;
      }
      case "recall": {
        // 07-06 (am-observability-panel R2b/A7): the agent loop
        // emitted a recall notice (FTS at turn start, or pitfall on
        // a tool-call dispatch). Route the hits into the memory
        // store — the ChatPanel recall chip reads
        // `recallHitsForSession(currentSessionId)`. We do NOT attach
        // anything to the message buffer (the hit list is transient,
        // per-turn; it must not pollute the persisted DB shape —
        // same rationale as `retrying`). The store accumulates; a
        // new user message (`startRequest`) clears the slice.
        if (!Array.isArray(event.hits)) {
          // Defensive — the Rust side always sends `hits` for
          // `recall`, but a future wire change dropping it would
          // push `undefined` into the store. Drop the event.
          break;
        }
        useMemoryStore().pushRecallHits(req.sessionId, event.hits);
        break;
      }
      case "context_compacted": {
        // E2 (harness trace pipeline, 2026-07-14): the C3
        // context-compaction trace event. Routes into the
        // trace store so the live panel can render the
        // "compaction" sub-card on the matching TurnCard. The
        // controller's outer `activeRequests.get(request_id)`
        // filter ensures the event belongs to an in-flight
        // main-loop request; the trace store's `applyEvent`
        // is session-agnostic (the `seq` is the per-session
        // turn counter, which is unique within a session).
        useTraceStore().applyEvent({
          kind: "context_compacted",
          request_id: event.request_id,
          seq: event.seq ?? 0,
          tokens_before: event.tokens_before ?? 0,
          tokens_after: event.tokens_after ?? 0,
          dropped_count: event.dropped_count ?? 0,
          degradation: event.degradation ?? "none",
        });
        break;
      }
      case "loop_hint": {
        // E2: C2 loop-detection soft hint (1-2 consecutive
        // hits). Same flow as `context_compacted`.
        useTraceStore().applyEvent({
          kind: "loop_hint",
          request_id: event.request_id,
          seq: event.seq ?? 0,
          hit_count: event.hit_count ?? 0,
          verdict_kind: event.verdict_kind ?? "soft",
        });
        break;
      }
      case "workflow_breadcrumb": {
        // E2: per-turn workflow breadcrumb snapshot. The
        // `task_slug` / `status` are `null` when there's no
        // active workflow task (the bootstrap breadcrumb
        // branch); the renderer shows the breadcrumb_text
        // verbatim and hides the slug/status fields.
        useTraceStore().applyEvent({
          kind: "workflow_breadcrumb",
          request_id: event.request_id,
          seq: event.seq ?? 0,
          task_slug: event.task_slug ?? null,
          status: event.status ?? null,
          breadcrumb_text: event.breadcrumb_text ?? "",
        });
        break;
      }
    }
  }

  function handleToolCall(payload: ToolCallPayload): void {
    const req = activeRequests.get(payload.request_id);
    if (!req) return;
    const msgs = messagesBySession.get(req.sessionId);
    if (!msgs) return;
    const last = msgs[msgs.length - 1];
    if (!last || last.role !== "assistant") return;
    if (!last.toolCalls) last.toolCalls = [];
    last.toolCalls.push({ id: payload.id, name: payload.name, input: payload.input });
    // 交错思考(实时态): tool_use 按到达顺序进 contentBlocks。先 seal 活动
    // thinking + flush pending text(它们应排在工具调用之前),保持流序。
    sealActiveThinking(req);
    flushPendingTimelineText(req, last);
    ensureContentBlocks(last).push({
      kind: "tool_use",
      id: payload.id,
      name: payload.name,
      input: payload.input,
    });
    // B12 Checklist (PR2 frontend, 2026-06-19): route the
    // `update_checklist` tool_use to the checklist store so the
    // floating `<ChecklistCard>` overlay updates live. The store
    // parses `input.items` and re-coerces at-most-one
    // `in_progress` client-side (mirroring PR1's Rust coerce). We
    // do this BEFORE the F5 timestamp stamp so the card appears
    // immediately on the tool_use event (the LLM-side result text
    // is purely for the LLM; the UI reads the structured input).
    if (payload.name === CHECKLIST_TOOL_NAME) {
      useChecklistStore().handleToolCall(
        req.sessionId,
        payload.name,
        payload.input,
      );
    }
    // C2 (review visualization view, 2026-07-26): route
    // `write_file` tool:call events whose `input.path` hits the
    // current task's `review-state.json` to the review-state
    // store. The store slug-gates + debounces (200ms) then
    // re-reads via `get_review_state`. No backend event — C3
    // cut `emit_review_state_updated`, so refresh is 100%
    // frontend-driven off this global tool:call listener
    // (design.md §2). Mirrors the B12 checklist route above.
    if (payload.name === "write_file") {
      const path = payload.input?.path;
      const reviewStore = useReviewStateStore();
      const slug = reviewStore.currentSlugForRouting;
      if (
        typeof path === "string" &&
        slug &&
        matchesReviewStatePath(path, slug)
      ) {
        reviewStore.handleReviewStateWritten(req.sessionId, slug);
      }
    }
    // F5: capture the start timestamp for the per-tool
    // duration. The matching `tool:result` reads it, computes
    // `durationMs = now - toolStartedAt`, and writes it onto
    // the in-memory `toolResult` + fires the
    // `record_tool_duration` IPC to persist the patch into
    // the `messages.content` JSON's `tool_result` block.
    // Stale entries (no `tool:result` ever arrived — cancel
    // mid-tool) are harmless: the Map is dropped with the
    // request state on `finalizeRequest`.
    req.toolStartedAt.set(payload.id, Date.now());
    // F5 follow-up: a `tool:call` arriving without an
    // intervening text `delta` means the model went
    // F5 follow-up per-turn: the close-thinking-timer
    // branch is gone. The `tool:call` boundary is closed
    // on the backend (the agent loop's `ChatEvent::ToolCall`
    // arm sets `turn_thinking_done = Some(Instant::now())`),
    // and the duration ships back through the corresponding
    // `ChatEvent::TurnComplete` event. The placeholder's
    // `last.thinkingDurationMs` is written by the
    // `turn_complete` case, not here.
  }

  function handleToolResult(payload: ToolResultPayload): void {
    const req = activeRequests.get(payload.request_id);
    if (!req) return;
    const msgs = messagesBySession.get(req.sessionId);
    if (!msgs) return;
    const last = msgs[msgs.length - 1];
    if (!last || last.role !== "assistant") return;
    if (!last.toolResults) last.toolResults = [];
    // F5: compute the per-tool duration. If the matching
    // `tool:call` never set a timestamp (defensive — the
    // events could in principle be out-of-order on a buggy
    // SSE stream), the duration stays `undefined` and the
    // ToolCallCard renders no time; the IPC is also skipped.
    const start = req.toolStartedAt.get(payload.tool_use_id);
    let durationMs: number | undefined;
    if (typeof start === "number") {
      durationMs = Math.max(0, Date.now() - start);
    }
    last.toolResults.push({
      toolUseId: payload.tool_use_id,
      content: payload.content,
      isError: payload.is_error,
      ...(durationMs !== undefined ? { durationMs } : {}),
    });
    // 交错思考(实时态): tool_result **不进** assistant contentBlocks ——
    // 对齐 reload 后(rehydrate 的 assistant contentBlocks 只有 tool_use,
    // tool_result 在 user-role 行;后端红线:ToolResult 永不进 assistant)。
    // seal 活动 thinking + flush pending text(若有),保持 thinking/text 流序。
    sealActiveThinking(req);
    flushPendingTimelineText(req, last);
    // 注:tool_result 不 push 进 contentBlocks(否则实时态多出来,reload 后
    // 消失,造成前后不一致)。toolResults 分桶数组仍正常累积(工具卡片用它)。
    // F5: persist the duration into `messages.content` JSON
    // (the `tool_result` block). Fire-and-forget; a failure
    // logs but doesn't surface to the user. The in-memory
    // value is what the UI shows.
    if (durationMs !== undefined) {
      void transport.invoke("record_tool_duration", {
        sessionId: req.sessionId,
        toolUseId: payload.tool_use_id,
        durationMs,
      }).catch((e) => {
        console.error(
          "[streamController] record_tool_duration failed:",
          e,
        );
      });
    }
  }

  /** Phase C3 (2026-06-30, `ask_user_question` task): pull the
   *  authoritative pending-interaction state from the backend's
   *  QuestionStore via the `get_pending_interaction` IPC
   *  (Phase B4, 2026-07-07 — supersedes the deprecated
   *  `get_pending_question`).
   *
   *  Source-of-truth correction (design §5.4): the live
   *  `tool:question` / `mode:change:request` listeners are the
   *  optimistic push side; this pull is the authoritative
   *  side. Called from `ensureLoaded` (every reload, including
   *  the cache-hit early-return path — that's the LRU
   *  correction point) and `reloadAfterFinalize` (every stream
   *  completion — corrects stale cache after resolve/cancel).
   *
   *  Behavior:
   *    - Backend returns `Some(entry)` → overwrite cache with
   *      the tagged `PendingInteraction`. Both Question and
   *      ModeChange variants flow through the same per-session
   *      Map; the card dispatch reads `entry.kind` to pick the
   *      right component.
   *    - Backend returns `None` → remove cache entry (backend
   *      says "no pending" — correct any drift from a stale
   *      optimistic push).
   *    - IPC fails (defensive) → swallow the error. The
   *      optimistic push already populated the cache when the
   *      live event arrived, so a missed pull costs us
   *      staleness, not missing state. The card will simply
   *      render whatever the cache had (a future
   *      `ensureLoaded` will retry the pull).
   *
   *  Why this lives in its own function: three call sites
   *  (`ensureLoaded` cache-hit, `ensureLoaded` cache-miss,
   *  `reloadAfterFinalize`) all need the same pull logic. The
   *  helper enforces single-source-of-truth (any future fix —
   *  e.g. debouncing, batching — lands here once). */
  async function reconcilePendingInteractionFromBackend(
    sessionId: string,
  ): Promise<void> {
    try {
      const entry = await transport.invoke<PendingInteraction | null>(
        GET_PENDING_INTERACTION_CMD,
        { sessionId },
      );
      const cards = useQuestionCardsStore();
      if (entry) {
        cards.addPending(sessionId, entry);
      } else {
        cards.removePending(sessionId);
      }
    } catch {
      // Defensive swallow — see comment above.
    }
  }

  /** Phase C3 (2026-06-30, `ask_user_question` task): handler for
   *  the `tool:question` IPC event. Pushes the payload into the
   *  `questionCards` store's per-session cache so the
   *  `<AskUserQuestionCard>` (Phase D) can render it
   *  immediately.
   *
   *  Cache semantics (per design §5.4): this handler is the
   *  OPTIMISTIC push side. The AUTHORITATIVE pull side is
   *  `ensureLoaded` / `reloadAfterFinalize`, which invoke
   *  `get_pending_interaction` to overwrite the cache with the
   *  backend's QuestionStore state. The push side keeps the UI
   *  snappy (no IPC round-trip before the card renders); the
   *  pull side corrects any drift (e.g. session-switch LRU
   *  eviction cleared the cache while the backend's pending
   *  interaction still lives).
   *
   *  Snake_case mapping: the backend's `ToolQuestionPayload`
   *  emits snake_case (no `rename_all`); we wrap it into the
   *  store's tagged-union `PendingInteraction` (also snake_case
   *  inside) — single conversion point, no other call site has
   *  to remember.
   *
   *  Single-pending mutex (PRD R12): the backend's QuestionStore
   *  guarantees only one pending interaction per session. If a
   *  second event somehow arrives (shouldn't happen in
   *  production), `addPending`'s overwrite semantics replace
   *  the prior entry — the new event wins. */
  function handleToolQuestion(payload: ToolQuestionPayload): void {
    // C2+ loop-intervention (chat_loop's ≥3 consecutive loop-detection
    // hits) reuses the `tool:question` channel + `ToolQuestionPayload`
    // shape but carries a synthetic `tool_use_id` of
    // `loop_intervention_{turn}` (no real ask_user_question tool_use
    // block exists). Tag it as `loop_intervention` so the frontend
    // renders it as a FLOATING card (ChatPanel top overlay) instead
    // of trying to anchor it under a non-existent tool_use block —
    // which never matched and silently dropped the intervention
    // (2026-07-28 incident, session e8a1ad96…).
    const isLoopIntervention = payload.tool_use_id.startsWith(
      "loop_intervention_",
    );
    useQuestionCardsStore().addPending(payload.session_id, {
      kind: isLoopIntervention ? "loop_intervention" : "question",
      payload,
    });
    maybeNotifyPending(payload.session_id, "question");
  }

  /** Phase B4 (2026-07-07, `request_mode_change` task): handler
   *  for the `mode:change:request` IPC event. Pushes the
   *  payload into the `questionCards` store's per-session cache
   *  so the `<RequestModeChangeCard>` (Phase C) can render it
   *  immediately.
   *
   *  Mirrors `handleToolQuestion`'s push-side semantics: this
   *  is the OPTIMISTIC push side; the AUTHORITATIVE pull side
   *  is `ensureLoaded` / `reloadAfterFinalize`, which invoke
   *  `get_pending_interaction` to overwrite the cache. Same
   *  single-pending-mutex (QuestionStore.register enforces one
   *  pending per session — a `tool:question` + a
   *  `mode:change:request` for the same session can't both
   *  succeed; the second `register` returns `AlreadyPending`).
   *
   *  Snake_case mapping: the backend's `ModeChangePayload`
   *  emits snake_case (no `rename_all`); we wrap it into the
   *  store's tagged-union `PendingInteraction` verbatim — no
   *  field rename at the IPC boundary. */
  function handleModeChangeRequest(payload: ModeChangePayload): void {
    useQuestionCardsStore().addPending(payload.session_id, {
      kind: "mode_change",
      payload,
    });
    maybeNotifyPending(payload.session_id, "mode_change", payload.target_mode);
  }

  /** Cross-session pending-interaction toast (2026-07-08
   *  `cross-session-pending-indicator`). When a pending mode_change
   *  or question lands on a session OTHER than the current one,
   *  surface a global toast so a user working in a different
   *  session perceives it without switching. Current-session
   *  pendings are NOT toasted — their inline card is already
   *  visible (per Q3: minimize disruption to the active session).
   *
   *  Title: only the current project's sessions live in
   *  `chatStore.sessions`, so a cross-project target falls back to
   *  the generic "另一项目的会话" label (avoids session→project
   *  mapping per Q4). The toast carries `sessionId` so AppShell's
   *  click handler can same-project-jump. */
  function maybeNotifyPending(
    sessionId: string,
    kind: "mode_change" | "question",
    targetMode?: "edit" | "plan" | "yolo",
  ): void {
    const chatStore = useChatStore();
    const n = buildPendingNotification(
      sessionId,
      kind,
      chatStore.currentSessionId,
      chatStore.sessions,
      targetMode,
    );
    if (n) {
      useProjectsStore().showToast(n.message, "info", 6000, {
        sessionId: n.sessionId,
      });
    }
  }

  /** Push a `task:state:transition:request` event payload into the
   *  questionCards store as a tagged-union
   *  `{ kind: "task_state_transition", payload }`. Sibling of
   *  `handleModeChangeRequest` (2026-07-09,
   *  `07-09-workflow-transition-card`). Same single-pending gate +
   *  same authoritative-pull reconciliation. The backend's event
   *  payload is flat snake_case (no `kind` field); we wrap it into
   *  the store's tagged union here — verbatim, no field rename at
   *  the IPC boundary. */
  function handleTaskStateTransition(
    payload: TaskStateTransitionPayload,
  ): void {
    useQuestionCardsStore().addPending(payload.session_id, {
      kind: "task_state_transition",
      payload,
    });
  }

  /** Mark a request as finished: drop from activeRequests, unpin
   *  its session, and reload from DB to replace the streaming buffer
   *  with the per-turn persisted shape.
   *
   *  BUG FIX (06-09-fix-stream-finalize-flash-blank): the old
   *  `evict(sessionId)` removed the in-memory cache entirely,
   *  causing `messages` computed to return `[]` → blank page flash.
   *  The evict was needed to prevent the 2013 wire invariant
   *  (streaming buffer is a single merged assistant message, DB is
   *  per-turn split). The fix: instead of bare evict, reload from
   *  DB and *replace* the buffer atomically. The old streaming
   *  buffer stays visible during the async DB load, so the user
   *  never sees a blank page. When the load completes, `putMessages`
   *  does delete+set in the same synchronous tick (LRU touch), so
   *  Vue batches the update without a visible gap.
   *
   *  The 2013 invariant is preserved because the reload fetches the
   *  per-turn split shape from DB. The diff cache is still
   *  invalidated so the worktree chip reflects post-send state. */
  function finalizeRequest(requestId: string, sessionId: string, _errored: boolean): void {
    // F5: the synchronous cleanup (activeRequests.delete +
    // pinnedSessions.delete + invalidateDiff) is the part
    // that matches the pre-F5 contract — locked by the
    // 2013 wire-invariant test (`finalizeRequest` clears
    // `messagesBySession` and `loadedFromDb` via the
    // follow-up `reloadAfterFinalize`, but the *immediate*
    // teardown of `activeRequests` / `pinnedSessions` is
    // synchronous). Keeping it synchronous also means the
    // existing test suite (which calls `finalizeRequest`
    // and asserts immediate state cleanup) keeps passing.
    //
    // The F5 IPC fire is async — it runs inside
    // `reloadAfterFinalize` after the agent loop's
    // `load_session` roundtrip returns with the assistant
    // row's seq. We move the request state from
    // `activeRequests` to `completedRequests` so the IPC
    // can read the stashed `latencyPending` even after
    // `activeRequests.delete`. The `completedRequests`
    // entry is removed inside `reloadAfterFinalize` after
    // the IPC is fired (or skipped, if there's no
    // latency to persist).
    const req = activeRequests.get(requestId);
    if (req) {
      completedRequests.set(requestId, req);
    }
    activeRequests.delete(requestId);
    pinnedSessions.delete(sessionId);
    useChatStore().invalidateDiff(sessionId);
    // Fire-and-forget: replace streaming buffer with DB version.
    // Old buffer stays visible until DB load completes.
    void reloadAfterFinalize(sessionId, requestId);
  }

  /** Reload a session's messages from DB after a stream finishes.
   *  Replaces the in-memory streaming buffer with the per-turn
   *  persisted shape, preventing the 2013 wire invariant without
   *  causing a blank-page flash.
   *
   *  F5: also captures the `seq` of the assistant message that
   *  the agent loop just persisted, then fires the
   *  `update_message_latency` IPC (carrying the values the
   *  `done` handler stashed on `req.latencyPending`). The
   *  `done` event fires AFTER `persist_turn` returns
   *  (the agent loop emits `done` only after the row is in
   *  place — see `agent::chat::chat`), so the seq is stable
   *  by the time we read it here.
   *
   *  This function also owns the post-`done` cleanup of the
   *  request state (`activeRequests.delete` + `pinnedSessions.delete`).
   *  Moving it here (vs. in `finalizeRequest`) means the
   *  request state is alive for the entire IPC path. */
  async function reloadAfterFinalize(sessionId: string, requestId?: string): Promise<void> {
    const loaded = await transport.invoke<LoadedSession | null>("load_session", {
      sessionId,
    });
    const messages = loaded ? rehydrateMessages(loaded.messages) : [];
    // putMessages does delete+set in same tick (LRU touch) — Vue
    // batches the update so there's no visible blank gap.
    putMessages(sessionId, messages, false);
    loadedFromDb.add(sessionId);
    // F4: notify MessageList to re-scroll after buffer replacement
    // to avoid position jitter.
    useChatStore().scrollAfterReload++;
    // B12 Checklist (PR2 frontend, 2026-06-19): re-derive the
    // session's current checklist from the freshly-loaded DB
    // history. The scan finds the LAST `update_checklist`
    // tool_use whose paired tool_result has `is_error === false`
    // (a cancelled update gets a synthetic is_error: true result;
    // rendering it would freeze the card on the interruption
    // text). Drops any prior live state if no committed checklist
    // exists in history.
    useChecklistStore().rehydrateFromMessages(sessionId, messages);
    // Phase C3 (2026-06-30): same authoritative pull as
    // `ensureLoaded`. Fires after every stream completion —
    // see the helper's doc for the source-of-truth rationale.
    await reconcilePendingInteractionFromBackend(sessionId);
    // F5: persist the per-message latency to the DB. The
    // rehydrated messages carry the seq on each row, so we
    // find the LAST assistant message (the one the agent
    // loop just persisted) and use its seq. The
    // `latencyPending` was stashed on the request by the
    // `done` / `error` handler; if it's null, the request
    // was canceled before any latency was computed (no
    // IPC needed). The request entry itself is now in
    // `completedRequests` (moved there by `finalizeRequest`),
    // not `activeRequests` — we read from there and drop
    // the entry after the IPC fires (or the request
    // becomes obsolete).
    if (requestId) {
      const req = completedRequests.get(requestId);
      // F5 follow-up per-turn: iterate `latencyByTurn`
      // (the per-turn Map populated by `case "turn_complete"`)
      // and fire one `update_message_latency` IPC per entry.
      // N entries → N IPCs (one per assistant row), not the
      // F5 "max seq" single fire. The `done` / `error`
      // handler no longer stashes a `latencyPending`
      // single value — the `turn_complete` handler is
      // the single source for per-turn latency, and
      // `reloadAfterFinalize` reads it back here.
      //
      // Empty-map case: an error path that bailed out
      // before reaching `persist_turn` (no
      // `TurnComplete` emitted) — no IPC fires, no row
      // to UPDATE. The in-memory `last.latency` /
      // `last.thinkingDurationMs` writes from the `error`
      // handler are the only place those values live.
      if (req && req.latencyByTurn.size > 0) {
        const reactiveMessages = messagesBySession.get(sessionId);
        for (const [, turnLatency] of req.latencyByTurn) {
          // Reactivity note (F5 bug fix, kept from
          // the F5 commit `74e43e4`): the `putMessages`
          // call above wraps the rehydrated array in
          // `reactive()` (see `putMessages` doc for the
          // rationale), so `messagesBySession.get(sessionId)`
          // returns a reactive proxy of the array, and
          // `.find(...)` returns a reactive proxy of the
          // matching item. Mutating `target.latency = ...`
          // crosses the proxy's set trap, which fires the
          // effect tracker and re-evaluates the
          // `currentSessionLatencyTurns` computed in
          // chat.ts. Without the wrap, this assignment
          // was a write to a plain object and silently
          // dropped — the cumulative chip in the popover
          // would show "累计 10.1s · 轮次 0".
          //
          // The per-turn target is the rehydrated
          // message whose `seq` matches this turn's
          // `seq` (assigned by the agent loop in the
          // per-session `next_seq` counter). The
          // rehydrated messages carry the seq on each
          // row from the DB; we find by exact match
          // instead of the F5 "max seq" approach so
          // every turn's row gets its own latency.
          if (reactiveMessages) {
            const target = reactiveMessages.find(
              (m) =>
                m.role === "assistant" && m.seq === turnLatency.seq,
            );
            if (target) {
              target.latency = {
                ...(turnLatency.ttfbMs !== null
                  ? { ttfbMs: turnLatency.ttfbMs }
                  : {}),
                ...(turnLatency.genMs !== null
                  ? { genMs: turnLatency.genMs }
                  : {}),
                ...(turnLatency.totalMs !== null
                  ? { totalMs: turnLatency.totalMs }
                  : {}),
              };
              if (turnLatency.thinkingMs !== null) {
                target.thinkingDurationMs = turnLatency.thinkingMs;
              }
            }
          }
          void transport.invoke("update_message_latency", {
            sessionId,
            seq: turnLatency.seq,
            ttfbMs: turnLatency.ttfbMs,
            genMs: turnLatency.genMs,
            totalMs: turnLatency.totalMs,
            thinkingMs: turnLatency.thinkingMs,
          }).catch((e) => {
            console.error(
              "[streamController] update_message_latency failed:",
              e,
            );
          });
        }
      }
      // Drop the completed request from the map now that
      // we've either fired the IPC or decided to skip it.
      // The Map has at most 1-2 entries at any time
      // (in-flight + just-completed), so the size bound
      // is tight.
      completedRequests.delete(requestId);
    }
  }

  // ---------------------------------------------------------------------
  // Public API — listener lifecycle
  // ---------------------------------------------------------------------

  /** Idempotent: registering a second time is a no-op. */
  async function start(): Promise<void> {
    if (listenerWired) return;
    unlistenChat = await transport.listen<ChatEventPayload>(
      "chat-event",
      (payload) => {
        handleChatEvent(payload);
      },
    );
    unlistenTC = await transport.listen<ToolCallPayload>(
      "tool:call",
      (payload) => {
        handleToolCall(payload);
      },
    );
    unlistenTR = await transport.listen<ToolResultPayload>(
      "tool:result",
      (payload) => {
        handleToolResult(payload);
      },
    );
    // Phase C3 (2026-06-30): the `ask_user_question` blocking
    // reverse-question tool emits a `tool:question` event when
    // the backend registers a pending question. The listener
    // pushes the payload into the questionCards store's cache;
    // `ensureLoaded` later corrects the cache via
    // `get_pending_interaction` (the authoritative source —
    // QuestionStore lives in AppState, not in the LRU-bounded
    // messagesBySession, so it survives session-switch reloads
    // intact per PRD R9-R11). Mirrors the listener style of
    // `tool:call` / `tool:result` / `permission:ask` — one
    // global listener, routes by payload fields (session_id)
    // rather than by current session (the whole point of the
    // controller pattern).
    unlistenTQ = await transport.listen<ToolQuestionPayload>(
      TOOL_QUESTION_EVENT,
      (payload) => {
        handleToolQuestion(payload);
      },
    );
    // Phase B4 (2026-07-07, `request_mode_change` task): the
    // `request_mode_change` blocking tool emits a
    // `mode:change:request` event when the backend registers a
    // pending mode change. Same single-pending-gate + same
    // authoritative-pull reconciliation as the question
    // listener above — distinct channel so the listener can be
    // wired independently. The handler pushes the payload into
    // the questionCards store as a tagged-union
    // `{ kind: "mode_change", payload }` so the store's
    // `pendingBySession` carries both variants under one map.
    unlistenMC = await transport.listen<ModeChangePayload>(
      MODE_CHANGE_EVENT,
      (payload) => {
        handleModeChangeRequest(payload);
      },
    );
    // 2026-07-09 (`07-09-workflow-transition-card`): the
    // `request_task_state_transition` blocking tool emits a
    // `task:state:transition:request` event when the backend
    // registers a pending workflow state transition. Third sibling
    // on the same single-pending gate — distinct channel so the
    // listener wires independently. Handler pushes the payload as
    // `{ kind: "task_state_transition", payload }` into the store.
    unlistenTST = await transport.listen<TaskStateTransitionPayload>(
      TASK_STATE_TRANSITION_EVENT,
      (payload) => {
        handleTaskStateTransition(payload);
      },
    );
    listenerWired = true;
    listenerReady.value = true;
  }

  /** Unregister listeners. Called from `onUnmounted` of the
   *  app-root component. After `stop`, `start` may be called
   *  again to re-arm. */
  function stop(): void {
    unlistenChat?.();
    unlistenTC?.();
    unlistenTR?.();
    unlistenTQ?.();
    unlistenMC?.();
    unlistenTST?.();
    unlistenChat = null;
    unlistenTC = null;
    unlistenTR = null;
    unlistenTQ = null;
    unlistenMC = null;
    unlistenTST = null;
    listenerWired = false;
    listenerReady.value = false;
  }

  // ---------------------------------------------------------------------
  // Public API — message buffer access
  // ---------------------------------------------------------------------

  /** Read the messages for a session, touching the LRU so the
   *  session is marked recently-used. Returns `undefined` if
   *  the session isn't in the cache (caller should then call
   *  `ensureLoaded` to populate it). */
  function getMessages(sessionId: string): ChatMessage[] | undefined {
    // PURE READ — do not mutate `messagesBySession` here. This function
    // is called from Vue computeds (`chat.ts` `messages` and
    // `currentSessionLatencyTurns`). The old "LRU touch" (delete +
    // re-set) made those computeds mutate a reactive Map inside their
    // own getter, recursively re-invalidating themselves → Vue's
    // "Maximum recursive updates exceeded" guard fired on every event
    // and the scheduler dropped the DOM update, so streaming deltas
    // never rendered until a full array replacement (session switch)
    // forced re-evaluation. The touch now lives in the non-computed
    // callers (`ensureLoaded`, `startRequest`), which are safe to
    // mutate.
    return messagesBySession.get(sessionId);
  }

  /** Make sure `sessionId` is in the cache. If it's already
   *  there (either from a prior load or from a prior send in
   *  this app session), returns immediately. Otherwise fetches
   *  from the DB and seeds the cache. */
  async function ensureLoaded(sessionId: string): Promise<ChatMessage[]> {
    const existing = getMessages(sessionId);
    if (existing) {
      // Touch LRU (move to MRU) — relocated here from `getMessages`
      // so the Vue computeds that read messages can stay pure (see
      // `getMessages` for the recursive-update rationale). This is a
      // plain async function, not a computed, so mutating the Map is
      // safe.
      messagesBySession.delete(sessionId);
      messagesBySession.set(sessionId, existing);
      // Phase C3 (2026-06-30): even on the early-return (cache-hit)
      // path, we MUST pull the authoritative pending-question state.
      // This is the key LRU correction point — without it, a session
      // whose `messagesBySession` was kept in memory but whose
      // pending question was overwritten by a fresh live event
      // would render the stale cache. The pull is the source of
      // truth; the optimistic push (live listener) is just for
      // snappiness before the pull can complete.
      await reconcilePendingInteractionFromBackend(sessionId);
      return existing;
    }
    const loaded = await transport.invoke<LoadedSession | null>("load_session", {
      sessionId,
    });
    const messages = loaded ? rehydrateMessages(loaded.messages) : [];
    putMessages(sessionId, messages, pinnedSessions.has(sessionId));
    loadedFromDb.add(sessionId);
    // Token usage is seeded from `loadSessions` (chat.ts:393-403)
    // when the project's session list is loaded — that path runs
    // once per project change and covers every session in the
    // project, so by the time `ensureLoaded` fires for a specific
    // session the per-session `tokenUsageBySession` map already
    // holds the DB cumulative values.
    //
    // Previously this function also called `accumulateTokenUsage`
    // here, but that produced a 2× seed (loadSessions seeded the
    // running total, then `ensureLoaded` added the same DB value
    // on top — see DB session 631362ab input_tokens_total=1.69M
    // displaying as 3.4M / 100% in the ChatInput hint). Kept here
    // as a single source-of-truth note so future readers don't
    // re-introduce the double-seed.
    // F5: seed the per-session latency total from the
    // rehydrated messages. We sum `latency.totalMs` over
    // every assistant role (matches the PRD R6 口径:
    // "SUM(total_ms) WHERE session_id = ? AND role =
    // 'assistant' AND total_ms IS NOT NULL"). Pre-F5
    // messages have `latency` undefined; the sum ignores
    // them. The seeded value is added to the running total
    // on every subsequent `done` event via
    // `accumulateLatency`.
    let totalLatencyMs = 0;
    let sawAnyLatency = false;
    for (const m of messages) {
      if (m.role === "assistant" && m.latency && typeof m.latency.totalMs === "number") {
        totalLatencyMs += m.latency.totalMs;
        sawAnyLatency = true;
      }
    }
    if (sawAnyLatency) {
      useChatStore().accumulateLatency(sessionId, totalLatencyMs);
    }
    // B12 Checklist (PR2 frontend, 2026-06-19): re-derive the
    // session's checklist from the just-loaded DB history. Same
    // scan as `reloadAfterFinalize` — the user might land on a
    // session whose checklist was set by a prior run, and the
    // card should reflect that history immediately.
    useChecklistStore().rehydrateFromMessages(sessionId, messages);
    // Phase C3 (2026-06-30): authoritative pull for the
    // pending-question cache. Single source of truth lives in
    // the helper `reconcilePendingInteractionFromBackend`.
    await reconcilePendingInteractionFromBackend(sessionId);
    // Return the reactive proxy, NOT the plain `messages` array. Callers
    // like `chat.ts` `send` / `resendMessage` push user/assistant
    // placeholders into the returned reference; pushing into the plain
    // array bypasses the proxy's set trap and never triggers Vue's
    // reactivity — so the new message (and every subsequent delta) failed
    // to render until a full array replacement (session switch) forced
    // `messages` to re-evaluate. `getMessages` returns the proxy that
    // `putMessages` just stored (reactive(messages)); the plain `messages`
    // above stays the read-side input for the seeding loops.
    return getMessages(sessionId)!;
  }

  /** Explicit eviction. Used on session delete so the cache
   *  doesn't keep a stale entry. Also unpins, just in case. */
  function evict(sessionId: string): void {
    pinnedSessions.delete(sessionId);
    loadedFromDb.delete(sessionId);
    messagesBySession.delete(sessionId);
    // Phase C3 (2026-06-30): clear the questionCards cache too.
    // Mirrors the B12 checklist cleanup (which the chat store's
    // deleteSession path fires explicitly) — without this, a
    // session re-created with the same id would briefly show a
    // stale pending card from the prior session. The backend
    // QuestionStore also drops the entry on session delete
    // (Phase A's `commands::sessions::delete_session` clears it),
    // so removing the cache here matches the backend's cleared
    // state — no source-of-truth drift.
    useQuestionCardsStore().removePending(sessionId);
  }

  /** Step 4 follow-up: force a re-load of `sessionId` from the DB.
   *  `ensureLoaded` is a no-op for cached sessions; worktree
   *  transitions (attach / detach / delete) inject a system
   *  event into the messages table, and the LLM's NEXT chat
   *  must see it (REQ-17 / REQ-18 in prd.md). The frontend's
   *  cache holds the pre-transition messages; without an
   *  explicit re-load, the next `send()` would build a history
   *  missing the event. `refresh` evicts + re-loads in one
   *  step. Safe to call mid-stream? No — `evict` drops
   *  `pinnedSessions`, so the LRU could reclaim the session
   *  if the user navigates away. We pin it via `ensureLoaded`
   *  (`putMessages` re-pins when the second arg is true and
   *  the session was in `pinnedSessions`, which we just
   *  removed). The caller should not call `refresh` while the
   *  session is in-flight (the chat cancel hook ensures
   *  this for detach / delete; for attach, the frontend
   *  UI never disables attach, but in practice a user
   *  won't click "attach" mid-stream anyway — the dropdown
   *  is the only path). */
  async function refresh(sessionId: string): Promise<ChatMessage[]> {
    evict(sessionId);
    return ensureLoaded(sessionId);
  }

  // ---------------------------------------------------------------------
  // Public API — request lifecycle
  // ---------------------------------------------------------------------

  interface StartRequestArgs {
    sessionId: string;
    projectId: string;
    userMsg: ChatMessage;
    assistantMsg: ChatMessage;
    /** Wire-format history (the `messages` array the backend's
     *  `chat` command expects). The caller (chat.ts) builds this
     *  so it can reuse the existing `toPayloadContent` logic. */
    history: unknown[];
    /** D3 PR3 (2026-06-17): when set, the backend treats this
     *  stream as a Resend (re-fire of an existing user message).
     *  The agent loop's user-message persist site writes a
     *  `resend_message` audit row pointing at the original
     *  user message's seq. `undefined` for normal first-time
     *  sends. Plumbed through the `chat` IPC's `resendSeq`
     *  parameter (Tauri auto-converts the snake_case Rust
     *  field to camelCase JS). */
    resendSeq?: number;
    /** explicit-agent-dispatch (2026-06-30): when set, the backend
     *  short-circuits the LLM and dispatches the named subagent
     *  directly (the `@@<agent> <task>` prefix the user typed).
     *  `undefined` for normal sends + resends.
     *
     *  B6+ B (2026-07-07): `model_id` carries an optional per-dispatch
     *  model override (a model id; the frontend resolves display_name→id
     *  via `resolveModelInput` before sending). Field name is
     *  `model_id` (snake_case) to match the backend `ForcedDispatch`
     *  struct — nested IPC struct fields pass through serde verbatim
     *  (no Tauri arg auto-camel). Omitted when no `--model=` flag. */
    forcedDispatch?: { subagent: string; task: string; model_id?: string };
  }

  /** Kick off a new stream. The caller is responsible for
   *  pushing `userMsg` and `assistantMsg` into the session's
   *  message buffer (or having them already there) before
   *  calling — otherwise the delta events will not find a
   *  `last` assistant message to mutate. Returns the
   *  `requestId` so the caller can later call `cancel`. */
  async function startRequest(args: StartRequestArgs): Promise<string> {
    await start();
    const requestId = genId();
    activeRequests.set(requestId, {
      requestId,
      sessionId: args.sessionId,
      projectId: args.projectId,
      userMsgId: args.userMsg.id,
      assistantMsgId: args.assistantMsg.id,
      history: args.history,
      // F5: capture the send timestamp for TTFB / total
      // calculation. The `firstDeltaAt` field stays null until
      // the first `delta` event arrives.
      sendAt: Date.now(),
      firstDeltaAt: null,
      // F5 follow-up per-turn: thinking-time tracking
      // moved entirely to the `case "turn_complete"`
      // handler — the `ChatEvent::TurnComplete` payload
      // from the agent loop carries the 4 ms values
      // (ttfb_ms / gen_ms / total_ms / thinking_ms)
      // computed on the backend from per-turn
      // `Instant` timestamps. The frontend no longer
      // maintains a per-request `thinkingStartedAt` /
      // `thinkingDurationMs` single-value timer — the
      // 4 close boundaries (`delta` / `tool:call` /
      // `done` / `error`) used to snapshot a single
      // per-request duration, which the F5 follow-up
      // per-turn fix removes. The placeholder's
      // `last.thinkingDurationMs` is now set ONLY by
      // the `turn_complete` case (per turn, with
      // that turn's own thinking wall clock).
      toolStartedAt: new Map(),
      // F5 follow-up per-turn: -1 = "no start event received
      // yet" (the first `case "start"` increments to 0).
      // Using -1 instead of 0 avoids the off-by-one where
      // turn 0 would land on index 1 (the bug that the
      // original F5 single-value RequestState papered over
      // by always writing to the same slot).
      currentTurnIndex: -1,
      latencyByTurn: new Map(),
      pendingTimelineText: null,
      activeThinkingIdx: null,
    });
    // Pin the session while streaming — it cannot be evicted
    // even if the user visits 20+ other sessions.
    pinnedSessions.add(args.sessionId);
    // 07-06 (am-observability-panel B1/D7): a new user message starts
    // a fresh turn — clear the prior turn's accumulated recall hits
    // so the ChatPanel chip shows "本次召回" (this turn), not a
    // running total across the whole conversation. The store owns the
    // slice; the controller only triggers the clear here (single
    // ownership point — every user-message send funnels through
    // startRequest).
    useMemoryStore().clearRecallHits(args.sessionId);
    // E2 (harness trace pipeline, 2026-07-14): a new user message
    // starts a fresh request — the trace timeline must reset to
    // the new session's history. We clear the in-memory
    // `currentSessionTraces` and reload from DB; subsequent
    // live events (context_compacted / loop_hint /
    // workflow_breadcrumb) will upsert into the freshly-loaded
    // Map. The same `startRequest` is the single funnel for every
    // user-message send, so this is the single ownership point
    // for the "fresh turn → fresh trace view" invariant.
    //
    // Fire-and-forget — a slow `loadHistory` shouldn't block the
    // stream start. The user's perception of latency is the
    // LLM's first-delta, not the trace reload. The trace store
    // has its own `loading` flag for the panel's loading skeleton.
    void useTraceStore().resetForNewSession(args.sessionId);
    // Touch the session's messages (in case it was just loaded)
    // so it sits at MRU.
    const msgs = messagesBySession.get(args.sessionId);
    if (msgs) {
      messagesBySession.delete(args.sessionId);
      messagesBySession.set(args.sessionId, msgs);
    }
    try {
      await transport.invoke("chat", {
        requestId,
        sessionId: args.sessionId,
        messages: args.history,
        // D3 PR3 (2026-06-17): pass through the resend flag.
        // When `undefined`, the Rust side receives `None` and
        // treats this as a normal first-time send (no audit).
        // When a number, the agent loop fires `resend_message`
        // audit at the user-message persist site.
        resendSeq: args.resendSeq,
        // explicit-agent-dispatch: thread the forced dispatch into
        // the loop's turn-1 short-circuit. `null` (not undefined)
        // so the Rust Option<ForcedDispatch> deserializes to None.
        forcedDispatch: args.forcedDispatch ?? null,
      });
    } catch (e) {
      const msgs = messagesBySession.get(args.sessionId);
      if (msgs) {
        const last = msgs[msgs.length - 1];
        if (last && last.role === "assistant") {
          last.streaming = false;
          last.error = { message: extractErrorMessage(e), category: "server" };
        }
      }
      finalizeRequest(requestId, args.sessionId, true);
    }
    return requestId;
  }

  /** Cancel an in-flight request by requestId. The backend's
   *  agent loop notices on the next event boundary, bails out,
   *  and emits a `done` event with `stop_reason: "cancelled"`.
   *  That `done` flows through `handleChatEvent` →
   *  `finalizeRequest`, which clears state. So this call is a
   *  fire-and-forget IPC; the actual state reset happens via
   *  the `done` event. */
  async function cancel(requestId: string): Promise<void> {
    try {
      await transport.invoke("cancel_chat", { requestId });
    } catch (e) {
      // A failed cancel is logged but not user-facing — the
      // user already saw the Stop button and clicked it. The
      // stream finishes on its own (or the next event errors
      // out), and the existing `done` / `error` path resets
      // state.
      console.error("[streamController] cancel failed:", e);
    }
  }

  /** The requestId of the current session's active stream, or
   *  null if the current session is not streaming. Convenience
   *  for the chat input's "is the stop button enabled?" check. */
  function currentRequestId(sessionId: string): string | null {
    for (const r of activeRequests.values()) {
      if (r.sessionId === sessionId) return r.requestId;
    }
    return null;
  }

  return {
    // State (exposed as refs / reactive proxies)
    messagesBySession,
    activeRequests,
    listenerReady,
    // BUG FIX (06-08-06-08): expose `pinnedSessions` + `loadedFromDb`
    // so the wire-invariant test can assert the post-`finalizeRequest`
    // state without spinning up an IPC + agent loop. Both are
    // internal Sets that the production code never reads via the
    // public API — they're accessed only by the same-file
    // `ensureLoaded` / `evict` helpers. Adding them to the return
    // makes them reactive-readable from the outside, which is
    // harmless (nothing subscribes to them in production code).
    pinnedSessions,
    loadedFromDb,
    // Derived
    streamingSessionIds: streamingSessionIds as ComputedRef<Set<string>>,
    streamingProjectIds: streamingProjectIds as ComputedRef<Set<string>>,
    // Methods
    start,
    stop,
    getMessages,
    ensureLoaded,
    evict,
    refresh,
    startRequest,
    cancel,
    currentRequestId,
    // BUG FIX (06-08-06-08): exposed for tests so the 2013-wire-invariant
    // test can drive the full send-completion path without spinning up
    // a Tauri IPC + a real agent loop. Not part of the public API that
    // UI components call — production callers go through `startRequest`
    // which routes the `done` / `error` events through this function.
    finalizeRequest,
    // F5 follow-up: exposed for the thinking-timer boundary
    // regression test. The test drives the `tool:call`
    // path directly because the full IPC → event-emitter
    // chain requires a Tauri mock we don't have in the
    // test env. The test asserts the close-on-tool-call
    // rule (thinking → tool_use with no text in between
    // still closes the timer) — keeping the close logic
    // in the same function as the per-tool timing
    // means the two concerns share a test surface.
    handleToolCall,
    // F5 follow-up debug: exposed for the full-streaming
    // flow test (thinking_delta → delta → done path).
    // The test asserts that the per-message
    // `thinkingDurationMs` lands on the in-memory
    // `last` message when the close-boundary in the
    // `delta` case fires — this is the production path
    // the user's "Thought for —" screenshot was
    // failing. The previous test (handleToolCall
    // boundary) only covered the no-text-in-between
    // edge case; this one covers the common shape.
    handleChatEvent,
    // F5 follow-up: exposed for the per-item latency reactivity
    // regression test. Production callers go through
    // `ensureLoaded` / `reloadAfterFinalize`, which both
    // route to this function. The test needs to call it
    // directly because the alternatives (`messagesBySession.set`
    // from outside) would bypass the `reactive()` wrap and
    // defeat the purpose of the test.
    putMessages,
  };
});
