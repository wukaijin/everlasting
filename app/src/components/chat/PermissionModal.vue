<script setup lang="ts">
// PermissionModal — ⑨ 关 三-button 确认弹窗 (PR3 of A2 + B7,
// path range row added in PR2 of re-grill 2026-06-13).
//
// Triggers when the backend agent loop emits `permission:ask`
// (via `usePermissionsStore.pendingPermission`). Renders the
// 6-part layout specified in
// `.trellis/tasks/06-12-a2-b7-permission-and-mode/research/permission-modal-ux.md`
// §"Final Output: 可粘贴到 PRD 的 spec 段落",
// extended in re-grill Q10 with a **path range row** for path tools:
//
//   1. Header — shield icon 容器 (56x56, risk-tint bg) + 标题 + 关闭 X
//   2. Subtitle — "Agent 想在项目 X 下执行以下操作:"
//   3. Path range row — only for path tools (read_file / write_file
//      / edit_file / list_dir / grep / glob); shows the path the
//      agent wants to touch + an in-repo (emerald) / out-of-repo
//      (amber) badge. For shell / web_fetch this row is entirely
//      hidden (no empty placeholder, no layout shift).
//   4. Command preview — terminal icon + `<pre>` + copy icon
//   5. Risk label — "工具类别: <tool> · 风险等级: <risk-label-cn>"
//   6. Footer — 3 buttons (拒绝 / 仅一次 / 始终允许),critical 时
//      focus 改"拒绝"
//
// 关闭/取消语义 (per spec Q6):
//   - Esc / X / 遮罩点击 = "deny" (`is_error: true` 给 LLM,不
//     触发 CancellationToken — deny ≠ cancel 整轮)
//   - 120s 超时 → usePermissionsStore 自身 fire "deny" + toast
//
// Visual style:
//   - Center modal, width `min(560px, 90vw)`, max-height 80vh
//   - Backdrop `color-mix(--color-bg-app 70%, transparent)` + 4px blur
//   - Critical risk 时 modal 卡片左 border 3px red (3px 而非 4px —
//     design-tokens.md "Border width is always 1px" 例外,跟
//     YoloConfirmModal 的 3px 对齐)
//   - Path range row badge: in-repo = `--color-tool-write`
//     (emerald, same family as `write_file` tool color); out-of-repo
//     = `--color-tool-shell` (amber, same family as `shell` tool
//     color). Per re-grill spec the original brief mentioned
//     `--color-tool-success` / `--color-tool-warning` but those
//     tokens don't exist in `app/src/style.css` today; we reuse the
//     closest existing tool-color tokens (same Tailwind 400-500
//     palette) per design-tokens.md "Don't add a new `--color-*`
//     token for a one-off use".
//   - Reka-ui 2.9.9 `DialogContent` portals to <body> — see the
//     `:deep()` gotcha notes in `.trellis/spec/frontend/reka-ui-usage.md`.
//     All `permission-modal__*` classes used in CSS are wrapped in
//     `:deep()`.
//
// 测试: `PermissionModal.test.ts` 覆盖 3-button emission, critical
// risk focus, Esc/click-outside cancel, copy button, path range
// row presence/absence. Reka-ui `DialogContent` 的实际渲染由
// component test 覆盖。

import { computed, nextTick, onUnmounted, ref, watch } from "vue";
import Icon from "../Icon.vue";
import { useChatStore } from "../../stores/chat";
import { isPathInRoot } from "../../utils/path";
import {
  RISK_META,
  usePermissionsStore,
  type PermissionDecision,
} from "../../stores/permissions";

const store = usePermissionsStore();
const chatStore = useChatStore();

/** Open when there's a pending ask. Mirrors the
 *  `YoloConfirmModal` pattern of deriving `open` from a store
 *  flag, so the modal mounts/unmounts cleanly across
 *  multi-tool_use batches. */
const open = computed<boolean>(() => store.pendingPermission !== null);

/** The active ask, or `null` for the unmounted state. */
const ask = computed(() => store.pendingPermission);

