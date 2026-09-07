import { defineStore } from "pinia";
import { ref } from "vue";
import { transport } from "../transport";

/** TypeScript type mirroring the backend `ProviderRow` IPC payload.
 *  Field names are camelCase (Tauri 2 auto-converts from Rust snake_case
 *  via `#[serde(rename_all = "camelCase")]`). */
export interface ProviderRow {
  id: string;
  protocol: string; // "anthropic" | "openai"
  displayName: string;
  baseUrl: string;
  /** RULE-D-001 (2026-06-24): 是否已设置 api_key. 后端不再回传明文 key
   *  (ProviderRow.api_key `#[serde(skip)]`), 前端只知"有没有". */
  hasKey: boolean;
  /** 2026-09-07 (provider-model-disable): 禁用开关。可选 —— 旧 daemon
   *  不回传该字段,undefined 按 false(启用)处理。`true` = 该 provider
   *  及其全部模型从各模型选择列表隐藏(模型侧经 list_models 的
   *  providerDisabled 反范式联动);分发明不受影响。 */
  disabled?: boolean;
  createdAt: string;
  updatedAt: string;
}

export const useProvidersStore = defineStore("providers", () => {
  const providers = ref<ProviderRow[]>([]);
  const loaded = ref(false);

  /** Fetch all providers from the backend. Replaces the entire in-memory
   *  list on success. */
  async function load() {
    providers.value = await transport.invoke<ProviderRow[]>("list_providers");
    loaded.value = true;
  }

  /** Create a new provider and append it to the in-memory list. */
  async function add(
    protocol: string,
    displayName: string,
    baseUrl: string,
    apiKey: string,
  ) {
    const row = await transport.invoke<ProviderRow>("add_provider", {
      protocol,
      displayName,
      baseUrl,
      apiKey,
    });
    providers.value.push(row);
    return row;
  }

  /** Update an existing provider. Refreshes the in-memory entry on success.
   *  RULE-D-001: apiKey 留空(undefined)=保持原 key; 传值=覆盖.
   *  undefined 时省略 apiKey 字段 → Rust `Option<String>` = None. */
  async function update(
    id: string,
    protocol: string,
    displayName: string,
    baseUrl: string,
    apiKey?: string,
  ) {
    const payload: Record<string, string> = { id, protocol, displayName, baseUrl };
    if (apiKey && apiKey.trim()) payload.apiKey = apiKey.trim();
    const row = await transport.invoke<ProviderRow | null>("update_provider", payload);
    if (row) {
      const idx = providers.value.findIndex((p) => p.id === id);
      if (idx >= 0) providers.value[idx] = row;
    }
    return row;
  }

  /** Delete a provider by id. Removes from the in-memory list on success.
   *  Backend cascades to associated models (ON DELETE CASCADE). */
  async function remove(id: string) {
    const ok = await transport.invoke<boolean>("delete_provider", { id });
    if (ok) providers.value = providers.value.filter((p) => p.id !== id);
    return ok;
  }

  /** 2026-09-07 (provider-model-disable): 翻转 provider 禁用态。回读
   *  整行替换;调用方还需刷新 modelsStore(models.providerDisabled
   *  是 list_models 的 JOIN 反范式,不随本表联动)。 */
  async function setDisabled(id: string, disabled: boolean) {
    const row = await transport.invoke<ProviderRow | null>("set_provider_disabled", {
      id,
      disabled,
    });
    if (row) {
      const idx = providers.value.findIndex((p) => p.id === id);
      if (idx >= 0) providers.value[idx] = row;
    }
    return row;
  }

  /** Look up a provider by id. Returns `undefined` if not found. */
  function byId(id: string): ProviderRow | undefined {
    return providers.value.find((p) => p.id === id);
  }

  return { providers, loaded, load, add, update, remove, setDisabled, byId };
});
