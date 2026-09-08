<script setup lang="ts">
// GroupChatConfigModal — create / edit modal for a group_chat session.
//
// 07-29-group-chat (Phase 4 Step 3 TODO-E6): serves BOTH the
// create-session flow (from SessionList's "新建群聊" button) AND the
// runtime re-edit flow (opened from the chat header). Same component,
// two modes:
//   - mode: "create" → calls `createNewSession` with the new roster;
//     closes on success.
//   - mode: "edit" → calls `updateGroupChatConfig` for the given
//     sessionId; closes on success.
//
// gce-m4c (09-08, 预设优先单弹窗重设计, design §4):
//   - create: preset 单选卡区(review/fe_review/arch/retro,共享
//     scripts/group-chat-presets.json,选中即预填阵容 + 主持人默认)→
//     阵容微调(2-3 上限,交互保留)→ 主持人 Select(preset 默认可改
//     选,提交写 `create_session` 的 model 参数)→ token_budget 输入 +
//     量级提示。议题不进弹窗(D2)。
//   - edit: 阵容编辑照旧(不引入 preset 重选)+ 主持人只读区照旧 +
//     成本区(per-speaker「tokens · 缓存率」合并行 + 预算进度条;
//     `group_chat_token_usage` + `group_chat_cache_rates` 两次查询,
//     失败降级「—」不阻塞编辑)。
//
// MVP boundary (D5): 2-3 participants. "+" button hidden at 3
// participants; delete button disabled when only 2 remain.
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
  RadioGroupRoot,
  RadioGroupItem,
  RadioGroupIndicator,
} from "reka-ui";
import { useChatStore } from "../../stores/chat";
import { useModelsStore } from "../../stores/models";
import type {
  ParticipantConfig,
  SessionSummary,
  SpeakerCacheUsage,
  GroupChatTokenUsage,
} from "../../stores/chat.types";
import { transport } from "../../transport";
import { cacheRatePercent, formatTokensWan } from "../../utils/tokenUsage";
// gce-m4c(09-08,弹窗重设计):preset 单一事实源与展开逻辑与定时表单
// 共享(utils/groupChatPresets.ts,persona 组装 / 模型解析逐字同形)。
import {
  GC_PRESETS,
  composePersonaMd,
  resolveModelRef,
} from "../../utils/groupChatPresets";
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
  /** C1.2 (09-08-gc-c1-stoploss): existing `metadata.token_budget`
   *  for the edit flow (prefills the budget input). `undefined` on
   *  create (input starts empty = unlimited). */
  initialTokenBudget?: number | null;
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

// C1.2 token budget draft (string — number inputs with a possible
// empty state bind cleanly as text; parsed on submit).
const tokenBudgetInput = ref<string>("");

const parsedTokenBudget = computed<number | null>(() => {
  // v-model on type="number" coerces valid input to a NUMBER
  // (looseToNumber) and keeps invalid input as string — normalize both.
  const raw = tokenBudgetInput.value;
  const t = String(raw ?? "").trim();
  if (t === "") return null;
  const n = Number(t);
  if (!Number.isInteger(n) || n <= 0) return null;
  return n;
});

const invalidBudget = computed(() => {
  const t = String(tokenBudgetInput.value ?? "").trim();
  return t !== "" && parsedTokenBudget.value === null;
});

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
  if (invalidBudget.value) return false;
  // gce-m4c:create 选中 preset 且其主持人未解析/未改选 → 阻止提交
  // (提示条已给出去向;绝不静默回落全局默认——prd R2)。未选 preset
  // 时不要求主持人(不传 model = 全局默认,既有行为)。
  if (props.mode === "create" && selectedPreset.value && !moderatorId.value) {
    return false;
  }
  return true;
});

// Models for the dropdown: FULL catalog (modelLabel 反查用 —— 已指向
// 被禁用模型的行,显示名仍要可解析)。
const availableModels = computed(() => modelsStore.models ?? []);

