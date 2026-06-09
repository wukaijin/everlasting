import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

/** TypeScript type mirroring the backend `ProviderRow` IPC payload.
 *  Field names are camelCase (Tauri 2 auto-converts from Rust snake_case
 *  via `#[serde(rename_all = "camelCase")]`). */
export interface ProviderRow {
  id: string;
  protocol: string; // "anthropic" | "openai"
  displayName: string;
  baseUrl: string;
  apiKey: string;
  createdAt: string;
  updatedAt: string;
}

export const useProvidersStore = defineStore("providers", () => {
  const providers = ref<ProviderRow[]>([]);
  const loaded = ref(false);

  /** Fetch all providers from the backend. Replaces the entire in-memory
   *  list on success. */
  async function load() {
    providers.value = await invoke<ProviderRow[]>("list_providers");
    loaded.value = true;
  }

  /** Create a new provider and append it to the in-memory list. */
  async function add(
    protocol: string,
    displayName: string,
    baseUrl: string,
    apiKey: string,
  ) {
    const row = await invoke<ProviderRow>("add_provider", {
      protocol,
      displayName,
      baseUrl,
      apiKey,
    });
    providers.value.push(row);
    return row;
  }

  /** Update an existing provider. Refreshes the in-memory entry on success. */
  async function update(
    id: string,
    protocol: string,
    displayName: string,
    baseUrl: string,
    apiKey: string,
  ) {
    const row = await invoke<ProviderRow | null>("update_provider", {
      id,
      protocol,
      displayName,
      baseUrl,
      apiKey,
    });
    if (row) {
      const idx = providers.value.findIndex((p) => p.id === id);
      if (idx >= 0) providers.value[idx] = row;
    }
    return row;
  }

  /** Delete a provider by id. Removes from the in-memory list on success.
   *  Backend cascades to associated models (ON DELETE CASCADE). */
  async function remove(id: string) {
    const ok = await invoke<boolean>("delete_provider", { id });
    if (ok) providers.value = providers.value.filter((p) => p.id !== id);
    return ok;
  }

  /** Look up a provider by id. Returns `undefined` if not found. */
  function byId(id: string): ProviderRow | undefined {
    return providers.value.find((p) => p.id === id);
  }

  return { providers, loaded, load, add, update, remove, byId };
});
