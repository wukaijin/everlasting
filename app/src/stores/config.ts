import { defineStore } from "pinia";
import { ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";

interface LlmConfig {
  model: string;
  baseUrl: string;
  configured: boolean;
}

/** localStorage key for the last active project id. Restored on app
 *  start (Q1 / PROPOSAL §5.5). The value is a project UUID; if it
 *  doesn't match any loaded project on start, the chat store's
 *  watcher falls back to the first visible project. */
const LAST_ACTIVE_PROJECT_KEY = "everlasting.lastActiveProjectId";

export const useConfigStore = defineStore("config", () => {
  const model = ref<string>("");
  const baseUrl = ref<string>("");
  const configured = ref(false);
  const loaded = ref(false);

  // Persisted across sessions via localStorage. Loaded synchronously
  // at store creation so it's available before the chat store's
  // watcher fires its first run.
  const lastActiveProjectId = ref<string | null>(readLastActive());

  function readLastActive(): string | null {
    try {
      return window.localStorage.getItem(LAST_ACTIVE_PROJECT_KEY);
    } catch {
      return null;
    }
  }

  function writeLastActive(id: string | null): void {
    try {
      if (id) {
        window.localStorage.setItem(LAST_ACTIVE_PROJECT_KEY, id);
      } else {
        window.localStorage.removeItem(LAST_ACTIVE_PROJECT_KEY);
      }
    } catch {
      // localStorage may be unavailable (private mode, etc.) — fail
      // silently; the in-memory value is still correct.
    }
  }

  // Persist on every change. The chat store updates
  // `lastActiveProjectId` whenever the user switches tabs.
  watch(lastActiveProjectId, (id) => {
    writeLastActive(id);
  });

  async function load() {
    try {
      const cfg = await invoke<LlmConfig>("get_llm_config");
      model.value = cfg.model;
      baseUrl.value = cfg.baseUrl;
      configured.value = cfg.configured;
    } catch (e) {
      console.error("failed to load LLM config:", e);
    } finally {
      loaded.value = true;
    }
  }

  return {
    model,
    baseUrl,
    configured,
    loaded,
    lastActiveProjectId,
    load,
  };
});