/** The `decision` the Enter key would fire. Critical risk
 *  → "deny" (per spec audit §6.2 — "critical Enter 改'拒绝'");
 *  otherwise "allow_once" (the spec's "默认主操作"). */
const defaultDecision = computed<PermissionDecision>(() => {
  if (!ask.value) return "allow_once";
  return ask.value.risk === "critical" ? "deny" : "allow_once";
});

/** Refs to the 3 button DOM elements so we can drive focus. */
const denyButton = ref<HTMLButtonElement | null>(null);
const allowOnceButton = ref<HTMLButtonElement | null>(null);
const allowAlwaysButton = ref<HTMLButtonElement | null>(null);

/** Formatted JSON for the `<pre>` block. Pretty-printed with
 *  2-space indent + tab-size 2 (per spec). Falls back to
 *  `String(toolInput)` when the input isn't a plain object
 *  (defensive — the IPC payload could in principle be an array
 *  or a scalar). */
const formattedInput = computed<string>(() => {
  const ti = ask.value?.toolInput;
  if (ti === undefined || ti === null) return "";
  try {
    return JSON.stringify(ti, null, 2);
  } catch {
    return String(ti);
  }
});

/** Critical-risk variant flag. Drives the 3px red left border
 *  + shield-x icon. */
const isCritical = computed<boolean>(
  () => ask.value?.risk === "critical",
);

/** True for `low` risk (info icon). Centralized so the
 *  icon-name lookup stays consistent. */
const iconName = computed<string>(() => {
  const r = ask.value?.risk;
  if (!r) return "info";
  return RISK_META[r].iconName;
});

/** Risk-tint background color (12% alpha mix of the risk
 *  full color, per spec). Returns a typed `CSSProperties` so
 *  the `<div :style="...">` binding is type-safe. */
const iconTintStyle = computed<Record<string, string>>(() => {
  const r = ask.value?.risk;
  if (!r) return {};
  const full = RISK_META[r].iconColor;
  return {
    color: full,
    background: `color-mix(in srgb, ${full} 12%, transparent)`,
  } as Record<string, string>;
});

/** Risk label text + the colored dot. */
const riskLabelText = computed<string>(() => {
  const r = ask.value?.risk;
  if (!r) return "";
  return RISK_META[r].label;
});

/** Whether to render the path range row (re-grill 2026-06-13
 *  PR2). Mirrors the backend's `#[serde(skip_serializing_if =
 *  "Option::is_none")]` on `path`: true only when the active
 *  ask has a `path` field. Shell / web_fetch leave this row
 *  entirely hidden. */
const hasPath = computed<boolean>(
  () => typeof ask.value?.path === "string" && ask.value.path.length > 0,
);

/** The path string to display, or empty string when absent. */
const pathText = computed<string>(() => ask.value?.path ?? "");

/** In-repo vs out-of-repo flag. Mirrors the Rust
 *  `projects::boundary::is_within_root` predicate (re-grill
 *  2026-06-13). The frontend helper `isPathInRoot` is a
 *  component-wise lexical match — see `.trellis/utils/path.ts`
 *  for the rationale and edge cases.
 *
 *  Empty / unknown cwd (e.g. very early in app boot before the
 *  chat store has resolved a session) → we treat the path as
 *  "outside" defensively (out-of-repo badge). This matches the
 *  "default-allow in-repo / ask out-of-repo" Tier 4 contract
 *  from the re-grill spec — better to ask one extra time than
 *  to silently bypass the gate. */
const isInRepo = computed<boolean>(() => {
  if (!hasPath.value) return false;
  const root = chatStore.currentCwd;
  if (!root) return false;
  return isPathInRoot(pathText.value, root);
});

/** Path range row badge: "仓库内" (in-repo, emerald) or "仓库外"
 *  (out-of-repo, amber). Color tokens reuse `--color-tool-write`
 *  and `--color-tool-shell` (see file header note on the design
 *  tokens naming). */
