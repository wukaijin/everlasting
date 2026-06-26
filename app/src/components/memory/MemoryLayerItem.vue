<script setup lang="ts">
// MemoryLayerItem — single layer card in the Memory Preview panel.
//
// Renders one of the 4 fixed memory files (User CLAUDE.md /
// User AGENTS.md / Project CLAUDE.md / Project AGENTS.md) with:
//   - Title row: `[User CLAUDE.md]` + token count + status badge
//   - Body: rendered markdown (or "Missing" / "Error" placeholder)
//   - Footer: "在外部编辑器打开" button (only when Loaded)
//
// 3-state rendering (per PRD R5):
//   - Loaded: green dot, markdown body, "Open in editor" button
//   - Missing: gray dot, "(文件不存在)" placeholder, no editor button
//   - Error:   yellow dot, "(加载失败)" with tooltip showing the
//              reason, no editor button
//
// Markdown rendering reuses the project-wide `renderMarkdown`
// utility (marked + DOMPurify, see `app/src/utils/markdown.ts`).
// We do NOT use the debounced renderer here — the file is fetched
// once on expand, not streamed, so a 50ms debounce adds nothing.

import { computed, ref, watch } from "vue";

import { renderMarkdown } from "../../utils/markdown";
import { useMemoryStore, type MemoryLayerInfo } from "../../stores/memory";
import Icon from "../Icon.vue";

const props = defineProps<{
  layer: MemoryLayerInfo;
}>();

const emit = defineEmits<{
  /** User clicked "在外部编辑器打开". */
  "open-editor": [path: string];
}>();

// `expanded` is the local open/close state for the body. Defaults
// to false; the panel renders a compact list when collapsed, the
// full body when expanded.
const expanded = ref<boolean>(false);

// `bodyHtml` is the sanitized HTML for the expanded body. `null`
// means "not yet loaded" — we fetch on first expand and cache in
// the store. We DO NOT cache in this component (the store does
// that) so a re-mount or project switch picks up the latest.
const bodyHtml = ref<string | null>(null);
const bodyLoading = ref<boolean>(false);
const bodyError = ref<string | null>(null);
const bodyTruncated = ref<boolean>(false);

const isLoaded = computed<boolean>(
  () => props.layer.status.kind === "loaded",
);
const isMissing = computed<boolean>(
  () => props.layer.status.kind === "missing",
);
const isError = computed<boolean>(
  () => props.layer.status.kind === "error",
);
const errorReason = computed<string | null>(
  () => (props.layer.status.kind === "error" ? props.layer.status.reason : null),
);

// Stable label like `[User CLAUDE.md]`. The Rust side already
// renders this in the LLM banner; we use the same string for
// visual consistency.
const title = computed<string>(() => {
  const k =
    props.layer.kind.charAt(0).toUpperCase() + props.layer.kind.slice(1);
  return `[${k} ${props.layer.source === "claude" ? "CLAUDE.md" : "AGENTS.md"}]`;
});

// Token / char count display. Token is the LLM-facing number
// (cl100k_base estimate). Char count is the local-only secondary
// indicator — the human eye reads it as "size".
const meta = computed<string>(() => {
  if (isMissing.value) return "未创建";
  if (isError.value) return "加载失败";
  // Loaded: "<N> tokens" + char count for human reading.
  const t = props.layer.tokens;
  const c = props.layer.char_count;
  if (t === 0 && c === 0) return "空文件";
  return `${t} tokens · ${c} chars`;
});

// Lazy fetch the body on first expand. After that, the store
// cache (in `useMemoryStore().contentCache`) takes over and a
// second expand is free.
watch(
  expanded,
  async (now) => {
    if (!now || !isLoaded.value) return;
    if (bodyHtml.value !== null) return;
    bodyLoading.value = true;
    bodyError.value = null;
    try {
      const store = useMemoryStore();
      const text = await store.fetchContent(props.layer.path);
      // Truncate very large files: a 100 KiB markdown blob can take
      // seconds to parse + sanitize. The PRD allows per-layer
      // truncation as a UX escape hatch.
      const MAX_BODY_CHARS = 50_000;
      let display = text;
      if (text.length > MAX_BODY_CHARS) {
        display = text.slice(0, MAX_BODY_CHARS);
        bodyTruncated.value = true;
      } else {
        bodyTruncated.value = false;
      }
      bodyHtml.value = renderMarkdown(display);
    } catch (e) {
      bodyError.value = String(e);
    } finally {
      bodyLoading.value = false;
    }
  },
  { immediate: false },
);

function onOpenEditor() {
  emit("open-editor", props.layer.path);
}
</script>

