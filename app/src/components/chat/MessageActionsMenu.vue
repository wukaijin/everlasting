<script setup lang="ts">
// MessageActionsMenu — hover-triggered dropdown with Edit / Resend / Copy
// items, one per chat message. Renders a small ⋯ button on hover
// (or always on touch) and opens a reka-ui DropdownMenu on click.
//
// D3 PR2 (2026-06-17): UI half of the session message edit / resend
// feature. PR1 landed the backend `edit_user_message` IPC; PR2 wires
// the hover menu + the Edit flow on the frontend. Resend stays
// disabled with a "PR3 待实施" tooltip — the actual resend pipeline
// needs a new `ChatEvent::Resend` variant + the user-facing
// "重发当前 prompt" UX decisions, both of which are out of PR2 scope
// per the dispatch prompt.
//
// Why reka-ui DropdownMenu (not the hand-rolled popover pattern):
//   The hand-rolled `ModelSelect` / `ModeSelect` / `TriggerMenu` pattern
//   in `.trellis/spec/frontend/popover-pattern.md` works for chip-
//   attached dropdowns where the trigger is a stable UI element. The
//   message-hover ⋯ button is per-row ephemeral (appears on hover, hides
//   on leave) — reka-ui's `DropdownMenu` gives us the keyboard arrow /
//   Esc / focus-return a11y out of the box, and its `DropdownMenuTrigger`
//   `as-child` mode lets us style the existing button without wrapping
//   in a new element. Trade-off (acknowledged in `popover-pattern.md`):
//   we now have two popover implementations in the codebase; future
//   work could extract `usePopover`.
//
// Disabled-state rules (A1 + A7 + safety):
//   - Edit: enabled only when `role === "user"` AND `!isEditing` AND
//     `!isStreaming`. Editing an assistant message is intentionally
//     NOT supported (D3 PR1 `Don't edit assistant messages` — assistant
//     `tool_use` blocks have stable `tool_use_id`s that downstream
//     `tool_result` rows reference; mutating the assistant content
//     without rewriting the dependent tool_results would produce
//     orphan-request / orphan-result pairs and Anthropic 400s).
//   - Resend: always disabled in PR2. The tooltip says "PR3 待实施".
//     Backend work (new `ChatEvent::Resend` variant + audit kind)
//     lands in PR3.
//   - Copy: always enabled. Uses `navigator.clipboard.writeText` and
//     surfaces a "已复制" toast via `projectsStore.showToast`.
//   - Whole trigger is disabled when `isStreaming` is true (defense
//     against mid-stream edits racing the LLM).
//
// Wiring:
//   - Parent (`MessageItem.vue`) passes `messageSeq`, `sessionId`,
//     `content`, `role`, `isEditing`, `isStreaming`.
//   - `edit` emit bubbles to the parent which flips the message into
//     edit mode (textarea + Save / Cancel).
//   - `copy` is handled in-place (clipboard API + toast); no bubble
//     needed for the common case.

import {
  DropdownMenuRoot,
  DropdownMenuTrigger,
  DropdownMenuPortal,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  TooltipProvider,
  TooltipRoot,
  TooltipTrigger,
  TooltipPortal,
  TooltipContent,
} from "reka-ui";

import Icon from "../Icon.vue";
import { useProjectsStore } from "../../stores/projects";

const props = withDefaults(
  defineProps<{
    /** Per-message seq used for the Edit IPC. Passed up by the
     *  parent so the message row's seq (from the DB) is the same
     *  handle the backend `edit_user_message` uses. */
    messageSeq: number;
    /** Current session id (for the Edit IPC + the Edit tool's
     *  copy-to-clipboard scope if the user clicks Copy). */
    sessionId: string;
    /** The plain-text content of the message. The Copy action
     *  serializes this to the clipboard; the Edit action
     *  pre-fills the textarea with this. */
    content: string;
    /** Role gates Edit (user-only) and Resend (also user-only in
     *  the D3 design — assistant resend is undefined; the model
     *  can be re-prompted via a new user turn). */
    role: "user" | "assistant";
    /** True while the parent MessageItem is showing the inline
     *  edit textarea. We hide the menu trigger when this is set
     *  so the user can't open the menu mid-edit. */
    isEditing: boolean;
    /** True while a chat stream is in-flight on this session.
     *  Disables the entire menu trigger — the only safe action
     *  during streaming is Stop (which is in the chat input, not
     *  the message row). */
    isStreaming: boolean;
  }>(),
  {
    isEditing: false,
    isStreaming: false,
  },
);

