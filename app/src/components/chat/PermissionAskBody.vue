<script setup lang="ts">
// PermissionAskBody — shared body component for permission ask UI.
//
// FT-F-001 PR1 (2026-06-20): extracted from `ToolCallCard.vue`
// so the same rendering can be reused by the main chat panel AND
// the future `<SubagentDrawer>` (FT-F-001 stage 2) when it
// routes `permission_ask` transcript entries to typed cards.
//
// Per D1/D2/D3/D4/D6 decisions:
//   - 1 component, explicit `mode` prop (D4 — interactive vs
//     historical)
//   - decoupled data props `{ mode, ask, onRespond? }` (D2/D4)
//   - `interactive` mode fires `onRespond(decision, reason?)`;
//     `historical` mode renders info-only and never calls
//     `onRespond` (D4 — drawer needs no UI buttons)
//   - store dependency: NONE (D3) — parent owns the store and
//     passes `onRespond` callback
//   - scoped CSS using existing `--color-*` tokens (D7)
//
// Visual contract (interactive mode): matches
// `ToolCallCard.vue:486-525` exactly — the 4-action row
// (仅一次 / 始终允许|本次运行始终允许 / 拒绝 / 拒绝并说明), the
// optional feedback textarea, the risk dot, the reason line, the
// path-with-badge row. The 中间 button label forks by
// `ask.workerRunId` since task `06-26-subagent-per-run-grant` Step 2
// (2026-06-26): main-chat → "始终允许" (persists to DB), worker →
// "本次运行始终允许" (persists to per-run memory cache).
//
// Visual contract (historical mode): info-only, no buttons.
// Renders the risk label + tool name + path badge (when
// applicable) + reason (when present). Used by the drawer
// for worker `permission_ask` transcript entries that were
// collapsed on the worker side (RULE-A-016 / FT-A-016 PR3a).

import { computed, ref } from "vue";
import {
  RISK_LABEL_CN,
  RISK_META,
  type PermissionAsk,
  type PermissionDecision,
} from "../../stores/permissions";
import { isPathInRoot } from "../../utils/path";

const props = withDefaults(
  defineProps<{
    mode: "interactive" | "historical";
    ask: PermissionAsk;
    /** Required when mode === "interactive" (TS will surface the
     *  "missing onRespond" via the parent's call site — we don't
     *  enforce at the type level here because Vue 3 withDefaults
     *  does not support per-variant required-prop narrowing).
     *  Ignored when mode === "historical". */
    onRespond?: (decision: PermissionDecision, reason?: string) => void;
    /** Used by the path-range badge in BOTH modes (interactive and
     *  historical). The parent (`ToolCallCard.vue`) passes
     *  `chatStore.currentCwd` — which IS the asking session's cwd
     *  because this card renders the current session — matching
     *  the cross-session cwd mix-up fix from 2026-06-16. */
    repoRoot?: string;
    /** When `true`, the "始终允许" (allow_always) button is NOT
     *  rendered in interactive mode.
     *
     *  History:
     *    - Pre-2026-06-26 (PR2 RULE-FrontSubagent-003, 2026-06-22):
     *      the worker-ask path (`DrawerPermissionAskCard`) set this
     *      to `true` (derived from `ask.workerRunId`) because the
     *      backend worker `AllowAlways` arm silently downgraded to
     *      `AllowOnce` (workers do NOT persist grants to
     *      `session_tool_permissions` — would cross privilege
     *      boundaries). Showing a "persist" button that didn't
     *      actually persist was misleading UX, so the worker-ask
     *      card hid it.
     *    - 2026-06-26 (task `06-26-subagent-per-run-grant` Step 2):
     *      the backend now persists worker `AllowAlways` to a per-run
     *      in-memory grant cache (`RunGrantCache`), so the button is
     *      meaningful again (scope = "本次运行"). The default
     *      (`false`) now applies to BOTH main-chat and worker paths;
     *      the label forks by `ask.workerRunId` (主对话 "始终允许"
     *      / worker "本次运行始终允许"). The prop is retained for
     *      callers that need to hide the button explicitly
     *      (defensive / future use). */
    hideAllowAlways?: boolean;
    /** 2026-06-22 (RULE-WorkerAsk-001): the resolve outcome of a
     *  worker's ask. Only rendered in historical mode (interactive
     *  cards are live-pending — no outcome yet). Surfaced by
     *  `pairSections` when it pairs a matching
     *  `PermissionAskResolved` transcript entry by `rid`.
     *  `undefined` → render the neutral ask-context line
     *  (backward compat with pre-this-task transcripts). */
    outcome?: "allow" | "deny" | "timeout" | "cancel";
  }>(),
  {
    onRespond: undefined,
    repoRoot: "",
    hideAllowAlways: false,
    outcome: undefined,
  },
);

const riskMeta = computed(() => RISK_META[props.ask.risk]);

