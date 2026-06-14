<script setup lang="ts">
// MemoryModal — reka-ui Dialog wrapper around the project-layer
// MemoryPreview view. Replaces the hand-rolled popover that used
// to live in ProjectTabs.vue (B5 follow-up,
// `06-11-memory-modal-appheader-entry`). The popover had a
// `right: 0; min-width: 480px` overflow bug when its trigger was
// not at the viewport's right edge; a centered modal sidesteps
// the entire class of positioning issues.
//
// Composition mirrors SettingsModal (`reka-ui` DialogRoot /
// DialogPortal / DialogOverlay / DialogContent / DialogClose),
// but skips Tabs — this modal only renders the 2 project layers
// (CLAUDE.md + AGENTS.md) for the active project. User-layer
// memory continues to live in the Settings → Memory tab.
//
// Sizing: width `80vw` clamped to `min 640px / max 900px`, height
// capped at `80vh`. Inside, MemoryPreview's own list scrolls when
// content overflows (it already does this for the 50KB cap, so
// no extra scroll wrapper is needed here).

import { DialogRoot, DialogPortal, DialogOverlay, DialogContent, DialogTitle, DialogClose } from "reka-ui";

import { useProjectsStore } from "../../stores/projects";
import Icon from "../Icon.vue";
import MemoryPreview from "./MemoryPreview.vue";

const open = defineModel<boolean>("open", { required: true });

const projectsStore = useProjectsStore();
</script>

<template>
  <DialogRoot v-model:open="open">
    <DialogPortal>
      <DialogOverlay class="memory-modal__overlay" />
      <DialogContent
        class="memory-modal"
        :aria-describedby="undefined"
        @pointerdown-outside="open = false"
      >
        <header class="memory-modal__header">
          <DialogTitle class="memory-modal__title">
            项目指令文件
          </DialogTitle>
          <DialogClose as-child>
            <button type="button" class="memory-modal__close" aria-label="Close">
              <Icon name="x" :size="14" />
            </button>
          </DialogClose>
        </header>

        <div class="memory-modal__body">
          <MemoryPreview
            kind="project"
            :project-id="projectsStore.currentProjectId"
          />
        </div>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>

<style scoped>
/*
 * IMPORTANT — `reka-ui` DialogPortal teleports DialogOverlay and
 * DialogContent to <body>, so the compiled scoped-CSS selectors
 * normally would not match. SettingsModal proves Vue 3.5's scoped
 * compiler keeps `data-v-*` attributes on Teleport children, so we
 * mirror its non-`:deep()` style here. If a future Vue upgrade
 * breaks this assumption, wrap the .memory-modal* rules in
 * `:deep(...)` per `.trellis/spec/frontend/reka-ui-usage.md`.
 */

.memory-modal__overlay {
  position: fixed;
  inset: 0;
  background: color-mix(in srgb, var(--color-bg-app) 70%, transparent);
  backdrop-filter: blur(4px);
  z-index: 2000;
  animation: memory-modal-fade 150ms ease-out;
}

.memory-modal__overlay[data-state="closed"] {
  animation: memory-modal-fade-out 100ms ease-in forwards;
}

.memory-modal {
  position: fixed;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  /* Width strategy: track the viewport (80vw) but clamp to a
     readable range. 640px guarantees the MemoryPreview header chip
     doesn't wrap; 900px stops the modal from becoming a wall of
     near-empty space on a 4K display. */
  width: 80vw;
  min-width: 640px;
  max-width: min(900px, calc(100vw - 40px));
  max-height: 80vh;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 8px;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
  z-index: 2001;
  /* reka-ui DialogContent sets outline on focus; suppress for our
     design system (the inner focus indicators of MemoryPreview are
     enough). */
  outline: none;
  animation: memory-modal-zoom 150ms ease-out;
}

.memory-modal[data-state="closed"] {
  animation: memory-modal-zoom-out 100ms ease-in forwards;
}

@keyframes memory-modal-fade {
  from { opacity: 0; }
  to   { opacity: 1; }
}

@keyframes memory-modal-fade-out {
  from { opacity: 1; }
  to   { opacity: 0; }
}

@keyframes memory-modal-zoom {
  from { opacity: 0; transform: translate(-50%, -50%) scale(0.96); }
  to   { opacity: 1; transform: translate(-50%, -50%) scale(1); }
}

@keyframes memory-modal-zoom-out {
  from { opacity: 1; transform: translate(-50%, -50%) scale(1); }
  to   { opacity: 0; transform: translate(-50%, -50%) scale(0.96); }
}

.memory-modal__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 16px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  flex-shrink: 0;
}

.memory-modal__title {
  margin: 0;
  font-size: 13px;
  font-weight: 600;
  color: var(--color-text-primary);
}

.memory-modal__close {
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

.memory-modal__close:hover {
  background: var(--color-bg-border);
  color: var(--color-text-primary);
}

.memory-modal__body {
  flex: 1;
  overflow-y: auto;
  padding: 16px;
  background: var(--color-bg-app);
  min-height: 0;
}
</style>
