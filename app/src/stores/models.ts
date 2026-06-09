import { defineStore } from "pinia";
import { ref, computed } from "vue";
import { invoke } from "@tauri-apps/api/core";

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
  /** `null` means "fall back to the global default (16384)". */
  maxTokens: number | null;
  /** `null` means "fall back to the global default (high)". */
  thinkingEffort: string | null;
  supportsThinking: boolean;
  contextWindow: number;
  createdAt: string;
  updatedAt: string;
  // Denormalized from the parent provider row (via JOIN).
  providerDisplayName: string;
  providerProtocol: string;
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

  /** Fetch all models + the current default. Replaces the entire
   *  in-memory list on success. */
  async function load() {
    const [modelList, def] = await Promise.all([
      invoke<ModelWithProvider[]>("list_models"),
      invoke<ModelWithProvider | null>("get_default_model"),
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
      contextWindow: number;
    },
  ) {
    // Spread `opts` so `undefined` fields are omitted (not sent as
    // `null`) — Tauri 2 IPC treats `null` as a missing required
    // field and the error message hides the field name.
    // See HACKING-wsl FU-1.
    await invoke("add_model", {
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
      contextWindow: number;
    },
  ) {
    await invoke("update_model", { id, providerId, modelName, displayName, ...opts });
    await load();
  }

  /** Delete a model by id. Removes from the in-memory list on success.
   *  Note: this leaves dangling `sessions.model_id` references — the
   *  backend resolve-default fallback handles them transparently. */
  async function remove(id: string) {
    const ok = await invoke<boolean>("delete_model", { id });
    if (ok) models.value = models.value.filter((m) => m.id !== id);
    return ok;
  }

  /** Set the default model. Persists to `app_config.default_model_id`
   *  and updates the local ref immediately (optimistic). */
  async function setDefault(modelId: string) {
    await invoke("set_default_model", { modelId });
    defaultModelId.value = modelId;
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
    load,
    add,
    update,
    remove,
    setDefault,
    byId,
    modelsByProvider,
  };
});
