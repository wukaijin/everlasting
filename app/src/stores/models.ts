import { defineStore } from "pinia";
import { ref, computed } from "vue";
import { transport } from "../transport";

/** TypeScript type mirroring the backend `ModelWithProvider` IPC payload.
 *  The backend uses `#[serde(flatten)]` so model fields and denormalized
 *  provider fields all appear at the top level. Field names are camelCase
 *  (Tauri 2 auto-converts from Rust snake_case via
 *  `#[serde(rename_all = "camelCase")]`). */
export interface ModelWithProvider {
  id: string;
  providerId: string;
  modelName: string;
  displayName: string;
  /** `null` means "fall back to the global default (128000)". */
  maxTokens: number | null;
  /** `null` means "fall back to the global default (high)". */
  thinkingEffort: string | null;
  supportsThinking: boolean;
  /** B1 (2026-08-16) image-multimodal R1: the model accepts image
   *  blocks (vision). `false` → the wire layer downgrades images to
   *  text placeholders (R3) and the frontend toasts a hint on send.
   *  Explicitly configured — no model-name heuristics (design §1). */
  supportsImages: boolean;
  contextWindow: number;
  /** 2026-09-07 (provider-model-disable): 模型级禁用开关。可选 —— 旧
   *  daemon 不回传该字段,undefined 按 false(启用)处理。有效禁用 =
   *  `disabled || providerDisabled`;禁用只过滤「选用列表」,不影响
   *  已在用的会话 / 全局默认的分发。 */
  disabled?: boolean;
  /** 父 provider 的禁用态(后端 JOIN 反范式)。可选,语义同上。 */
  providerDisabled?: boolean;
  createdAt: string;
  updatedAt: string;
  // Denormalized from the parent provider row (via JOIN).
  providerDisplayName: string;
  providerProtocol: string;
}

/** 有效禁用判定(旧 daemon 无字段 → undefined 按 false 容错)。 */
export function isModelEffectivelyDisabled(m: ModelWithProvider): boolean {
  return !!(m.disabled || m.providerDisabled);
}

