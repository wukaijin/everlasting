<script setup lang="ts">
// WorkflowToggle — W1 (Workflow integration, Step 0.2 —
// 2026-07-08): per-session workflow opt-in chip. Mirrors
// `<ModeSelect>`'s shape (chip-in-input-row, no popover) but
// is a simple two-state toggle: the whole chip is the
// button — no menu, no Yolo-style confirm modal (workflow
// toggling is a plain UI preference, NOT a privileged
// operation).
//
// **Visual states**:
// - workflow OFF → ghost-style chip matching
//   `<ModeSelect>`'s inactive outline (`text-secondary`
//   with `bg-elevated` on hover). Label: `Wf`.
// - workflow ON  → accent-tinted chip indicating "agent
//   follows the active plugin's state machine". Label:
//   `Wf ●` (a single right-side dot, color = the workflow
//   accent). Tooltip explains the state.
//
// **Why a custom chip and not reusing `<ModeSelect>`**:
// ModeSelect has a 3-entry popover because Mode has 3 user
// modes. Workflow has only 2 (on / off) and the toggle
// target is small (a footnote-size chip on the chat input
// row). Different trigger + label + interaction surface — a
// dedicated component reads cleaner than overloading
// ModeSelect with a workflow branch.
//
// **Streaming / mid-turn semantics**: matches
// `set_session_mode` — the flip is accepted at any time and
// applies on the next turn boundary (see
// `agent/chat_loop.rs:396`). The chip label flips
// immediately so the user sees the change; the breadcrumb
// injection lands on the next turn (Phase 0 Step 0.5).
//
// **Placement**: mounted in `<ChatInput>` immediately to
// the right of `<ModeSelect>` (the B7 sibling). Both are
// peer chips on the same flex row; the input editor
// follows them on the same line.

import { computed } from "vue";

import { useChatStore } from "../../stores/chat";
import Icon from "../Icon.vue";

const chatStore = useChatStore();

/** True if there's an active session to toggle on. When no
 *  session is active we don't render the chip — there's
 *  nothing to switch. Mirrors `<ModeSelect>`'s
 *  `hasSession` gate so the input row doesn't gain a
 *  dangling chip before the user has picked a session. */
const hasSession = computed<boolean>(() => !!chatStore.currentSessionId);

/** Current workflow opt-in flag for the active session.
 *  Reads from the per-session `SessionSummary.workflow_enabled`
 *  wire field (rehydrated from `sessions.workflow_enabled`,
 *  defaulted to `false` for pre-Step-0.2 sessions via the
 *  `INTEGER NOT NULL DEFAULT 0` migration). Defaults to
 *  `false` when `hasSession` is false OR the summary
 *  lookup misses (defensive — should never happen when
 *  `hasSession` is true since `currentSessionId` is
 *  backed by the same sessions array). */
const workflowEnabled = computed<boolean>(() => {
  const sid = chatStore.currentSessionId;
  if (!sid) return false;
  const s = chatStore.sessions.find((x) => x.id === sid);
  return s?.workflow_enabled ?? false;
});

/** Click handler. Single click flips the flag via the
 *  chat store's `requestSetWorkflowEnabled` action (which
 *  owns the optimistic update + IPC + rollback; this
 *  component is dumb on purpose). No streaming guard —
 *  matches `requestSetMode`'s contract that the flip
 *  applies on the next turn. */
async function onToggle() {
  const sid = chatStore.currentSessionId;
  if (!sid) return;
  await chatStore.requestSetWorkflowEnabled(sid, !workflowEnabled.value);
}

/** Aria label differs based on the current state so screen
 *  reader users hear what the click will do, not just
 *  "Workflow toggle". */
const ariaLabel = computed<string>(() =>
  workflowEnabled.value
    ? "Workflow is ON. Click to turn OFF."
    : "Workflow is OFF. Click to turn ON.",
);
</script>

