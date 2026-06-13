<script setup lang="ts">
// AuditLogModal — reka-ui Dialog wrapper for the C4 audit-log
// query UI (PR2, 2026-06-14).
//
// Composed of:
//   1. A `v-model:open` reka-ui `Dialog*` shell (composition
//      mirrors `MemoryModal.vue` and `SettingsModal.vue`).
//   2. A header row: title + close X.
//   3. A filter row: kind dropdown (native `<select>` themed to
//      match the project's form-control look, per reka-ui-usage.md
//      "Gotcha: TextFieldRoot does NOT exist in 2.9.9" → native
//      `<input>` / `<select>` is the project pattern) + a "仅
//      critical" checkbox (native `<input type="checkbox">`) +
//      a count chip + a manual refresh button.
//   4. A scrollable list of `<AuditLogItem>` rows. Empty state
//      shows "暂无审计事件".
//
// Visual language follows design-tokens.md: every color references
// a `--color-*` token, no hardcoded hex. The reka-ui `DialogContent`
// is rendered through `<DialogPortal>` (teleport to body), so the
// `<style scoped>` rules for portal children use Vue 3.5's
// preserved `data-v-*` attribute (the same approach MemoryModal
// takes — see its header comment).
//
// Loading on open: `useAuditStore.loadForSession(sessionId)` is
// called from a watcher on `open`. The watcher gates on
// `open === true` AND `sessionId !== null`, so opening the modal
// with no active session is a no-op (the entry-point button is
// also `v-if`'d on `currentSessionId`, so the modal shouldn't
// open in that state in practice).
//
// MVP scope per PRD "Edge Cases": no virtual scroll / pagination,
// no live push. A manual 刷新 button is the stopgap for the
// "Modal 开着期间 agent 又写新事件" case.

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
import AuditLogItem from "./AuditLogItem.vue";
import { useAuditStore } from "../../stores/audit";
import { useChatStore } from "../../stores/chat";
import { AUDIT_KIND_OPTIONS } from "../../utils/audit";

const open = defineModel<boolean>("open", { required: true });

const store = useAuditStore();
const chatStore = useChatStore();

/** The session this modal is bound to. Bound to the CURRENT session
 *  at the moment of opening (the entry button sits in ChatPanel's
 *  header, right next to the Memory button, and is `v-if`'d on
 *  `chatStore.currentSessionId`). When the user switches session
 *  while the modal is open, the ChatPanel watcher closes the
 *  modal (see ChatPanel.vue), so `boundSessionId` stays stable
 *  for the lifetime of one open. */
const boundSessionId = computed<string | null>(
  () => chatStore.currentSessionId,
);

/** Display title for the modal header. We snapshot the session
 *  title at the time of opening — the modal is short-lived, so
 *  a rename mid-open is acceptable to show stale (the user would
 *  close + reopen to refresh). Falls back to "当前会话" when no
 *  session is active. */
const sessionTitle = computed<string>(() => {
  const sid = boundSessionId.value;
  if (!sid) return "当前会话";
  const s = chatStore.sessions.find((x) => x.id === sid);
  return s?.title?.trim() || "新对话";
});

const modalTitle = computed<string>(() => `审计日志 — ${sessionTitle.value}`);

/** Reactively re-load whenever the modal transitions to open.
 *  Skip when `boundSessionId` is null (defensive — the entry
 *  button is also gated on this, so in practice the modal can't
 *  open in that state). */
watch(
  () => open.value,
  (isOpen) => {
    if (!isOpen) return;
    const sid = boundSessionId.value;
    if (!sid) return;
    void store.loadForSession(sid);
  },
);

/** The kind dropdown's two-way model. The store keeps
 *  `kindFilter: string | null`; the native `<select>` only deals
 *  in strings, so we map `null` ↔ `"__all__"` on the wire. */
