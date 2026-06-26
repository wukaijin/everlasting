<script setup lang="ts">
// ModelSelect — model picker in the chat-input hint row. Mirrors
// the worktree dropdown's hand-written popover pattern (see
// `app/src/components/chat/ChatPanel.vue:127-149`); the only
// difference is the popover opens UPWARD (`bottom: calc(100% +
// 4px)`) because the trigger is anchored to the bottom of the
// chat panel — opening down would clip the menu under the next
// sibling / the input's own focus ring.
//
// State sources (no props — pure store reads):
// - `useConfigStore().loaded`         → don't render until catalog is loaded
// - `useModelsStore().modelsGroupedByProvider` → grouped model list (with provider display name)
// - `useModelsStore().defaultModelId` → global default model id
// - `useChatStore().currentSessionId` → which session is active
// - `useChatStore().isCurrentSessionStreaming` → disable + tooltip
//
// Per the PR5 spec, the result of selecting a model fires
// `update_session_model_id` IPC to persist the per-session override
// — the dropdown UX is the worktree-style popover.

import { computed, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

import { useConfigStore } from "../../stores/config";
import { useModelsStore } from "../../stores/models";
import { useChatStore } from "../../stores/chat";
import Icon from "../Icon.vue";

const config = useConfigStore();
const modelsStore = useModelsStore();
const chatStore = useChatStore();

const menuOpen = ref(false);
const menuRoot = ref<HTMLElement | null>(null);

/** True when the user has any models configured at all. When
 *  false we render the "(未选择模型)" gray placeholder and clicking
 *  it is a no-op (the user has to open Settings, which lives in
 *  the Sidebar footer — not on this component). */
const hasModels = computed<boolean>(() => modelsStore.models.length > 0);

/** Per-session model override, falling back to the global
 *  default. */
const currentModelId = computed<string | null>(() => {
  const sid = chatStore.currentSessionId;
  if (sid) {
    const s = chatStore.sessions.find((x) => x.id === sid);
    if (s?.model_id) return s.model_id;
  }
  return modelsStore.defaultModelId ?? null;
});

/** Display label for the current model, or the gray placeholder
 *  string when no model is set. The lookup walks the full list
 *  once per id change — the catalog is small (typical user has
 *  < 20 models). */
const currentModelLabel = computed<string>(() => {
  if (!hasModels.value) return "(未选择模型)";
  const id = currentModelId.value;
  if (!id) return "(未选择模型)";
  const m = modelsStore.models.find((x) => x.id === id);
  return m?.displayName ?? "(未选择模型)";
});

/** Streaming disables the trigger so the user can't switch
 *  mid-request. The tooltip mirrors the worktree dropdown's
 *  "can't detach while streaming" rationale. */
const isStreaming = computed<boolean>(
  () => chatStore.isCurrentSessionStreaming,
);

const isPlaceholder = computed<boolean>(
  () => !currentModelId.value || !hasModels.value,
);

function toggleMenu() {
  if (isStreaming.value) return;
  menuOpen.value = !menuOpen.value;
}

function closeMenu() {
  menuOpen.value = false;
}

/** Click outside the popover root closes it. Mirrors the
 *  worktree dropdown's `onDocumentClick` pattern. */
function onDocumentClick(e: MouseEvent) {
  if (!menuOpen.value) return;
  const target = e.target as Node | null;
  if (menuRoot.value && target && !menuRoot.value.contains(target)) {
    menuOpen.value = false;
  }
}

/** Esc closes the popover. Bound on `window` because the trigger
 *  may not have focus when the popover is open. */
function onKeyDown(e: KeyboardEvent) {
  if (e.key === "Escape" && menuOpen.value) {
    menuOpen.value = false;
  }
}

if (typeof document !== "undefined") {
  document.addEventListener("click", onDocumentClick);
}
if (typeof window !== "undefined") {
  window.addEventListener("keydown", onKeyDown);
}
onUnmounted(() => {
  if (typeof document !== "undefined") {
    document.removeEventListener("click", onDocumentClick);
  }
  if (typeof window !== "undefined") {
    window.removeEventListener("keydown", onKeyDown);
  }
});

/** Click a model in the popover: persist the per-session override
 *  via IPC, update the local session summary, close. */
async function onModelPick(modelId: string) {
  const sid = chatStore.currentSessionId;
  if (!sid) return;
  closeMenu();
  try {
    await invoke("update_session_model_id", {
      sessionId: sid,
      modelId,
    });
    // Optimistic local update so the trigger label flips
    // immediately (the next `load` will re-fetch and the row
    // matches anyway).
    const summary = chatStore.sessions.find((s) => s.id === sid);
    if (summary) {
      (summary as { model_id: string | null }).model_id = modelId;
    }
  } catch (e) {
    console.error("Failed to update session model:", e);
  }
}
</script>

<template>
  <div
    v-if="config.loaded"
    ref="menuRoot"
    class="model-select"
  >
    <button
      type="button"
      class="model-select__trigger"
      :class="{
        'model-select__trigger--placeholder': isPlaceholder,
        'model-select__trigger--disabled': isStreaming,
      }"
      :disabled="isStreaming"
      :aria-haspopup="'menu'"
      :aria-expanded="menuOpen"
      :title="
        isStreaming
          ? 'Streaming 中，无法切换模型'
          : (isPlaceholder ? '请到 Sidebar 设置里添加模型' : '切换模型')
      "
      @click="toggleMenu"
    >
      <span class="model-select__label">{{ currentModelLabel }}</span>
      <Icon
        :name="menuOpen ? 'chevron-down' : 'chevron-up'"
        :size="10"
        class="model-select__chevron"
      />
    </button>
    <Transition name="model-select-popover">
      <div
        v-if="menuOpen"
        class="model-select__menu"
        role="menu"
      >
        <div
          v-for="group in modelsStore.modelsGroupedByProvider"
          :key="group.provider.id"
          class="model-select__group"
        >
          <div class="model-select__group-header">
            <Icon name="server" :size="12" />
            {{ group.provider.displayName }}
          </div>
          <button
            v-for="m in group.models"
            :key="m.id"
            type="button"
            class="model-select__item"
            :class="{
              'model-select__item--active': m.id === currentModelId,
            }"
            role="menuitem"
            @click="onModelPick(m.id)"
          >
            <span class="model-select__item-name">{{ m.displayName }}</span>
            <span
              v-if="m.id === currentModelId"
              class="model-select__item-check"
              aria-hidden="true"
            >●</span>
          </button>
        </div>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
