<script setup lang="ts">
// PluginSelect — W1 (Workflow integration, Step 2.2 +
// 2026-07-09 chip-merge): per-session workflow + active
// plugin merged into ONE chip + popover, replacing the
// former `WorkflowToggle` + `PluginSelect` two-chip layout
// (task 07-09-07-09-workflow-chip-merge).
//
// **What it replaces**:
// - `WorkflowToggle.vue` (deleted in this task) — was the
//   bare `Wf` / `Wf ●` chip, click flipped workflow on/off.
// - The previous `PluginSelect.vue` — was the
//   `Wf · <plugin> ▾` chip + plugin-list popover, gated
//   behind `workflowEnabled` so it disappeared when
//   workflow was off.
//
// **Visual states**:
// - workflow OFF → ghost chip `Wf ▾` (mirrors the old
//   `WorkflowToggle`'s OFF state, but with a chevron so
//   the user can see it's a dropdown). The popover's
//   plugin list renders in a disabled row group below the
//   toggle so the user can SEE what plugins exist before
//   committing to enable.
// - workflow ON  → accent chip `Wf · <plugin> ▾` (matches
//   the old `WorkflowToggle`'s ON accent + the old
//   `PluginSelect`'s label). The popover's plugin list is
//   fully active and clickable.
//
// **Popover shape**:
// ┌────────────────────────────────┐
// │ ● Workflow              ON >  │  ← top toggle row
// ├────────────────────────────────┤
// │ dev                       ✓   │  ← plugin list (greyed
// │ experimental                  │     when OFF)
// └────────────────────────────────┘
//
// **Why keep the plugin list visible when OFF** (vs hide
// it): the list is the onboarding signal — first-time
// users see "ah, `dev` is available" before they commit.
// Hiding it would force an extra click for the same info.
// The "disabled" treatment is unambiguous (reduced opacity
// + `not-allowed` cursor + ignore clicks).
//
// **Streaming / mid-turn semantics**: matches both former
// components' contract — both flags flip in the DB
// immediately (optimistic) but the agent's
// `build_workflow_ctx` reads the persisted flags on the
// NEXT turn boundary, so mid-stream flips don't surprise
// the user with an instant state-machine change.
//
// **Open direction**: UPWARD (`bottom: calc(100% + 4px);
// top: auto;`). The chat-input row sits at the bottom of
// the chat panel (above the viewport bottom), so a
// downward popover would overlap the textarea below the
// chips and feel "trapped under the row". Upward matches
// `<ModeSelect>` and `<ModelSelect>` (also in the same
// input row, see `popover-pattern.md` §"Position
// Direction Rule"), giving the user one consistent
// "popovers open up from this row" mental model.

import { computed, onBeforeUnmount, onUnmounted, ref } from "vue";

import { useChatStore } from "../../stores/chat";
import Icon from "../Icon.vue";

const chatStore = useChatStore();

/** Same `hasSession` gate as the former `WorkflowToggle`
 *  — no point showing the chip without an active session
 *  to bind the workflow flag to. Mirrors `<ModeSelect>`. */
const hasSession = computed<boolean>(() => !!chatStore.currentSessionId);

/** Workflow opt-in for the active session. Defensive
 *  default `false` when `hasSession` is false or the
 *  summary row is missing. */
const workflowEnabled = computed<boolean>(() => {
  const sid = chatStore.currentSessionId;
  if (!sid) return false;
  const s = chatStore.sessions.find((x) => x.id === sid);
  return s?.workflow_enabled ?? false;
});

/** Active plugin name for the current session. May be
 *  `null` when workflow has never been turned ON for
 *  this session (the backend's `plugin_name` column
 *  defaults to NULL until the first set call); the chip
 *  renders `Wf · — ▾` in that case. */
const currentPlugin = computed<string | null>(() => {
  const sid = chatStore.currentSessionId;
  if (!sid) return null;
  const s = chatStore.sessions.find((x) => x.id === sid);
  return s?.plugin_name ?? null;
});