/** 2026-06-26 (task `06-26-subagent-per-run-grant` Step 2): the
 *  "allow_always" button label forks by ask scope.
 *    - Main-chat ask (`ask.workerRunId` absent) → `始终允许`. The
 *      backend persists the grant to `session_tool_permissions`
 *      (survives across requests in the same session).
 *    - Worker ask (`ask.workerRunId` present) → `本次运行始终允许`.
 *      The backend persists the grant to a per-run in-memory cache
 *      (`RunGrantCache`) that dies with the worker run — the label
 *      makes the run-scoped semantics explicit so the user doesn't
 *      confuse it with the main-chat session-level persistence.
 *
 *  The wire is still `"allow_always"`; the backend forks the
 *  persistence target by `is_worker` (parent → DB; worker → run
 *  cache). */
const allowAlwaysLabel = computed<string>(() =>
  props.ask.workerRunId ? "本次运行始终允许" : "始终允许",
);

/** 2026-06-22 (RULE-WorkerAsk-001): outcome metadata for the
 *  historical card's outcome badge. Drives the Chinese label
 *  + the color token used for the badge text + border.
 *
 *  Color token reuse (per design-tokens spec — no one-off
 *  `--color-*` tokens):
 *    - allow  → `--color-tool-write` (emerald — same as
 *               `write_file` success / "仓库内" badge).
 *    - deny   → `--color-tool-error` (red — standard error).
 *    - timeout / cancel → `--color-text-muted` (neutral muted —
 *               these are soft-terminal states the user did not
 *               actively choose; muted avoids implying user
 *               intent).
 */
const OUTCOME_META: Record<
  "allow" | "deny" | "timeout" | "cancel",
  { label: string; color: string }
> = {
  allow: { label: "✓ 已允许", color: "var(--color-tool-write)" },
  deny: { label: "✗ 已拒绝", color: "var(--color-tool-error)" },
  timeout: { label: "⏱ 已超时", color: "var(--color-text-muted)" },
  cancel: { label: "⊘ 已取消", color: "var(--color-text-muted)" },
};

/** Outcome metadata for the current `outcome` prop. `null` when
 *  `outcome` is `undefined` (no resolved entry) — the historical
 *  branch falls back to the neutral ask-context line. */
const outcomeMeta = computed(() =>
  props.outcome ? OUTCOME_META[props.outcome] : null,
);

/** Badge text for the path range row. Mirrors the inline
 *  `pathBadgeText` logic in `ToolCallCard.vue` — `仓库内` if
 *  the ask's path is within `repoRoot`, otherwise `仓库外`.
 *  Empty root → always `仓库外` (defensive). Only rendered
 *  when the ask carries a `path` field (shell / web_fetch
 *  leave it undefined). */
const pathBadgeText = computed<string>(() => {
  const p = props.ask.path;
  if (!p) return "";
  if (!props.repoRoot) return "仓库外";
  return isPathInRoot(p, props.repoRoot) ? "仓库内" : "仓库外";
});

const pathBadgeColor = computed<string>(() =>
  pathBadgeText.value === "仓库内"
    ? "var(--color-tool-write)"
    : "var(--color-tool-shell)",
);

// Interactive-mode-only local state. In historical mode the
// `v-if="mode === 'interactive'"` gates below ensure these
// refs are never read, so the cost is just a couple of unused
// reactive cells.
const showFeedback = ref(false);
const feedback = ref("");

function respond(decision: PermissionDecision): void {
  if (!props.onRespond) return;
  props.onRespond(decision);
}

function submitFeedback(): void {
  if (!props.onRespond) return;
  props.onRespond("deny", feedback.value.trim() || undefined);
  showFeedback.value = false;
  feedback.value = "";
}

function cancelFeedback(): void {
  showFeedback.value = false;
  feedback.value = "";
}
</script>

