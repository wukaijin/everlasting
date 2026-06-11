<script setup lang="ts">
// ToolCallCard — a single tool invocation with its input / output
// blocks. Per spike-003 the card has a 3px left bar that switches
// color by tool name: read_file → cyan, write_file → emerald,
// shell → amber. The error state (the matching tool_result reports
// is_error) flips the bar to red and tints the card body.
//
// D5 restructure: the header is a single line — icon + tool name +
// file path on the left, status on the right. Matches the spike-003
// reference (ui-A.png). The input section stays collapsed by default;
// the output is shown directly when present (not inside <details>)
// so the user sees the result immediately. Long input / output is
// capped at ~200px tall and overflows with scroll.
//
// F5 (LLM Latency Tracking): the header's right-side status row
// also renders the per-tool duration, measured by the controller
// from `tool:call` → `tool:result` and embedded in the persisted
// `tool_result` block as `duration_ms` (per PRD R2 / ADR-lite
// decision 1). The duration shows next to the status text:
//   - running: "…" (the in-flight indicator — duration unknown)
//   - done / error: "0.3s" / "1.2s" / "12.4s" via
//     `abbreviateDuration`. The `?` cursor + the existing
//     color-when-error treatment still apply.

import { computed, ref } from "vue";
import {
  useChatStore,
  type ToolCallInfo,
  type ToolResultInfo,
} from "../../stores/chat";
import {
  extractToolResultDisplay,
  formatToolInput,
  truncateOutput,
  toolAccentVar,
  toolIcon,
} from "../../utils/messageFormat";
import { abbreviateDuration } from "../../utils/duration";
import DiffView from "./DiffView.vue";
import Icon from "../Icon.vue";

const props = defineProps<{
  call: ToolCallInfo;
  result?: ToolResultInfo;
}>();

const accent = computed(() => {
  if (props.result?.isError) return "var(--color-tool-error)";
  return toolAccentVar(props.call.name);
});

const isError = computed(() => !!props.result?.isError);
const hasResult = computed(() => !!props.result);

/** Best-effort file path for display in the header. Most tools pass
 *  `path` in their input; shell uses `command` which is too long to
 *  fit, so we leave it out. Non-string values are guarded. */
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

/** F5: per-tool duration label rendered next to `statusText` in
 *  the header. Returns:
 *    - "…" while the tool is running (no `result` yet — duration
 *      unknown; the placeholder reads as a generic "still going")
 *    - "0.3s" / "1.2s" / "12.4s" / "1m 4s" when `result.durationMs`
 *      is set (post-F5 row with timing; pre-F5 rows have it
 *      undefined and we render nothing)
 *    - "" (empty) when the result is present but no duration was
 *      captured (pre-F5 row, OR the in-memory measurement race
 *      lost — defensive)
 *
 *  The `?` cursor and the right-side `tool-card__status` flex gap
 *  (4px) take care of the visual when the label is empty. */
const durationLabel = computed<string>(() => {
  if (!hasResult.value) return "…";
  const d = props.result?.durationMs;
  if (typeof d !== "number") return "";
  return abbreviateDuration(d);
});

/** Map the run state to a heroicon name for the status indicator.
 *  "running" uses an animated ellipsis (handled by CSS); the other
 *  two are static check / X marks. */
const statusIconName = computed<string>(() => {
  if (isError.value) return "x";
  if (hasResult.value) return "check";
  return "ellipsis";
});

/** Human-readable size of the tool result content for the <summary>
 *  hint. We use character count (not UTF-8 bytes) because tool
 *  results in this app are always text and chars read more honestly
 *  for that case. The label "chars" is omitted when the number is
 *  under 1024 (just a bare count reads fine for a few hundred
 *  characters); the suffix reappears for K/M to disambiguate.
 *
 *  Step 4 follow-up: the LLM-facing content is the cwd envelope
 *  (`{result, cwd}`), but the UI display is the unwrapped
 *  `result` string. The size hint must reflect what the user
 *  sees, so we run the content through `extractToolResultDisplay`
 *  before counting chars. */