/** Project root for plugin discovery. Mirrors the prior
 *  `PluginSelect.projectPath` semantics verbatim — reads
 *  `current_cwd` directly. The IPC reads
 *  `.everlasting/workflow/<dir>/workflow.json` under this
 *  path verbatim (no parent walk). For the dev plugin
 *  (the only shipped one) this is fine because it lives
 *  at the repo root = `current_cwd`. Project-binding work
 *  (Phase 3 archive) will refine this to read
 *  `projects.path` directly. */
const projectPath = computed<string>(() => {
  const sid = chatStore.currentSessionId;
  if (!sid) return "";
  const s = chatStore.sessions.find((x) => x.id === sid);
  return s?.current_cwd ?? "";
});

/** Popover state. Lazy-loaded: `plugins` is fetched on
 *  first open via `listWorkflowPlugins`. Subsequent opens
 *  reuse the cached list — it only changes when the user
 *  adds a new plugin directory on disk, which is rare
 *  enough that a watcher isn't worth it. */
const open = ref<boolean>(false);
const plugins = ref<string[]>([]);
const loaded = ref<boolean>(false);
const loading = ref<boolean>(false);
const rootEl = ref<HTMLElement | null>(null);

/** Toggle the popover. Closes on outside-click / Esc via
 *  the document + window listeners below. */
async function onTriggerClick() {
  if (open.value) {
    open.value = false;
    return;
  }
  open.value = true;
  if (!loaded.value) {
    loading.value = true;
    plugins.value = await chatStore.listWorkflowPlugins(projectPath.value);
    loaded.value = true;
    loading.value = false;
  }
}

/** Toggle row click → flip workflow_enabled. Does NOT
 *  close the popover — the user is interacting with the
 *  popover and may want to immediately pick a plugin
 *  after enabling. This mirrors the prior `WorkflowToggle`
 *  behavior (click did nothing else). */
async function onToggleWorkflow() {
  const sid = chatStore.currentSessionId;
  if (!sid) return;
  await chatStore.requestSetWorkflowEnabled(sid, !workflowEnabled.value);
}

/** Plugin row click → set plugin_name + close popover.
 *  Only callable when `workflowEnabled === true` (the
 *  row is rendered as a non-clickable display when OFF,
 *  see template). */
async function onPick(name: string) {
  if (!workflowEnabled.value) return;
  const sid = chatStore.currentSessionId;
  if (!sid) return;
  await chatStore.requestSetPluginName(sid, name);
  open.value = false;
}

function onDocClick(ev: MouseEvent) {
  if (!open.value) return;
  const target = ev.target as Node | null;
  if (rootEl.value && target && !rootEl.value.contains(target)) {
    open.value = false;
  }
}

function onEsc(ev: KeyboardEvent) {
  if (open.value && ev.key === "Escape") {
    open.value = false;
  }
}

if (typeof document !== "undefined") {
  document.addEventListener("click", onDocClick);
}
if (typeof window !== "undefined") {
  window.addEventListener("keydown", onEsc);
}
onBeforeUnmount(() => {
  if (typeof document !== "undefined") {
    document.removeEventListener("click", onDocClick);
  }
});
onUnmounted(() => {
  if (typeof window !== "undefined") {
    window.removeEventListener("keydown", onEsc);
  }
});

/** Aria label combines workflow state + current plugin so
 *  screen reader users hear the full picture, not just
 *  "Workflow chip". */
const ariaLabel = computed<string>(() => {
  const state = workflowEnabled.value ? "ON" : "OFF";
  const plugin = currentPlugin.value ?? "none";
  return `Workflow ${state}, plugin ${plugin}. Click to open menu.`;
});

const titleText = computed<string>(() =>
  open.value
    ? "Workflow 菜单"
    : workflowEnabled.value
      ? `Workflow ON · 当前 plugin: ${currentPlugin.value ?? "—"}`
      : "Workflow OFF · 点开选择 plugin",
);
</script>

