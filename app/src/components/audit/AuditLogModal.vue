<script setup lang="ts">
// AuditLogModal — reka-ui Dialog wrapper for the C4 audit-log
// query UI (PR2, 2026-06-14).
//
// Composed of:
//   1. A `v-model:open` reka-ui `Dialog*` shell (composition
//      mirrors `MemoryModal.vue` and `SettingsModal.vue`).
//   2. A header row: title + close X.
//   3. A filter row: kind dropdown (reka-ui `Select*` matching the
//      Settings forms, per reka-ui-usage.md "Convention: Wrap
//      reka-ui primitives in project-scoped CSS classes") + a "仅
//      critical" checkbox (reka-ui `CheckboxRoot`/`CheckboxIndicator`,
//      mirroring `ModelForm.vue`) + a count chip + a manual refresh
//      button.
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

import { computed, useId, watch } from "vue";
import {
  DialogRoot,
  DialogPortal,
  DialogOverlay,
  DialogContent,
  DialogTitle,
  DialogClose,
  SelectRoot,
  SelectTrigger,
  SelectValue,
  SelectIcon,
  SelectPortal,
  SelectContent,
  SelectViewport,
  SelectItem,
  SelectItemText,
  CheckboxRoot,
  CheckboxIndicator,
} from "reka-ui";

import Icon from "../Icon.vue";
import AuditLogItem from "./AuditLogItem.vue";
import { useAuditStore } from "../../stores/audit";
import { useChatStore } from "../../stores/chat";
import { AUDIT_KIND_OPTIONS } from "../../utils/audit";

const open = defineModel<boolean>("open", { required: true });

const store = useAuditStore();
const chatStore = useChatStore();

/** Stable unique id for the "仅 critical" checkbox. Used to link
 *  the `<label :for>` to the reka-ui `CheckboxRoot` (which renders
 *  as `<button role="checkbox">` — `<label>` cannot contain a
 *  button per HTML spec, so the association must go through
 *  `for`/`id` instead). `useId()` is Vue 3.5's SSR-safe unique id
 *  generator; we generate it once at setup and reuse it for the
 *  component's lifetime. */
const onlyCriticalId = useId();

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
 *  `kindFilter: string | null`; the reka-ui `SelectRoot` only
 *  deals in strings, so we map `null` ↔ `"__all__"` on the wire
 *  (the sentinel is also what the `<SelectItem :value="...">`
 *  emits for the "全部" option). */
const kindSelectValue = computed<string>({
  get: () => store.kindFilter ?? "__all__",
  set: (v: string) => {
    store.setKindFilter(v === "__all__" ? null : v);
  },
});

