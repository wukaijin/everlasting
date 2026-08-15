<script setup lang="ts">
// ButtonPrimitive — B9+ D3 `button` renderer
// (07-13-b9plus-generative-ui-followup, 2026-07-13).
//
// Registered in `uiPrimitiveRegistry` as the `button` entry;
// `<UiCard>` mounts one of these per
// `{ type: "button", action, label?, payload? }`.
//
// # Action dispatch (D-Q2a/b: predefined enum)
//
// Three actions land in the renderer. Each follows the user-
// intent semantics of D-Q1 (the click IS the authorization; no
// permission-layer round-trip):
//
// - `apply_diff` → invokes `apply_ui_diff` IPC with
//   `payload.diff_text` (same IPC as `<DiffPrimitive>` Apply).
// - `copy` → writes `payload.text` to the user's clipboard via
//   `navigator.clipboard.writeText` (pure frontend, no backend).
// - `dismiss` → hides this card locally (pure frontend).
//
// Success: per-action feedback (toast for apply_diff / copy; card
// hides for dismiss). Failure for `apply_diff`: inline error keyed
// by backend `kind` (boundary / parse / conflict / io / empty).
//
// The card stays visible after a failed apply so the user can read
// the error and decide to retry / dismiss.

import { ref, computed } from "vue";
import { applyUiDiff, ApplyUiDiffError } from "../../../utils/uiDiffApply";
import { useChatStore } from "../../../stores/chat";
import { useProjectsStore } from "../../../stores/projects";
import type { UiButtonPrimitive, UiButtonAction } from "../uiCard.types";

const props = defineProps<{ primitive: UiButtonPrimitive }>();

const chatStore = useChatStore();
const projectsStore = useProjectsStore();

/** Action display label fallback when the LLM doesn't provide `label`.
 *  Keeps the renderer self-contained — every action has a sensible
 *  default so the LLM doesn't have to spell it out every time. */
const DEFAULT_LABELS: Record<UiButtonAction, string> = {
  apply_diff: "应用",
  copy: "复制",
  dismiss: "关闭",
};

const label = computed<string>(
  () => props.primitive.label || DEFAULT_LABELS[props.primitive.action],
);

/** Click-handler state. `idle` (default) → `working` (in-flight) →
 *  `done` (success, hide after 1.5s). For `dismiss`, transitions
 *  straight to `done` immediately. Failure: back to `idle` with
 *  `errorKind` populated. */
type ButtonState = "idle" | "working" | "done";
const state = ref<ButtonState>("idle");
const errorKind = ref<string | null>(null);

const canApply = computed<boolean>(() => {
  if (state.value !== "idle") return false;
  // `apply_diff` requires an active session (the IPC writes files
  // under `session.worktree_path` ?? `session.current_cwd`).
  if (props.primitive.action === "apply_diff" && !chatStore.currentSessionId) {
    return false;
  }
  return true;
});

const disabledReason = computed<string>(() => {
  if (state.value === "working") return "处理中...";
  if (state.value === "done") return label.value;
  if (
    props.primitive.action === "apply_diff" &&
    !chatStore.currentSessionId
  ) {
    return "无活跃会话";
  }
  return "";
});

async function onClick() {
  if (!canApply.value) return;
  state.value = "working";
  errorKind.value = null;
  const action = props.primitive.action;
  try {
    if (action === "apply_diff") {
      const sid = chatStore.currentSessionId;
      if (!sid) throw new Error("missing sessionId");
      const diffText = String(props.primitive.payload?.diff_text ?? "");
      const written = await applyUiDiff(sid, diffText);
      projectsStore.showToast(`已应用 ${written.length} 个文件`, "info", 3000);
      state.value = "done";
    } else if (action === "copy") {
      const text = String(props.primitive.payload?.text ?? "");
      if (!text) throw new Error("copy action missing payload.text");
      await navigator.clipboard.writeText(text);
      projectsStore.showToast("已复制到剪贴板", "info", 1500);
      state.value = "done";
    } else if (action === "dismiss") {
      // Pure-frontend hide — no IPC, no toast. Local-only.
      state.value = "done";
      // Card hides immediately (v-if below).
    }
  } catch (e) {
    state.value = "idle";
    if (e instanceof ApplyUiDiffError) {
      errorKind.value = e.kind;
    } else {
      errorKind.value = "io";
      // eslint-disable-next-line no-console
      console.error("button action error", e);
    }
  }
}
</script>

<template>
  <div v-if="state !== 'done'" class="ui-prim ui-prim--button">
    <button
      class="ui-prim__btn"
      :class="`ui-prim__btn--${primitive.action}`"
      :disabled="!canApply"
      :title="disabledReason"
      @click="onClick"
    >
      {{ state === "working" ? "处理中..." : label }}
    </button>
    <div v-if="errorKind" class="ui-prim__error" role="alert">
      {{ errorKind === "io" ? "操作失败 — 请查看 console" : `失败：${errorKind}` }}
    </div>
  </div>
</template>

<style scoped>
.ui-prim--button {
  display: inline-flex;
  flex-direction: column;
  gap: 4px;
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  background: var(--color-bg-surface);
  padding: 8px 10px;
  max-width: fit-content;
}
.ui-prim__btn {
  font-size: var(--text-sm);
  padding: 4px 12px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  border: 1px solid var(--color-bg-border);
  background: transparent;
  transition:
    background var(--duration-fast) var(--ease-out),
    color var(--duration-fast) var(--ease-out),
    border-color var(--duration-fast) var(--ease-out);
}
/* Action-specific accent so the user reads the action shape at a
   glance. Matches the `--color-tool-*` family convention used
   elsewhere (write=emerald, shell=amber, error=red). */
.ui-prim__btn--apply_diff {
  color: var(--color-tool-write);
  border-color: color-mix(in srgb, var(--color-tool-write) 40%, var(--color-bg-border));
}
.ui-prim__btn--apply_diff:hover:not(:disabled) {
  background: var(--color-tool-write);
  color: var(--color-bg-app);
}
.ui-prim__btn--copy {
  color: var(--color-text-primary);
}
.ui-prim__btn--copy:hover:not(:disabled) {
  background: var(--color-bg-elevated);
}
.ui-prim__btn--dismiss {
  color: var(--color-text-muted);
}
.ui-prim__btn--dismiss:hover:not(:disabled) {
  color: var(--color-tool-error-text);
  border-color: color-mix(in srgb, var(--color-tool-error) 40%, var(--color-bg-border));
}
.ui-prim__btn:disabled {
  cursor: not-allowed;
  opacity: 0.5;
}
.ui-prim__error {
  font-size: var(--text-xs);
  color: var(--color-tool-error-text);
  padding: 2px 0;
}
</style>