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
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { useChatStore, type ChatMessage, type ErrorCategory } from "./chat";

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
    | "error";
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
    for (const b of blocks) {
      if (!b || typeof b.type !== "string") continue;
      if (b.type === "thinking") {
        thinkingBlocks.push({
          text: (b.thinking as string) ?? "",
          signature: (b.signature as string) ?? "",
        });
      } else if (b.type === "redacted_thinking" && typeof b.data === "string") {
        redactedThinkingData.push(b.data);
      } else if (
        b.type === "tool_use" &&
        typeof b.id === "string" &&
        typeof b.name === "string"
      ) {
        toolCalls.push({ id: b.id, name: b.name, input: (b.input as Record<string, unknown>) ?? {} });
      } else if (b.type === "tool_result" && typeof b.tool_use_id === "string") {
        toolResults.push({
          toolUseId: b.tool_use_id,
          content: (b.content as string) ?? "",
          isError: !!b.is_error,
        });
      }
    }
    const msg: ChatMessage = { id: `${m.session_id}-${m.seq}`, role: m.role, content: m.text };
    if (toolCalls.length) msg.toolCalls = toolCalls;
    if (toolResults.length) msg.toolResults = toolResults;
    if (thinkingBlocks.length) msg.thinkingBlocks = thinkingBlocks;
    if (redactedThinkingData.length) msg.redactedThinkingData = redactedThinkingData;
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
  }
  return out;
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
   *  `delete` for us, so we just mutate it directly. */
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
    messagesBySession.set(sessionId, messages);
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

  /** Get the in-flight thinking block of an assistant message,
   *  opening a new one if the previous is already sealed with a
   *  signature (interleaved thinking). Mirrors the helper in
   *  chat.ts so the controller can handle `thinking_delta` /
   *  `signature_delta` events for streams that didn't originate
   *  from the current session. */
  function currentThinkingBlock(m: ChatMessage) {
    if (!m.thinkingBlocks || m.thinkingBlocks.length === 0) {
      m.thinkingBlocks = [{ text: "", signature: "" }];
    } else {
      const last = m.thinkingBlocks[m.thinkingBlocks.length - 1];
      if (last.signature) {
        m.thinkingBlocks.push({ text: "", signature: "" });
      }
    }
    return m.thinkingBlocks[m.thinkingBlocks.length - 1];
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
        last.streaming = true;
        last.error = undefined;
        break;
      case "delta":
        if (event.text) last.content += event.text;
        break;
      case "thinking_delta":
        if (event.text) currentThinkingBlock(last).text += event.text;
        break;
      case "signature_delta":
        if (event.signature) currentThinkingBlock(last).signature += event.signature;
        break;
      case "redacted_thinking_delta":
        if (event.data) {
          if (!last.redactedThinkingData) last.redactedThinkingData = [];
          last.redactedThinkingData.push(event.data);
        }
        break;
      case "done":
        // CRITICAL (PR3 self-check fix): the old chat.ts handler
        // set `last.streaming = false` here, which extinguishes the
        // blinking ▍ cursor in MessageItem.vue (rendered under
        // `v-if="message.streaming"`) and lets the markdown
        // pipeline `flush()` the final frame (watch on streaming
        // in MessageItem.vue). Forgetting it leaves the cursor
        // blinking forever after the stream completes — a
        // regression that violates AC6.3 ("streaming=false,光标消失").
        last.streaming = false;
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
        // A4 (Token Usage Tracking): per-turn usage report
        // arrives on the `done` event. Hand the payload off to
        // the chat store which owns the per-session running
        // totals (rendered by ChatInput.vue's hint area).
        if (event.usage) {
          useChatStore().accumulateTokenUsage(req.sessionId, event.usage);
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
        // Same post-stream markRaw — the error case is terminal
        // just like `done`, the arrays won't grow further.
        if (last.toolCalls) markRaw(last.toolCalls);
        if (last.toolResults) markRaw(last.toolResults);
        if (last.thinkingBlocks) markRaw(last.thinkingBlocks);
        if (last.redactedThinkingData) markRaw(last.redactedThinkingData);
        // F2: reset force-follow on error too.
        useChatStore().forceFollowActive = false;
        finalizeRequest(req.requestId, req.sessionId, true);
        break;
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
  }

  function handleToolResult(payload: ToolResultPayload): void {
    const req = activeRequests.get(payload.request_id);
    if (!req) return;
    const msgs = messagesBySession.get(req.sessionId);
    if (!msgs) return;
    const last = msgs[msgs.length - 1];
    if (!last || last.role !== "assistant") return;
    if (!last.toolResults) last.toolResults = [];
    last.toolResults.push({
      toolUseId: payload.tool_use_id,
      content: payload.content,
      isError: payload.is_error,
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
    activeRequests.delete(requestId);
    pinnedSessions.delete(sessionId);
    useChatStore().invalidateDiff(sessionId);
    // Fire-and-forget: replace streaming buffer with DB version.
    // Old buffer stays visible until DB load completes.
    void reloadAfterFinalize(sessionId);
  }

  /** Reload a session's messages from DB after a stream finishes.
   *  Replaces the in-memory streaming buffer with the per-turn
   *  persisted shape, preventing the 2013 wire invariant without
   *  causing a blank-page flash. */
  async function reloadAfterFinalize(sessionId: string): Promise<void> {
    const loaded = await invoke<LoadedSession | null>("load_session", {
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
  }

  // ---------------------------------------------------------------------
  // Public API — listener lifecycle
  // ---------------------------------------------------------------------

  /** Idempotent: registering a second time is a no-op. */
  async function start(): Promise<void> {
    if (listenerWired) return;
    unlistenChat = await listen<ChatEventPayload>("chat-event", (e) => {
      handleChatEvent(e.payload);
    });
    unlistenTC = await listen<ToolCallPayload>("tool:call", (e) => {
      handleToolCall(e.payload);
    });
    unlistenTR = await listen<ToolResultPayload>("tool:result", (e) => {
      handleToolResult(e.payload);
    });
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
    unlistenChat = null;
    unlistenTC = null;
    unlistenTR = null;
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
    const v = messagesBySession.get(sessionId);
    if (v) {
      // Touch: delete + re-set to move to MRU end of the
      // reactive Map's iteration order.
      messagesBySession.delete(sessionId);
      messagesBySession.set(sessionId, v);
    }
    return v;
  }

  /** Make sure `sessionId` is in the cache. If it's already
   *  there (either from a prior load or from a prior send in
   *  this app session), returns immediately. Otherwise fetches
   *  from the DB and seeds the cache. */
  async function ensureLoaded(sessionId: string): Promise<ChatMessage[]> {
    const existing = getMessages(sessionId);
    if (existing) return existing;
    const loaded = await invoke<LoadedSession | null>("load_session", {
      sessionId,
    });
    const messages = loaded ? rehydrateMessages(loaded.messages) : [];
    putMessages(sessionId, messages, pinnedSessions.has(sessionId));
    loadedFromDb.add(sessionId);
    // A4: seed the per-session token usage map from the
    // freshly-loaded session row. Without this, a page reload
    // would show "—" in the ChatInput hint area until the next
    // LLM turn in this session. The chat store owns the Map;
    // the controller hands the row data over via the public
    // `accumulateTokenUsage` API. (We use the same Map as the
    // `done`-event path; first call seeds, subsequent calls
    // add — so reload-then-`done` is correct: the first done
    // event's `usage` is added to the seeded value.)
    if (loaded && loaded.session.input_tokens_total !== null) {
      useChatStore().accumulateTokenUsage(sessionId, {
        input_tokens: loaded.session.input_tokens_total,
        output_tokens: loaded.session.output_tokens_total ?? 0,
        cache_creation_input_tokens:
          loaded.session.cache_creation_total ?? 0,
        cache_read_input_tokens: loaded.session.cache_read_total ?? 0,
      });
    }
    return messages;
  }

  /** Explicit eviction. Used on session delete so the cache
   *  doesn't keep a stale entry. Also unpins, just in case. */
  function evict(sessionId: string): void {
    pinnedSessions.delete(sessionId);
    loadedFromDb.delete(sessionId);
    messagesBySession.delete(sessionId);
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
    });
    // Pin the session while streaming — it cannot be evicted
    // even if the user visits 20+ other sessions.
    pinnedSessions.add(args.sessionId);
    // Touch the session's messages (in case it was just loaded)
    // so it sits at MRU.
    const msgs = messagesBySession.get(args.sessionId);
    if (msgs) {
      messagesBySession.delete(args.sessionId);
      messagesBySession.set(args.sessionId, msgs);
    }
    try {
      await invoke("chat", {
        requestId,
        sessionId: args.sessionId,
        messages: args.history,
      });
    } catch (e) {
      const msgs = messagesBySession.get(args.sessionId);
      if (msgs) {
        const last = msgs[msgs.length - 1];
        if (last && last.role === "assistant") {
          last.streaming = false;
          last.error = { message: String(e), category: "server" };
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
      await invoke("cancel_chat", { requestId });
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
  };
});
