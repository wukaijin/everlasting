import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

interface LlmConfig {
  model: string;
  baseUrl: string;
  configured: boolean;
}

export const useConfigStore = defineStore("config", () => {
  const model = ref<string>("");
  const baseUrl = ref<string>("");
  const configured = ref(false);
  const loaded = ref(false);

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

  return { model, baseUrl, configured, loaded, load };
});
