// Memory store — wraps the 3 B5 memory Tauri commands into a reactive
// cache that the Memory Preview panel reads from, plus the 2 P2
// runtime-memory commands (`list_autonomous_memories` /
// `delete_autonomous_memory`).
//
// PRD: B5 Memory V2 1 期 (2026-06-10). PR1 of the task (backend) shipped
// the 3 commands: `read_memory_layers`, `read_memory_content`,
// `open_memory_in_editor`. This store is the single source of truth
// for the frontend's view of the 4 fixed memory files (User CLAUDE.md
// + User AGENTS.md + Project CLAUDE.md + Project AGENTS.md) AND the
// P2 runtime memories (autonomous memories the agent wrote via
// `remember` tool / will be written via P4 reflection).
//
// State model:
//   - `layers` is a `MemoryLayerInfo[]` summary list (no `content`).
//   - `contentCache` is a `Map<path, string>` of lazily-fetched bodies.
//   - `loading` / `error` / `lastProjectId` are bookkeeping for the
//     panel's render state.
//   - `runtimeMemories` is the P2 list of `AutonomousMemory` rows
//     visible to the current project (user-scope + project-scope).
//     `runtimeMemoriesLoading` / `runtimeMemoriesError` mirror the
//     layer fetch's loading/error pattern for the new section.
//
// Re-fetch triggers:
//   1. `loadForProject(projectId)` on panel mount + on project change.
//   2. The "刷新" button in the panel header, exposed via `refresh()`.
//   3. The runtime-memories section's own 刷新 button + the
//      auto-fetch on mount, exposed via `fetchMemories()`.
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

// --- P2 (2026-06-29): AutonomousMemory — mirror of the Rust
// `db::memories::MemoryRow` crossing the IPC boundary. The Rust
// struct derives `#[serde(rename_all = "camelCase")]` so the wire
// shape uses camelCase fields (matches the project-wide convention
// in `stores/subagentRuns.types.ts` and `stores/permissions.ts`).
//
// `tags` and `pathGlobs` are stored as JSON TEXT in the DB. The
// Rust side round-trips them verbatim, so the wire exposes them as
// raw JSON strings — the frontend parses on demand (the preview
// list never needs to drill into them; full MarkdownDetailModal
// would). For the P2 PR3 list view, both fields are treated as
// opaque strings.
//
// Field semantics (mirrors `db/memories.rs:MemoryRow`):
//   - `id`           : SQLite auto-id (display-only; the IPC
//                      delete key is `memoryId`).
//   - `memoryId`     : UUID v7 — the global key. Stable across
//                      re-imports; the canonical identifier.
//   - `scope`        : "user" | "project" — visibility layer.
//   - `projectId`    : present iff `scope === "project"`.
//   - `kind`         : "pitfall" | "preference" | "fact" | "decision".
//   - `status`       : "candidate" | "active" | "verified" | "demoted".
//   - `title`        : ≤200 chars, user-visible header.
//   - `content`      : ≤500 chars, user-visible body.
//   - `tags`         : JSON-encoded Vec<String>.
//   - `toolName`     : pitfall trigger key (set iff kind=pitfall).
//   - `commandPattern`: pitfall trigger key (set iff kind=pitfall).
//   - `pathGlobs`    : pitfall trigger key (JSON Vec<String>).
//   - `sourceSessionId`: provenance — which chat session wrote it.
//   - `sourceRef`    : opaque provenance ref (e.g. "remember tool
//                      call 3"; future P4 will set to "auto-reflect").
//   - `confidence`   : 0.0..=1.0 (P5 status machine).
//   - `hitCount`     : recall count (P5 status machine).
//   - `lastUsedAt`   : RFC 3339 string of last recall, or null.
//   - `createdAt`    : RFC 3339 string.
//   - `updatedAt`    : RFC 3339 string.
//   - `demotedReason`: free-form reason set when status → demoted.
export interface AutonomousMemory {
  id: number;
  memoryId: string;
  scope: string;
  projectId: string | null;
  kind: string;
  status: string;
  title: string;
  content: string;
  tags: string;
  toolName: string | null;
  commandPattern: string | null;
  pathGlobs: string | null;
  sourceSessionId: string | null;
  sourceRef: string | null;
  confidence: number;
  hitCount: number;
  lastUsedAt: string | null;
  createdAt: string;
  updatedAt: string;
  demotedReason: string | null;
}

