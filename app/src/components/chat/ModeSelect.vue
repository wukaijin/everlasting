<script setup lang="ts">
// ModeSelect — per-session Mode picker. Mirrors `ModelSelect.vue`'s
// hand-written popover pattern (see `.trellis/spec/frontend/popover-pattern.md`)
// with two differences:
//
// 1. **Upward vs downward popover**: `ModelSelect` opens UP because
//    the trigger sits at the bottom of the chat input (same
//    geometry as ModeSelect — both open UP). Same `bottom: calc(100%
//    + 4px); top: auto;` and `translateY(4px → 0)` slide.
//
// 2. **Compact 3-row list vs grouped model list**: ModeSelect has
//    exactly 3 entries (Edit / Plan / Yolo), so no
//    grouping / scrolling is needed. Width can be narrower
//    (~220px vs `ModelSelect`'s 220px). 3 档化 2026-06-13: Review
//    移除 (跟 Plan 行为重复), Chat 改名 Edit (语义更清晰)。
//
// UX flow:
// - Click trigger → popover opens with the 3 modes listed.
// - Click a non-Yolo mode → popover closes immediately, IPC fires.
// - Click Yolo → popover closes, **YoloConfirmModal** opens
//   (driven by the shared `pendingYoloConfirm` flag in the
//   chat store — see `useChatStore.requestSetMode`).
//
// Streaming state: the trigger + popover are CLICKABLE while
// `isCurrentSessionStreaming` is true. The backend's mode
// change applies on the next turn boundary (read at
// `chat_loop.rs:396`, not mid-stream), so a mode flip made
// during streaming only takes effect for the NEXT turn. To
// avoid the user expecting "I clicked Yolo and the next tool
// ran unprompted", we surface a `projectsStore.showToast`
// hint ONLY while streaming — non-streaming flips are
// immediately visible (the chip label flips to the new mode)
// so a toast there would be noise.
//
// `Shift+Tab` cycle is registered in `ChatInput.vue` via
// `useKeyboard` and routes through the SAME `requestSetMode`
// store action — so a Shift+Tab flip into Yolo opens the same
// modal the popover would.

import { computed, onUnmounted, ref } from "vue";

import { useChatStore } from "../../stores/chat";
import type { SessionMode } from "../../stores/chat.types";
import { useProjectsStore } from "../../stores/projects";
import Icon from "../Icon.vue";
import YoloConfirmModal from "./YoloConfirmModal.vue";

const chatStore = useChatStore();
const projectsStore = useProjectsStore();

const menuOpen = ref(false);
const menuRoot = ref<HTMLElement | null>(null);

/** Mode options shown in the popover. Order matches
 *  `MODE_CYCLE` in `chat.ts` so the popover reads top-to-bottom
 *  in the same order Shift+Tab cycles. `Background` is
 *  intentionally excluded (reserved in backend enum only).
 *  3 档化 2026-06-13: 删 Review, Chat 改名 Edit。 */
interface ModeOption {
  value: SessionMode;
  label: string;
  /** Lucide icon name from `Icon.vue`'s registry. */
  icon: string;
  /** Plain-language description rendered under the label. */
  description: string;
}

const modeOptions: readonly ModeOption[] = [
  {
    value: "edit",
    label: "Edit",
    icon: "pencil",
    description: "默认模式，可调用所有工具，危险操作需确认",
  },
  {
    value: "plan",
    label: "Plan",
    icon: "clipboard-list",
    description: "只分析与制定方案，不执行写操作",
  },
  {
    value: "yolo",
    label: "Yolo",
    icon: "shield-x",
    description: "跳过所有用户确认，硬 kill list 仍然拦截",
  },
] as const;

/** True if the user has any session active. When no session is
 *  active we don't render the chip — there's nothing to switch. */
const hasSession = computed<boolean>(() => !!chatStore.currentSessionId);

/** Current mode for the active session. Reads from the
 *  `SessionSummary.mode` wire field (per-session override). Falls
 *  back to `"chat"` for pre-PR2 sessions whose wire payload
 *  may be missing the field (defensive — PR1's backfill
 *  guarantees it but legacy rows could theoretically still hit
 *  the UI). */
const currentMode = computed<SessionMode>(() => {
  const sid = chatStore.currentSessionId;
  if (!sid) return "edit";
  const s = chatStore.sessions.find((x) => x.id === sid);
  if (!s) return "edit";
  if (s.mode === "plan" || s.mode === "yolo") {
    return s.mode;
  }
  // Anything else (Edit / Background / legacy / null) → Edit as default.
  return "edit";
});

