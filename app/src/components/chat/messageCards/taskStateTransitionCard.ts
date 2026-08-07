// RequestTaskStateTransitionCard 状态解析簇(拆分自 MessageItem.vue, 08-07-large-file-splitting)。
// 纯函数:入参 message + toolUseId,不读组件作用域。

import type { ChatMessage } from "../../../stores/chat.types";
import { getToolResult } from "../../../utils/messageFormat";
import { useChatStore } from "../../../stores/chat";
import { useQuestionCardsStore } from "../../../stores/questionCards";
import {
  REQUEST_TASK_STATE_TRANSITION_TOOL_NAME,
  type WorkflowState,
} from "../../../stores/questionCards.types";
// --- task_state_transition card dispatch (07-09-workflow-transition-card) --
// Mirrors the mode_change dispatch above. The envelope shapes differ:
//   allowed  → `{"allowed": true, "prev_state": "...", "new_state": "..."}`
//   denied   → `{"cancelled_by_user": true, "target_state": "..."}`
//   cancelled by session → `{"cancelled_by_session": true}`
// (vs mode_change's prev_mode/new_mode + no target_state on deny).

/** Parse a `request_task_state_transition` tool_result envelope
 *  (historical / rehydrate path). Returns the discriminated shape
 *  or `undefined` when the content isn't a recognized envelope. */
export function parseTaskStateTransitionEnvelope(
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
export function resolveTaskStateTransitionCardState(message: ChatMessage, toolUseId: string): {
  state: "pending" | "allowed" | "denied";
  targetState: WorkflowState;
  currentState: WorkflowState | null;
  slug: string;
  reason: string | null;
} | null {
  const chatStore = useChatStore();
  const questionCardsStore = useQuestionCardsStore();
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
  const tr = getToolResult(message, toolUseId);
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

export function resolveTaskStateTransitionCardProps(message: ChatMessage, toolUseId: string):
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
  const chatStore = useChatStore();
  const resolved = resolveTaskStateTransitionCardState(message, toolUseId);
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
export function taskStateTransitionCardPropsFor(message: ChatMessage, tc: { id: string; name: string }):
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
  return resolveTaskStateTransitionCardProps(message, tc.id);
}