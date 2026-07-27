// useReviewStateStore — C2 (review visualization view, 2026-07-26).
//
// Holds the parsed `review-state.json` for the CURRENT task, used
// by the `<ReviewMatrix>` panel in `ChatPanel.vue`. Mirrors the
// `useChecklistStore` "called by streamController" pattern (B12):
// the store does NOT self-listen on any event. Refresh is driven
// by `streamController.handleToolCall`, which routes
// `write_file` tool:call events whose `input.path` hits the
// current task's `review-state.json` to
// `handleReviewStateWritten(sessionId, slug)` below.
//
// Why not self-listen (mirrors checklist.ts):
//   - The backend emits NO review-specific event (C3 cut
//     `emit_review_state_updated` — see design.md §2). The
//     streamController already has a global `tool:call` listener;
//     reusing it is the zero-backend-event path.
//   - Centralizing the route in streamController keeps the
//     pattern uniform with B12 checklist (one routing site, not
//     N listener registrations).
//
// Lifecycle:
//   - `ChatPanel` onMounted (review session) → `start(slug)`:
//     sets `currentSlug`, fires the first `refresh`.
//   - `ChatPanel` watch(currentSessionId) / onUnmounted →
//     `stop()`: clears `currentSlug` + cancels any pending
//     debounce timer so a stale refresh can't land after the
//     user switched away.
//   - `handleReviewStateWritten(sessionId, slug)`: called by
//     streamController. Slug-gated (ignores writes for other
//     tasks) + debounced 200ms (one revising turn may write the
//     file in multiple chunks; we coalesce).

import { defineStore } from "pinia";
import { computed, ref } from "vue";

import { transport } from "../transport";
import type {
  CurrentTaskInfo,
  ReviewState,
  ReviewStatePayload,
} from "../types/review-state";

/** Error shape surfaced to the UI. `kind: "missing"` is NOT
 *  surfaced as an error in the panel — the panel hides itself on
 *  missing; only `invalid` / `network` show the error state. */
export interface ReviewStateError {
  kind: "missing" | "invalid" | "network";
  detail?: string;
}

/** Debounce window for `handleReviewStateWritten`. 200ms is
 *  enough to coalesce a multi-chunk atomic write (tmp + rename
 *  in C3 wf-synthesize) without visible lag. */
const REFRESH_DEBOUNCE_MS = 200;

/**
 * Match a `write_file` tool:call `input.path` against the
 * current task's `review-state.json` location.
 *
 * Conservative on purpose: a false positive only triggers one
 * extra `get_review_state` (which returns `Missing` if the file
 * isn't actually there — harmless). A false negative leaves the
 * view stale until the user folds/unfolds the panel or switches
 * session (which re-`start`s).
 *
 * Three hit conditions (any):
 *   1. Normalized path `endsWith("/tasks/<slug>/review-state.json")`
 *      — the absolute-path case.
 *   2. `basename === "review-state.json"` AND the path contains
 *      `/tasks/<slug>/` — covers sibling-file relative writes
 *      like `../<slug>/review-state.json`.
 *   3. `basename === "review-state.json"` AND the path has no
 *      separator — the bare-relative `review-state.json` fallback
 *      (the agent often writes from inside the task dir).
 *
 * Exported for the vitest (`reviewState.test.ts`).
 */