const emit = defineEmits<{
  /** Parent should enter edit mode for this message row. The
   *  parent owns the local `editingMessageSeq` state and the
   *  textarea / Save / Cancel UI; this component only fires the
   *  intent. */
  edit: [messageSeq: number];
}>();

const projectsStore = useProjectsStore();

/** Edit is enabled when the message is a user turn AND no
 *  edit is in progress AND the session isn't streaming. The
 *  streaming guard is also applied at the trigger level, so
 *  this computed exists mostly for the `data-disabled` prop
 *  reka-ui reads to grey out the item. */
const canEdit = (): boolean =>
  props.role === "user" && !props.isEditing && !props.isStreaming;

/** Resend is permanently disabled in PR2. The handler is a
 *  no-op so a future PR3 (or a stray click) cannot trigger
 *  an undefined resend path. */
const canResend = (): boolean => false;

/** Copy is always enabled. */
const canCopy = (): boolean => true;

function onEdit() {
  if (!canEdit()) return;
  emit("edit", props.messageSeq);
}

async function onCopy() {
  if (!canCopy()) return;
  // `navigator.clipboard` is available in the Tauri webview
  // (WebKitGTK + wry); the fallback below covers the rare
  // case where the API isn't exposed (older WebKit / dev
  // tooling without permissions).
  try {
    if (navigator.clipboard && typeof navigator.clipboard.writeText === "function") {
      await navigator.clipboard.writeText(props.content);
    } else {
      // Last-resort fallback: a hidden textarea + execCommand.
      // execCommand is deprecated but still works in WebKitGTK
      // and is the only path when the async Clipboard API
      // is unavailable.
      const ta = document.createElement("textarea");
      ta.value = props.content;
      ta.setAttribute("readonly", "");
      ta.style.position = "fixed";
      ta.style.left = "-9999px";
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      document.body.removeChild(ta);
    }
    projectsStore.showToast("已复制", "info", 1800);
  } catch (e) {
    // Some browser contexts reject clipboard writes that
    // didn't originate from a user gesture — show a toast so
    // the user knows what happened instead of silently failing.
    projectsStore.showToast(
      `复制失败: ${String(e)}`,
      "error",
    );
  }
}
</script>

<template>
  <div
    class="msg-actions"
    :class="{
      'msg-actions--editing': isEditing,
      'msg-actions--streaming': isStreaming,
    }"
  >
    <DropdownMenuRoot>
      <TooltipProvider>
        <TooltipRoot :delay-duration="300">
          <TooltipTrigger as-child>
            <DropdownMenuTrigger
              as-child
              :disabled="isEditing || isStreaming"
            >
              <button
                type="button"
                class="msg-actions__trigger"
                :aria-label="isStreaming ? '流式生成中,无法操作' : '消息操作'"
                data-testid="msg-actions-trigger"
                @click.stop
              >
                <Icon
                  name="ellipsis"
                  :size="16"
                  icon-class="msg-actions__icon"
                />
              </button>
            </DropdownMenuTrigger>
          </TooltipTrigger>
          <TooltipPortal>
            <TooltipContent
              class="msg-actions__tooltip"
              :side-offset="6"
            >
              <span v-if="isStreaming">流式生成中</span>
              <span v-else-if="isEditing">编辑中</span>
              <span v-else>消息操作</span>
            </TooltipContent>
          </TooltipPortal>
        </TooltipRoot>
      </TooltipProvider>
      <DropdownMenuPortal>
        <DropdownMenuContent
          class="msg-actions__content"
          :side-offset="4"
          align="end"
        >
          <DropdownMenuItem
            class="msg-actions__item"
            :disabled="!canEdit()"
            data-testid="msg-actions-edit"
            @select="onEdit"
          >
            <Icon
              name="pencil"
              :size="14"
              icon-class="msg-actions__item-icon"
            />
            <span>编辑</span>
            <span
              v-if="role !== 'user'"
              class="msg-actions__item-hint"
            >仅 user 消息</span>
          </DropdownMenuItem>

          <DropdownMenuItem
            class="msg-actions__item"
            :disabled="!canResend()"
            data-testid="msg-actions-resend"
            @select.prevent
          >
            <Icon
              name="refresh"
              :size="14"
              icon-class="msg-actions__item-icon"
            />
            <span>重发</span>
            <span class="msg-actions__item-hint">PR3 待实施</span>
          </DropdownMenuItem>

          <DropdownMenuSeparator class="msg-actions__separator" />

          <DropdownMenuItem
            class="msg-actions__item"
            :disabled="!canCopy()"
            data-testid="msg-actions-copy"
            @select="onCopy"
          >
            <Icon
              name="copy"
              :size="14"
              icon-class="msg-actions__item-icon"
            />
            <span>复制</span>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenuPortal>
    </DropdownMenuRoot>
  </div>