const pathBadgeText = computed<string>(() =>
  isInRepo.value ? "仓库内" : "仓库外",
);
const pathBadgeColor = computed<string>(() =>
  isInRepo.value ? "var(--color-tool-write)" : "var(--color-tool-shell)",
);

/** Copy-state ref: `false` → show copy icon, `true` → show check
 *  icon (after a successful clipboard write). Auto-resets to
 *  `false` after 2s. */
const justCopied = ref<boolean>(false);

/** Run `navigator.clipboard.writeText(...)` with a 2s checkmark
 *  feedback. Toast z-index is 10000 per the spec — see
 *  `useProjectsStore.showToast` for the actual z-index. */
async function copyInput(): Promise<void> {
  const text = formattedInput.value;
  if (!text) return;
  try {
    await navigator.clipboard.writeText(text);
    justCopied.value = true;
    window.setTimeout(() => {
      justCopied.value = false;
    }, 2000);
  } catch (e) {
    // Best-effort; in a non-secure context clipboard may be
    // unavailable. The text is already on-screen — the user can
    // still copy manually.
    console.warn("PermissionModal.copyInput failed:", e);
  }
}

/** Decision dispatcher. Called from any of the 3 buttons OR
 *  from the Enter-key handler in the modal. The `rid` is the
 *  active ask's rid — the store keys the response by it. */
async function onDecision(decision: PermissionDecision): Promise<void> {
  const active = store.pendingPermission;
  if (!active) return;
  await store.respond(active.rid, decision);
}

/** Esc / X / 遮罩点击 = deny. We don't trigger a CancellationToken
 *  here — per spec Q6 "deny ≠ cancel 整轮". */
function onCancel(): void {
  const active = store.pendingPermission;
  if (!active) return;
  void store.respond(active.rid, "deny");
}

/** Global keydown handler — Esc / Enter. Bound on `window`
 *  because the modal itself may not have focus right after
 *  mount. Mirrors `ConfirmDialog`/`YoloConfirmModal` pattern. */
function onKeyDown(e: KeyboardEvent): void {
  if (!open.value) return;
  if (e.key === "Escape") {
    e.preventDefault();
    onCancel();
  } else if (e.key === "Enter") {
    // Enter → default decision (critical → deny, otherwise →
    // allow_once). Per spec audit §6.2.
    e.preventDefault();
    void onDecision(defaultDecision.value);
  }
}

if (typeof window !== "undefined") {
  window.addEventListener("keydown", onKeyDown);
}
onUnmounted(() => {
  if (typeof window !== "undefined") {
    window.removeEventListener("keydown", onKeyDown);
  }
});

/** Focus the default-button on every `open` transition. Watch
 *  the `ask` ref (not `open`) so we re-focus when a new ask
 *  replaces the previous one (the modal remounts via `:key`,
 *  but the spec also supports same-instance replace; the watch
 *  is the safe belt). We use `nextTick` because the DOM may not
 *  be ready immediately after `v-if` mounts. */
watch(
  () => ask.value?.rid,
  async () => {
    if (!open.value) return;
    await nextTick();
    const target =
      defaultDecision.value === "deny"
        ? denyButton.value
        : defaultDecision.value === "allow_once"
          ? allowOnceButton.value
          : allowAlwaysButton.value;
    target?.focus();
  },
);
</script>

