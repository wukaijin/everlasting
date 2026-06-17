<script setup lang="ts">
// TriggerMenu — generic prefix-triggered autocomplete panel.
//
// **Why this component exists (B3 /command)**: Claude-Code-style slash
// commands need an autocomplete panel that opens when the user types a
// trigger character (`/`) at the start of the current input line. The
// PRD (`.trellis/tasks/06-16-b3-command-palette/prd.md`) deliberately
// builds the panel as a **reusable skeleton** so the follow-up tasks
// B2 (@file completion) and B4 (skill picker) can share it.
//
// **Design choices**:
// - **Hand-rolled popover, not reka-ui**. Mirrors `ModeSelect.vue` /
//   `ModelSelect.vue` per `.trellis/spec/frontend/popover-pattern.md`
//   (onDocumentClick close + Esc close + upward geometry + 150ms /
//   100ms fade+slide animation). reka-ui's `Popover` would render with
//   different chrome and fork the popover implementations (PR5 D3).
// - **Upward geometry**. The trigger is the chat input at the bottom of
//   the panel; opening downward would clip below the viewport. Same
//   `bottom: calc(100% + 4px); top: auto;` + `translateY(4px → 0)`
//   slide as `ModeSelect` / `ModelSelect`.
// - **Prefix filter by default; opt-in fuzzy**. B3 (/command, ~10
//   items) uses the simple `name.startsWith(filter)` branch. B2
//   (@file, thousands of paths) passes `fuzzy` → fuzzysort
//   (substring-aware, score-sorted) so `appcp` matches
//   `app/src/components/chat/ChatPanel.vue`.
// - **snake_case wire DTO**. `CommandInfo` fields mirror the Rust
//   struct verbatim per BACKLOG §5.2 (`argument_hint` etc.). The
//   Tauri command ARG is camelCase per HACKING-wsl FU-4.
//
// **Extensibility for B2 / B4**:
//   - `trigger` prop (default `"/"`) lets a future consumer open on
//     `@` (B2 file completion) or another token.
//   - `items` prop + `@select` event lets the data source be swapped
//     (B3 sources from `list_commands`; B2 will source from a file
//     walker). The component itself is data-source-agnostic — it only
//     renders rows + drives keyboard navigation.
//   - `#row` slot lets each consumer render its own row chrome (icon,
//     description, hint chip) without forking the panel shell.
//
// **Lifecycle**: the parent (`ChatInput.vue`) decides WHEN to open the
// panel (line-head `/` detection + IME gate) and WHAT to do on select
// (clear the prefix, dispatch builtin vs. custom). This component only
// owns the panel DOM + keyboard navigation + click-outside/Esc close.

import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import fuzzysort from "fuzzysort";

import Icon from "../Icon.vue";

/** Max rows the fuzzy matcher surfaces. B2 passes thousands of file
 *  paths; capping the panel keeps the render + scroll cheap. B3
 *  (/command, ~10 items) never hits this. */
const FUZZY_LIMIT = 50;

/** A single row in the panel. `CommandInfo` from the Rust
 *  `resource_loader::CommandInfo` (BACKLOG §5.2: TS interface mirrors
 *  the Rust struct field names verbatim — snake_case, NOT camelCase).
 *  B2 (@file) will reuse this shape with `source: "file"` etc. */
export interface TriggerMenuItem {
  /** Stable key for `v-for`. Typically the command name (B3) or the
   *  absolute file path (B2). */
  key: string;
  /** Primary label rendered in monospace (B3: command name like
   *  `clear`; B2: relative file path). */
  name: string;
  /** Secondary line under the name. B3: command description. May be
   *  empty for files. */
  description?: string;
  /** Optional hint chip on the right (B3: `[msg]` argument hint;
   *  empty for builtins). */
  argument_hint?: string;
  /** Source badge text rendered as a small uppercase chip on the
   *  right (B3: `builtin` / `user` / `project`). */
  source?: string;
  /** B3-specific: builtins dispatch to frontend actions; custom
   *  commands will expand their body in PR3. The parent reads this
   *  to decide which dispatch path to take. */
  is_builtin?: boolean;
}