const currentModeLabel = computed<string>(() => {
  const m = modeOptions.find((o) => o.value === currentMode.value);
  return m?.label ?? "Edit";
});

function toggleMenu() {
  menuOpen.value = !menuOpen.value;
}

function closeMenu() {
  menuOpen.value = false;
}

/** Click outside the popover root closes it. Mirrors
 *  `ModelSelect.onDocumentClick` / worktree dropdown. */
function onDocumentClick(e: MouseEvent) {
  if (!menuOpen.value) return;
  const target = e.target as Node | null;
  if (menuRoot.value && target && !menuRoot.value.contains(target)) {
    menuOpen.value = false;
  }
}

/** Esc closes the popover. Bound on `window` because the
 *  trigger may not have focus when the popover is open. */
function onKeyDown(e: KeyboardEvent) {
  if (e.key === "Escape" && menuOpen.value) {
    menuOpen.value = false;
  }
}

if (typeof document !== "undefined") {
  document.addEventListener("click", onDocumentClick);
}
if (typeof window !== "undefined") {
  window.addEventListener("keydown", onKeyDown);
}
onUnmounted(() => {
  if (typeof document !== "undefined") {
    document.removeEventListener("click", onDocumentClick);
  }
  if (typeof window !== "undefined") {
    window.removeEventListener("keydown", onKeyDown);
  }
});

/** Click handler for a popover row. Delegates to the chat
 *  store so the Yolo confirm flow is shared with the
 *  `Shift+Tab` keyboard entry (see `useKeyboard` /
 *  `ChatInput.cycleMode`). Always closes the popover so the
 *  user sees immediate feedback (the modal — if any — opens
 *  on top).
 *
 *  Toast hint semantics:
 *  - Yolo (modal path) → no toast here; the modal's confirm
 *    button handler awaits `confirmYolo` and toasts there.
 *  - Edit / Plan (direct IPC) → toast ONLY while streaming,
 *    because mid-stream flips apply on the next turn boundary
 *    and the chip label change alone might look "stuck" until
 *    the next turn starts. Non-streaming flips are immediately
 *    visible (chip label flips), so a toast there would be noise. */
async function onModePick(mode: SessionMode) {
  closeMenu();
  const sid = chatStore.currentSessionId;
  if (!sid) return;
  const applied = await chatStore.requestSetMode(sid, mode);
  if (!applied) return; // Yolo modal flow — toast deferred to confirm
  if (chatStore.isCurrentSessionStreaming) {
    projectsStore.showToast(
      "Mode 已切换，将在下一轮 turn 生效",
      "info",
      3000,
    );
  }
}

/** Yolo modal confirm handler. Awaits the IPC + optimistic
 *  update, then surfaces the same "next-turn applies" toast
 *  the non-Yolo path emits — only when streaming. We route
 *  the toast here (rather than in `chat.ts`) so the modal's
 *  cancel path doesn't need a counter-handler. */
async function onYoloConfirm() {
  const applied = await chatStore.confirmYolo();
  if (applied && chatStore.isCurrentSessionStreaming) {
    projectsStore.showToast(
      "Mode 已切换，将在下一轮 turn 生效",
      "info",
      3000,
    );
  }
}
</script>

<template>
  <div
    v-if="hasSession"
    ref="menuRoot"
    class="mode-select"
  >
    <button
      type="button"
      class="mode-select__trigger"
      :class="{
        'mode-select__trigger--edit': currentMode === 'edit',
        'mode-select__trigger--plan': currentMode === 'plan',
        'mode-select__trigger--yolo': currentMode === 'yolo',
      }"
      :aria-haspopup="'menu'"
      :aria-expanded="menuOpen"
      title="点击切换当前 session 的 Mode(Shift+Tab 循环)"
      @click="toggleMenu"
    >
      <span class="mode-select__label">{{ currentModeLabel }}</span>
      <Icon
        :name="menuOpen ? 'chevron-down' : 'chevron-up'"
        :size="10"
        class="mode-select__chevron"
      />
    </button>
    <Transition name="mode-select-popover">
      <div
        v-if="menuOpen"
        class="mode-select__menu"
        role="menu"
      >
        <button
          v-for="opt in modeOptions"
          :key="opt.value"
          type="button"
          class="mode-select__item"
          :class="{
            'mode-select__item--active': opt.value === currentMode,
          }"
          role="menuitem"
          @click="onModePick(opt.value)"
        >
          <span class="mode-select__item-name">
            <Icon :name="opt.icon" :size="14" />
            {{ opt.label }}
          </span>
          <span class="mode-select__item-desc">{{ opt.description }}</span>
          <span
            v-if="opt.value === currentMode"
            class="mode-select__item-check"
            aria-hidden="true"
          >●</span>
        </button>
      </div>
    </Transition>

    <!-- Yolo confirm modal — driven by the store's
         `pendingYoloConfirm` flag so both the popover and the
         Shift+Tab cycle can open it from one place. The modal
         calls our `onYoloConfirm` (await + toast on success)
         on confirm, `chatStore.cancelYolo()` on cancel. The
         `:disabled` prop is left unset so streaming-state
         clicks can still open the modal and confirm. -->
    <YoloConfirmModal
      :open="chatStore.pendingYoloConfirm"
      @cancel="chatStore.cancelYolo()"
      @confirm="onYoloConfirm"
    />
  </div>
