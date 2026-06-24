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
//
// FT-F-001 PR1 (2026-06-20): the input/output `<details>` blocks
// and the inline approval UI have been extracted into
// `ToolInputBody` / `ToolOutputBody` / `PermissionAskBody`
// (shared body components). The outer wrapper keeps all store
// dependencies (`useChatStore` / `usePermissionsStore` /
// `useSubagentRunsStore`) — bodies don't read stores; this card
// owns the store and passes plain data props + a callback.

import { computed, onUnmounted, ref, watch } from "vue";
import { useChatStore } from "../../stores/chat";
import type {
  ToolCallInfo,
  ToolResultInfo,
} from "../../stores/chat.types";
import {
  extractToolResultDisplay,
  toolAccentVar,
  toolIcon,
} from "../../utils/messageFormat";
import {
  usePermissionsStore,
  type PermissionDecision,
} from "../../stores/permissions";
import { useSubagentRunsStore } from "../../stores/subagentRuns";
import type { SubagentRunSummary } from "../../stores/subagentRuns.types";
import { abbreviateDuration } from "../../utils/duration";
import DiffView from "./DiffView.vue";
import Icon from "../Icon.vue";
import ToolCallHeader from "./ToolCallHeader.vue";
import ToolInputBody from "./ToolInputBody.vue";
import ToolOutputBody from "./ToolOutputBody.vue";
import PermissionAskBody from "./PermissionAskBody.vue";

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

/** Display-only view of the tool result content. Strips the cwd
 *  envelope (see REQ-16 in prd.md) so the card shows the actual
 *  tool output, not the raw JSON. Used by the dispatch_subagent
 *  preview fallback (FT-F-001 PR1 — the input/output bodies now
 *  own their own envelope unwrapping, but the dispatch preview
 *  still needs the unwrapped string for its 200-char summary). */
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
//
// FT-F-001 PR1 (D6): the diff popover stays inline — only the
// edit_file tool uses diffs, and the drawer does not render diffs.
// Extracting a `DiffBody` would have ~10 lines of CSS for marginal
// reuse; not worth the extra component surface.
// -----------------------------------------------------------------------
const chatStore = useChatStore();
const permStore = usePermissionsStore();
const subagentRuns = useSubagentRunsStore();
const fileDiffOpen = ref(false);
const fileDiffLoading = ref(false);
const fileDiffError = ref<string | null>(null);

