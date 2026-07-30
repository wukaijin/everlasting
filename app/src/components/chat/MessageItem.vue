<script setup lang="ts">
// MessageItem — single chat message bubble. Renders (in order):
//   1. Thinking block (if any) — violet left bar, collapsed by default
//   2. Redacted-thinking notice (rare, opaque data preserved for LLM)
//   3. Tool call cards (one per tool_use, with the matching result
//      looked up from the same message's `toolResults`)
//   4. The visible text bubble (with the blinking streaming cursor)
//   5. The error footer (if the turn failed)
//   6. F5 latency footer (right-aligned, hover tooltip with the
//      TTFB / gen / total breakdown)
//
// The "is there a bubble" predicate mirrors the original ChatWindow
// logic: any of {content, toolCalls, toolResults, thinkingBlocks,
// redactedThinkingData} → no bubble. The bubble is the fallback for
// the plain-text-only case.
//
// Markdown rendering (PR6):
//   The bubble text is now `v-html`'d through a debounced marked +
//   DOMPurify pipeline. See `utils/markdown.ts` for the XSS story.
//   The 50ms debounce collapses bursts of SSE deltas into a single
//   re-render; on stream end we flush so the final frame doesn't
//   wait out the timer.
//
// D3 PR2 (2026-06-17): inline message edit (user messages only).
//   On hover, a small ⋯ button appears at the top-right of the
//   <li> via `<MessageActionsMenu>`. Clicking it opens a
//   DropdownMenu with Edit / Resend / Copy; only Edit is wired
//   (Resend is a PR3 placeholder, Copy just hits the clipboard).
//   Edit replaces the bubble with a <textarea> + Save / Cancel
//   buttons; Save fires the chat store's `editMessage` (which
//   cancels any in-flight stream, fires the backend IPC, then
//   refreshes the in-memory buffer). Failure keeps the edit
//   mode active so the user can retry. The streaming state on
//   the parent <li> blocks the menu trigger entirely (defense
//   against mid-stream edits racing the LLM).

import { computed, ref, watch, onUnmounted } from "vue";
import type { ChatMessage, ThinkingBlockInfo } from "../../stores/chat.types";
import { extractErrorMessage } from "../../utils/useErrorBus";
import { categoryRetryable } from "../../utils/error";
import { useChatStore } from "../../stores/chat";
import { useProjectsStore } from "../../stores/projects";
import { useStreamControllerStore } from "../../stores/streamController";
import { getToolResult } from "../../utils/messageFormat";
import { createDebouncedRenderer, renderMarkdown } from "../../utils/markdown";
import ThinkingBlock from "./ThinkingBlock.vue";
import ToolCallCard from "./ToolCallCard.vue";
import AskUserQuestionCard from "./AskUserQuestionCard.vue";
import RequestModeChangeCard from "./RequestModeChangeCard.vue";
import RequestTaskStateTransitionCard from "./RequestTaskStateTransitionCard.vue";
import UiCard from "./UiCard.vue";
import FileInjectionsHint from "./FileInjectionsHint.vue";
import MessageActionsMenu from "./MessageActionsMenu.vue";
import MessageItemEdit from "./MessageItemEdit.vue";
import MessageItemFooter from "./MessageItemFooter.vue";
import Icon from "../Icon.vue";
import {
  ASK_USER_QUESTION_TOOL_NAME,
  REQUEST_MODE_CHANGE_TOOL_NAME,
  REQUEST_TASK_STATE_TRANSITION_TOOL_NAME,
} from "../../stores/questionCards.types";
import type { WorkflowState } from "../../stores/questionCards.types";
import { USE_UI_TOOL_NAME } from "./uiCard.types";
import {
  useQuestionCardsStore,
} from "../../stores/questionCards";
import type { QuestionCardState, ToolQuestionAnswer } from "../../stores/questionCards.types";

const props = defineProps<{
  message: ChatMessage;
}>();

const chatStore = useChatStore();
const projectsStore = useProjectsStore();
const controller = useStreamControllerStore();
const questionCardsStore = useQuestionCardsStore();

const hasVisibleBubble = computed<boolean>(() => {
  const m = props.message;
  return (
    !!m.content ||
    !!(m.toolCalls && m.toolCalls.length) ||
    !!(m.toolResults && m.toolResults.length) ||
    !!(m.thinkingBlocks && m.thinkingBlocks.length) ||
    !!(m.redactedThinkingData && m.redactedThinkingData.length)
  );
});

const showBubble = computed<boolean>(
  () =>
    !!props.message.content ||
    (!props.message.toolCalls?.length &&
      !props.message.toolResults?.length &&
      !props.message.thinkingBlocks?.length &&
      !props.message.redactedThinkingData?.length),
);

const showStreamingHint = computed<boolean>(
  () => !!props.message.streaming && !props.message.content,
);

// B12 Checklist (PR2 frontend, 2026-06-19): the
// `update_checklist` tool is rendered as a floating
// `<ChecklistCard>` overlay (mounted in ChatPanel), NOT as a
// per-call ToolCallCard in the message stream. Filter the tool
// list so the message bubble doesn't double-render the same
// state. The `use_skill` tool has no special treatment today
// (it renders as a normal ToolCallCard), so this is the first
// "virtual" tool suppression in the codebase. The filter is
// cheap (one linear pass per render); if more virtual tools
// accumulate, extract a `VIRTUAL_TOOLS` constant set.
const VIRTUAL_TOOLS = new Set<string>(["update_checklist"]);
const visibleToolCalls = computed(
  () =>
    props.message.toolCalls?.filter((tc) => !VIRTUAL_TOOLS.has(tc.name)) ?? [],
);

// ---------------------------------------------------------------------------
// 交错思考(interleaved thinking): 按 LLM 真实流式到达顺序排列
// thinking + text 块的渲染时间轴。核心诉求 —— 让思考穿插在文本之间
// (Claude.ai/Cursor 形态),而非旧的"所有思考扎堆在气泡顶部"。
//
// 数据源优先级:
//   1. `message.contentBlocks`(reload 后由 rehydrate 从 DB content 数组按
//      原序透传 —— 后端已按流序落库,见 chat_loop.rs ordered_blocks)。
//   2. 回退到分桶数组(thinkingBlocks + content)的固定顺序 —— 兼容旧消息
//      (无 contentBlocks)和实时流式态(placeholder 还没有 contentBlocks;
//      流式时 content 在累积,走单一 `rendered` 路径)。
//
// 范围: 时间轴交错 **thinking + text**。tool_use 仍作为整体工具区(msg__tools)
// 渲染在原位(工具卡片含 4 个 resolver + footer 归属,打散进 v-for 风险过高)。
// 每个 thinking 块独立成渲染点(不合并相邻),text 块各自 markdown 渲染。
//
// 与 msg__bubble 的关系: 走 contentBlocks 时间轴时(`useTimeline` 为真),
// 文本由时间轴渲染,`msg__bubble` 只保留流式 cursor + edited 标签(避免文本
// 重复)。回退路径下 `msg__bubble` 仍渲染完整文本(旧行为)。
// ---------------------------------------------------------------------------
type TimelineItem =
  | { kind: "thinking"; blocks: ThinkingBlockInfo[] }
  | { kind: "text"; text: string; html: string };

const renderTimeline = computed<TimelineItem[]>(() => {
  const m = props.message;
  if (m.contentBlocks && m.contentBlocks.length > 0) {
    const out: TimelineItem[] = [];
    for (const b of m.contentBlocks) {
      if (b.kind === "thinking") {
        // ContentBlockView(thinking) → ThinkingBlockInfo(去 kind)。
        out.push({
          kind: "thinking",
          blocks: [{ text: b.text, signature: b.signature }],
        });
      } else if (b.kind === "text" && b.text) {
        out.push({ kind: "text", text: b.text, html: renderMarkdown(b.text) });
      }
    }
    return out;
  }
  // 回退: 分桶固定顺序(thinking 在前, text 在后)。与改造前观感一致。
  const out: TimelineItem[] = [];
  if (m.thinkingBlocks && m.thinkingBlocks.length) {
    out.push({ kind: "thinking", blocks: m.thinkingBlocks });
  }
  if (m.content) {
    out.push({ kind: "text", text: m.content, html: rendered.value });
  }
  return out;
});

