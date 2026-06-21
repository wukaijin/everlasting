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
//   So this wrapper re-implements the HEADER markup + scoped CSS
//   (reusing design tokens, no hex copies) and mounts the ALREADY-
//   EXTRACTED pure-props body components (`ToolInputBody` /
//   `ToolOutputBody` — FT-F-001 PR1, 2026-06-20). The header CSS
//   block below intentionally mirrors `ToolCallCard.vue`'s
//   `.tool-card*` rules; the visual is identical, the CSS is
//   duplicated by design (see "Why not extract ToolCallHeader.vue"
//   note below).
//
// Why not extract a shared `ToolCallHeader.vue` to kill the CSS
// duplication:
//   The Risk table in the PRD locks "PR4 只抽不改动主 panel
//   ToolCallCard 本体" — extracting a header component would
//   touch `ToolCallCard.vue` (replace its inline header with the
//   new shared component), violating the main-path-0-touch
//   guarantee. The right time to extract is a follow-up that
//   also consolidates the diff/approval/dispatch branches; that's
//   out of PR4's scope. For now: duplicate CSS, single source of
//   truth for tokens (via `--color-*` CSS vars), and a clear
//   comment block so a future refactor knows where to look.
//
// Props are the canonical `ToolCallInfo` + optional `ToolResultInfo`
// (same types `ToolCallCard` consumes) so PR5's DrawerSection can
// build them via the transcript pairing layer without reshaping.

import { computed } from "vue";
import type { ToolCallInfo, ToolResultInfo } from "../../stores/chat";
import {
  toolAccentVar,
  toolIcon,
} from "../../utils/messageFormat";
import { abbreviateDuration } from "../../utils/duration";
import Icon from "../Icon.vue";
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
    subtree. The CSS rules mirror `.tool-card*` 1:1 (same tokens,
    same box model) so the visual matches.
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
    <div class="drawer-tool-card__header">
      <div class="drawer-tool-card__title">
        <span class="drawer-tool-card__icon">
          <Icon :name="toolIcon(call.name)" :size="14" />
        </span>
        <span class="drawer-tool-card__name">{{ call.name }}</span>
        <span v-if="filePath" class="drawer-tool-card__path" :title="filePath">
          · {{ filePath }}
        </span>
      </div>
      <div class="drawer-tool-card__status">
        <span
          :class="[
            'drawer-tool-card__status-icon',
            { 'drawer-tool-card__status-icon--running': !hasResult && !isError },
          ]"
        >
          <Icon :name="statusIconName" :size="14" />
        </span>
        <span>{{ statusText }}</span>
        <span v-if="durationLabel" class="drawer-tool-card__duration">{{
          durationLabel
        }}</span>
      </div>
    </div>

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
/* Mirrors `ToolCallCard.vue` `.tool-card*` 1:1. The class name is
   renamed (`.drawer-tool-card*`) to avoid scoped-CSS collisions;
   the rule bodies are identical and reference the same design
   tokens. See the file header for why a shared `ToolCallHeader.vue`
   extraction is deferred to a follow-up. */

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

.drawer-tool-card__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  min-width: 0;
}

.drawer-tool-card__title {
  display: inline-flex;
  align-items: baseline;
  gap: 6px;
  min-width: 0;
  flex: 1;
  overflow: hidden;
  white-space: nowrap;
}

.drawer-tool-card__icon {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  color: var(--color-text-secondary);
}

.drawer-tool-card--error .drawer-tool-card__icon {
  color: var(--color-tool-error);
}

.drawer-tool-card__name {
  font-weight: 600;
  color: var(--color-text-primary);
}

.drawer-tool-card--error .drawer-tool-card__name {
  color: var(--color-tool-error);
}

.drawer-tool-card__path {
  color: var(--color-text-secondary);
  font-size: 11px;
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
  flex: 1;
}

.drawer-tool-card__status {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 11px;
  color: var(--color-text-muted);
  flex-shrink: 0;
}

.drawer-tool-card__status-icon {
  display: inline-flex;
  align-items: center;
  line-height: 1;
}

.drawer-tool-card__status-icon--running {
  animation: drawer-tool-card-pulse 1.4s ease-in-out infinite;
}

@keyframes drawer-tool-card-pulse {
  0%,
  100% {
    opacity: 1;
  }
  50% {
    opacity: 0.35;
  }
}

.drawer-tool-card--error .drawer-tool-card__status {
  color: var(--color-tool-error);
}

/* F5 per-tool duration — identical to `.tool-card__duration` in
   the main panel. */
.drawer-tool-card__duration {
  display: inline-flex;
  align-items: center;
  margin-left: 2px;
  font-size: 11px;
  font-family: var(--font-mono);
  color: var(--color-text-secondary);
  font-weight: 500;
  user-select: none;
}

.drawer-tool-card--error .drawer-tool-card__duration {
  color: var(--color-tool-error);
}
</style>
