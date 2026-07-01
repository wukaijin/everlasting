<script setup lang="ts">
// SettingsModal — full-viewport overlay with 3 tabs (Providers, Models,
// Default). Uses reka-ui DialogRoot/DialogContent for overlay + focus trap
// and TabsRoot/TabsList/TabsTrigger/TabsContent for the tab switcher.
// Receives v-model:open from the parent (Sidebar footer button).

import { DialogRoot, DialogPortal, DialogOverlay, DialogContent, DialogTitle, DialogClose } from "reka-ui";
import { TabsRoot, TabsList, TabsTrigger, TabsContent } from "reka-ui";
import Icon from "../Icon.vue";
import ProvidersTab from "./ProvidersTab.vue";
import ModelsTab from "./ModelsTab.vue";
import DefaultTab from "./DefaultTab.vue";
import MemoryTab from "./MemoryTab.vue";

const open = defineModel<boolean>("open", { required: true });
</script>

<template>
  <DialogRoot v-model:open="open">
    <DialogPortal>
      <DialogOverlay class="settings-modal__overlay" />
      <DialogContent class="settings-modal" @pointerdown-outside="open = false">
        <header class="settings-modal__header">
          <DialogTitle class="settings-modal__title">Settings</DialogTitle>
          <DialogClose as-child>
            <button type="button" class="settings-modal__close" aria-label="Close">
              <Icon name="x" :size="14" />
            </button>
          </DialogClose>
        </header>

        <TabsRoot default-value="providers" class="settings-modal__body">
          <TabsList class="settings-modal__tabs">
            <TabsTrigger value="providers" class="settings-modal__tab">
              Providers
            </TabsTrigger>
            <TabsTrigger value="models" class="settings-modal__tab">
              Models
            </TabsTrigger>
            <TabsTrigger value="default" class="settings-modal__tab">
              Default
            </TabsTrigger>
            <TabsTrigger value="memory" class="settings-modal__tab">
              Memory
            </TabsTrigger>
          </TabsList>

          <TabsContent value="providers" class="settings-modal__content">
            <ProvidersTab />
          </TabsContent>
          <TabsContent value="models" class="settings-modal__content">
            <ModelsTab />
          </TabsContent>
          <TabsContent value="default" class="settings-modal__content">
            <DefaultTab />
          </TabsContent>
          <TabsContent value="memory" class="settings-modal__content">
            <MemoryTab />
          </TabsContent>
        </TabsRoot>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>

<style scoped>
.settings-modal__overlay {
  position: fixed;
  inset: 0;
  background: color-mix(in srgb, var(--color-bg-app) 70%, transparent);
  backdrop-filter: blur(4px);
  z-index: 2000;
}

.settings-modal {
  position: fixed;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  width: 640px;
  max-width: calc(100vw - 40px);
  max-height: 80vh;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-lg);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  box-shadow: var(--shadow-xl);
  z-index: 2001;
  /* reka-ui DialogContent sets outline on focus; suppress for our design */
  outline: none;
  animation: settings-modal-zoom var(--duration-modal-in) var(--ease-modal-in) both;
}

.settings-modal[data-state="closed"] {
  animation: settings-modal-zoom-out var(--duration-modal-out) var(--ease-accelerate) forwards;
}

@keyframes settings-modal-zoom {
  from { opacity: 0; transform: translate(-50%, -50%) scale(0.1); }
  to   { opacity: 1; transform: translate(-50%, -50%) scale(1); }
}

@keyframes settings-modal-zoom-out {
  from { opacity: 1; transform: translate(-50%, -50%) scale(1); }
  to   { opacity: 0; transform: translate(-50%, -50%) scale(0.1); }
}

.settings-modal__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 16px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  flex-shrink: 0;
}

.settings-modal__title {
  margin: 0;
  font-size: var(--text-base);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
}

.settings-modal__close {
  background: transparent;
  border: 0;
  color: var(--color-text-muted);
  cursor: pointer;
  padding: 4px;
  border-radius: var(--radius-sm);
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

.settings-modal__close:hover {
  background: var(--color-bg-border);
  color: var(--color-text-primary);
}

.settings-modal__body {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-height: 0;
}

.settings-modal__tabs {
  display: flex;
  gap: 0;
  padding: 0 16px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  flex-shrink: 0;
}

.settings-modal__tab {
  padding: 8px 16px;
  font-size: var(--text-sm);
  font-weight: var(--weight-medium);
  color: var(--color-text-muted);
  background: transparent;
  border: 0;
  border-bottom: 2px solid transparent;
  cursor: pointer;
  transition: color var(--duration-base) var(--ease-out), border-color var(--duration-base) var(--ease-out);
}

.settings-modal__tab:hover {
  color: var(--color-text-secondary);
}

.settings-modal__tab[data-state="active"] {
  color: var(--color-text-primary);
  border-bottom-color: var(--color-accent);
}

.settings-modal__content {
  flex: 1;
  overflow-y: auto;
  padding: 16px;
  background: var(--color-bg-app);
}
</style>