const fileDiff = computed<import("../../stores/chat.types").FileDiff | null>(() => {
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
//
// FT-F-001 PR1: the inline approval block has been extracted into
// `<PermissionAskBody mode="interactive" :ask="pendingAsk" :onRespond="respondApproval" />`.
// The store + response handler stay here; the body just renders.
// `repoRoot` is passed so the body can compute the in-repo /
// out-of-repo badge against the asking session's cwd (matches the
// pre-extraction behavior — `chatStore.currentCwd` IS the asking
// session's cwd because this card renders the current session).
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

async function respondApproval(
  decision: PermissionDecision,
  reason?: string,
): Promise<void> {
  if (!pendingAsk.value) return;
  await permStore.respond(pendingAsk.value.rid, decision, reason);
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
//
// FT-F-001 PR1 (D1): the dispatch_subagent preview stays inline —
// it's the main panel's collapsed affordance for the drawer trigger;
// the drawer itself does not render a pre-dispatch preview.
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
    const display = displayContent.value ?? props.result.content;
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
 *  store to open the drawer. Handles the B6 PR3b (2026-06-20)
 *  race: if the cache is empty (e.g. the initial fetchForSession
 *  raced against the backend's insert_run), the click triggers a
 *  waiting state and polls for up to 1.5s. The store's
 *  `subagent:event` listener bridges the same race via eager-fetch
 *  on first event arrival; the polling here catches the
 *  "click-before-first-event" window (which the listener can't
 *  help with since no event has fired yet). */
const workerWaiting = ref(false);

/** FT-F-002 (2026-06-21): third card state — a prior click's 1.5s
 *  retry polling exhausted without resolving (worker never emitted
 *  its first event / DB insert failed / IPC down). Unlike
 *  `workerWaiting` (transient spinner during polling), this sticks
 *  until the user clicks again (retry clears it at the top of
 *  `openSubagentDrawer`) or the component unmounts (session switch
 *  rebuilds the card → resets to default). Drives the "⚠ worker
 *  未响应,点此重试" inline hint so the silent fallback to the
 *  default visual becomes an explicit "that didn't work" signal. */
const workerMissed = ref(false);

/** FT-F-003 (2026-06-20): unmount guard for the retry polling loop
 *  below. `openSubagentDrawer`'s while loop uses `await new
 *  Promise(r => setTimeout(r, 300))` to pace its 5 ticks — there's
 *  no timer id to clearTimeout (it's an await loop, not a nested
 *  setTimeout chain). When the component unmounts mid-poll (e.g.
 *  the user switches sessions during the 1.5s window), the pending
 *  `await` resolves on an unmounted card and the loop would
 *  otherwise continue writing `workerWaiting` / calling
 *  `openDrawer` on the dead instance. The guard below is checked
 *  after every await + before every side-effect; unmount sets it
 *  to true and the loop returns early on the next tick.
 *
 *  Sufficient set: `await` is the only yield point in the loop, so
 *  unmount can only happen during a pending await. Checking
 *  immediately after each await + before each side-effect covers
 *  every possible unmount window with the minimum number of guards.
 *  (immediate / afterRetry early-return branches also guard their
 *  post-await openDrawer calls defensively — see comments inline.)
 */
let unmounted = false;
onUnmounted(() => {
  unmounted = true;
});

async function openSubagentDrawer(): Promise<void> {
  // FT-F-002: clear any prior missed state — this click is a fresh
  // attempt (first open, or a retry after a previous miss).
  workerMissed.value = false;
  const sid = chatStore.currentSessionId;
  // Explicit type annotation: vue-tsc's narrowing on
  // ComputedRef.value through `if (immediate)` is unreliable
  // (collapses to `never`); the annotation gives the narrowed
  // branch a concrete type without relying on the narrowing.
  const immediate: SubagentRunSummary | undefined = workerSummary.value;
  if (immediate) {
    await subagentRuns.openDrawer(immediate.id);
    // FT-F-003: defensive guard — openDrawer is async (it awaits
    // fetchRun), so the component can unmount during the await.
    // Skip the (no-op) return path side-effects once unmounted.
    if (unmounted) return;
    return;
  }
  // 2. Cache miss — show waiting UI + fire one extra fetchForSession.
  //    Covers the case where the mount-time IPC is still in flight
  //    OR lost the race. Pinia replaces the cache atomically on
  //    resolve, so even a redundant fetch is safe.
  workerWaiting.value = true;
  if (sid) await subagentRuns.fetchForSession(sid);
  // FT-F-003: fetchForSession is async — the component can unmount
  // during the IPC await. Bail before writing workerWaiting /
  // calling openDrawer on a dead card.
  if (unmounted) return;
  const afterRetry: SubagentRunSummary | undefined = workerSummary.value;
  if (afterRetry) {
    workerWaiting.value = false;
    await subagentRuns.openDrawer(afterRetry.id);
    if (unmounted) return;
    return;
  }
  // 3. Still miss — poll for up to 1.5s (300ms intervals, ~5 ticks).
  //    The store's IPC listener will eager-fetch on the first
  //    subagent:event arrival; this loop catches that + gives the
  //    IPC roundtrip room to complete.
  const start = Date.now();
  while (Date.now() - start < 1500) {
    await new Promise((r) => setTimeout(r, 300));
    // FT-F-003: the await above is the primary unmount window.
    // Bail before any side-effect — don't fetchForSession, don't
    // read the computed, don't write workerWaiting / openDrawer.
    if (unmounted) return;
    if (sid) await subagentRuns.fetchForSession(sid);
    // FT-F-003: fetchForSession is itself async — re-check after
    // it resolves so we don't act on a card that unmounted
    // during the IPC round-trip.
    if (unmounted) return;
    const s: SubagentRunSummary | undefined = workerSummary.value;
    if (s) {
      workerWaiting.value = false;
      await subagentRuns.openDrawer(s.id);
      if (unmounted) return;
      return;
    }
  }
  // 4. 1.5s elapsed without resolving. Likely the worker hasn't
  //    emitted its first event yet OR the DB insert failed. Let
  //    the user retry by clicking again — don't trap them in a
  //    permanent spinner.
  // FT-F-003: re-check after the final await — if the component
  // unmounted during the last tick's await, leave workerWaiting
  // alone (writing it on an unmounted ref is the original bug).
  if (unmounted) return;
  workerWaiting.value = false;
  // FT-F-002: 1.5s elapsed without resolving — surface an explicit
  // "didn't work" hint instead of silently falling back to the
  // default visual. The hint re-points the user at the retry path
  // (clicking the card again re-enters openSubagentDrawer, which
  // clears workerMissed at the top).
  workerMissed.value = true;
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
    :class="[
      'tool-card',
      { 'tool-card--error': isError, 'tool-card--running': !hasResult && !isError, 'tool-card--subagent': isDispatchSubagent, 'tool-card--subagent-waiting': isDispatchSubagent && workerWaiting },
    ]"
    :style="{ borderLeftColor: accent }"
    :role="isDispatchSubagent ? 'button' : undefined"
    :tabindex="isDispatchSubagent ? 0 : undefined"
    :aria-busy="isDispatchSubagent && workerWaiting ? true : undefined"
    @click="isDispatchSubagent ? openSubagentDrawer() : undefined"
    @keydown.enter.prevent="isDispatchSubagent ? openSubagentDrawer() : undefined"
    @keydown.space.prevent="isDispatchSubagent ? openSubagentDrawer() : undefined"
  >
    <ToolCallHeader
      :icon-name="toolIcon(call.name)"
      :name="call.name"
      :file-path="filePath"
      :status-text="isDispatchSubagent ? workerStatusText : statusText"
      :status-icon-name="statusIconName"
      :duration-label="durationLabel"
      :is-error="isError"
      :is-running="!hasResult && !isError"
    >
      <template #status-extra>
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
      </template>
    </ToolCallHeader>

    <!--
      2026-06-16 inline approval: when this tool_use is the one the
      backend is asking about, render the approval actions inline
      (replaces the removed global <PermissionModal>). Only shows
      while there's no result yet (isPendingApproval).

      FT-F-001 PR1: the inline block has been extracted into
      `<PermissionAskBody>`. `repoRoot` is passed so the body can
      compute the in-repo / out-of-repo badge using
      `chatStore.currentCwd` (which IS the asking session's cwd —
      see the cross-session cwd mix-up fix from 2026-06-16).

      The outer `.tool-card__approval` wrapper class is preserved
      for behavioral parity with the pre-extraction layout — the
      existing `ToolCallCard.test.ts` tests check for this class
      to detect "approval UI present / absent" (lock against
      regressions where the body fails to mount). The wrapper
      carries no visual styling itself; the body renders its own
      scoped CSS.
    -->
    <div v-if="isPendingApproval && pendingAsk" class="tool-card__approval">
      <PermissionAskBody
        mode="interactive"
        :ask="pendingAsk"
        :on-respond="respondApproval"
        :repo-root="chatStore.currentCwd"
      />
    </div>

    <!--
      B6 PR3: dispatch_subagent collapsed preview. When this card is a
      dispatch_subagent, we show a one-line status + summary preview
      AND suppress the default input/output <details> (the user clicks
      the card to open the drawer instead). The clickable affordance
      is on the root .tool-card element (see template @click).

      FT-F-001 PR1 (D1): stays inline. Drawer doesn't need a
      pre-dispatch preview.
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
      <p
        v-else-if="workerWaiting"
        class="tool-card__subagent-summary tool-card__subagent-summary--muted"
      >等待 worker 注册…</p>
      <!-- FT-F-002 (2026-06-21): 1.5s retry exhausted without
           resolving — explicit "didn't work" hint replacing the
           old silent fallback to the default visual. warn icon
           reuses the registry entry FT-F-005 introduced; the
           --missed variant tints it as a warning. Clicking the
           card re-enters openSubagentDrawer (retry). -->
      <p
        v-else-if="workerMissed"
        class="tool-card__subagent-summary tool-card__subagent-summary--missed"
      ><Icon name="warn" :size="11" /> worker 未响应,点此重试</p>
      <p
        v-else
        class="tool-card__subagent-summary tool-card__subagent-summary--muted"
      >点击查看 worker 详情</p>
    </div>

    <!--
      Per-file diff popover. Rendered only for edit_file cards
      when the user clicks the diff button. The popover is inline
      (not floating) so it scrolls with the message list; for long
      diffs the inner DiffView scrolls its own body.

      FT-F-001 PR1 (D6): stays inline. Drawer doesn't render diffs.
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

    <!--
      FT-F-001 PR1: input/output bodies are now shared components.
      `<ToolInputBody>` auto-renders an empty-input gated block
      (the body's `<details>` is the only element; empty input is
      handled at the call site to match the old `Object.keys(...).length`
      gate that suppressed the empty `<details>`).
    -->
    <ToolInputBody
      v-if="!isDispatchSubagent && call.input && Object.keys(call.input).length > 0"
      :name="call.name"
      :input="call.input"
    />
    <ToolOutputBody
      v-if="!isDispatchSubagent && result"
      :content="result.content"
      :is-error="result.isError"
      :duration-ms="result.durationMs"
    />
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

/* header markup + CSS 已抽到 `<ToolCallHeader>` (RULE-FrontSubagent-001,
   2026-06-25)。本组件保留 card 容器 + error/running/subagent 容器变体 +
   diff-btn / diff popover / approval / dispatch preview 等非 header 规则。 */

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

/* B6 PR3b (2026-06-20): waiting state during the click-time race
   resolution (1.5s max polling window — see openSubagentDrawer).
   Overrides the default pointer cursor + hover affordance so the
   user sees the click was registered and is being processed.
   No new colors (per Q4 decision D4) — the existing
   --color-bg-border keeps the card visually unchanged; only the
   cursor + absence of hover lift signal the waiting. */
.tool-card--subagent-waiting {
  cursor: wait;
}
.tool-card--subagent-waiting:hover {
  filter: none;
  border-color: var(--color-bg-border);
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

/* FT-F-002 (2026-06-21): missed-state hint — warning tint
   (--color-tool-shell, the same amber FT-F-005's cancelled banner
   + permission_ask badge use, NOT error red — a miss isn't a hard
   failure, the worker may just be slow). inline-flex aligns the
   warn icon with the text. */
.tool-card__subagent-summary--missed {
  color: var(--color-tool-shell);
  display: inline-flex;
  align-items: center;
  gap: 4px;
}
</style>