</template>

<style scoped>
/* Hand-written popover mirroring `ModelSelect`. The trigger sits
   at the bottom of the chat input row, so the popover opens
   UPWARD (matches `ModelSelect`'s geometry). Width matches the
   longest entry's label+description pair. */
.mode-select {
  position: relative;
  display: inline-flex;
}

.mode-select__trigger {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 3px 8px;
  background: transparent;
  border: 1px solid transparent;
  border-radius: var(--radius-md);
  color: var(--color-text-secondary);
  cursor: pointer;
  font: inherit;
  font-family: var(--font-mono);
  /* 3 档化 2026-06-13: trigger font bumped 11px → 13px so the
     mode label reads at the same scale as the textarea text
     (14px) instead of looking like a tiny status chip. */
  font-size: var(--text-base);
  font-weight: var(--weight-medium);
  max-width: 120px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  transition: background var(--duration-fast) var(--ease-out), color var(--duration-fast) var(--ease-out), border-color var(--duration-fast) var(--ease-out);
}

.mode-select__trigger:hover:not(:disabled) {
  background: var(--color-bg-elevated);
  border-color: var(--color-bg-border);
  color: var(--color-text-primary);
}

/* 3 档 mode 用设计 token 区分颜色 (3 档化 2026-06-13):
   - edit (默认， full power) → 蓝色 accent
   - plan (read-only, safe)   → 青色 tool-read
   - yolo (no-ask, 危险)      → 红色 tool-error
   hover 态不变， 仍走 text-primary; mode color 主要用于 idle 态。 */
.mode-select__trigger--edit {
  color: var(--color-accent);
}
.mode-select__trigger--plan {
  color: var(--color-tool-read);
}
.mode-select__trigger--yolo {
  color: var(--color-tool-error);
}

.mode-select__label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.mode-select__chevron {
  flex-shrink: 0;
  opacity: 0.6;
}

.mode-select__menu {
  position: absolute;
  bottom: calc(100% + 4px);
  top: auto;
  right: 0;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
  min-width: 220px;
  z-index: 100;
  padding: 4px;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.mode-select__item {
  display: grid;
  grid-template-columns: auto 1fr auto;
  align-items: baseline;
  column-gap: 8px;
  row-gap: 1px;
  padding: 6px 8px;
  background: transparent;
  border: 0;
  color: var(--color-text-primary);
  font: inherit;
  font-family: var(--font-sans);
  font-size: var(--text-sm);
  text-align: left;
  cursor: pointer;
  border-radius: var(--radius-sm);
}

.mode-select__item:hover:not(:disabled) {
  background: var(--color-bg-elevated);
}

.mode-select__item--active {
  color: var(--color-accent);
  font-weight: var(--weight-medium);
}

.mode-select__item-name {
  font-family: var(--font-mono);
  font-size: var(--text-sm);
  font-weight: var(--weight-semibold);
}

.mode-select__item-desc {
  color: var(--color-text-muted);
  font-size: 10px;
  line-height: 1.4;
  overflow: hidden;
  text-overflow: ellipsis;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
}

.mode-select__item--active .mode-select__item-desc {
  color: var(--color-text-secondary);
}

.mode-select__item-check {
  color: var(--color-accent);
  font-size: 10px;
  flex-shrink: 0;
  align-self: center;
}

/* Upward-opening popover slide animation — same shape as
   `ModelSelect`'s upward transition (see popover-pattern.md
   "Popover: fade + slide (direction matches position)"). */
.mode-select-popover-enter-active,
.mode-select-popover-leave-active {
  transition: opacity var(--duration-base) var(--ease-out), transform var(--duration-base) var(--ease-out);
  transform-origin: bottom right;
}

.mode-select-popover-enter-from,
.mode-select-popover-leave-to {
  opacity: 0;
  transform: translateY(4px);
}

.mode-select-popover-leave-active {
  transition-duration: 100ms;
  transition-timing-function: ease-in;
}
</style>