<template>
  <div
    v-if="hasSession"
    ref="rootEl"
    class="plugin-select"
  >
    <button
      type="button"
      class="plugin-select__chip"
      :class="{
        'plugin-select__chip--on': workflowEnabled,
        'plugin-select__chip--open': open,
      }"
      :aria-haspopup="'menu'"
      :aria-expanded="open"
      :aria-label="ariaLabel"
      :title="titleText"
      @click="onTriggerClick"
    >
      <!-- Brand prefix — same `Wf` mnemonic the deleted
           WorkflowToggle used, so muscle memory carries
           over. When ON, an inline bolt icon (mirrors the
           deleted toggle's ON state dot) reinforces the
           "workflow is engaged" cue without taking extra
           horizontal space. -->
      <Icon
        v-if="workflowEnabled"
        name="bolt"
        :size="12"
        class="plugin-select__icon"
      />
      <span class="plugin-select__prefix">Wf</span>
      <template v-if="workflowEnabled">
        <span class="plugin-select__sep">·</span>
        <span class="plugin-select__name">{{ currentPlugin ?? "—" }}</span>
      </template>
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
      <!-- Top row: workflow master toggle. Rendered as a
           role="switch" button (per WAI-ARIA switch pattern)
           with aria-checked so screen readers announce the
           state. The right-side pill is a visual indicator
           only — clicks bubble to the button. -->
      <button
        type="button"
        role="switch"
        :aria-checked="workflowEnabled"
        class="plugin-select__toggle-row"
        @click="onToggleWorkflow"
      >
        <span class="plugin-select__toggle-label">Workflow</span>
        <span
          class="plugin-select__toggle-pill"
          :class="{
            'plugin-select__toggle-pill--on': workflowEnabled,
          }"
          aria-hidden="true"
        >
          <span class="plugin-select__toggle-knob" />
        </span>
      </button>

      <div
        class="plugin-select__divider"
        aria-hidden="true"
      />

      <!-- Plugin list. When workflow is OFF, the entire
           group is visually dimmed + cursor-not-allowed +
           non-clickable. The `disabled` group semantics
           mean click handlers short-circuit (see
           `onPick`'s `workflowEnabled` guard). -->
      <div
        class="plugin-select__plugin-group"
        :class="{
          'plugin-select__plugin-group--disabled': !workflowEnabled,
        }"
        :aria-disabled="!workflowEnabled"
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
          :disabled="!workflowEnabled"
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
  </div>
</template>

<style scoped>
.plugin-select {
  position: relative;
  display: inline-flex;
}

/* Chip shape unifies the old WorkflowToggle + PluginSelect
   chips into one. Same recipe (mono font, radius-md,
   transparent → bg-elevated on hover, slim margin-left to
   separate from ModeSelect). Three modifier classes:
   - default  : OFF, ghost (text-secondary)
   - --on     : workflow ON, accent border + text
   - --open   : popover open, accent border + bg-elevated
                (overrides --on when both apply — same as
                the prior PluginSelect)

   08-24 btn-family 归类:chip / toggle-row / item 均为 chat-input
   chip 族(ModeSelect / ModelSelect 同款)的下拉 trigger 与 popover
   菜单行 —— radius-md + mono + --on/--open 选中态无家族变体对应,
   且三件套需同步迁移,本文件特例保留本地。 */
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
  /* OFF: "Wf ▾" ~38px; ON: "Wf · dev ▾" ~80px; cap at
     120px (matches the prior PluginSelect cap) so long
     plugin names don't asymmetrically stretch the row. */
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

.plugin-select__chip--on {
  color: var(--color-accent-text);
  border-color: var(--color-accent);
}

.plugin-select__chip--on:hover:not(:disabled) {
  /* Keep accent border on hover — losing it on hover
     would make the active state look unstable (same
     reasoning as the deleted WorkflowToggle's --on:hover
     rule). */
  background: var(--color-bg-elevated);
  color: var(--color-accent-text);
}

