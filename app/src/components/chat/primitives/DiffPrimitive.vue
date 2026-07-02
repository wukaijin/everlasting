<script setup lang="ts">
// DiffPrimitive — B9 `diff` renderer (Child C of 07-02-b9-diff-primitive,
// 2026-07-02). Registered in `uiPrimitiveRegistry` as the `diff` entry;
// `<UiCard>` mounts one of these per `{ type: "diff", diff_text, title? }`.
//
// The LLM emits a unified-diff string as `diff_text`. This component
// parses it (jsdiff `parsePatch`) into one+ file sections, rebuilds a
// per-file `FileDiff` (path/added/removed/status), and hands them to the
// existing `<DiffView>` — so the rendering (per-line +/- coloring,
// collapse, raw fallback) is identical to the git-diff view. MVP is
// read-only (D4: no apply) + a copy button.
//
// `diff_text` may cover multiple files (one unified blob with several
// `diff --git` / `+++` headers); each parsed patch becomes its own card.

import { ref, computed } from "vue";
import { parsePatch } from "diff";
import DiffView from "../DiffView.vue";
import type { FileDiff } from "../DiffView.vue";
import type { UiPrimitive } from "../uiCard.types";

type ParsedPatch = ReturnType<typeof parsePatch>[number];

const props = defineProps<{ primitive: UiPrimitive }>();

const diffText = computed(() => String(props.primitive.diff_text ?? ""));
const copied = ref(false);

/** Rebuild a unified-diff string for a single parsed patch (DiffView
 *  re-parses it internally — the round-trip is cheap and keeps DiffView's
 *  raw-`<pre>` fallback path intact for malformed hunks). */
function patchToText(p: ParsedPatch): string {
  let out = `--- ${p.oldFileName ?? "a"}\n+++ ${p.newFileName ?? "b"}\n`;
  for (const h of p.hunks) {
    out += `@@ -${h.oldStart},${h.oldLines} +${h.newStart},${h.newLines} @@\n`;
    for (const line of h.lines) out += line + "\n";
  }
  return out.trimEnd();
}

/** Strip the `a/` `b/` prefix git adds to unified-diff headers. */
function cleanPath(name: string | undefined): string {
  const raw = (name ?? "diff").replace(/^"["]*|"["]*$/g, "");
  return raw.replace(/^[ab]\//, "") || "diff";
}

const files = computed<FileDiff[]>(() => {
  const text = diffText.value;
  if (!text.trim()) return [];
  try {
    const patches = parsePatch(text);
    // parsePatch returns [] for input with no patch structure at all.
    // It also returns [{ hunks: [] }] (length 1, empty hunks) for
    // LLM-style +/- fragments that lack `---`/`+++`/`@@` headers —
    // a common misuse from `use_ui`. Both shapes have nothing
    // DiffView can render, so surface the raw text via its raw
    // fallback (`<pre>` path) instead of re-round-tripping an
    // empty `--- a\n+++ b` blob that produces a silently empty body.
    const allHunksEmpty =
      patches.length > 0 && patches.every((p) => p.hunks.length === 0);
    if (patches.length === 0 || allHunksEmpty) {
      return [{ path: "diff", status: "modified", added: 0, removed: 0, diff_text: text }];
    }
    return patches.map((p) => {
      let added = 0;
      let removed = 0;
      for (const h of p.hunks) {
        for (const line of h.lines) {
          if (line.startsWith("+") && !line.startsWith("+++")) added++;
          else if (line.startsWith("-") && !line.startsWith("---")) removed++;
        }
      }
      const status = added === 0 && removed > 0 ? "deleted"
        : removed === 0 && added > 0 ? "added"
        : "modified";
      return {
        path: cleanPath(p.newFileName || p.oldFileName),
        status,
        added,
        removed,
        diff_text: patchToText(p),
      };
    });
  } catch {
    return [{ path: "diff", status: "modified", added: 0, removed: 0, diff_text: text }];
  }
});

async function copyDiff() {
  try {
    await navigator.clipboard.writeText(diffText.value);
    copied.value = true;
    setTimeout(() => { copied.value = false; }, 2000);
  } catch {
    // clipboard unavailable → silent
  }
}
</script>

<template>
  <div class="ui-prim ui-prim--diff">
    <div class="ui-prim__head">
      <span class="ui-prim__type">diff</span>
      <span v-if="primitive.title" class="ui-prim__title">{{ primitive.title }}</span>
      <button class="ui-prim__copy" @click="copyDiff">
        {{ copied ? "已复制" : "复制" }}
      </button>
    </div>
    <DiffView :files="files" />
  </div>
</template>

<style scoped>
.ui-prim--diff {
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
  font-family: var(--font-mono);
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
/* DiffView fills the rest of the card body. */
.ui-prim--diff :deep(.diff-view) {
  gap: 0;
}
.ui-prim--diff :deep(.diff-file) {
  border: 0;
  border-radius: 0;
}
.ui-prim--diff :deep(.diff-file__body) {
  max-height: 400px;
}
</style>
