<script setup lang="ts">
// SessionGroupHeader — collapsible group label for the sidebar's
// session list. Pure presentation: the parent owns the
// collapsed state and passes it down via `collapsed`; clicking
// the header emits `toggle`. Used by `SessionList.vue` to render
// 今天 / 昨天 / 本周 / 更早 buckets with one header per non-empty
// group.
//
// 2026-06-27 sidebar 分组 (PR-of-PRs, 3 features): the sidebar
// was previously a flat list of N sessions capped at 8 visible
// with a "查看更早的 N 个" disclosure. With grouping, the
// sidebar shows four labeled sections and each section is
// individually collapsible. Today is expanded by default so the
// most-recent work is always visible; other groups are collapsed
// by default so they don't dominate the view.

import Icon from "./Icon.vue";

defineProps<{
  /** Chinese group label (e.g. "今天", "昨天", "本周", "更早"). */
  label: string;
  /** Number of sessions in the group; rendered after the label
   *  in muted mono text. */
  count: number;
  /** True when the group is collapsed (items hidden). Drives
   *  the chevron rotation and ARIA state. */
  collapsed: boolean;
}>();

defineEmits<{
  /** Emitted on click of the header or chevron. Parent flips
   *  the collapsed state. */
  (e: "toggle"): void;
}>();
</script>

<template>
  <li
    :class="[
      'session-group-header',
      { 'session-group-header--collapsed': collapsed },
    ]"
    role="button"
    tabindex="0"
    :aria-expanded="!collapsed"
    @click="$emit('toggle')"
    @keydown.enter.prevent="$emit('toggle')"
    @keydown.space.prevent="$emit('toggle')"
  >
    <Icon
      :name="collapsed ? 'chevron-right' : 'chevron-down'"
      :size="10"
      class="session-group-header__chevron"
    />
    <span class="session-group-header__label">{{ label }}</span>
    <span class="session-group-header__count">{{ count }}</span>
  </li>
</template>

<style scoped>
/* 2026-06-27 sidebar 分组: collapsible section header. The row
   mimics the existing `.session-item` left padding (10px) so
   the chevron aligns with the session item's title baseline.
   Hover lifts the chevron + label color so the user sees the
   row is interactive (same feedback vocabulary as
   `.session-item:hover`). */
.session-group-header {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 10px 4px 10px;
  margin-top: 4px;
  cursor: pointer;
  border-radius: var(--radius-sm);
  list-style: none;
  user-select: none;
  transition: background var(--duration-fast) var(--ease-out);
}

.session-group-header:hover {
  background: var(--color-bg-hover);
}

.session-group-header__chevron {
  flex-shrink: 0;
  color: var(--color-text-muted);
}

.session-group-header__label {
  font-size: var(--text-2xs);
  font-weight: var(--weight-semibold);
  color: var(--color-text-muted);
  text-transform: uppercase;
  letter-spacing: 0.05em;
  font-family: var(--font-mono);
  flex: 1;
  min-width: 0;
}

.session-group-header__count {
  flex-shrink: 0;
  font-size: var(--text-2xs);
  font-family: var(--font-mono);
  color: var(--color-text-muted);
  font-variant-numeric: tabular-nums;
}

/* Collapsed: dim the label slightly so the user can scan which
   groups are open at a glance. Count stays full-strength so
   the user always sees "12 sessions in 更早, click to expand". */
.session-group-header--collapsed .session-group-header__label {
  color: var(--color-text-muted);
  opacity: 0.75;
}
</style>
