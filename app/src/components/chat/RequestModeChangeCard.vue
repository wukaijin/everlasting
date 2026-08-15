<script setup lang="ts">
// RequestModeChangeCard — inline message card for the
// `request_mode_change` blocking tool (Phase C of
// `07-07-07-07-request-mode-change-tool`, 2026-07-07).
//
// Per PRD R6 / design §5.5 (UI 红线 — inherited from the
// ask_user_question pattern):
//   - **inline card**, NEVER modal — the DOM is a child of the
//     message stream (mounted by MessageItem.vue's tool-name
//     dispatch in Phase D, sitting below `<ToolCallCard>`)
//   - NO reka-ui `Dialog`/`Popover`/`AlertDialog` portals
//   - NO `<Teleport to="body">`
//   - NO floating overlay / mask / backdrop
//
// Why no modal: the request_mode_change tool shares the
// session-singleton mutex with ask_user_question (PRD R12 —
// one pending interaction per session); modals can't survive
// session-switch re-renders cleanly (the user can switch to
// another session, work there, switch back, and the card is
// still answerable). Inline cards ride on the message stream's
// normal scroll / render lifecycle.
//
// Card shape (PRD R8): single card with a header chip naming
// the target mode + the LLM's reason + a before/after mode
// comparison pill + the bottom action row (允许 / 拒绝). Three
// states:
//   - pending  → two action buttons + reason + comparison row
//   - allowed  → "已切换" pill + before/after comparison
//   - denied   → "已拒绝" pill
//
// Yolo handling (design §6 + implement.md §7 cross-phase
// notes): when targetMode === "yolo" and the user clicks
// 允许, the card DOES NOT call `resolveModeChange` directly.
// It calls `chatStore.requestSetMode(sid, "yolo")` to trigger
// the existing `pendingYoloConfirm` modal flow (the Yolo
// gate is shared with the user-initiated Shift+Tab / popover
// paths). The modal's confirm handler extends `confirmYolo`
// with an optional `pendingResolveRequest` parameter that
// fires `resolveModeChange` AFTER the Yolo IPC succeeds —
// see `stores/chat.ts`'s `confirmYolo` doc comment for the
// full timing. On Yolo cancel, `cancelYolo(pendingResolveRequest)`
// fires `resolveModeChange(allow=false)` so the agent loop
// sees `cancelled_by_user`.
//
// "Noop" handling: backend short-circuits the tool when
// target_mode == current_mode and never emits a card (no
// `mode:change:request` event). The card doesn't need a
// special noop UI path.
//
// Audit: NO new audit writes — the backend already writes
// `mode_change_requested` / `mode_change_allowed` /
// `mode_change_denied` at the tool entry / IPC exit. The
// card is purely a UI surface.

import { computed, ref } from "vue";

import Icon from "../Icon.vue";
import { extractErrorMessage } from "../../utils/useErrorBus";
import { useChatStore } from "../../stores/chat";
import { useQuestionCardsStore } from "../../stores/questionCards";
import type { SessionMode } from "../../stores/chat.types";

interface Props {
  /** Active session id (for routing the resolve IPC). */
  sessionId: string;
  /** The tool_use_id from the LLM's
   *  `ToolUse(request_mode_change)` block — echoed back in the
   *  resolve payload so the backend can pair the resolve with
   *  the right oneshot (QuestionStore keys by session_id; the
   *  tool_use_id is the matching aid). */
  toolUseId: string;
  /** The mode the LLM wants (`"edit" | "plan" | "yolo"`).
   *  Backend-validated against the enum at the tool entry;
   *  the card assumes a valid value. */
  targetMode: "edit" | "plan" | "yolo";
  /** Current mode snapshot (server-side) at tool-invocation
   *  time. May be `null` for the rehydrated historical path
   *  (pre-PR data, defensive). */
  currentMode?: string | null;
  /** LLM-supplied explanation (≤500 chars, optional). */
  reason?: string | null;
  /** Initial state — `pending` when freshly mounted from the
   *  live `mode:change:request` event, `allowed` / `denied`
   *  when rehydrated from a tool_result. Defaults to
   *  `"pending"` so the parent (MessageItem) doesn't have to
   *  thread state for the common mount case. The card flips
   *  its own `localState` on submit / cancel. */
  state?: "pending" | "allowed" | "denied";
  /** When the card mounts directly in `allowed` state
   *  (rehydrated history path), this carries the mode the
   *  session was switched to. Used by the "已切换" pill + the
   *  before/after comparison row. */
  allowedMode?: "edit" | "plan" | "yolo" | null;
}

