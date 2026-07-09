<script setup lang="ts">
// RequestTaskStateTransitionCard — inline message card for the
// `request_task_state_transition` blocking tool
// (`07-09-workflow-transition-card`, 2026-07-09).
//
// Sibling of `RequestModeChangeCard.vue`. Same UI 红线 (PRD R6 /
// design §5.5): inline card, NEVER modal — the DOM is a child of
// the message stream (mounted by MessageItem.vue's tool-name
// dispatch, sitting below `<ToolCallCard>`). NO reka-ui
// `Dialog`/`Popover`/`AlertDialog` portals, NO `<Teleport>`, NO
// floating overlay. Inline cards ride on the message stream's
// normal scroll / render lifecycle + survive session-switch
// re-renders (the backend's QuestionStore holds the pending
// state across sessions).
//
// Card shape: single card with a header chip naming the target
// state + the agent's reason + a before/after state comparison
// row + the bottom action row (允许 / 拒绝). Three states:
//   - pending  → two action buttons + reason + comparison row
//   - allowed  → "已转换" pill + before/after comparison
//   - denied   → "已拒绝" pill
//
// Simpler than `RequestModeChangeCard`:
//   - NO Yolo special-case (workflow states have no analogue of
//     Yolo's shared-confirm-modal gate).
//   - NO per-state color mapping — a single accent color is used
//     (the four states don't carry the danger/safe semantics edit/
//     plan/yolo do; a uniform accent keeps the visual simple).
//   - NO session-summary mode patch — a workflow transition does
//     not change the session's edit/plan/yolo mode (the store
//     action only clears the pending card).
//
// `slug` flows through as a prop solely so the resolve handler can
// pass it to the `resolve_task_state_transition` IPC (the backend
// handler has no WorkflowCtx and locates
// `<project>/.everlasting/tasks/<slug>/task.json` to read the
// current `from` state off disk). It's not displayed.

import { ref } from "vue";

import Icon from "../Icon.vue";
import { extractErrorMessage } from "../../utils/useErrorBus";
import { useQuestionCardsStore } from "../../stores/questionCards";
import type { WorkflowState } from "../../stores/questionCards.types";

interface Props {
  /** Active session id (for routing the resolve IPC). */
  sessionId: string;
  /** The tool_use_id from the LLM's
   *  `ToolUse(request_task_state_transition)` block — echoed back
   *  in the resolve payload so the backend can pair the resolve
   *  with the right oneshot (QuestionStore keys by session_id; the
   *  tool_use_id is the matching aid). */
  toolUseId: string;
  /** The workflow state the agent wants to move to. Backend-
   *  validated against the workflow's `states` at tool entry; the
   *  card assumes a valid value. */
  targetState: WorkflowState;
  /** The task slug — REQUIRED by the resolve IPC (the backend
   *  handler locates `<project>/.everlasting/tasks/<slug>/task.json`
   *  to read the current `from` state off disk). Passed through
   *  from the event payload; not displayed. */
  slug: string;
  /** Current state snapshot (server-side) at tool-invocation time.
   *  May be `null` for the rehydrated historical path or when the
   *  backend had no workflow task resolved. */
  currentState?: WorkflowState | null;
  /** Agent-supplied explanation (≤500 chars, optional). */
  reason?: string | null;
  /** Initial state — `pending` when freshly mounted from the live
   *  `task:state:transition:request` event, `allowed` / `denied`
   *  when rehydrated from a tool_result. Defaults to `"pending"`. */
  state?: "pending" | "allowed" | "denied";
}

const props = withDefaults(defineProps<Props>(), {
  currentState: null,
  reason: null,
  state: "pending",
});

const emit = defineEmits<{
  /** Fired on successful allow — card flips to "allowed" right
   *  before this emit fires. */
  (e: "allowed", newState: WorkflowState): void;
  /** Fired on successful deny — card flips to "denied" right
   *  before. */
  (e: "denied"): void;
}>();

// ---------------------------------------------------------------------
// Local state
// ---------------------------------------------------------------------

/** Card's local view of the state. Tracks the prop on mount but
 *  flips on submit / cancel so the bottom action row transitions
 *  to "allowed" / "denied" without parent re-rendering. */
const localState = ref<"pending" | "allowed" | "denied">(props.state);

/** In-flight guard — disables both buttons while the resolve IPC
 *  is pending. The optimistic state flip happens AFTER the IPC
 *  resolves; on rejection, we revert + surface an inline error so
 *  the user can retry. */
const submitting = ref<boolean>(false);

