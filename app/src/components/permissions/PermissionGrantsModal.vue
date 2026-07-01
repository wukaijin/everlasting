<script setup lang="ts">
// PermissionGrantsModal — reka-ui Dialog wrapper for the
// permission-grant management UI (task 07-01-permission-grant-list-ui).
//
// Lists the current session's "always allow" grants (tool / path /
// prefix kinds) and lets the user revoke any single row by PK.
// Composition mirrors AuditLogModal.vue (same Dialog shell, same
// load-on-open watcher, same empty/loading/error body states) —
// only the filter row is dropped (grant lists are short; no kind
// filter needed for MVP).
//
// MVP scope: no filter, no live push. A manual 刷新 button covers
// the "modal open while the agent loop adds a new grant" edge case.
//
// Design D1 (immediate effect): revoking a row deletes it from the
// DB; the check path re-reads the DB on the next tool_use, so the
// revoke takes effect with no cache signal. The UI removes the row
// locally on success.

import { computed, watch } from "vue";
import {
  DialogRoot,
  DialogPortal,
  DialogOverlay,
  DialogContent,
  DialogTitle,
  DialogClose,
} from "reka-ui";

import Icon from "../Icon.vue";
import PermissionGrantItem from "./PermissionGrantItem.vue";
import { usePermissionGrantsStore, type PermissionGrantRow } from "../../stores/permissionGrants";
import { useChatStore } from "../../stores/chat";

const open = defineModel<boolean>("open", { required: true });

const store = usePermissionGrantsStore();
const chatStore = useChatStore();

/** The session this modal is bound to — the CURRENT session at
 *  open time (the entry button is `v-if`'d on `currentSessionId`).
 *  The ChatPanel watcher closes the modal on session switch, so
 *  this stays stable for one open. */
const boundSessionId = computed<string | null>(() => chatStore.currentSessionId);

const sessionTitle = computed<string>(() => {
  const sid = boundSessionId.value;
  if (!sid) return "当前会话";
  const s = chatStore.sessions.find((x) => x.id === sid);
  return s?.title?.trim() || "新对话";
});

const modalTitle = computed<string>(() => `放行管理 — ${sessionTitle.value}`);

/** Reactively re-load whenever the modal transitions to open. */
watch(
  () => open.value,
  (isOpen) => {
    if (!isOpen) return;
    const sid = boundSessionId.value;
    if (!sid) return;
    void store.loadForSession(sid);
  },
);

async function onRefresh(): Promise<void> {
  await store.refresh();
}

function onRevoke(row: PermissionGrantRow): void {
  void store.revoke(row);
}
</script>

<template>
  <DialogRoot v-model:open="open">
    <DialogPortal>
      <DialogOverlay class="grant-modal__overlay" />
      <DialogContent
        class="grant-modal"
        :aria-describedby="undefined"
        @pointerdown-outside="open = false"
      >
        <header class="grant-modal__header">
          <DialogTitle class="grant-modal__title">{{ modalTitle }}</DialogTitle>
          <DialogClose as-child>
            <button type="button" class="grant-modal__close" aria-label="Close">
              <Icon name="x" :size="14" />
            </button>
          </DialogClose>
        </header>

        <div class="grant-modal__toolbar">
          <span class="grant-modal__count">{{ store.grants.length }} 项放行</span>
          <button
            type="button"
            class="grant-modal__refresh"
            :disabled="store.loading"
            title="刷新"
            @click="onRefresh"
          >
            <Icon name="refresh" :size="12" />
            <span v-if="store.loading">加载中…</span>
            <span v-else>刷新</span>
          </button>
        </div>

        <div class="grant-modal__body">
          <div v-if="store.error" class="grant-modal__error">
            操作失败: {{ store.error }}
          </div>

          <div
            v-else-if="store.loading && store.grants.length === 0"
            class="grant-modal__placeholder"
          >
            正在加载放行记录…
          </div>

          <div v-else-if="store.grants.length === 0" class="grant-modal__placeholder">
            当前会话暂无「始终允许」放行
          </div>

          <ul v-else class="grant-modal__list">
            <PermissionGrantItem
              v-for="row in store.grants"
              :key="`${row.toolName}|${row.matchKind}|${row.matchValue ?? ''}`"
              :row="row"
              @revoke="onRevoke"
            />
          </ul>
        </div>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>

<style scoped>
/* reka-ui DialogPortal teleports the overlay + content to <body>.
 * Vue 3.5 keeps `data-v-*` on Teleport children (verified by
 * SettingsModal.vue / MemoryModal.vue / AuditLogModal.vue), so the
 * rules below are NOT wrapped in `:deep()`. If a future Vue upgrade
 * breaks that, wrap in `:deep(...)` per
 * `.trellis/spec/frontend/reka-ui-usage.md`. */

.grant-modal__overlay {
  position: fixed;
  inset: 0;
  background: color-mix(in srgb, var(--color-bg-app) 70%, transparent);
  backdrop-filter: blur(4px);
  z-index: 2000;
  animation: grant-modal-fade var(--duration-base) var(--ease-out);
}

.grant-modal {
  position: fixed;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  width: 80vw;
  min-width: 560px;
  max-width: min(880px, calc(100vw - 40px));
  min-height: 360px;
  max-height: 80vh;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-lg);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  box-shadow: var(--shadow-xl);
  z-index: 2001;
  outline: none;
  animation: grant-modal-zoom var(--duration-base) var(--ease-out);
}

@keyframes grant-modal-fade {
  from { opacity: 0; }
  to { opacity: 1; }
}

@keyframes grant-modal-zoom {
  from { opacity: 0; transform: translate(-50%, -50%) scale(0.96); }
  to { opacity: 1; transform: translate(-50%, -50%) scale(1); }
}

.grant-modal__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 16px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  flex-shrink: 0;
}

.grant-modal__title {
  margin: 0;
  font-size: var(--text-base);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.grant-modal__close {
  background: transparent;
  border: 0;
  color: var(--color-text-muted);
  cursor: pointer;
  padding: 4px;
  border-radius: var(--radius-sm);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}

.grant-modal__close:hover {
  background: var(--color-bg-border);
  color: var(--color-text-primary);
}

.grant-modal__toolbar {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 10px 16px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-app);
  flex-shrink: 0;
}

.grant-modal__count {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--color-text-muted);
}

.grant-modal__refresh {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: var(--text-xs);
  padding: 4px 8px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  color: var(--color-text-secondary);
  cursor: pointer;
  font-family: inherit;
  margin-left: auto;
  transition: background var(--duration-fast) var(--ease-out),
    color var(--duration-fast) var(--ease-out),
    border-color var(--duration-fast) var(--ease-out);
}

.grant-modal__refresh:hover:not(:disabled) {
  background: var(--color-accent-muted);
  border-color: var(--color-accent);
  color: var(--color-accent);
}

.grant-modal__refresh:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

.grant-modal__body {
  flex: 1;
  overflow-y: auto;
  background: var(--color-bg-app);
  min-height: 0;
}

.grant-modal__error {
  padding: 16px;
  color: var(--color-tool-error);
  font-size: var(--text-sm);
  text-align: center;
}

.grant-modal__placeholder {
  padding: 32px 16px;
  text-align: center;
  color: var(--color-text-muted);
  font-size: var(--text-sm);
}

.grant-modal__list {
  list-style: none;
  margin: 0;
  padding: 0;
}
</style>
