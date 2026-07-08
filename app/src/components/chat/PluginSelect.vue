<script setup lang="ts">
// PluginSelect — W1 (Workflow integration, Step 2.2 —
// 2026-07-08): per-session active workflow plugin chip.
// Sibling to `<WorkflowToggle>` on the chat input row;
// click opens a popover listing the discovered plugins
// under `<project>/.everlasting/workflow/`.
//
// **Visual states**:
// - workflow OFF → chip hidden (the chip is meaningless
//   without a workflow session; mirrors the
//   `PluginSelect` chip being conditional on the parent
//   `workflowEnabled` flag, same as the agent toggle's
//   `hasSession` gate).
// - workflow ON, no popover open → ghost-style chip
//   showing the current plugin name (e.g. `Wf: dev`).
//   Same chip shape as `<ModeSelect>` for sibling
//   readability; only the leading label differs (`Wf`
//   prefix instead of empty).
// - workflow ON, popover open → accent-tinted chip +
//   popover listing every discovered plugin; the active
//   one is checked. Click an entry → IPC + chip label
//   flips → popover closes.
//
// **Why a chip + popover (and not a direct cycle like
// Shift+Tab)**: the plugin list is open-ended
// (`list_workflow_plugins` discovers whatever's on disk)
// and we want plugin authors to be able to add a plugin
// without UI changes. A popover scales; a cycle doesn't.
//
// **Streaming / mid-turn semantics**: matches
// `WorkflowToggle`'s contract — the flip applies on the
// next turn boundary (the next `build_workflow_ctx` call
// reads the persisted name from `SessionRow.plugin_name`).
// The chip label + popover state flip immediately for
// snappy feedback.
//
// **Discovery caching**: `listWorkflowPlugins` is
// fire-and-forget on popover open (no global cache here
// — the call is cheap, the popover is rare). The IPC
// reads `<project>/.everlasting/workflow/<dir>/workflow.json`
// for each subdirectory, returning the directory names.

import { computed, onBeforeUnmount, ref } from "vue";

import { useChatStore } from "../../stores/chat";
import Icon from "../Icon.vue";

const chatStore = useChatStore();

/** Same gate as `<WorkflowToggle>` — no point showing the
 *  chip if there's no active session or workflow isn't on. */
const hasSession = computed<boolean>(
  () => !!chatStore.currentSessionId,
);
const workflowEnabled = computed<boolean>(() => {
  const sid = chatStore.currentSessionId;
  if (!sid) return false;
  const s = chatStore.sessions.find((x) => x.id === sid);
  return s?.workflow_enabled ?? false;
});

/** Current plugin name for the active session, or null
 *  when there's no session. */
const currentPlugin = computed<string | null>(() => {
  const sid = chatStore.currentSessionId;
  if (!sid) return null;
  const s = chatStore.sessions.find((x) => x.id === sid);
  return s?.plugin_name ?? null;
});

/** Popover state. Lazy-loaded on first open. */
const open = ref<boolean>(false);
const plugins = ref<string[]>([]);
const loading = ref<boolean>(false);
const rootEl = ref<HTMLElement | null>(null);

/** Project path for the active session. Read from the
 *  `SessionSummary.current_cwd` parent directory — for
 *  the chip's purposes, the project root is "where
 *  `.everlasting/workflow/` would live". When the project
 *  hasn't been set up yet (no session / no project_id),
 *  return empty string → IPC returns `[]` → popover shows
 *  an empty list. */
const projectPath = computed<string>(() => {
  const sid = chatStore.currentSessionId;
  if (!sid) return "";
  const s = chatStore.sessions.find((x) => x.id === sid);
  // current_cwd is the worktree-or-project root; the
  // discovery IPC only needs any path that has
  // `.everlasting/workflow/` somewhere up the tree.
  // The IPC reads the directory verbatim (no parent walk),
  // so we MUST pass the project root, not the worktree
  // sub-path. For now we use current_cwd verbatim; this
  // is fine because the dev plugin (the only one today)
  // lives at the repo root, which is also current_cwd.
  // Phase 3's archive + project-binding work will refine
  // this to read `projects.path` directly.
  return s?.current_cwd ?? "";
});

/** Toggle the popover. Closes on outside-click via the
 *  document listener installed below. */
async function onTriggerClick() {
  if (open.value) {
    open.value = false;
    return;
  }
  open.value = true;
  loading.value = true;
  plugins.value = await chatStore.listWorkflowPlugins(projectPath.value);
  loading.value = false;
}

async function onPick(name: string) {
  const sid = chatStore.currentSessionId;
  if (!sid) return;
  await chatStore.requestSetPluginName(sid, name);
  open.value = false;
}

function onDocClick(ev: MouseEvent) {
  if (!open.value) return;
  const target = ev.target as Node | null;
  if (rootEl.value && target && rootEl.value.contains(target)) {
    return;
  }
  open.value = false;
}

function onEsc(ev: KeyboardEvent) {
  if (ev.key === "Escape") open.value = false;
}

