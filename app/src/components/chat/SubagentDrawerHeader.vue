<script setup lang="ts">
// SubagentDrawerHeader — top-of-drawer header bar.
//
// Split out of SubagentDrawer (2026-06-23, PRD
// `06-23-06-23-split-subagent-drawer`) so the main drawer can drop
// to ~900 lines. Pure presentation — no store reads, no ticker, no
// scroll orchestration. The main drawer passes in everything this
// component needs to render: the run row, the typed status, the
// pre-computed status pill text/color, the optional failure banner
// text, and the `truncated` flag (drives the small "原 transcript 已
// 截断" hint at the bottom of the header).
//
// Layout (PRD R6 R25 / FT-F-005):
//
//   +----------------------------------------------------------------+
//   | [运行中 8.2s]  worker                                       [X]|
//   | ⚠ Worker exited with error: ...                                |
//   | 开始 12:34:56  结束 12:35:11                                   |
//   | short summary text...                                          |
//   | 原 transcript 已截断 (head + tail)                             |
//   +----------------------------------------------------------------+
//
// Note: the `↗ jump to latest` button (visible when `!autoFollow &&
// sections.length > 0`) was REMOVED from the header and moved to
// the body top — its visibility + click handler depend on the body's
// `autoFollow` / `newCount` / `bodyEl` state, which the main drawer
// orchestrates. Decoupling the header from those is the main point
// of this split (A 方案, user-confirmed in the PRD ADR-lite section).

import { DialogClose } from "reka-ui";
import Icon from "../Icon.vue";
import { formatTime } from "../../utils/time";
import type {
  SubagentRunRow,
  SubagentStatus,
} from "../../stores/subagentRuns.types";

/** Status pill text/color/suffix, pre-computed by the main drawer
 *  (the 100ms `nowTick` ticker lives in main — this component just
 *  reads the snapshot). Avoids re-exporting the ticker through a
 *  ref / event chain. */
export type SubagentDrawerHeaderStatusDisplay = {
  label: string;
  color: string;
  suffix: string;
};

/** Failure / warning banner text. The main drawer's `bannerText`
 *  computed returns `null` for `running` / `completed` (and when
 *  the row hasn't loaded yet) — we use that to v-if the entire
 *  banner block. */
export type SubagentDrawerHeaderBannerText = {
  kind: "error" | "warning";
  text: string;
} | null;

defineProps<{
  /** The cached full row for the currently-open run (main drawer's
   *  `store.openRun`). `undefined` while `fetchRun` is in flight —
   *  the header renders name/summary placeholders in that window. */
  run: SubagentRunRow | undefined;
  /** The typed status (coerced from `run.status` raw string by
   *  main's `coerceStatus`). Drives the `title=` tooltip on the
   *  status badge ("Status: running", etc.). */
  status: SubagentStatus;
  /** Pre-computed status pill: `label` (e.g. "运行中"), `color`
   *  (a `--color-tool-*` token resolved by the main drawer's
   *  STATUS_META), and `suffix` (e.g. " 8.2s" for running,
   *  " at 4.2s" for terminal states). */
  statusDisplay: SubagentDrawerHeaderStatusDisplay;
  /** Optional failure banner text. `null` → no banner rendered. */
  bannerText: SubagentDrawerHeaderBannerText;
  /** True when the row's `transcriptTruncated` is non-zero — drives
   *  the small "原 transcript 已截断 (head + tail)" hint at the
   *  bottom of the header (matches the 4 MiB cap on the backend). */
  truncated: boolean;
}>();
</script>

<template>
  <header class="subagent-drawer__header">
    <div class="subagent-drawer__title-row">
      <span
        class="subagent-drawer__status"
        :style="{ color: statusDisplay.color, borderColor: statusDisplay.color }"
        :title="`Status: ${status}`"
      >{{ statusDisplay.label }}{{ statusDisplay.suffix }}</span>
      <span class="subagent-drawer__name">
        {{ run?.subagentName ?? "worker" }}
      </span>
      <DialogClose
        class="subagent-drawer__close"
        aria-label="Close"
      >
        <Icon name="x" :size="14" />
      </DialogClose>
    </div>
    <div
      v-if="bannerText"
      :class="[
        'subagent-drawer__banner',
        `subagent-drawer__banner--${bannerText.kind}`,
      ]"
      role="status"
      :aria-label="bannerText.text"
    >
      <Icon name="warn" :size="14" />
      <span class="subagent-drawer__banner-text">{{ bannerText.text }}</span>
    </div>
    <div
      v-if="run?.startedAt"
      class="subagent-drawer__meta"
    >
      <span class="subagent-drawer__meta-time">
        <Icon name="clock" :size="11" />
        开始 {{ formatTime(run.startedAt) }}
      </span>
      <span
        v-if="run.finishedAt"
        class="subagent-drawer__meta-time"
      >
        <Icon name="clock" :size="11" />
        结束 {{ formatTime(run.finishedAt) }}
      </span>
    </div>
    <p
      v-if="run?.summary"
      class="subagent-drawer__summary"
    >{{ run.summary }}</p>
    <span
      v-if="truncated"
      class="subagent-drawer__truncated"
      title="原 transcript 超过 4 MiB,backend 已截断保留 head + tail"
    >
      原 transcript 已截断 (head + tail)
    </span>
  </header>
</template>

<style scoped>
.subagent-drawer__header {
  padding: 12px 16px;
  border-bottom: 1px solid var(--color-bg-border);
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.subagent-drawer__title-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.subagent-drawer__status {
  padding: 2px 8px;
  border: 1px solid;
  border-radius: 999px;
  font-size: 11px;
  font-weight: 600;
  background: color-mix(in srgb, currentColor 10%, transparent);
}

.subagent-drawer__name {
  font-weight: 600;
  font-size: 13px;
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.subagent-drawer__close {
  font: inherit;
  font-family: var(--font-sans);
  display: inline-flex;
  align-items: center;
  background: transparent;
  border: 0;
  color: var(--color-text-muted);
  cursor: pointer;
  padding: 4px;
  border-radius: 4px;
}
.subagent-drawer__close:hover {
  color: var(--color-text-primary);
  background: var(--color-bg-elevated);
}

.subagent-drawer__banner {
  display: flex;
  align-items: flex-start;
  gap: 6px;
  padding: 6px 8px;
  border-radius: 4px;
  border-left: 3px solid currentColor;
  font-family: var(--font-sans);
  font-size: 11px;
  line-height: 1.4;
  background: color-mix(in srgb, currentColor 8%, transparent);
  word-break: break-word;
}
.subagent-drawer__banner--error {
  color: var(--color-tool-error);
}
.subagent-drawer__banner--warning {
  color: var(--color-tool-shell);
}
.subagent-drawer__banner-text {
  flex: 1;
  min-width: 0;
}

.subagent-drawer__meta {
  display: flex;
  gap: 12px;
  font-size: 11px;
  color: var(--color-text-muted);
  font-family: var(--font-mono);
}

.subagent-drawer__meta-time {
  display: inline-flex;
  align-items: center;
  gap: 3px;
}

.subagent-drawer__summary {
  margin: 0;
  font-size: 12px;
  color: var(--color-text-secondary);
  line-height: 1.5;
  max-height: 100px;
  overflow-y: auto;
}

.subagent-drawer__truncated {
  color: var(--color-tool-shell);
  font-size: 10px;
  cursor: help;
  font-family: var(--font-mono);
}
</style>
