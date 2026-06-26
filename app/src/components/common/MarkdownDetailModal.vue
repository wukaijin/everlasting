<script setup lang="ts">
// MarkdownDetailModal — generic reka-ui Dialog wrapper for viewing
// the full version of a truncated markdown blob. PR3 of the
// subagent-drawer redesign (2026-06-21).
//
// The SubagentDrawer (PR5) renders `task` (the parent LLM's prompt
// that dispatched the worker) and `finalText` (the worker's
// terminal reply) as truncated previews with a "View full →" link
// next to each. Clicking that link opens THIS modal with the
// complete markdown rendered through `renderMarkdown`.
//
// Why a generic modal (not 2 specialised components):
//   The drawer's two consumers (Prompt + Reply segments) need the
//   same behaviour — render markdown, scrollable body, header
//   with a title and a small "source" chip that hints at where the
//   text came from. Two specialised components would duplicate the
//   wrapper + styling; one generic component with a `source` prop
//   covers both.
//
// Why reka-ui DialogRoot (not the hand-rolled ConfirmDialog pattern):
//   `MemoryModal.vue` and `SettingsModal.vue` use this exact
//   pattern. The Drawer (PR5) already imports reka-ui primitives
//   elsewhere, so this stays inside the established ecosystem.
//   ConfirmDialog is hand-rolled because it's transient (no scroll,
//   no large body); MarkdownDetailModal may show 100k chars of
//   markdown, so it needs the proper focus trap + Esc + overlay
//   click semantics that reka-ui Dialog provides out of the box.
//
// Composition mirrors MemoryModal: DialogRoot / DialogPortal /
// DialogOverlay / DialogContent / DialogTitle / DialogClose. The
// `:deep()` gotcha from `reka-ui-usage.md` does NOT apply here —
// Vue 3.5's scoped-CSS compiler keeps `data-v-*` on Teleport
// children, so the regular scoped rules work (same pattern as
// MemoryModal).
//
// Sizing: width `80vw` clamped to `min 640px / max 900px`, body
// max-height `70vh` (taller than the 50KB Memory cap because
// markdown can be verbose; the user explicitly asked to see the
// whole thing).

import {
  DialogRoot,
  DialogPortal,
  DialogOverlay,
  DialogContent,
  DialogTitle,
  DialogClose,
} from "reka-ui";

import { computed } from "vue";
import { renderMarkdown } from "../../utils/markdown";
import Icon from "../Icon.vue";

/**
 * Where the markdown content came from. Drives the small chip in
 * the modal header so the user has a hint about what they're
 * reading. The drawer passes `"prompt"` for the parent's LLM
 * prompt and `"reply"` for the worker's final reply; `"worker"`
 * is reserved for future surfaces (e.g. a worker-internal log
 * viewer) and `null`/`undefined` disables the chip entirely.
 */
export type MarkdownDetailSource = "worker" | "prompt" | "reply" | null | undefined;

const props = withDefaults(
  defineProps<{
    /** Modal open state. v-model:open style — the parent binds
     *  via `v-model:open="someRef"`. */
    open: boolean;
    /** Header title text (e.g. "Worker Prompt" / "Final Reply"). */
    title: string;
    /** The full markdown string to render. The drawer's
     *  truncated preview is the affordance that triggers this
     *  modal; the modal itself always renders the COMPLETE
     *  string (no further truncation). */
    markdown: string;
    /** Optional hint for the header chip. When null/undefined,
     *  no chip is rendered. */
    source?: MarkdownDetailSource;
  }>(),
  {
    source: null,
  },
);

const emit = defineEmits<{
  /** Emitted when the user closes the modal (X / Esc / overlay
   *  click). The parent should sync its `open` ref to `false`
   *  — reka-ui's DialogRoot only updates its own state on the
   *  close event; without this emit the parent's v-model would
   *  not flip and the modal would reopen on the next render. */
  "update:open": [value: boolean];
}>();

/** Reactive close handler used by the X button + the
 *  pointerdown-outside event on the overlay. We deliberately
 *  avoid reka-ui's automatic two-way binding here because we
 *  want a single source of truth — the parent's v-model. */
function close(): void {
  emit("update:open", false);
}

/** Source-chip label + icon name per `MarkdownDetailSource`. Kept
 *  inline (not extracted to a constant) because the mapping is
 *  read once at component definition; a constant table would
 *  scatter the source-specific copy. */