<template>
  <Teleport to="body">
    <Transition name="permission-modal">
      <div
        v-if="open && ask"
        class="permission-modal-backdrop"
        @click.self="onCancel"
      >
        <div
          class="permission-modal"
          :class="{ 'permission-modal--critical': isCritical }"
          role="dialog"
          aria-modal="true"
          :aria-labelledby="'permission-modal-title'"
        >
          <!-- Header: 56x56 icon container + title + close X -->
          <header class="permission-modal__header">
            <div class="permission-modal__icon" :style="iconTintStyle">
              <Icon :name="iconName" :size="28" />
            </div>
            <div class="permission-modal__title-row">
              <h2
                id="permission-modal-title"
                class="permission-modal__title"
              >
                {{ RISK_META[ask.risk].title }}
              </h2>
              <p
                v-if="ask.reason"
                class="permission-modal__reason"
              >
                {{ ask.reason }}
              </p>
            </div>
            <button
              type="button"
              class="permission-modal__close"
              aria-label="Close"
              @click="onCancel"
            >
              <Icon name="x" :size="14" />
            </button>
          </header>

          <!-- Body: subtitle + path range + command preview + risk label -->
          <div class="permission-modal__body">
            <p class="permission-modal__subtitle">
              Agent 想在当前项目下执行以下操作:
            </p>

            <!-- Path range row (re-grill 2026-06-13 PR2, Q10):
                 Only renders when `ask.path` is set (path tools —
                 read_file / write_file / edit_file / list_dir /
                 grep / glob). For shell / web_fetch the row is
                 entirely absent (no empty placeholder, no layout
                 shift). The badge reflects the in-repo / out-of-repo
                 decision computed against the session's currentCwd;
                 the badge color reuses the existing tool-color
                 tokens (see file header note). -->
            <div v-if="hasPath" class="permission-modal__path-range">
              <span
                class="permission-modal__path-range-icon"
                aria-hidden="true"
              >
                <Icon name="folder" :size="14" />
              </span>
              <code class="permission-modal__path-range-text">{{ pathText }}</code>
              <span
                class="permission-modal__path-range-badge"
                :style="{
                  color: pathBadgeColor,
                  borderColor: pathBadgeColor,
                  background: `color-mix(in srgb, ${pathBadgeColor} 12%, transparent)`,
                }"
              >
                {{ pathBadgeText }}
              </span>
            </div>

            <!-- Command preview block: terminal icon (left) +
                 <pre> + copy icon (right) -->
            <div class="permission-modal__preview">
              <span class="permission-modal__preview-icon" aria-hidden="true">
                <Icon name="terminal" :size="14" />
              </span>
              <pre class="permission-modal__preview-pre">{{ formattedInput }}</pre>
              <button
                type="button"
                class="permission-modal__copy"
                :title="justCopied ? '已复制' : '复制'"
                @click="copyInput"
              >
                <Icon
                  :name="justCopied ? 'check-mini' : 'copy'"
                  :size="14"
                />
              </button>
            </div>

            <!-- Risk label (full chinese per audit §6.2) -->
            <div class="permission-modal__risk">
              <span
                class="permission-modal__risk-dot"
                :style="{ background: RISK_META[ask.risk].iconColor }"
              ></span>
              <span>工具类别:</span>
              <span class="permission-modal__risk-tool">{{ ask.toolName }}</span>
              <span class="permission-modal__risk-sep">·</span>
              <span>风险等级:</span>
              <span class="permission-modal__risk-label">{{ riskLabelText }}</span>
            </div>
          </div>

          <!-- Footer: 3 buttons, 等宽 33%, 顺序: 拒绝 / 仅一次 / 始终允许 -->
          <footer class="permission-modal__actions">
            <button
              ref="denyButton"
              type="button"
              class="permission-modal__btn permission-modal__btn--deny"
              @click="onDecision('deny')"
            >
              拒绝
            </button>
            <button
              ref="allowOnceButton"
              type="button"
              class="permission-modal__btn permission-modal__btn--once"
              @click="onDecision('allow_once')"
            >
              仅一次
            </button>
            <button
              ref="allowAlwaysButton"
              type="button"
              class="permission-modal__btn permission-modal__btn--always"
              @click="onDecision('allow_always')"
            >
              始终允许
            </button>
          </footer>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
/* The modal itself lives in <style scoped>, but the rendered
   DOM (via <Teleport to="body">) is OUTSIDE the scoped
   boundary — reka-ui's own <DialogContent> would have the
   same issue; we're using hand-rolled markup here so we get
   the same `:deep()`-stripped selectors. Every
   `.permission-modal__*` rule below is therefore wrapped in
   `:deep()`. The Vue compiler adds a `data-v-xxx` attribute
   to elements INSIDE the component template (none here, since
   everything is portal'd), and `:deep()` strips that for the
   inner selector. See `.trellis/spec/frontend/reka-ui-usage.md`
   §"Gotcha: <style scoped> does NOT apply to portal children"
   for the underlying mechanism. */

