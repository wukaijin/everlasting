<script setup lang="ts">
// PermissionGrantsModal — reka-ui Dialog wrapper for the
// permission-grant management UI (task 07-01-permission-grant-list-ui).
//
// Lists the current session's "always allow" grants (tool / path /
// prefix kinds) and lets the user revoke any single row by PK.
// Composition mirrors AuditLogModal.vue (same Dialog shell, same
// load-on-open watcher, same empty/loading/error body states) —
// only the filter row is dropped (grant lists are short; no kind
// filter needed for MVP).
//
// MVP scope: no filter, no live push. A manual 刷新 button covers
// the "modal open while the agent loop adds a new grant" edge case.
//
// Design D1 (immediate effect): revoking a row deletes it from the
// DB; the check path re-reads the DB on the next tool_use, so the
// revoke takes effect with no cache signal. The UI removes the row
// locally on success.
//
// BUGLIST CH7-4 (2026-08-29): D1's one-click immediacy is kept, but the
// click now routes through a ConfirmDialog first — grants are security
// posture and an accidental revoke re-prompts on the next tool_use,
// so the mistake cost is asymmetric. The dialog is rendered INSIDE
// DialogContent (RuntimeMemoryModal/MemoryPreview precedent): it's a
// DOM descendant of the z-modal stacking context, so its own
// --z-confirm backdrop still paints above the modal without any
// z-index override.

import { computed, ref, watch } from "vue";
import {
  DialogRoot,
  DialogPortal,
  DialogOverlay,
  DialogContent,
  DialogTitle,
  DialogClose,
} from "reka-ui";

import Icon from "../Icon.vue";
import ConfirmDialog from "../common/ConfirmDialog.vue";
import PermissionGrantItem from "./PermissionGrantItem.vue";
import {
  matchKindLabel,
  usePermissionGrantsStore,
  type PermissionGrantRow,
} from "../../stores/permissionGrants";
import { useChatStore } from "../../stores/chat";

const open = defineModel<boolean>("open", { required: true });

const store = usePermissionGrantsStore();
const chatStore = useChatStore();

/** The session this modal is bound to — the CURRENT session at
 *  open time (the entry button is `v-if`'d on `currentSessionId`).
 *  The ChatPanel watcher closes the modal on session switch, so
 *  this stays stable for one open. */
const boundSessionId = computed<string | null>(() => chatStore.currentSessionId);

const sessionTitle = computed<string>(() => {
  const sid = boundSessionId.value;
  if (!sid) return "当前会话";
  const s = chatStore.sessions.find((x) => x.id === sid);
  return s?.title?.trim() || "新对话";
});

/** Header layout (2026-09-05, mirrors AuditLogModal): the fixed
 *  label 「放行管理」 is the DialogTitle's primary text; the
 *  session title renders as a secondary chip inside the same
 *  DialogTitle. Replaces the old 「放行管理 — 会话名」 one-line
 *  cram that ellipsized long session names. */

/** Reactively re-load whenever the modal transitions to open. */
watch(
  () => open.value,
  (isOpen) => {
    if (!isOpen) return;
    const sid = boundSessionId.value;
    if (!sid) return;
    void store.loadForSession(sid);
  },
);

async function onRefresh(): Promise<void> {
  await store.refresh();
}

/** CH7-4: the row awaiting revoke confirmation. `null` = dialog
 *  closed; `onRevoke` (the item's 撤销 click) only STAGES the row
 *  here — the actual `store.revoke` fires from the dialog's
 *  confirm handler. Cancel / Esc / backdrop all just null it out
 *  with zero side effects. */
const revokeTarget = ref<PermissionGrantRow | null>(null);

function onRevoke(row: PermissionGrantRow): void {
  revokeTarget.value = row;
}

async function onRevokeConfirm(): Promise<void> {
  const row = revokeTarget.value;
  revokeTarget.value = null;
  if (row) await store.revoke(row);
}
</script>

<template>
  <DialogRoot v-model:open="open">
    <DialogPortal>
      <DialogOverlay class="grant-modal__overlay" />
      <DialogContent
        class="grant-modal"
        :aria-describedby="undefined"
        @pointerdown-outside="open = false"
      >
        <header class="grant-modal__header">
          <DialogTitle class="grant-modal__title">
            <span class="grant-modal__title-label">放行管理</span>
            <span class="grant-modal__title-session">{{
              sessionTitle
            }}</span>
          </DialogTitle>
          <DialogClose as-child>
            <button type="button" class="grant-modal__close btn btn--icon btn--ghost" aria-label="Close">
              <Icon name="x" :size="14" />
            </button>
          </DialogClose>
        </header>

        <div class="grant-modal__toolbar">
          <span class="grant-modal__count">{{ store.grants.length }} 项放行</span>
          <button
            type="button"
            class="grant-modal__refresh btn btn--muted btn--sm"
            :disabled="store.loading"
            title="刷新"
            @click="onRefresh"
          >
            <Icon name="refresh" :size="12" />
            <span v-if="store.loading">加载中…</span>
            <span v-else>刷新</span>
          </button>
        </div>

        <div class="grant-modal__body">
          <div v-if="store.error" class="grant-modal__error">
            操作失败: {{ store.error }}
          </div>

          <div
            v-else-if="store.loading && store.grants.length === 0"
            class="grant-modal__placeholder"
          >
            <span class="app-spinner app-spinner--sm" aria-hidden="true" />
            <p class="grant-modal__placeholder-title">正在加载放行记录…</p>
          </div>

          <div
            v-else-if="store.grants.length === 0"
            class="grant-modal__placeholder"
          >
            <span class="grant-modal__placeholder-icon" aria-hidden="true">
              <Icon name="key" :size="20" />
            </span>
            <p class="grant-modal__placeholder-title">
              当前会话暂无「始终允许」放行
            </p>
            <p class="grant-modal__placeholder-hint">
              审批弹窗里勾选「始终允许」的放行会列在这里,可随时撤销
            </p>
          </div>

          <ul v-else class="grant-modal__list">
            <PermissionGrantItem
              v-for="row in store.grants"
              :key="`${row.toolName}|${row.matchKind}|${row.matchValue ?? ''}`"
              :row="row"
              @revoke="onRevoke"
            />
          </ul>
        </div>

        <!-- CH7-4 revoke confirmation. Rendered inside DialogContent
             (stacking-context precedent, see file header). The body
             mirrors the row's own presentation (kind label via the
             shared matchKindLabel + tool name + match value). -->
        <ConfirmDialog
          :open="revokeTarget !== null"
          title="撤销此放行?"
          variant="danger"
          confirm-text="撤销"
          data-testid="grant-revoke-confirm"
          @cancel="revokeTarget = null"
          @confirm="onRevokeConfirm"
        >
          <template v-if="revokeTarget">
            确认撤销「{{ matchKindLabel(revokeTarget.matchKind) }} ·
            {{ revokeTarget.toolName }}{{
              revokeTarget.matchValue ? ` ${revokeTarget.matchValue}` : ""
            }}」?
            撤销后下一个工具调用将重新询问。
          </template>
        </ConfirmDialog>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>