/** 是否走 contentBlocks 时间轴(true → 文本由时间轴渲染,
 *  msg__bubble 只留 cursor/edited)。仅在 reload 后且有 contentBlocks
 *  时为真;实时流式态/旧消息为 false(走回退 + msg__bubble)。 */
const useTimeline = computed(
  () =>
    !!props.message.contentBlocks &&
    props.message.contentBlocks.length > 0 &&
    props.message.role === "assistant",
);

// -----------------------------------------------------------------
// 2026-06-30 (Phase E, `06-30-ask-user-question-tool`): per-tool
// dispatch. R22 / AC11 — tool_use blocks with
// `name === ASK_USER_QUESTION_TOOL_NAME` render an inline
// `<AskUserQuestionCard>` directly BELOW their
// `<ToolCallCard>` (per PRD R22 / design §5.6). Other tool
// names continue to render only the `<ToolCallCard>`.
//
// State resolution (per design §5.6 + implement.md Phase E):
//   - `pending` → a live `tool:question` event has the matching
//     `tool_use_id` in `questionCardsStore.pendingBySession`
//     (keyed by sessionId; the store's pending entry carries
//     `toolUseId` so we pair by exact match).
//   - `answered` → the matching `tool_result` block in the
//     message stream is a non-cancelled answer array (we parse
//     the result content as JSON and look for an `answer` array).
//   - `cancelled` → the matching `tool_result` block is
//     `{ "cancelled": true }` (PRD R5 wire shape).
//   - Otherwise (no pending AND no tool_result) → we DON'T
//     render the inline card. This defensive guard prevents
//     rendering empty cards during the brief window between
//     the tool_use emit and the tool_result arrival (a
//     regression would render `<AskUserQuestionCard>` with no
//     questions list — see Phase E spec).
//
// Selected-answer recovery (R7a): on the historical reload
// path, the live store has no pending entry (QuestionStore is
// session-scoped but ephemeral — the answer is in the DB
// `messages.content` tool_result block). We parse the answer
// out of `result.content` and pass it as the
// `selectedAnswer` prop so the card renders the
// "answered" summary with the user's selections highlighted.
// -----------------------------------------------------------------

/** Parse the user-answer wire shape out of a tool_result content
 *  string. Returns `undefined` if the content isn't a
 *  recognized answer envelope (cancelled / non-answer / parse
 *  failure). The envelope shape follows PRD R4 (an array of
 *  `{ question, options, multi_select, header? }` entries).
 *  Cancellation uses `{ "cancelled": true }` per PRD R5. */
function parseAnswerEnvelope(
  content: string,
): { cancelled: true } | { answer: ToolQuestionAnswer[] } | undefined {
  if (!content) return undefined;
  // Fast path: not even JSON-looking, skip the parse.
  const trimmed = content.trim();
  if (trimmed[0] !== "{" && trimmed[0] !== "[") return undefined;
  try {
    const parsed: unknown = JSON.parse(trimmed);
    if (
      parsed &&
      typeof parsed === "object" &&
      !Array.isArray(parsed) &&
      (parsed as { cancelled?: unknown }).cancelled === true
    ) {
      return { cancelled: true };
    }
    if (
      parsed &&
      typeof parsed === "object" &&
      !Array.isArray(parsed) &&
      Array.isArray((parsed as { answer?: unknown }).answer)
    ) {
      const arr = (parsed as { answer: ToolQuestionAnswer[] }).answer;
      // Defensive: filter to entries with at least the
      // required fields. The Rust shape is enforced server-
      // side; a defensive skip here just keeps a malformed
      // row from crashing the UI render.
      const valid = arr.filter(
        (a): a is ToolQuestionAnswer =>
          !!a &&
          typeof a === "object" &&
          typeof a.question === "string" &&
          Array.isArray(a.options) &&
          typeof a.multi_select === "boolean",
      );
      return { answer: valid };
    }
  } catch {
    // not JSON, fall through
  }
  return undefined;
}

/** Resolve the per-tool-call state for the AskUserQuestionCard
 *  dispatch. Returns `null` when no inline card should render
 *  (defensive guard — see comment above). The lookup walks:
 *    1. live `pendingBySession` keyed by `toolUseId` (the
 *       tool_use_id is the LLM-assigned block id from the
 *       original `ToolUse(ask_user_question)` payload);
 *    2. DB tool_result content parsed for the answer envelope;
 *    3. `null` (no card). */
function resolveAskCardState(toolUseId: string): {
  state: QuestionCardState;
  questions: import("../../stores/questionCards.types").Question[];
  selectedAnswer: ToolQuestionAnswer[] | undefined;
} | null {
  // 1. Live pending — match by session + tool_use_id. The
  //    `pendingBySession` value is a tagged `PendingInteraction`
  //    union (Phase B, 2026-07-07) — we narrow to the Question
  //    variant here because the AskUserQuestionCard only renders
  //    for question flows; a pending mode change renders via
  //    `<RequestModeChangeCard>` (dispatch added in Phase D).
  const sid = chatStore.currentSessionId;
  if (sid) {
    const pending = questionCardsStore.getPending(sid);
    if (
      pending &&
      pending.kind === "question" &&
      pending.payload.tool_use_id === toolUseId
    ) {
      return {
        state: "pending",
        questions: pending.payload.questions,
        selectedAnswer: undefined,
      };
    }
  }
  // 2. Historical — look up the matching tool_result on the
  //    same message (rehydrate path attaches tool_results to
  //    the assistant message for UI grouping; same lookup the
  //    ToolCallCard uses via `getToolResult`).
  //
  //    We do NOT short-circuit on `tr.isError` here — the backend
  //    marks BOTH the user-cancelled path (PRD R5, wire
  //    `{"cancelled": true}`) AND the session-cancelled path
  //    (wire `{"cancelled_by_session": true}`) as `is_error: true`
  //    because from the LLM's perspective the tool produced no
  //    actionable answer. The UI distinguishes these states by
  //    parsing the envelope — `parseAnswerEnvelope` returns the
  //    cancelled / session-cancelled / answer variants
  //    explicitly. The "true error" case (backend 5xx during
  //    question registration) returns the generic
  //    QuestionStoreError envelope — we still want to render the
  //    card in its 'cancelled' state with the raw error text so
  //    the user can see what happened (better UX than an empty
  //    card or a missing card).
  const tr = getToolResult(props.message, toolUseId);
  if (tr) {
    const envelope = parseAnswerEnvelope(tr.content);
    if (envelope && "cancelled" in envelope) {
      // Cancelled — pass empty questions so the card renders
      // the cancelled state with the standard "user skipped"
      // note (the card itself doesn't need the question
      // list to render the cancelled badge).
      return {
        state: "cancelled",
        questions: [],
        selectedAnswer: undefined,
      };
    }
    if (envelope && "answer" in envelope && envelope.answer.length > 0) {
      // Answered — synthesize a Question[] from the answer
      // entries so the card renders the header chips + body
      // in the answered-state summary row (R7a "答完保留展开全程"
      // — the card shows the question text + selected labels).
      const synthQuestions: import("../../stores/questionCards.types").Question[] =
        envelope.answer.map((a) => ({
          question: a.question,
          ...(a.header !== undefined ? { header: a.header } : {}),
          // We don't have the original option list in the answer
          // payload (PRD R4 only echoes labels + multi_select).
          // Render labels as the only options so the card's
          // "selected" highlight class matches — see AskUserQuestionCard
          // test 6 "renders selected labels in the summary".
          // R6: when the user typed a custom answer, `a.options` is
          // `[]`; synthesize the custom text as an extra label so the
          // summary isn't empty (the card ALSO renders the custom via
          // `answer.custom` directly — both paths keep the replayed
          // summary consistent with the submit-time summary).
          options: [
            ...a.options.map((label) => ({ label })),
            ...(a.custom ? [{ label: `自定义: ${a.custom}` }] : []),
          ],
          multi_select: a.multi_select,
        }));
      return {
        state: "answered",
        questions: synthQuestions,
        selectedAnswer: envelope.answer,
      };
    }
  }
  return null;
}