const outputSize = computed<string>(() => {
  if (!props.result) return "";
  const display = extractToolResultDisplay(props.result.content);
  const n = display.length;
  if (n < 1024) return `${n} chars`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)}K chars`;
  return `${(n / 1024 / 1024).toFixed(1)}M chars`;
});

/** Display-only view of the tool result content. Strips the cwd
 *  envelope (see REQ-16 in prd.md) so the card shows the actual
 *  tool output, not the raw JSON. */
const displayContent = computed<string | null>(() => {
  if (!props.result) return null;
  return extractToolResultDisplay(props.result.content);
});

// -----------------------------------------------------------------------
// Step 4 / PR3: per-edit_file diff popover. The card's "diff" button
// is only rendered for the `edit_file` tool and only when the tool
// has a `path` AND a session is active (so we know which session's
// diff to query). Clicking toggles a small popover below the card
// header that shows that file's portion of the session diff.
//
// We use the session-level diff (worktree vs project main) rather
// than capturing a pre-edit snapshot per call: the LLM's
// `read_file` tool result is in the message history, and showing
// the cumulative session diff per file is usually more useful than
// isolating a single edit_file's contribution. The popover falls
// back to "no changes in this session" when the file isn't in the
// cached diff (e.g. the edit failed or was on a file the LLM
// didn't actually change).
// -----------------------------------------------------------------------
const chatStore = useChatStore();
const fileDiffOpen = ref(false);
const fileDiffLoading = ref(false);
const fileDiffError = ref<string | null>(null);

const fileDiff = computed<import("../../stores/chat").FileDiff | null>(() => {
  const sid = chatStore.currentSessionId;
  if (!sid || !filePath.value) return null;
  return chatStore.getFileDiff(sid, filePath.value);
});

/** Step 4 follow-up: only render the diff button when the session
 *  has an active worktree. Pre-follow-up the diff was always
 *  there; now sessions can be in `none` (no worktree) or
 *  `detached` (worktree not bound to this session), in which
 *  case the per-file diff is meaningless (the agent's tools
 *  ran against the project root, not a per-session worktree). */
const showDiffButton = computed<boolean>(
  () =>
    props.call.name === "edit_file" &&
    !!filePath.value &&
    chatStore.sessions.find((s) => s.id === chatStore.currentSessionId)
      ?.worktree_state === "active",
);

async function toggleFileDiff() {
  if (fileDiffOpen.value) {
    fileDiffOpen.value = false;
    return;
  }
  fileDiffOpen.value = true;
  if (fileDiff.value) {
    // Already cached.
    return;
  }
  // Fetch the session diff so getFileDiff has something to read.
  const sid = chatStore.currentSessionId;
  if (!sid) return;
  fileDiffLoading.value = true;
  fileDiffError.value = null;
  try {
    await chatStore.fetchDiff(sid);
  } catch (e) {
    fileDiffError.value = e instanceof Error ? e.message : String(e);
  } finally {
    fileDiffLoading.value = false;
  }
}
</script>

<template>
  <div
    :class="['tool-card', { 'tool-card--error': isError, 'tool-card--running': !hasResult && !isError }]"
    :style="{ borderLeftColor: accent }"
  >
    <div class="tool-card__header">
      <div class="tool-card__title">
        <span class="tool-card__icon">
          <Icon :name="toolIcon(call.name)" :size="14" />
        </span>
        <span class="tool-card__name">{{ call.name }}</span>
        <span v-if="filePath" class="tool-card__path" :title="filePath">
          · {{ filePath }}
        </span>
      </div>
      <div class="tool-card__status">
        <span
          :class="['tool-card__status-icon', { 'tool-card__status-icon--running': !hasResult && !isError }]"
        >
          <Icon :name="statusIconName" :size="14" />
        </span>
        <span>{{ statusText }}</span>
        <!--
          F5: per-tool duration (next to status text). Renders
          the `durationLabel` ("…" while running, "0.3s" /
          "1.2s" after done; empty for pre-F5 rows). The
          separate `<span>` (vs. concatenating into statusText)
          keeps the `?` cursor / color theming on the status
          text untouched — the duration is its own visual
          element with its own muted color and a slight
          left margin to avoid the icon-glyph crowding.
        -->
        <span v-if="durationLabel" class="tool-card__duration">{{ durationLabel }}</span>
        <button
          v-if="showDiffButton"
          type="button"
          class="tool-card__diff-btn"
          :title="
            fileDiffOpen
              ? 'Hide diff for this file'
              : 'Show diff for this file in this session'
          "
          @click="toggleFileDiff"
        >
            <Icon :name="fileDiffOpen ? 'chevron-down' : 'chevron-right'" :size="12" />
            diff
        </button>
      </div>
    </div>

    <!--
      Per-file diff popover. Rendered only for edit_file cards
      when the user clicks the diff button. The popover is inline
      (not floating) so it scrolls with the message list; for long
      diffs the inner DiffView scrolls its own body.
    -->
    <div v-if="fileDiffOpen && showDiffButton" class="tool-card__diff">
      <div v-if="fileDiffLoading" class="tool-card__diff-loading">
        Loading diff…
      </div>
      <div v-else-if="fileDiffError" class="tool-card__diff-error">
        {{ fileDiffError }}
      </div>
      <DiffView
        v-else-if="fileDiff"
        :files="[fileDiff]"
      />
      <div
        v-else
        class="tool-card__diff-empty"
      >
        <em>No changes to this file in this session (the edit may have failed or was a no-op).</em>
      </div>
    </div>

    <details v-if="call.input && Object.keys(call.input).length" class="tool-card__details">
      <summary>input</summary>
      <pre class="tool-card__pre tool-card__pre--input">{{ formatToolInput(call) }}</pre>
    </details>

    <details v-if="result" class="tool-card__details tool-card__details--output">
      <summary>output · {{ outputSize }}</summary>
      <pre class="tool-card__pre tool-card__pre--output">{{ truncateOutput(displayContent ?? result.content) }}</pre>
    </details>
  </div>
</template>

<style scoped>
.tool-card {
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

.tool-card--error {
  border-color: var(--color-tool-error);
  background: var(--color-bg-elevated);
}

.tool-card--running {
  border-left-color: var(--color-tool-shell);
}

.tool-card__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  min-width: 0;
}

.tool-card__title {
  display: inline-flex;
  align-items: baseline;
  gap: 6px;
  min-width: 0;
  flex: 1;
  overflow: hidden;
  white-space: nowrap;
}

.tool-card__icon {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  color: var(--color-text-secondary);
}

.tool-card--error .tool-card__icon {
  color: var(--color-tool-error);
}

.tool-card__name {
  font-weight: 600;
  color: var(--color-text-primary);
}

.tool-card--error .tool-card__name {
  color: var(--color-tool-error);
}

.tool-card__path {
  color: var(--color-text-secondary);
  font-size: 11px;
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
  flex: 1;
}

.tool-card__status {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 11px;
  color: var(--color-text-muted);
  flex-shrink: 0;
}

.tool-card__status-icon {
  display: inline-flex;
  align-items: center;
  line-height: 1;
}

.tool-card__status-icon--running {
  animation: tool-card-pulse 1.4s ease-in-out infinite;
}

@keyframes tool-card-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.35; }
}

.tool-card--error .tool-card__status {
  color: var(--color-tool-error);
}

/* F5 (LLM Latency Tracking): per-tool duration display. Renders
   the "0.3s" / "…" label inside the existing `tool-card__status`
   row, right after the status text. Mono font matches the rest
   of the status row; the slightly-elevated color is a hint
   that this is a measured value, not part of the status
   label itself. */
.tool-card__duration {
  display: inline-flex;
  align-items: center;
  margin-left: 2px;
  font-size: 11px;
  font-family: var(--font-mono);
  color: var(--color-text-secondary);
  font-weight: 500;
  user-select: none;
}

.tool-card--error .tool-card__duration {
  color: var(--color-tool-error);
}

.tool-card__details {
  margin-top: 6px;
}

.tool-card__details summary {
  cursor: pointer;
  color: var(--color-text-secondary);
  font-size: 11px;
  user-select: none;
  list-style: none;
}

.tool-card__details summary::-webkit-details-marker {
  display: none;
}

.tool-card__details summary::before {
  content: "▸ ";
  color: var(--color-text-muted);
}

.tool-card__details[open] summary::before {
  content: "▾ ";
}

.tool-card__details summary:hover {
  color: var(--color-text-primary);
}

.tool-card__details--output {
  margin-top: 6px;
}

.tool-card__pre {
  margin: 0;
  padding: 6px 8px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 4px;
  white-space: pre-wrap;
  word-break: break-all;
  max-height: 200px;
  overflow-y: auto;
  font-size: 11px;
  line-height: 1.4;
  color: var(--color-text-primary);
  font-family: var(--font-mono);
}

/* Step 4 / PR3: per-file diff button + popover inside the
 * tool card. The button sits in the header status row; the
 * popover replaces the regular card body when open. */
.tool-card__diff-btn {
  margin-left: 8px;
  display: inline-flex;
  align-items: center;
  gap: 3px;
  padding: 2px 8px;
  background: var(--color-accent-muted);
  color: var(--color-accent);
  border: 1px solid var(--color-accent);
  border-radius: 4px;
  font: inherit;
  font-size: 11px;
  cursor: pointer;
}

.tool-card__diff-btn:hover {
  background: var(--color-accent);
  color: var(--color-bg-app);
}

.tool-card__diff {
  margin-top: 6px;
  padding: 8px;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: 4px;
  max-height: 480px;
  overflow-y: auto;
}

.tool-card__diff-loading,
.tool-card__diff-error,
.tool-card__diff-empty {
  padding: 12px;
  text-align: center;
  color: var(--color-text-muted);
  font-size: 11px;
  font-family: var(--font-sans);
}

.tool-card__diff-error {
  color: var(--color-tool-error);
}
</style>
