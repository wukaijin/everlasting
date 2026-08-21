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
import { computed, reactive, ref, type ComputedRef } from "vue";
import { transport, type UnlistenFn } from "../transport";
import { extractErrorMessage } from "../utils/useErrorBus";

import { useChatStore } from "./chat";
import type {
  ChatMessage,
  ContentBlockView,
  ErrorCategory,
  InjectionEntry,
} from "./chat.types";
import { useChecklistStore } from "./checklist";
import { useMemoryStore } from "./memory";
import { createStreamEventHandlers, type StreamEventsContext } from "./streamEvents";
import { rehydrateMessages, type LoadedSession } from "./streamRehydrate";
import { useTraceStore } from "./traceStore";
import { useQuestionCardsStore } from "./questionCards";
import {
  MODE_CHANGE_EVENT,
  TASK_STATE_TRANSITION_EVENT,
  TOOL_QUESTION_EVENT,
  type ModeChangePayload,
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

export interface RequestState {
  requestId: string;
  sessionId: string;
  projectId: string;
  userMsgId: string;
  assistantMsgId: string;
  // 08-04 follow-up (群聊逐轮流式): `true` when the session is a
  // `group_chat`. The orchestrator (`run_group_chat_loop`) reuses ONE
  // request_id across every inner `run_chat_loop` (moderator +
  // participants) and emits one `Done` per speaker turn; only the
  // final `Done { stop_reason: "group_chat_end" }` terminates the
  // request. When set, the `done` handler seals the current speaker's
  // placeholder but does NOT finalize the request, and the `start`
  // handler pushes a fresh placeholder for each new speaker.
  groupChat: boolean;
  // 08-04 follow-up (群聊逐轮流式): whether this group-chat request
  // has seen its first `start`. The FIRST `start` reuses the assistant
  // placeholder `chat.ts` pushed alongside the user message; every
  // LATER `start` means the orchestrator moved to a new speaker turn,
  // so a fresh placeholder is pushed.
  groupChatStarted: boolean;
  // 08-04 follow-up (实时 speaker 标识): the speaker announced by the
  // orchestrator's `speaker` event for the upcoming turn. The `start`
  // handler stamps it on the freshly-pushed placeholder's `speaker`
  // field so the MessageItem chip renders the name live; cleared once
  // consumed (and on `done`, so the chip doesn't linger on the wrong
  // speaker between turns).
  pendingSpeaker: string | null;
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
export interface TurnLatency {
  seq: number;
  ttfbMs: number | null;
  genMs: number | null;
  totalMs: number | null;
  thinkingMs: number | null;
}

export interface ChatEventPayload {
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
    | "workflow_breadcrumb"
    // unified-context-budget WP2 (2026-08-19): the 关卡⑤ hard gate
    // trimmed the outgoing request before send (read-only /
    // non-persistent on the Rust side, `Retrying` 先例). Mirrors
    // Rust `ChatEvent::BudgetTrim { request_id, seq, freed_tokens,
    // post_total, window }` (snake_case wire). The handler attaches
    // the payload to the in-flight assistant placeholder's transient
    // `budgetTrim` field AND routes a copy into
    // `useTraceStore().applyEvent` for the TurnCard badge.
    | "budget_trim"
    // 08-20-turn-usage-event-quota-view WP1: per-turn token
    // observation emitted at the agent loop's Done arm next to the
    // turn_trace upsert (read-only / non-persistent, `BudgetTrim`
    // 先例). Mirrors Rust `ChatEvent::TurnUsage`. The handler routes
    // it into `useTraceStore().applyEvent` so the TurnCard token
    // cells appear immediately (no waiting for the next
    // `loadHistory`). Slice fields are `number | null` on the wire
    // (Rust `Option<u32>`).
    | "turn_usage"
    // 08-04 follow-up (实时 speaker 标识): emitted by the group-chat
    // orchestrator (`run_group_chat_loop`) right before each inner
    // speaker turn, so the frontend knows whose placeholder is about to
    // stream. Mirrors Rust `ChatEvent::Speaker { speaker }`.
    | "speaker";
  text?: string;
  signature?: string;
  data?: string;
  // 08-04 follow-up: present only when `kind === "speaker"`. The
  // speaker name ("moderator" or a participant name) for the upcoming
  // turn.
  speaker?: string;
  stop_reason?: string;
  message?: string;
  category?: ErrorCategory;
  /** unified-context-budget WP2: only present when
   *  `kind === "budget_trim"`. Mirrors Rust `ChatEvent::BudgetTrim`
   *  (snake_case wire). */
  freed_tokens?: number;
  post_total?: number;
  window?: number;
  /** 08-20-turn-usage-event-quota-view WP1: only present when
   *  `kind === "turn_usage"`. Mirrors Rust `ChatEvent::TurnUsage`
   *  (snake_case wire; slices are `number | null` per Rust
   *  `Option<u32>`). */
  run_id?: string;
  tools_token?: number | null;
  memory_token?: number | null;
  images_token?: number | null;
  at_files_token?: number | null;
  system_token?: number | null;
  context_window?: number;
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
  // 08-18-llm-context-compaction PR2:压缩路径("none" /
  // "summary" / "mechanical")。旧后端 wire 无此字段 —— 归一化层
  // `?? "none"` 兜底。
  method?: string;
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
 *  IPC convention — see backend/token-usage-tracking.md "Scenario: Token
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

export interface ToolCallPayload {
  request_id: string;
  id: string;
  name: string;
  input: Record<string, unknown>;
}

export interface ToolResultPayload {
  request_id: string;
  tool_use_id: string;
  content: string;
  is_error: boolean;
  /** 08-21-b1-image-followups R6: backend `AttachmentRef` list when
   *  the tool returned images (read_file on an image). Optional —
   *  text-only results omit it (serde skip_serializing_if). */
  images?: Array<{
    file: string;
    media_type: string;
    source: string;
    tokens_est?: number;
  }>;
}

export const genId = () =>
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

/** 08-07-group-chat-review-fixes R2: map a group-chat orchestrator
 *  boundary `stop_reason` to a user-facing notice string. Returns
 *  `null` for any reason that is not an orchestrator boundary signal
 *  (ordinary `end_turn` / `cancelled` / `group_chat_end` etc.). The
 *  `done` handler calls this and attaches the result to the in-flight
 *  placeholder's `notice` field so MessageItem can render a muted row.
 *  Pure (no store access) so it is unit-testable in isolation.
 *
 *  Note: `group_chat_end` (the normal end) is intentionally NOT mapped
 *  to a notice — a clean discussion end is not an abnormal event worth
 *  flagging; only the skip/abnormal-end reasons surface a notice. */
export function groupChatNotice(stopReason: string | undefined): string | null {
  switch (stopReason) {
    case "max_rounds":
      return "讨论已达轮次上限，自动停止。";
    case "nominee_unknown":
      return "主持人点名的发言者不在列表中，跳过该轮。";
    case "participant_unresolved":
      return "某参与者的模型不可用，跳过该轮。";
    default:
      return null;
  }
}

interface StartRequestArgs {
  sessionId: string;
  projectId: string;
  userMsg: ChatMessage;
  assistantMsg: ChatMessage;
  /** 08-04 follow-up (群聊逐轮流式): `true` when the session is a
   *  `group_chat` — the request stays alive across the inner
   *  per-speaker `Done`s and only finalizes on
   *  `Done { stop_reason: "group_chat_end" }`. `false`/omitted for
   *  ordinary chat (existing behavior: first `Done` finalizes). */
  groupChat?: boolean;
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
  // Event handling (08-07-large-file-splitting: 拆至 streamEvents.ts)
  // ---------------------------------------------------------------------
  const eventCtx: StreamEventsContext = {
    messagesBySession,
    pinnedSessions,
    loadedFromDb,
    activeRequests,
    completedRequests,
    putMessages,
    ensureContentBlocks,
    sealActiveThinking,
    flushPendingTimelineText,
  };
  const events = createStreamEventHandlers(eventCtx);

  // ---------------------------------------------------------------------
  // Public API — listener lifecycle
  // ---------------------------------------------------------------------

  /** Idempotent: registering a second time is a no-op. */
  async function start(): Promise<void> {
    if (listenerWired) return;
    unlistenChat = await transport.listen<ChatEventPayload>(
      "chat-event",
      (payload) => {
        events.handleChatEvent(payload);
      },
    );
    unlistenTC = await transport.listen<ToolCallPayload>(
      "tool:call",
      (payload) => {
        events.handleToolCall(payload);
      },
    );
    unlistenTR = await transport.listen<ToolResultPayload>(
      "tool:result",
      (payload) => {
        events.handleToolResult(payload);
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
        events.handleToolQuestion(payload);
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
        events.handleModeChangeRequest(payload);
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
        events.handleTaskStateTransition(payload);
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
      await events.reconcilePendingInteractionFromBackend(sessionId);
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
    await events.reconcilePendingInteractionFromBackend(sessionId);
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
      groupChat: args.groupChat ?? false,
      groupChatStarted: false,
      pendingSpeaker: null,
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
      events.finalizeRequest(requestId, args.sessionId, true);
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
    finalizeRequest: events.finalizeRequest,
    // 08-18-manual-compact-command: /compact 落库后前端刷新消息流复用
    // 同一"DB 已变,拉平视图"路径(done 后 reload 同款:rehydrate +
    // checklist 重推导 + pending reconcile)。requestId 缺省时 F5 延迟
    // 回写分支自然跳过(req == null)。
    reloadAfterFinalize: events.reloadAfterFinalize,
    // F5 follow-up: exposed for the thinking-timer boundary
    // regression test. The test drives the `tool:call`
    // path directly because the full IPC → event-emitter
    // chain requires a Tauri mock we don't have in the
    // test env. The test asserts the close-on-tool-call
    // rule (thinking → tool_use with no text in between
    // still closes the timer) — keeping the close logic
    // in the same function as the per-tool timing
    // means the two concerns share a test surface.
    handleToolCall: events.handleToolCall,
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
    handleChatEvent: events.handleChatEvent,
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