<script setup lang="ts">
// DrawerPermissionAskCard — drawer-side permission-ask card.
//
// PR2 of RULE-FrontSubagent-003 (2026-06-22). Renders
// `PermissionAskSection` entries as MODE-AWARE cards: interactive
// when the ask is a LIVE pending in the permissions store, historical
// when the ask has been resolved (or arrived only via the transcript
// cache from a previous run).
//
// History:
//   - PR5 of the subagent-drawer redesign (2026-06-21): introduced as
//     historical-only.
//   - PR6 R24 (2026-06-21): DOWNGRADE to historical-only. Three
//     blockers were identified at the time (worker deny-collapse /
//     synthetic rids / parent session reuse) — all resolved by PR1 of
//     RULE-FrontSubagent-003 (2026-06-22).
//   - PR2 of RULE-FrontSubagent-003 (2026-06-22): flipped back to
//     interactive mode for LIVE asks. The `interactive` prop is driven
//     by the parent drawer's reconciliation:
//     `getPendingByRid(ask.rid)` decides buttons vs static body.
//
// Why a dedicated wrapper (not just inlining `PermissionAskBody` in
// the drawer):
//   - The card needs the same `.drawer-tool-card` chrome (3px amber
//     left border + header + icon) as `DrawerToolCallCard` so the
//     visual language stays consistent inside the Tools segment.
//   - `PermissionAskBody` is a body-only component (no card chrome)
//     — it expects to be mounted inside a card.
//   - Keeping the wrapper co-located with `DrawerToolCallCard` /
//     `DrawerThinkingBlock` (sibling files in `components/chat/`)
//     makes the drawer's data → view path easy to audit.
//
// RULE-FrontSubagent-001 (2026-06-25): header markup + CSS 抽到共享
//   `<ToolCallHeader>` —— 本组件传 iconName="shield-check" + suffix=
//   "权限询问" + statusVariant(interactive 时 accent 强调) 复用同一
//   header。card chrome(amber border + 容器)保留本组件;interactive 的
//   accent 边框/底色是 card 容器变体，header status 的 accent 色由
//   ToolCallHeader statusVariant prop 自治。
//
// `synthesizeAsk` lives in the drawer (the parent) and is passed
// down as a typed `PermissionAsk`. The mapping from the wire
// `payload_json` (camelCase per Rust `PermissionAskPayload`'s
// `#[serde(rename_all = "camelCase")]`, with snake_case defensive
// fallback) is documented in the drawer's `synthesizeAsk` docstring.

import { computed } from "vue";
import ToolCallHeader from "./ToolCallHeader.vue";
import PermissionAskBody from "./PermissionAskBody.vue";
import {
  usePermissionsStore,
  type PermissionAsk,
  type PermissionDecision,
} from "../../stores/permissions";

const props = withDefaults(
  defineProps<{
    /** Synthesized `PermissionAsk` (camelCase, typed). The drawer's
     *  `synthesizeAsk` helper produces this from the wire
     *  `payload_json` (reading both camelCase and snake_case keys). */
    ask: PermissionAsk;
    /** Repo root for the path badge (仓库内 / 仓库外). Passed
     *  through to `PermissionAskBody.repoRoot`. The drawer sources
     *  this from `chatStore.currentCwd` (the worker is assumed to
     *  run under the parent session's cwd). */
    repoRoot: string;
    /** When `true`, the card renders interactive Allow / Deny buttons
     *  wired to the `permission:response` IPC. When `false`, the
     *  card renders a static historical body (no buttons).
     *
     *  PR2 (2026-06-22): the drawer's reconciliation passes
     *  `interactive = !!permissionsStore.getPendingByRid(ask.rid)`
     *  — a live pending ask → interactive; a resolved / transcript-
     *  only ask → historical. The default (`false`) keeps the card
     *  safe for callers that don't perform the reconciliation (e.g.
     *  test mounts that only care about historical rendering). */
    interactive?: boolean;
    /** 2026-06-22 (RULE-WorkerAsk-001): the resolve outcome of the
     *  worker's ask, surfaced by `pairSections` when it pairs a
     *  matching `PermissionAskResolved` transcript entry (matched by
     *  `rid`). `undefined` when no matching resolved entry was found.
     *  Passed through to `PermissionAskBody.outcome` for the
     *  historical card's outcome badge render. The interactive mode
     *  ignores this (live cards don't show a resolve badge). */
    outcome?: "allow" | "deny" | "timeout" | "cancel";
  }>(),
  { interactive: false, outcome: undefined },
);

// Wire the IPC respond path. The drawer owns the store dependency
// (per the D3 "body component has no store imports" rule from
// FT-F-001) — but the cleanest wiring is for THIS wrapper to call
// the store directly, because:
//   - The rid is already on `props.ask.rid` (no parent lookup needed).
//   - The main-chat `ToolCallCard` also calls `permissionsStore
//     .respond` directly from the card — symmetric pattern.
const permissionsStore = usePermissionsStore();

/** Header name. Prefer the tool name; fall back to "permission ask"
 *  when the synthesized ask is missing `toolName` (defensive against
 *  malformed payload_json). */