/* PR5: hand-written popover matching `ChatPanel.vue`'s worktree
   dropdown. The popover opens UPWARD (the trigger sits at the
   bottom of the chat input; opening down would clip under the
   next sibling). The worktree dropdown uses `top: calc(100% +
   4px)`; this one uses `bottom: calc(100% + 4px); top: auto;`
   — same shape, opposite direction. */
.model-select {
  position: relative;
  display: inline-flex;
}

.model-select__trigger {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 6px;
  background: transparent;
  border: 1px solid transparent;
  border-radius: var(--radius-sm);
  color: var(--color-text-secondary);
  cursor: pointer;
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  font-weight: var(--weight-medium);
  max-width: 220px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font: inherit;
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  font-weight: var(--weight-medium);
  transition: background var(--duration-fast) var(--ease-out), color var(--duration-fast) var(--ease-out), border-color var(--duration-fast) var(--ease-out);
}

.model-select__trigger:hover:not(:disabled) {
  background: var(--color-bg-elevated);
  border-color: var(--color-bg-border);
  color: var(--color-text-primary);
}

.model-select__trigger--placeholder {
  color: var(--color-text-muted);
  font-style: italic;
  font-weight: 400;
}

.model-select__trigger--placeholder:hover:not(:disabled) {
  color: var(--color-text-secondary);
}

.model-select__trigger--disabled,
.model-select__trigger:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.model-select__label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.model-select__chevron {
  flex-shrink: 0;
  opacity: 0.6;
}

.model-select__menu {
  position: absolute;
  bottom: calc(100% + 4px);
  top: auto;
  right: 0;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-md);
  min-width: 220px;
  max-height: 320px;
  overflow-y: auto;
  z-index: 100;
  padding: 4px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.model-select__group {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.model-select__group-header {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 4px 8px 2px;
  font-size: var(--text-2xs);
  font-weight: var(--weight-semibold);
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--color-text-muted);
}

.model-select__item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 6px 8px;
  background: transparent;
  border: 0;
  color: var(--color-text-primary);
  font: inherit;
  font-family: var(--font-sans);
  font-size: var(--text-sm);
  text-align: left;
  cursor: pointer;
  border-radius: var(--radius-sm);
}

.model-select__item:hover:not(:disabled) {
  background: var(--color-bg-elevated);
}

.model-select__item--active {
  color: var(--color-accent);
  font-weight: var(--weight-medium);
}

.model-select__item-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.model-select__item-check {
  color: var(--color-accent);
  font-size: var(--text-2xs);
  flex-shrink: 0;
}

/* R4 popup animation: model-select opens UPWARD, so the slide
   origin is slightly below the final position (translateY(4px))
   and rises into place. Exit reverses the same distance. */
.model-select-popover-enter-active,
.model-select-popover-leave-active {
  transition: opacity var(--duration-base) var(--ease-out), transform var(--duration-base) var(--ease-out);
  transform-origin: bottom right;
}

.model-select-popover-enter-from,
.model-select-popover-leave-to {
  opacity: 0;
  transform: translateY(4px);
}

.model-select-popover-leave-active {
  transition-duration: 100ms;
  transition-timing-function: ease-in;
}
</style>
