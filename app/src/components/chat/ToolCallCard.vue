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

import { computed, ref, watch } from "vue";
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
import {
  RISK_LABEL_CN,
  RISK_META,
  usePermissionsStore,
  type PermissionDecision,
} from "../../stores/permissions";
import { useSubagentRunsStore } from "../../stores/subagentRuns";
import { isPathInRoot } from "../../utils/path";
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
const permStore = usePermissionsStore();
const subagentRuns = useSubagentRunsStore();
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

// -----------------------------------------------------------------------
// 2026-06-16 (inline approval): when this card's tool_use is the one
// the backend is asking permission for (`pendingAsk.toolUseId ===
// call.id`), render the inline approval UI (replaces the old global
// <PermissionModal>). The ask is per-session; this card only renders
// the current session's messages, so `currentCwd` IS the asking
// session's cwd — fixing the old modal's cross-session cwd mix-up.
// -----------------------------------------------------------------------
const pendingAsk = computed(() => {
  const sid = chatStore.currentSessionId;
  if (!sid) return undefined;
  const ask = permStore.getPending(sid);
  return ask && ask.toolUseId === props.call.id ? ask : undefined;
});

/** Show the approval UI only while there's no result yet — once the
 *  tool resolves (allow→exec→result, deny→result, cancel→result)
 *  the approval window is closed and the regular result view takes
 *  over. */
const isPendingApproval = computed(
  () => !hasResult.value && !!pendingAsk.value,
);

const riskMeta = computed(() =>
  pendingAsk.value ? RISK_META[pendingAsk.value.risk] : null,
);

const showFeedback = ref(false);
const feedback = ref("");

/** In-repo / out-of-repo badge for path tools. `currentCwd` is the
 *  asking session's cwd (this card renders the current session). */
const pathBadgeText = computed(() => {
  const p = pendingAsk.value?.path;
  if (!p) return "";
  const root = chatStore.currentCwd;
  if (!root) return "仓库外";
  return isPathInRoot(p, root) ? "仓库内" : "仓库外";
});
const pathBadgeColor = computed(() =>
  pathBadgeText.value === "仓库内"
    ? "var(--color-tool-write)"
    : "var(--color-tool-shell)",
);

async function respondApproval(
  decision: PermissionDecision,
  reason?: string,
): Promise<void> {
  if (!pendingAsk.value) return;
  await permStore.respond(pendingAsk.value.rid, decision, reason);
  showFeedback.value = false;
  feedback.value = "";
}

function submitDenyFeedback(): void {
  void respondApproval("deny", feedback.value.trim() || undefined);
}

function cancelFeedback(): void {
  showFeedback.value = false;
  feedback.value = "";
}

/** When a result arrives the approval is resolved (allow→exec, deny,
 *  or cancel) — clear the store's pending so its 120s timer can't
 *  later fire a misleading "已超时" toast. */
watch(hasResult, (now, was) => {
  if (now && !was) {
    const sid = chatStore.currentSessionId;
    if (sid && permStore.hasPending(sid)) {
      permStore.clearPending(sid);
    }
  }
});

// -----------------------------------------------------------------------
// B6 PR3 (2026-06-20): dispatch_subagent special branch. Clicking the
// whole card opens the <SubagentDrawer> for the worker run spawned by
// this tool_use (rather than expanding an inline transcript). The
// drawer reads all state from the subagentRuns store; this card only
// needs to resolve the run id and call openDrawer.
// -----------------------------------------------------------------------

/** Is this tool_use a `dispatch_subagent` invocation? */
const isDispatchSubagent = computed<boolean>(
  () => props.call.name === "dispatch_subagent",
);

/** The summary for the worker spawned by this tool_use. Looked up via
 *  `subagentRuns.getSummaryByToolUseId(sessionId, call.id)`, which
 *  matches `parentRequestId.endsWith("-sub-" + call.id)` (the backend
 *  formats the worker rid as `"{parent_rid}-sub-{tool_use_id}"`).
 *  `undefined` until the list cache is populated (e.g. a card that
 *  renders before `fetchForSession` resolves). */
const workerSummary = computed(() => {
  const sid = chatStore.currentSessionId;
  if (!sid) return undefined;
  return subagentRuns.getSummaryByToolUseId(sid, props.call.id);
});

/** Status badge for the collapsed card. Derived from the summary if
 *  we have one; falls back to "running…" while the worker is in
 *  flight but the row hasn't landed yet. */
const workerStatusText = computed<string>(() => {
  const s = workerSummary.value?.status;
  if (!s) {
    // No summary yet: if the card has a result, the worker is done
    // but the cache hasn't loaded; otherwise treat as running.
    return hasResult.value ? "done" : "running…";
  }
  if (s === "running") return "running…";
  return s;
});

/** Short summary preview (≤200 chars). Pulled from the cached summary
 *  if available; otherwise falls back to the tool_result content. */
