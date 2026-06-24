<script setup lang="ts">
// DrawerToolCallCard — drawer-side tool-call card.
//
// PR4 of the subagent-drawer redesign (2026-06-21). Per PRD R6 +
// R7 + Decision 1: the drawer shares the main panel's VISUAL
// appearance (same header layout + ToolInputBody + ToolOutputBody
// + design tokens) but does NOT reuse `<ToolCallCard>` directly.
//
// Why not `<ToolCallCard :call :result>`:
//   `ToolCallCard.vue` is NOT a pure visual component — it reads 3
//   Pinia stores (useChatStore / usePermissionsStore /
//   useSubagentRunsStore) for:
//     - per-file diff popover (edit_file → `chatStore.getFileDiff`
//       + `chatStore.fetchDiff` + worktree_state gate)
//     - inline approval UI (permission:ask matching `call.id` →
//       `permStore.getPending(currentSessionId)` + `respond`)
//     - dispatch_subagent collapsed preview + click-to-open-drawer
//       (`subagentRuns.getSummaryByToolUseId` + `openDrawer`)
//
//   The drawer renders the WORKER's transcript (a sub-agent's
//   tool_call entries), not the parent session's messages. Wiring
//   those store reads to the worker's context would either:
//     (a) mis-resolve (worker permission asks don't hang off the
//         parent session's `permStore.getPending(currentSessionId)`)
//     (b) trigger a recursive drawer-open (a worker that itself
//         dispatches a sub-sub-agent would call `openDrawer` and
//         replace the currently-open drawer — violates PRD Out of
//         Scope "单例 drawer")
//     (c) pull in diff popover the PRD explicitly defers (PR4 does
//         not render worker diffs; PR6 only adds permission_ask
//         handling)
//
// RULE-FrontSubagent-001 (2026-06-25): header markup + CSS 抽到共享
//   `<ToolCallHeader>` —— redesign PR1-6 收尾后,原 PR4「主 panel
//   ToolCallCard 本体 0 改动」约束解除,故可抽(推翻 chat.md 旧决策)。
//   本 wrapper 仍只复用 0-store 的纯展示子组件:ToolCallHeader(header)
//   + ToolInputBody / ToolOutputBody(FT-F-001 PR1,body)。card 容器
//   chrome(背景 / 边框 / 3px left bar / padding)保留在本组件 scoped CSS;
//   header 内文字颜色由 ToolCallHeader 的 isError / isRunning prop 自治。
//
// Props are the canonical `ToolCallInfo` + optional `ToolResultInfo`
// (same types `ToolCallCard` consumes) so PR5's DrawerSection can
// build them via the transcript pairing layer without reshaping.

import { computed } from "vue";
import type { ToolCallInfo, ToolResultInfo } from "../../stores/chat.types";
import {
  toolAccentVar,
  toolIcon,
} from "../../utils/messageFormat";
import { abbreviateDuration } from "../../utils/duration";
import ToolCallHeader from "./ToolCallHeader.vue";
import ToolInputBody from "./ToolInputBody.vue";
import ToolOutputBody from "./ToolOutputBody.vue";

const props = defineProps<{
  call: ToolCallInfo;
  /** The matching tool_result, if any. Absent while the worker
   *  is still executing this tool_use (the card shows "running…"). */
  result?: ToolResultInfo;
}>();

const isError = computed(() => !!props.result?.isError);
const hasResult = computed(() => !!props.result);

/** 3px left bar color. Error flips it to red regardless of tool
 *  name (matches the main panel's `tool-card--error` treatment). */
const accent = computed(() => {
  if (isError.value) return "var(--color-tool-error)";
  return toolAccentVar(props.call.name);
});

/** Best-effort file path for the header. Most tools pass `path`;
 *  shell uses `command` which is too long, so we leave it out
 *  (matches `ToolCallCard.vue`'s `filePath` computed). */
const filePath = computed<string | null>(() => {
  const input = props.call.input;
  if (!input) return null;
  const p = input.path;
  if (typeof p === "string" && p.length > 0) return p;
  return null;
});

const statusText = computed<string>(() => {
  if (isError.value) return "error";
  if (hasResult.value) return "done";
  return "running…";
});

/** Per-tool duration label rendered next to `statusText`. Same
 *  rules as `ToolCallCard.vue`'s `durationLabel`:
 *    - "…" while running (no result yet)
 *    - "0.3s" / "1m 4s" via `abbreviateDuration` when
 *      `result.durationMs` is set
 *    - "" (empty) when the result is present but no duration
 *      (pre-F5 row, or worker transcript row that didn't carry
 *      the measurement — defensive) */
const durationLabel = computed<string>(() => {
  if (!hasResult.value) return "…";
  const d = props.result?.durationMs;
  if (typeof d !== "number") return "";
  return abbreviateDuration(d);
});

/** Status icon name (heroicons key in Icon.vue registry).
 *  - error   → x
 *  - done    → check
 *  - running → ellipsis (CSS animates the pulse) */
const statusIconName = computed<string>(() => {
  if (isError.value) return "x";
  if (hasResult.value) return "check";
  return "ellipsis";
});
</script>

<template>
  <!--
    Root class is `.drawer-tool-card` (distinct from the main panel's
    `.tool-card`) so the scoped CSS below doesn't collide with
    `ToolCallCard.vue`'s selectors if both ever render in the same
    subtree. The card chrome (container + error/running variants) is
    kept here; the header markup + CSS lives in `<ToolCallHeader>`.
  -->
  <div
    :class="[
      'drawer-tool-card',
      {
        'drawer-tool-card--error': isError,
        'drawer-tool-card--running': !hasResult && !isError,
      },
    ]"
    :style="{ borderLeftColor: accent }"
  >
    <ToolCallHeader
      :icon-name="toolIcon(call.name)"
      :name="call.name"
      :file-path="filePath"
      :status-text="statusText"
      :status-icon-name="statusIconName"
      :duration-label="durationLabel"
      :is-error="isError"
      :is-running="!hasResult && !isError"
    />

    <!--
      Reusing the already-extracted pure-props body components
      (`ToolInputBody` / `ToolOutputBody` — FT-F-001 PR1, 2026-06-20).
      Same v-if gates as `ToolCallCard.vue` so the visual matches:
        - input <details> hidden when input is empty
        - output <details> only when a result is present
      No diff button, no approval UI, no dispatch_subagent preview —
      those are main-panel-only concerns (see file header).
    -->
    <ToolInputBody
      v-if="call.input && Object.keys(call.input).length > 0"
      :name="call.name"
      :input="call.input"
    />
    <ToolOutputBody
      v-if="result"
      :content="result.content"
      :is-error="result.isError"
      :duration-ms="result.durationMs"
    />
  </div>
</template>

<style scoped>
/* Card 容器 chrome。header markup + CSS 已抽到 `<ToolCallHeader>`
   (RULE-FrontSubagent-001, 2026-06-25);本组件只保留 card 容器
   (背景 / 边框 / 3px left bar / padding / 字体) + error/running
   容器变体(控制 card 边框/背景;header 内文字颜色由 ToolCallHeader
   的 isError / isRunning prop 自治)。0 hex,全 design token。 */

.drawer-tool-card {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-left: 3px solid var(--color-text-muted);
  border-radius: 6px;
  padding: 8px 12px;
  font-size: 12px;
  font-family: var(--font-mono);
  color: var(--color-text-primary);
  max-width: 100%;
}

.drawer-tool-card--error {
  border-color: var(--color-tool-error);
  background: var(--color-bg-elevated);
}

.drawer-tool-card--running {
  border-left-color: var(--color-tool-shell);
}
</style>