/** Vue `v-bind` shim for the inline `<AskUserQuestionCard>` —
 *  resolves the card props for a given `tool_use.id`. Returns
 *  the literal props object (or `undefined` when no card
 *  should render, in which case the `v-if` on the binding is
 *  false and the component never mounts).
 *
 *  We pass the active session id from the chat store so the
 *  card's `resolveToolQuestion` IPC is routed to the right
 *  backend session. The card itself doesn't read Pinia stores
 *  (Phase D contract — see AskUserQuestionCard.vue file
 *  header); this parent owns the store-orchestrating glue per
 *  the design's split between rendering and routing.
 *
 *  Memoization: the per-render computed keys on `props.message`
 *  (which carries `toolResults` + the live store reactive
 *  read inside `resolveAskCardState`) — Vue's reactivity
 *  re-evaluates this on every relevant change. We don't add
 *  a `computed` Map here because the message has at most a
 *  handful of tool_calls, and the parsing is cheap (a JSON.parse
 *  on a small string, at most once per render per tool call).
 */
function resolveAskCardProps(
  toolUseId: string,
):
  | {
      sessionId: string;
      toolUseId: string;
      questions: import("../../stores/questionCards.types").Question[];
      state: QuestionCardState;
      selectedAnswer: ToolQuestionAnswer[] | undefined;
    }
  | undefined {
  const resolved = resolveAskCardState(toolUseId);
  if (!resolved) return undefined;
  const sid = chatStore.currentSessionId ?? "";
  return {
    sessionId: sid,
    toolUseId,
    questions: resolved.questions,
    state: resolved.state,
    selectedAnswer: resolved.selectedAnswer,
  };
}

/** Cached per-tool-call prop resolver used by the template's
 *  `v-for` so each tool_call only invokes the parser once per
 *  render. The template binds both `v-if` and `v-bind` to the
 *  same call so we never re-parse the same tool_result. */
function askCardPropsFor(tc: { id: string; name: string }):
  | {
      sessionId: string;
      toolUseId: string;
      questions: import("../../stores/questionCards.types").Question[];
      state: QuestionCardState;
      selectedAnswer: ToolQuestionAnswer[] | undefined;
    }
  | undefined {
  if (tc.name !== ASK_USER_QUESTION_TOOL_NAME) return undefined;
  return resolveAskCardProps(tc.id);
}

// -----------------------------------------------------------------
// 2026-07-07 (Phase D of `07-07-07-07-request-mode-change-tool`):
// per-tool dispatch for `request_mode_change`. R23 / AC11
// verification — tool_use blocks with `name ===
// REQUEST_MODE_CHANGE_TOOL_NAME` render an inline
// `<RequestModeChangeCard>` directly BELOW their
// `<ToolCallCard>`. The dispatch mirrors the ask_user_question
// pattern: live pending from the cards store + historical
// tool_result fallback + defensive guard for the brief
// tool_use → tool_result window.
//
// State resolution:
//   - `pending` → a live `mode:change:request` event has the
//     matching `tool_use_id` in
//     `questionCardsStore.pendingBySession` (tagged
//     `kind === "mode_change"`).
//   - `allowed` → the matching `tool_result` block in the
//     message stream has `{"allowed": true, prev_mode, new_mode}`.
//   - `denied` → the matching `tool_result` block is
//     `{"cancelled_by_user": true}` (PRD R5 wire shape).
//   - Otherwise (no pending AND no tool_result) → no card
//     (defensive guard mirrors the ask_user_question pattern).
// -----------------------------------------------------------------

/** Parse the request_mode_change tool_result content into a
 *  state tuple. Returns `undefined` when the content isn't a
 *  recognized envelope (no tool_result / non-allow / non-deny
 *  / parse failure). The envelope shape follows PRD R5:
 *    allowed  → `{"allowed": true, prev_mode, new_mode}`
 *    denied   → `{"cancelled_by_user": true}`
 *  The backend marks BOTH paths with `is_error: true` on the
 *  deny path (the LLM shouldn't see a denied request as a
 *  clean tool_result). */
function parseModeChangeEnvelope(
  content: string,
):
  | { allowed: true; prevMode: string | null; newMode: string }
  | { denied: true }
  | undefined {
  if (!content) return undefined;
  const trimmed = content.trim();
  if (trimmed[0] !== "{") return undefined;
  try {
    const parsed: unknown = JSON.parse(trimmed);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      return undefined;
    }
    const obj = parsed as Record<string, unknown>;
    if (obj.cancelled_by_user === true) {
      return { denied: true };
    }
    if (obj.allowed === true) {
      return {
        allowed: true,
        prevMode:
          typeof obj.prev_mode === "string" ? obj.prev_mode : null,
        newMode:
          typeof obj.new_mode === "string"
            ? obj.new_mode
            : "edit", // defensive fallback; backend always sends
      };
    }
  } catch {
    // not JSON, fall through
  }
  return undefined;
}

/** Resolve the per-tool-call state for the
 *  `<RequestModeChangeCard>` dispatch. Returns `null` when no
 *  card should render (defensive guard — see ask_user_question
 *  analogue). The lookup walks:
 *    1. live `pendingBySession` keyed by `toolUseId` AND
 *       `kind === "mode_change"`;
 *    2. DB tool_result content parsed for the envelope;
 *    3. `null` (no card). */
function resolveModeChangeCardState(toolUseId: string): {
  state: "pending" | "allowed" | "denied";
  targetMode: "edit" | "plan" | "yolo";
  currentMode: string | null;
  reason: string | null;
  allowedMode: "edit" | "plan" | "yolo" | null;
} | null {
  // 1. Live pending — match by session + tool_use_id. The
  //    `pendingBySession` value is a tagged `PendingInteraction`
  //    union; we narrow to the mode_change variant here.
  const sid = chatStore.currentSessionId;
  if (sid) {
    const pending = questionCardsStore.getPending(sid);
    if (
      pending &&
      pending.kind === "mode_change" &&
      pending.payload.tool_use_id === toolUseId
    ) {
      const p = pending.payload;
      return {
        state: "pending",
        targetMode: p.target_mode,
        currentMode: p.current_mode ?? null,
        reason: p.reason ?? null,
        allowedMode: null,
      };
    }
  }
  // 2. Historical — look up the matching tool_result on the
  //    same message (rehydrate path). The backend stores the
  //    answer envelope on the user-role message's tool_result.
  const tr = getToolResult(props.message, toolUseId);
  if (tr) {
    const envelope = parseModeChangeEnvelope(tr.content);
    if (envelope && "denied" in envelope) {
      return {
        state: "denied",
        targetMode: "edit", // best-effort placeholder; backend
        currentMode: null, // doesn't echo target_mode in the
        reason: null, // denied envelope (the user already saw
        allowedMode: null, // the target in the live card).
      };
    }
    if (envelope && "allowed" in envelope) {
      const newMode = envelope.newMode;
      const targetMode: "edit" | "plan" | "yolo" =
        newMode === "edit" || newMode === "plan" || newMode === "yolo"
          ? newMode
          : "edit";
      return {
        state: "allowed",
        targetMode,
        currentMode: envelope.prevMode,
        reason: null,
        allowedMode: targetMode,
      };
    }
  }
  return null;
}

function resolveModeChangeCardProps(toolUseId: string):
  | {
      sessionId: string;
      toolUseId: string;
      targetMode: "edit" | "plan" | "yolo";
      currentMode: string | null;
      reason: string | null;
      state: "pending" | "allowed" | "denied";
      allowedMode: "edit" | "plan" | "yolo" | null;
    }
  | undefined {
  const resolved = resolveModeChangeCardState(toolUseId);
  if (!resolved) return undefined;
  const sid = chatStore.currentSessionId ?? "";
  return {
    sessionId: sid,
    toolUseId,
    targetMode: resolved.targetMode,
    currentMode: resolved.currentMode,
    reason: resolved.reason,
    state: resolved.state,
    allowedMode: resolved.allowedMode,
  };
}

/** Cached per-tool-call prop resolver used by the template's
 *  `v-for` so each tool_call only invokes the parser once per
 *  render. Mirrors `askCardPropsFor`. */
function modeChangeCardPropsFor(tc: { id: string; name: string }):
  | {
      sessionId: string;
      toolUseId: string;
      targetMode: "edit" | "plan" | "yolo";
      currentMode: string | null;
      reason: string | null;
      state: "pending" | "allowed" | "denied";
      allowedMode: "edit" | "plan" | "yolo" | null;
    }
  | undefined {
  if (tc.name !== REQUEST_MODE_CHANGE_TOOL_NAME) return undefined;
  return resolveModeChangeCardProps(tc.id);
}

