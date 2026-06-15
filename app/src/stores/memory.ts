// Memory store — wraps the 3 B5 memory Tauri commands into a reactive
// cache that the Memory Preview panel reads from.
//
// PRD: B5 Memory V2 1 期 (2026-06-10). PR1 of the task (backend) shipped
// the 3 commands: `read_memory_layers`, `read_memory_content`,
// `open_memory_in_editor`. This store is the single source of truth
// for the frontend's view of the 4 fixed memory files (User CLAUDE.md
// + User AGENTS.md + Project CLAUDE.md + Project AGENTS.md).
//
// State model:
//   - `layers` is a `MemoryLayerInfo[]` summary list (no `content`).
//   - `contentCache` is a `Map<path, string>` of lazily-fetched bodies.
//   - `loading` / `error` / `lastProjectId` are bookkeeping for the
//     panel's render state.
//
// Re-fetch triggers:
//   1. `loadForProject(projectId)` on panel mount + on project change.
//   2. The "刷新" button in the panel header, exposed via `refresh()`.
//
// Freshness note (2026-06-15): the backend has NO background watcher
// now — every `read_memory_layers` call stats each file's mtime and
// reloads on change (RULE-C-001 fence). So a re-fetch (trigger 1 or
// 2) always returns current state; there is no event to listen for.
//
// Failure policy: any `invoke` failure is caught and stored in
// `error`; the layers array is left at its previous value so the
// panel can render the stale state with an error banner, instead of
// crashing. The frontend never propagates a Rust `Err` to the panel.

import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

// --- Types — mirror `MemoryLayerInfo` from the Rust side. -----------
// Field names are snake_case to match the serde renaming on the
// backend (`#[serde(rename_all = "lowercase")]` on `MemoryKind`,
// `#[serde(rename_all = "snake_case")]` on `MemorySource`, the
// `LayerStatus` is a tag/discriminated union).
//
// IMPORTANT: PathBuf serializes to a string in Tauri's IPC. The
// Rust `PathBuf` does NOT round-trip a JSON object — it serializes
// as a string. So `path` is `string`, not `{ ... }`.

export type MemoryKind = "user" | "project" | "session" | "runtime";

export type MemorySource = "claude" | "agents";

/** Status: a discriminated union matching the Rust `LayerStatus`:
 *  - `Loaded`     → `null` (no extra payload)
 *  - `Missing`    → `null`
 *  - `Error`      → `{ reason: string }`
 *  The `kind` field is the discriminator. */
export type LayerStatus =
  | { kind: "loaded" }
  | { kind: "missing" }
  | { kind: "error"; reason: string };

export interface MemoryLayerInfo {
  kind: MemoryKind;
  source: MemorySource;
  path: string;
  tokens: number;
  status: LayerStatus;
  char_count: number;
}

export const useMemoryStore = defineStore("memory", () => {
  // ---------------------------------------------------------------------
  // State
  // ---------------------------------------------------------------------

  const layers = ref<MemoryLayerInfo[]>([]);
  const contentCache = ref<Map<string, string>>(new Map());
  const loading = ref<boolean>(false);
  const error = ref<string | null>(null);
  const lastProjectId = ref<string | null>(null);

  // ---------------------------------------------------------------------
  // Fetching
  // ---------------------------------------------------------------------

  /** Fetch the per-session memory layer summary for the given
   *  project. On success, populates `layers`. On failure, sets
   *  `error` and leaves `layers` at its previous value (defensive
   *  — the panel can still render the stale state). */
  async function fetchLayers(projectId: string): Promise<void> {
    loading.value = true;
    try {
      const next = await invoke<MemoryLayerInfo[]>("read_memory_layers", {
        projectId,
      });
      layers.value = next;
      lastProjectId.value = projectId;
      // Invalidate the content cache when the layer set changes
      // (paths may have moved; stale entries would render old
      // bodies for new paths). Easiest correct behavior: drop the
      // entire cache; re-fetches are cheap.
      contentCache.value = new Map();
      error.value = null;
    } catch (e) {
      error.value = String(e);
    } finally {
      loading.value = false;
    }
  }

  /** One-shot entry point used by the Memory Preview panel on
   *  mount / on project change. Wires the reload listener and
   *  fires the initial fetch. Safe to call multiple times — the
   *  listener is registered once, the fetch just overwrites
   *  state. */
  async function loadForProject(projectId: string): Promise<void> {
    await fetchLayers(projectId);
  }

  /** Manually re-fetch (e.g. from the panel's 刷新 button). */
  async function refresh(): Promise<void> {
    if (lastProjectId.value) {
      await fetchLayers(lastProjectId.value);
    }
  }

  // ---------------------------------------------------------------------
  // Content fetching (lazy, on-demand)
  // ---------------------------------------------------------------------

  /** Fetch the body of a single memory file. Caches by path so
   *  repeated reads of the same layer don't re-invoke the IPC. */
  async function fetchContent(path: string): Promise<string> {
    const cached = contentCache.value.get(path);
    if (cached !== undefined) return cached;
    if (!lastProjectId.value) {
      throw new Error("memory: fetchContent called before loadForProject");
    }
    const text = await invoke<string>("read_memory_content", {
      projectId: lastProjectId.value,
      path,
    });
    // Re-clone the Map so the ref actually changes (Vue's ref
    // equality is reference equality on the Map itself, not
    // deep).
    const next = new Map(contentCache.value);
    next.set(path, text);
    contentCache.value = next;
    return text;
  }

  // ---------------------------------------------------------------------
  // External editor
  // ---------------------------------------------------------------------

  /** Spawn the user's editor ($EDITOR → xdg-open / open / cmd /c
   *  start) for the given memory file. Best-effort; the Rust
   *  side already handles the fallback chain and the IPC returns
   *  `Err` only on hard failures (e.g. project_id not found). */
  async function openInEditor(path: string): Promise<void> {
    if (!lastProjectId.value) {
      throw new Error("memory: openInEditor called before loadForProject");
    }
    await invoke("open_memory_in_editor", {
      projectId: lastProjectId.value,
      path,
    });
  }

  // ---------------------------------------------------------------------
  // Filtering helpers (the Settings page shows only the 2 User
  // layers; the Project panel shows only the 2 Project layers).
  // ---------------------------------------------------------------------

  function layersOfKind(kind: MemoryKind): MemoryLayerInfo[] {
    return layers.value.filter((l) => l.kind === kind);
  }

  return {
    layers,
    contentCache,
    loading,
    error,
    lastProjectId,
    loadForProject,
    refresh,
    fetchContent,
    openInEditor,
    layersOfKind,
  };
});