const props = withDefaults(
  defineProps<{
    /** Two-way bound open state. Parent opens via `v-model:open`. */
    open: boolean;
    /** Items to render. The parent sources these (B3: from
     *  `list_commands`; B2 will source from a file walker). */
    items: TriggerMenuItem[];
    /** Current filter text the user typed AFTER the trigger char.
     *  The parent (ChatInput) computes this from its textarea and
     *  passes it down; the panel does prefix matching locally so
     *  the parent doesn't have to re-filter on every keystroke. */
    filter: string;
    /** Trigger character this panel was opened for. Cosmetic —
     *  shown in the header row so the user knows what they're
     *  completing. B3 passes `"/"`; B2 will pass `"@"`. */
    trigger?: string;
    /** Header label. B3: `"命令"`; B2: `"文件"`. */
    headerLabel?: string;
    /** Empty-state message when no item matches the filter. */
    emptyLabel?: string;
    /** Optional reference to the trigger element (e.g. the
     *  ChatInput textarea). When provided, clicks INSIDE this
     *  element are treated as "inside the popover" for the
     *  outside-click close check — i.e. they do NOT close the
     *  panel. This is necessary because the TriggerMenu's own
     *  `root` only wraps the panel DOM; the real trigger (the
     *  textarea) is a sibling owned by the parent, so without
     *  this prop a click-to-reposition-cursor in the textarea
     *  would close the panel mid-typing. ModeSelect / ModelSelect
     *  don't hit this because their trigger is a button inside
     *  their own `root`; TriggerMenu's trigger is external.
     *  Passed as a plain element (the parent's template ref is
     *  auto-unwrapped by Vue's template binding). */
    triggerEl?: HTMLElement | null;
    /** When true, filter via fuzzysort (substring-aware, score-sorted)
     *  instead of plain prefix match. B2 (@file, thousands of paths)
     *  sets this; B3 (/command) leaves the default prefix behaviour
     *  unchanged. */
    fuzzy?: boolean;
  }>(),
  {
    trigger: "/",
    headerLabel: "命令",
    emptyLabel: "无匹配项",
    triggerEl: null,
    fuzzy: false,
  },
);

const emit = defineEmits<{
  /** Parent should close the panel (set `open` to false) and run
   *  the item's action. */
  select: [item: TriggerMenuItem];
  /** Esc pressed OR click outside. Parent closes the panel. */
  close: [];
}>();

const root = ref<HTMLElement | null>(null);

/** Locally-filtered items. Two modes:
 *  - `fuzzy` (B2 @file): fuzzysort substring match, score-sorted, capped
 *    at `FUZZY_LIMIT`. Thousands of paths need this — `appcp` should
 *    find `app/src/components/chat/ChatPanel.vue`.
 *  - default (B3 /command): prefix match on `name` (case-insensitive;
 *    CJK has no case so the lowercasing is a no-op). Builtins keep
 *    their `builtin > project > user` order (we preserve `items` order). */
const filtered = computed<TriggerMenuItem[]>(() => {
  const raw = props.filter.trim();
  if (!raw) return props.items;
  if (props.fuzzy) {
    return fuzzysort
      .go(raw, props.items, { key: "name", limit: FUZZY_LIMIT })
      .map((r) => r.obj);
  }
  const f = raw.toLowerCase();
  return props.items.filter((it) => it.name.toLowerCase().startsWith(f));
});

/** Active (keyboard-highlighted) index. Reset to 0 whenever the
 *  filtered list changes (clamped to the new length). */
const activeIndex = ref(0);

watch(filtered, () => {
  activeIndex.value = filtered.value.length > 0 ? 0 : -1;
});

// Reset to 0 every time the panel opens (so re-opening with the
// arrow keys already pointing at the top item feels predictable).
watch(
  () => props.open,
  (open) => {
    if (open) activeIndex.value = filtered.value.length > 0 ? 0 : -1;
  },
);