const props = withDefaults(defineProps<Props>(), {
  currentMode: null,
  reason: null,
  state: "pending",
  allowedMode: null,
});

const emit = defineEmits<{
  /** Fired on successful allow — card flips to "allowed" right
   *  before this emit fires (non-Yolo path; the Yolo path lets
   *  the parent unmount the card via the cards store clearing). */
  (e: "allowed", newMode: SessionMode): void;
  /** Fired on successful deny / cancel — card flips to
   *  "denied" right before. */
  (e: "denied"): void;
}>();

// ---------------------------------------------------------------------
// Local state
// ---------------------------------------------------------------------

/** Card's local view of the state. Tracks the prop on mount but
 *  flips on submit / cancel so the bottom action row transitions
 *  to "allowed" / "denied" without parent re-rendering. */
const localState = ref<"pending" | "allowed" | "denied">(props.state);

/** In-flight guard — disables both buttons while the resolve
 *  IPC is pending. The optimistic state flip happens AFTER the
 *  IPC resolves; on rejection, we revert + surface an inline
 *  error so the user can retry. (For the Yolo path the
 *  `submitting` flag stays true through the modal flow — the
 *  card unmounts when the cards store clears, so a stale true
 *  is harmless.) */
const submitting = ref<boolean>(false);

/** Last submit error (string message). Shown inline above the
 *  bottom row in the pending state when non-null; cleared on
 *  the next attempt. */
const submitError = ref<string | null>(null);

// ---------------------------------------------------------------------
// Derived display helpers
// ---------------------------------------------------------------------

/** The mode actually applied, for the "allowed" pill text. When
 *  the user allowed a non-Yolo change, this equals
 *  `props.targetMode`. For the Yolo path, after the Yolo modal
 *  confirms, the resolved session mode is also
 *  `props.targetMode` (= "yolo"). For the rehydrated historical
 *  path, we prefer the explicit `props.allowedMode` (the
 *  rehydrated SessionRow.mode), falling back to `targetMode`
 *  when not provided. */
const effectiveAllowedMode = computed<"edit" | "plan" | "yolo">(() => {
  if (
    props.allowedMode === "edit" ||
    props.allowedMode === "plan" ||
    props.allowedMode === "yolo"
  ) {
    return props.allowedMode;
  }
  return props.targetMode;
});

/** Display label for a mode value (matches `ModeSelect.modeOptions`'s
 *  English brand names). */
function modeLabel(mode: "edit" | "plan" | "yolo"): string {
  if (mode === "edit") return "Edit";
  if (mode === "plan") return "Plan";
  return "Yolo";
}

/** Color modifier class for a mode value, applied to the
 *  root card + the header icon + the primary action button.
 *  Mirrors `ModeSelect`'s convention:
 *   - edit (默认, full power) → accent blue
 *   - plan (read-only, safe)   → tool-read cyan
 *   - yolo (no-ask, 危险)      → tool-error red
 *  The header + primary button use the TARGET mode color
 *  (signals "what mode is being asked for"). */
function modeColorClass(mode: "edit" | "plan" | "yolo"): string {
  if (mode === "plan") return "mode-card--plan";
  if (mode === "yolo") return "mode-card--yolo";
  return "mode-card--edit";
}

const targetColorClass = computed<string>(() =>
  modeColorClass(props.targetMode),
);

/** Narrowing helper for the `currentMode` prop — it's
 *  typed as `string | null` (defensive for legacy rehydrate
 *  data) but the card's comparison row wants a narrow union. */
function isKnownMode(
  m: string | null | undefined,
): m is "edit" | "plan" | "yolo" {
  return m === "edit" || m === "plan" || m === "yolo";
}

// ---------------------------------------------------------------------
// Submit / deny handlers
// ---------------------------------------------------------------------