export function matchesReviewStatePath(path: string, slug: string): boolean {
  if (!path || !slug) return false;
  // Normalize: strip leading "./", unify backslashes (Windows
  // agent paths), collapse duplicate slashes.
  const normalized = path
    .replace(/^\.\//, "")
    .replace(/\\/g, "/")
    .replace(/\/+/g, "/");
  const basename = normalized.slice(normalized.lastIndexOf("/") + 1);
  if (basename !== "review-state.json") return false;
  if (normalized.endsWith(`/tasks/${slug}/review-state.json`)) return true;
  // The `/tasks/<slug>/` substring covers the absolute case + the
  // `./tasks/...` case (after the `./` strip). We also accept
  // `tasks/<slug>/` without a leading separator so a bare
  // sibling-relative write like `tasks/demo-task/review-state.json`
  // matches (the agent's cwd is one level above `.everlasting`).
  if (normalized.includes(`/tasks/${slug}/`)) return true;
  if (normalized.startsWith(`tasks/${slug}/`)) return true;
  // Bare relative fallback: path is just "review-state.json"
  // (no "/"). The agent's cwd during revising is the task dir,
  // so this is the common case.
  return !normalized.includes("/");
}

export const useReviewStateStore = defineStore("reviewState", () => {
  /** Parsed review-state.json for the current task. `null` when
   *  not yet loaded / missing / the user switched away. */
  const state = ref<ReviewState | null>(null);

  /** Current error. `null` when state is loaded OR when the file
   *  is missing (missing → panel hidden, no error UI). Only
   *  `invalid` / `network` populate this. */
  const error = ref<ReviewStateError | null>(null);

  /** True while a `refresh` is in flight. The panel uses this
   *  for a subtle loading shimmer on the header. */
  const loading = ref(false);

  /** Current task slug — the gate for `handleReviewStateWritten`.
   *  `null` when no review session is active (panel hidden).
   *
   *  Kept as a ref (NOT a plain `let`) so the routing accessor
   *  below re-evaluates reactively when `start` / `stop` flip it.
   *  Pinia wraps object-literal getters into computeds; a plain
   *  closure variable wouldn't track, so the getter would
   *  forever return the value captured at store-creation time
   *  (null). */
  const currentSlug = ref<string | null>(null);

  /** Current project path — passed to `get_review_state` /
   *  `get_current_task_slug`. Captured from `start` (ChatPanel
   *  reads `currentSession.current_cwd`). Plain closure variable
   *  is fine — no reactive read site needs to track it. */
  let currentProjectPath: string | null = null;

  /** Pending debounce timer for `handleReviewStateWritten`. */
  let debounceTimer: ReturnType<typeof setTimeout> | null = null;

  /** Re-read review-state.json for `taskSlug` and split the
   *  three-state payload into `state` / `error`. `projectPath`
   *  defaults to `currentProjectPath` (the value captured at
   *  `start`); callers may override for the rare case where the
   *  project root differs. */
  async function refresh(
    taskSlug: string,
    projectPath: string = currentProjectPath ?? "",
  ): Promise<void> {
    if (!taskSlug) return;
    loading.value = true;
    try {
      const payload = await transport.invoke<ReviewStatePayload>(
        "get_review_state",
        { projectPath, taskSlug },
      );
      applyPayload(payload);
    } catch (e) {
      // Network / IPC failure (transport rejection). Surface as
      // `network` so the panel shows the retry affordance.
      error.value = {
        kind: "network",
        detail: e instanceof Error ? e.message : String(e),
      };
      state.value = null;
    } finally {
      loading.value = false;
    }
  }

  /** Apply a `ReviewStatePayload` to `state` / `error`. Pure
   *  (no transport) — split out so the route + initial load
   *  share the same three-state branching. */
  function applyPayload(payload: ReviewStatePayload): void {
    if (payload.kind === "state") {
      state.value = payload.state;
      error.value = null;
    } else if (payload.kind === "missing") {
      state.value = null;
      error.value = null; // missing → hide panel, NOT an error
    } else {
      // invalid → keep panel visible (if it had prior state)
      // but surface the error so the user can fall back to
      // SubagentDrawer. We clear `state` so stale data doesn't
      // mask the parse failure.
      state.value = null;
      error.value = { kind: "invalid", detail: payload.detail };
    }
  }

  /** Hook called by `streamController.handleToolCall` when a
   *  `write_file` tool:call hits the current task's
   *  `review-state.json` (matched via `matchesReviewStatePath`).
   *
   *  Slug-gated: if the write is for a DIFFERENT task than
   *  `currentSlug`, ignore it (streamController is a global
   *  listener — it sees every session's tool:call).
   *
   *  Debounced: one revising turn may write the file in
   *  multiple chunks (atomic tmp + rename, multi-step JSON
   *  build); we coalesce to one `refresh` 200ms after the last
   *  write. The `slug` arg from the route is the source of
   *  truth for which task was written — but we still gate on
   *  `currentSlug` so a write to a non-active task (e.g. a
   *  background session) doesn't disturb the visible panel. */
  function handleReviewStateWritten(
    _sessionId: string,
    slug: string,
  ): void {
    if (!currentSlug.value) return; // no active review session
    // The route pre-filters by path shape, but defensively
    // double-check the slug matches the active task. This also
    // covers the case where the route fell back to the
    // bare-basename match without a slug in the path.
    if (slug !== currentSlug.value) return;
    if (debounceTimer) clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => {
      debounceTimer = null;
      const active = currentSlug.value;
      if (!active) return; // stop() ran mid-debounce
      void refresh(active, currentProjectPath ?? "");
    }, REFRESH_DEBOUNCE_MS);
  }

  /** Called by `ChatPanel` onMounted (review session). Captures
   *  `slug` + `projectPath` and fires the first `refresh`. */
  async function start(
    slug: string,
    projectPath: string,
  ): Promise<void> {
    currentSlug.value = slug;
    currentProjectPath = projectPath;
    await refresh(slug, projectPath);
  }

  /** Called by `ChatPanel` on session switch / unmount. Clears
   *  state + cancels any pending debounce so a stale refresh
   *  can't land on the wrong session. */
  function stop(): void {
    currentSlug.value = null;
    currentProjectPath = null;
    if (debounceTimer) {
      clearTimeout(debounceTimer);
      debounceTimer = null;
    }
    state.value = null;
    error.value = null;
    loading.value = false;
  }

  return {
    // reactive state
    state,
    error,
    loading,
    /** Read-only slug for streamController's routing layer.
     *  Returned as a plain value (Pinia unwraps the ref on
     *  access) — reads always see the live `currentSlug.value`.
     *  Mutating it from outside is impossible: the only setters
     *  are the `start` / `stop` lifecycle hooks. */
    currentSlugForRouting: computed(() => currentSlug.value),
    // lifecycle
    start,
    stop,
    refresh,
    handleReviewStateWritten,
  };
});

/** Re-exported for ChatPanel so it doesn't need a separate
 *  import path to the types module. */
export type { CurrentTaskInfo };