/** Scroll the active row into view after the index changes. Uses
 *  `nextTick` so the DOM has updated before we measure offsets. */
async function scrollActiveIntoView() {
  await nextTick();
  const el = root.value?.querySelector<HTMLElement>(
    `[data-idx="${activeIndex.value}"]`,
  );
  el?.scrollIntoView({ block: "nearest" });
}

watch(activeIndex, () => {
  void scrollActiveIntoView();
});

/** Parent-driven keydown hook. The parent (ChatInput) intercepts
 *  ArrowUp / ArrowDown / Enter / Escape on its textarea WHEN the
 *  panel is open and calls these methods. We do NOT bind a window
 *  keydown listener here — that would race with the textarea's own
 *  Enter handling. The parent owns the routing. */
function moveActive(delta: number) {
  const len = filtered.value.length;
  if (len === 0) return;
  // Wrap around so the user can scroll past either end. `-1` (no
  // selection) is treated as 0 for the purpose of computing the
  // next index.
  const cur = activeIndex.value < 0 ? 0 : activeIndex.value;
  activeIndex.value = (cur + delta + len) % len;
}

function confirmActive() {
  const item = filtered.value[activeIndex.value];
  if (item) emit("select", item);
}

/** Click outside the panel root closes it. Mirrors ModeSelect /
 *   ModelSelect per `.trellis/spec/frontend/popover-pattern.md`.
 *   The `triggerEl` prop extends "inside" to include the parent's
 *   trigger element (e.g. the textarea) so click-to-position-cursor
 *   in the textarea does NOT close the panel mid-typing. */
function onDocumentClick(e: MouseEvent) {
  if (!props.open) return;
  const target = e.target as Node | null;
  if (!target) return;
  if (root.value && root.value.contains(target)) return;
  if (props.triggerEl && props.triggerEl.contains(target)) return;
  emit("close");
}

/** Esc closes. Bound on `window` because the textarea may hold
 *  focus while the panel is open (the panel itself has no focusable
 *  elements until a row is clicked). The parent's textarea Esc
 *  handler also routes here via `@keydown.escape`. */
function onKeyDown(e: KeyboardEvent) {
  if (!props.open) return;
  if (e.key === "Escape") {
    e.preventDefault();
    emit("close");
  }
}

onMounted(() => {
  if (typeof document !== "undefined") {
    document.addEventListener("click", onDocumentClick);
  }
  if (typeof window !== "undefined") {
    window.addEventListener("keydown", onKeyDown);
  }
});
onUnmounted(() => {
  if (typeof document !== "undefined") {
    document.removeEventListener("click", onDocumentClick);
  }
  if (typeof window !== "undefined") {
    window.removeEventListener("keydown", onKeyDown);
  }
});

/** Mouse-enter on a row moves the highlight. We do NOT move the
 *  highlight on mousemove (would fight keyboard nav); only on enter
 *  so a click is preceded by a visible highlight. */
function onRowEnter(i: number) {
  activeIndex.value = i;
}

function onRowClick(item: TriggerMenuItem) {
  emit("select", item);
}

defineExpose({
  /** Let the parent drive keyboard navigation without having to
   *  know about the filtered list's bounds. */
  moveActive,
  confirmActive,
});
</script>

