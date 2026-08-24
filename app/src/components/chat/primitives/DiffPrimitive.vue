<script setup lang="ts">
// DiffPrimitive — B9 `diff` renderer (Child C of 07-02-b9-diff-primitive,
// 2026-07-02; D4 apply/reject buttons added in 07-13-b9plus-generative-
// ui-followup, 2026-07-13). Registered in `uiPrimitiveRegistry` as the
// `diff` entry; `<UiCard>` mounts one of these per `{ type: "diff",
// diff_text, title? }`.
//
// The LLM emits a unified-diff string as `diff_text`. This component
// parses it (jsdiff `parsePatch`) into one+ file sections, rebuilds a
// per-file `FileDiff` (path/added/removed/status), and hands them to the
// existing `<DiffView>` — so the rendering (per-line +/- coloring,
// collapse, raw fallback) is identical to the git-diff view. MVP is
// read-only + a copy button.
//
// # B9+ D4: apply / reject buttons (2026-07-13)
//
// Two buttons in the header (`应用` / `拒绝`):
// - `应用` invokes `applyUiDiff(sessionId, diff_text)`. On success:
//   toast "已应用 N 个文件" + card marked「已应用」(buttons disabled).
//   On failure: inline error keyed by backend `kind`
//   (`boundary` / `parse` / `conflict` / `io` / `empty`).
// - `拒绝` is a no-op local-only dismiss (hides the card).
//
// **Raw-fallback gate**: if the input is a headerless LLM-style
// +/- fragment (no `---`/`+++` path headers), the apply button is
// `disabled` with a tooltip「该 diff 格式不可应用」— there is no
// unambiguous file target the backend can write to. The reject
// button still works (the user can dismiss noise cards).
//
// `applyUiDiff` is a USER IPC (not an LLM tool), so it does not
// consult the permission layer — the click IS the authorization.
// See `commands/ui.rs::apply_ui_diff` for the Rust contract.

import { ref, computed } from "vue";
import { parsePatch } from "diff";
import DiffView from "../DiffView.vue";
import type { FileDiff } from "../DiffView.vue";
import type { UiPrimitive } from "../uiCard.types";
import {
  applyUiDiff,
  ApplyUiDiffError,
  APPLY_UI_DIFF_ERROR_TEXT,
  type ApplyUiDiffFailureKind,
} from "../../../utils/uiDiffApply";
import { useChatStore } from "../../../stores/chat";
import { useProjectsStore } from "../../../stores/projects";

type ParsedPatch = ReturnType<typeof parsePatch>[number];

const props = defineProps<{ primitive: UiPrimitive }>();

const chatStore = useChatStore();
const projectsStore = useProjectsStore();

const diffText = computed(() => String(props.primitive.diff_text ?? ""));
const copied = ref(false);

/** True when `diff_text` looks like a standard unified diff
 *  (has `--- ` and `+++ ` path headers somewhere in the body).
 *  LLM-style headerless +/- fragments are invalid apply targets
 *  — the backend would parse-fail with `kind = "parse"`. We gate
 *  the apply button here so the user gets immediate visual feedback
 *  instead of triggering a round-trip just to surface the same
 *  error.
 *
 *  Cheap regex probe (no full parse). Matches the prefix of any
 *  line; jsdiff's parsePatch is the authoritative check on the
 *  Rust side. */