const chatStore = useChatStore();
const cardsStore = useQuestionCardsStore();

/** Click on 允许. Two paths:
 *
 *  1. Non-Yolo: directly call `resolveModeChange(true)` → DB
 *     mode update + `mode_change_allowed` audit + oneshot
 *     resolve + store clears the pending entry → MessageItem
 *     re-renders without the inline card (the tool_result
 *     block on the message stream carries the final outcome).
 *
 *  2. Yolo: route through the shared `requestSetMode(sid,
 *     "yolo")` → existing `pendingYoloConfirm` modal → user
 *     confirms → `confirmYolo(pendingResolveRequest)` fires
 *     Yolo IPC THEN `resolveModeChange(true)`. User cancels
 *     modal → `cancelYolo(pendingResolveRequest)` fires
 *     `resolveModeChange(false)`. The store clears
 *     `pendingBySession[sid]` after the resolve fires, so
 *     MessageItem's dispatch unmounts the inline card.
 *
 *  The shared Yolo modal flow keeps the UX identical to the
 *  user-initiated Shift+Tab / ModeSelect paths (design §6
 *  decision: "preserve Yolo二次确认 UX"). */
async function onAllow(): Promise<void> {
  if (submitting.value) return;
  if (props.targetMode === "yolo") {
    submitting.value = true;
    submitError.value = null;
    // Write the pending resolve into the chat store BEFORE
    // opening the modal — the modal-driven extended
    // `confirmYolo` / `cancelYolo` reads it from
    // `chatStore.pendingResolveRequest` and fires
    // `resolveModeChange` after the Yolo IPC resolves.
    chatStore.pendingResolveRequest = {
      sessionId: props.sessionId,
      toolUseId: props.toolUseId,
      targetMode: props.targetMode,
    };
    // Fire-and-forget: `requestSetMode` flips the modal flag
    // and returns false (the modal gates the actual apply).
    // The modal confirm handler (in ModeSelect.vue's
    // `onYoloConfirm`) calls our extended `confirmYolo`
    // which reads + clears `pendingResolveRequest`.
    void chatStore.requestSetMode(props.sessionId, "yolo");
    // Don't reset `submitting` — the card unmounts when the
    // store clears, so a stale true is harmless. We also
    // can't await "modal completion" here: `requestSetMode`
    // returns immediately after flipping the modal flag.
    return;
  }

  // Non-Yolo path: directly resolve.
  submitting.value = true;
  submitError.value = null;
  try {
    await cardsStore.resolveModeChange(
      props.sessionId,
      props.toolUseId,
      props.targetMode,
      true,
    );
    localState.value = "allowed";
    emit("allowed", props.targetMode);
  } catch (e) {
    submitError.value = extractErrorMessage(e);
  } finally {
    submitting.value = false;
  }
}

/** Click on 拒绝. Always directly fires
 *  `resolveModeChange(allow=false)` — the Yolo flow doesn't
 *  have a "deny" step (the user just clicks 拒绝 and the
 *  backend records `mode_change_denied`). */
