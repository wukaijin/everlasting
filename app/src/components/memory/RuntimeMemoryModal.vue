<script setup lang="ts">
// RuntimeMemoryModal — detail + management modal for a single
// autonomous (runtime) memory row.
//
// 07-06 (am-observability-panel B3/R1+R3+R4+R5). Opened by the
// host of `MemoryPreview` when the user clicks a runtime row (the
// row emits `manage(id)`; the host resolves the row + binds this
// modal's `:memory` prop). Distinct from `MemoryModal.vue`, which
// is the project-instruction-file editor (the 4 fixed CLAUDE.md /
// AGENTS.md files). This modal deals with the P2 autonomous rows
// the agent wrote via `remember` / P4 auto-reflect — and lets the
// user OBSERVE (stats), MANAGE (status transitions), and EDIT
// (title + content) them.
//
// Surface (design D5/D6/D7):
//   - Stats grid: hitCount / lastUsedAt / confidence / source
//     provenance (sourceSessionId + sourceRef) + createdAt +
//     updatedAt + editedByUser badge (AC2).
//   - Editable title <input> + content <textarea>, gated behind an
//     "编辑" toggle (read-only by default → flips to editable so a
//     stray click can't corrupt the row). Save calls
//     `store.updateMemory` (optimistic + IPC + rollback).
//   - Status dropdown: options computed from
//     `LEGAL_STATUS_TRANSITIONS[current]` (the P5 matrix, design
//     D6). Backend re-validates; this store is the optimistic
//     layer. Transitioning INTO demoted prompts a free-text reason
//     via an inline input (the matrix only accepts a reason on
//     →demoted edges).
//   - Delete: reuses ConfirmDialog (mirrors MemoryPreview's delete
//     flow), calls `store.deleteMemory`.
//
// Nesting: this modal is rendered inside ChatPanel's MemoryModal
// (which is itself a reka-ui Dialog). reka-ui 2.9.9 supports
// nested Dialogs — the focus trap moves to the innermost open
// dialog. Both use DialogPortal (teleport to <body>), so they
// stack via z-index rather than DOM nesting.

import { computed, ref, watch } from "vue";
import {
  DialogRoot,
  DialogPortal,
  DialogOverlay,
  DialogContent,
  DialogTitle,
  DialogClose,
  SelectRoot,
  SelectTrigger,
  SelectValue,
  SelectIcon,
  SelectPortal,
  SelectContent,
  SelectViewport,
  SelectItem,
  SelectItemText,
} from "reka-ui";
import {
  useMemoryStore,
  LEGAL_STATUS_TRANSITIONS,
  type AutonomousMemory,
} from "../../stores/memory";
import Icon from "../Icon.vue";
import ConfirmDialog from "../common/ConfirmDialog.vue";

const props = defineProps<{
  /** The memory row to render. `null` renders nothing (the host
   *  guards the modal open state on this being non-null, but we
   *  double-guard here so a template race can't deref null). */
  memory: AutonomousMemory | null;
}>();

const open = defineModel<boolean>("open", { required: true });

const store = useMemoryStore();

// ---------------------------------------------------------------------
// Edit mode (title + content)
// ---------------------------------------------------------------------
// Read-only by default — the modal opens in an "observe" posture so
// a stray click can't corrupt the row. The "编辑" button flips
// `editing` to true, which enables the inputs + reveals the
// save/cancel row. Cancelling restores the draft from the live row.
const editing = ref<boolean>(false);
const draftTitle = ref<string>("");
const draftContent = ref<string>("");

function startEdit() {
  if (!props.memory) return;
  draftTitle.value = props.memory.title;
  draftContent.value = props.memory.content;
  editing.value = true;
}

function cancelEdit() {
  editing.value = false;
}

const saving = ref<boolean>(false);

async function saveEdit() {
  if (!props.memory || saving.value) return;
  saving.value = true;
  try {
    const ok = await store.updateMemory(
      props.memory.id,
      draftTitle.value,
      draftContent.value,
    );
    if (ok) editing.value = false;
    // On failure the store sets `runtimeMemoriesError`; the banner
    // below renders it. Stay in edit mode so the user can retry.
  } finally {
    saving.value = false;
  }
}

