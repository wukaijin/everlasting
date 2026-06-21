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
//     blockers were identified at the time:
//       (a) Worker's `PermissionContext.is_worker = true` caused an
//           immediate `Decision::Deny` collapse — the worker NEVER
//           emitted a `permission:ask` IPC for path / shell tools.
//       (b) Historical transcript entries carried synthetic rids
//           (`uuid::Uuid::new_v4()`) NOT registered in the
//           `permission_asks: PermissionStore` oneshot map — the
//           `permission_response` IPC could not route a response.
//       (c) Worker reused `parent_session_id` with no independent
//           permission session — an interactive response would have
//           no receiver.
//   - PR2 of RULE-FrontSubagent-003 (2026-06-22): the PR1 backend
//     restructuring resolves ALL three blockers:
//       (a) Worker now emits REAL `permission:ask` IPC events (the
//           Tier 4 collapse path was changed to emit-then-await
//           instead of deny-immediately when the worker has an
//           independent permission session).
//       (b) Worker asks carry REAL rids (registered in a dedicated
//           `"worker:{workerRunId}"` permission session in the
//           backend's `PermissionStore`); the rid routes correctly
//           via the existing `permission_response` IPC.
//       (c) Worker has an independent permission session, isolated
//           from the parent's slot.
//     This file flips back to interactive mode for LIVE asks. The
//     `interactive` prop is driven by the parent drawer's
//     reconciliation: `getPendingByRid(ask.rid)` decides whether
//     the card renders buttons or a static historical body.
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
// `synthesizeAsk` lives in the drawer (the parent) and is passed
// down as a typed `PermissionAsk`. The mapping from the wire
// `payload_json` (camelCase per Rust `PermissionAskPayload`'s
// `#[serde(rename_all = "camelCase")]`, with snake_case defensive
// fallback) is documented in the drawer's `synthesizeAsk` docstring.

import { computed } from "vue";
import Icon from "../Icon.vue";
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
  }>(),
  { interactive: false },
);

// Wire the IPC respond path. The drawer owns the store dependency
// (per the D3 "body component has no store imports" rule from
// FT-F-001) — but the cleanest wiring is for THIS wrapper to call
// the store directly, because:
//   - The rid is already on `props.ask.rid` (no parent lookup needed).
//   - The main-chat `ToolCallCard` also calls `permissionsStore
//     .respond` directly from the card (see `ToolCallCard.vue`'s
//     approval buttons) — symmetric pattern.
// Emitting to the parent would just forward the call one level up
// without adding any value.
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

/** Hide the "始终允许" (allow_always) button when this card is for
 *  a WORKER ask. The backend worker path (`permissions/mod.rs
 *  ::ask_path` AllowAlways arm) treats `PermissionResponse
 *  ::AllowAlways` as AllowOnce — workers do NOT persist grants to
 *  `session_tool_permissions` (would cross privilege boundaries
 *  by extending parent-session permissions from a worker). So
 *  showing a "persist" button that doesn't actually persist is
 *  misleading UX. The main-chat ToolCallCard path does NOT set
 *  this and keeps all 4 buttons. Derived from `ask.workerRunId`
 *  (present = worker ask; absent = main-chat ask). */
const hideAllowAlways = computed<boolean>(() => !!props.ask.workerRunId);

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
    <div class="drawer-permission-ask-card__header">
      <div class="drawer-permission-ask-card__title">
        <span class="drawer-permission-ask-card__icon">
          <Icon name="shield-check" :size="14" />
        </span>
        <span class="drawer-permission-ask-card__name">{{ headerName }}</span>
        <span class="drawer-permission-ask-card__suffix">权限询问</span>
      </div>
      <div class="drawer-permission-ask-card__status">
        <span>{{ statusText }}</span>
      </div>
    </div>
    <PermissionAskBody
      :mode="bodyMode"
      :ask="ask"
      :repo-root="repoRoot"
      :hide-allow-always="hideAllowAlways"
      :on-respond="interactive ? onRespond : undefined"
    />
  </div>
</template>

<style scoped>
/* Mirrors `DrawerToolCallCard.vue`'s `.drawer-tool-card*` rules 1:1
   (same tokens, same box model). The class name is distinct
   (`.drawer-permission-ask-card*`) to avoid scoped-CSS collisions
   and to signal the card variant (amber left border regardless of
   tool name — permission asks always read as "extra caution"). */

.drawer-permission-ask-card {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-left: 3px solid var(--color-tool-shell);
  border-radius: 6px;
  padding: 8px 12px;
  font-size: 12px;
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

.drawer-permission-ask-card__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  min-width: 0;
  margin-bottom: 4px;
}

.drawer-permission-ask-card__title {
  display: inline-flex;
  align-items: baseline;
  gap: 6px;
  min-width: 0;
  flex: 1;
  overflow: hidden;
  white-space: nowrap;
}

.drawer-permission-ask-card__icon {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  color: var(--color-tool-shell);
}

.drawer-permission-ask-card__name {
  font-weight: 600;
  color: var(--color-text-primary);
}

.drawer-permission-ask-card__suffix {
  color: var(--color-text-muted);
  font-size: 11px;
}

.drawer-permission-ask-card__status {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 11px;
  color: var(--color-text-muted);
  flex-shrink: 0;
}

/* Interactive status pill gets the accent color so the user's eye
   is drawn to "waiting on you" cards. Historical status stays muted. */
.drawer-permission-ask-card--interactive .drawer-permission-ask-card__status {
  color: var(--color-accent);
  font-weight: 600;
}
</style>
