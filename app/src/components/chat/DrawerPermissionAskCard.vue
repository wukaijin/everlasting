<script setup lang="ts">
// DrawerPermissionAskCard — drawer-side permission-ask card.
//
// PR5 of the subagent-drawer redesign (2026-06-21). Renders
// `PermissionAskSection` entries as static historical-mode cards.
//
// PR6 (2026-06-21) — R24 DOWNGRADE: historical-mode ONLY.
//
// The PRD's R24 originally specified interactive Allow / Deny buttons
// wired to the `permission:response` IPC. After Explore-agent
// investigation this is NOT possible without backend restructuring:
//
//   - Worker's `PermissionContext.is_worker = true` (see
//     `app/src-tauri/src/agent/permissions/mod.rs:287`). When the
//     worker Tier 4 `ask_path` / `ask_shell` branch fires, the worker
//     context causes an immediate `Decision::Deny` collapse — the
//     worker NEVER emits a `permission:ask` IPC for path / shell
//     tools (`mod.rs:1003-1045`).
//   - For the cases that DO surface here (historical `permission_ask`
//     transcript entries from RULE-A-016 / FT-A-016 PR3a — worker
//     wanted a path/shell tool, the collapse was logged to transcript
//     instead of parent audit), the entries carry synthetic rids
//     (`uuid::Uuid::new_v4()`) that are NOT registered in any
//     `permission_asks: PermissionStore` oneshot map. The
//     `permission_response` IPC handler (`commands/permissions.rs:197-234`)
//     cannot route a response back to the worker.
//   - Worker reuses `parent_session_id` (`agent/subagent.rs:597`) with
//     no independent permission session — an interactive response
//     would have no receiver.
//
// Future implementation path (TODO — tracked in DEBT.md R24):
//   - Give workers an independent permission session (own session_id
//     in the permission store, own oneshot map entries).
//   - Surface worker `permission:ask` events to the frontend with a
//     `workerRunId` field so the drawer can correlate them.
//   - Change the worker Tier 4 `ask_path` / `ask_shell` collapse to
//     actually emit (not deny) when the worker has a UI-side receiver.
//
// Until then, this card is historical-only. The user sees the
// collapsed-ask context but CANNOT approve / deny from the drawer —
// the worker has already auto-denied by the time the card renders.
//
// Why a dedicated wrapper (not just inlining `PermissionAskBody` in
// the drawer):
//   - The card needs the same `.drawer-tool-card` chrome (3px amber
//     left border + header + icon) as `DrawerToolCallCard` so the
//     visual language stays consistent inside the Tools segment.
//   - The historical `PermissionAskBody` is a body-only component
//     (no card chrome) — it expects to be mounted inside a card.
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
import type { PermissionAsk } from "../../stores/permissions";

const props = defineProps<{
  /** Synthesized `PermissionAsk` (camelCase, typed). The drawer's
   *  `synthesizeAsk` helper produces this from the wire
   *  `payload_json` (reading both camelCase and snake_case keys). */
  ask: PermissionAsk;
  /** Repo root for the historical-mode path badge (仓库内 / 仓库外).
   *  Passed through to `PermissionAskBody.repoRoot`. The drawer
   *  sources this from `chatStore.currentCwd` (the worker is
   *  assumed to run under the parent session's cwd). */
  repoRoot: string;
}>();

/** Header name. Prefer the tool name; fall back to "permission ask"
 *  when the synthesized ask is missing `toolName` (defensive against
 *  malformed payload_json). */
const headerName = computed<string>(() => props.ask.toolName || "permission ask");
</script>

<template>
  <div
    class="drawer-permission-ask-card"
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
        <span>worker · 自动拒绝</span>
      </div>
    </div>
    <PermissionAskBody
      mode="historical"
      :ask="ask"
      :repo-root="repoRoot"
    />
    <!-- PR6 R24 downgrade notice: make it visually explicit that this
         card is historical-only. The worker context auto-denies the
         ask at the permission layer (RULE-A-016 / FT-A-016 PR3a
         collapse); the user cannot approve / deny from the drawer.
         See file header for the full R24 downgrade rationale + the
         TODO for the future interactive path. -->
    <p class="drawer-permission-ask-card__auto-denied-note">
      worker context · ask collapsed · 自动拒绝（不可交互）
    </p>
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

/* PR6 R24: explicit "auto-denied · not interactive" notice below the
   historical body. Amber tint to signal the worker-context collapse
   (matches the card's left-border accent). */
.drawer-permission-ask-card__auto-denied-note {
  margin: 6px 0 0 0;
  padding: 4px 8px;
  border-radius: 4px;
  background: color-mix(in srgb, var(--color-tool-shell) 10%, transparent);
  color: var(--color-tool-shell);
  font-family: var(--font-sans);
  font-size: 10px;
  line-height: 1.4;
  font-style: italic;
}
</style>