// ---------------------------------------------------------------------
// Status transition (P5 matrix)
// ---------------------------------------------------------------------
// Legal targets come from the static `LEGAL_STATUS_TRANSITIONS`
// map (mirrors the Rust matrix). The dropdown v-model is a local
// ref; on change we fire `store.updateMemoryStatus` (optimistic +
// IPC). We do NOT pre-validate legality here beyond the dropdown
// only OFFERING legal targets — the backend matrix is the hard
// wall and the store rolls back on rejection.
const legalTargets = computed<string[]>(() => {
  if (!props.memory) return [];
  return LEGAL_STATUS_TRANSITIONS[props.memory.status] ?? [];
});

const STATUS_LABELS: Record<string, string> = {
  candidate: "candidate",
  active: "active",
  verified: "verified",
  demoted: "demoted",
};

// Transitioning INTO demoted requires a reason (matrix rule). We
// prompt for it via an inline input that appears when the user
// picks "demoted" from the dropdown, BEFORE firing the IPC.
const pendingDemote = ref<boolean>(false);
const demoteReason = ref<string>("");

function onStatusChange(next: string) {
  if (!props.memory) return;
  if (next === "demoted") {
    // Prompt for a reason first; the actual IPC fires in
    // `confirmDemote`.
    pendingDemote.value = true;
    demoteReason.value = props.memory.demotedReason ?? "";
    return;
  }
  void store.updateMemoryStatus(props.memory.id, next);
}

function cancelDemote() {
  pendingDemote.value = false;
}

async function confirmDemote() {
  if (!props.memory) return;
  pendingDemote.value = false;
  await store.updateMemoryStatus(
    props.memory.id,
    "demoted",
    demoteReason.value.trim() || null,
  );
}

// ---------------------------------------------------------------------
// Delete (reuse ConfirmDialog — mirrors MemoryPreview's flow)
// ---------------------------------------------------------------------
const deleteOpen = ref<boolean>(false);

async function onDeleteConfirm() {
  deleteOpen.value = false;
  if (!props.memory) return;
  await store.deleteMemory(props.memory.id);
  // Closing the parent modal on delete is the host's call; we just
  // fire the IPC. The row vanishes from `runtimeMemories`, so if
  // the host keeps the modal open the props go stale — the host
  // should watch for `memory` becoming null and close. We close
  // defensively here too.
  open.value = false;
}

// ---------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------
function formatTimestamp(rfc3339: string | null): string {
  if (!rfc3339) return "—";
  // RFC 3339 example: "2026-06-29T12:34:56.789+00:00" → "2026-06-29 12:34"
  const m = rfc3339.match(/^(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2})/);
  return m ? `${m[1]} ${m[2]}` : rfc3339;
}

function kindLabel(kind: string): string {
  return kind;
}

function scopeLabel(scope: string): string {
  return scope === "user" ? "user" : "project";
}

// Reset transient state when the modal closes or the row changes —
// otherwise a stale `editing` draft from row A could leak into row B
// on reopen.
watch(
  () => [open.value, props.memory?.id],
  () => {
    if (!open.value || !props.memory) {
      editing.value = false;
      pendingDemote.value = false;
      deleteOpen.value = false;
    }
  },
);
</script>