async function onDeny(): Promise<void> {
  if (submitting.value) return;
  submitting.value = true;
  submitError.value = null;
  try {
    await cardsStore.resolveModeChange(
      props.sessionId,
      props.toolUseId,
      props.targetMode,
      false,
    );
    localState.value = "denied";
    emit("denied");
  } catch (e) {
    submitError.value = extractErrorMessage(e);
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <div
    class="mode-card"
    :class="targetColorClass"
    data-testid="mode-card"
  >
    <div class="mode-card__head">
      <span class="mode-card__head-icon">
        <Icon name="refresh" :size="14" />
      </span>
      <span class="mode-card__head-title">
        切换到 {{ modeLabel(targetMode) }}
      </span>
      <span
        v-if="localState === 'allowed'"
        class="mode-card__state mode-card__state--allowed"
        data-testid="mode-card-state-allowed"
      >✓ 已切换</span>
      <span
        v-else-if="localState === 'denied'"
        class="mode-card__state mode-card__state--denied"
        data-testid="mode-card-state-denied"
      >⊘ 已拒绝</span>
    </div>

    <p
      v-if="reason && localState === 'pending'"
      class="mode-card__reason"
      data-testid="mode-card-reason"
    >{{ reason }}</p>

    <!--
      Mode comparison row. Renders only in `pending` + `allowed`
      states (the `denied` state hides the comparison — the user
      chose not to switch, so the "before/after" is moot).
    -->
    <div
      v-if="localState === 'pending' && isKnownMode(currentMode)"
      class="mode-card__compare"
      data-testid="mode-card-compare"
    >
      <span class="mode-card__compare-from">{{ modeLabel(currentMode) }}</span>
      <span class="mode-card__compare-arrow" aria-hidden="true">→</span>
      <span class="mode-card__compare-to">{{ modeLabel(targetMode) }}</span>
    </div>
    <div
      v-else-if="localState === 'allowed'"
      class="mode-card__compare mode-card__compare--after"
      data-testid="mode-card-compare-after"
    >
      <span class="mode-card__compare-from">
        {{ modeLabel(isKnownMode(currentMode) ? currentMode : effectiveAllowedMode) }}
      </span>
      <span class="mode-card__compare-arrow" aria-hidden="true">→</span>
      <span class="mode-card__compare-to">
        {{ modeLabel(effectiveAllowedMode) }}
      </span>
    </div>

    <!--
      Bottom action row (pending state only). Two buttons,
      equal width, 允许 on the left + 拒绝 on the right. The
      允许 button color follows the target mode (matches the
      ModeSelect trigger convention: plan=cyan, edit=accent,
      yolo=red).
    -->
    <div
      v-if="localState === 'pending'"
      class="mode-card__actions"
    >
      <p
        v-if="submitError"
        class="mode-card__error"
        role="alert"
        data-testid="mode-card-error"
      >{{ submitError }}</p>
      <button
        type="button"
        class="mode-card__btn mode-card__btn--primary"
        :class="targetColorClass"
        :disabled="submitting"
        data-testid="mode-card-allow"
        @click="onAllow"
      >
        <Icon name="check" :size="12" />
        允许
      </button>
      <button
        type="button"
        class="mode-card__btn"
        :disabled="submitting"
        data-testid="mode-card-deny"
        @click="onDeny"
      >拒绝</button>
    </div>

    <p
      v-else-if="localState === 'denied'"
      class="mode-card__denied-note"
      data-testid="mode-card-denied-note"
    >用户拒绝切换,LLM 会按取消处理。</p>
  </div>
</template>

<style scoped>
/*
 * Visual contract — reuses the project's ToolCallCard chrome
 * tokens (per design §5.5: "复用 ToolCallCard 现有 Card 样式系统").
 * The card sits inline in the message stream (message-flow
 * child, no portal, no overlay). All CSS references project
 * tokens — no hardcoded hex.
 *
 * Root class `.mode-card` is namespaced (no collision with
 * `.ask-card` / `.tool-card` / `.permission-ask-body` etc).
 *
 * Color tokens per mode (mirrors `ModeSelect`):
 *   - edit  → var(--color-accent)        (default blue)
 *   - plan  → var(--color-tool-read)     (cyan)
 *   - yolo  → var(--color-tool-error)    (red)
 * Mode-specific accents go in the `--plan` / `--yolo` / `--edit`
 * modifier classes on the root, the state pill, and the primary
 * button — reuses the existing 3-mode visual convention without
 * introducing new color tokens.
 */
.mode-card {
  margin-top: 8px;
  padding: var(--space-3);
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  font-family: var(--font-sans);
  color: var(--color-text-primary);
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}

/* Left accent bar — color depends on target mode. Mirrors the
   AskUserQuestionCard's left-bar accent but in 3 mode colors. */
.mode-card--edit {
  border-left: 3px solid var(--color-accent);
}
.mode-card--plan {
  border-left: 3px solid var(--color-tool-read);
}
.mode-card--yolo {
  border-left: 3px solid var(--color-tool-error);
}

.mode-card__head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  font-size: var(--text-sm);
  font-weight: var(--weight-semibold);
}

.mode-card__head-icon {
  display: inline-flex;
  flex-shrink: 0;
}
.mode-card--edit .mode-card__head-icon {
  color: var(--color-accent-text);
}
.mode-card--plan .mode-card__head-icon {
  color: var(--color-tool-read);
}
.mode-card--yolo .mode-card__head-icon {
  color: var(--color-tool-error-text);
}

.mode-card__head-title {
  flex: 1;
}

/* Status pills — allowed (tool-write green) / denied (tool-error
   red). Mirrors `AskUserQuestionCard` state pill structure. */
.mode-card__state {
  font-size: var(--text-xs);
  font-family: var(--font-mono);
  padding: 2px 8px;
  border-radius: var(--radius-pill);
  border: 1px solid currentColor;
}

.mode-card__state--allowed {
  color: var(--color-tool-write);
}
.mode-card__state--denied {
  color: var(--color-tool-error-text);
}

.mode-card__reason {
  margin: 0;
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
  line-height: var(--leading-normal);
  white-space: pre-wrap;
  word-break: break-word;
}

/* Mode comparison row — "Edit → Yolo" style. Two pills
   separated by an arrow icon. Uses the same color tokens as
   the head icon. */
.mode-card__compare {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: var(--text-xs);
  font-family: var(--font-mono);
  color: var(--color-text-secondary);
}
.mode-card__compare-from {
  padding: 1px 6px;
  border: 1px solid var(--color-bg-border-strong);
  border-radius: var(--radius-sm);
  background: var(--color-bg-elevated);
}
.mode-card__compare-to {
  padding: 1px 6px;
  border: 1px solid currentColor;
  border-radius: var(--radius-sm);
}
.mode-card--edit .mode-card__compare-to {
  color: var(--color-accent-text);
}
.mode-card--plan .mode-card__compare-to {
  color: var(--color-tool-read);
}
.mode-card--yolo .mode-card__compare-to {
  color: var(--color-tool-error-text);
}

.mode-card__compare-arrow {
  flex-shrink: 0;
  color: var(--color-text-muted);
}

/* Allowed state comparison — uses the same row but tints the
   "from" pill to indicate "where we came from" (muted) vs
   "to" (highlighted, already-applied mode). */
.mode-card__compare--after .mode-card__compare-from {
  color: var(--color-text-muted);
  border-style: dashed;
}

/* Bottom action row (pending state). Two equal-width buttons
   side-by-side. */
.mode-card__actions {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
}

.mode-card__btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 4px;
  flex: 1;
  padding: 6px 12px;
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
  border: 1px solid var(--color-bg-border-strong);
  border-radius: var(--radius-sm);
  font-size: var(--text-sm);
  font-family: inherit;
  cursor: pointer;
  transition: background-color var(--duration-fast) var(--ease-out),
    border-color var(--duration-fast) var(--ease-out);
}
.mode-card__btn:hover:not(:disabled) {
  background: var(--color-bg-hover);
  border-color: var(--color-accent);
}
.mode-card__btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

