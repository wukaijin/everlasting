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
        </TabsRoot>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>

<style scoped>
.settings-modal__overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.6);
  z-index: 2000;
  animation: settings-modal-fade 150ms ease-out;
}

.settings-modal__overlay[data-state="closed"] {
  animation: settings-modal-fade-out 100ms ease-in forwards;
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
  border-radius: 8px;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
  z-index: 2001;
  /* reka-ui DialogContent sets outline on focus; suppress for our design */
  outline: none;
  animation: settings-modal-zoom 150ms ease-out;
}

.settings-modal[data-state="closed"] {
  animation: settings-modal-zoom-out 100ms ease-in forwards;
}

@keyframes settings-modal-fade {
  from { opacity: 0; }
  to   { opacity: 1; }
}

@keyframes settings-modal-fade-out {
  from { opacity: 1; }
  to   { opacity: 0; }
}

@keyframes settings-modal-zoom {
  from { opacity: 0; transform: translate(-50%, -50%) scale(0.96); }
  to   { opacity: 1; transform: translate(-50%, -50%) scale(1); }
}

@keyframes settings-modal-zoom-out {
  from { opacity: 1; transform: translate(-50%, -50%) scale(1); }
  to   { opacity: 0; transform: translate(-50%, -50%) scale(0.96); }
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
  font-size: 13px;
  font-weight: 600;
  color: var(--color-text-primary);
}

.settings-modal__close {
  background: transparent;
  border: 0;
  color: var(--color-text-muted);
  cursor: pointer;
  padding: 4px;
  border-radius: 4px;
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
  font-size: 12px;
  font-weight: 500;
  color: var(--color-text-muted);
  background: transparent;
  border: 0;
  border-bottom: 2px solid transparent;
  cursor: pointer;
  transition: color 0.15s, border-color 0.15s;
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
