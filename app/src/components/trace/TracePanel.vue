<script setup lang="ts">
// TracePanel — the right-side trace-viewer drawer.
//
// E2 (harness trace pipeline, 2026-07-14): renders the
// per-turn trace timeline for a session, both **live**
// (current session's in-flight trace events) and **回看**
// (historical session, loaded from `turn_trace` + audit
// rows).
//
// Composed of:
//   1. Header: session title, close button, "清理" button
//      (ConfirmDialog gated — destructive action).
//   2. Body: `<TurnTimeline>` (sequence of `<TurnCard>`s).
//
// Lifecycle:
//   - The panel is mounted in AppShell (F5) as a v-if
//     sibling of the main slot, only when
//     `useTraceStore().panelOpen` is `true`. The slide-in
//     / slide-out transition uses Vue's `<Transition>`.
//   - On mount, the panel calls
//     `useTraceStore().loadHistory(currentSessionId)` to
//     pull the trace + audit rows. The body renders the
//     loading skeleton until the load resolves.
//   - On close, the panel sets `panelOpen = false`. The
//     in-memory `currentSessionTraces` is NOT cleared —
//     the timeline re-renders instantly when the user
//     re-opens the drawer (no re-fetch needed).
//   - On session switch, the panel re-fetches via the
//     `watch(currentSessionId)` hook.
//
// Failure modes: the panel's body is the timeline; the
// timeline handles the loading / error / empty states. The
// panel's own error surface is just the close + cleanup
// buttons (always interactive).
//
// Wire shape compliance: per design §4 / §5, the panel
// uses **no reka-ui Drawer primitive** (reka-ui 2.9.9
// ships no `Sheet` — see reka-ui-usage.md). The slide-in
// is hand-rolled with `<Transition name="trace-panel">` +
// CSS transforms, mirroring the SubagentDrawer's pattern.

import { computed, onMounted, ref, watch } from "vue";
import Icon from "../Icon.vue";
import ConfirmDialog from "../common/ConfirmDialog.vue";
import { useTraceStore } from "../../stores/traceStore";
import { useChatStore } from "../../stores/chat";
import { useProjectsStore } from "../../stores/projects";
import TurnTimeline from "./TurnTimeline.vue";

const traceStore = useTraceStore();
const chatStore = useChatStore();
const projectsStore = useProjectsStore();

/** ConfirmDialog visibility. Toggled by the header "清理"
 *  button; the actual `clear_session_trace` IPC fires
 *  on `confirm`. */
const showClearConfirm = ref<boolean>(false);

/** The session this panel is bound to. Bound to
 *  `chatStore.currentSessionId` at the time of opening;
 *  when the user switches session while the panel is open,
 *  the watcher below re-fetches the trace history. */
const boundSessionId = computed<string | null>(
  () => chatStore.currentSessionId,
);

/** Display title for the header. Mirrors the AuditLogModal
 *  pattern: snapshot the session title at panel-open (the
 *  panel is short-lived, a rename mid-open is acceptable to
 *  show stale). Falls back to "当前会话" / "新对话" / "新会话"
 *  for the empty / no-active-session / no-title cases. */
const sessionTitle = computed<string>(() => {
  const sid = boundSessionId.value;
  if (!sid) return "当前会话";
  const s = chatStore.sessions.find((x) => x.id === sid);
  return s?.title?.trim() || "新对话";
});

/** True when the cleanup button should be enabled. Disabled
 *  during loading or when there's no active session (no rows
 *  to clear anyway). */
const cleanupEnabled = computed<boolean>(
  () => !!boundSessionId.value && !traceStore.loading,
);

/** Close handler: just flips the panelOpen gate. The
 *  in-memory state is preserved for the next open. */
function onClose(): void {
  traceStore.setPanelOpen(false);
}

/** Cleanup click handler: opens the ConfirmDialog. The
 *  actual `clear_session_trace` IPC fires on confirm. */
function onClearClick(): void {
  showClearConfirm.value = true;
}

/** Cleanup confirm: invokes the IPC, refreshes the local
 *  state via `loadHistory`, and shows a success toast. The
 *  `clearSessionTrace` action already throws on IPC failure
 *  (the error is set on the store); the catch path
 *  surfaces a toast. */
async function onClearConfirm(): Promise<void> {
  const sid = boundSessionId.value;
  if (!sid) {
    showClearConfirm.value = false;
    return;
  }
  try {
    await traceStore.clearSessionTrace(sid);
    projectsStore.showToast("trace 数据已清理", "info", 3000);
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    projectsStore.showToast(`清理失败: ${msg}`, "error", 5000);
  } finally {
    showClearConfirm.value = false;
  }
}

/** Initial load: when the panel mounts, fetch the trace
 *  history for the current session. */
onMounted(() => {
  if (boundSessionId.value) {
    void traceStore.loadHistory(boundSessionId.value);
  }
});

/** Re-fetch on session switch (regardless of panel state). Without
 *  this, a session switch while the panel is closed would leave the
 *  store's `currentSessionTraces` Map holding the previous session's
 *  rows; the next time the user opens the panel, the timeline would
 *  show stale data. `loadHistory` is idempotent + cheap (in-memory
 *  upsert), so unconditionally reloading on session switch is safe. */