/** The critical-only checkbox's two-way model. Routed through
 *  the store action (not directly mutating `store.onlyCritical`)
 *  so the toggle semantics stay centralized — symmetric with
 *  `kindSelectValue`'s setter dispatching `setKindFilter`. The
 *  boolean shape pairs with reka-ui `CheckboxRoot`'s default
 *  `v-model` (which binds `modelValue`/`update:modelValue` in
 *  reka-ui 2.9.9 — `v-model:checked` is NOT a valid binding in
 *  this version; the original C4 follow-up PR used `v-model:checked`
 *  by mistake, which silently dropped the toggle event and the
 *  checkbox could not be checked. See ModelForm.vue for the
 *  canonical `v-model` usage on the same primitive.) */
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
          <div class="audit-modal__filter">
            <span class="audit-modal__filter-label">类别</span>
            <SelectRoot v-model="kindSelectValue">
              <SelectTrigger class="audit-modal__select-trigger" aria-label="类别">
                <!-- The placeholder is a NEUTRAL hint distinct from the
                     "全部" item label. SelectValue falls back to the
                     placeholder ONLY when no SelectItem matches the
                     current modelValue; since the store's initial
                     kindFilter === null maps to v-model="__all__",
                     which matches the first SelectItem (value
                     "__all__", label "全部"), the trigger renders the
                     SELECTED item text — not the placeholder — on
                     open. This keeps "全部" a real selection (normal
                     text color, no data-placeholder attribute) instead
                     of a placeholder-style ghost. Keeping the
                     placeholder as a separate cue ("选择类别") makes
                     the selected-vs-placeholder state visually
                     auditable. See reka-ui 2.9.9 SelectValue.js:
                     slotText = selectedLabel.length ? join : placeholder. -->
                <SelectValue placeholder="选择类别" />
                <SelectIcon class="audit-modal__select-icon">
                  <Icon name="chevron-down" :size="12" />
                </SelectIcon>
              </SelectTrigger>
              <SelectPortal>
                <SelectContent
                  class="audit-modal__select-content"
                  position="popper"
                  :side-offset="4"
                >
                  <SelectViewport class="audit-modal__select-viewport">
                    <SelectItem
                      v-for="opt in AUDIT_KIND_OPTIONS"
                      :key="opt.value ?? '__all__'"
                      :value="opt.value ?? '__all__'"
                      class="audit-modal__select-option"
                    >
                      <SelectItemText>{{ opt.label }}</SelectItemText>
                    </SelectItem>
                  </SelectViewport>
                </SelectContent>
              </SelectPortal>
            </SelectRoot>
          </div>

          <!-- reka-ui CheckboxRoot renders as `<button role="checkbox">`,
               so a wrapping `<label>` is illegal HTML (label cannot contain
               interactive elements) and causes a double-toggle on label
               text click (browser default click forwarding + reka-ui's
               own click handler). The outer element is therefore a `<div>`;
               the caption lives in a sibling `<label :for>` that targets
               the CheckboxRoot's stable `:id` (`onlyCriticalId`), so the
               text↔control association still works through the standard
               `for`/`id` mechanism. Mirrors `.model-form__field--check`
               in ModelForm.vue (which also drops the label wrapper). -->
          <div class="audit-modal__check">
            <CheckboxRoot
              v-model="onlyCriticalModel"
              class="audit-modal__checkbox"
              aria-label="仅 critical"
              :id="onlyCriticalId"
            >
              <CheckboxIndicator class="audit-modal__checkbox-indicator">
                <Icon name="check" :size="11" />
              </CheckboxIndicator>
            </CheckboxRoot>
            <label
              class="audit-modal__check-label"
              :for="onlyCriticalId"
            >仅 critical ({{ store.criticalCount }})</label>
          </div>

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
  background: color-mix(in srgb, var(--color-bg-app) 70%, transparent);
  backdrop-filter: blur(4px);
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
  /* min-height keeps the modal from looking thin/top-heavy when a
     session has only 1-2 events (or the empty state). 440px pairs
     with min-width: 640px to a ~3:4 aspect that feels substantial
     without crowding the 80vh max-height ceiling. The flex column
     layout means the extra height is absorbed by
     `.audit-modal__body` (flex: 1, min-height: 0) — header +
     filters stay pinned to the top, the list region grows. This
     matches MemoryModal's approach (it relies on content height +
     max-height: 80vh + min-height: 0 on the scroll region; here
     we additionally pin a floor so an empty session doesn't
     collapse to ~120px tall). No separate footer element exists
     (the 刷新 button lives inside `.audit-modal__filters`), so
     there is no fixed-bottom layer to displace. */
  min-height: 440px;
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

/* reka-ui Select trigger — rendered inside this component's own
   template, so the rule stays scoped (no `:deep()` needed). The
   visual contract mirrors `.providers-tab__trigger` /
   `.models-tab__trigger` from the Settings forms (see
   reka-ui-usage.md "Convention: Wrap reka-ui primitives in
   project-scoped CSS classes"). Smaller padding here because the
   filter row is denser than a Settings form field. */
