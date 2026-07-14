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
          </h2>
          <span class="trace-panel__subtitle">— {{ sessionTitle }}</span>
        </div>
        <div class="trace-panel__actions">
          <button
            type="button"
            class="trace-panel__btn trace-panel__btn--clear"
            :disabled="!cleanupEnabled"
            :title="cleanupEnabled ? '清理本 session 的 trace 数据' : '请先选择 session'"
            @click="onClearClick"
          >
            <Icon name="trash" :size="12" />
            <span>清理</span>
          </button>
          <button
            type="button"
            class="trace-panel__close"
            aria-label="关闭"
            @click="onClose"
          >
            <Icon name="x" :size="14" />
          </button>
        </div>
      </header>

      <TurnTimeline />

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
  width: min(420px, 90vw);
  background: var(--color-bg-surface);
  border-left: 1px solid var(--color-bg-border);
  box-shadow: var(--shadow-md);
  display: flex;
  flex-direction: column;
  z-index: 100;
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
}

.trace-panel__title-icon {
  color: var(--color-accent);
}

.trace-panel__subtitle {
  font-size: var(--text-sm);
  color: var(--color-text-muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
}

.trace-panel__actions {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
}

.trace-panel__btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  background: var(--color-bg-surface);
  color: var(--color-text-secondary);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  padding: 4px 8px;
  font-size: var(--text-xs);
  cursor: pointer;
  transition: background var(--duration-fast) var(--ease-out);
}

.trace-panel__btn:not(:disabled):hover {
  background: var(--color-bg-border);
  color: var(--color-text-primary);
}

.trace-panel__btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.trace-panel__btn--clear:not(:disabled):hover {
  color: var(--color-tool-error);
  border-color: color-mix(in srgb, var(--color-tool-error) 35%, transparent);
}

.trace-panel__close {
  background: transparent;
  border: 0;
  color: var(--color-text-muted);
  cursor: pointer;
  padding: 4px;
  border-radius: var(--radius-sm);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  transition: background var(--duration-fast) var(--ease-out),
    color var(--duration-fast) var(--ease-out);
}

.trace-panel__close:hover {
  background: var(--color-bg-border);
  color: var(--color-text-primary);
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