watch(boundSessionId, (newSid) => {
  if (newSid) {
    void traceStore.loadHistory(newSid);
  }
});

/** E2 trace (2026-07-14) check fix: also reload when the user opens
 *  the panel. Covers two edge cases:
 *  1. App starts with no session → `onMounted` skips → user picks a
 *     session → user opens the panel for the first time → without
 *     this watch, the Map is empty until the next `startRequest`.
 *  2. Panel was open, user closes it, session switches, user reopens
 *     → the `boundSessionId` watcher above already covers the
 *     re-load, but this watch keeps the contract simple ("open the
 *     panel → see fresh data") even if the session hasn't changed
 *     but the store was cleared for some other reason. */
watch(
  () => traceStore.panelOpen,
  (open) => {
    if (open && boundSessionId.value) {
      void traceStore.loadHistory(boundSessionId.value);
    }
  }
);
</script>

<template>
  <Transition name="trace-panel">
    <aside v-if="traceStore.panelOpen" class="trace-panel">
      <header class="trace-panel__header">
        <div class="trace-panel__title-row">
          <h2 class="trace-panel__title">
            <Icon name="chart" :size="14" class="trace-panel__title-icon" />
            Trace 时间线
            <!-- Session chip inside the same heading (mirrors the
                 AuditLogModal's 2026-09-05 title treatment): the
                 fixed label + the bound session as a secondary
                 pill, no "—" cram in between. -->
            <span class="trace-panel__session">{{
              sessionTitle
            }}</span>
          </h2>
        </div>
        <div class="trace-panel__actions">
          <button
            type="button"
            class="trace-panel__btn trace-panel__btn--clear btn btn--muted btn--sm"
            :disabled="!cleanupEnabled"
            :title="cleanupEnabled ? '清理本 session 的 trace 数据' : '请先选择 session'"
            @click="onClearClick"
          >
            <Icon name="trash" :size="12" />
            <span>清理</span>
          </button>
          <button
            type="button"
            class="trace-panel__close btn btn--icon btn--ghost"
            aria-label="关闭"
            @click="onClose"
          >
            <Icon name="x" :size="14" />
          </button>
        </div>
      </header>

      <div class="trace-panel__body">
        <TurnTimeline />
      </div>

      <ConfirmDialog
        :open="showClearConfirm"
        title="清理 trace 数据"
        variant="danger"
        confirm-text="清理"
        @cancel="showClearConfirm = false"
        @confirm="onClearConfirm"
      >
        <p>
          将删除本 session 的全部 <strong>turn_trace</strong> 行（包括压缩 / 循环 / 流程 / token 记录）。
        </p>
        <p>审计事件（<code>session_audit_events</code>）不会受影响，可继续在审计日志中查看。</p>
        <p>此操作不可恢复。</p>
      </ConfirmDialog>
    </aside>
  </Transition>
</template>

<style scoped>
.trace-panel {
  position: fixed;
  top: 0;
  right: 0;
  bottom: 0;
  width: min(560px, 92vw);
  background: var(--color-bg-surface);
  border-left: 1px solid var(--color-bg-border);
  box-shadow: var(--shadow-md);
  display: flex;
  flex-direction: column;
  z-index: var(--z-raised);
}

/* Scroll region. `.trace-panel` is a fixed-height (viewport-tall)
   flex column: header is `flex-shrink: 0`, this body absorbs the
   rest. Without `min-height: 0` + `overflow-y: auto` here, a long
   timeline overflows the viewport and can't be scrolled (the
   TurnTimeline root has no overflow of its own). Mirrors
   `.audit-modal__body` in AuditLogModal.vue. */
.trace-panel__body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
}

.trace-panel__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 14px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  flex-shrink: 0;
  gap: 12px;
}

.trace-panel__title-row {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  flex: 1;
}

.trace-panel__title {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: var(--text-md);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
  margin: 0;
  white-space: nowrap;
  min-width: 0;
}

.trace-panel__title-icon {
  color: var(--color-accent-text);
}

/* Session chip (2026-09-05): secondary identity inside the title,
   mirrors .audit-modal__title-session. Ellipsizes long session
   names instead of pushing the header actions out. */
.trace-panel__session {
  font-size: var(--text-xs);
  font-weight: var(--weight-regular);
  color: var(--color-text-secondary);
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-pill);
  padding: 2px 10px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
}

.trace-panel__actions {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
}

/* 按钮由全局 .btn 家族承载(close = ghost icon / clear = muted sm);
 * clear 是删除语义,muted 家族 hover 上叠红字红边覆写。 */
.trace-panel__btn--clear:not(:disabled):hover {
  background: var(--color-bg-border);
  color: var(--color-tool-error-text);
  border-color: color-mix(in srgb, var(--color-tool-error) 35%, transparent);
}

/* Slide-in / slide-out transition (mirrors SubagentDrawer). */
.trace-panel-enter-active,
.trace-panel-leave-active {
  transition: transform var(--duration-slow) var(--ease-decelerate),
    opacity var(--duration-slow) var(--ease-decelerate);
}

.trace-panel-enter-from,
.trace-panel-leave-to {
  transform: translateX(100%);
  opacity: 0.4;
}
</style>