.audit-modal__select-trigger {
  display: inline-flex;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
  padding: 4px 8px;
  /* min-width stabilizes the trigger's rendered width across
     short ("全部" = 2 CJK chars) and long ("tool_permission_ask"
     dropdown labels don't land here, but the widest selected
     label is "Yolo 静默拒绝" = 7 CJK chars) option values. Without
     it, the trigger width jitters as the user picks different
     options. 140px comfortably fits the widest Chinese label +
     the chevron icon with a small cushion; mirrors the per-trigger
     width strategy that `ProvidersTab.vue`'s `.providers-tab__trigger`
     applies via `width: 100%` in a form context (here we want a
     compact inline control in a dense filter row, so min-width
     instead of full width). */
  min-width: 140px;
  box-sizing: border-box;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 4px;
  color: var(--color-text-primary);
  font-size: 12px;
  font-family: inherit;
  cursor: pointer;
  outline: none;
  transition: border-color 0.15s, box-shadow 0.15s;
}

.audit-modal__select-trigger:hover {
  border-color: var(--color-accent-muted);
}

.audit-modal__select-trigger[data-state="open"] {
  border-color: var(--color-accent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--color-accent) 20%, transparent);
}

.audit-modal__select-icon {
  color: var(--color-text-muted);
  display: inline-flex;
  align-items: center;
}

/* reka-ui SelectContent / Viewport / Item are rendered to <body>
   via <SelectPortal>, so they escape this component's scoped-CSS
   boundary. Wrap each rule in `:deep(...)` per
   `.trellis/spec/frontend/reka-ui-usage.md` "Gotcha: `<style
   scoped>` does NOT apply to portal children". Width strategy
   uses `--reka-select-trigger-width` (the `--reka-` prefix, NOT
   `--radix-`) to size the dropdown to its trigger. */
:deep(.audit-modal__select-content) {
  position: fixed;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
  min-width: var(--reka-select-trigger-width, 200px);
  width: var(--reka-select-trigger-width);
  max-height: var(--reka-select-content-available-height);
  z-index: 3000 !important;
  overflow: hidden;
}

:deep(.audit-modal__select-viewport) {
  padding: 4px;
}

:deep(.audit-modal__select-option) {
  display: flex;
  align-items: center;
  padding: 6px 10px;
  font-size: 12px;
  color: var(--color-text-primary);
  border-radius: 4px;
  cursor: pointer;
  user-select: none;
  outline: none;
}

:deep(.audit-modal__select-option[data-highlighted]) {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
}

:deep(.audit-modal__select-option[data-state="checked"]) {
  color: var(--color-accent);
}

/* Container for the "仅 critical" checkbox + caption. Was a `<label>`
   in the first C4 follow-up draft, but reka-ui CheckboxRoot renders
   as `<button role="checkbox">` — a `<label>` cannot contain a
   button per HTML spec, and the browser's default label→control click
   forwarding stacked on top of reka-ui's own click handler, producing
   a double-toggle. Now a plain `<div>`; the caption lives in a
   sibling `<label :for>` so the click association still works
   through the standard `for`/`id` mechanism. */
.audit-modal__check {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 11px;
  color: var(--color-text-secondary);
  user-select: none;
}

.audit-modal__check-label {
  cursor: pointer;
}

/* reka-ui CheckboxRoot — does NOT portal, so scoped rule applies.
   Visual contract mirrors `.model-form__checkbox` in ModelForm.vue
   (16px square, `--color-bg-app` bg, accent on
   `[data-state="checked"]`). */
.audit-modal__checkbox {
  width: 16px;
  height: 16px;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: 3px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  transition:
    border-color 0.15s,
    background 0.15s;
}

.audit-modal__checkbox:hover {
  border-color: var(--color-accent-muted);
}

.audit-modal__checkbox[data-state="checked"] {
  background: var(--color-accent);
  border-color: var(--color-accent);
}

.audit-modal__checkbox-indicator {
  /* The white check mark sits on `--color-accent` (the checked
     state bg). Hardcoded `#fff` mirrors `.model-form__checkbox-indicator`
     in ModelForm.vue — there is no `--color-on-accent` token yet, and
     the project's convention is to inline the constant white when the
     contrasting-on-accent use case appears (see design-tokens.md
     "Don't: Hardcode color" — exception: indicators on accent bg). */
  color: #fff;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  line-height: 0;
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
