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
import type {
  ParticipantConfig,
  SessionSummary,
  SpeakerCacheUsage,
} from "../../stores/chat.types";
import { transport } from "../../transport";
import { cacheRatePercent } from "../../utils/tokenUsage";
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

// ---------------------------------------------------------------------
// Group-chat cache rates (08-10-group-chat-cache-rate, R6/R7)
// ---------------------------------------------------------------------

// Per-speaker latest-turn cache-usage map, keyed by the persisted
// `messages.speaker` value (participant name / "moderator").
// Read-only auxiliary info: fetched once per open in edit mode
// (R7), failures degrade to "—" and never block editing.
const cacheRates = ref<Map<string, SpeakerCacheUsage>>(new Map());

// Speaker names per row index, snapshotted at open time. The rate
// rows must survive the user editing a name in the draft — the
// persisted `messages.speaker` still holds the old name until
// save, so lookups key off the snapshot, not the live draft.
const rosterSpeakers = ref<string[]>([]);

// The current session record (for the moderator's model label —
// the host only passes sessionId; the model lives on
// `SessionSummary.model_id`).
const currentSession = computed<SessionSummary | null>(() => {
  if (!props.sessionId) return null;
  return chatStore.sessions.find((s) => s.id === props.sessionId) ?? null;
});

async function loadCacheRates(sessionId: string) {
  cacheRates.value = new Map();
  try {
    const rows = await transport.invoke<SpeakerCacheUsage[]>("group_chat_cache_rates", {
      sessionId,
    });
    cacheRates.value = new Map(rows.map((r) => [r.speaker, r]));
  } catch (e) {
    // Silent degradation: the rate is auxiliary, the edit flow
    // stays usable (design.md: 不阻塞编辑).
    console.error("group_chat_cache_rates failed:", e);
  }
}

/** Latest-turn cache rate (%) for `speaker`, or `null` when there
 *  is no usable usage row (no turns yet / all cancelled / legacy
 *  `context_input = 0` / request failure) → "—" placeholder. */
function cacheRateFor(speaker: string): number | null {
  const u = cacheRates.value.get(speaker);
  if (!u) return null;
  return cacheRatePercent(u.cache_read, u.context_input);
}

function cacheRateText(speaker: string): string {
  const pct = cacheRateFor(speaker);
  return pct === null ? "—" : `${pct}%`;
}

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
      }));
      rosterSpeakers.value = props.initialParticipants.map((p) => p.name.trim());
    } else if (props.mode === "create") {
      // Seed two empty participants (D5 minimum).
      participants.value = [
        { name: "", model: availableModels.value[0]?.id ?? "" },
        { name: "", model: availableModels.value[0]?.id ?? "" },
      ];
      rosterSpeakers.value = [];
    }
  },
  { immediate: true },
);

