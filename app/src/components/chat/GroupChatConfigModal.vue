<script setup lang="ts">
// GroupChatConfigModal — create / edit modal for a group_chat
// session's participant roster.
//
// 07-29-group-chat (Phase 4 Step 3 TODO-E6): serves BOTH
// the create-session flow (from SessionList's "新建群聊"
// button) AND the runtime re-edit flow (opened from the
// chat header). Same component, two modes:
//   - mode: "create" → calls `createNewSession` with the
//     new roster; closes on success.
//   - mode: "edit" → calls `updateGroupChatConfig` for the
//     given sessionId; closes on success.
//
// MVP boundary (D5): 2-3 participants. "+" button hidden
// at 3 participants; delete button disabled when only 2
// remain.
//
// Each participant row has:
//   - name (text input, must be unique within the session)
//   - model (Select dropdown from modelsStore.models)
//   - persona_md (textarea, optional)
//
// Reka-ui Dialog primitives (consistent with RuntimeMemoryModal
// nesting pattern). Models select pulled from modelsStore —
// the catalog is loaded at app startup so the list is hot.

import { computed, ref, watch } from "vue";
import {
  DialogRoot,
  DialogPortal,
  DialogOverlay,
  DialogContent,
  DialogTitle,
  DialogDescription,
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
import { useChatStore } from "../../stores/chat";
import { useModelsStore } from "../../stores/models";
import type { ParticipantConfig } from "../../stores/chat.types";
import Icon from "../Icon.vue";

const props = defineProps<{
  /** open/close v-model (consistent with other reka-ui Dialogs in
   *  the codebase). The host binds `:open` + `@update:open`. */
  open: boolean;
  /** "create" → fresh session flow (no sessionId yet).
   *  "edit" → re-edit existing session's roster. */
  mode: "create" | "edit";
  /** Required for mode="edit"; ignored for mode="create". */
  sessionId?: string;
  /** Initial roster for the edit flow (preserves existing names
   *  + model + persona). For mode="create" the host should pass
   *  `undefined` (the modal seeds an empty 2-participant default). */
  initialParticipants?: ParticipantConfig[];
}>();

const emit = defineEmits<{
  /** v-model open state (used by the host's `:open` binding). */
  (e: "update:open", value: boolean): void;
  /** Emitted when the user completes the action successfully.
   *  For mode="edit" the host listens to refresh the chat view. */
  (e: "created", sessionId: string): void;
  (e: "updated"): void;
}>();

const chatStore = useChatStore();
const modelsStore = useModelsStore();

// Participant list (local draft). Mirrors the deserialize
// ParticipantConfig shape (snake_case per `chat.types.ts`).
const participants = ref<ParticipantConfig[]>([]);

const MAX_PARTICIPANTS = 3;
const MIN_PARTICIPANTS = 2;

// Form error state (single banner — no per-field error UI to keep
// the MVP scope small).
const errorMessage = ref<string | null>(null);
const submitting = ref<boolean>(false);

// Names array — drives the "duplicate name" validation.
const participantNames = computed(() => participants.value.map((p) => p.name.trim()));

const duplicateName = computed(() => {
  const seen = new Set<string>();
  for (const n of participantNames.value) {
    if (!n) continue;
    if (seen.has(n)) return n;
    seen.add(n);
  }
  return null;
});

const emptyName = computed(() => {
  // Some participant has empty name → invalid
  return participants.value.some((p) => !p.name.trim());
});

const isValid = computed(() => {
  if (participants.value.length < MIN_PARTICIPANTS) return false;
  if (participants.value.length > MAX_PARTICIPANTS) return false;
  if (emptyName.value) return false;
  if (duplicateName.value) return false;
  if (participants.value.some((p) => !p.model.trim())) return false;
  return true;
});

// Models available for the dropdown.
const availableModels = computed(() => modelsStore.models ?? []);

// Seed / re-seed the draft when the modal opens.
watch(
  () => [props.open, props.mode, props.sessionId, props.initialParticipants] as const,
  ([isOpen]) => {
    if (!isOpen) return;
    errorMessage.value = null;
    if (props.mode === "edit" && props.initialParticipants) {
      // Deep-clone so cancel-discard works on the live draft.
      participants.value = props.initialParticipants.map((p) => ({
        name: p.name,
        model: p.model,
        persona_md: p.persona_md,
        order: p.order,
      }));
    } else if (props.mode === "create") {
      // Seed two empty participants (D5 minimum).
      participants.value = [
        { name: "", model: availableModels.value[0]?.id ?? "" },
        { name: "", model: availableModels.value[0]?.id ?? "" },
      ];
    }
  },
  { immediate: true },
);

// ---------------------------------------------------------------------
// Mutators
// ---------------------------------------------------------------------

function addParticipant() {
  if (participants.value.length >= MAX_PARTICIPANTS) return;
  participants.value.push({
    name: "",
    model: availableModels.value[0]?.id ?? "",
  });
}

function removeParticipant(idx: number) {
  if (participants.value.length <= MIN_PARTICIPANTS) return;
  participants.value.splice(idx, 1);
}

function moveUp(idx: number) {
  if (idx <= 0) return;
  const arr = participants.value;
  [arr[idx - 1], arr[idx]] = [arr[idx], arr[idx - 1]];
}

function moveDown(idx: number) {
  if (idx >= participants.value.length - 1) return;
  const arr = participants.value;
  [arr[idx], arr[idx + 1]] = [arr[idx + 1], arr[idx]];
}

// ---------------------------------------------------------------------
// Submit
// ---------------------------------------------------------------------

async function submit() {
  if (!isValid.value || submitting.value) return;
  submitting.value = true;
  errorMessage.value = null;
  try {
    // Strip empty persona_md (cleaner DB).
    const payload: ParticipantConfig[] = participants.value.map((p, i) => {
      const out: ParticipantConfig = {
        name: p.name.trim(),
        model: p.model,
        order: i,
      };
      const pm = p.persona_md?.trim();
      if (pm) out.persona_md = pm;
      return out;
    });

    if (props.mode === "create") {
      const newSessionId = await chatStore.createNewSession({
        sessionType: "group_chat",
        participants: participants.value.map((q, i) => {
          // Strip empty persona_md (cleaner DB).
          const out: ParticipantConfig = {
            name: q.name.trim(),
            model: q.model,
            order: i,
          };
          const pm = q.persona_md?.trim();
          if (pm) out.persona_md = pm;
          return out;
        }),
      });
      emit("created", newSessionId);
    } else {
      if (!props.sessionId) {
        throw new Error("GroupChatConfigModal: sessionId required for edit mode");
      }
      await chatStore.updateGroupChatConfig(props.sessionId, payload);
      emit("updated");
    }
    emit("update:open", false);
  } catch (e: unknown) {
    errorMessage.value = e instanceof Error ? e.message : String(e);
  } finally {
    submitting.value = false;
  }
}

function cancel() {
  errorMessage.value = null;
  emit("update:open", false);
}

// Expose a model label helper for the Select display.
function modelLabel(id: string): string {
  if (!id) return "";
  const m = availableModels.value.find((x) => x.id === id);
  return m ? `${m.displayName} (${m.providerDisplayName})` : id;
}
</script>

<template>
  <DialogRoot :open="open" @update:open="(v: boolean) => emit('update:open', v)">
    <DialogPortal>
      <DialogOverlay class="gcfg-overlay" />
      <DialogContent class="gcfg-content" aria-describedby="gcfg-desc">
        <DialogTitle class="gcfg-title">
          {{ mode === "create" ? "新建群聊" : "编辑参与者" }}
        </DialogTitle>
        <DialogDescription class="gcfg-subtitle">
          {{ mode === "create"
            ? "配置 2-3 个参与者(不含主持人)。"
            : "修改当前群聊的参与者配置。" }}
        </DialogDescription>

        <div v-if="errorMessage" class="gcfg-error" role="alert">
          {{ errorMessage }}
        </div>

        <div class="gcfg-list">
          <div
            v-for="(_, idx) in participants"
            :key="idx"
            class="gcfg-row"
            :data-testid="`gcfg-row-${idx}`"
          >
            <div class="gcfg-row__head">
              <span class="gcfg-row__title">参与者 #{{ idx + 1 }}</span>
              <div class="gcfg-row__actions">
                <button
                  type="button"
                  class="gcfg-icon-btn"
                  :disabled="idx === 0"
                  :aria-label="`Move participant ${idx + 1} up`"
                  @click="moveUp(idx)"
                >
                  ↑
                </button>
                <button
                  type="button"
                  class="gcfg-icon-btn"
                  :disabled="idx === participants.length - 1"
                  :aria-label="`Move participant ${idx + 1} down`"
                  @click="moveDown(idx)"
                >
                  ↓
                </button>
                <button
                  type="button"
                  class="gcfg-icon-btn gcfg-icon-btn--danger"
                  :disabled="participants.length <= MIN_PARTICIPANTS"
                  :aria-label="`Remove participant ${idx + 1}`"
                  :data-testid="`gcfg-remove-${idx}`"
                  @click="removeParticipant(idx)"
                >
                  ✕
                </button>
              </div>
            </div>

            <label class="gcfg-field">
              <span class="gcfg-field__label">名字</span>
              <input
                v-model="participants[idx].name"
                type="text"
                class="gcfg-input"
                placeholder="例如:Alex"
                :data-testid="`gcfg-name-${idx}`"
              />
            </label>

            <label class="gcfg-field">
              <span class="gcfg-field__label">模型</span>
              <SelectRoot v-model="participants[idx].model">
                <SelectTrigger class="gcfg-trigger" :data-testid="`gcfg-model-${idx}`">
                  <SelectValue :placeholder="modelLabel(participants[idx].model)">
                    {{ modelLabel(participants[idx].model) }}
                  </SelectValue>
                  <SelectIcon>
                    <Icon name="chevron-down" />
                  </SelectIcon>
                </SelectTrigger>
                <SelectPortal>
                  <SelectContent class="gcfg-select-content">
                    <SelectViewport>
                      <SelectItem
                        v-for="m in availableModels"
                        :key="m.id"
                        :value="m.id"
                        class="gcfg-select-item"
                        :data-testid="`gcfg-model-option-${idx}-${m.id}`"
                      >
                        <SelectItemText>{{ modelLabel(m.id) }}</SelectItemText>
                      </SelectItem>
                    </SelectViewport>
                  </SelectContent>
                </SelectPortal>
              </SelectRoot>
            </label>

            <label class="gcfg-field">
              <span class="gcfg-field__label">人设 (可选)</span>
              <textarea
                v-model="participants[idx].persona_md"
                class="gcfg-textarea"
                rows="3"
                placeholder="例如:你是 Alex,关注..."
                :data-testid="`gcfg-persona-${idx}`"
              />
            </label>
          </div>
        </div>

        <button
          v-if="participants.length < MAX_PARTICIPANTS"
          type="button"
          class="gcfg-add"
          :data-testid="'gcfg-add'"
          @click="addParticipant"
        >
          + 添加参与者
        </button>

        <div class="gcfg-footer">
          <button
            type="button"
            class="gcfg-btn gcfg-btn--secondary"
            :data-testid="'gcfg-cancel'"
            @click="cancel"
          >
            取消
          </button>
          <button
            type="button"
            class="gcfg-btn gcfg-btn--primary"
            :disabled="!isValid || submitting"
            :data-testid="'gcfg-submit'"
            @click="submit"
          >
            {{ submitting ? "保存中…" : (mode === "create" ? "创建群聊" : "保存") }}
          </button>
        </div>

        <DialogClose class="gcfg-close" aria-label="Close">
          <Icon name="x" />
        </DialogClose>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>

<style scoped>
.gcfg-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.5);
  z-index: 9999;
}

