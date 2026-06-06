// Projects store — owns the list of registered directories and the
// "current project" state. Sessions in `chat.ts` are scoped to the
// current project; switching tabs in the UI calls `switchProject`,
// which fires the watcher in `chat.ts` and triggers a sessions
// reload.
//
// `pick_project_dir` semantics (Q8v2 / PROPOSAL §5.4):
//   - `Ok(Some(path))` → user picked; create the project (or focus an
//     existing one with the same path) and switch to it.
//   - `Ok(None)` → user cancelled; silent.
//   - `Err(_)` → dialog failed (e.g. backend dir gone); toast the
//     error, do NOT re-open the dialog.

import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Project as returned over Tauri IPC. Mirrors `projects::ProjectRow`
 *  in Rust. Field names are snake_case to match the Rust serialization
 *  (PR1 did not add `#[serde(rename_all = "camelCase")]`). */
export interface ProjectInfo {
  id: string;
  name: string;
  path: string;
  is_git_repo: boolean;
  /** Current branch name, or `null` for non-git projects. The literal
   *  string `"HEAD"` is stored for detached-HEAD repos so the UI can
   *  distinguish detached state from a real branch. PR2 added this
   *  field; legacy projects created before PR2 may return `null` and
   *  will be re-probed on the next `update_project_path` call. */
  git_branch: string | null;
  is_legacy: boolean;
  created_at: string;
  updated_at: string;
  hidden: boolean;
  metadata: string | null;
}

export type ToastKind = "info" | "warn" | "error";

export interface ToastMessage {
  message: string;
  kind: ToastKind;
}

let toastTimer: number | null = null;

// Module-level handle for the `projects:refreshed` listener. Set
// once on the first `loadProjects()` call and never re-registered
// (the Pinia store is a singleton for the app's lifetime, so we
// don't need to unregister on store disposal). Mirrors the
// `unlistenChat` pattern in `chat.ts`.
let unlistenRefresh: UnlistenFn | null = null;