:deep(.permission-modal-backdrop) {
  position: fixed;
  inset: 0;
  background: color-mix(in srgb, var(--color-bg-app) 70%, transparent);
  backdrop-filter: blur(4px);
  z-index: 9998;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
}

:deep(.permission-modal) {
  position: fixed;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 8px;
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);
  width: min(560px, 90vw);
  max-height: 80vh;
  display: flex;
  flex-direction: column;
  z-index: 9999;
  overflow: hidden;
}

/* Critical variant: 3px red left border (per spec — only the
   border-LEFT is thicker; design-tokens.md "Border width is
   always 1px" 例外, 跟 YoloConfirmModal 的 3px 对齐). */
:deep(.permission-modal--critical) {
  border-left: 3px solid var(--color-tool-error);
}

/* Header layout: 56x56 icon container + title block + close X */
:deep(.permission-modal__header) {
  display: grid;
  grid-template-columns: auto 1fr auto;
  align-items: flex-start;
  gap: 14px;
  padding: 16px 16px 12px;
  border-bottom: 1px solid var(--color-bg-border);
}

:deep(.permission-modal__icon) {
  width: 56px;
  height: 56px;
  border-radius: 12px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}

:deep(.permission-modal__title-row) {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
  padding-top: 6px;
}

:deep(.permission-modal__title) {
  margin: 0;
  font-size: 16px;
  font-weight: 600;
  color: var(--color-text-primary);
  font-family: var(--font-sans);
  line-height: 1.3;
}

:deep(.permission-modal__reason) {
  margin: 0;
  font-size: 12px;
  color: var(--color-text-muted);
  line-height: 1.4;
}

:deep(.permission-modal__close) {
  background: transparent;
  border: 0;
  color: var(--color-text-muted);
  cursor: pointer;
  padding: 4px;
  border-radius: 4px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  margin-top: 4px;
}

:deep(.permission-modal__close:hover) {
  background: var(--color-bg-border);
  color: var(--color-text-primary);
}

/* Body: subtitle + command preview + risk label */
:deep(.permission-modal__body) {
  padding: 14px 16px 16px;
  display: flex;
  flex-direction: column;
  gap: 12px;
  overflow-y: auto;
}

:deep(.permission-modal__subtitle) {
  margin: 0;
  font-size: 14px;
  color: var(--color-text-secondary);
  font-family: var(--font-sans);
  line-height: 1.4;
}

/* Path range row (re-grill 2026-06-13 PR2). Renders only when
   `ask.path` is set; the inline badge color is bound via :style
   to the in-repo / out-of-repo tool-color token. The badge
   uses an inline `style` for color + a 12% mix for background
   (matches the risk-icon container's `iconTintStyle` pattern
   above, so the visual language is consistent). */
:deep(.permission-modal__path-range) {
  display: flex;
  align-items: center;
  gap: 8px;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: 8px;
  padding: 8px 12px;
  min-width: 0;
}

:deep(.permission-modal__path-range-icon) {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--color-text-muted);
  flex-shrink: 0;
}