const workerSummaryPreview = computed<string>(() => {
  const s = workerSummary.value?.summary;
  if (s) return s.length > 200 ? s.slice(0, 200) + "…" : s;
  // Fall back to the tool_result content (which carries the
  // `[status: ...]` prefix from `format_dispatch_result`).
  if (props.result) {
    const display = extractToolResultDisplay(props.result.content);
    return display.length > 200 ? display.slice(0, 200) + "…" : display;
  }
  return "";
});

/** Subagent name to display in the collapsed card. Prefer the cached
 *  summary's name (post-persist canonical value); fall back to the
 *  tool_use's raw `input.subagent` field (so the name shows even
 *  before the row lands / when the worker is in flight). */
const workerDisplayName = computed<string>(() => {
  const summary = workerSummary.value;
  if (summary?.subagentName) return summary.subagentName;
  const input = props.call.input as { subagent?: unknown } | undefined;
  if (input && typeof input.subagent === "string") return input.subagent;
  return "worker";
});

/** Click handler for the whole card when it's a dispatch_subagent.
 *  Resolves the worker's run id (from the summary) and asks the
 *  store to open the drawer. No-op if we haven't resolved the
 *  summary yet — the user can retry once the cache loads. */
async function openSubagentDrawer(): Promise<void> {
  const summary = workerSummary.value;
  if (!summary) return;
  await subagentRuns.openDrawer(summary.id);
}

/** Lazy-load the session's subagent summaries the first time this
 *  card mounts as a dispatch_subagent. The chat store doesn't
 *  pre-fetch this list (it's only needed when a dispatch_subagent
 *  card is visible), so we trigger it here on mount. Idempotent —
 *  the store's `fetchForSession` replaces the cache, so multiple
 *  dispatch_subagent cards in the same session just re-fetch the
 *  same data. */
watch(
  isDispatchSubagent,
  (active) => {
    if (!active) return;
    const sid = chatStore.currentSessionId;
    if (!sid) return;
    void subagentRuns.fetchForSession(sid);
  },
  { immediate: true },
);
</script>

<template>
  <div
    :class="['tool-card', { 'tool-card--error': isError, 'tool-card--running': !hasResult && !isError, 'tool-card--subagent': isDispatchSubagent }]"
    :style="{ borderLeftColor: accent }"
    :role="isDispatchSubagent ? 'button' : undefined"
    :tabindex="isDispatchSubagent ? 0 : undefined"
    @click="isDispatchSubagent ? openSubagentDrawer() : undefined"
    @keydown.enter.prevent="isDispatchSubagent ? openSubagentDrawer() : undefined"
    @keydown.space.prevent="isDispatchSubagent ? openSubagentDrawer() : undefined"
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
        <span>{{ isDispatchSubagent ? workerStatusText : statusText }}</span>
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
      2026-06-16 inline approval: when this tool_use is the one the
      backend is asking about, render the approval actions inline
      (replaces the removed global <PermissionModal>). Only shows
      while there's no result yet (isPendingApproval).
    -->
    <div
      v-if="isPendingApproval && pendingAsk"
      class="tool-card__approval"
    >
      <div class="tool-card__approval-head">
        <span
          class="tool-card__approval-dot"
          :style="{ background: riskMeta?.iconColor }"
        ></span>
        <span class="tool-card__approval-title">需要权限</span>
        <span class="tool-card__approval-risk">风险: {{ RISK_LABEL_CN[pendingAsk.risk] }}</span>
      </div>
      <p v-if="pendingAsk.reason" class="tool-card__approval-reason">{{ pendingAsk.reason }}</p>
      <div v-if="pendingAsk.path" class="tool-card__approval-path">
        <code>{{ pendingAsk.path }}</code>
        <span
          class="tool-card__approval-badge"
          :style="{ color: pathBadgeColor, borderColor: pathBadgeColor }"
        >{{ pathBadgeText }}</span>
      </div>

      <div v-if="showFeedback" class="tool-card__approval-feedback">
        <textarea
          v-model="feedback"
          class="tool-card__approval-textarea"
          rows="2"
          placeholder="告诉 agent 为什么拒绝 / 该怎么做（可选）"
        ></textarea>
        <div class="tool-card__approval-feedback-actions">
          <button type="button" class="tool-card__approval-btn tool-card__approval-btn--deny" @click="submitDenyFeedback">提交拒绝</button>
          <button type="button" class="tool-card__approval-btn" @click="cancelFeedback">取消</button>
        </div>
      </div>
      <div v-else class="tool-card__approval-actions">
        <button type="button" class="tool-card__approval-btn tool-card__approval-btn--once" @click="respondApproval('allow_once')">仅一次</button>
        <button type="button" class="tool-card__approval-btn tool-card__approval-btn--always" @click="respondApproval('allow_always')">始终允许</button>
        <button type="button" class="tool-card__approval-btn tool-card__approval-btn--deny" @click="respondApproval('deny')">拒绝</button>
        <button type="button" class="tool-card__approval-btn tool-card__approval-btn--deny" @click="showFeedback = true">拒绝并说明</button>
      </div>
    </div>

    <!--
      B6 PR3: dispatch_subagent collapsed preview. When this card is a
      dispatch_subagent, we show a one-line status + summary preview
      AND suppress the default input/output <details> (the user clicks
      the card to open the drawer instead). The clickable affordance
      is on the root .tool-card element (see template @click).
    -->
    <div
      v-if="isDispatchSubagent"
      class="tool-card__subagent-preview"
    >
      <div class="tool-card__subagent-meta">
        <span class="tool-card__subagent-name">
          {{ workerDisplayName }}
        </span>
        <span class="tool-card__subagent-status">{{ workerStatusText }}</span>
      </div>
      <p
        v-if="workerSummaryPreview"
        class="tool-card__subagent-summary"
      >{{ workerSummaryPreview }}</p>
      <p v-else class="tool-card__subagent-summary tool-card__subagent-summary--muted">
        点击查看 worker 详情
      </p>
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

    <details v-if="!isDispatchSubagent && call.input && Object.keys(call.input).length" class="tool-card__details">
      <summary>input</summary>
      <pre class="tool-card__pre tool-card__pre--input">{{ formatToolInput(call) }}</pre>
    </details>

    <details v-if="!isDispatchSubagent && result" class="tool-card__details tool-card__details--output">
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