const kindSelectValue = computed<string>({
  get: () => store.kindFilter ?? "__all__",
  set: (v: string) => {
    store.setKindFilter(v === "__all__" ? null : v);
  },
});

/** The critical-only checkbox's two-way model. Routed through
 *  the store action (not directly mutating `store.onlyCritical`)
 *  so the toggle semantics stay centralized — symmetric with
 *  `kindSelectValue`'s setter dispatching `setKindFilter`. */
const onlyCriticalModel = computed<boolean>({
  get: () => store.onlyCritical,
  set: (_v: boolean) => store.toggleCritical(),
});

/** Count chip text: "X / Y 项" (filtered / total) when a filter
 *  is active, "X 项" otherwise. */
const countText = computed<string>(() => {
  const f = store.filteredCount;
  const t = store.totalCount;
  if (store.kindFilter !== null || store.onlyCritical) {
    return `${f} / ${t} 项`;
  }
  return `${t} 项`;
});

async function onRefresh(): Promise<void> {
  await store.refresh();
}
</script>

<template>
  <DialogRoot v-model:open="open">
    <DialogPortal>
      <DialogOverlay class="audit-modal__overlay" />
      <DialogContent
        class="audit-modal"
        :aria-describedby="undefined"
        @pointerdown-outside="open = false"
      >
        <header class="audit-modal__header">
          <DialogTitle class="audit-modal__title">
            {{ modalTitle }}
          </DialogTitle>
          <DialogClose as-child>
            <button
              type="button"
              class="audit-modal__close"
              aria-label="Close"
            >
              <Icon name="x" :size="14" />
            </button>
          </DialogClose>
        </header>

        <div class="audit-modal__filters">
          <label class="audit-modal__filter">
            <span class="audit-modal__filter-label">类别</span>
            <select v-model="kindSelectValue" class="audit-modal__select">
              <option
                v-for="opt in AUDIT_KIND_OPTIONS"
                :key="opt.value ?? '__all__'"
                :value="opt.value ?? '__all__'"
              >
                {{ opt.label }}
              </option>
            </select>
          </label>

          <label class="audit-modal__check">
            <input
              v-model="onlyCriticalModel"
              type="checkbox"
              class="audit-modal__checkbox"
            />
            <span>仅 critical ({{ store.criticalCount }})</span>
          </label>

          <span class="audit-modal__count">{{ countText }}</span>

          <button
            type="button"
            class="audit-modal__refresh"
            :disabled="store.loading"
            title="刷新"
            @click="onRefresh"
          >
            <Icon name="refresh" :size="12" />
            <span v-if="store.loading">加载中…</span>
            <span v-else>刷新</span>
          </button>
        </div>

        <div class="audit-modal__body">
          <div v-if="store.error" class="audit-modal__error">
            加载失败: {{ store.error }}
          </div>

          <div
            v-else-if="store.loading && store.events.length === 0"
            class="audit-modal__placeholder"
          >
            正在加载审计事件…
          </div>

          <div
            v-else-if="store.filteredEvents.length === 0"
            class="audit-modal__placeholder"
          >
            {{ store.events.length === 0 ? "暂无审计事件" : "无匹配事件" }}
          </div>

          <ul v-else class="audit-modal__list">
            <AuditLogItem
              v-for="row in store.filteredEvents"
              :key="row.id"
              :row="row"
            />
          </ul>
        </div>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>

<style scoped>
/*
 * IMPORTANT — `reka-ui` DialogPortal teleports DialogOverlay and
 * DialogContent to <body>. Vue 3.5's scoped-CSS compiler keeps
 * `data-v-*` attributes on Teleport children (verified by
 * SettingsModal.vue and MemoryModal.vue in this codebase), so
 * we mirror their non-`:deep()` style here. If a future Vue
 * upgrade breaks this assumption, wrap the .audit-modal* rules
 * in `:deep(...)` per
 * `.trellis/spec/frontend/reka-ui-usage.md`.
 */