<template>
  <DialogRoot v-model:open="open">
    <DialogPortal>
      <DialogOverlay class="runtime-memory-modal__overlay" />
      <DialogContent
        class="runtime-memory-modal"
        :aria-describedby="undefined"
        @pointerdown-outside="open = false"
      >
        <template v-if="memory">
          <header class="runtime-memory-modal__header">
            <div class="runtime-memory-modal__title-wrap">
              <DialogTitle class="runtime-memory-modal__title">
                {{ memory.title }}
              </DialogTitle>
              <span
                class="runtime-memory-modal__chip runtime-memory-modal__chip--kind"
                :class="`runtime-memory-modal__chip--kind-${memory.kind}`"
              >
                {{ kindLabel(memory.kind) }}
              </span>
              <span
                class="runtime-memory-modal__chip runtime-memory-modal__chip--scope"
              >
                {{ scopeLabel(memory.scope) }}
              </span>
              <span
                class="runtime-memory-modal__chip runtime-memory-modal__chip--status"
                :class="`runtime-memory-modal__chip--status-${memory.status}`"
              >
                {{ memory.status }}
              </span>
              <span
                v-if="memory.editedByUser"
                class="runtime-memory-modal__chip runtime-memory-modal__chip--edited"
                title="此记忆由人工编辑过"
              >
                人工编辑
              </span>
            </div>
            <DialogClose as-child>
              <button
                type="button"
                class="runtime-memory-modal__close"
                aria-label="关闭"
              >
                <Icon name="x" :size="14" />
              </button>
            </DialogClose>
          </header>

          <!-- Error banner: surfaces store.runtimeMemoriesError
               (status-transition / edit / delete failures). The
               store clears it on the next successful op. -->
          <p
            v-if="store.runtimeMemoriesError"
            class="runtime-memory-modal__error"
          >
            <Icon name="warn" :size="12" />
            {{ store.runtimeMemoriesError }}
          </p>

          <div class="runtime-memory-modal__body">
            <!-- Stats grid (AC2). Mono font + compact rows; reads as
                 a property sheet, not prose. -->
            <section class="runtime-memory-modal__stats">
              <div class="runtime-memory-modal__stat">
                <span class="runtime-memory-modal__stat-label">召回次数</span>
                <span class="runtime-memory-modal__stat-value">
                  {{ memory.hitCount }}
                </span>
              </div>
              <div class="runtime-memory-modal__stat">
                <span class="runtime-memory-modal__stat-label">上次召回</span>
                <span class="runtime-memory-modal__stat-value">
                  {{ formatTimestamp(memory.lastUsedAt) }}
                </span>
              </div>
              <div class="runtime-memory-modal__stat">
                <span class="runtime-memory-modal__stat-label">置信度</span>
                <span class="runtime-memory-modal__stat-value">
                  {{ (memory.confidence * 100).toFixed(0) }}%
                </span>
              </div>
              <div class="runtime-memory-modal__stat">
                <span class="runtime-memory-modal__stat-label">来源会话</span>
                <span class="runtime-memory-modal__stat-value">
                  {{ memory.sourceSessionId ?? "—" }}
                </span>
              </div>
              <div class="runtime-memory-modal__stat">
                <span class="runtime-memory-modal__stat-label">来源标记</span>
                <span class="runtime-memory-modal__stat-value">
                  {{ memory.sourceRef ?? "—" }}
                </span>
              </div>
              <div class="runtime-memory-modal__stat">
                <span class="runtime-memory-modal__stat-label">创建于</span>
                <span class="runtime-memory-modal__stat-value">
                  {{ formatTimestamp(memory.createdAt) }}
                </span>
              </div>
              <div class="runtime-memory-modal__stat">
                <span class="runtime-memory-modal__stat-label">更新于</span>
                <span class="runtime-memory-modal__stat-value">
                  {{ formatTimestamp(memory.updatedAt) }}
                </span>
              </div>
            </section>

            <!-- Status transition (D6 matrix). The Select only lists
                 legal targets; backend re-validates. -->
            <section class="runtime-memory-modal__field">
              <span class="runtime-memory-modal__label">状态</span>
              <SelectRoot
                :model-value="memory.status"
                @update:model-value="onStatusChange($event as string)"
              >
                <SelectTrigger
                  class="runtime-memory-modal__status-trigger"
                  aria-label="记忆状态"
                >
                  <SelectValue :placeholder="memory.status" />
                  <SelectIcon class="runtime-memory-modal__status-icon">
                    <Icon name="chevron-down" :size="12" />
                  </SelectIcon>
                </SelectTrigger>
                <SelectPortal>
                  <SelectContent
                    class="runtime-memory-modal__status-content"
                    position="popper"
                    :side-offset="4"
                  >
                    <SelectViewport
                      class="runtime-memory-modal__status-viewport"
                    >
                      <SelectItem
                        v-for="target in legalTargets"
                        :key="target"
                        :value="target"
                        class="runtime-memory-modal__status-option"
                      >
                        <SelectItemText>
                          {{ STATUS_LABELS[target] ?? target }}
                        </SelectItemText>
                      </SelectItem>
                    </SelectViewport>
                  </SelectContent>
                </SelectPortal>
              </SelectRoot>
              <p
                v-if="memory.status === 'demoted' && memory.demotedReason"
                class="runtime-memory-modal__demote-reason"
              >
                降级原因: {{ memory.demotedReason }}
              </p>
            </section>

            <!-- Demote-reason prompt (inline; appears when the user
                 picks "demoted" from the dropdown). -->
            <section
              v-if="pendingDemote"
              class="runtime-memory-modal__demote-prompt"
            >
              <span class="runtime-memory-modal__label">降级原因(可选)</span>
              <input
                v-model="demoteReason"
                type="text"
                class="runtime-memory-modal__input"
                placeholder="为什么降级这条记忆?"
              />
              <div class="runtime-memory-modal__demote-actions">
                <button
                  type="button"
                  class="runtime-memory-modal__btn runtime-memory-modal__btn--ghost"
                  @click="cancelDemote"
                >
                  取消
                </button>
                <button
                  type="button"
                  class="runtime-memory-modal__btn runtime-memory-modal__btn--primary"
                  @click="confirmDemote"
                >
                  确认降级
                </button>
              </div>
            </section>

            <!-- Editable title + content. Read-only until "编辑"
                 is clicked. -->
            <section class="runtime-memory-modal__field">
              <div class="runtime-memory-modal__field-head">
                <span class="runtime-memory-modal__label">标题</span>
                <button
                  v-if="!editing"
                  type="button"
                  class="runtime-memory-modal__btn runtime-memory-modal__btn--ghost runtime-memory-modal__btn--sm"
                  @click="startEdit"
                >
                  <Icon name="pencil" :size="11" />
                  编辑
                </button>
              </div>
              <input
                v-if="editing"
                v-model="draftTitle"
                type="text"
                class="runtime-memory-modal__input"
                :maxlength="200"
              />
              <p v-else class="runtime-memory-modal__readonly">
                {{ memory.title }}
              </p>
            </section>

            <section class="runtime-memory-modal__field">
              <span class="runtime-memory-modal__label">内容</span>
              <textarea
                v-if="editing"
                v-model="draftContent"
                class="runtime-memory-modal__textarea"
                rows="5"
                :maxlength="500"
              />
              <p v-else class="runtime-memory-modal__readonly runtime-memory-modal__readonly--content">
                {{ memory.content }}
              </p>
              <div v-if="editing" class="runtime-memory-modal__edit-actions">
                <button
                  type="button"
                  class="runtime-memory-modal__btn runtime-memory-modal__btn--ghost"
                  :disabled="saving"
                  @click="cancelEdit"
                >
                  取消
                </button>
                <button
                  type="button"
                  class="runtime-memory-modal__btn runtime-memory-modal__btn--primary"
                  :disabled="saving"
                  @click="saveEdit"
                >
                  <Icon name="check" :size="11" />
                  {{ saving ? "保存中…" : "保存" }}
                </button>
              </div>
            </section>

            <!-- Delete (ConfirmDialog portals to body; layers
                 correctly below this Dialog's z-index). -->
            <section class="runtime-memory-modal__danger">
              <button
                type="button"
                class="runtime-memory-modal__btn runtime-memory-modal__btn--danger"
                @click="deleteOpen = true"
              >
                <Icon name="trash" :size="11" />
                删除记忆
              </button>
            </section>
          </div>
        </template>

        <ConfirmDialog
          :open="deleteOpen"
          title="删除这条记忆?"
          variant="danger"
          confirm-text="删除"
          @cancel="deleteOpen = false"
          @confirm="onDeleteConfirm"
        >
          删除后无法撤销。确认删除「{{ memory?.title }}」?
        </ConfirmDialog>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>

<style scoped>
/* Scaffolding mirrors MemoryModal.vue (reka-ui Dialog 6-piece,
   portal-to-body, fixed center, 80vw/min640/max900/80vh). The
   scoped compiler keeps data-v-* on teleported children (Vue 3.5),
   so plain scoped selectors reach the portal content — see
   reka-ui-usage.md. Select portal children are the exception
   (nested render) and need :deep() below. */

.runtime-memory-modal__overlay {
  position: fixed;
  inset: 0;
  background: var(--color-backdrop, rgba(0, 0, 0, 0.4));
  z-index: 2000;
}

.runtime-memory-modal {
  position: fixed;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  width: 80vw;
  min-width: 640px;
  max-width: min(900px, calc(100vw - 40px));
  max-height: 80vh;
  display: flex;
  flex-direction: column;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-xl);
  /* 容器接 programmatic focus,整框上环无意义;内部控件由全局 :focus-visible 基线负责(style.css) */
  outline: none;
  z-index: 2001;
  animation: runtime-memory-modal-zoom var(--duration-modal-in, 160ms)
    var(--ease-modal-in, ease-out) both;
}
.runtime-memory-modal[data-state="closed"] {
  animation: runtime-memory-modal-zoom-out var(--duration-modal-out, 120ms)
    var(--ease-accelerate, ease-in) forwards;
}