// Fetch cache rates once per open (edit mode only — a fresh group
// chat has no turns, R6). Separate watcher so the draft seeding
// above stays untouched. `immediate: true` so a mount with
// `open=true` (tests / fast open) fetches on first render; the
// open-state change still refetches on every reopen (R7).
watch(
  () => [props.open, props.mode, props.sessionId] as const,
  ([isOpen, mode, sessionId]) => {
    if (!isOpen) return;
    cacheRates.value = new Map();
    if (mode === "edit" && sessionId) {
      void loadCacheRates(sessionId);
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
  // Keep the roster snapshot index-aligned with the draft: the new
  // row has no history → empty speaker → "—" rate.
  rosterSpeakers.value.push("");
}

function removeParticipant(idx: number) {
  if (participants.value.length <= MIN_PARTICIPANTS) return;
  participants.value.splice(idx, 1);
  // Splice the snapshot at the SAME index — otherwise every
  // subsequent row shifts by one and shows the wrong speaker's
  // cache rate (row N would render rosterSpeakers[N] which now
  // belongs to the removed participant's successor).
  rosterSpeakers.value.splice(idx, 1);
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
    const payload: ParticipantConfig[] = participants.value.map((p) => {
      const out: ParticipantConfig = {
        name: p.name.trim(),
        model: p.model,
      };
      const pm = p.persona_md?.trim();
      if (pm) out.persona_md = pm;
      return out;
    });

    if (props.mode === "create") {
      const newSessionId = await chatStore.createNewSession({
        sessionType: "group_chat",
        participants: participants.value.map((q) => {
          // Strip empty persona_md (cleaner DB).
          const out: ParticipantConfig = {
            name: q.name.trim(),
            model: q.model,
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
      <DialogContent class="gcfg-content">
        <div class="gcfg-header">
          <div class="gcfg-header__text">
            <DialogTitle class="gcfg-title">
              {{ mode === "create" ? "新建群聊" : "编辑参与者" }}
            </DialogTitle>
            <DialogDescription class="gcfg-subtitle">
              {{ mode === "create"
                ? "配置 2-3 个参与者(不含主持人)。"
                : "修改当前群聊的参与者配置。" }}
            </DialogDescription>
          </div>
          <DialogClose class="gcfg-close" aria-label="Close">
            <Icon name="x" />
          </DialogClose>
        </div>

        <!-- 滚动 body: 错误条 + 参与者列表 + 添加按钮。
             标题/副标题/footer 留在滚动区外, 内容超高时只滚动这里。
             见 R3 / RuntimeMemoryModal 的 flex-column + overflow body 模式。 -->
        <div class="gcfg-body">
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
                    class="gcfg-icon-btn gcfg-icon-btn--danger"
                    :disabled="participants.length <= MIN_PARTICIPANTS"
                    :aria-label="`Remove participant ${idx + 1}`"
                    :data-testid="`gcfg-remove-${idx}`"
                    @click="removeParticipant(idx)"
                  >
                    <Icon name="x" :size="14" />
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
                    <SelectContent
                      class="gcfg-select-content"
                      position="popper"
                    >
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

              <!--
                Group-chat cache rate (08-10-group-chat-cache-rate,
                R6): read-only line showing THIS speaker's latest
                LLM call cache-hit rate ("—" when no usable usage
                row yet / legacy context_input=0 / fetch failure).
                Keyed off the roster snapshot — the draft name may
                be mid-edit while the DB still holds the old
                speaker value.
              -->
              <div
                v-if="mode === 'edit'"
                class="gcfg-cache-rate"
                :data-testid="`gcfg-cache-rate-${idx}`"
              >
                缓存率 {{ cacheRateText(rosterSpeakers[idx] ?? "") }}
              </div>

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

          <!--
            Moderator zone (08-10-group-chat-cache-rate, R6):
            read-only row at the bottom of the edit modal. The
            moderator is not in the participant roster; its
            speaker key is fixed to "moderator" (matches the
            backend `group_chat_loop` write), and its model is
            the session's own `model_id`.
          -->
          <div
            v-if="mode === 'edit'"
            class="gcfg-moderator"
            data-testid="gcfg-moderator"
          >
            <span class="gcfg-moderator__label">主持人</span>
            <span class="gcfg-moderator__model">
              {{ modelLabel(currentSession?.model_id ?? "") }}
            </span>
            <span
              class="gcfg-moderator__rate"
              data-testid="gcfg-moderator-cache-rate"
            >
              缓存率 {{ cacheRateText("moderator") }}
            </span>
          </div>
        </div>

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

      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>

<style scoped>
/* 样式对齐项目 modal 家族(MemoryModal / RuntimeMemoryModal,规范见
   .trellis/spec/frontend/popover-pattern.md + design-tokens.md):
   - 全部改用 --color-* / --radius-* / --text-* / --shadow-* token——旧版
     引用的 --ev-color-* 在本项目从未定义,一直落在硬编码中性灰 fallback
     上,色相与普鲁士蓝暗色主题脱节,是"风格不统一"的根因;
   - A 类 reka-ui Dialog 惯例:mask 不动画,content 做 scale 0.1↔1 zoom,
     阴影用最大档 --shadow-xl;
   - 结构 = elevated 头/脚 + border 分隔线 + app 底色滚动 body。
   z-index 层级沿用家族基线(overlay 2000 / content 2001 / Select portal
   3000),此 modal 与 RuntimeMemoryModal 不会同时打开,无冲突。 */
.gcfg-overlay {
  position: fixed;
  inset: 0;
  background: color-mix(in srgb, var(--color-bg-app) 70%, transparent);
  backdrop-filter: blur(4px);
  z-index: var(--z-modal-overlay);
}

/* DialogContent 本身不滚动: flex 列容器, header/footer 固定, 只有
   .gcfg-body 滚动。根类名 .gcfg-content 被全局移动端全屏覆盖块
   (style.css @media max-width:767px)与测试引用,不可改名。 */
.gcfg-content {
  position: fixed;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  width: min(640px, calc(100vw - 40px));
  max-height: 80vh;
  display: flex;
  flex-direction: column;
  background: var(--color-bg-surface);
  color: var(--color-text-primary);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-xl);
  /* 容器接 programmatic focus,整框上环无意义;内部控件由全局 :focus-visible 基线负责(style.css) */
  outline: none;
  overflow: hidden;
  z-index: var(--z-modal);
  animation: gcfg-zoom var(--duration-modal-in) var(--ease-modal-in) both;
}
.gcfg-content[data-state="closed"] {
  animation: gcfg-zoom-out var(--duration-modal-out) var(--ease-accelerate)
    forwards;
}

@keyframes gcfg-zoom {
  from {
    opacity: 0;
    transform: translate(-50%, -50%) scale(0.1);
  }
  to {
    opacity: 1;
    transform: translate(-50%, -50%) scale(1);
  }
}
@keyframes gcfg-zoom-out {
  from {
    opacity: 1;
    transform: translate(-50%, -50%) scale(1);
  }
  to {
    opacity: 0;
    transform: translate(-50%, -50%) scale(0.1);
  }
}

.gcfg-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 16px 12px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  flex-shrink: 0;
}

.gcfg-header__text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.gcfg-title {
  margin: 0;
  font-size: var(--text-base);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
}

.gcfg-subtitle {
  margin: 0;
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
}

.gcfg-close {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  background: transparent;
  border: 0;
  color: var(--color-text-muted);
  cursor: pointer;
  padding: 4px;
  border-radius: var(--radius-sm);
  transition: background var(--duration-base) var(--ease-out);
}
.gcfg-close:hover {
  background: var(--color-bg-border);
  color: var(--color-text-primary);
}

/* 滚动 body: 撑满 dialog 剩余高度, 内容超高时只滚这里。
   min-height:0 是 flex 子项可收缩的关键; 子项间距统一走 gap。 */
.gcfg-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 16px;
  background: var(--color-bg-app);
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.gcfg-error {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 8px 12px;
  border: 1px solid
    color-mix(in srgb, var(--color-tool-error) 20%, transparent);
  border-radius: var(--radius-md);
  background: color-mix(in srgb, var(--color-tool-error) 8%, transparent);
  color: var(--color-tool-error-text);
  font-size: var(--text-sm);
}

.gcfg-list {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.gcfg-row {
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  padding: 12px;
  background: var(--color-bg-surface);
}

.gcfg-row__head {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 10px;
}

.gcfg-row__title {
  font-size: var(--text-sm);
  font-weight: var(--weight-medium);
  color: var(--color-text-secondary);
}

.gcfg-icon-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  padding: 0;
  background: transparent;
  border: none;
  color: var(--color-text-muted);
  cursor: pointer;
  border-radius: var(--radius-sm);
  transition: background var(--duration-base) var(--ease-out);
}
.gcfg-icon-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
.gcfg-icon-btn--danger:hover:not(:disabled) {
  background: color-mix(in srgb, var(--color-tool-error) 10%, transparent);
  color: var(--color-tool-error-text);
}

.gcfg-field {
  display: flex;
  flex-direction: column;
  gap: 4px;
  margin-bottom: 10px;
}
.gcfg-field:last-child {
  margin-bottom: 0;
}

.gcfg-field__label {
  font-size: var(--text-xs);
  font-weight: var(--weight-medium);
  color: var(--color-text-secondary);
}

/* Read-only cache-rate line on each participant row
   (08-10-group-chat-cache-rate). */
.gcfg-cache-rate {
  font-size: var(--text-xs);
  font-family: var(--font-mono);
  color: var(--color-text-muted);
  margin-bottom: 10px;
}

/* Read-only moderator zone at the bottom of the edit modal. */
.gcfg-moderator {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  background: var(--color-bg-surface);
  font-size: var(--text-sm);
}

.gcfg-moderator__label {
  font-weight: var(--weight-medium);
}

.gcfg-moderator__model {
  color: var(--color-text-secondary);
}

.gcfg-moderator__rate {
  margin-left: auto;
  font-size: var(--text-xs);
  font-family: var(--font-mono);
  color: var(--color-text-muted);
}

/* 输入控件比所在卡片(surface)低一层(app),与家族
   elevated 壳 + surface 输入框的"内嵌暗一档"关系一致。 */
.gcfg-input,
.gcfg-textarea {
  width: 100%;
  box-sizing: border-box;
  padding: 6px 10px;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  color: var(--color-text-primary);
  font: inherit;
  font-size: var(--text-sm);
  /* 自有焦点替代::focus 换 accent 边框(见下),无需 UA 默认环 */
  outline: none;
}
.gcfg-input:focus,
.gcfg-textarea:focus {
  border-color: var(--color-accent);
}
.gcfg-input::placeholder,
.gcfg-textarea::placeholder {
  color: var(--color-text-muted);
}

.gcfg-textarea {
  resize: vertical;
  min-height: 80px;
  line-height: var(--leading-normal);
}

.gcfg-trigger {
  display: inline-flex;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
  width: 100%;
  padding: 6px 10px;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  color: var(--color-text-primary);
  font: inherit;
  font-size: var(--text-sm);
  cursor: pointer;
}
.gcfg-trigger[data-state="open"] {
  border-color: var(--color-accent);
}

/* Select 内容由 <SelectPortal> teleport 到 <body>, 是嵌套渲染的 portal
   子元素——scoped 选择器在 Vue 3.5 下不一定稳定命中, 必须用 :deep()。
   z-index 抬到 3000 高于 dialog(2001); 宽度贴合 trigger。对齐
   RuntimeMemoryModal 既有规范。见 .trellis/spec/frontend/reka-ui-usage.md。 */
:deep(.gcfg-select-content) {
  width: var(--reka-select-trigger-width);
  min-width: var(--reka-select-trigger-width, 240px);
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  box-shadow: var(--shadow-md);
  padding: 4px;
  max-height: 240px;
  z-index: var(--z-over-modal) !important;
}

:deep(.gcfg-select-item) {
  display: flex;
  align-items: center;
  padding: 6px 10px;
  border-radius: 3px;
  font-size: var(--text-sm);
  color: var(--color-text-primary);
  cursor: pointer;
}
:deep(.gcfg-select-item[data-highlighted]) {
  background: var(--color-bg-surface);
}
:deep(.gcfg-select-item[data-state="checked"]) {
  color: var(--color-accent-text);
}

.gcfg-add {
  display: block;
  width: 100%;
  padding: 8px;
  background: transparent;
  border: 1px dashed
    color-mix(in srgb, var(--color-text-muted) 45%, transparent);
  border-radius: var(--radius-sm);
  color: var(--color-text-secondary);
  font: inherit;
  font-size: var(--text-sm);
  cursor: pointer;
  transition: background var(--duration-base) var(--ease-out);
}
.gcfg-add:hover {
  background: var(--color-bg-surface);
  border-color: color-mix(in srgb, var(--color-text-muted) 70%, transparent);
  color: var(--color-text-primary);
}

.gcfg-footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  padding: 12px 16px;
  border-top: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  flex-shrink: 0;
}

.gcfg-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 6px 14px;
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  background: var(--color-bg-surface);
  color: var(--color-text-primary);
  font: inherit;
  font-size: var(--text-sm);
  cursor: pointer;
  transition: background var(--duration-base) var(--ease-out);
}
.gcfg-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.gcfg-btn--secondary:hover:not(:disabled) {
  background: var(--color-bg-border);
}
.gcfg-btn--primary {
  background: var(--color-accent);
  border-color: var(--color-accent);
  color: var(--color-text-on-accent, #fff);
}
.gcfg-btn--primary:hover:not(:disabled) {
  filter: brightness(1.1);
}
</style>