// 2026-09-07 (provider-model-disable): 可选项 = 启用模型 ∪ 草稿各行
// 已选 id ∪ 主持人当前值。禁用模型不再可被改选,但编辑态回显的旧阵容
// 仍要能显示与保留(后端 catalog 不滤,禁用不影响已在用的群聊分发)。
const selectableModels = computed(() => {
  const pinned = new Set([
    ...participants.value.map((p) => p.model),
    moderatorId.value,
  ].filter(Boolean));
  return modelsStore.models.filter(
    (m) => pinned.has(m.id) || !(m.disabled || m.providerDisabled),
  );
});

// ---------------------------------------------------------------------
// gce-m4c (09-08): create 模式的 preset 单选卡 + 主持人 Select
// ---------------------------------------------------------------------

// preset 键序 = JSON 声明序(review / fe_review / arch / retro);卡的展示名是
// 纯 UI 映射(描述文案取 JSON `description`)。
const GC_PRESET_LABELS: Record<string, string> = {
  review: "评审团",
  fe_review: "前端评审",
  arch: "架构",
  retro: "复盘",
};

function presetLabel(key: string): string {
  return GC_PRESET_LABELS[key] ?? key;
}

const gcPresetEntries = computed(() => Object.entries(GC_PRESETS.presets));

/** reka `update:model-value` 载荷归一化(ScheduledTasksTab 同款)。 */
function normalizeSelectValue(v: unknown): string {
  if (Array.isArray(v)) return typeof v[0] === "string" ? v[0] : "";
  return typeof v === "string" ? v : "";
}

/** 当前选中的 preset("" = 未选择——改任何阵容字段不回退此状态,
 *  无「自定义」显式态;design §4.1)。 */
const selectedPreset = ref("");

/** 主持人模型目录 id(create 模式草稿;空 = 未解析/未改选)。选中
 *  preset 即重置为该 preset 的 moderator_model 解析结果;用户改选覆盖
 *  (解析失败时保持空,提示条引导手动改选,不阻塞 Select)。 */
const moderatorId = ref("");

/** 选中 preset → 立即展开预填阵容 + 主持人默认(persona_md =
 *  composePersonaMd 展开,与 script/定时表单逐字同形;模型引用解析
 *  失败的行留空模型,由行内空 Select + 提示条暴露,绝不静默造数)。 */
function applyPreset(key: string): void {
  const def = GC_PRESETS.presets[key];
  if (!def) return;
  participants.value = def.participants.map((p) => {
    const persona = composePersonaMd(p.persona);
    const out: ParticipantConfig = {
      name: p.name,
      model: resolveModelRef(modelsStore.models ?? [], p.model) ?? "",
    };
    if (persona !== null) out.persona_md = persona;
    return out;
  });
  moderatorId.value =
    resolveModelRef(modelsStore.models ?? [], def.moderator_model) ?? "";
}

function onPickPreset(v: unknown): void {
  const k = normalizeSelectValue(v);
  if (!k || !(k in GC_PRESETS.presets)) return;
  selectedPreset.value = k;
  applyPreset(k);
}

function onPickModerator(v: unknown): void {
  const m = normalizeSelectValue(v);
  if (m === "" || selectableModels.value.some((o) => o.id === m)) {
    moderatorId.value = m;
  }
}

/** preset 展开暴露的模型缺失(创建态提示条;绝不静默降级——prd R2):
 *  主持人解析失败显示 preset 原名(定时表单同款文案);参与行模型
 *  解析失败读草稿空模型行(用户手动改选后自然消隐)。 */