// --- task_state_transition card dispatch (07-09-workflow-transition-card) --
// Mirrors the mode_change dispatch above. The envelope shapes differ:
//   allowed  → `{"allowed": true, "prev_state": "...", "new_state": "..."}`
//   denied   → `{"cancelled_by_user": true, "target_state": "..."}`
//   cancelled by session → `{"cancelled_by_session": true}`
// (vs mode_change's prev_mode/new_mode + no target_state on deny).

/** Parse a `request_task_state_transition` tool_result envelope
 *  (historical / rehydrate path). Returns the discriminated shape
 *  or `undefined` when the content isn't a recognized envelope. */
function parseTaskStateTransitionEnvelope(
  content: string,
):
  | { allowed: true; prevState: WorkflowState; newState: WorkflowState }
  | { denied: true; targetState: WorkflowState }
  | { cancelled: true }
  | undefined {
  if (!content) return undefined;
  const trimmed = content.trim();
  if (trimmed[0] !== "{") return undefined;
  try {
    const parsed: unknown = JSON.parse(trimmed);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      return undefined;
    }
    const obj = parsed as Record<string, unknown>;
    if (obj.cancelled_by_session === true) {
      return { cancelled: true };
    }
    if (obj.cancelled_by_user === true) {
      const ts = obj.target_state;
      if (ts === "planning" || ts === "in_progress" || ts === "done") {
        return { denied: true, targetState: ts };
      }
      return undefined;
    }
    if (obj.allowed === true) {
      const ns = obj.new_state;
      const ps = obj.prev_state;
      // prev_state may be "" when the backend had no current task;
      // fall back to new_state so the comparison row still renders.
      const newState =
        ns === "planning" || ns === "in_progress" || ns === "done"
          ? ns
          : undefined;
      if (newState === undefined) return undefined;
      const prevState =
        ps === "planning" || ps === "in_progress" || ps === "done"
          ? ps
          : newState;
      return { allowed: true, prevState, newState };
    }
  } catch {
    // not JSON, fall through
  }
  return undefined;
}

/** Resolve the per-tool-call state for the
 *  `<RequestTaskStateTransitionCard>` dispatch. Returns `null` when
 *  no card should render (defensive guard — mirrors the
 *  ask_user_question / mode_change analogues). The lookup walks:
 *    1. live `pendingBySession` keyed by `toolUseId` AND
 *       `kind === "task_state_transition"`;
 *    2. DB tool_result content parsed for the envelope;
 *    3. `null` (no card). */
function resolveTaskStateTransitionCardState(toolUseId: string): {
  state: "pending" | "allowed" | "denied";
  targetState: WorkflowState;
  currentState: WorkflowState | null;
  slug: string;
  reason: string | null;
} | null {
  // 1. Live pending — match by session + tool_use_id.
  const sid = chatStore.currentSessionId;
  if (sid) {
    const pending = questionCardsStore.getPending(sid);
    if (
      pending &&
      pending.kind === "task_state_transition" &&
      pending.payload.tool_use_id === toolUseId
    ) {
      const p = pending.payload;
      return {
        state: "pending",
        targetState: p.target_state,
        currentState: p.current_state ?? null,
        slug: p.slug ?? "",
        reason: p.reason ?? null,
      };
    }
  }
  // 2. Historical — look up the matching tool_result (rehydrate).
  const tr = getToolResult(props.message, toolUseId);
  if (tr) {
    const envelope = parseTaskStateTransitionEnvelope(tr.content);
    if (envelope && "denied" in envelope) {
      return {
        state: "denied",
        targetState: envelope.targetState,
        currentState: null,
        slug: "",
        reason: null,
      };
    }
    if (envelope && "allowed" in envelope) {
      return {
        state: "allowed",
        targetState: envelope.newState,
        currentState: envelope.prevState,
        slug: "",
        reason: null,
      };
    }
    // cancelled-by-session: render nothing (no card for a session-
    // cancel; the turn ended abnormally).
  }
  return null;
}

function resolveTaskStateTransitionCardProps(toolUseId: string):
  | {
      sessionId: string;
      toolUseId: string;
      targetState: WorkflowState;
      currentState: WorkflowState | null;
      slug: string;
      reason: string | null;
      state: "pending" | "allowed" | "denied";
    }
  | undefined {
  const resolved = resolveTaskStateTransitionCardState(toolUseId);
  if (!resolved) return undefined;
  const sid = chatStore.currentSessionId ?? "";
  return {
    sessionId: sid,
    toolUseId,
    targetState: resolved.targetState,
    currentState: resolved.currentState,
    slug: resolved.slug,
    reason: resolved.reason,
    state: resolved.state,
  };
}

/** Cached per-tool-call prop resolver used by the template's
 *  `v-for`. Mirrors `modeChangeCardPropsFor`. */
function taskStateTransitionCardPropsFor(tc: { id: string; name: string }):
  | {
      sessionId: string;
      toolUseId: string;
      targetState: WorkflowState;
      currentState: WorkflowState | null;
      slug: string;
      reason: string | null;
      state: "pending" | "allowed" | "denied";
    }
  | undefined {
  if (tc.name !== REQUEST_TASK_STATE_TRANSITION_TOOL_NAME) return undefined;
  return resolveTaskStateTransitionCardProps(tc.id);
}

// --- Streaming state ----------------------------------------------------
// D3 PR2: the `MessageActionsMenu` greys out its trigger entirely
// when a stream is in flight on the same session. We read the
// controller's `streamingSessionIds` directly so the menu gets
// a per-session view (other sessions can keep streaming; only
// the current session's edit affordance is locked). The `isAtLeastOne`
// shape avoids subscribing to per-message deltas — we only need a
// boolean per session.
const isStreaming = computed<boolean>(() => {
  if (props.message.streaming) return true;
  // The streaming flag on the placeholder covers the user-sent
  // turn's own assistant message; for the per-session guard we
  // additionally read the controller's set. The two overlap on
  // the placeholder but neither subscribes to the other, so
  // a stale read of one is caught by the other.
  const sid = chatStore.currentSessionId;
  if (!sid) return false;
  return controller.streamingSessionIds.has(sid);
});

// --- D3 PR2: inline edit state -----------------------------------------
// `editingMessageSeq` lives on the chat store so it survives the
// MessageList re-render (Vue key-based remount on session switch
// would lose a local `ref`). A single ref is enough — only one
// row can be in edit mode at a time (opening a second one closes
// the first). The `isEditingThisMessage` computed derives the
// boolean for the current row.
//
// The actual edit UI (textarea + Save / Cancel / inline error)
// lives in `<MessageItemEdit>` (2026-06-23 split). This parent
// keeps three roles:
//   1. `isEditingThisMessage` computed: read-only check used by
//      the v-if gate + the v-bind into the child.
//   2. `editSaving` ref: tracks the in-flight `editMessage` IPC.
//      Passed to the child as the `saving` prop so the Save
//      button can flip to "保存中..." and disable Cancel.
//   3. Three handler functions (`handleSave` / `handleCancel` /
//      `handleResend`): own the store interactions
//      (`chatStore.editMessage` / `chatStore.resendMessage` /
//      `chatStore.editingMessageSeq = null`) and surface
//      toasts on failure. The child only emits intents.
const isEditingThisMessage = computed<boolean>(
  () =>
    chatStore.editingMessageSeq !== null &&
    chatStore.editingMessageSeq === props.message.seq,
);

/** True while the `editMessage` IPC is in flight. Disables
 *  the child editor's Save / Cancel buttons and flips the
 *  Save label to "保存中...". Reset to false on success
 *  (edit mode closes) and on failure (caught in the IPC
 *  promise, see `handleSave`). */
const editSaving = ref<boolean>(false);

/** Inline error message shown above the Save / Cancel row
 *  by `<MessageItemEdit>`. Set when the `editMessage` IPC
 *  rejects; cleared on the next edit-mode entry (the
 *  parent flips it to null when `isEditingThisMessage`
 *  flips to true). */
const editError = ref<string | null>(null);

watch(
  () => isEditingThisMessage.value,
  (now) => {
    if (now) {
      // Fresh edit session: clear any stale error from
      // the previous attempt. The save-in-flight flag
      // can't be stale (a previous save would have closed
      // edit mode on success or routed through the catch
      // on failure).
      editError.value = null;
    }
  },
  { immediate: true },
);

/** `MessageActionsMenu`'s `edit` emit handler. Routes to the
 *  chat store so the editing-message-seq flips to this row's
 *  seq; the local `isEditingThisMessage` then re-evaluates and
 *  the textarea renders. */
