import { defineStore } from "pinia";
import { ref, computed, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";

import { useProvidersStore } from "./providers";
import { useModelsStore } from "./models";

/** localStorage key for the last active project id. Restored on app
 *  start (Q1 / PROPOSAL §5.5). The value is a project UUID; if it
 *  doesn't match any loaded project on start, the chat store's
 *  watcher falls back to the first visible project. */
const LAST_ACTIVE_PROJECT_KEY = "everlasting.lastActiveProjectId";
const LAST_SESSION_KEY_PREFIX = "everlasting.lastSession_";

export const useConfigStore = defineStore("config", () => {
  const loaded = ref(false);

  // PR3 (BACKLOG §5.1 follow-up): the home directory is fetched once
  // on app start and cached here so the chat panel header can
  // shorten the cwd display (`/home/carlos/code/foo` -> `~/code/foo`).
  // `null` means "not yet loaded" or "load failed" — in either case
  // the helper `simplifyPath` returns the original path unchanged,
  // so the UI is safe to render before this resolves.
  const homeDir = ref<string | null>(null);

  // Persisted across sessions via localStorage. Loaded synchronously
  // at store creation so it's available before the chat store's
  // watcher fires its first run.
  const lastActiveProjectId = ref<string | null>(readLastActive());

  // -----------------------------------------------------------------------
  // Backward-compatible computed properties (derived from the catalog).
  // Existing components (Settings status badges, etc.) still read these.
  // Step 5 will clean up all call sites and remove these fields.
  // -----------------------------------------------------------------------

  /** The display name of the current default model, or `""` if none.
   *  Note: Pinia auto-unwraps refs on the store proxy, so
   *  `useModelsStore().defaultModel` is already `ModelWithProvider | null`
   *  (not a ComputedRef). We read it directly without `.value`. */
  const model = computed<string>(() => {
    const modelsStore = useModelsStore();
    return modelsStore.defaultModel?.displayName ?? "";
  });

  /** The base URL of the default model's provider, or `""` if none. */
  const baseUrl = computed<string>(() => {
    const modelsStore = useModelsStore();
    const dm = modelsStore.defaultModel;
    if (!dm) return "";
    const provider = useProvidersStore().byId(dm.providerId);
    return provider?.baseUrl ?? "";
  });

  /** True when a default model exists AND its provider has a non-empty
   *  api_key. Drives the Settings tab's "(api key 未设置)" hint and
   *  the warn styling. */
  const configured = computed<boolean>(() => {
    const modelsStore = useModelsStore();
    const dm = modelsStore.defaultModel;
    if (!dm) return false;
    const provider = useProvidersStore().byId(dm.providerId);
    return !!provider?.apiKey;
  });

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

  // F1: per-project last active session persistence.
  function readLastSession(projectId: string): string | null {
    try {
      return window.localStorage.getItem(LAST_SESSION_KEY_PREFIX + projectId);
    } catch {
      return null;
    }
  }

  function writeLastSession(projectId: string, sessionId: string | null): void {
    try {
      if (sessionId) {
        window.localStorage.setItem(LAST_SESSION_KEY_PREFIX + projectId, sessionId);
      } else {
        window.localStorage.removeItem(LAST_SESSION_KEY_PREFIX + projectId);
      }
    } catch {
      // fail silently
    }
  }

  // Persist on every change. The chat store updates
  // `lastActiveProjectId` whenever the user switches tabs.
  watch(lastActiveProjectId, (id) => {
    writeLastActive(id);
  });

  async function load() {
    // Load providers + models from the catalog (replaces the old
    // `get_llm_config` env path). Store references are obtained at
    // runtime (inside the function body) to avoid Pinia circular
    // dependency issues during setup.
    const providersStore = useProvidersStore();
    const modelsStore = useModelsStore();

    await Promise.all([providersStore.load(), modelsStore.load()]);

    // PR3: home_dir is a best-effort cache for display. A failure
    // (rare — sandboxed container without `$HOME`) is logged but
    // never propagates; the UI degrades to rendering the full
    // cwd path. We deliberately do NOT roll this into the same
    // `try` as the catalog: a missing provider/api_key would
    // otherwise mask the home-dir load.
    try {
      homeDir.value = await invoke<string | null>("get_home_dir");
    } catch (e) {
      console.error("failed to load home dir:", e);
      homeDir.value = null;
    } finally {
      loaded.value = true;
    }
  }

  return {
    model,
    baseUrl,
    configured,
    loaded,
    homeDir,
    lastActiveProjectId,
    readLastSession,
    writeLastSession,
    load,
  };
});