const hasUnifiedHeaders = computed<boolean>(() => {
  const t = diffText.value;
  return /^--- /m.test(t) && /^\+\+\+ /m.test(t);
});

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
    const allHunksEmpty =
      patches.length > 0 && patches.every((p) => p.hunks.length === 0);
    if (patches.length === 0 || allHunksEmpty) {
      // Raw fallback: count +/- lines so DiffView's header surfaces
      // the same +N/-M badge that a parsed unified-diff would.
      let added = 0;
      let removed = 0;
      for (const line of text.split("\n")) {
        if (line.startsWith("+") && !line.startsWith("+++")) added++;
        else if (line.startsWith("-") && !line.startsWith("---")) removed++;
      }
      return [{ path: "diff", status: "modified", added, removed, diff_text: text }];
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

// ---- Apply / reject state machine ----

/** `idle` (default) → `applying` (in-flight) → `applied` (success) |
 *  `rejected` (user clicked 拒绝, hidden). On failure: back to `idle`
 *  with `errorKind` populated for inline error rendering. The card
 *  stays visible on failure so the user can read the error and decide
 *  to retry / reject. */
type ApplyState = "idle" | "applying" | "applied" | "rejected";
const applyState = ref<ApplyState>("idle");
const errorKind = ref<ApplyUiDiffFailureKind | null>(null);

const canApply = computed<boolean>(
  () =>
    hasUnifiedHeaders.value &&
    applyState.value !== "applying" &&
    applyState.value !== "applied" &&
    applyState.value !== "rejected" &&
    !!chatStore.currentSessionId,
);

const applyDisabledReason = computed<string>(() => {
  if (applyState.value === "applied") return "已应用";
  if (applyState.value === "applying") return "应用中...";
  if (applyState.value === "rejected") return "已拒绝";
  if (!hasUnifiedHeaders.value) return "该 diff 格式不可应用（需带 ---/+++ 路径头的标准 unified diff）";
  if (!chatStore.currentSessionId) return "无活跃会话";
  return "";
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

async function applyDiff() {
  const sid = chatStore.currentSessionId;
  if (!sid || !canApply.value) return;
  applyState.value = "applying";
  errorKind.value = null;
  try {
    const written = await applyUiDiff(sid, diffText.value);
    applyState.value = "applied";
    projectsStore.showToast(
      `已应用 ${written.length} 个文件`,
      "info",
      3000,
    );
  } catch (e) {
    applyState.value = "idle";
    if (e instanceof ApplyUiDiffError) {
      errorKind.value = e.kind;
    } else {
      // Unexpected error (network, IPC serialization, etc.) — surface
      // as `io` to fit the known-kind taxonomy.
      errorKind.value = "io";
      // eslint-disable-next-line no-console
      console.error("apply_ui_diff: unexpected error", e);
    }
  }
}

function rejectDiff() {
  applyState.value = "rejected";
}
</script>

<template>
  <div
    v-if="applyState !== 'rejected'"
    class="ui-prim ui-prim--diff"
  >
    <div class="ui-prim__head">
      <span class="ui-prim__type">diff</span>
      <span v-if="primitive.title" class="ui-prim__title">{{ primitive.title }}</span>
      <span v-if="applyState === 'applied'" class="ui-prim__applied-tag">已应用</span>
      <button
        v-if="applyState === 'idle' || applyState === 'applying'"
        class="ui-prim__apply"
        :disabled="!canApply"
        :title="applyDisabledReason"
        @click="applyDiff"
      >
        {{ applyState === "applying" ? "应用中..." : "应用" }}
      </button>
      <button
        v-if="applyState === 'idle'"
        class="ui-prim__reject"
        title="关闭此 diff 卡片（不影响文件）"
        @click="rejectDiff"
      >
        拒绝
      </button>
      <button class="ui-prim__copy" @click="copyDiff">
        {{ copied ? "已复制" : "复制" }}
      </button>
    </div>
    <div v-if="errorKind" class="ui-prim__error" role="alert">
      {{ APPLY_UI_DIFF_ERROR_TEXT[errorKind] }}
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
.ui-prim__applied-tag {
  color: var(--color-tool-write);
  font-weight: var(--weight-medium);
  font-size: var(--text-xs);
  border: 1px solid color-mix(in srgb, var(--color-tool-write) 35%, transparent);
  border-radius: 999px;
  padding: 1px 8px;
}
.ui-prim__copy,
.ui-prim__apply,
.ui-prim__reject {
  margin-left: auto;
  padding: 2px 8px;
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--color-text-secondary);
  font-size: 11px;
  cursor: pointer;
  transition: color var(--duration-fast) var(--ease-out),
    border-color var(--duration-fast) var(--ease-out);
}
.ui-prim__apply,
.ui-prim__reject {
  margin-left: auto;
}
/* Spacing between consecutive header buttons: use a small left
   margin on copy so it doesn't crowd the apply/reject pair. */
.ui-prim__copy {
  margin-left: 0;
}
.ui-prim__apply {
  color: var(--color-tool-write);
  border-color: color-mix(in srgb, var(--color-tool-write) 40%, var(--color-bg-border));
}
.ui-prim__apply:hover:not(:disabled) {
  color: var(--color-bg-app);
  background: var(--color-tool-write);
}
.ui-prim__apply:disabled {
  cursor: not-allowed;
  opacity: 0.5;
}
.ui-prim__reject:hover {
  color: var(--color-tool-error-text);
  border-color: color-mix(in srgb, var(--color-tool-error) 40%, var(--color-bg-border));
}
.ui-prim__copy:hover {
  color: var(--color-text-primary);
}
.ui-prim__error {
  padding: 6px 10px;
  background: color-mix(in srgb, var(--color-tool-error) 10%, transparent);
  border-bottom: 1px solid color-mix(in srgb, var(--color-tool-error) 35%, transparent);
  color: var(--color-tool-error-text);
  font-size: var(--text-xs);
  line-height: 1.4;
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