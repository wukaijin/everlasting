// streamController 事件处理块(拆分自 streamController.ts, 08-07-large-file-splitting)。
import { markRaw } from "vue";
import { transport } from "../transport";
import { useChatStore } from "./chat";
import { useChecklistStore, CHECKLIST_TOOL_NAME } from "./checklist";
import { useMemoryStore } from "./memory";
import { useQuestionCardsStore } from "./questionCards";
import { useProjectsStore } from "./projects";
import {
  GET_PENDING_INTERACTION_CMD,
  PendingInteraction,
} from "./questionCards.types";
import { matchesReviewStatePath, useReviewStateStore } from "./reviewState";
import { useQuotaStore } from "./quota";
import { useTraceStore } from "./traceStore";
import type { ChatMessage, ContentBlockView } from "./chat.types";
import type {
  ModeChangePayload,
  TaskStateTransitionPayload,
  ToolQuestionPayload,
} from "./questionCards.types";
import type {
  ChatEventPayload,
  RequestState,
  ToolCallPayload,
  ToolResultPayload,
  TurnLatency,
} from "./streamController";
import { buildPendingNotification, genId, groupChatNotice } from "./streamController";
import { rehydrateMessages, type LoadedSession } from "./streamRehydrate";

export interface StreamEventsContext {
  messagesBySession: Map<string, ChatMessage[]>;
  pinnedSessions: Set<string>;
  loadedFromDb: Set<string>;
  activeRequests: Map<string, RequestState>;
  completedRequests: Map<string, RequestState>;
  putMessages: (sessionId: string, messages: ChatMessage[], pinned: boolean) => void;
  ensureContentBlocks: (m: ChatMessage) => ContentBlockView[];
  sealActiveThinking: (req: RequestState) => void;
  flushPendingTimelineText: (req: RequestState, m: ChatMessage) => void;
}
export function createStreamEventHandlers(ctx: StreamEventsContext) {
  const {
    messagesBySession, pinnedSessions, loadedFromDb, activeRequests, completedRequests,
    putMessages, ensureContentBlocks, sealActiveThinking, flushPendingTimelineText,
  } = ctx;

  // ---------------------------------------------------------------------
  // Event handlers (one global listener; routes by request_id)
  // ---------------------------------------------------------------------

  function handleChatEvent(event: ChatEventPayload): void {
    const req = activeRequests.get(event.request_id);
    if (!req) return; // event for unknown / already-finished request — drop
    const msgs = messagesBySession.get(req.sessionId);
    if (!msgs) return; // session was evicted mid-stream — shouldn't happen because pinned, but guard
    let last = msgs[msgs.length - 1];
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
        // 08-04 follow-up (群聊逐轮流式): a `start` from a NEW speaker
        // (any start after the FIRST) means the orchestrator moved on
        // from the previous speaker — push a fresh assistant placeholder
        // so this speaker's deltas land on it instead of overwriting the
        // previous speaker's message. The FIRST `start` reuses the
        // placeholder `chat.ts` pushed alongside the user message. The
        // orchestration shares one `request_id` across every inner
        // `run_chat_loop`, so without this every speaker after the first
        // would write into the first placeholder (the "群聊内容不实时出现"
        // bug).
        if (req.groupChat && req.groupChatStarted) {
          msgs.push({
            id: genId(),
            role: "assistant",
            content: "",
          });
          last = msgs[msgs.length - 1];
        }
        req.groupChatStarted = true;
        last.streaming = true;
        last.error = undefined;
        // 08-04 follow-up (实时 speaker 标识): stamp the announced
        // speaker on this turn's placeholder so the MessageItem chip
        // renders the name live. Cleared here (consumed) so a later
        // `done` without a new `speaker` doesn't leave a stale name.
        if (req.pendingSpeaker !== null) {
          last.speaker = req.pendingSpeaker;
          req.pendingSpeaker = null;
        }
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
        // 08-07-group-chat-review-fixes R2: a new speaker turn also
        // clears the prior orchestrator notice (e.g. a
        // `nominee_unknown` skip was followed by a successful
        // nomination + a real speaker turn).
        last.notice = undefined;
        break;
      case "speaker":
        // 08-04 follow-up (实时 speaker 标识): the orchestrator
        // announced which speaker's turn is next. Stash it on the
        // request; the upcoming `start` stamps it on the placeholder.
        // Only meaningful for group-chat requests, but harmless for
        // ordinary chat (no `speaker` events are ever emitted there).
        if (typeof event.speaker === "string") {
          req.pendingSpeaker = event.speaker;
        }
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
        // unified-context-budget WP2: terminal `done` — the budget-trim
        // chip described THIS request's shape; the stream is over, so
        // clear it (the durable record lives in the audit row + trace
        // badge).
        last.budgetTrim = undefined;
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
        // 08-07-group-chat-review-fixes R2: when the orchestrator emits
        // a boundary `stop_reason`, attach a transient notice to the
        // placeholder so the user sees WHY the discussion skipped a turn
        // or ended abnormally. Done before the finalize gate so the
        // notice lands on the placeholder while it still exists. The
        // notice is NOT persisted (transient field, like `retrying`).
        const gcNotice = groupChatNotice(event.stop_reason);
        if (gcNotice) {
          last.notice = gcNotice;
        }
        // 08-04 follow-up (群聊逐轮流式): for a group-chat request the
        // per-speaker `Done`s are turn boundaries, NOT the end of the
        // orchestration (one request_id spans every speaker). Keep the
        // request alive and only finalize on the terminal signals —
        // `group_chat_end` (the orchestrator ended the discussion),
        // `max_rounds` (the outer loop hit its bound — added 08-07 R2,
        // the loop has exited so no further events will arrive), or
        // `cancelled` (human preemption). The non-terminal
        // `nominee_unknown` / `participant_unresolved` reasons do NOT
        // finalize — the orchestrator keeps going and the post-loop
        // terminal `Done` is what finalizes. (08-07 R2 originally also
        // had `moderator_stuck` as terminal; 08-07-toolset R2 removed
        // the streak mechanism that produced it, so it's gone.) The
        // speaker placeholder is already sealed (`last.streaming =
        // false` above); the next speaker's `start` pushes a fresh one.
        const isTerminal =
          !req.groupChat ||
          event.stop_reason === "group_chat_end" ||
          event.stop_reason === "cancelled" ||
          event.stop_reason === "max_rounds";
        if (isTerminal) {
          finalizeRequest(req.requestId, req.sessionId, false);
        }
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
        // unified-context-budget WP2: terminal `error` — 同款清理。
        last.budgetTrim = undefined;
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
        // 08-04 follow-up (群聊逐轮流式): an inner speaker's turn
        // erroring mid-orchestration must NOT tear down the request —
        // the orchestrator keeps going (round-robin fallback) and the
        // terminal `group_chat_end` / `cancelled` `done` finalizes.
        // Ordinary chat: error is terminal (existing behavior).
        if (!req.groupChat) {
          finalizeRequest(req.requestId, req.sessionId, true);
        }
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
          // 08-18 PR2:压缩路径透传(summary/mechanical/none);
          // 旧后端 wire 无该字段 → "none"。
          method: event.method ?? "none",
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
      case "budget_trim": {
        // unified-context-budget WP2 (2026-08-19): the 关卡⑤ hard
        // gate trimmed the outgoing request just before send. Two
        // sinks: (a) the trace store (TurnCard badge — durable
        // observation, same flow as `context_compacted`), and
        // (b) the in-flight assistant placeholder's transient
        // `budgetTrim` field (MessageItem renders a "✂ 预算裁剪"
        // chip above the bubble). Unlike `retrying`, the chip is
        // NOT cleared on `start` / `delta` — the trim describes
        // this request's shape and should stay visible through
        // the stream; the terminal `done` / `error` handlers
        // clear it, and `rehydrateMessages` never copies it.
        if (
          typeof event.freed_tokens !== "number" ||
          typeof event.post_total !== "number" ||
          typeof event.window !== "number"
        ) {
          // Defensive — the Rust side always sends all fields for
          // `budget_trim`; drop a malformed event rather than risk
          // the renderer.
          break;
        }
        useTraceStore().applyEvent({
          kind: "budget_trim",
          request_id: event.request_id,
          seq: event.seq ?? 0,
          freed_tokens: event.freed_tokens,
          post_total: event.post_total,
          window: event.window,
        });
        last.budgetTrim = {
          freedTokens: event.freed_tokens,
          postTotal: event.post_total,
          window: event.window,
        };
        break;
      }
      case "turn_usage": {
        // 08-20-turn-usage-event-quota-view WP1: per-turn token 观察,
        // 唯一 sink 是 trace store(TurnCard token cells 即时可见;
        // 不挂消息占位符 —— ChatInput hint 的实时路径在 `done` case
        // 早已存在,两者口径不同:hint = 上轮上下文快照,此处 =
        // trace 明细)。防御同 `budget_trim`:malformed drop。
        // 事件缺失(usage=None 的取消/错误轮)时该轮 cells 保持
        // "—",退化等下一次 loadHistory —— 与 Rust 侧双层门对称。
        if (
          typeof event.seq !== "number" ||
          typeof event.context_window !== "number" ||
          !event.usage ||
          typeof event.usage.input_tokens !== "number"
        ) {
          break;
        }
        // `context_input_tokens` 在 streamController 的 wire 类型上是
        // optional(legacy 后端兜底)—— Rust TurnUsage 恒发全 5 字段,
        // 缺失时按 `done` case 同款 Anthropic 归一化兜底。
        const u = event.usage;
        const normalizedUsage = {
          input_tokens: u.input_tokens,
          output_tokens: u.output_tokens,
          cache_creation_input_tokens: u.cache_creation_input_tokens,
          cache_read_input_tokens: u.cache_read_input_tokens,
          context_input_tokens:
            u.context_input_tokens ??
            u.input_tokens + u.cache_creation_input_tokens + u.cache_read_input_tokens,
        };
        useTraceStore().applyEvent({
          kind: "turn_usage",
          request_id: event.request_id,
          seq: event.seq,
          run_id: event.run_id ?? "",
          usage: normalizedUsage,
          tools_token: event.tools_token ?? null,
          memory_token: event.memory_token ?? null,
          images_token: event.images_token ?? null,
          at_files_token: event.at_files_token ?? null,
          system_token: event.system_token ?? null,
          context_window: event.context_window,
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
      ...(payload.images?.length ? { images: payload.images } : {}),
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
    // MAX_TURNS softcap (08-18-max-turns-softcap): the main loop hit
    // its turn budget and asks 继续(+200)/压缩后续跑/停止. Same
    // floating-card rationale as loop_intervention — the synthetic
    // `turn_limit_softcap_{turn}` id has no tool_use anchor.
    const isTurnLimitSoftcap = payload.tool_use_id.startsWith(
      "turn_limit_softcap_",
    );
    useQuestionCardsStore().addPending(payload.session_id, {
      kind: isLoopIntervention
        ? "loop_intervention"
        : isTurnLimitSoftcap
          ? "turn_limit_softcap"
          : "question",
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
    // 08-20-turn-usage-event-quota-view WP3: request 结束 = 一次轻量
    // 配额窗口重查(滑动窗口客户端推算必漂,design 取舍"跑完这轮后
    // 刷新";fire-and-forget,不挡消息面)。
    void useQuotaStore().refresh();
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
  return {
    handleChatEvent,
    handleToolCall,
    handleToolResult,
    handleToolQuestion,
    handleModeChangeRequest,
    handleTaskStateTransition,
    reconcilePendingInteractionFromBackend,
    maybeNotifyPending,
    finalizeRequest,
    reloadAfterFinalize,
  };
}