/** Last submit error (string message). Shown inline above the
 *  bottom row in the pending state when non-null; cleared on the
 *  next attempt. */
const submitError = ref<string | null>(null);

// ---------------------------------------------------------------------
// Derived display helpers
// ---------------------------------------------------------------------

/** Display label for a workflow state value. Matches the dev
 *  plugin's breadcrumb Chinese (planning=规划 / implement=实现 /
 *  check=校验 / done=完成) so the card stays consistent with the
 *  breadcrumb the agent sees in the same turn. */
function stateLabel(state: WorkflowState): string {
  if (state === "planning") return "规划";
  if (state === "implement") return "实现";
  if (state === "check") return "校验";
  return "完成";
}

// ---------------------------------------------------------------------
// Submit / deny handlers
// ---------------------------------------------------------------------

const cardsStore = useQuestionCardsStore();

/** Click on 允许. Directly calls
 *  `resolveTaskStateTransition(allow=true)` → backend's
 *  `set_task_state` writes task.json.status + dispatches the
 *  `from → to` Rust hook + writes `task_state_transition_allowed`
 *  audit + resolves the oneshot → store clears the pending entry
 *  → MessageItem re-renders without the inline card (the
 *  tool_result block on the message stream carries the final
 *  outcome). */
async function onAllow(): Promise<void> {
  if (submitting.value) return;
  submitting.value = true;
  submitError.value = null;
  try {
    await cardsStore.resolveTaskStateTransition(
      props.sessionId,
      props.toolUseId,
      props.targetState,
      props.slug,
      true,
    );
    localState.value = "allowed";
    emit("allowed", props.targetState);
  } catch (e) {
    submitError.value = extractErrorMessage(e);
  } finally {
    submitting.value = false;
  }
}

/** Click on 拒绝. Fires `resolveTaskStateTransition(allow=false)` —
 *  the backend skips `set_task_state` entirely, records
 *  `task_state_transition_denied` audit, and resolves the oneshot
 *  as `Cancelled` (the tool_result becomes
 *  `{ cancelled_by_user: true }`). */
