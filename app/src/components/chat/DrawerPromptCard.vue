<script setup lang="ts">
// DrawerPromptCard — the top-of-drawer card showing the parent
// LLM's prompt that dispatched the worker.
//
// PR5 of the subagent-drawer redesign (2026-06-21). Per PRD R1 +
// R13 + Q5 + Decision (DrawerPromptCard 独立卡片): the drawer's
// first body element (below the header) is a non-collapsible card
// that shows `run.task` (PR1 column — the parent LLM's
// `dispatch_subagent` task argument). Long prompts are truncated
// to 120 chars (per PRD R13 budget); a "View full →" link opens
// `MarkdownDetailModal` with `source="prompt"`.
//
// Why a standalone card (not part of a DrawerSection):
//   - The prompt is a SINGLE blob (not a stream of entries), so
//     the `DrawerSection` collapse + entry-list pattern doesn't
//     fit. A flat card matches the "header → single body" shape.
//   - The prompt is the FIRST thing the user wants to see when
//     they open a worker drawer ("what did the parent LLM ask
//     the worker to do?"). Collapsing it by default would hide
//     the most load-bearing context.
//   - The card is hidden entirely when `run.task === null`
//     (pre-PR1 legacy row, or worker still starting). This
//     avoids an empty card flashing on first paint.
//
// `truncate` lives in `utils/useTruncate.ts` (PR3) — pure function,
// no reactivity. The 120-char budget matches the PRD R13 default;
// the suffix "…" is the single Unicode ellipsis.

import { computed, ref } from "vue";
import { truncate } from "../../utils/useTruncate";
import { renderMarkdown } from "../../utils/markdown";
import MarkdownDetailModal from "../common/MarkdownDetailModal.vue";
import Icon from "../Icon.vue";

const props = defineProps<{
  /** The parent LLM's prompt verbatim (PR1 column
   *  `subagent_runs.task`). Nullable for legacy rows + workers
   *  that haven't received a task yet. When null, the parent
   *  drawer does NOT mount this card at all (v-if gate). */
  task: string | null;
}>();

/** Truncation budget per PRD R13. Single source of truth —
 *  changing this constant updates both the preview and the
 *  "View full →" affordance visibility. */
const TASK_MAX_CHARS = 120;

const modalOpen = ref<boolean>(false);

/** Truncated preview. Empty string when `task` is null/empty
 *  (the parent's v-if hides the whole card in that case, but
 *  the computed is still called defensively). */
const preview = computed<string>(() => {
  const t = props.task ?? "";
  if (t.length === 0) return "";
  return truncate(t, TASK_MAX_CHARS);
});

/** Whether the full text differs from the preview (i.e. the
 *  truncate call actually shortened it). Drives the "View full →"
 *  affordance visibility. */
const isTruncated = computed<boolean>(() => {
  const t = props.task ?? "";
  return t.length > preview.value.length;
});

/** Rendered HTML for the preview. Goes through `renderMarkdown`
 *  (marked + DOMPurify) so markdown formatting survives the
 *  truncation. The truncation happens on the RAW markdown string
 *  (before rendering), which means a code fence split at the
 *  boundary is backtracked to the fence opener by `truncate` —
 *  the rendered preview never shows an unclosed code block. */
const previewHtml = computed<string>(() => renderMarkdown(preview.value));
</script>

<template>
  <section v-if="task && task.length > 0" class="drawer-prompt-card">
    <header class="drawer-prompt-card__header">
      <span class="drawer-prompt-card__icon">
        <Icon name="thinking" :size="12" />
      </span>
      <span class="drawer-prompt-card__label">Worker Prompt</span>
    </header>
    <div class="drawer-prompt-card__body">
      <div class="drawer-prompt-card__markdown" v-html="previewHtml" />
      <button
        v-if="isTruncated"
        type="button"
        class="drawer-prompt-card__view-full"
        @click="modalOpen = true"
      >
        View full →
      </button>
    </div>
    <MarkdownDetailModal
      v-model:open="modalOpen"
      title="Worker Prompt"
      :markdown="task ?? ''"
      source="prompt"
    />
  </section>
</template>

<style scoped>
.drawer-prompt-card {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-left: 3px solid var(--color-accent);
  border-radius: var(--radius-md);
  padding: 8px 12px;
  font-size: var(--text-sm);
  color: var(--color-text-primary);
  max-width: 100%;
  margin-bottom: 8px;
}

.drawer-prompt-card__header {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 4px;
  color: var(--color-accent);
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  font-weight: var(--weight-semibold);
  text-transform: uppercase;
  letter-spacing: 0.04em;
}

.drawer-prompt-card__icon {
  display: inline-flex;
  align-items: center;
}

.drawer-prompt-card__label {
  /* visual rhythm with the Tools / Reply segment chips */
}

.drawer-prompt-card__body {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.drawer-prompt-card__markdown {
  font-size: var(--text-sm);
  line-height: 1.5;
  color: var(--color-text-secondary);
  max-height: 160px;
  overflow-y: auto;
}

.drawer-prompt-card__markdown :deep(p) {
  margin: 0 0 6px 0;
}

.drawer-prompt-card__markdown :deep(p:last-child) {
  margin-bottom: 0;
}

.drawer-prompt-card__markdown :deep(code) {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  background: var(--color-bg-elevated);
  padding: 1px 4px;
  border-radius: 3px;
}

.drawer-prompt-card__markdown :deep(pre) {
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  padding: 8px 10px;
  margin: 6px 0;
  overflow-x: auto;
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  line-height: 1.4;
}

.drawer-prompt-card__view-full {
  align-self: flex-start;
  background: transparent;
  border: 0;
  color: var(--color-accent);
  cursor: pointer;
  font: inherit;
  font-family: var(--font-sans);
  font-size: var(--text-xs);
  padding: 2px 0;
  text-decoration: none;
}

.drawer-prompt-card__view-full:hover {
  text-decoration: underline;
}
</style>