.gcfg-content {
  position: fixed;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  width: min(640px, 92vw);
  max-height: 88vh;
  overflow-y: auto;
  background: var(--ev-color-bg-panel, #1e1e1e);
  color: var(--ev-color-text, #e0e0e0);
  border-radius: 8px;
  padding: 24px;
  z-index: 10000;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.4);
}

.gcfg-title {
  font-size: 18px;
  font-weight: 600;
  margin: 0 0 4px;
}

.gcfg-subtitle {
  font-size: 13px;
  color: var(--ev-color-text-muted, #8a8a8a);
  margin: 0 0 16px;
}

.gcfg-error {
  background: var(--ev-color-error-bg, #5a1a1a);
  color: var(--ev-color-error-text, #ffb3b3);
  padding: 8px 12px;
  border-radius: 4px;
  font-size: 13px;
  margin-bottom: 12px;
}

.gcfg-list {
  display: flex;
  flex-direction: column;
  gap: 12px;
  margin-bottom: 12px;
}

.gcfg-row {
  border: 1px solid var(--ev-color-border, #333);
  border-radius: 6px;
  padding: 12px;
  background: var(--ev-color-bg-input, #2a2a2a);
}

.gcfg-row__head {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 8px;
}

.gcfg-row__title {
  font-weight: 500;
  font-size: 14px;
}

.gcfg-row__actions {
  display: flex;
  gap: 4px;
}

.gcfg-icon-btn {
  background: transparent;
  border: 1px solid var(--ev-color-border, #444);
  color: var(--ev-color-text, #e0e0e0);
  cursor: pointer;
  padding: 2px 6px;
  font-size: 12px;
  border-radius: 3px;
}
.gcfg-icon-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
.gcfg-icon-btn--danger:hover:not(:disabled) {
  background: var(--ev-color-error-bg, #5a1a1a);
}

.gcfg-field {
  display: flex;
  flex-direction: column;
  gap: 4px;
  margin-bottom: 8px;
}
.gcfg-field:last-child {
  margin-bottom: 0;
}

.gcfg-field__label {
  font-size: 12px;
  color: var(--ev-color-text-muted, #8a8a8a);
}

.gcfg-input,
.gcfg-textarea {
  width: 100%;
  background: var(--ev-color-bg-page, #1a1a1a);
  border: 1px solid var(--ev-color-border, #444);
  border-radius: 4px;
  padding: 6px 8px;
  color: var(--ev-color-text, #e0e0e0);
  font: inherit;
  font-size: 13px;
  box-sizing: border-box;
}

.gcfg-textarea {
  font-family: var(--ev-font-mono, monospace);
  resize: vertical;
}

.gcfg-trigger {
  display: flex;
  align-items: center;
  justify-content: space-between;
  width: 100%;
  background: var(--ev-color-bg-page, #1a1a1a);
  border: 1px solid var(--ev-color-border, #444);
  border-radius: 4px;
  padding: 6px 8px;
  color: var(--ev-color-text, #e0e0e0);
  font: inherit;
  font-size: 13px;
  cursor: pointer;
}

.gcfg-select-content {
  background: var(--ev-color-bg-panel, #1e1e1e);
  border: 1px solid var(--ev-color-border, #444);
  border-radius: 4px;
  max-height: 240px;
  z-index: 10001;
}

.gcfg-select-item {
  padding: 6px 12px;
  font-size: 13px;
  cursor: pointer;
  outline: none;
}
.gcfg-select-item[data-highlighted] {
  background: var(--ev-color-bg-hover, #2a2a2a);
}

.gcfg-add {
  display: block;
  width: 100%;
  padding: 8px;
  margin-bottom: 16px;
  background: transparent;
  border: 1px dashed var(--ev-color-border, #555);
  border-radius: 4px;
  color: var(--ev-color-text-muted, #8a8a8a);
  cursor: pointer;
  font-size: 13px;
}
.gcfg-add:hover {
  background: var(--ev-color-bg-hover, #2a2a2a);
  color: var(--ev-color-text, #e0e0e0);
}

.gcfg-footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 16px;
}

.gcfg-btn {
  padding: 8px 16px;
  border-radius: 4px;
  border: 1px solid transparent;
  font: inherit;
  font-size: 13px;
  cursor: pointer;
}
.gcfg-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.gcfg-btn--secondary {
  background: transparent;
  border-color: var(--ev-color-border, #444);
  color: var(--ev-color-text, #e0e0e0);
}
.gcfg-btn--primary {
  background: var(--ev-color-accent, #4a8eff);
  color: white;
}
.gcfg-btn--primary:hover:not(:disabled) {
  background: var(--ev-color-accent-hover, #3a7eef);
}

.gcfg-close {
  position: absolute;
  top: 12px;
  right: 12px;
  background: transparent;
  border: none;
  color: var(--ev-color-text-muted, #8a8a8a);
  cursor: pointer;
  padding: 4px;
}
.gcfg-close:hover {
  color: var(--ev-color-text, #e0e0e0);
}
</style>
