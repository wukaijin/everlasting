// AskUserQuestionCard 状态解析簇(拆分自 MessageItem.vue, 08-07-large-file-splitting)。
// 纯函数:入参 message + toolUseId,不读组件作用域。

import type { ChatMessage } from "../../../stores/chat.types";
import { getToolResult } from "../../../utils/messageFormat";
import { useChatStore } from "../../../stores/chat";
import { useQuestionCardsStore } from "../../../stores/questionCards";
import {
  ASK_USER_QUESTION_TOOL_NAME,
  type QuestionCardState,
  type ToolQuestionAnswer,
} from "../../../stores/questionCards.types";
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
export function parseAnswerEnvelope(
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
export function resolveAskCardState(message: ChatMessage, toolUseId: string): {
  state: QuestionCardState;
  questions: import("../../../stores/questionCards.types").Question[];
  selectedAnswer: ToolQuestionAnswer[] | undefined;
} | null {
  const chatStore = useChatStore();
  const questionCardsStore = useQuestionCardsStore();
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
  const tr = getToolResult(message, toolUseId);
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
      const synthQuestions: import("../../../stores/questionCards.types").Question[] =
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
 *  Memoization: the per-render computed keys on `message`
 *  (which carries `toolResults` + the live store reactive
 *  read inside `resolveAskCardState`) — Vue's reactivity
 *  re-evaluates this on every relevant change. We don't add
 *  a `computed` Map here because the message has at most a
 *  handful of tool_calls, and the parsing is cheap (a JSON.parse
 *  on a small string, at most once per render per tool call).
 */
export function resolveAskCardProps(message: ChatMessage, toolUseId: string):
  | {
      sessionId: string;
      toolUseId: string;
      questions: import("../../../stores/questionCards.types").Question[];
      state: QuestionCardState;
      selectedAnswer: ToolQuestionAnswer[] | undefined;
    }
  | undefined {
  const chatStore = useChatStore();
  const resolved = resolveAskCardState(message, toolUseId);
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
export function askCardPropsFor(message: ChatMessage, tc: { id: string; name: string }):
  | {
      sessionId: string;
      toolUseId: string;
      questions: import("../../../stores/questionCards.types").Question[];
      state: QuestionCardState;
      selectedAnswer: ToolQuestionAnswer[] | undefined;
    }
  | undefined {
  if (tc.name !== ASK_USER_QUESTION_TOOL_NAME) return undefined;
  return resolveAskCardProps(message, tc.id);
}