document.addEventListener("click", onDocClick);
document.addEventListener("keydown", onEsc);
onBeforeUnmount(() => {
  document.removeEventListener("click", onDocClick);
  document.removeEventListener("keydown", onEsc);
});

/** Aria label flips based on state. */
const ariaLabel = computed<string>(() =>
  workflowEnabled.value
    ? `Workflow plugin: ${currentPlugin.value ?? "unknown"}. Click to switch.`
    : "Workflow plugin (workflow OFF — toggle workflow to enable plugin switching)",
);

/** Title flips between "currently X" and "click to choose". */
const titleText = computed<string>(() => {
  if (open.value) return "选择一个 workflow plugin";
  return `当前 plugin: ${currentPlugin.value ?? "—"}`;
});
</script>

<template>
  <div
    v-if="hasSession && workflowEnabled"
    ref="rootEl"
    class="plugin-select"
  >
    <button
      type="button"
      class="plugin-select__chip"
      :class="{
        'plugin-select__chip--open': open,
      }"
      :aria-expanded="open"
      :aria-label="ariaLabel"
      :title="titleText"
      @click="onTriggerClick"
    >
      <span class="plugin-select__prefix">Wf</span>
      <span class="plugin-select__sep">·</span>
      <span class="plugin-select__name">{{ currentPlugin ?? "—" }}</span>
      <Icon
        name="chevron-down"
        :size="10"
        class="plugin-select__chevron"
      />
    </button>
    <div
      v-if="open"
      class="plugin-select__popover"
      role="menu"
    >
      <div
        v-if="loading"
        class="plugin-select__empty"
      >
        loading…
      </div>
      <div
        v-else-if="plugins.length === 0"
        class="plugin-select__empty"
      >
        暂无可用 plugin
      </div>
      <button
        v-for="name in plugins"
        v-else
        :key="name"
        type="button"
        role="menuitem"
        class="plugin-select__item"
        :class="{
          'plugin-select__item--active': name === currentPlugin,
        }"
        @click="onPick(name)"
      >
        <span class="plugin-select__item-name">{{ name }}</span>
        <Icon
          v-if="name === currentPlugin"
          name="check"
          :size="12"
          class="plugin-select__item-check"
        />
      </button>
    </div>
  </div>
</template>

<style scoped>
.plugin-select {
  position: relative;
  display: inline-flex;
}

/* Chip shape mirrors <WorkflowToggle>'s ghost-pill so
   the two chips read as siblings on the input row.
   Same recipe: mono font, radius-md, transparent
   background, slim margin-left to separate from
   WorkflowToggle. */
.plugin-select__chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 3px 8px;
  margin-left: 4px;
  background: transparent;
  border: 1px solid transparent;
  border-radius: var(--radius-md);
  color: var(--color-text-secondary);
  cursor: pointer;
  font: inherit;
  font-family: var(--font-mono);
  font-size: var(--text-base);
  font-weight: var(--weight-medium);
  /* "Wf · dev ▾" max ~80px; cap so the chip doesn't
     asymmetrically stretch the row when a plugin name
     is unusually long (e.g. "experimental-v2"). */
  max-width: 120px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  transition: background var(--duration-fast) var(--ease-out),
              color var(--duration-fast) var(--ease-out),
              border-color var(--duration-fast) var(--ease-out);
}

.plugin-select__chip:hover:not(:disabled) {
  background: var(--color-bg-elevated);
  border-color: var(--color-bg-border);
  color: var(--color-text-primary);
}

.plugin-select__chip--open {
  background: var(--color-bg-elevated);
  border-color: var(--color-accent);
  color: var(--color-accent);
}

.plugin-select__prefix,
.plugin-select__name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* Slim separator between "Wf" prefix and plugin name —
   visual cue that the prefix is metadata, not part of
   the plugin name. */
.plugin-select__sep {
  color: var(--color-text-tertiary);
}

.plugin-select__chevron {
  flex-shrink: 0;
  opacity: 0.7;
}

/* Popover: standard menu styling. Anchored below the
   chip with the same z-index family as ModeSelect's
   popover (which is shared via TriggerMenu but the
   chip is not a Trigger — this is a sibling popover
   model, not the command palette). */
.plugin-select__popover {
  position: absolute;
  top: calc(100% + 4px);
  left: 0;
  z-index: 20;
  min-width: 160px;
  max-width: 240px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-md);
  padding: 4px;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.plugin-select__item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 6px 8px;
  border-radius: var(--radius-sm);
  background: transparent;
  border: none;
  color: var(--color-text-primary);
  font: inherit;
  font-family: var(--font-mono);
  font-size: var(--text-sm);
  cursor: pointer;
  text-align: left;
  transition: background var(--duration-fast) var(--ease-out);
}

.plugin-select__item:hover:not(:disabled) {
  background: var(--color-bg-overlay);
}

.plugin-select__item--active {
  color: var(--color-accent);
}

.plugin-select__item-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.plugin-select__item-check {
  flex-shrink: 0;
}

.plugin-select__empty {
  padding: 6px 8px;
  color: var(--color-text-tertiary);
  font-size: var(--text-sm);
  font-family: var(--font-mono);
}
</style>