<template>
  <Transition name="trigger-menu">
    <div
      v-if="open"
      ref="root"
      class="trigger-menu"
      role="listbox"
      :aria-label="headerLabel"
    >
    <div class="trigger-menu__header">
      <Icon name="command-line" :size="11" />
      <span class="trigger-menu__header-label">{{ headerLabel }}</span>
      <span
        v-if="filter"
        class="trigger-menu__header-filter"
        :title="`输入 ${trigger}${filter} 过滤中`"
      >{{ trigger }}{{ filter }}</span>
    </div>
    <div
      v-if="filtered.length === 0"
      class="trigger-menu__empty"
    >
      {{ emptyLabel }}
    </div>
    <div
      v-else
      class="trigger-menu__list"
    >
      <button
        v-for="(item, i) in filtered"
        :key="item.key"
        type="button"
        class="trigger-menu__row"
        :class="{ 'trigger-menu__row--active': i === activeIndex }"
        :data-idx="i"
        role="option"
        :aria-selected="i === activeIndex"
        @mouseenter="onRowEnter(i)"
        @click="onRowClick(item)"
      >
        <!-- Row slot: B3 renders name + description + hint + source
             chip; B2 (@file) will render a file icon + relative path.
             The default slot keeps B3 self-contained today; a future
             B2 task can replace the slot content without touching the
             panel shell. -->
        <slot
          name="row"
          :item="item"
          :active="i === activeIndex"
        >
          <span class="trigger-menu__row-main">
            <span class="trigger-menu__row-name">
              <span class="trigger-menu__row-trigger">{{ trigger }}</span>{{ item.name }}
            </span>
            <span
              v-if="item.description"
              class="trigger-menu__row-desc"
            >{{ item.description }}</span>
          </span>
          <span class="trigger-menu__row-meta">
            <span
              v-if="item.argument_hint"
              class="trigger-menu__row-hint"
            >{{ item.argument_hint }}</span>
            <span
              v-if="item.source"
              class="trigger-menu__row-source"
              :class="`trigger-menu__row-source--${item.source}`"
            >{{ item.source }}</span>
          </span>
        </slot>
      </button>
    </div>
    </div>
  </Transition>
</template>

<style scoped>
/* Hand-written upward-opening popover. Mirrors `ModeSelect` /
   `ModelSelect` per `.trellis/spec/frontend/popover-pattern.md`:
   - position: absolute relative to the chat-input row
   - bottom: calc(100% + 4px); top: auto; — opens UP
   - dark surface + border + 6px radius + 4-12 shadow
   - z-index 200 (same as the latency popover; below reka-ui portal
     modals at 9999). */
.trigger-menu {
  position: absolute;
  bottom: calc(100% + 4px);
  top: auto;
  left: 0;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
  min-width: 320px;
  max-width: 420px;
  max-height: 320px;
  z-index: 200;
  padding: 6px;
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-family: var(--font-sans);
  color: var(--color-text-primary);
}

.trigger-menu__header {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 2px 6px 4px;
  border-bottom: 1px solid var(--color-bg-border);
  color: var(--color-text-muted);
  font-size: 10px;
  font-family: var(--font-mono);
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.04em;
}

.trigger-menu__header-label {
  color: var(--color-text-secondary);
}