<template>
  <div
    class="memory-layer"
    :class="{
      'memory-layer--loaded': isLoaded,
      'memory-layer--missing': isMissing,
      'memory-layer--error': isError,
    }"
  >
    <button
      class="memory-layer__head"
      :aria-expanded="expanded"
      type="button"
      :disabled="isMissing"
      @click="expanded = !expanded"
    >
      <span
        class="memory-layer__status"
        :class="{
          'memory-layer__status--loaded': isLoaded,
          'memory-layer__status--missing': isMissing,
          'memory-layer__status--error': isError,
        }"
        :title="errorReason ?? ''"
        aria-hidden="true"
      />
      <span class="memory-layer__title">{{ title }}</span>
      <span class="memory-layer__meta">{{ meta }}</span>
      <Icon
        v-if="isLoaded"
        :name="expanded ? 'chevron-up' : 'chevron-down'"
        :size="12"
        icon-class="memory-layer__chevron"
      />
    </button>

    <div v-if="expanded && isLoaded" class="memory-layer__body">
      <div v-if="bodyLoading" class="memory-layer__loading">加载中…</div>
      <div v-else-if="bodyError" class="memory-layer__error-text">
        加载失败:{{ bodyError }}
      </div>
      <div v-else>
        <div class="memory-layer__markdown" v-html="bodyHtml ?? ''" />
        <div v-if="bodyTruncated" class="memory-layer__truncated">
          (内容已截断;在外部编辑器中查看完整文件)
        </div>
      </div>
      <div class="memory-layer__actions">
        <span class="memory-layer__path" :title="layer.path">
          {{ layer.path }}
        </span>
        <button
          class="memory-layer__open"
          type="button"
          @click="onOpenEditor"
        >
          <Icon name="pencil" :size="12" />
          <span>在外部编辑器打开</span>
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.memory-layer {
  display: flex;
  flex-direction: column;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  overflow: hidden;
  transition: border-color var(--duration-base) var(--ease-out);
}

.memory-layer:hover {
  border-color: var(--color-bg-border-strong);
}

.memory-layer--missing {
  opacity: 0.6;
}

.memory-layer__head {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 8px 10px;
  background: transparent;
  border: 0;
  cursor: pointer;
  font-family: inherit;
  font-size: var(--text-sm);
  text-align: left;
  color: var(--color-text-primary);
}

.memory-layer__head:disabled {
  cursor: default;
}

.memory-layer__head:hover:not(:disabled) {
  background: var(--color-bg-border);
}

.memory-layer__status {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
  background: var(--color-bg-border-strong);
}

.memory-layer__status--loaded {
  background: var(--color-status-success); /* green-400 — same family as token-usage ok */
  box-shadow: 0 0 0 2px color-mix(in srgb, var(--color-status-success) 25%, transparent);
}

.memory-layer__status--missing {
  background: var(--color-text-muted);
}

.memory-layer__status--error {
  background: var(--color-status-warn); /* amber-400 — same family as token-usage warn */
  box-shadow: 0 0 0 2px color-mix(in srgb, var(--color-status-warn) 25%, transparent);
}

.memory-layer__title {
  font-weight: var(--weight-medium);
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--color-text-primary);
}

.memory-layer__meta {
  flex: 1;
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  font-family: var(--font-mono);
  text-align: right;
}

.memory-layer__chevron {
  color: var(--color-text-muted);
  flex-shrink: 0;
}

.memory-layer__body {
  border-top: 1px solid var(--color-bg-border);
  padding: 10px;
  background: var(--color-bg-surface);
}

.memory-layer__loading,
.memory-layer__error-text {
  font-size: var(--text-sm);
  color: var(--color-text-muted);
  padding: 8px 0;
}

.memory-layer__error-text {
  color: var(--color-tool-error);
}

.memory-layer__markdown {
  font-size: var(--text-sm);
  line-height: 1.6;
  color: var(--color-text-secondary);
  /* Don't let giant tables blow out the panel; horizontal scroll
     is the standard markdown-render escape hatch. */
  overflow-x: auto;
  max-height: 60vh;
  overflow-y: auto;
}

.memory-layer__markdown :deep(p) {
  margin: 0 0 8px 0;
}

.memory-layer__markdown :deep(p:last-child) {
  margin-bottom: 0;
}

.memory-layer__markdown :deep(pre) {
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  padding: 8px 10px;
  overflow-x: auto;
  font-size: var(--text-xs);
  font-family: var(--font-mono);
}

.memory-layer__markdown :deep(code) {
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: 3px;
  padding: 1px 4px;
  font-size: var(--text-xs);
  font-family: var(--font-mono);
}

.memory-layer__markdown :deep(pre code) {
  background: transparent;
  border: 0;
  padding: 0;
}

.memory-layer__markdown :deep(h1),
.memory-layer__markdown :deep(h2),
.memory-layer__markdown :deep(h3) {
  color: var(--color-text-primary);
  font-weight: var(--weight-semibold);
  margin: 12px 0 6px 0;
}

.memory-layer__markdown :deep(ul),
.memory-layer__markdown :deep(ol) {
  padding-left: 20px;
  margin: 0 0 8px 0;
}

.memory-layer__markdown :deep(a) {
  color: var(--color-accent);
  text-decoration: underline;
}

.memory-layer__truncated {
  margin-top: 6px;
  padding: 4px 8px;
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  background: var(--color-bg-elevated);
  border-radius: var(--radius-sm);
  font-style: italic;
}

.memory-layer__actions {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-top: 10px;
  padding-top: 8px;
  border-top: 1px solid var(--color-bg-border);
  gap: 8px;
}

.memory-layer__path {
  font-family: var(--font-mono);
  font-size: var(--text-2xs);
  color: var(--color-text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
  flex: 1;
}

.memory-layer__open {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
  padding: 4px 8px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  color: var(--color-text-primary);
  font-size: var(--text-xs);
  font-family: inherit;
  cursor: pointer;
  transition: border-color var(--duration-base) var(--ease-out), background var(--duration-base) var(--ease-out);
}

.memory-layer__open:hover {
  background: var(--color-accent-muted);
  border-color: var(--color-accent);
  color: var(--color-text-primary);
}
</style>