function onEdit(messageSeq: number) {
  if (props.message.role !== "user") return;
  if (isStreaming.value) return;
  chatStore.editingMessageSeq = messageSeq;
}

/** D3 PR3 (2026-06-17): `MessageActionsMenu`'s `resend` emit
 *  handler. Re-fires the user message through the chat store,
 *  which (1) cancels any in-flight stream, (2) re-fires the
 *  `chat` IPC with the `resendSeq` flag, (3) the backend writes
 *  a `resend_message` audit row at the user-message persist
 *  site. We pass `props.message.content` as the user prompt —
 *  the backend treats the resend as identical to a normal
 *  send (same content, same history). On error, the
 *  `chatStore.resendMessage` promise rejects and we surface a
 *  toast (same pattern as `handleSave`'s catch path). */
async function onResend(messageSeq: number) {
  if (props.message.role !== "user") return;
  if (isStreaming.value) return;
  const sid = chatStore.currentSessionId;
  if (!sid) {
    projectsStore.showToast("重发失败: 无当前 session", "error");
    return;
  }
  try {
    await chatStore.resendMessage(sid, messageSeq, props.message.content);
  } catch (e) {
    projectsStore.showToast(
      `重发失败: ${extractErrorMessage(e)}`,
      "error",
    );
  }
}

/** A5 R2 (2026-07-17): `MessageItemFooter`'s `retry` emit
 *  handler. Re-fires the chat stream against the errored
 *  assistant row by calling `chatStore.retryChat` (which
 *  cancels any in-flight stream, mutates the errored row,
 *  strips the `ERROR_MARKER` tail, and starts a new stream
 *  without a `resendSeq` flag — no audit, no content
 *  mutation). Returns gracefully (no throw) so the loading
 *  state in the footer resets regardless of the IPC outcome.
 *
 *  During the retry we set `retryLoading = true` so the
 *  footer button flips to its disabled "重试中..." state;
 *  the watcher on `streamingSessionIds` below clears it when
 *  the new stream ends (`done` / `error` / cancel).
 *
 *  Why guard `categoryRetryable`: defensively prevent a click
 *  that shouldn't be possible (footer only renders the button
 *  when retryable is true) from firing the IPC — keeps the
 *  log quiet on stale UI snapshots. */
const retryLoading = ref(false);

async function onRetry(messageSeq: number) {
  if (typeof messageSeq !== "number") return;
  if (!categoryRetryable(props.message.error?.category)) return;
  if (isStreaming.value) return;
  const sid = chatStore.currentSessionId;
  if (!sid) {
    projectsStore.showToast("重试失败: 无当前 session", "error");
    return;
  }
  retryLoading.value = true;
  try {
    await chatStore.retryChat(sid, messageSeq);
  } catch (e) {
    projectsStore.showToast(
      `重试失败: ${extractErrorMessage(e)}`,
      "error",
    );
    retryLoading.value = false;
  }
  // Success path leaves `retryLoading=true`; the watcher
  // below clears it on the next stream-id transition.
}

/** A5 R2: when this message's session leaves the streaming
 *  set (the retry's `done` / `error` event landed), reset
 *  the footer's `retryLoading` flag so the button goes back
 *  to its non-loading label. Watching `streamingSessionIds`
 *  is cheaper than watching `message.streaming` because
 *  the latter is set true during the retry's placeholder
 *  mutation, which would never go back to false unless we
 *  watched something else too. The streaming set is the
 *  single source of truth for "the session has an in-flight
 *  request" — when it loses this message's session, the
 *  retry chain is over. */
watch(
  () => controller.streamingSessionIds,
  (ids) => {
    const sid = chatStore.currentSessionId;
    if (sid && !ids.has(sid) && retryLoading.value) {
      retryLoading.value = false;
    }
  },
);

/** `<MessageItemEdit>`'s `save` emit handler. Called with
 *  the trimmed textarea content. Cancels any in-flight
 *  stream, fires the backend `edit_user_message` IPC, then
 *  refreshes the in-memory buffer. On success, closes
 *  edit mode; on failure, surfaces an inline error + a
 *  toast and keeps edit mode active for retry. */
async function handleSave(trimmed: string) {
  if (!props.message.seq) {
    editError.value = "消息缺少 seq,无法编辑";
    return;
  }
  if (editSaving.value) return;
  // The session id we send must be the one this message
  // belongs to. The store's `currentSessionId` is the user's
  // *current* session — for a rehydrated message this is
  // always the same value (MessageList only renders messages
  // for the active session), so this is correct. Defensive:
  // if the user somehow triggers edit on a message from a
  // different session (shouldn't happen, the menu is per-
  // message in the active list), the IPC would error with
  // "session not found" and the catch path surfaces it.
  const sid = chatStore.currentSessionId;
  if (!sid) {
    editError.value = "editMessage: no current session";
    return;
  }
  editSaving.value = true;
  editError.value = null;
  try {
    await chatStore.editMessage(sid, props.message.seq, trimmed);
    // Refresh succeeded. The controller's `refresh` has
    // already replaced the in-memory buffer; we close the
    // edit mode so the bubble re-renders with the new
    // content (the rehydrated message carries the new
    // `text` column).
    chatStore.editingMessageSeq = null;
  } catch (e) {
    // Failure path: keep edit mode active so the user can
    // adjust and retry. The error message is the IPC's
    // `String` rejection (e.g. "edit_user_message: user
    // message at seq 5 not found in session ...") or a
    // generic message for client-side errors.
    editError.value = extractErrorMessage(e);
    projectsStore.showToast(
      `编辑失败: ${extractErrorMessage(e)}`,
      "error",
    );
  } finally {
    editSaving.value = false;
  }
}

/** `<MessageItemEdit>`'s `cancel` emit handler. Closes
 *  edit mode without saving. Also covers the child-side
 *  "same-content no-op" path (the child emits `cancel`
 *  when the trimmed buffer equals `props.message.content`,
 *  so the user doesn't see a textarea stuck open). */
function handleCancel() {
  chatStore.editingMessageSeq = null;
  editError.value = null;
}

/** `<MessageItemEdit>`'s `resend` emit handler. Re-fires
 *  the user prompt through the chat store. The child
 *  currently does not render a Resend button (the user
 *  has to go through the `<MessageActionsMenu>` to get
 *  there), but the prop+emit is exposed for any future
 *  flow that wants to surface Resend from the editor. */
async function handleResend() {
  if (props.message.role !== "user") return;
  if (isStreaming.value) return;
  const sid = chatStore.currentSessionId;
  if (!sid) {
    projectsStore.showToast("重发失败: 无当前 session", "error");
    return;
  }
  if (!props.message.seq) {
    projectsStore.showToast("重发失败: 消息缺少 seq", "error");
    return;
  }
  try {
    await chatStore.resendMessage(sid, props.message.seq, props.message.content);
  } catch (e) {
    projectsStore.showToast(
      `重发失败: ${extractErrorMessage(e)}`,
      "error",
    );
  }
}

// --- Markdown pipeline ----------------------------------------------------
// `createDebouncedRenderer` collapses the SSE delta stream into
// one render per 50ms quiet window; the `flush()` on stream end
// renders the final frame immediately so the user doesn't see
// a 50ms gap between the last delta and the rendered terminal
// state. The watcher drives the pipeline off `message.content`.
//
// Note: there is no `displayContent` gate here. The pre-split
// `displayContent` computed returned `""` while the row was in
// edit mode, on the theory that a streaming delta could clobber
// the textarea via the markdown render path. The bubble
// template's `v-if="showBubble"` already removes the
// `v-html="rendered"` element when the row is in edit mode
// (the `<MessageItemEdit>` block is the v-if alternative), so
// the markdown output has nowhere to render — the gate is
// redundant. The watcher watches the raw content directly and
// the only side-effect of a streaming delta mid-edit is one
// wasted `schedule()` call (debounced to 50ms, no-op because
// the bubble is unmounted).
const { rendered, schedule, flush, dispose } = createDebouncedRenderer(50);

watch(
  () => props.message.content,
  (next) => {
    schedule(next);
  },
  { immediate: true },
);

// When the stream ends, render the final frame immediately so the user
// doesn't see a 50ms gap between the last delta and the rendered
// terminal state. `streaming` is `true` only while SSE is active.
watch(
  () => props.message.streaming,
  (isStreaming) => {
    if (!isStreaming) flush();
  },
);