.trigger-menu__header-filter {
  margin-left: auto;
  color: var(--color-accent);
  text-transform: none;
  letter-spacing: 0;
  font-weight: 500;
  max-width: 160px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.trigger-menu__empty {
  padding: 12px 6px;
  text-align: center;
  color: var(--color-text-muted);
  font-size: 12px;
}

.trigger-menu__list {
  display: flex;
  flex-direction: column;
  gap: 1px;
  overflow-y: auto;
  max-height: 260px;
}

/* Each row is a grid: main (name + desc) on the left, meta
   (hint + source chip) on the right. Same column structure as
   ModeSelect's `.mode-select__item`. */
.trigger-menu__row {
  display: grid;
  grid-template-columns: 1fr auto;
  align-items: center;
  column-gap: 10px;
  row-gap: 1px;
  padding: 6px 8px;
  background: transparent;
  border: 0;
  border-radius: 4px;
  color: var(--color-text-primary);
  font: inherit;
  font-family: var(--font-sans);
  font-size: 12px;
  text-align: left;
  cursor: pointer;
  transition: background 0.08s;
}

.trigger-menu__row:hover,
.trigger-menu__row--active {
  background: var(--color-bg-elevated);
}

.trigger-menu__row--active {
  /* Subtle accent tint on the active row so keyboard nav is
     visible even when the row is not hovered. Reuses the same
     accent-muted token as the focused chat-input border for
     visual consistency. */
  box-shadow: inset 2px 0 0 var(--color-accent);
}

.trigger-menu__row-main {
  display: flex;
  flex-direction: column;
  gap: 1px;
  min-width: 0;
}

.trigger-menu__row-name {
  font-family: var(--font-mono);
  font-size: 12px;
  font-weight: 600;
  color: var(--color-text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.trigger-menu__row-trigger {
  color: var(--color-accent);
  margin-right: 1px;
}

.trigger-menu__row-desc {
  color: var(--color-text-muted);
  font-size: 10px;
  line-height: 1.4;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.trigger-menu__row--active .trigger-menu__row-desc {
  color: var(--color-text-secondary);
}

.trigger-menu__row-meta {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
}

.trigger-menu__row-hint {
  font-family: var(--font-mono);
  font-size: 10px;
  color: var(--color-text-muted);
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 3px;
  padding: 1px 5px;
}

/* Source chip color coding (mirrors the tool-color family from
   design-tokens.md so the user reads "builtin = system / project =
   local override / user = personal" at a glance). */
.trigger-menu__row-source {
  font-family: var(--font-mono);
  font-size: 9px;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  padding: 1px 5px;
  border-radius: 999px;
  border: 1px solid var(--color-bg-border);
  color: var(--color-text-muted);
}

/* Builtins use the accent (system / always-available). */
.trigger-menu__row-source--builtin {
  color: var(--color-accent);
  border-color: color-mix(in srgb, var(--color-accent) 40%, transparent);
  background: color-mix(in srgb, var(--color-accent) 12%, transparent);
}

/* Project overrides = local-to-repo; use the read color (matches
   the read_file tool's family). */
.trigger-menu__row-source--project {
  color: var(--color-tool-read);
  border-color: color-mix(in srgb, var(--color-tool-read) 40%, transparent);
  background: color-mix(in srgb, var(--color-tool-read) 12%, transparent);
}

/* User = personal; use the write color (matches write_file). */
.trigger-menu__row-source--user {
  color: var(--color-tool-write);
  border-color: color-mix(in srgb, var(--color-tool-write) 40%, transparent);
  background: color-mix(in srgb, var(--color-tool-write) 12%, transparent);
}

/* B4 Stretch 2 (2026-06-18): in the merged `/`-trigger panel, custom
   commands are no longer split into "user" / "project" rows — the
   backend collapses them to a single "command" source (the project
   > user precedence is enforced inside the listing). The chip uses
   a neutral read color so it's clearly distinct from `builtin`
   (accent) and `skill` (thinking) without the user/project split
   the B3 panel had. */
.trigger-menu__row-source--command {
  color: var(--color-tool-read);
  border-color: color-mix(in srgb, var(--color-tool-read) 40%, transparent);
  background: color-mix(in srgb, var(--color-tool-read) 12%, transparent);
}

/* Skill = directive layer; use the thinking color (matches the
   pre-staged skill token color in chatInputTokens.ts). B4 Stretch 2
   added `skill` to the merged `/`-trigger panel as a third source
   type, distinct from `command` (file/dir) and `builtin` (system). */
.trigger-menu__row-source--skill {
  color: var(--color-tool-thinking);
  border-color: color-mix(in srgb, var(--color-tool-thinking) 40%, transparent);
  background: color-mix(in srgb, var(--color-tool-thinking) 12%, transparent);
}

/* Open/close animation. Upward popover → slides from translateY(4px)
   up into place, matching `ModeSelect` / `ModelSelect`. Enter
   150ms ease-out, leave 100ms ease-in. */
.trigger-menu-enter-active,
.trigger-menu-leave-active {
  transition: opacity 150ms ease-out, transform 150ms ease-out;
  transform-origin: bottom left;
}

.trigger-menu-enter-from,
.trigger-menu-leave-to {
  opacity: 0;
  transform: translateY(4px);
}

.trigger-menu-leave-active {
  transition-duration: 100ms;
  transition-timing-function: ease-in;
}
</style>
