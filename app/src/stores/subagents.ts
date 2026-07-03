// useSubagentsStore — Pinia store for per-subagent model configuration
// (Settings → Subagents tab).
//
// 2026-07-03 (task 07-03-subagent-per-agent-model-ui, 阶段 4):
// surfaces the new `list_subagents_with_model` / `set_subagent_model`
// IPCs to the Settings UI. The store owns the in-memory list of
// per-agent resolved-model rows + a per-row spinner state for
// in-flight writes (the per-run pattern mirrors
// `subagentRuns.mergeStateByRunId` so multiple "set" operations
// on different agents can run concurrently without blocking each
// other).
//
// Cross-layer note: the wire shape is
// `SubagentWithModelRow` (camelCase via the backend
// `#[serde(rename_all = "camelCase")]`). The TypeScript interface
// below mirrors it verbatim; a drift is a cross-layer bug (the
// same `modelDisplay` null-safe contract the backend's
// `resolve_worker_provider` returns).
import { defineStore } from "pinia";
import { reactive, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { extractErrorMessage } from "../utils/useErrorBus";

/** One row in the `list_subagents_with_model` response. Mirrors
 *  the backend `SubagentWithModelRow` (camelCase). */
export interface SubagentWithModelRow {
  name: string;
  description: string;
  /** `"builtin"` | `"user"` | `"project"`. */
  source: "builtin" | "user" | "project";
  tools: string[];
  /** Final model id after `DB override > frontmatter > None`.
   *  `null` = inherit parent. */
  resolvedModelId: string | null;
  /** `models.display_name` for the resolved id. `null` when
   *  `resolvedModelId` is null OR when the model was deleted
   *  (catalog miss → UI shows the raw id + a red badge). */
  resolvedModelDisplay: string | null;
  /** Raw `model:` value from the frontmatter (before DB
   *  overlay). For the debug / "what does the file say" hint. */
  declaredModelId: string | null;
  /** `true` iff the DB override table has a row for this
   *  name. Drives the "(DB override)" chip in the UI. */
  hasDbOverride: boolean;
  /** `true` for `source=user|project` (frontmatter is writable).
   *  `false` for `source=builtin` (writes route to the DB
   *  table). */
  writable: boolean;
}

/** Per-row spinner state for the Subagents tab. Mirrors the
 *  `subagentRuns.mergeStateByRunId` pattern — keyed by agent
 *  name so multiple "set" operations can run concurrently
 *  without blocking each other. */
type SpinnerState = { loading: true };

export const useSubagentsStore = defineStore("subagents", () => {
  // -----------------------------------------------------------------------
  // Reactive state
  // -----------------------------------------------------------------------

  /** Per-agent list state. Keyed by the lowercase `name` (the
   *  loader normalizes names to `[a-zA-Z0-9_-]` so this is
   *  safe). A `null` value means "we've fetched for this name
   *  and got an empty result" — distinct from the absent key
   *  ("haven't fetched yet"). The UI doesn't need the absent
   *  vs empty distinction (it renders the same placeholder),
   *  but a future "show me only configured agents" filter
   *  might. */
  const rows = reactive(new Map<string, SubagentWithModelRow>());

  /** `true` after the first `fetchForProject` resolves. The UI
   *  shows a loading skeleton while `false`. */
  const loaded = ref(false);

  /** Per-agent spinner state. Keyed by `name`; presence = a
   *  write is in flight for that agent. `delete` in `finally`
   *  so the spinner never sticks (the "click and forget"
   *  UX is the worst case for stuck spinners). */
  const spinnerByName = reactive(new Map<string, SpinnerState>());

  /** Snapshot of `currentCwd` (canonical project path) at
   *  fetch time. Stored so the UI knows what `projectPath`
   *  value to pass to subsequent `setSubagentModel` calls
   *  (the IPC signature is per-project, even though the DB
   *  override is global). */
  let lastProjectPath: string | null = null;

  // -----------------------------------------------------------------------
  // Actions
  // -----------------------------------------------------------------------

  /** Fetch the list of subagents (with resolved models) for a
   *  given project. Replaces the entire in-memory map. */
  async function fetchForProject(projectPath: string) {
    const list = await invoke<SubagentWithModelRow[]>(
      "list_subagents_with_model",
      { projectPath },
    );
    rows.clear();
    for (const row of list) {
      rows.set(row.name, row);
    }
    lastProjectPath = projectPath;
    loaded.value = true;
  }

  /** Set or clear an agent's model. `modelId = null` clears
   *  (the "inherit parent" affordance). Returns the
   *  post-update row; the caller refreshes local state from
   *  it. */
  async function setModel(
    name: string,
    source: "builtin" | "user" | "project",
    modelId: string | null,
  ): Promise<SubagentWithModelRow> {
    if (lastProjectPath === null) {
      throw new Error(
        "subagents store: setModel called before fetchForProject",
      );
    }
    // Spinner guard: a second click while a write is in
    // flight is a no-op (the UI also disables the button —
    // this is defensive).
    if (spinnerByName.has(name)) {
      throw new Error("another action is already in flight for this agent");
    }
    spinnerByName.set(name, { loading: true });
    try {
      const updated = await invoke<SubagentWithModelRow>(
        "set_subagent_model",
        {
          name,
          source,
          projectPath: lastProjectPath,
          modelId,
        },
      );
      rows.set(updated.name, updated);
      return updated;
    } catch (e) {
      throw new Error(extractErrorMessage(e));
    } finally {
      spinnerByName.delete(name);
    }
  }

  return {
    rows,
    loaded,
    spinnerByName,
    fetchForProject,
    setModel,
  };
});