<template>
  <div
    class="permission-ask-body"
    :class="`permission-ask-body--${mode}`"
  >
    <div class="permission-ask-body__head">
      <span
        class="permission-ask-body__dot"
        :style="{ background: riskMeta.iconColor }"
      ></span>
      <span class="permission-ask-body__title">
        {{ mode === "interactive" ? "需要权限" : "权限询问" }}
      </span>
      <span class="permission-ask-body__risk">
        风险: {{ RISK_LABEL_CN[ask.risk] }}
      </span>
    </div>
    <p v-if="ask.reason" class="permission-ask-body__reason">{{ ask.reason }}</p>
    <div v-if="ask.path" class="permission-ask-body__path">
      <code>{{ ask.path }}</code>
      <span
        v-if="pathBadgeText"
        class="permission-ask-body__badge"
        :style="{ color: pathBadgeColor, borderColor: pathBadgeColor }"
      >{{ pathBadgeText }}</span>
    </div>

    <!-- Interactive: render 4-action approval UI.
         Only mounts when onRespond is provided AND mode is
         interactive — if onRespond is undefined the component
         silently skips (defensive; matches the historical use
         case semantically). -->
    <template v-if="mode === 'interactive' && onRespond">
      <div v-if="showFeedback" class="permission-ask-body__feedback">
        <textarea
          v-model="feedback"
          class="permission-ask-body__textarea"
          rows="2"
          placeholder="告诉 agent 为什么拒绝 / 该怎么做（可选）"
        ></textarea>
        <div class="permission-ask-body__feedback-actions">
          <button
            type="button"
            class="permission-ask-body__btn permission-ask-body__btn--deny"
            @click="submitFeedback"
          >提交拒绝</button>
          <button
            type="button"
            class="permission-ask-body__btn"
            @click="cancelFeedback"
          >取消</button>
        </div>
      </div>
      <div v-else class="permission-ask-body__actions">
        <button
          type="button"
          class="permission-ask-body__btn permission-ask-body__btn--once"
          @click="respond('allow_once')"
        >仅一次</button>
        <button
          v-if="!hideAllowAlways"
          type="button"
          class="permission-ask-body__btn permission-ask-body__btn--always"
          @click="respond('allow_always')"
        >{{ allowAlwaysLabel }}</button>
        <button
          type="button"
          class="permission-ask-body__btn permission-ask-body__btn--deny"
          @click="respond('deny')"
        >拒绝</button>
        <button
          type="button"
          class="permission-ask-body__btn permission-ask-body__btn--deny"
          @click="showFeedback = true"
        >拒绝并说明</button>
      </div>
    </template>

    <!-- Historical: info-only marker. Renders a single muted line
         showing the ask context. After the 2026-06-22
         RULE-FrontSubagent-003 fix, worker asks are interactive
         while live (the `pending` branch above renders Allow/Deny
         buttons); this `historical` branch only renders once the
         ask is resolved (or for pre-fix transcript entries). No
         buttons.

         2026-06-22 (RULE-WorkerAsk-001): when an `outcome` prop is
         present (the pairing layer found a matching
         `PermissionAskResolved` transcript entry by rid), the
         historical branch renders an outcome badge
         (✓ 已允许 / ✗ 已拒绝 / ⏱ 已超时 / ⊘ 已取消) above the
         ask-context line. When `outcome` is absent (no resolved
         entry — pre-this-task transcript or a rare rid mismatch),
         the branch falls back to the neutral ask-context line
         (backward compat). -->
    <template v-else-if="mode === 'historical'">
      <span
        v-if="outcomeMeta"
        class="permission-ask-body__outcome-badge"
        :style="{ color: outcomeMeta.color, borderColor: outcomeMeta.color }"
      >{{ outcomeMeta.label }}</span>
      <p class="permission-ask-body__historical-note">
        worker asked for {{ ask.toolName || "this tool" }}<span v-if="ask.path"> at {{ ask.path }}</span><span v-if="ask.workerRunId"> · worker</span>
      </p>
    </template>
  </div>
</template>

<style scoped>
.permission-ask-body {
  margin-top: 8px;
  padding: 8px 10px;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.permission-ask-body__head {
  display: flex;
  align-items: center;
  gap: 6px;
  font-family: var(--font-sans);
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
}

.permission-ask-body__dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
}

.permission-ask-body__title {
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
}

.permission-ask-body__risk {
  color: var(--color-text-muted);
}

.permission-ask-body__reason {
  margin: 0;
  font-family: var(--font-sans);
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  line-height: 1.4;
}

.permission-ask-body__path {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}

.permission-ask-body__path code {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--color-text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
  flex: 1;
}

.permission-ask-body__badge {
  flex-shrink: 0;
  padding: 1px 6px;
  border: 1px solid;
  border-radius: 999px;
  font-family: var(--font-sans);
  font-size: 10px;
  line-height: 1.4;
  background: color-mix(in srgb, currentColor 12%, transparent);
}

.permission-ask-body__actions,
.permission-ask-body__feedback-actions {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
}

.permission-ask-body__btn {
  font: inherit;
  font-family: var(--font-sans);
  font-size: var(--text-xs);
  padding: 3px 10px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  border: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
  transition: filter var(--duration-fast) var(--ease-out);
}

.permission-ask-body__btn:hover {
  filter: brightness(1.08);
}

.permission-ask-body__btn--always {
  background: var(--color-accent);
  color: #ffffff;
  border-color: var(--color-accent);
}

.permission-ask-body__btn--deny {
  color: var(--color-tool-error);
  border-color: var(--color-tool-error);
}

.permission-ask-body__textarea {
  width: 100%;
  font: inherit;
  font-family: var(--font-sans);
  font-size: var(--text-xs);
  padding: 4px 6px;
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  background: var(--color-bg-surface);
  color: var(--color-text-primary);
  resize: vertical;
}

.permission-ask-body__historical-note {
  margin: 0;
  font-family: var(--font-sans);
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  font-style: italic;
  line-height: 1.4;
}

/* 2026-06-22 (RULE-WorkerAsk-001): outcome badge for the
 * historical card. Mirrors the `.permission-ask-body__badge`
 * pill style used for the path range badge — same padding,
 * border-radius, background mix — so the visual language stays
 * consistent inside the card. The badge color is bound inline
 * via :style (per-outcome color token); the CSS here only
 * defines the shared chrome. */
.permission-ask-body__outcome-badge {
  align-self: flex-start;
  padding: 1px 6px;
  border: 1px solid;
  border-radius: 999px;
  font-family: var(--font-sans);
  font-size: 10px;
  font-weight: var(--weight-semibold);
  line-height: 1.4;
  background: color-mix(in srgb, currentColor 12%, transparent);
}
</style>