<template>
  <div
    v-if="hasSession"
    class="workflow-toggle"
  >
    <button
      type="button"
      class="workflow-toggle__chip"
      :class="{
        'workflow-toggle__chip--on': workflowEnabled,
      }"
      :aria-pressed="workflowEnabled"
      :title="
        workflowEnabled
          ? 'Workflow ON：agent 跟随当前 plugin 状态机(下一轮 turn 生效)'
          : 'Workflow OFF：默认 chat-loop 行为(无状态机注入)'
      "
      :aria-label="ariaLabel"
      @click="onToggle"
    >
      <!-- Active-state icon (bolt) sized inline with the
           label. The "Wf" prefix doubles as the chip
           mnemonic so the user can spot the chip without
           reading its tooltip. -->
      <Icon
        v-if="workflowEnabled"
        name="bolt"
        :size="12"
        class="workflow-toggle__icon"
      />
      <span class="workflow-toggle__label">Wf</span>
      <!-- State dot: solid accent when ON, hollow border
           when OFF. Sized at 6px so it's clearly visible
           without crowding the label; gap mirrors
           ModeSelect's chevron gap so the chips look like
           peers. -->
      <span
        class="workflow-toggle__dot"
        :class="{
          'workflow-toggle__dot--on': workflowEnabled,
        }"
        aria-hidden="true"
      />
    </button>
  </div>
</template>

<style scoped>
.workflow-toggle {
  position: relative;
  display: inline-flex;
}

/* Chip shape mirrors `<ModeSelect>`'s trigger pill so the
   two chips read as siblings on the input row. Same
   `font-mono` + `radius-md` + `text-base` recipe; the
   only difference is a slim margin-left so it sits next
   to ModeSelect without touching its border on focus. */
.workflow-toggle__chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 3px 8px;
  margin-left: 4px;
  background: transparent;
  border: 1px solid transparent;
  border-radius: var(--radius-md);
  color: var(--color-text-secondary);
  cursor: pointer;
  font: inherit;
  font-family: var(--font-mono);
  font-size: var(--text-base);
  font-weight: var(--weight-medium);
  /* Wf + dot = ~28px max; cap matches ModeSelect's
     `max-width: 120px` so the two chips don't asymmetrically
     stretch the row. */
  max-width: 56px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  transition: background var(--duration-fast) var(--ease-out), color var(--duration-fast) var(--ease-out), border-color var(--duration-fast) var(--ease-out);
}

.workflow-toggle__chip:hover:not(:disabled) {
  background: var(--color-bg-elevated);
  border-color: var(--color-bg-border);
  color: var(--color-text-primary);
}

/* Active state — accent hue matches the project's "this
   feature is engaged" convention (same color family used
   by the active ModeSelect edit state). We deliberately
   do NOT use the destructive red (tool-error) — workflow
   ON is a non-privileged augmentation, not a danger
   state. */
.workflow-toggle__chip--on {
  color: var(--color-accent);
  border-color: var(--color-accent);
}

.workflow-toggle__chip--on:hover:not(:disabled) {
  /* Keep the accent border when ON even on hover — losing
     the border on hover would make the active state look
     unstable ("did my toggle actually stick?"). */
  background: var(--color-bg-elevated);
  color: var(--color-accent);
}

.workflow-toggle__label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.workflow-toggle__icon {
  flex-shrink: 0;
}

/* Status dot — hollow when OFF, solid accent when ON. The
   dot is the visual cue that one of two things changed
   under the hood (the `workflow_enabled` DB flag), which
   ModeSelect's editable text doesn't need. */
.workflow-toggle__dot {
  display: inline-block;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  border: 1px solid var(--color-text-secondary);
  flex-shrink: 0;
  transition: background var(--duration-fast) var(--ease-out), border-color var(--duration-fast) var(--ease-out);
}

.workflow-toggle__dot--on {
  background: var(--color-accent);
  border-color: var(--color-accent);
}
</style>