const sourceChip = computed<{ label: string; icon: string } | null>(() => {
  switch (props.source) {
    case "prompt":
      return { label: "Prompt", icon: "thinking" };
    case "reply":
      return { label: "Reply", icon: "document" };
    case "worker":
      return { label: "Worker", icon: "server" };
    default:
      return null;
  }
});

/** Rendered HTML for the body. Goes through the project's
 *  `renderMarkdown` pipeline (marked + DOMPurify) — same path the
 *  main chat panel uses. A blank/whitespace-only input renders
 *  as an empty string (renderMarkdown returns "" for those), so
 *  the body element collapses cleanly. */
const bodyHtml = computed<string>(() => renderMarkdown(props.markdown));
</script>

<template>
  <DialogRoot
    :open="props.open"
    @update:open="(v: boolean) => emit('update:open', v)"
  >
    <DialogPortal>
      <DialogOverlay class="markdown-detail-modal__overlay" />
      <DialogContent
        class="markdown-detail-modal"
        :aria-describedby="undefined"
        @pointerdown-outside="close"
      >
        <header class="markdown-detail-modal__header">
          <DialogTitle class="markdown-detail-modal__title">
            <span class="markdown-detail-modal__title-text">{{ props.title }}</span>
            <span
              v-if="sourceChip"
              class="markdown-detail-modal__source-chip"
              :data-source="props.source ?? undefined"
            >
              <Icon :name="sourceChip.icon" :size="12" />
              <span>{{ sourceChip.label }}</span>
            </span>
          </DialogTitle>
          <DialogClose as-child>
            <button
              type="button"
              class="markdown-detail-modal__close"
              aria-label="Close"
              @click="close"
            >
              <Icon name="x" :size="14" />
            </button>
          </DialogClose>
        </header>

        <!--
          The body is a scroll container so a 100k-char markdown
          doesn't blow up the modal. `min-height: 0` is required on
          the flex child for `overflow-y: auto` to take effect on
          WebKit (well-known flex/overflow interaction). The inner
          markdown wrapper applies the standard message-bubble
          typography tokens via the shared `.markdown-body` class
          family.
        -->
        <div class="markdown-detail-modal__body">
          <div
            class="markdown-detail-modal__markdown"
            v-html="bodyHtml"
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
 * normally would not match. SettingsModal + MemoryModal prove Vue
 * 3.5's scoped compiler keeps `data-v-*` attributes on Teleport
 * children, so we mirror their non-`:deep()` style here. If a
 * future Vue upgrade breaks this assumption, wrap the rules in
 * `:deep(...)` per `.trellis/spec/frontend/reka-ui-usage.md`.
 */

.markdown-detail-modal__overlay {
  position: fixed;
  inset: 0;
  background: color-mix(in srgb, var(--color-bg-app) 70%, transparent);
  backdrop-filter: blur(4px);
  z-index: 2000;
  animation: markdown-detail-modal-fade var(--duration-base) var(--ease-out);
}

.markdown-detail-modal__overlay[data-state="closed"] {
  animation: markdown-detail-modal-fade-out var(--duration-fast) ease-in forwards;
}

.markdown-detail-modal {
  position: fixed;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  /* Width strategy: track the viewport (80vw) but clamp to a
     readable range. 640px matches MemoryModal — markdown in this
     modal is often longer than Memory's 50KB cap, so we widen
     the upper bound to 900px to use horizontal space more
     efficiently on wide viewports. */
  width: 80vw;
  min-width: 640px;
  max-width: min(900px, calc(100vw - 40px));
  /* Body height cap: 70vh (taller than MemoryModal's 80vh body
     cap because this modal is narrower — 80vh on a narrow modal
     creates very tall scroll on a typical viewport). */
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
  animation: markdown-detail-modal-zoom var(--duration-base) var(--ease-out);
}

.markdown-detail-modal[data-state="closed"] {
  animation: markdown-detail-modal-zoom-out var(--duration-fast) ease-in forwards;
}

@keyframes markdown-detail-modal-fade {
  from { opacity: 0; }
  to   { opacity: 1; }
}

@keyframes markdown-detail-modal-fade-out {
  from { opacity: 1; }
  to   { opacity: 0; }
}

