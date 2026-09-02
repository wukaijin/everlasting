// Projects store — owns the list of registered directories and the
// "current project" state. Sessions in `chat.ts` are scoped to the
// current project; switching tabs in the UI calls `switchProject`,
// which fires the watcher in `chat.ts` and triggers a sessions
// reload.
//
// "添加项目" flow — unified across ALL modes (desktop / browser /
// sidecar / remote) since 2026-09-03, task
// `09-03-dirbrowser-desktop-unify`: `openDirBrowser()` opens the
// frontend-rendered DirBrowserModal (the former Tauri-only native
// folder picker was fully removed). The modal browses via the
// `browse_dir` IPC; the chosen path is registered by
// `addProjectByPath` (dedup / unhide / create + focus,
// RULE-FrontProj-001).

import { defineStore } from "pinia";
import { ref } from "vue";
import { transport, type UnlistenFn } from "../transport";
import { extractErrorMessage } from "../utils/useErrorBus";

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
  /** P3c per-project sandbox policy tier. `readwrite` is the default
   *  (every command sandboxed, worktree writable); `readonly` = hard
   *  isolation (worktree read-only face); `off` = no sandbox (classic
   *  approval path). Legacy daemons may omit the field — treated as
   *  `readwrite` by the backend DEFAULT, so the UI defaults its
   *  selection there when absent. */
  sandbox_policy?: "off" | "readwrite" | "readonly";
}

export type ToastKind = "info" | "warn" | "error";

export interface ToastMessage {
  message: string;
  kind: ToastKind;
  /** Optional session id for clickable toasts (cross-session
   *  pending-interaction notifications, 2026-07-08
   *  `cross-session-pending-indicator`). When set AND the session
   *  belongs to the current project, clicking the toast switches
   *  to that session. Absent for project-operation toasts → they
   *  just dismiss on click (existing behavior preserved). */
  sessionId?: string;
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

  // DirBrowserModal open flag. `openDirBrowser()` flips this to
  // `true`; the AppShell-mounted modal browses via `browse_dir` and
  // registers the chosen path through `addProjectByPath`. Landed
  // 2026-09-02 as the browser-mode degrade, promoted 2026-09-03 to
  // the unified "add project" entry for every mode.
  const dirBrowserOpen = ref(false);

  // -----------------------------------------------------------------------
  // Toast (lightweight, no UI library)
  // -----------------------------------------------------------------------

