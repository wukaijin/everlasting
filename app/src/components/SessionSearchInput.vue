<script setup lang="ts">
// SessionSearchInput — sidebar session filter input. Pure
// presentation: parent owns `modelValue` via v-model; emits
// `clear` when the user clicks the ✕ button or presses Esc.
//
// 2026-08-30: ✕ 常驻(此前只在 query 非空时渲染),空查询时也能
// 点它关闭搜索行 —— 旧版空态没有任何可见关闭入口,只能靠 Esc。
// 点击语义与 Esc 对齐:有文字=清空+关闭,空=关闭。
//
// 2026-06-27 sidebar 搜索入口 (PR-of-PRs, 3 features): the
// sidebar previously had no way to filter the session list. At
// 10+ sessions the list becomes a wall of text; a simple
// substring filter scoped to titles is enough for the common
// "where was that conversation about X?" question.
//
// Autofocus: the input mounts focused so the user can type
// immediately after clicking the 🔍 icon. We focus on mount +
// after the parent's `searchActive` flips from false to true
// (re-entry case: user closed the input then reopens it).

import { ref, watch, nextTick, onMounted } from "vue";
import Icon from "./Icon.vue";

const props = withDefaults(
  defineProps<{
    /** v-model binding for the input value. Empty string when
     *  cleared. */
    modelValue: string;
    /** Placeholder shown when the input is empty. Defaults to
     *  the sidebar's standard "搜索会话标题…" prompt. */
    placeholder?: string;
  }>(),
  { placeholder: "搜索会话标题…" },
);

const emit = defineEmits<{
  (e: "update:modelValue", value: string): void;
  /** Emitted when the user clicks the ✕ button or presses Esc.
   *  Parent typically clears the query and (optionally) hides
   *  the search input row. */
  (e: "clear"): void;
}>();

const inputRef = ref<HTMLInputElement | null>(null);

function focusInput() {
  inputRef.value?.focus();
  inputRef.value?.select();
}

/** Public focus handle. Exposed via `defineExpose` so the parent
 *  (SessionList) can call `searchInputRef.value.focus()` from its
 *  Cmd/Ctrl+K handler and from its `searchActive` watcher. Without
 *  this, the parent's `ref` would receive the component instance
 *  (which has no `.focus()` method) and vue-tsc errors out. */
defineExpose({ focus: focusInput });

onMounted(() => {
  focusInput();
});

// Re-focus when the input remounts (e.g. user closes the
// sidebar's search row then reopens it — Vue unmounts/remounts
// this component, so onMounted fires again, so we don't need
// an explicit `watch` here). Kept the onMounted above; this
// watch is a belt-and-braces guard for any future refactor
// that re-uses this component in a keep-alive slot.
watch(
  () => props.modelValue,
  () => {
    nextTick(focusInput);
  },
);

function onInput(e: Event) {
  const v = (e.target as HTMLInputElement).value;
  emit("update:modelValue", v);
}

function onClear() {
  emit("update:modelValue", "");
  emit("clear");
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === "Escape") {
    e.preventDefault();
    e.stopPropagation();
    if (props.modelValue.length > 0) {
      // First Esc clears the query; a second Esc closes the
      // input row (handled by the parent's clear handler).
      emit("update:modelValue", "");
    } else {
      emit("clear");
    }
  }
}
</script>

<template>
  <div class="session-search">
    <Icon
      name="magnifying-glass"
      :size="12"
      class="session-search__icon"
    />
    <input
      ref="inputRef"
      type="text"
      class="session-search__input"
      :value="modelValue"
      :placeholder="placeholder"
      maxlength="80"
      autocomplete="off"
      spellcheck="false"
      aria-label="搜索会话"
      @input="onInput"
      @keydown="onKeydown"
    />
    <button
      class="session-search__clear btn btn--icon btn--ghost"
      type="button"
      :title="modelValue.length > 0 ? '清空并关闭 (Esc)' : '关闭搜索 (Esc)'"
      :aria-label="modelValue.length > 0 ? '清空并关闭搜索' : '关闭搜索'"
      @click="onClear"
    >
      <Icon name="x" :size="12" />
    </button>
  </div>
</template>

<style scoped>
/* 2026-06-27 sidebar 搜索入口: thin input row with a leading
   magnifier icon and a trailing close button (常驻,2026-08-30 起
   空查询也渲染 — 见文件头注)。Visual sits in the same elevation tier
   as the session items below it (--color-bg-elevated) so the
   input feels "anchored" without competing with the list. */
.session-search {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 4px 8px;
  margin: 4px 8px 6px 8px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  transition: border-color var(--duration-fast) var(--ease-out),
    background var(--duration-fast) var(--ease-out);
}

.session-search:focus-within {
  border-color: var(--color-accent);
  background: var(--color-bg-surface);
}

.session-search__icon {
  flex-shrink: 0;
  color: var(--color-text-muted);
}

.session-search:focus-within .session-search__icon {
  color: var(--color-accent-text);
}

.session-search__input {
  flex: 1;
  min-width: 0;
  border: none;
  /* 焦点指示=输入光标 caret(裸搜索框,容器已提供上下文) */
  outline: none;
  background: transparent;
  color: var(--color-text-primary);
  font-family: inherit;
  font-size: var(--text-sm);
  padding: 2px 0;
}

.session-search__input::placeholder {
  color: var(--color-text-muted);
}

/* 关闭钮由全局 .btn 家族承载(ghost icon);此处仅几何。 */
.session-search__clear {
  flex-shrink: 0;
  width: 18px;
  height: 18px;
  padding: 0;
}
</style>