@keyframes markdown-detail-modal-zoom {
  from { opacity: 0; transform: translate(-50%, -50%) scale(0.96); }
  to   { opacity: 1; transform: translate(-50%, -50%) scale(1); }
}

@keyframes markdown-detail-modal-zoom-out {
  from { opacity: 1; transform: translate(-50%, -50%) scale(1); }
  to   { opacity: 0; transform: translate(-50%, -50%) scale(0.96); }
}

.markdown-detail-modal__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 16px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  flex-shrink: 0;
  gap: 12px;
}

.markdown-detail-modal__title {
  margin: 0;
  font-size: var(--text-base);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
  display: inline-flex;
  align-items: center;
  gap: 8px;
  flex: 1;
  min-width: 0;
}

.markdown-detail-modal__title-text {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.markdown-detail-modal__source-chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 8px;
  font-size: var(--text-xs);
  font-weight: var(--weight-medium);
  border-radius: 10px;
  background: var(--color-bg-border);
  color: var(--color-text-secondary);
  font-family: var(--font-mono);
  flex-shrink: 0;
}

.markdown-detail-modal__source-chip[data-source="prompt"] {
  background: color-mix(in srgb, var(--color-accent) 18%, transparent);
  color: var(--color-accent);
}

.markdown-detail-modal__source-chip[data-source="reply"] {
  background: color-mix(in srgb, var(--color-tool-write) 18%, transparent);
  color: var(--color-tool-write);
}

.markdown-detail-modal__source-chip[data-source="worker"] {
  background: color-mix(in srgb, var(--color-tool-shell) 18%, transparent);
  color: var(--color-tool-shell);
}

.markdown-detail-modal__close {
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

.markdown-detail-modal__close:hover {
  background: var(--color-bg-border);
  color: var(--color-text-primary);
}

.markdown-detail-modal__body {
  flex: 1;
  overflow-y: auto;
  padding: 16px 20px;
  background: var(--color-bg-app);
  min-height: 0;
}

.markdown-detail-modal__markdown {
  font-size: var(--text-base);
  line-height: 1.55;
  color: var(--color-text-primary);
  /* Reuse the bubble typography — the project doesn't have a
     shared `.markdown-body` class; the rules below mirror the
     MessageItem bubble's content styling so markdown looks
     consistent across main chat + drawer modal. */
}

.markdown-detail-modal__markdown :deep(p) {
  margin: 0 0 8px 0;
}

.markdown-detail-modal__markdown :deep(p:last-child) {
  margin-bottom: 0;
}

.markdown-detail-modal__markdown :deep(h1),
.markdown-detail-modal__markdown :deep(h2),
.markdown-detail-modal__markdown :deep(h3) {
  margin: 16px 0 8px 0;
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
}

.markdown-detail-modal__markdown :deep(h1) {
  font-size: 18px;
}
.markdown-detail-modal__markdown :deep(h2) {
  font-size: 15px;
}
.markdown-detail-modal__markdown :deep(h3) {
  font-size: var(--text-base);
}

.markdown-detail-modal__markdown :deep(ul),
.markdown-detail-modal__markdown :deep(ol) {
  margin: 0 0 8px 0;
  padding-left: 20px;
}

.markdown-detail-modal__markdown :deep(li) {
  margin-bottom: 2px;
}

.markdown-detail-modal__markdown :deep(code) {
  font-family: var(--font-mono);
  font-size: var(--text-sm);
  background: var(--color-bg-elevated);
  padding: 1px 4px;
  border-radius: 3px;
  color: var(--color-text-primary);
}

.markdown-detail-modal__markdown :deep(pre) {
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  padding: 10px 12px;
  margin: 8px 0;
  overflow-x: auto;
  font-family: var(--font-mono);
  font-size: var(--text-sm);
  line-height: 1.45;
}

.markdown-detail-modal__markdown :deep(pre code) {
  background: transparent;
  padding: 0;
  border-radius: 0;
}

.markdown-detail-modal__markdown :deep(blockquote) {
  margin: 8px 0;
  padding: 4px 12px;
  border-left: 3px solid var(--color-bg-border-strong);
  color: var(--color-text-secondary);
}

.markdown-detail-modal__markdown :deep(a) {
  color: var(--color-accent);
  text-decoration: none;
}

.markdown-detail-modal__markdown :deep(a:hover) {
  text-decoration: underline;
}
</style>