<style scoped>
/* reka-ui DialogPortal teleports the overlay + content to <body>.
 * Vue 3.5 keeps `data-v-*` on Teleport children (verified by
 * SettingsModal.vue / MemoryModal.vue / AuditLogModal.vue), so the
 * rules below are NOT wrapped in `:deep()`. If a future Vue upgrade
 * breaks that, wrap in `:deep(...)` per
 * `.trellis/spec/frontend/reka-ui-usage.md`. */

.grant-modal__overlay {
  position: fixed;
  inset: 0;
  background: color-mix(in srgb, var(--color-bg-app) 70%, transparent);
  backdrop-filter: blur(4px);
  z-index: var(--z-modal-overlay);
}

.grant-modal {
  position: fixed;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  width: 80vw;
  min-width: 560px;
  max-width: min(880px, calc(100vw - 40px));
  /* Height hugs content (2026-09-05, mirrors AuditLogModal): the
     old min-height: 360px manufactured a dead zone under short
     lists / the empty state. The flex column absorbs extra height
     in `.grant-modal__body` (flex: 1, min-height: 0) only past
     80vh; the empty state carries its own vertical presence via
     the placeholder padding. */
  max-height: 80vh;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-lg);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  box-shadow: var(--shadow-xl);
  z-index: var(--z-modal);
  /* 容器接 programmatic focus,整框上环无意义;内部控件由全局 :focus-visible 基线负责(style.css) */
  outline: none;
  animation: grant-modal-zoom var(--duration-modal-in) var(--ease-modal-in) both;
}

.grant-modal[data-state="closed"] {
  animation: grant-modal-zoom-out var(--duration-modal-out) var(--ease-accelerate) forwards;
}

@keyframes grant-modal-zoom {
  from { opacity: 0; transform: translate(-50%, -50%) scale(0.1); }
  to { opacity: 1; transform: translate(-50%, -50%) scale(1); }
}

@keyframes grant-modal-zoom-out {
  from { opacity: 1; transform: translate(-50%, -50%) scale(1); }
  to { opacity: 0; transform: translate(-50%, -50%) scale(0.1); }
}

.grant-modal__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 16px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  flex-shrink: 0;
}

.grant-modal__title {
  margin: 0;
  display: inline-flex;
  align-items: center;
  gap: 8px;
  min-width: 0; /* let the session chip, not the header, absorb overflow */
  font-size: var(--text-base);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
}

.grant-modal__title-label {
  flex-shrink: 0;
}

/* Session chip: mirrors .audit-modal__title-session (2026-09-05). */
.grant-modal__title-session {
  font-size: var(--text-xs);
  font-weight: var(--weight-regular);
  color: var(--color-text-secondary);
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-pill);
  padding: 2px 10px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
}

/* 按钮样式由全局 .btn 家族承载(close = ghost icon);flex 几何保留。 */
.grant-modal__close {
  flex-shrink: 0;
}

.grant-modal__toolbar {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 10px 16px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-app);
  flex-shrink: 0;
}

.grant-modal__count {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--color-text-muted);
}

/* 按钮样式由全局 .btn 家族承载(refresh = muted·sm,hover 转 accent
   与原语义一致);margin 几何保留。 */
.grant-modal__refresh {
  margin-left: auto;
}

.grant-modal__body {
  flex: 1;
  overflow-y: auto;
  background: var(--color-bg-app);
  min-height: 0;
}

.grant-modal__error {
  padding: var(--space-4);
  color: var(--color-tool-error-text);
  font-size: var(--text-sm);
  text-align: center;
}

/* Empty / loading states (2026-09-05): composed tile + title +
   hint column, mirrors .audit-modal__placeholder. With the
   modal's min-height floor gone, this padding IS the short-state
   vertical presence. */
.grant-modal__placeholder {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-7) var(--space-4);
  text-align: center;
}

.grant-modal__placeholder-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 40px; /* icon tile: 20px glyph + 10px cushion each side */
  height: 40px;
  margin-bottom: var(--space-1);
  border-radius: var(--radius-lg);
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  color: var(--color-text-muted);
}

.grant-modal__placeholder-title {
  margin: 0;
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
}

.grant-modal__placeholder-hint {
  margin: 0;
  font-size: var(--text-xs);
  color: var(--color-text-muted);
}

.grant-modal__list {
  list-style: none;
  margin: 0;
  padding: 0;
}
</style>