const headerName = computed<string>(() => props.ask.toolName || "permission ask");

/** Status pill text — drives the header right-side label.
 *  - Interactive: "等待审批" (live pending; user can act).
 *  - Historical: "已记录" (resolved or transcript-only; read-only). */
const statusText = computed<string>(() =>
  props.interactive ? "等待审批" : "已记录",
);

/** The `PermissionAskBody` mode to render. `interactive` maps to
 *  the body's `interactive` mode (renders buttons when `onRespond`
 *  is also supplied); `historical` keeps the body info-only. */
const bodyMode = computed<"interactive" | "historical">(() =>
  props.interactive ? "interactive" : "historical",
);

/** Whether to hide the "始终允许" (allow_always) button.
 *
 *  History:
 *    - Pre-2026-06-26 (task `06-26-subagent-per-run-grant`): worker
 *      asks forced this to `true` (derived from `ask.workerRunId`)
 *      because the backend worker `AllowAlways` arm silently
 *      downgraded to `AllowOnce` — workers did NOT persist grants to
 *      `session_tool_permissions` (would cross privilege boundaries
 *      by extending parent-session permissions from a worker), so a
 *      "persist" button that didn't actually persist was misleading
 *      UX.
 *    - 2026-06-26 (task `06-26-subagent-per-run-grant` Step 2): the
 *      backend now persists worker `AllowAlways` to a per-run
 *      in-memory grant cache (`RunGrantCache`, lives in the worker's
 *      `PermissionContext`, dies with the worker run, NEVER writes
 *      `session_tool_permissions`). So a worker CAN now have a
 *      meaningful "持久" option — but its scope is "本次运行" not
 *      "本 session". The button reappears; the label is forked by
 *      `PermissionAskBody` based on `ask.workerRunId` (主对话 →
 *      "始终允许"; worker → "本次运行始终允许"). The wire is still
 *      `"allow_always"`; the backend forks the semantics by
 *      `is_worker` (parent → write DB; worker → write run cache).
 *
 *  Always `false` now — both main-chat and worker asks render the
 *  persist button. The label divergence lives in
 *  `PermissionAskBody.vue`. */
const hideAllowAlways = computed<boolean>(() => false);

/** The `onRespond` callback. Only attached in interactive mode —
 *  `PermissionAskBody` guards its render of the action row on
 *  `mode === "interactive" && onRespond`, so returning `undefined`
 *  in historical mode keeps the body's historical branch active. */
function onRespond(decision: PermissionDecision, reason?: string): void {
  // Fire-and-forget — the IPC resolves the backend oneshot; the
  // store's `respond` clears the local pending slot + timer. Errors
  // are swallowed inside `respond` (logged via console.error).
  void permissionsStore.respond(props.ask.rid, decision, reason);
}
</script>

<template>
  <div
    class="drawer-permission-ask-card"
    :class="{ 'drawer-permission-ask-card--interactive': interactive }"
    :style="{ borderLeftColor: 'var(--color-tool-shell)' }"
  >
    <ToolCallHeader
      icon-name="shield-check"
      :name="headerName"
      suffix="权限询问"
      :status-text="statusText"
      :status-variant="interactive ? 'accent' : 'default'"
    />
    <PermissionAskBody
      :mode="bodyMode"
      :ask="ask"
      :repo-root="repoRoot"
      :hide-allow-always="hideAllowAlways"
      :on-respond="interactive ? onRespond : undefined"
      :outcome="outcome"
    />
  </div>
</template>

<style scoped>
/* Card 容器 chrome。header markup + CSS 已抽到 `<ToolCallHeader>`
   (RULE-FrontSubagent-001, 2026-06-25);本组件保留 card 容器(amber
   border-left 恒表"额外谨慎") + interactive 容器变体(accent border-left +
   底色 tint)。interactive 的 header status accent 色由 ToolCallHeader 的
   statusVariant="accent" prop 自治，不靠 card 后代选择器。0 hex,全 token。
   header 与下方 body 的 4px gap 用 :deep 注入 ToolCallHeader root。 */

.drawer-permission-ask-card {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-left: 3px solid var(--color-tool-shell);
  border-radius: var(--radius-md);
  padding: 8px 12px;
  font-size: var(--text-sm);
  font-family: var(--font-mono);
  color: var(--color-text-primary);
  max-width: 100%;
}

/* Interactive variant — subtle accent shift so the user can spot
   live pending asks at a glance (vs. resolved historical entries
   in the same Tools segment). Tints the border-left with the
   accent color to signal "action required". */
.drawer-permission-ask-card--interactive {
  border-left-color: var(--color-accent);
  background: color-mix(in srgb, var(--color-accent) 4%, var(--color-bg-surface));
}

/* header 与下方 PermissionAskBody 的 4px gap。header 在 ToolCallHeader
   子组件内，用 :deep 跨 scoped 边界注入 margin-bottom(原
   `.drawer-permission-ask-card__header { margin-bottom: 4px }` 迁移)。 */
.drawer-permission-ask-card :deep(.tool-call-header) {
  margin-bottom: 4px;
}
</style>