onUnmounted(() => {
  dispose();
});

// --- D3 PR3 (2026-06-17): "(edited)" label ----------------------------------
// When the row's metadata carries `edited_at` (written by the
// backend's `edit_user_message` transaction; see
// `.trellis/spec/backend/database-guidelines.md` "Pattern:
// `edit_user_message`"), we render a small grey "(edited)"
// label next to the bubble. The label is intentionally short —
// the user just needs a hint that this row's content was
// edited (vs. an un-edited row); the precise timestamp lives
// in the audit log (the `edit_message` audit row carries
// `edited_at`). Both user AND assistant messages can show the
// label (D3 PR1 in principle only allows user edits, but the
// metadata is read generically — defensive rendering for any
// future edit path). Hidden while the bubble is streaming
// (the placeholder has no metadata until the row is
// persisted) and while the row is in edit mode (the user is
// looking at the editor, not the bubble).
const editedAt = computed<string | null>(() => {
  const meta = props.message.metadata;
  if (!meta || typeof meta !== "object") return null;
  const v = (meta as Record<string, unknown>).edited_at;
  if (typeof v !== "string" || v.length === 0) return null;
  return v;
});

const showEditedLabel = computed<boolean>(
  () =>
    editedAt.value !== null &&
    !props.message.streaming &&
    !isEditingThisMessage.value,
);
</script>

<template>
  <li
    :class="[
      'msg',
      `msg--${message.role}`,
      {
        'msg--err': message.error,
        'msg--editing': isEditingThisMessage,
      },
    ]"
  >
    <!--
      D3 PR2: hover-triggered actions menu. Renders a small ⋯
      button at the top-right of the row (absolute-positioned
      via the .msg-actions class). Hidden when the message is
      being edited or the session is streaming. The hover
      affordance is the parent <li>'s `:hover` so the menu
      stays visible while the cursor moves onto it. See the
      `<MessageActionsMenu>` component for the dropdown shape
      and the disable rules.
    -->
    <MessageActionsMenu
      v-if="message.seq !== undefined"
      :message-seq="message.seq"
      :session-id="chatStore.currentSessionId ?? ''"
      :content="message.content"
      :role="message.role"
      :is-editing="isEditingThisMessage"
      :is-streaming="isStreaming"
      @edit="onEdit"
      @resend="onResend"
    />

    <ThinkingBlock
      v-if="
        message.role === 'assistant' &&
        !useTimeline &&
        message.thinkingBlocks &&
        message.thinkingBlocks.length
      "
      :blocks="message.thinkingBlocks"
      :streaming="message.streaming"
      :show-streaming-hint="showStreamingHint"
      :thinking-duration-ms="message.thinkingDurationMs"
    />

    <!--
      交错思考: contentBlocks 时间轴(reload 后有 contentBlocks 时启用)。
      按 LLM 真实流序渲染 thinking + text 块,思考穿插在文本之间。
      每个 thinking 块独立折叠(ThinkingBlock 接收单块 blocks 数组);
      text 块各自 markdown 渲染。回退路径(useTimeline=false)不进这里,
      走顶部 ThinkingBlock + msg__bubble 的旧行为。
    -->
    <template v-if="useTimeline">
      <template v-for="(item, idx) in renderTimeline" :key="idx">
        <ThinkingBlock
          v-if="item.kind === 'thinking'"
          :blocks="item.blocks"
          :streaming="message.streaming"
          :show-streaming-hint="showStreamingHint"
          :thinking-duration-ms="message.thinkingDurationMs"
        />
        <div v-else class="msg__bubble msg__bubble--timeline">
          <span
            class="msg__markdown"
            v-html="item.html"
          />
        </div>
      </template>
      <!-- 流式 cursor(实时态不进时间轴,这里仅 reload 后的静止态,
           但保留以兼容 useTimeline 为真且仍在 streaming 的边界)。 -->
      <span
        v-if="message.streaming"
        class="msg__cursor"
        aria-hidden="true"
        >▍</span
      >
      <!-- (edited) 标签:useTimeline 时文本已进时间轴,msg__bubble 不渲染,
           所以 edited 标签在这里单独补上(assistant 行)。 -->
      <span
        v-if="showEditedLabel"
        class="msg__edited"
        :title="`最后编辑于 ${editedAt}`"
        data-testid="msg-edited-label"
      >
        (edited)
      </span>
    </template>

    <!--
      A5+ (2026-07-04, R8): transient retry notice. While the
      agent loop's `LlmRetrySink` sleeps between retry attempts
      (Full Jitter backoff or honored retry-after advisory),
      the in-flight assistant placeholder carries a `retrying`
      object. We render a small chip above the bubble so the
      user understands the stream is paused, not dead — without
      this row a multi-second backoff looks identical to a
      frozen UI (the user can't tell whether to wait or ressend).

      Visibility rules:
        - Only when `retrying` is set (the controller clears it
          on the next `start` / `delta` / `done` / `error`, so
          the row naturally disappears the moment the retry
          resolves or fails terminally).
        - Assistant rows only (the field is never attached to
          user bubbles).
        - NOT persisted to DB: `rehydrateMessages` does not
          copy `retrying`, so a session reload drops the chip.

      The text is Chinese (对齐 L3b PR4 风格 — no i18n key, the
      project is single-locale). The arrow ↩ mirrors the chat
      affordance icon the user already knows from MessageActionsMenu.
    -->
    <div
      v-if="message.role === 'assistant' && message.retrying"
      class="msg__retrying"
      data-testid="msg-retrying"
      :title="`重试中 ${message.retrying.attempt}/${message.retrying.maxAttempts},${(message.retrying.waitMs / 1000).toFixed(1)}s 后重发`"
    >
      <Icon name="refresh" :size="12" icon-class="msg__retrying-icon" />
      <span class="msg__retrying-text">
        重试中 {{ message.retrying.attempt }}/{{ message.retrying.maxAttempts }},{{
          (message.retrying.waitMs / 1000).toFixed(1)
        }}s 后重发…({{ message.retrying.reason }})
      </span>
    </div>

    <div
      v-if="message.redactedThinkingData && message.redactedThinkingData.length"
      class="msg__redacted"
      :title="`${message.redactedThinkingData.length} redacted thinking block(s); preserved verbatim for the LLM but not displayable`"
    >
      <Icon name="lock" :size="12" icon-class="msg__redacted-icon" />
      {{ message.redactedThinkingData.length }} redacted thinking block{{
        message.redactedThinkingData.length === 1 ? "" : "s"
      }}
      (preserved for LLM)
    </div>

    <div
      v-if="visibleToolCalls.length"
      class="msg__tools"
    >
      <!--
        2026-06-30 Phase E (R22 / AC11): per-tool dispatch. The
        template iterates `visibleToolCalls` and renders each
        ToolCallCard; for `ask_user_question` blocks, an inline
        `<AskUserQuestionCard>` is mounted directly BELOW the
        matching ToolCallCard (sibling within the same
        `msg__tools` flex column). The card reuses the message
        stream's scroll / render lifecycle — no portal, no
        modal (per design §5.5 UI red line + AC10).

        The fragment (`<template v-for>`) keeps the AskUserQuestionCard
        OUTSIDE the ToolCallCard's DOM tree (so it doesn't
        collide with the card's own click handlers / animation
        state). The two are visually adjacent — the user sees
        one logical "ask" affordance composed of the tool
        metadata header (ToolCallCard) + the question body
        (AskUserQuestionCard).

        We render `<AskUserQuestionCard>` only when
        `resolveAskCardState` returns a non-null tuple —
        defensive guard against the brief window between
        tool_use emit and tool_result arrival where neither
        live pending nor DB result exists yet.
      -->
      <template v-for="tc in visibleToolCalls" :key="tc.id">
        <ToolCallCard :call="tc" :result="getToolResult(message, tc.id)" />
        <AskUserQuestionCard
          v-if="askCardPropsFor(tc) !== undefined"
          v-bind="askCardPropsFor(tc)!"
        />
        <!--
          2026-07-07 Phase D (`07-07-07-07-request-mode-change-tool`):
          per-tool dispatch for `request_mode_change`. Inline
          `<RequestModeChangeCard>` mounts directly BELOW the
          matching ToolCallCard (sibling within the same
          `msg__tools` flex column). Reuses the message
          stream's scroll / render lifecycle — no portal, no
          modal (per design §5.5 UI red line + AC10).
        -->
        <RequestModeChangeCard
          v-if="modeChangeCardPropsFor(tc) !== undefined"
          v-bind="modeChangeCardPropsFor(tc)!"
        />
        <!--
          2026-07-09 (`07-09-workflow-transition-card`): per-tool
          dispatch for `request_task_state_transition`. Inline
          `<RequestTaskStateTransitionCard>` mounts directly BELOW
          the matching ToolCallCard (sibling within the same
          `msg__tools` flex column). Same no-portal / no-modal UI
          red line as the two cards above.
        -->
        <RequestTaskStateTransitionCard
          v-if="taskStateTransitionCardPropsFor(tc) !== undefined"
          v-bind="taskStateTransitionCardPropsFor(tc)!"
        />
        <UiCard v-if="tc.name === USE_UI_TOOL_NAME" :call="tc" />
      </template>
      <!--
        2026-06-27 polish: when the message has tool calls but no
        text bubble (the common "LLM only emitted tools" turn), the
        F5 latency chip used to render OUTSIDE msg__tools, leaving
        a visually-detached `2.7s` label floating in space below
        the last tool card. Moving the footer INSIDE msg__tools
        attaches the chip to the last tool card visually. When the
        message has a text bubble, the v-if below short-circuits and
        the footer renders in its original bubble-anchored position
        (where the latency is conceptually attached to the LLM's
        prose, not its tool calls).
      -->
      <MessageItemFooter
        v-if="!showBubble && !isEditingThisMessage"
        :role="message.role"
        :streaming="!!message.streaming"
        :latency="message.latency"
        :error="message.error"
        :message-seq="message.seq"
        :retry-loading="retryLoading"
        @retry="onRetry"
      />
    </div>

    <!--
      D3 PR2 (2026-06-17): inline edit mode for user messages.
      2026-06-23 split: the editor UI lives in
      `<MessageItemEdit>` — this parent only handles the
      v-if gate, the store-orchestrating handlers
      (`handleSave` / `handleCancel` / `handleResend`),
      and the IPC state machine (`editSaving` /
      `editError`). The child is a pure presentation layer
      that emits `save(trimmed)` / `cancel` / `resend`;
      no Pinia store import.

      The edit-mode branch is mutually exclusive with
      the streaming branch (the menu trigger is disabled
      when streaming, so the user can't open edit during
      a stream), but the v-if checks both
      `isEditingThisMessage` AND the absence of streaming
      as a defensive guard.
    -->
    <MessageItemEdit
      v-if="isEditingThisMessage && !isStreaming && message.role === 'user'"
      :seq="message.seq ?? 0"
      :content="message.content"
      :is-streaming="isStreaming"
      :current-session-id="chatStore.currentSessionId"
      :is-editing-this-message="isEditingThisMessage"
      :saving="editSaving"
      :error-message="editError"
      @save="handleSave"
      @cancel="handleCancel"
      @resend="handleResend"
    />

    <div v-else-if="showBubble && !useTimeline" class="msg__bubble">
      <span
        v-if="hasVisibleBubble || message.content"
        class="msg__markdown"
        v-html="rendered"
      />
      <span v-if="message.streaming" class="msg__cursor" aria-hidden="true"
        >▍</span
      >
      <!--
        D3 PR3 (2026-06-17): "(edited)" label. Renders
        inline at the bottom-right of the bubble when the
        row's metadata has `edited_at`. The label is a small
        grey mono-text chip — visually quiet so it doesn't
        compete with the bubble content. The `title`
        attribute surfaces the precise edit timestamp on
        hover for users who care to look. We keep this
        separate from the F5 latency chip (which renders
        BELOW the bubble in `.msg__latency`) so the two
        never collide when both are present (assistant
        message with both latency + edited_at).
      -->
      <span
        v-if="showEditedLabel"
        class="msg__edited"
        :title="`最后编辑于 ${editedAt}`"
        data-testid="msg-edited-label"
      >
        (edited)
      </span>
    </div>

    <!--
      B2 PR3: per-user-turn `@relpath` injection hint row.
      Renders the agent loop's verdict for every @file
      token the user typed in this message — text
      injections (with line count), image/PDF/Office/
      binary degradations, and out-of-root / missing /
      unreadable skips. Mounted ONLY for user messages
      (the assistant never has @ tokens) and ONLY when
      the `injections` array is non-empty (a no-@ user
      message leaves the field undefined; the
      `v-if` keeps the DOM clean for the common case).
      The component is a thin renderer — see
      `FileInjectionsHint.vue` for the per-row shape.
    -->
    <FileInjectionsHint
      v-if="
        message.role === 'user' &&
        message.injections &&
        message.injections.length > 0
      "
      :injections="message.injections"
    />

    <!--
      2026-06-23 split: error row + F5 latency chip extracted
      into `<MessageItemFooter>`. Per the task's ADR-2
      decision, the (edited) label stays in the parent
      (inside the bubble div) — it is visually distinct
      from the error / latency chips that hang below the
      bubble, and it shares a flex column with the bubble
      text. The footer only handles error + latency.

      The parent passes the raw `error` / `latency` from
      the ChatMessage and the streaming flag (the footer
      reads them through the same v-if gate as before).

      2026-06-27 polish: when the message has tool calls but
      no text bubble, the footer is rendered INSIDE
      `msg__tools` above (so the latency chip attaches to
      the last tool card). The outer footer here only
      renders when there's NO tool-calls/no-bubble mismatch
      (i.e., bubble-only or user-role / system rows). The
      `v-if` gates both: no tools AND no bubble visible.
    -->
    <MessageItemFooter
      v-if="!visibleToolCalls.length || showBubble"
      :role="message.role"
      :streaming="!!message.streaming"
      :latency="message.latency"
      :error="message.error"
      :message-seq="message.seq"
      :retry-loading="retryLoading"
      @retry="onRetry"
    />
  </li>
