<script setup lang="ts">
// CodeBlockPrimitive — B9 `code_block` renderer (Child B of
// 07-02-b9-code-block-primitive, 2026-07-02). Registered in
// `uiPrimitiveRegistry` as the `code_block` entry; `<UiCard>` mounts
// one of these per `{ type: "code_block", code, language?, title? }`.
//
// Highlights via the shared `renderCodeHtml` (same helper the markdown
// pipeline uses → identical language support). The primitive is a
// structured card, NOT markdown text, so it does NOT go through marked
// — it calls `renderCodeHtml` directly and binds the result via v-html
// (hljs escapes its input, so the output is safe HTML).
//
// Copy button: `navigator.clipboard.writeText` + a 2s "已复制" ack.
// The clipboard API can throw outside a secure context (http); we
// swallow that silently (Tauri runs under https/file, so this is
// defensive).

import { ref, computed } from "vue";
import { renderCodeHtml } from "../../../utils/highlight";
import type { UiPrimitive } from "../uiCard.types";
// hljs theme — global CSS (NOT scoped). Applies the color palette to
// `.hljs` + `.hljs-*` token classes emitted by renderCodeHtml. Imported
// here so the theme loads iff a code primitive mounts (code-split).
import "highlight.js/styles/github-dark.css";

const props = defineProps<{ primitive: UiPrimitive }>();

const code = computed(() => String(props.primitive.code ?? ""));
const language = computed(() => String(props.primitive.language ?? ""));
const highlighted = computed(() => renderCodeHtml(code.value, language.value));

const copied = ref(false);
async function copyCode() {
  try {
    await navigator.clipboard.writeText(code.value);
    copied.value = true;
    setTimeout(() => {
      copied.value = false;
    }, 2000);
  } catch {
    // clipboard unavailable (non-secure context) → silent
  }
}
</script>

<template>
  <div class="ui-prim ui-prim--code">
    <div class="ui-prim__head">
      <span class="ui-prim__type">{{ language || "code" }}</span>
      <span v-if="primitive.title" class="ui-prim__title">{{ primitive.title }}</span>
      <button class="ui-prim__copy" @click="copyCode">
        {{ copied ? "已复制" : "复制" }}
      </button>
    </div>
    <pre class="ui-prim__code"><code class="hljs" v-html="highlighted"></code></pre>
  </div>
</template>

<style scoped>
.ui-prim--code {
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  background: var(--color-bg-surface);
  overflow: hidden;
}
.ui-prim__head {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 10px;
  border-bottom: 1px solid var(--color-bg-border);
  font-size: 12px;
}
.ui-prim__type {
  font-family: monospace;
  font-weight: 600;
  color: var(--color-text-primary);
}
.ui-prim__title {
  color: var(--color-text-secondary);
}
.ui-prim__copy {
  margin-left: auto;
  padding: 2px 8px;
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm, 4px);
  background: transparent;
  color: var(--color-text-secondary);
  font-size: 11px;
  cursor: pointer;
}
.ui-prim__copy:hover {
  color: var(--color-text-primary);
}
.ui-prim__code {
  margin: 0;
  padding: 10px 12px;
  overflow: auto;
  font-family: monospace;
  font-size: 12px;
  line-height: 1.5;
  max-height: 400px;
}
</style>