.plugin-select__chip--open {
  background: var(--color-bg-elevated);
  border-color: var(--color-accent);
  color: var(--color-accent-text);
}

.plugin-select__prefix,
.plugin-select__name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.plugin-select__sep {
  color: var(--color-text-tertiary);
}

.plugin-select__icon {
  flex-shrink: 0;
}

.plugin-select__chevron {
  flex-shrink: 0;
  opacity: 0.7;
}

/* Popover — opens UPWARD from the chip. Per
   `popover-pattern.md` §"Position Direction Rule":
   triggers at the bottom of the viewport open upward.
   ChatInput's chip row sits at the bottom of the chat
   panel, matching ModeSelect / ModelSelect's geometry
   — same `bottom: calc(100% + 4px); top: auto;` recipe
   so the three popovers read as one family. */
.plugin-select__popover {
  position: absolute;
  bottom: calc(100% + 4px);
  top: auto;
  left: 0;
  /* 局部层:头部下拉家族(Mode/ModelSelect 同款几何),低于抽屉/遮罩带 */
  z-index: 20;
  min-width: 220px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-md);
  padding: 4px;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

/* Toggle row (top of popover). Full-width button
   styled as a row, with a switch-pill on the right.
   Uses role="switch" + aria-checked for screen-reader
   semantics. */
.plugin-select__toggle-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
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
  width: 100%;
  transition: background var(--duration-fast) var(--ease-out);
}

.plugin-select__toggle-row:hover:not(:disabled) {
  /* 原 var(--color-bg-overlay) 为失效 token(全库无定义,规则实际
     不生效)。popover 底是 elevated,hover 需抬一层:用全局 hover wash
     token(任何底色上都可见,与 .btn--ghost hover 同源)。 */
  background: var(--color-bg-hover);
}

.plugin-select__toggle-label {
  flex: 1;
}

.plugin-select__toggle-pill {
  display: inline-flex;
  align-items: center;
  width: 28px;
  height: 16px;
  border-radius: 999px;
  border: 1px solid var(--color-text-secondary);
  background: transparent;
  padding: 0 1px;
  transition: background var(--duration-fast) var(--ease-out),
              border-color var(--duration-fast) var(--ease-out);
  flex-shrink: 0;
}

.plugin-select__toggle-pill--on {
  background: var(--color-accent);
  border-color: var(--color-accent);
  /* Justify-content flips when ON so the knob hugs the
     right edge; otherwise it sits left. */
  justify-content: flex-end;
}

.plugin-select__toggle-knob {
  display: inline-block;
  width: 10px;
  height: 10px;
  border-radius: 50%;
  background: var(--color-text-secondary);
  transition: background var(--duration-fast) var(--ease-out);
}

.plugin-select__toggle-pill--on .plugin-select__toggle-knob {
  background: var(--color-bg-elevated);
}

/* Slim divider between toggle row and plugin list.
   Inset 8px on each side to align with the row content. */
.plugin-select__divider {
  height: 1px;
  margin: 4px 8px;
  background: var(--color-bg-border);
}

/* Plugin group — wrapping div instead of a raw fragment
   so the disabled state can style the whole group at once
   (opacity + cursor). The individual `.plugin-select__item`
   buttons inside use `:disabled` for the actual click
   block (button[disabled] doesn't fire click events). */
.plugin-select__plugin-group {
  display: flex;
  flex-direction: column;
  gap: 2px;
  transition: opacity var(--duration-fast) var(--ease-out);
}

.plugin-select__plugin-group--disabled {
  opacity: 0.5;
  cursor: not-allowed;
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
  /* 同上:失效 --color-bg-overlay → 全局 hover wash。 */
  background: var(--color-bg-hover);
}

.plugin-select__item:disabled {
  cursor: not-allowed;
}

.plugin-select__item--active {
  color: var(--color-accent-text);
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