export const useProjectsStore = defineStore("projects", () => {
  // -----------------------------------------------------------------------
  // State
  // -----------------------------------------------------------------------

  const projects = ref<ProjectInfo[]>([]);
  const hiddenProjects = ref<ProjectInfo[]>([]);
  const currentProjectId = ref<string | null>(null);
  const toast = ref<ToastMessage | null>(null);

  // -----------------------------------------------------------------------
  // Toast (lightweight, no UI library)
  // -----------------------------------------------------------------------

  function showToast(
    message: string,
    kind: ToastKind = "info",
    durationMs = 3500,
  ): void {
    toast.value = { message, kind };
    if (toastTimer !== null) {
      window.clearTimeout(toastTimer);
    }
    toastTimer = window.setTimeout(() => {
      toast.value = null;
      toastTimer = null;
    }, durationMs);
  }

  function dismissToast(): void {
    toast.value = null;
    if (toastTimer !== null) {
      window.clearTimeout(toastTimer);
      toastTimer = null;
    }
  }

  // -----------------------------------------------------------------------
  // CRUD
  // -----------------------------------------------------------------------

  async function loadProjects(): Promise<void> {
    // Idempotent: register the `projects:refreshed` listener on the
    // first load so the startup backfill (PR2 follow-up) can poke
    // us once it has written the re-probed git metadata into the
    // DB. The Rust side only emits this event when at least one
    // project was updated, so users with no stale projects see no
    // extra IPC traffic. See
    // `.trellis/tasks/06-06-pr2-backfill-fix/prd.md`.
    await ensureRefreshListener();
    projects.value = await invoke<ProjectInfo[]>("list_projects", {
      filter: { hidden: false },
    });
  }

  /** Register the `projects:refreshed` listener exactly once. The
   *  backend (lib.rs::AppState::load) spawns a backfill task on
   *  startup that re-probes git metadata for pre-PR2 project
   *  rows; when it finishes it emits this event with the number
   *  of updated rows as the payload. We respond by reloading the
   *  visible project list so the chat panel's git chip picks up
   *  the real branch name without the user having to switch
   *  tabs. */
  async function ensureRefreshListener(): Promise<void> {
    if (unlistenRefresh !== null) return;
    try {
      unlistenRefresh = await listen<number>("projects:refreshed", () => {
        // The payload is the number of updated rows, which the UI
        // does not need to display; the only useful side effect is
        // a fresh load so the chip renders the new branch.
        void loadProjects();
      });
    } catch (e) {
      // `listen` failing at startup would be a Tauri runtime
      // problem, not a data problem — log so it's visible in
      // devtools but don't crash the store.
      // eslint-disable-next-line no-console
      console.error("ensureRefreshListener failed:", e);
    }
  }

  async function loadHiddenProjects(): Promise<void> {
    hiddenProjects.value = await invoke<ProjectInfo[]>(
      "list_hidden_projects",
    );
  }

  /** Open the native folder picker and (on success) register the chosen
   *  directory as a new project. Returns the created (or already
   *  existing) project, or `null` if the user cancelled or the picker
   *  failed. */
  async function addProject(): Promise<ProjectInfo | null> {
    let picked: string | null = null;
    let pickError: string | null = null;
    try {
      picked = await invoke<string | null>("pick_project_dir", {
        fallback: false,
      });
    } catch (e) {
      pickError = String(e);
    }

    if (pickError) {
      // Dialog could not be opened (or backend dir gone). Show a
      // toast, do NOT re-open the dialog (Q8v2: no manual input
      // fallback — Tauri's `pick_folder` IS the tree-walk).
      showToast(`添加项目失败: ${pickError}`, "error");
      return null;
    }
    if (picked === null) {
      // User cancelled. Silent.
      return null;
    }

    // Picked a path — check if a project with this path already
    // exists. If so, focus it instead of re-adding.
    const existing = projects.value.find((p) => p.path === picked);
    if (existing) {
      currentProjectId.value = existing.id;
      showToast(`项目已存在: ${existing.name}`, "info");
      return existing;
    }

    try {
      const created = await invoke<ProjectInfo>("create_project", {
        path: picked,
      });
      await loadProjects();
      currentProjectId.value = created.id;
      return created;
    } catch (e) {
      showToast(`添加项目失败: ${String(e)}`, "error");
      return null;
    }
  }

  /** Switch to a different project. Sessions are reloaded by the
   *  watcher in `chat.ts` (single source of truth for cross-store
   *  coordination). */
  async function switchProject(id: string): Promise<void> {
    if (currentProjectId.value === id) return;
    currentProjectId.value = id;
  }

  async function hideProject(id: string): Promise<void> {
    try {
      await invoke("hide_project", { id });
    } catch (e) {
      showToast(`关闭项目失败: ${String(e)}`, "error");
      return;
    }
    // The current project may have just been hidden — fall back to
    // the first remaining visible project, or null if none.
    if (currentProjectId.value === id) {
      await loadProjects();
      currentProjectId.value = projects.value[0]?.id ?? null;
    } else {
      await loadProjects();
    }
  }

  async function unhideProject(id: string): Promise<void> {
    try {
      await invoke("unhide_project", { id });
    } catch (e) {
      showToast(`重新打开项目失败: ${String(e)}`, "error");
      return;
    }
    await loadHiddenProjects();
    await loadProjects();
    // Auto-focus the freshly unhidden project.
    const fresh = projects.value.find((p) => p.id === id);
    if (fresh) currentProjectId.value = fresh.id;
  }

  async function renameProject(id: string, name: string): Promise<void> {
    const trimmed = name.trim();
    if (!trimmed) {
      showToast("项目名不能为空", "warn");
      return;
    }
    try {
      await invoke<ProjectInfo>("update_project_name", {
        id,
        newName: trimmed,
      });
      await loadProjects();
    } catch (e) {
      showToast(`重命名失败: ${String(e)}`, "error");
    }
  }

  function projectById(id: string | null): ProjectInfo | undefined {
    if (!id) return undefined;
    return projects.value.find((p) => p.id === id);
  }

  /** Basename of a path — used for tooltips and default display name. */
  function basenameOf(path: string): string {
    const norm = path.replace(/[\\/]+$/, "");
    const idx = Math.max(norm.lastIndexOf("/"), norm.lastIndexOf("\\"));
    return idx >= 0 ? norm.slice(idx + 1) : norm;
  }

  return {
    projects,
    hiddenProjects,
    currentProjectId,
    toast,
    showToast,
    dismissToast,
    loadProjects,
    loadHiddenProjects,
    addProject,
    switchProject,
    hideProject,
    unhideProject,
    renameProject,
    projectById,
    basenameOf,
  };
});