/* Primary (允许) button — color follows target mode. The
   `.mode-card__btn--primary` base sets the "filled" look
   (matching AskUserQuestionCard's primary button); the
   `--plan` / `--yolo` / `--edit` modifiers tint it. */
.mode-card__btn--primary {
  color: var(--color-text-on-accent);
}
.mode-card__btn--primary.mode-card--edit {
  background: var(--color-accent);
  border-color: var(--color-accent);
}
.mode-card__btn--primary.mode-card--edit:hover:not(:disabled) {
  background: var(--color-accent-hover);
  border-color: var(--color-accent-hover);
}
.mode-card__btn--primary.mode-card--plan {
  background: var(--color-tool-read);
  border-color: var(--color-tool-read);
}
.mode-card__btn--primary.mode-card--plan:hover:not(:disabled) {
  filter: brightness(1.1);
}
.mode-card__btn--primary.mode-card--yolo {
  background: var(--color-tool-error);
  border-color: var(--color-tool-error);
}
.mode-card__btn--primary.mode-card--yolo:hover:not(:disabled) {
  filter: brightness(1.1);
}

.mode-card__error {
  flex: 1;
  margin: 0;
  font-size: var(--text-xs);
  color: var(--color-tool-error-text);
  font-family: var(--font-mono);
}

.mode-card__denied-note {
  margin: 0;
  font-size: var(--text-xs);
  color: var(--color-tool-error-text);
  font-style: italic;
}
</style>