const presetWarnings = computed<string[]>(() => {
  if (props.mode !== "create" || !selectedPreset.value) return [];
  const warnings: string[] = [];
  const def = GC_PRESETS.presets[selectedPreset.value]!;
  if (!moderatorId.value) {
    warnings.push(
      `预设主持人模型「${def.moderator_model}」不在模型目录中,请先在「模型」页添加`,
    );
  }
  for (const row of participants.value) {
    if (!row.model.trim()) {
      warnings.push(
        `参与者「${row.name || "(未命名)"}」的模型不在模型目录中,请先在「模型」页添加`,
      );
    }
  }
  return warnings;
});

// ---------------------------------------------------------------------
// Group-chat cache rates (08-10-group-chat-cache-rate, R6/R7)
// ---------------------------------------------------------------------

// Per-speaker latest-turn cache-usage map, keyed by the persisted
// `messages.speaker` value (participant name / "moderator").
// Read-only auxiliary info: fetched once per open in edit mode
// (R7), failures degrade to "—" and never block editing.
const cacheRates = ref<Map<string, SpeakerCacheUsage>>(new Map());

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

// ---------------------------------------------------------------------
// gce-m4c (09-08): edit 模式成本区 —— per-speaker token 核算 + 预算
// 进度条。数据 = `group_chat_token_usage`(新)与既有
// `group_chat_cache_rates` 两次 invoke(design §4.2:不合并端点);
// 任一查询失败该位降级「—」,绝不阻塞编辑(既有 cacheRates 同款)。
// ---------------------------------------------------------------------

// Per-discussion billed-token totals (`db::trace::GroupChatTokenUsage`,
// snake_case wire). `null` = query failed → tokens render "—".
const tokenUsage = ref<GroupChatTokenUsage | null>(null);

async function loadTokenUsage(sessionId: string) {
  tokenUsage.value = null;
  try {
    const usage = await transport.invoke<GroupChatTokenUsage>(
      "group_chat_token_usage",
      { sessionId },
    );
    // 防御形状校验:半写 / 非预期响应按失败降级,别让 NaN 漏进进度条。
    tokenUsage.value =
      usage && typeof usage === "object" && typeof usage.total === "number" && Array.isArray(usage.by_speaker)
        ? usage
        : null;
  } catch (e) {
    // Silent degradation: cost is auxiliary, the edit flow stays usable.
    console.error("group_chat_token_usage failed:", e);
  }
}

/** speaker → 累计计费 token(四字段口径,与 C1.2 硬停一致)。 */
const tokensBySpeaker = computed<Map<string, number>>(() => {
  const m = new Map<string, number>();
  for (const r of tokenUsage.value?.by_speaker ?? []) m.set(r.speaker, r.tokens);
  return m;
});

// Cost-zone speaker list, snapshotted at open time (roster names +
// "moderator"). Historical consumption must survive the user editing /
// removing a draft row — rows key off the persisted `messages.speaker`,
// not the live draft. Extra speakers present in either payload (but
// missing from the snapshot) are appended defensively.
const costSpeakers = ref<string[]>([]);

const costRows = computed<{ speaker: string; label: string; tokens: number | null; rate: string }[]>(
  () => {
    const rows: { speaker: string; label: string; tokens: number | null; rate: string }[] = [];
    const seen = new Set<string>();
    const pushRow = (speaker: string) => {
      if (!speaker || seen.has(speaker)) return;
      seen.add(speaker);
      rows.push({
        speaker,
        label: speaker === "moderator" ? "主持人" : speaker,
        tokens: tokensBySpeaker.value.get(speaker) ?? null,
        rate: cacheRateText(speaker),
      });
    };
    for (const s of costSpeakers.value) pushRow(s);
    for (const s of tokensBySpeaker.value.keys()) pushRow(s);
    for (const s of cacheRates.value.keys()) pushRow(s);
    return rows;
  },
);

/** 预算进度:initialTokenBudget(metadata token_budget)× usage total。
 *  超额 → 进度条满格 + 数字转 error 系(design §4.2)。total 查询失败
 *  (null)时进度条整体不渲染(行内降级「—」)。 */