async function onDeny(): Promise<void> {
  if (submitting.value) return;
  submitting.value = true;
  submitError.value = null;
  try {
    await cardsStore.resolveTaskStateTransition(
      props.sessionId,
      props.toolUseId,
      props.targetState,
      props.slug,
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
  <div class="wf-state-card" data-testid="wf-state-card">
    <div class="wf-state-card__head">
      <span class="wf-state-card__head-icon">
        <Icon name="refresh" :size="14" />
      </span>
      <span class="wf-state-card__head-title">
        工作流状态转移 · {{ stateLabel(targetState) }}
      </span>
      <span
        v-if="localState === 'allowed'"
        class="wf-state-card__state wf-state-card__state--allowed"
        data-testid="wf-state-card-state-allowed"
      >✓ 已转换</span>
      <span
        v-else-if="localState === 'denied'"
        class="wf-state-card__state wf-state-card__state--denied"
        data-testid="wf-state-card-state-denied"
      >⊘ 已拒绝</span>
    </div>

    <p
      v-if="reason && localState === 'pending'"
      class="wf-state-card__reason"
      data-testid="wf-state-card-reason"
    >{{ reason }}</p>

    <!--
      State comparison row. Renders only in `pending` + `allowed`
      states (the `denied` state hides the comparison — the user
      chose not to transition, so "before/after" is moot).
    -->
    <div
      v-if="localState === 'pending' && currentState"
      class="wf-state-card__compare"
      data-testid="wf-state-card-compare"
    >
      <span class="wf-state-card__compare-from">{{ stateLabel(currentState) }}</span>
      <span class="wf-state-card__compare-arrow" aria-hidden="true">→</span>
      <span class="wf-state-card__compare-to">{{ stateLabel(targetState) }}</span>
    </div>
    <div
      v-else-if="localState === 'allowed'"
      class="wf-state-card__compare wf-state-card__compare--after"
      data-testid="wf-state-card-compare-after"
    >
      <span class="wf-state-card__compare-from">
        {{ stateLabel(currentState ?? targetState) }}
      </span>
      <span class="wf-state-card__compare-arrow" aria-hidden="true">→</span>
      <span class="wf-state-card__compare-to">{{ stateLabel(targetState) }}</span>
    </div>

    <!--
      Bottom action row (pending state only). Two buttons, 允许 on
      the left + 拒绝 on the right.
    -->
    <div
      v-if="localState === 'pending'"
      class="wf-state-card__actions"
    >
      <p
        v-if="submitError"
        class="wf-state-card__error"
        role="alert"
        data-testid="wf-state-card-error"
      >{{ submitError }}</p>
      <button
        type="button"
        class="wf-state-card__btn wf-state-card__btn--primary"
        :disabled="submitting"
        data-testid="wf-state-card-allow"
        @click="onAllow"
      >
        <Icon name="check" :size="12" />
        允许
      </button>
      <button
        type="button"
        class="wf-state-card__btn"
        :disabled="submitting"
        data-testid="wf-state-card-deny"
        @click="onDeny"
      >拒绝</button>
    </div>

    <p
      v-else-if="localState === 'denied'"
      class="wf-state-card__denied-note"
      data-testid="wf-state-card-denied-note"
    >用户拒绝状态转移,LLM 会按取消处理。</p>
  </div>
</template>

<style scoped>
/*
 * Visual contract — reuses the project's ToolCallCard chrome tokens
 * (per design §5.5: "复用 ToolCallCard 现有 Card 样式系统"). The
 * card sits inline in the message stream (message-flow child, no
 * portal, no overlay). All CSS references project tokens — no
 * hardcoded hex.
 *
 * Root class `.wf-state-card` is namespaced (no collision with
 * `.mode-card` / `.ask-card` / `.tool-card`). A single accent color
 * (no per-state color mapping) — workflow states don't carry the
 * danger/safe semantics edit/plan/yolo do.
 */
.wf-state-card {
  margin-top: 8px;
  padding: var(--space-3);
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-left: 3px solid var(--color-accent);
  border-radius: var(--radius-md);
  font-family: var(--font-sans);
  color: var(--color-text-primary);
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}

.wf-state-card__head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  font-size: var(--text-sm);
  font-weight: var(--weight-semibold);
}

.wf-state-card__head-icon {
  display: inline-flex;
  flex-shrink: 0;
  color: var(--color-accent);
}

.wf-state-card__head-title {
  flex: 1;
}

/* Status pills — allowed (tool-write green) / denied (tool-error
   red). Mirrors `AskUserQuestionCard` / `RequestModeChangeCard`
   state pill structure. */
.wf-state-card__state {
  font-size: var(--text-xs);
  font-family: var(--font-mono);
  padding: 2px 8px;
  border-radius: var(--radius-pill);
  border: 1px solid currentColor;
}

.wf-state-card__state--allowed {
  color: var(--color-tool-write);
}

.wf-state-card__state--denied {
  color: var(--color-tool-error);
}

.wf-state-card__reason {
  margin: 0;
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
  line-height: var(--leading-normal);
  white-space: pre-wrap;
  word-break: break-word;
}

/* State comparison row — "规划 → 实现" style. Two pills separated
   by an arrow icon. Uses the same color tokens as the head icon. */
.wf-state-card__compare {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: var(--text-xs);
  font-family: var(--font-mono);
  color: var(--color-text-secondary);
}
.wf-state-card__compare-from {
  padding: 1px 6px;
  border: 1px solid var(--color-bg-border-strong);
  border-radius: var(--radius-sm);
  background: var(--color-bg-elevated);
}
.wf-state-card__compare-to {
  padding: 1px 6px;
  border: 1px solid currentColor;
  border-radius: var(--radius-sm);
  color: var(--color-accent);
}

.wf-state-card__compare-arrow {
  flex-shrink: 0;
  color: var(--color-text-muted);
}

/* Allowed state comparison — "from" pill muted (dashed) to
   indicate history vs the highlighted "to" (applied state). */
.wf-state-card__compare--after .wf-state-card__compare-from {
  color: var(--color-text-muted);
  border-style: dashed;
}

/* Bottom action row (pending state). Two equal-width buttons. */
.wf-state-card__actions {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
}

.wf-state-card__btn {
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
.wf-state-card__btn:hover:not(:disabled) {
  background: var(--color-bg-hover);
  border-color: var(--color-accent);
}
.wf-state-card__btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

/* Primary (允许) button — accent filled. */
.wf-state-card__btn--primary {
  color: var(--color-text-on-accent);
  background: var(--color-accent);
  border-color: var(--color-accent);
}
.wf-state-card__btn--primary:hover:not(:disabled) {
  background: var(--color-accent-hover);
  border-color: var(--color-accent-hover);
}

.wf-state-card__error {
  flex: 1;
  margin: 0;
  font-size: var(--text-xs);
  color: var(--color-tool-error);
  font-family: var(--font-mono);
}

.wf-state-card__denied-note {
  margin: 0;
  font-size: var(--text-xs);
  color: var(--color-tool-error);
  font-style: italic;
}
</style>