:deep(.permission-modal__path-range-text) {
  flex: 1;
  min-width: 0;
  margin: 0;
  font-family: var(--font-mono);
  font-size: 12px;
  line-height: 1.5;
  color: var(--color-text-primary);
  background: transparent;
  border: 0;
  padding: 0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

:deep(.permission-modal__path-range-badge) {
  flex-shrink: 0;
  display: inline-block;
  padding: 2px 8px;
  border-radius: 999px;
  border: 1px solid;
  font-family: var(--font-sans);
  font-size: 11px;
  font-weight: 500;
  line-height: 1.4;
  white-space: nowrap;
}

/* Command preview block: terminal icon (left) + <pre> (mid) +
   copy icon (right). The <pre> uses `flex: 1` to fill the
   remaining width; copy icon is fixed-width at the right
   edge. */
:deep(.permission-modal__preview) {
  display: flex;
  align-items: stretch;
  gap: 8px;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: 8px;
  padding: 10px 12px;
  min-width: 0;
}

:deep(.permission-modal__preview-icon) {
  display: inline-flex;
  align-items: flex-start;
  justify-content: center;
  color: var(--color-text-muted);
  padding-top: 2px;
  flex-shrink: 0;
}

:deep(.permission-modal__preview-pre) {
  flex: 1;
  margin: 0;
  font-family: var(--font-mono);
  font-size: 12px;
  line-height: 1.5;
  color: var(--color-text-primary);
  background: transparent;
  border: 0;
  padding: 0;
  max-height: 240px;
  overflow: auto;
  white-space: pre-wrap;
  word-break: break-word;
  tab-size: 2;
  min-width: 0;
}

:deep(.permission-modal__copy) {
  background: transparent;
  border: 0;
  color: var(--color-text-muted);
  cursor: pointer;
  padding: 2px 4px;
  border-radius: 4px;
  display: inline-flex;
  align-items: flex-start;
  justify-content: center;
  flex-shrink: 0;
  margin-top: 2px;
}

:deep(.permission-modal__copy:hover) {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
}

/* Risk label row: 工具类别: <tool> · 风险等级: <label> */
:deep(.permission-modal__risk) {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 14px;
  color: var(--color-text-secondary);
  font-family: var(--font-sans);
  line-height: 1;
}

:deep(.permission-modal__risk-dot) {
  display: inline-block;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
}

:deep(.permission-modal__risk-tool) {
  font-family: var(--font-mono);
  color: var(--color-text-primary);
}

:deep(.permission-modal__risk-sep) {
  color: var(--color-text-muted);
  padding: 0 2px;
}

:deep(.permission-modal__risk-label) {
  color: var(--color-text-primary);
  font-weight: 500;
}

/* Footer: 3 buttons, 等宽 33%, 间距 8px */
:deep(.permission-modal__actions) {
  display: flex;
  gap: 8px;
  padding: 12px 16px 14px;
  border-top: 1px solid var(--color-bg-border);
}

:deep(.permission-modal__btn) {
  flex: 1;
  font: inherit;
  font-family: var(--font-sans);
  font-size: 13px;
  padding: 8px 16px;
  border-radius: 6px;
  cursor: pointer;
  border: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
  transition: filter 0.1s, border-color 0.1s;
}

:deep(.permission-modal__btn:hover) {
  filter: brightness(1.08);
  border-color: var(--color-accent-muted);
}

/* "始终允许" 主按钮: accent 背景, 最强强调 */
:deep(.permission-modal__btn--always) {
  background: var(--color-accent);
  color: #ffffff;
  border-color: var(--color-accent);
}

:deep(.permission-modal__btn--always:hover) {
  background: var(--color-accent-hover);
  border-color: var(--color-accent-hover);
  filter: none;
}

/* 150ms fade + scale 0.96 → 1 (popover-pattern.md modal conv.) */
.permission-modal-enter-active,
.permission-modal-leave-active {
  transition: opacity 150ms ease-out;
}

.permission-modal-enter-active :deep(.permission-modal),
.permission-modal-leave-active :deep(.permission-modal) {
  transition: opacity 150ms ease-out, transform 150ms ease-out;
}

.permission-modal-enter-from,
.permission-modal-leave-to {
  opacity: 0;
}

.permission-modal-enter-from :deep(.permission-modal),
.permission-modal-leave-to :deep(.permission-modal) {
  opacity: 0;
  transform: translate(-50%, -50%) scale(0.96);
}

.permission-modal-leave-active,
.permission-modal-leave-active :deep(.permission-modal) {
  transition-duration: 100ms;
  transition-timing-function: ease-in;
}
</style>