@keyframes runtime-memory-modal-zoom {
  from {
    transform: translate(-50%, -50%) scale(0.1);
    opacity: 0;
  }
  to {
    transform: translate(-50%, -50%) scale(1);
    opacity: 1;
  }
}
@keyframes runtime-memory-modal-zoom-out {
  from {
    transform: translate(-50%, -50%) scale(1);
    opacity: 1;
  }
  to {
    transform: translate(-50%, -50%) scale(0.1);
    opacity: 0;
  }
}

.runtime-memory-modal__header {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  padding: 16px 20px 12px;
  border-bottom: 1px solid var(--color-bg-border);
}

.runtime-memory-modal__title-wrap {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
  min-width: 0;
}

.runtime-memory-modal__title {
  margin: 0;
  font-size: var(--text-base);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
}

.runtime-memory-modal__chip {
  display: inline-block;
  padding: 1px 6px;
  border-radius: 3px;
  font-size: var(--text-2xs);
  font-family: var(--font-mono);
  border: 1px solid var(--color-bg-border);
  background: var(--color-bg-surface);
  color: var(--color-text-muted);
  line-height: 1.4;
}
.runtime-memory-modal__chip--kind-pitfall {
  color: var(--color-tool-error-text);
  border-color: color-mix(in srgb, var(--color-tool-error) 40%, transparent);
}
.runtime-memory-modal__chip--kind-preference {
  color: var(--color-accent-text);
  border-color: color-mix(in srgb, var(--color-accent) 40%, transparent);
}
.runtime-memory-modal__chip--edited {
  color: var(--color-status-warn);
  border-color: color-mix(in srgb, var(--color-status-warn) 40%, transparent);
  background: color-mix(in srgb, var(--color-status-warn) 8%, transparent);
}