const budgetNumber = computed<number | null>(() =>
  typeof props.initialTokenBudget === "number" ? props.initialTokenBudget : null,
);
const usageTotal = computed<number | null>(() =>
  tokenUsage.value ? tokenUsage.value.total : null,
);
const budgetPct = computed<number | null>(() => {
  if (budgetNumber.value === null || usageTotal.value === null) return null;
  if (budgetNumber.value <= 0) return null;
  return Math.round((usageTotal.value / budgetNumber.value) * 100);
});
const overBudget = computed<boolean>(
  () => budgetPct.value !== null && budgetPct.value > 100,
);
const budgetFillWidth = computed<string>(() =>
  overBudget.value ? "100%" : `${Math.max(0, budgetPct.value ?? 0)}%`,
);

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
      // Cost-zone speakers: open-time roster + the moderator (the
      // historical `messages.speaker` values — survive draft edits).
      costSpeakers.value = [
        ...props.initialParticipants.map((p) => p.name.trim()),
        "moderator",
      ];
      tokenBudgetInput.value =
        typeof props.initialTokenBudget === "number" ? String(props.initialTokenBudget) : "";
    } else if (props.mode === "create") {
      // Seed two empty participants (D5 minimum)。默认模型取首个「启用」
      // 模型(禁用模型不出现在选项里,也不做默认)。preset 不预选
      // (design §4.1:用户点卡才展开预填),主持人跟随。
      participants.value = [
        { name: "", model: selectableModels.value[0]?.id ?? "" },
        { name: "", model: selectableModels.value[0]?.id ?? "" },
      ];
      selectedPreset.value = "";
      moderatorId.value = "";
      costSpeakers.value = [];
      tokenBudgetInput.value = "";
    }
  },
  { immediate: true },
);

// Fetch cache rates + token usage once per open (edit mode only — a
// fresh group chat has no turns, R6). Separate watcher so the draft
// seeding above stays untouched. `immediate: true` so a mount with
// `open=true` (tests / fast open) fetches on first render; the
// open-state change still refetches on every reopen (R7).
watch(
  () => [props.open, props.mode, props.sessionId] as const,
  ([isOpen, mode, sessionId]) => {
    if (!isOpen) return;
    cacheRates.value = new Map();
    tokenUsage.value = null;
    if (mode === "edit" && sessionId) {
      void loadCacheRates(sessionId);
      void loadTokenUsage(sessionId);
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
    model: selectableModels.value[0]?.id ?? "",
  });
}