</template>

<style scoped>
.msg {
  display: flex;
  flex-direction: column;
  max-width: 75%;
  /* Position context for the absolute-positioned
     .msg-actions trigger — see MessageActionsMenu.vue.
     `relative` lets the trigger anchor to the row's
     top-right without flowing inline. */
  position: relative;
}

.msg--user {
  align-self: flex-end;
  margin-right: 16px;
}

.msg--assistant {
  align-self: flex-start;
}

/* D3 PR2: the inline edit mode gets a subtle accent border
   + a tinted background to signal "this row is in
   edit-mode" — analogous to the visual hint the
   .tool-card--pending class gives the tool card. The user
   can still see the surrounding context (no full
   `outline` ring) but the row is clearly demarcated. */
.msg--editing {
  padding: 4px 6px;
  margin: -4px -6px;
  border-radius: var(--radius-lg);
  background: color-mix(in srgb, var(--color-accent) 6%, transparent);
  border: 1px solid color-mix(in srgb, var(--color-accent) 40%, var(--color-bg-border));
}

/* D3 PR2: hover affordance for the .msg-actions trigger.
   The trigger is `position: absolute; top: -8px; right: 4px`
   inside MessageActionsMenu and starts at `opacity: 0`; we
   fade it in when the user hovers the row. `:focus-within`
   keeps it visible while keyboard focus is anywhere inside
   the row (e.g. a Save button after a click). The check for
   `msg--editing` / `msg--err` is handled by the
   MessageActionsMenu's own state classes (they keep
   `pointer-events: none` + `opacity: 0` even when the
   parent is hovered). */
.msg:hover .msg-actions,
.msg:focus-within .msg-actions {
  opacity: 1;
}

/* PR-3a (2026-06-27): whole-row hover tint. A 6% primary-text
   wash on the row tells the user "this is an interactive row"
   (not just the bubble — the row owns the actions menu).
   Excluded for edit/err states (they own their own visual
   treatment via .msg--editing / .msg--err backgrounds). The
   transition keeps the wash smooth and avoids a hard flash
   on rapid mouse passes. */
.msg:not(.msg--editing):not(.msg--err) {
    border-radius: var(--radius-lg);
    transition: background-color var(--duration-fast) var(--ease-out);
}
.msg:not(.msg--editing):not(.msg--err):hover,
.msg:not(.msg--editing):not(.msg--err):focus-within {
    background: var(--color-bg-hover);
}

