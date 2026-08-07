// RequestModeChangeCard 状态解析簇(拆分自 MessageItem.vue, 08-07-large-file-splitting)。
// 纯函数:入参 message + toolUseId,不读组件作用域。

import type { ChatMessage } from "../../../stores/chat.types";
import { getToolResult } from "../../../utils/messageFormat";
import { useChatStore } from "../../../stores/chat";
import { useQuestionCardsStore } from "../../../stores/questionCards";
import { REQUEST_MODE_CHANGE_TOOL_NAME } from "../../../stores/questionCards.types";
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
export function parseModeChangeEnvelope(
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
export function resolveModeChangeCardState(message: ChatMessage, toolUseId: string): {
  state: "pending" | "allowed" | "denied";
  targetMode: "edit" | "plan" | "yolo";
  currentMode: string | null;
  reason: string | null;
  allowedMode: "edit" | "plan" | "yolo" | null;
} | null {
  const chatStore = useChatStore();
  const questionCardsStore = useQuestionCardsStore();
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
  const tr = getToolResult(message, toolUseId);
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

export function resolveModeChangeCardProps(message: ChatMessage, toolUseId: string):
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
  const chatStore = useChatStore();
  const resolved = resolveModeChangeCardState(message, toolUseId);
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
export function modeChangeCardPropsFor(message: ChatMessage, tc: { id: string; name: string }):
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
  return resolveModeChangeCardProps(message, tc.id);
}