function removeParticipant(idx: number) {
  if (participants.value.length <= MIN_PARTICIPANTS) return;
  participants.value.splice(idx, 1);
  // Cost-zone rows are NOT spliced: they reflect the persisted
  // `messages.speaker` history (a removed participant's past turns
  // still consumed tokens), so they stay stable across draft edits.
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
        tokenBudget: parsedTokenBudget.value ?? undefined,
        // gce-m4c:主持人模型(preset 默认或手动改选)→ `create_session`
        // 的 model 参数;未选/未改选 = 不传(后端落全局默认,既有行为)。
        modelId: moderatorId.value || undefined,
      });
      emit("created", newSessionId);
    } else {
      if (!props.sessionId) {
        throw new Error("GroupChatConfigModal: sessionId required for edit mode");
      }
      await chatStore.updateGroupChatConfig(
        props.sessionId,
        payload,
        parsedTokenBudget.value,
      );
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
          <DialogClose class="gcfg-close btn btn--icon btn--ghost" aria-label="Close">
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

          <!-- gce-m4c(09-08,design §4.1):preset 单选卡区(create)。
               键序 = JSON 声明序;选中即预填阵容 + 主持人默认,改动阵容
               字段不回退 preset 状态(无「自定义」显式态)。 -->
          <div v-if="mode === 'create'" class="gcfg-presets">
            <span class="gcfg-field__label">审议预设</span>
            <RadioGroupRoot
              class="gcfg-preset-cards"
              :model-value="selectedPreset || undefined"
              @update:model-value="onPickPreset"
            >
              <label
                v-for="[key, def] in gcPresetEntries"
                :key="key"
                class="gcfg-preset-card"
                :class="{ 'gcfg-preset-card--active': selectedPreset === key }"
                :data-testid="`gcfg-preset-${key}`"
              >
                <RadioGroupItem :value="key" class="gcfg-preset-radio">
                  <RadioGroupIndicator class="gcfg-preset-radio-indicator" />
                </RadioGroupItem>
                <span class="gcfg-preset-text">
                  <span class="gcfg-preset-name">{{ presetLabel(key) }}</span>
                  <span class="gcfg-preset-desc" :title="def.description">
                    {{ def.description }}
                  </span>
                </span>
              </label>
            </RadioGroupRoot>
          </div>

          <!-- preset 展开暴露的模型缺失(create;定时表单同款文案形态,
               绝不静默降级)。主持人缺失时提交按钮同步禁用(isValid)，
               但 Select 始终可手动改选。 -->
          <div
            v-if="presetWarnings.length > 0"
            class="gcfg-error"
            data-testid="gcfg-preset-error"
            role="alert"
          >
            <p v-for="w in presetWarnings" :key="w" class="gcfg-error__line">{{ w }}</p>
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
                    class="gcfg-icon-btn gcfg-icon-btn--danger btn btn--icon btn--danger-soft"
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
                          v-for="m in selectableModels"
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
                gce-m4c (09-08): the per-row cache-rate line moved into
                the edit-mode cost zone (design §4.2 「缓存率行并入成本区
                同一行展示」) — each cost row is「speaker tokens · 缓存率」
                merged. The roster row itself is config-only now.
              -->

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
            class="gcfg-add btn btn--outline"
            :data-testid="'gcfg-add'"
            @click="addParticipant"
          >
            + 添加参与者
          </button>

          <!-- gce-m4c(09-08,design §4.1):主持人 Select(create)。默认 =
               选中 preset 的 moderator_model 解析结果;用户改选覆盖;
               preset 主持人解析失败 → 上方提示条 + 此处手动改选不被阻塞。
               未选 preset 且未改选 = 不传 model(后端落全局默认,既有行为)。 -->
          <label v-if="mode === 'create'" class="gcfg-field">
            <span class="gcfg-field__label">主持人模型</span>
            <SelectRoot
              :model-value="moderatorId || undefined"
              @update:model-value="onPickModerator"
            >
              <SelectTrigger class="gcfg-trigger" data-testid="gcfg-moderator-select">
                <SelectValue placeholder="默认(跟随全局设置)" />
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
                      v-for="m in selectableModels"
                      :key="m.id"
                      :value="m.id"
                      class="gcfg-select-item"
                      :data-testid="`gcfg-moderator-option-${m.id}`"
                    >
                      <SelectItemText>{{ modelLabel(m.id) }}</SelectItemText>
                    </SelectItem>
                  </SelectViewport>
                </SelectContent>
              </SelectPortal>
            </SelectRoot>
            <span class="gcfg-field__hint">讨论主持人;不选则跟随全局默认模型</span>
          </label>

          <!-- C1.2 (09-08-gc-c1-stoploss): per-discussion token
               ceiling. Empty = unlimited (no metadata key). Exceeded →
               the discussion halts at the next round head with
               stop_reason "budget". gce-m4c: 量级参考提示(D4,静态
               文案不进 presets.json)。 -->
          <label class="gcfg-field">
            <span class="gcfg-field__label">Token 预算(可选)</span>
            <input
              v-model="tokenBudgetInput"
              type="number"
              min="1"
              step="1"
              class="gcfg-input"
              placeholder="留空 = 不限"
              data-testid="gcfg-budget"
            />
            <span class="gcfg-field__hint">留空 = 不限;一场讨论通常 20-60 万 token</span>
          </label>

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

          <!-- gce-m4c(09-08,design §4.2):edit 模式成本区。per-speaker
               一行「N · tokens 万单位 · 缓存 x%」(主持人行也显示);有
               预算时顶部进度条,超额满格 + 数字转 error 系;任一查询失败
               该位降级「—」,绝不阻塞编辑。 -->
          <div
            v-if="mode === 'edit'"
            class="gcfg-cost"
            data-testid="gcfg-cost-zone"
          >
            <span class="gcfg-field__label">成本(累计计费 token)</span>
            <div
              v-if="budgetNumber !== null && usageTotal !== null"
              class="gcfg-cost__budget"
              data-testid="gcfg-budget-progress"
            >
              <div class="gcfg-cost__budget-bar">
                <div
                  class="gcfg-cost__budget-fill"
                  :class="{ 'gcfg-cost__budget-fill--over': overBudget }"
                  :style="{ width: budgetFillWidth }"
                />
              </div>
              <span
                class="gcfg-cost__budget-text"
                :class="{ 'gcfg-cost__budget-text--over': overBudget }"
                data-testid="gcfg-budget-text"
              >
                {{ formatTokensWan(usageTotal) }} / {{ formatTokensWan(budgetNumber) }}({{ budgetPct }}%)
              </span>
            </div>
            <ul class="gcfg-cost__rows" data-testid="gcfg-cost-rows">
              <li
                v-for="(row, i) in costRows"
                :key="row.speaker"
                class="gcfg-cost__row"
                :data-testid="`gcfg-cost-row-${i}`"
              >
                <span class="gcfg-cost__speaker">{{ row.label }}</span>
                <span class="gcfg-cost__tokens">
                  tokens {{ formatTokensWan(row.tokens) }}
                </span>
                <span class="gcfg-cost__rate">缓存 {{ row.rate }}</span>
              </li>
            </ul>
          </div>
        </div>

        <div class="gcfg-footer">
          <button
            type="button"
            class="gcfg-btn gcfg-btn--secondary btn btn--muted"
            :data-testid="'gcfg-cancel'"
            @click="cancel"
          >
            取消
          </button>
          <button
            type="button"
            class="gcfg-btn gcfg-btn--primary btn btn--primary"
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
  /* 多行化(preset 模型缺失可同时报主持人 + 参与行);单条消息视觉不变。 */
  flex-direction: column;
  align-items: flex-start;
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

/* 22px 固定尺寸 icon 钮(danger-soft 家族承载红 tint hover);本地仅保留
   显式几何 w/h + padding:0(见 style.css .btn--icon 注)。 */
.gcfg-icon-btn {
  width: 22px;
  height: 22px;
  padding: 0;
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

/* --- gce-m4c(09-08):preset 单选卡区(create)。选中态 accent 边框 +
   选中背景微调(accent-muted);RadioGroupItem 是 <button>,压回视觉
   尺寸,触控目标由整卡 <label> 承担(ScheduledTasksTab 目标卡同款)。 */
.gcfg-presets {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.gcfg-preset-cards {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}

.gcfg-preset-card {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  flex: 1 1 160px;
  /* min-width:auto(= min-content)会被 nowrap 描述行撑到 ~450px,
     三卡挤不下一行逐张换行满宽 —— 显式归零让 160px basis 生效,
     描述交给 .gcfg-preset-desc 的 ellipsis 截断。 */
  min-width: 0;
  padding: 8px 10px;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  cursor: pointer;
  transition: border-color var(--duration-base) var(--ease-out),
    background var(--duration-base) var(--ease-out);
}

.gcfg-preset-card:hover {
  border-color: var(--color-accent-muted);
}

.gcfg-preset-card--active {
  border-color: var(--color-accent);
  background: var(--color-accent-muted);
}

.gcfg-preset-radio {
  width: 14px;
  height: 14px;
  min-width: 14px;
  min-height: 14px;
  margin-top: 2px;
  border-radius: 50%;
  border: 2px solid var(--color-bg-border-strong);
  background: transparent;
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 0;
  cursor: pointer;
  transition: border-color var(--duration-base) var(--ease-out);
}

.gcfg-preset-radio[data-state="checked"] {
  border-color: var(--color-accent);
}

.gcfg-preset-radio-indicator {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--color-accent);
  display: block;
}

.gcfg-preset-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.gcfg-preset-name {
  font-size: var(--text-sm);
  font-weight: var(--weight-medium);
  color: var(--color-text-primary);
}

.gcfg-preset-card--active .gcfg-preset-name {
  color: var(--color-accent-text);
}

.gcfg-preset-desc {
  font-size: var(--text-xs);
  line-height: var(--leading-normal);
  color: var(--color-text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 表单 hint 行(预算量级提示 / 主持人说明)。 */
.gcfg-field__hint {
  font-size: var(--text-xs);
  line-height: var(--leading-normal);
  color: var(--color-text-muted);
}

.gcfg-error__line {
  margin: 0;
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

/* + 添加参与者:btn--outline 家族 + 本地覆写虚线描边(add-zone 形态,
   家族无 dashed 变体);border 色相/hover 抬底保留 muted 混色。 */
.gcfg-add {
  width: 100%;
  border-style: dashed;
  border-color: color-mix(in srgb, var(--color-text-muted) 45%, transparent);
}

.gcfg-add:hover:not(:disabled) {
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

/* --- gce-m4c(09-08):edit 成本区。进度条 + per-speaker「tokens · 缓存率」
   合并行;数字走 mono 家族(与 tokenUsage 计数位一致)。 --- */
.gcfg-cost {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 10px 12px;
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  background: var(--color-bg-surface);
}

.gcfg-cost__budget {
  display: flex;
  align-items: center;
  gap: 8px;
}

.gcfg-cost__budget-bar {
  flex: 1;
  height: 6px;
  border-radius: var(--radius-pill);
  background: var(--color-bg-app);
  overflow: hidden;
}

.gcfg-cost__budget-fill {
  height: 100%;
  border-radius: var(--radius-pill);
  background: var(--color-accent);
  transition: width var(--duration-slow) var(--ease-out);
}

/* 超额:满格条转 tool-error(图形 500 档),数字转 error-text 400 档。 */
.gcfg-cost__budget-fill--over {
  background: var(--color-tool-error);
}

.gcfg-cost__budget-text {
  flex-shrink: 0;
  font-size: var(--text-xs);
  font-family: var(--font-mono);
  color: var(--color-text-secondary);
}

.gcfg-cost__budget-text--over {
  color: var(--color-tool-error-text);
}

.gcfg-cost__rows {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.gcfg-cost__row {
  display: flex;
  align-items: baseline;
  gap: 8px;
  font-size: var(--text-xs);
  min-width: 0;
}

.gcfg-cost__speaker {
  color: var(--color-text-primary);
  flex-shrink: 0;
}

.gcfg-cost__tokens {
  font-family: var(--font-mono);
  color: var(--color-text-secondary);
}

.gcfg-cost__rate {
  margin-left: auto;
  font-family: var(--font-mono);
  color: var(--color-text-muted);
}

/* --- 移动端(@media 全屏块命中 .gcfg-content 由全局 style.css 承担;
   此处只做新区的布局自适应:preset 卡纵向堆叠,成本行允许换行)。 --- */
@media (max-width: 767px) {
  .gcfg-preset-card {
    flex-basis: 100%;
    min-height: 44px;
  }

  .gcfg-cost__row {
    flex-wrap: wrap;
  }
}

/* footer 按钮样式由全局 .btn 家族承载(secondary = muted / primary =
   primary);BEM 类仅为锚点保留。 */
</style>