.audit-modal__overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.6);
  z-index: 2000;
  animation: audit-modal-fade 150ms ease-out;
}

.audit-modal__overlay[data-state="closed"] {
  animation: audit-modal-fade-out 100ms ease-in forwards;
}

.audit-modal {
  position: fixed;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  width: 80vw;
  min-width: 640px;
  max-width: min(960px, calc(100vw - 40px));
  max-height: 80vh;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 8px;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
  z-index: 2001;
  outline: none;
  animation: audit-modal-zoom 150ms ease-out;
}

.audit-modal[data-state="closed"] {
  animation: audit-modal-zoom-out 100ms ease-in forwards;
}

@keyframes audit-modal-fade {
  from { opacity: 0; }
  to   { opacity: 1; }
}

@keyframes audit-modal-fade-out {
  from { opacity: 1; }
  to   { opacity: 0; }
}

@keyframes audit-modal-zoom {
  from { opacity: 0; transform: translate(-50%, -50%) scale(0.96); }
  to   { opacity: 1; transform: translate(-50%, -50%) scale(1); }
}

@keyframes audit-modal-zoom-out {
  from { opacity: 1; transform: translate(-50%, -50%) scale(1); }
  to   { opacity: 0; transform: translate(-50%, -50%) scale(0.96); }
}

.audit-modal__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 16px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  flex-shrink: 0;
}

.audit-modal__title {
  margin: 0;
  font-size: 13px;
  font-weight: 600;
  color: var(--color-text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.audit-modal__close {
  background: transparent;
  border: 0;
  color: var(--color-text-muted);
  cursor: pointer;
  padding: 4px;
  border-radius: 4px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}

.audit-modal__close:hover {
  background: var(--color-bg-border);
  color: var(--color-text-primary);
}

.audit-modal__filters {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 10px 16px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-app);
  flex-shrink: 0;
  flex-wrap: wrap;
}

.audit-modal__filter {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 11px;
  color: var(--color-text-secondary);
}

.audit-modal__filter-label {
  font-weight: 500;
}

/* Native `<select>` themed to match the project's form-control
   look (`.providers-tab__input` / `.models-tab__input` pattern
   from reka-ui-usage.md "Gotcha: TextFieldRoot does NOT exist
   in 2.9.9"). */
.audit-modal__select {
  padding: 4px 8px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 4px;
  color: var(--color-text-primary);
  font-size: 12px;
  font-family: inherit;
  outline: none;
  cursor: pointer;
  transition: border-color 0.15s, box-shadow 0.15s;
}

.audit-modal__select:focus {
  border-color: var(--color-accent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--color-accent) 20%, transparent);
}

.audit-modal__check {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 11px;
  color: var(--color-text-secondary);
  cursor: pointer;
  user-select: none;
}

.audit-modal__checkbox {
  margin: 0;
  cursor: pointer;
  accent-color: var(--color-accent);
}

.audit-modal__count {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--color-text-muted);
  margin-left: auto;
}

.audit-modal__refresh {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 11px;
  padding: 4px 8px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 4px;
  color: var(--color-text-secondary);
  cursor: pointer;
  font-family: inherit;
  transition: background 0.1s, color 0.1s, border-color 0.1s;
}

.audit-modal__refresh:hover:not(:disabled) {
  background: var(--color-accent-muted);
  border-color: var(--color-accent);
  color: var(--color-accent);
}

.audit-modal__refresh:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

.audit-modal__body {
  flex: 1;
  overflow-y: auto;
  background: var(--color-bg-app);
  min-height: 0;
}

.audit-modal__error {
  padding: 16px;
  color: var(--color-tool-error);
  font-size: 12px;
  text-align: center;
}

.audit-modal__placeholder {
  padding: 32px 16px;
  text-align: center;
  color: var(--color-text-muted);
  font-size: 12px;
}

.audit-modal__list {
  list-style: none;
  margin: 0;
  padding: 0;
}
</style>