  function showToast(
    message: string,
    kind: ToastKind = "info",
    durationMs = 3500,
    opts?: { sessionId?: string },
  ): void {
    toast.value = { message, kind, sessionId: opts?.sessionId };
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
    projects.value = await transport.invoke<ProjectInfo[]>("list_projects", {
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
      unlistenRefresh = await transport.listen<number>("projects:refreshed", () => {
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
    hiddenProjects.value = await transport.invoke<ProjectInfo[]>(
      "list_hidden_projects",
    );
  }

  /** Open the DirBrowserModal — the unified "add project" entry for
   *  every mode (desktop / browser / sidecar / remote). The modal
   *  (mounted in AppShell, driven by `dirBrowserOpen`) offers
   *  click-to-browse directory selection with an inline path input +
   *  前往; the chosen path is registered by `addProjectByPath`. */
  function openDirBrowser(): void {
    dirBrowserOpen.value = true;
  }

  /** Register a path chosen in the directory-browser modal (or any
   *  manual entry). Closes the browser modal on completion (success
   *  or error). Returns the project or `null`. */
  async function addProjectByPath(path: string): Promise<ProjectInfo | null> {
    const trimmed = path.trim();
    if (!trimmed) {
      showToast("项目路径不能为空", "warn");
      return null;
    }
    const result = await registerPickedPath(trimmed);
    dirBrowserOpen.value = false;
    return result;
  }

  /** Dismiss the directory-browser modal without registering. */
  function closeDirBrowser(): void {
    dirBrowserOpen.value = false;
  }

  /** Shared register-picked-path tail: dedup against visible + hidden,
   *  create if new, focus. Entry point is `addProjectByPath` (the
   *  DirBrowserModal's「选择此目录」exit). */
  async function registerPickedPath(picked: string): Promise<ProjectInfo | null> {
    // Picked a path — check the visible projects first. If the
    // project is already open, just focus it. The lazy `loadHidden`
    // call below is needed because the user may be reopening a
    // project they previously closed; without it, a path that only
    // exists in `hiddenProjects.value = []` would fall through to
    // `create_project` and hit UNIQUE.
    if (hiddenProjects.value.length === 0) {
      await loadHiddenProjects();
    }

    const visible = projects.value.find((p) => p.path === picked);
    if (visible) {
      currentProjectId.value = visible.id;
      showToast(`项目已存在: ${visible.name}`, "info");
      return visible;
    }

    // Hidden-project reopen path: un-hide and focus the existing row
    // instead of trying to re-create it (which would fail with
    // UNIQUE on `projects.path`). `unhideProject` already does the
    // full IPC + reload + focus sequence.
    const hidden = hiddenProjects.value.find((p) => p.path === picked);
    if (hidden) {
      const ok = await unhideProject(hidden.id);
      if (!ok) return null;
      // unhideProject already focused the project; surface a
      // success toast (it only toasts on failure).
      showToast(`已重新打开: ${hidden.name}`, "info");
      // Re-resolve the now-visible row so the returned `path` field
      // matches the freshly unhidden project (hiddenProjects has
      // already been reloaded inside unhideProject).
      return (
        projects.value.find((p) => p.id === hidden.id) ?? hidden
      );
    }

    try {
      const created = await transport.invoke<ProjectInfo>("create_project", {
        path: picked,
      });
      await loadProjects();
      currentProjectId.value = created.id;
      return created;
    } catch (e) {
      showToast(`添加项目失败: ${extractErrorMessage(e)}`, "error");
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
      await transport.invoke("hide_project", { id });
    } catch (e) {
      showToast(`关闭项目失败: ${extractErrorMessage(e)}`, "error");
      return;
    }
    await loadProjects();
    // BUGLIST CH3-1 (2026-08-29 GUI full-test): refresh the hidden
    // list too, mirroring `unhideProject`. Without this, the just-
    // hidden project stayed invisible in「已隐藏项目」until the next
    // page reload, and re-adding its path fell through to
    // `create_project` (stale-list miss) → misleading UNIQUE
    // "already exists" toast — no UI way back before a reload.
    await loadHiddenProjects();
    // The current project may have just been hidden — fall back to
    // the first remaining visible project, or null if none.
    if (currentProjectId.value === id) {
      currentProjectId.value = projects.value[0]?.id ?? null;
    }
  }

  /** Un-hide a previously closed project. Returns `true` on success,
   *  `false` if the IPC threw (a "重新打开项目失败" toast is surfaced
   *  in that case). The freshly unhidden project is auto-focused
   *  via `currentProjectId` so the caller does not need to do it.
   *
   *  Returns a boolean (rather than just `void`) so that
   *  `registerPickedPath`'s hidden-path branch can avoid showing a
   *  misleading "已重新打开" toast on failure — see the
   *  RULE-FrontProj-001 fix. */
  async function unhideProject(id: string): Promise<boolean> {
    try {
      await transport.invoke("unhide_project", { id });
    } catch (e) {
      showToast(`重新打开项目失败: ${extractErrorMessage(e)}`, "error");
      return false;
    }
    await loadHiddenProjects();
    await loadProjects();
    // Auto-focus the freshly unhidden project.
    const fresh = projects.value.find((p) => p.id === id);
    if (fresh) currentProjectId.value = fresh.id;
    return true;
  }

  async function renameProject(id: string, name: string): Promise<void> {
    const trimmed = name.trim();
    if (!trimmed) {
      showToast("项目名不能为空", "warn");
      return;
    }
    try {
      await transport.invoke<ProjectInfo>("update_project_name", {
        id,
        newName: trimmed,
      });
      await loadProjects();
    } catch (e) {
      showToast(`重命名失败: ${extractErrorMessage(e)}`, "error");
    }
  }

  /** P3c: persist the project's sandbox policy tier. Refreshes the
   *  project list so the settings tab reflects the stored value
   *  (single read model). Throws on failure — the caller (settings
   *  tab) owns the toast, so a store-level toast would double up. */
  async function setProjectSandboxPolicy(
    id: string,
    policy: "off" | "readwrite" | "readonly",
  ): Promise<void> {
    await transport.invoke<ProjectInfo>("update_project_sandbox_policy", {
      id,
      policy,
    });
    await loadProjects();
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
    dirBrowserOpen,
    showToast,
    dismissToast,
    loadProjects,
    loadHiddenProjects,
    addProjectByPath,
    openDirBrowser,
    closeDirBrowser,
    switchProject,
    hideProject,
    unhideProject,
    renameProject,
    setProjectSandboxPolicy,
    projectById,
    basenameOf,
  };
});