export const useModelsStore = defineStore("models", () => {
  const models = ref<ModelWithProvider[]>([]);
  const defaultModelId = ref<string | null>(null);
  const loaded = ref(false);

  /** The currently selected default model, resolved from the catalog. */
  const defaultModel = computed<ModelWithProvider | null>(() => {
    if (!defaultModelId.value) return null;
    return models.value.find((m) => m.id === defaultModelId.value) ?? null;
  });

  /** Models grouped by provider — for the ModelSelect dropdown in
   *  the chat input and the Models tab grouped list. Each group
   *  carries the provider's display name and protocol alongside
   *  its models. */
  const modelsGroupedByProvider = computed(() => {
    const groups = new Map<
      string,
      {
        provider: {
          id: string;
          displayName: string;
          protocol: string;
        };
        models: ModelWithProvider[];
      }
    >();
    for (const m of models.value) {
      if (!groups.has(m.providerId)) {
        groups.set(m.providerId, {
          provider: {
            id: m.providerId,
            displayName: m.providerDisplayName,
            protocol: m.providerProtocol,
          },
          models: [],
        });
      }
      groups.get(m.providerId)!.models.push(m);
    }
    return Array.from(groups.values());
  });

  /** 2026-09-07 (provider-model-disable): 有效启用的模型(过滤
   *  `disabled || providerDisabled`)。供各「选用」入口消费 —— 模型
   *  下拉 / 群聊阵容 / 定时任务 / 默认模型单选;Settings 的 Models
   *  列表仍用全量 `models`(禁用行要可见、可管理)。当前已选中但被
   *  禁用的模型由各消费方自行兜底显示(展示查全量,选项查本表)。 */
  const enabledModels = computed<ModelWithProvider[]>(() =>
    models.value.filter((m) => !isModelEffectivelyDisabled(m)),
  );

  /** `enabledModels` 的按 provider 分组形态(镜像
   *  `modelsGroupedByProvider` 的分组逻辑,仅供选用列表)。 */
  const enabledModelsGroupedByProvider = computed(() => {
    const groups = new Map<
      string,
      {
        provider: { id: string; displayName: string; protocol: string };
        models: ModelWithProvider[];
      }
    >();
    for (const m of enabledModels.value) {
      if (!groups.has(m.providerId)) {
        groups.set(m.providerId, {
          provider: {
            id: m.providerId,
            displayName: m.providerDisplayName,
            protocol: m.providerProtocol,
          },
          models: [],
        });
      }
      groups.get(m.providerId)!.models.push(m);
    }
    return Array.from(groups.values());
  });

  /** Fetch all models + the current default. Replaces the entire
   *  in-memory list on success. */
  async function load() {
    const [modelList, def] = await Promise.all([
      transport.invoke<ModelWithProvider[]>("list_models"),
      transport.invoke<ModelWithProvider | null>("get_default_model"),
    ]);
    models.value = modelList;
    defaultModelId.value = def?.id ?? null;
    loaded.value = true;
  }

  /** Add a new model. `add_model` returns a `ModelRow` (without the
   *  denormalized provider fields), so we reload the full list to get
   *  the complete `ModelWithProvider` shape. */
  async function add(
    providerId: string,
    modelName: string,
    displayName: string,
    opts: {
      maxTokens?: number;
      thinkingEffort?: string;
      supportsThinking: boolean;
      supportsImages: boolean;
      contextWindow: number;
    },
  ) {
    // Spread `opts` so `undefined` fields are omitted (not sent as
    // `null`) — Tauri 2 IPC treats `null` as a missing required
    // field and the error message hides the field name.
    // See HACKING-wsl FU-1.
    await transport.invoke("add_model", {
      providerId,
      modelName,
      displayName,
      ...opts,
    });
    await load();
  }

  /** Update an existing model. Reloads the list to refresh the
   *  denormalized provider fields. */
  async function update(
    id: string,
    providerId: string,
    modelName: string,
    displayName: string,
    opts: {
      maxTokens?: number;
      thinkingEffort?: string;
      supportsThinking: boolean;
      supportsImages: boolean;
      contextWindow: number;
    },
  ) {
    await transport.invoke("update_model", { id, providerId, modelName, displayName, ...opts });
    await load();
  }

  /** Delete a model by id. Removes from the in-memory list on success.
   *  Note: this leaves dangling `sessions.model_id` references — the
   *  backend resolve-default fallback handles them transparently. */
  async function remove(id: string) {
    const ok = await transport.invoke<boolean>("delete_model", { id });
    if (ok) models.value = models.value.filter((m) => m.id !== id);
    return ok;
  }

  /** Set the default model. Persists to `app_config.default_model_id`
   *  and updates the local ref immediately (optimistic). */
  async function setDefault(modelId: string) {
    await transport.invoke("set_default_model", { modelId });
    defaultModelId.value = modelId;
  }

  /** 2026-09-07 (provider-model-disable): 翻转模型禁用态并整表刷新
   *  (providerDisabled 反范式随 list_models 回来;选用列表走 computed
   *  自动联动)。参数名用 `id` —— 后端 Tauri 命令参数与 daemon 路由
   *  字段都叫 id(delete_model/delete_provider 同款约定);发 modelId
   *  会在 HTTP 路径变成 model_id 导致 422 missing field `id`。 */
  async function setDisabled(id: string, disabled: boolean) {
    await transport.invoke("set_model_disabled", { id, disabled });
    await load();
  }

  /** Look up a model by id. Returns `undefined` if not found. */
  function byId(id: string): ModelWithProvider | undefined {
    return models.value.find((m) => m.id === id);
  }

  /** Get all models belonging to a specific provider. */
  function modelsByProvider(providerId: string): ModelWithProvider[] {
    return models.value.filter((m) => m.providerId === providerId);
  }

  return {
    models,
    defaultModelId,
    defaultModel,
    loaded,
    modelsGroupedByProvider,
    enabledModels,
    enabledModelsGroupedByProvider,
    load,
    add,
    update,
    remove,
    setDefault,
    setDisabled,
    byId,
    modelsByProvider,
  };
});