.msg__redacted {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 4px;
  padding: 4px 10px;
  background: var(--color-bg-elevated);
  border: 1px dashed var(--color-bg-border);
  border-radius: var(--radius-md);
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  font-family: var(--font-mono);
}

.msg__redacted-icon {
  flex-shrink: 0;
  color: var(--color-text-secondary);
}

/*
  A5+ (2026-07-04): retry notice row. Visually a small inline chip
  above the bubble — same family as `msg__redacted` (dashed border,
  mono font, muted text) but with an amber/warning tint to signal
  "transient degraded state". Spins the icon via CSS animation so
  the user sees live progress (the icon's rotation period is
  decoupled from the wait_ms — it's just a "this is alive"
  affordance, not a precise countdown).
*/
.msg__retrying {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 4px;
  padding: 4px 10px;
  background: var(--color-bg-elevated);
  border: 1px dashed var(--color-status-warn, #f0ad4e);
  border-radius: var(--radius-md);
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  font-family: var(--font-mono);
}

.msg__retrying-icon {
  flex-shrink: 0;
  color: var(--color-status-warn, #f0ad4e);
  animation: msg__retrying-spin 1.4s linear infinite;
}

@keyframes msg__retrying-spin {
  from {
    transform: rotate(0deg);
  }
  to {
    transform: rotate(360deg);
  }
}

.msg__retrying-text {
  /* mono font already set on the parent; keep this span plain so
     the reason text wraps naturally on narrow viewports. */
  white-space: normal;
  word-break: break-word;
}

.msg__tools {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin-top: 4px;
  max-width: 100%;
}

.msg__bubble {
  padding: 10px 14px;
  border-radius: var(--radius-lg);
  /* `white-space: pre-wrap` removed in PR6 — markdown handles its own
     line breaks via `breaks: true` in the marked options, and
     pre-wrap would mangle <pre> code blocks (the leading whitespace
     on each line of code would be preserved literally, fighting the
     monospace font's own rendering). */
  word-break: break-word;
  line-height: var(--leading-relaxed);
  border: 1px solid var(--color-bg-border);

  margin-top: 4px;
  margin-bottom: 4px;
}

/* PR-3a (2026-06-27): user bubble lightened.
   Was: accent (#3b5bdb) fill + white text. Too visually heavy for
   a chat where the user message is one of two equally-weighted roles
   in a turn. New: accent-muted (#1e2a5e) fill + primary text
   (cbd5e1). WCAG 8.66:1 contrast — both AA (4.5) and AAA (7) pass.
   Subtle 30% accent border for delineation against chat-panel bg. */
.msg--user .msg__bubble {
  background: var(--color-accent-muted);
  color: var(--color-text-primary);
  border-color: color-mix(in srgb, var(--color-accent) 30%, transparent);
  /* PR5a (2026-06-27, D6 方案A): 3px accent left bar — a visual
     anchor for "this is my input" that distinguishes the user
     bubble from the assistant's elevated-gray bubble at a glance,
     reusing the tool-card left-bar semantic. Inset box-shadow
     (not border-left) so it doesn't perturb the bubble's 1px
     border-width or shift the layout. Assistant bubbles get no
     left bar. */
  box-shadow: inset 3px 0 0 var(--color-accent);
}

.msg--assistant .msg__bubble {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
}

.msg--err .msg__bubble {
  border-color: var(--color-tool-error);
  background: var(--color-bg-elevated);
}

/* 交错思考: 时间轴内的 text 块气泡。去掉独立 border + 收紧 margin ——
   一个 turn 里可能有多个 text 块(被 thinking 穿插),每个都套独立气泡
   边框会割裂"连续流动"的观感。改为无边框的连续文本流,与上下 ThinkingBlock
   自然衔接。assistant 主题色继承自 `.msg--assistant .msg__bubble`。 */
.msg__bubble--timeline {
  border: none;
  background: transparent;
  margin-top: 2px;
  margin-bottom: 2px;
  padding: 2px 0;
}

.msg__cursor {
  display: inline-block;
  margin-left: 2px;
  animation: blink 1s steps(1) infinite;
  color: var(--color-text-muted);
}

@keyframes blink {
  50% {
    opacity: 0;
  }
}

/* D3 PR3 (2026-06-17): "(edited)" label. Sits inline at the
   bottom-right of the bubble when the row's metadata has
   `edited_at`. Visually quiet (small mono grey, no border,
   no padding) so it doesn't compete with the bubble content
   or the F5 latency chip below. The `margin-left: auto`
   pushes it to the right edge of the bubble's flex column;
   for assistant bubbles the chip stays on the bubble's
   right side, matching the bubble's bottom-right
   alignment convention (the F5 latency chip lives
   separately below the bubble). */
.msg__edited {
  display: inline-flex;
  align-self: flex-end;
  margin-top: 2px;
  font-size: var(--text-2xs);
  font-family: var(--font-mono);
  color: var(--color-text-muted);
  font-style: italic;
  user-select: none;
}

/* Markdown content (v-html). The HTML lives in a child tree without
   scoped classes, so every selector below uses :deep() to reach into
   the rendered output. Keep the list focused on elements marked
   actually produces — avoid hypothetical selectors that will never
   match and just become dead code. */
.msg__markdown {
  display: block;
}

.msg__markdown :deep(p) {
  margin: 0 0 8px 0;
}

.msg__markdown :deep(p:last-child) {
  margin-bottom: 0;
}

.msg__markdown :deep(h1),
.msg__markdown :deep(h2),
.msg__markdown :deep(h3),
.msg__markdown :deep(h4),
.msg__markdown :deep(h5),
.msg__markdown :deep(h6) {
  margin: 12px 0 6px 0;
  font-weight: var(--weight-semibold);
  line-height: 1.3;
}

.msg__markdown :deep(h1) {
  font-size: 1.4em;
}
.msg__markdown :deep(h2) {
  font-size: 1.25em;
}
.msg__markdown :deep(h3) {
  font-size: 1.1em;
}
.msg__markdown :deep(h4) {
  font-size: 1em;
}

.msg__markdown :deep(h1:first-child),
.msg__markdown :deep(h2:first-child),
.msg__markdown :deep(h3:first-child),
.msg__markdown :deep(h4:first-child) {
  margin-top: 0;
}

.msg__markdown :deep(ul),
.msg__markdown :deep(ol) {
  margin: 6px 0;
  padding-left: 24px;
}

.msg__markdown :deep(li) {
  margin: 2px 0;
}

.msg__markdown :deep(strong) {
  font-weight: var(--weight-semibold);
}

.msg__markdown :deep(em) {
  font-style: italic;
}

.msg__markdown :deep(code) {
  font-family: var(--font-mono);
  font-size: 0.9em;
  padding: 1px 5px;
  border-radius: 3px;
  background: color-mix(in srgb, var(--color-text-primary) 8%, transparent);
  border: 1px solid var(--color-bg-border-strong);
}

.msg__markdown :deep(pre) {
  margin: 8px 0;
  padding: 10px 12px;
  background: color-mix(in srgb, var(--color-text-primary) 6%, transparent);
  border: 1px solid var(--color-bg-border-strong);
  border-radius: var(--radius-md);
  overflow-x: auto;
  line-height: 1.45;
}

.msg__markdown :deep(pre code) {
  padding: 0;
  background: transparent;
  border: 0;
  font-size: 0.9em;
  white-space: pre;
}

.msg__markdown :deep(a) {
  color: var(--color-accent);
  text-decoration: underline;
  text-underline-offset: 2px;
}

.msg__markdown :deep(blockquote) {
  margin: 8px 0;
  padding: 4px 12px;
  border-left: 3px solid var(--color-bg-border);
  color: var(--color-text-secondary);
  font-style: italic;
}

.msg__markdown :deep(hr) {
  border: 0;
  border-top: 1px solid var(--color-bg-border);
  margin: 12px 0;
}

.msg__markdown :deep(table) {
  border-collapse: collapse;
  margin: 8px 0;
  font-size: 0.95em;
}

.msg__markdown :deep(th),
.msg__markdown :deep(td) {
  /* Stronger border color than --color-bg-border because table cells
     sit on --color-bg-elevated (the bubble) and the regular border
     reads as invisible (only 4 luminance units of separation). */
  border: 1px solid var(--color-bg-border-strong);
  padding: 4px 8px;
  text-align: left;
}

.msg__markdown :deep(th) {
  background: var(--color-bg);
  font-weight: var(--weight-semibold);
}
</style>