export const useMemoryStore = defineStore("memory", () => {
  // ---------------------------------------------------------------------
  // State — instruction-file section (B5 PR2; unchanged)
  // ---------------------------------------------------------------------

  const layers = ref<MemoryLayerInfo[]>([]);
  const contentCache = ref<Map<string, string>>(new Map());
  const loading = ref<boolean>(false);
  const error = ref<string | null>(null);
  const lastProjectId = ref<string | null>(null);

  // ---------------------------------------------------------------------
  // State — runtime-memories section (P2 PR3; additive)
  // ---------------------------------------------------------------------

  const runtimeMemories = ref<AutonomousMemory[]>([]);
  const runtimeMemoriesLoading = ref<boolean>(false);
  const runtimeMemoriesError = ref<string | null>(null);

  // ---------------------------------------------------------------------
  // Fetching — instruction-file section (unchanged)
  // ---------------------------------------------------------------------

  /** Fetch the per-session memory layer summary for the given
   *  project. On success, populates `layers`. On failure, sets
   *  `error` and leaves `layers` at its previous value (defensive
   *  — the panel can still render the stale state).
   *
   *  P2 PR3: also clears the runtime-memories list. The list is
   *  project-isolated; showing proj-A's memories while proj-B's
   *  instruction-file layers are loading is a leak. The next
   *  `fetchMemories` (fired in parallel by `loadForProject`)
   *  repopulates with proj-B's rows. */
  async function fetchLayers(projectId: string): Promise<void> {
    loading.value = true;
    // Set `lastProjectId` synchronously (BEFORE the IPC awaits) so
    // the parallel `fetchMemories` call in `loadForProject` can read
    // it. Otherwise `Promise.all` schedules the two fetches
    // concurrently, and `fetchMemories` would see the old
    // `lastProjectId` (null) and short-circuit.
    lastProjectId.value = projectId;
    // Clear the project-scoped runtime list on project change
    // (defensive: prevents stale rows from being rendered during
    // the brief fetch window).
    runtimeMemories.value = [];
    runtimeMemoriesError.value = null;
    try {
      const next = await invoke<MemoryLayerInfo[]>("read_memory_layers", {
        projectId,
      });
      layers.value = next;
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
   *  state.
   *
   *  P2 PR3: also kicks off the runtime-memories fetch. The two
   *  fetches are independent (the instruction-file IPC is
   *  mtime-fenced, the runtime-memory IPC is a DB read), so we
   *  fire them in parallel — `Promise.all` shortens the panel's
   *  perceived load time on project switch. */
  async function loadForProject(projectId: string): Promise<void> {
    await Promise.all([fetchLayers(projectId), fetchMemories()]);
  }

  /** Manually re-fetch (e.g. from the panel's 刷新 button).
   *
   *  P2 PR3: refreshes BOTH the instruction-file layers AND the
   *  runtime-memories list. The panel's single 刷新 button is
   *  the user-facing entry point; it should refresh everything
   *  the panel displays, not just the instruction-file section. */
  async function refresh(): Promise<void> {
    if (lastProjectId.value) {
      await Promise.all([
        fetchLayers(lastProjectId.value),
        fetchMemories(),
      ]);
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

  // ---------------------------------------------------------------------
  // P2 PR3: runtime-memory fetching / deleting
  // ---------------------------------------------------------------------

  /** Fetch the list of autonomous (runtime) memories visible to the
   *  current project. The backend's `list_autonomous_memories`
   *  command is project-isolated: user-scope rows (global) plus
   *  this project's own project-scope rows; a project-scope row in
   *  proj-A is never surfaced when querying proj-B. Newest first
   *  (the DB ORDER BY `created_at DESC`).
   *
   *  Mirrors the instruction-file `fetchLayers` error policy: a
   *  failure is stored in `runtimeMemoriesError` and the previous
   *  list is left intact (defensive — the panel can still render
   *  the stale state with an error banner). */
  async function fetchMemories(): Promise<void> {
    if (!lastProjectId.value) {
      // No project → no memories to fetch. Mirror the instruction
      // file section: render the empty state, not an error. The
      // panel only shows the runtime-memories section when a
      // project is selected, so this is a defensive guard against
      // a race (e.g. project deselected mid-fetch).
      runtimeMemories.value = [];
      runtimeMemoriesError.value = null;
      return;
    }
    runtimeMemoriesLoading.value = true;
    try {
      const next = await invoke<AutonomousMemory[]>(
        "list_autonomous_memories",
        { projectId: lastProjectId.value },
      );
      runtimeMemories.value = next;
      runtimeMemoriesError.value = null;
    } catch (e) {
      runtimeMemoriesError.value = String(e);
    } finally {
      runtimeMemoriesLoading.value = false;
    }
  }

  /** Delete a single runtime memory by its `memoryId` (the UUID
   *  v7, the canonical key). Optimistic: on success, the row is
   *  removed from `runtimeMemories` without re-fetching. On
   *  failure, the error is stored in `runtimeMemoriesError` and
   *  the list is left unchanged.
   *
   *  The backend's `delete_autonomous_memory` is idempotent
   *  (returns `Ok(0)` for an already-deleted row), so a race
   *  between two delete clicks is safe.
   *
   *  The backend does NOT take a project_id — `memoryId` is
   *  globally unique. The MemoryPreview only displays memories
   *  already filtered to the current project, so the user can
   *  only see + click delete on memories they're allowed to
   *  manage. */
  async function deleteMemory(id: number): Promise<void> {
    // Resolve the row by `id` (the SQLite auto-id) — the panel's
    // `v-for :key` is on `id` (display-stable), and we need the
    // UUID `memoryId` for the IPC.
    const target = runtimeMemories.value.find((m) => m.id === id);
    if (!target) {
      // Defensive: the row vanished between render and click
      // (e.g. a concurrent delete from another panel mount). Treat
      // as a no-op so the optimistic remove below still runs.
      runtimeMemories.value = runtimeMemories.value.filter(
        (m) => m.id !== id,
      );
      return;
    }
    try {
      await invoke<number>("delete_autonomous_memory", {
        memoryId: target.memoryId,
      });
      // Optimistic remove: filter the row out of the in-memory
      // list. Cheaper than a full refetch and keeps the user's
      // place (scroll, expansion, etc.) intact.
      runtimeMemories.value = runtimeMemories.value.filter(
        (m) => m.id !== id,
      );
      runtimeMemoriesError.value = null;
    } catch (e) {
      runtimeMemoriesError.value = String(e);
    }
  }

  return {
    // instruction-file state
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
    // runtime-memory state (P2 PR3; additive)
    runtimeMemories,
    runtimeMemoriesLoading,
    runtimeMemoriesError,
    fetchMemories,
    deleteMemory,
  };
});