/* 2026-06-16 inline approval UI (replaces the global PermissionModal). */
.tool-card__approval {
  margin-top: 8px;
  padding: 8px 10px;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: 4px;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.tool-card__approval-head {
  display: flex;
  align-items: center;
  gap: 6px;
  font-family: var(--font-sans);
  font-size: 11px;
  color: var(--color-text-secondary);
}

.tool-card__approval-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
}

.tool-card__approval-title {
  font-weight: 600;
  color: var(--color-text-primary);
}

.tool-card__approval-risk {
  color: var(--color-text-muted);
}

.tool-card__approval-reason {
  margin: 0;
  font-family: var(--font-sans);
  font-size: 11px;
  color: var(--color-text-muted);
  line-height: 1.4;
}

.tool-card__approval-path {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}

.tool-card__approval-path code {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--color-text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
  flex: 1;
}

.tool-card__approval-badge {
  flex-shrink: 0;
  padding: 1px 6px;
  border: 1px solid;
  border-radius: 999px;
  font-family: var(--font-sans);
  font-size: 10px;
  line-height: 1.4;
  background: color-mix(in srgb, currentColor 12%, transparent);
}

.tool-card__approval-actions,
.tool-card__approval-feedback-actions {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
}

.tool-card__approval-btn {
  font: inherit;
  font-family: var(--font-sans);
  font-size: 11px;
  padding: 3px 10px;
  border-radius: 4px;
  cursor: pointer;
  border: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
  transition: filter 0.1s;
}

.tool-card__approval-btn:hover {
  filter: brightness(1.08);
}

.tool-card__approval-btn--always {
  background: var(--color-accent);
  color: #ffffff;
  border-color: var(--color-accent);
}

.tool-card__approval-btn--deny {
  color: var(--color-tool-error);
  border-color: var(--color-tool-error);
}

.tool-card__approval-textarea {
  width: 100%;
  font: inherit;
  font-family: var(--font-sans);
  font-size: 11px;
  padding: 4px 6px;
  border: 1px solid var(--color-bg-border);
  border-radius: 4px;
  background: var(--color-bg-surface);
  color: var(--color-text-primary);
  resize: vertical;
}

/* B6 PR3: dispatch_subagent collapsed card → click opens drawer.
   The whole card is a button (role/tabindex set in template); the
   cursor + hover affordance hint that. The preview row carries a
   short summary; clicking anywhere on the card opens the drawer. */
.tool-card--subagent {
  cursor: pointer;
  transition: filter 0.1s, border-color 0.1s;
}
.tool-card--subagent:hover {
  filter: brightness(1.04);
  border-color: var(--color-accent);
}

.tool-card__subagent-preview {
  margin-top: 6px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.tool-card__subagent-meta {
  display: flex;
  align-items: baseline;
  gap: 6px;
}

.tool-card__subagent-name {
  font-family: var(--font-mono);
  font-size: 11px;
  font-weight: 600;
  color: var(--color-text-primary);
}

.tool-card__subagent-status {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--color-text-muted);
}

.tool-card__subagent-summary {
  margin: 0;
  font-family: var(--font-sans);
  font-size: 11px;
  color: var(--color-text-secondary);
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-word;
}

.tool-card__subagent-summary--muted {
  color: var(--color-text-muted);
  font-style: italic;
}
</style>
