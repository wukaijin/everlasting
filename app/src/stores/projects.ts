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
import { transport, type UnlistenFn } from "../transport";
import { TransportError } from "../transport/http";
import { extractErrorMessage } from "../utils/useErrorBus";

/** P2.4 D6: detect that `pick_project_dir` is unavailable on the
 *  current transport (httpTransport throws `TransportError` with
 *  status 0 + an "unknown cmd" body because `pick_project_dir` has
 *  no daemon route). Used to flip the manual-path entry on instead
 *  of dead-ending on an error toast. */
function isPickUnavailable(e: unknown): boolean {
  if (e instanceof TransportError) {
    // status 0 = the httpTransport's synthetic "no domain mapping"
    // error (never a real HTTP status). Body carries the unknown-cmd
    // message.
    return e.status === 0;
  }
  return false;
}

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

  // P2.4 D6: browser-mode manual-path entry. When the native folder
  // picker is unavailable (httpTransport — `pick_project_dir` has no
  // daemon route), `addProject()` flips this to `true` and the UI
  // renders a path text-input. The user submits a path →
  // `addProjectByPath(path)` (the same register-picked-path tail
  // `addProject` uses for the native-picker success path).
  const manualPathOpen = ref(false);

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

  /** Open the native folder picker and (on success) register the chosen
   *  directory as a new project. Returns the created (or already
   *  existing) project, or `null` if the user cancelled or the picker
   *  failed.
   *
   *  If the picked path matches an already-hidden project (data is
   *  preserved, just hidden from the tab bar), we auto-unhide it
   *  instead of erroring. The previous behaviour would hit the
   *  backend `create_project` SQLite UNIQUE constraint and surface a
   *  misleading "already exists" toast — see fix for the "关闭项目后
   *  无法重新打开" bug (RULE-FrontProj-001).
   *
   *  **P2.4 D6 (browser degrade)**: under `httpTransport`,
   *  `pick_project_dir` has no daemon route (the Tauri dialog API
   *  can't run outside the GUI process), so `invoke` throws an
   *  "unknown cmd" `TransportError`. We detect that and flip
   *  `manualPathOpen = true` so the UI offers a manual path text
   *  input instead of erroring out (Q8v2 manual-input fallback is
   *  now permitted in browser mode — it was previously rejected
   *  because Tauri's `pick_folder` WAS the tree-walk, but browsers
   *  have no equivalent). */
  async function addProject(): Promise<ProjectInfo | null> {
    let picked: string | null = null;
    let pickError: string | null = null;
    try {
      picked = await transport.invoke<string | null>("pick_project_dir", {
        fallback: false,
      });
    } catch (e) {
      pickError = extractErrorMessage(e);
      // P2.4 D6: browser-mode degrade. The httpTransport throws a
      // TransportError(status=0, "unknown cmd ...") because
      // `pick_project_dir` is not in CMD_TO_DOMAIN. Surface the
      // manual-path entry instead of a dead-end error toast.
      if (isPickUnavailable(e)) {
        manualPathOpen.value = true;
        return null;
      }
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

    return registerPickedPath(picked);
  }

  /** P2.4 D6: register a manually-entered path (browser-mode entry).
   *  Same tail as the native-picker success path. Closes the manual
   *  path input on completion (success or error). Returns the project
   *  or `null`. */
  async function addProjectByPath(path: string): Promise<ProjectInfo | null> {
    const trimmed = path.trim();
    if (!trimmed) {
      showToast("项目路径不能为空", "warn");
      return null;
    }
    const result = await registerPickedPath(trimmed);
    manualPathOpen.value = false;
    return result;
  }

  /** P2.4 D6: dismiss the manual-path input without registering. */
  function cancelManualPath(): void {
    manualPathOpen.value = false;
  }

  /** Shared register-picked-path tail: dedup against visible + hidden,
   *  create if new, focus. Used by both the native picker
   *  (`addProject`) and the manual browser-mode entry
   *  (`addProjectByPath`). */
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
   *  `addProject`'s hidden-path branch can avoid showing a
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
   *  (single read model). */
  async function setProjectSandboxPolicy(
    id: string,
    policy: "off" | "readwrite" | "readonly",
  ): Promise<void> {
    try {
      await transport.invoke<ProjectInfo>("update_project_sandbox_policy", {
        id,
        policy,
      });
      await loadProjects();
    } catch (e) {
      showToast(`沙盒策略保存失败: ${extractErrorMessage(e)}`, "error");
      throw e;
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
    manualPathOpen,
    showToast,
    dismissToast,
    loadProjects,
    loadHiddenProjects,
    addProject,
    addProjectByPath,
    cancelManualPath,
    switchProject,
    hideProject,
    unhideProject,
    renameProject,
    setProjectSandboxPolicy,
    projectById,
    basenameOf,
  };
});