.runtime-memory-modal__close {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  border: none;
  background: transparent;
  color: var(--color-text-muted);
  border-radius: var(--radius-sm);
  cursor: pointer;
  transition: background var(--duration-base) var(--ease-out);
}
.runtime-memory-modal__close:hover {
  background: var(--color-bg-surface);
  color: var(--color-text-primary);
}

.runtime-memory-modal__error {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 0;
  padding: 8px 20px;
  font-size: var(--text-sm);
  color: var(--color-tool-error-text);
  background: color-mix(in srgb, var(--color-tool-error) 8%, transparent);
  border-bottom: 1px solid
    color-mix(in srgb, var(--color-tool-error) 20%, transparent);
}

.runtime-memory-modal__body {
  flex: 1;
  overflow-y: auto;
  min-height: 0;
  padding: 16px 20px;
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.runtime-memory-modal__stats {
  display: grid;
  grid-template-columns: repeat(2, 1fr);
  gap: 8px 16px;
  padding: 10px 12px;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
}

.runtime-memory-modal__stat {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}
.runtime-memory-modal__stat-label {
  /* 08-14 ux-polish-r1 WP2(评审 B3):统计标签常驻可见,10px → 11px。 */
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  font-family: var(--font-mono);
}
.runtime-memory-modal__stat-value {
  font-size: var(--text-sm);
  color: var(--color-text-primary);
  font-family: var(--font-mono);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.runtime-memory-modal__field {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.runtime-memory-modal__field-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
}
.runtime-memory-modal__label {
  font-size: var(--text-xs);
  font-weight: var(--weight-medium);
  color: var(--color-text-secondary);
}

.runtime-memory-modal__status-trigger {
  display: inline-flex;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
  width: 100%;
  padding: 6px 10px;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  font-size: var(--text-sm);
  font-family: var(--font-mono);
  color: var(--color-text-primary);
  cursor: pointer;
}
.runtime-memory-modal__status-trigger[data-state="open"] {
  border-color: var(--color-accent);
}
.runtime-memory-modal__status-icon {
  display: inline-flex;
  color: var(--color-text-muted);
}

/* Select portal children need :deep() — see reka-ui-usage.md. */
:deep(.runtime-memory-modal__status-content) {
  min-width: var(--reka-select-trigger-width, 240px);
  width: var(--reka-select-trigger-width);
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  box-shadow: var(--shadow-md);
  z-index: 3000 !important;
  padding: 4px;
}
:deep(.runtime-memory-modal__status-viewport) {
  /* 容器接 programmatic focus,整框上环无意义;内部控件由全局 :focus-visible 基线负责(style.css) */
  outline: none;
}
:deep(.runtime-memory-modal__status-option) {
  display: flex;
  align-items: center;
  padding: 6px 10px;
  border-radius: 3px;
  font-size: var(--text-sm);
  font-family: var(--font-mono);
  color: var(--color-text-primary);
  cursor: pointer;
}
:deep(.runtime-memory-modal__status-option[data-highlighted]) {
  background: var(--color-bg-surface);
}
:deep(.runtime-memory-modal__status-option[data-state="checked"]) {
  color: var(--color-accent-text);
}

.runtime-memory-modal__demote-reason {
  margin: 0;
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  font-style: italic;
}

.runtime-memory-modal__demote-prompt {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 10px 12px;
  background: color-mix(in srgb, var(--color-status-warn) 6%, transparent);
  border: 1px solid
    color-mix(in srgb, var(--color-status-warn) 30%, transparent);
  border-radius: var(--radius-md);
}
.runtime-memory-modal__demote-actions {
  display: flex;
  gap: 6px;
  justify-content: flex-end;
}

.runtime-memory-modal__input {
  width: 100%;
  padding: 6px 10px;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  font-size: var(--text-sm);
  color: var(--color-text-primary);
  /* 自有焦点替代::focus 换 accent 边框(见下),无需 UA 默认环 */
  outline: none;
}
.runtime-memory-modal__input:focus {
  border-color: var(--color-accent);
}

.runtime-memory-modal__textarea {
  width: 100%;
  padding: 8px 10px;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  font-size: var(--text-sm);
  color: var(--color-text-primary);
  resize: vertical;
  min-height: 80px;
  /* 自有焦点替代::focus 换 accent 边框(见下),无需 UA 默认环 */
  outline: none;
  font-family: inherit;
}
.runtime-memory-modal__textarea:focus {
  border-color: var(--color-accent);
}

.runtime-memory-modal__readonly {
  margin: 0;
  padding: 6px 10px;
  font-size: var(--text-sm);
  color: var(--color-text-primary);
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  word-break: break-word;
}
.runtime-memory-modal__readonly--content {
  white-space: pre-wrap;
  line-height: 1.5;
}

.runtime-memory-modal__edit-actions {
  display: flex;
  gap: 6px;
  justify-content: flex-end;
}

.runtime-memory-modal__danger {
  display: flex;
  justify-content: flex-end;
  padding-top: 8px;
  border-top: 1px solid var(--color-bg-border);
}

.runtime-memory-modal__btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 5px 12px;
  font-size: var(--text-sm);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  background: var(--color-bg-surface);
  color: var(--color-text-primary);
  cursor: pointer;
  transition: background var(--duration-base) var(--ease-out);
}
.runtime-memory-modal__btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.runtime-memory-modal__btn--sm {
  padding: 2px 8px;
  font-size: var(--text-xs);
}
.runtime-memory-modal__btn--ghost:hover:not(:disabled) {
  background: var(--color-bg-border);
}
.runtime-memory-modal__btn--primary {
  background: var(--color-accent);
  border-color: var(--color-accent);
  color: var(--color-text-on-accent, #fff);
}
.runtime-memory-modal__btn--primary:hover:not(:disabled) {
  filter: brightness(1.1);
}
.runtime-memory-modal__btn--danger {
  color: var(--color-tool-error-text);
  border-color: color-mix(in srgb, var(--color-tool-error) 40%, transparent);
}
.runtime-memory-modal__btn--danger:hover:not(:disabled) {
  background: color-mix(in srgb, var(--color-tool-error) 10%, transparent);
}
</style>