</template>

<style scoped>
/* The trigger is positioned absolutely at the top-right of the
   parent .msg <li>; the parent sets `position: relative` on the
   <li> via the existing layout. Opacity transitions make the
   hover-in / hover-out feel natural; the button stays focusable
   even when invisible (keyboard users can tab to it). The parent
   `.msg:hover` (in MessageItem.vue) flips the opacity to 1, so
   the button becomes visible when the user hovers anywhere on
   the row — not just on the button itself. We also keep
   `:focus-within` here for keyboard focus arriving directly
   on the wrapper. */
.msg-actions {
  position: absolute;
  top: -8px;
  right: 4px;
  z-index: 5;
  opacity: 0;
  transition: opacity 0.12s ease-out;
}

/* The parent .msg <li> drives the hover-in. We deliberately
   do NOT bind :hover on .msg-actions itself because the
   button is only 22px wide — a mouse user who moves from
   the row onto the button would briefly cross the gap and
   the `:hover` on .msg-actions would flicker. */
.msg-actions:focus-within {
  opacity: 1;
}

/* When the user is editing or streaming, the trigger should
   stay hidden (the hover affordance would be misleading).
   Streaming also disables the button via :disabled, but the
   opacity rule keeps the visual quiet. */
.msg-actions--editing,
.msg-actions--streaming {
  opacity: 0 !important;
  pointer-events: none;
}

.msg-actions__trigger {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  padding: 0;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 4px;
  color: var(--color-text-secondary);
  cursor: pointer;
  transition: background 0.1s, color 0.1s, border-color 0.1s;
  outline: none;
}

.msg-actions__trigger:hover {
  background: var(--color-bg-surface);
  color: var(--color-text-primary);
  border-color: var(--color-text-muted);
}

.msg-actions__trigger:focus-visible {
  border-color: var(--color-accent);
  box-shadow: 0 0 0 2px color-mix(in srgb, var(--color-accent) 25%, transparent);
}

.msg-actions__trigger:disabled {
  cursor: not-allowed;
  opacity: 0.5;
}

.msg-actions__icon {
  display: inline-flex;
}

/* Tooltip content (reka-ui `TooltipContent` portal to body —
   must use :deep() per `.trellis/spec/frontend/reka-ui-usage.md`
   gotcha). */
:deep(.msg-actions__tooltip) {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 4px;
  padding: 4px 8px;
  font-size: 11px;
  color: var(--color-text-primary);
  z-index: 3000;
  box-shadow: 0 2px 6px rgba(0, 0, 0, 0.3);
}

/* Dropdown content (reka-ui `DropdownMenuContent` portal to body —
   :deep() required for the same reason as the tooltip). */
:deep(.msg-actions__content) {
  min-width: 160px;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
  padding: 4px;
  z-index: 3000;
  font-size: 13px;
  color: var(--color-text-primary);
  /* reka-ui 2.9.9 default opening animation; ~120ms ease-out. */
  animation: msg-actions-content-enter 120ms ease-out;
}

@keyframes msg-actions-content-enter {
  from {
    opacity: 0;
    transform: translateY(-2px);
  }
  to {
    opacity: 1;
    transform: translateY(0);
  }
}

:deep(.msg-actions__item) {
  display: grid;
  grid-template-columns: 16px 1fr auto;
  align-items: center;
  column-gap: 8px;
  padding: 6px 8px;
  border-radius: 4px;
  font-size: 13px;
  line-height: 1.4;
  color: var(--color-text-primary);
  cursor: pointer;
  user-select: none;
  outline: none;
}

:deep(.msg-actions__item)[data-highlighted] {
  background: var(--color-bg-elevated);
  /* Keep the row's left padding to avoid visual shift; the
     box-shadow is cheaper than `margin-left` and doesn't
     fight the focus state. */
}

:deep(.msg-actions__item)[data-disabled] {
  color: var(--color-text-muted);
  cursor: not-allowed;
}

:deep(.msg-actions__item-icon) {
  display: inline-flex;
  color: var(--color-text-secondary);
}

:deep(.msg-actions__item)[data-disabled] :deep(.msg-actions__item-icon) {
  color: var(--color-text-muted);
}

:deep(.msg-actions__item-hint) {
  font-size: 10px;
  color: var(--color-text-muted);
  font-family: var(--font-mono);
  margin-left: 4px;
}

:deep(.msg-actions__separator) {
  height: 1px;
  background: var(--color-bg-border);
  margin: 4px 2px;
}
</style>
