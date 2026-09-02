// useBackgroundShellsStore — Pinia store for the chat ActivityPanel's
// 「后台命令」 section (2026-09-02, task `09-02-chat-task-panel`).
//
// Backend contract (Rust side, `background_shell/mod.rs`):
//
//   1. One IPC command (pull path):
//      `list_background_shells(sessionId) → BackgroundShellSummary[]`
//      — every non-pruned shell entry for the session, already
//      sorted running-first + newest-start first.
//   2. One push channel: `background_shell:update` events with
//      payload `{ kind: "started"|"exited"|"pruned", sessionId,
//      shellSessionId, shell }`. `started` / `exited` carry the full
//      summary; `pruned` carries ids only (the row is dropped).
//   3. `kill_background_shell(sessionId, shellSessionId)` —
//      idempotent process-group kill; the row flips to its terminal
//      state via the `exited` event, NOT via this command's return
//      (the store never fabricates terminal states locally).
//
// ⚠️ Time-source contract (PRD R5 / research §6):
//   `startedAtMs` / `elapsedMs` are PROCESS-MONOTONIC milliseconds —
//   never wall-clock. They are valid only for elapsed/duration
//   display and same-source subtraction. NEVER mix them with
//   `Date.now()` directly (e.g. `Date.now() - startedAtMs` is
//   garbage). The one sanctioned wall-clock use is
//   `receivedAtWallByShell`: the wall-clock moment a RUNNING summary
//   entered this store, used purely as an offset (`elapsedMs +
//   now - receivedAt`) so a running row's elapsed grows between
//   events without refetching. `startedAtMs` itself is never fed
//   into that arithmetic.

import { defineStore } from "pinia";
import { reactive } from "vue";
import { transport, type UnlistenFn } from "../transport";

/** Terminal / live status of one background shell. Mirrors the Rust
 *  `BackgroundShellSummary.status` plain string (`"running"` plus the
 *  snake_case `BackgroundShellOutcome` serde names). */
export type BackgroundShellStatus =
  | "running"
  | "completed"
  | "failed"
  | "killed"
  | "timed_out"
  | "spawn_failed";

/** UI-facing summary of one background shell. Mirrors the Rust
 *  `BackgroundShellSummary` (`#[serde(rename_all = "camelCase")]`) 1:1
 *  — see the Rust struct for field-level contracts. */
export interface BackgroundShellSummary {
  shellSessionId: string;
  sessionId: string;
  command: string;
  status: BackgroundShellStatus;
  /** Process-monotonic ms. Elapsed/duration ONLY — see file-top
   *  time-source contract. */
  startedAtMs: number;
  /** Running: snapshot at read time; terminal: final duration. */
  elapsedMs: number;
  exitCode: number | null;
  stdoutPreview: string | null;
  stderrPreview: string | null;
  fullOutputPath: string | null;
  originToolUseId: string | null;
}

/** `background_shell:update` event payload. Mirrors the Rust
 *  `ShellEventPayload` (camelCase). `shell` is `null` only for
 *  `pruned` (the frontend drops the row by id). */
export interface ShellEventPayload {
  kind: "started" | "exited" | "pruned";
  sessionId: string;
  shellSessionId: string;
  shell: BackgroundShellSummary | null;
}

/** Sort comparator for shell lists (mirrors the Rust
 *  `summary_sort` — keep the two in lockstep): running first, then
 *  `startedAtMs` descending (newest first). Exported as a pure
 *  function so vitest exercises the exact ordering the panel
 *  renders. */
export function compareShells(
  a: BackgroundShellSummary,
  b: BackgroundShellSummary,
): number {
  const aRunning = a.status === "running" ? 0 : 1;
  const bRunning = b.status === "running" ? 0 : 1;
  if (aRunning !== bRunning) return aRunning - bRunning;
  return b.startedAtMs - a.startedAtMs;
}

/** Live elapsed for a running shell: the store's monotonic snapshot
 *  plus the wall-clock time that has passed SINCE the snapshot
 *  entered the store. This is the sanctioned offset pattern (see
 *  file-top contract) — `startedAtMs` is never mixed with
 *  `Date.now()`. Terminal shells return their final duration
 *  unchanged. */
export function liveElapsedMs(
  shell: BackgroundShellSummary,
  receivedAtWallByShell: Map<string, number>,
  now: number,
): number {
  if (shell.status !== "running") return shell.elapsedMs;
  const receivedAt = receivedAtWallByShell.get(shell.shellSessionId);
  if (receivedAt === undefined) return shell.elapsedMs;
  return shell.elapsedMs + Math.max(0, now - receivedAt);
}

export const useBackgroundShellsStore = defineStore("backgroundShells", () => {
  /** Per-session shell summaries, keyed by `sessionId`. Sorted
   *  running-first + newest-start first after every write (fetch
   *  replace, event upsert) so the panel renders verbatim. */
  const shellsBySession = reactive(new Map<string, BackgroundShellSummary[]>());

  /** Wall-clock receive timestamps for RUNNING shells (offset base
   *  for `liveElapsedMs`). Non-contractual internal state — refreshed
   *  whenever a running summary enters the store (event upsert / fetch
   *  replace, see `stampReceivedAt`) and cleared whenever the shell
   *  leaves the running set (exited / pruned / fetch replace /
   *  clearSession). */
  const receivedAtWallByShell = new Map<string, number>();

  /** Listener unlisten handle. Set by `ensureStarted`, torn down by
   *  `stop()` (tests / hot-reload). */
  let unlisten: UnlistenFn | null = null;

  /** Idempotence guard for `ensureStarted` — the listener is global
   *  (one per app lifetime, like subagentRuns' ChatView-level start)
   *  and must NOT multiply when several panels mount. */
  let started = false;

  /** Refresh the receive timestamp for every RUNNING shell in a list
   *  entering the store; drop timestamps for shells no longer running.
   *  Called on fetch replace + upsert.
   *
   *  The stamp is REFRESHED (not kept) on every write: the payload's
   *  `elapsedMs` is backend-fresh as of emit/serialization time, so an
   *  older stamp would double-count the time before the new snapshot
   *  (e.g. switching back to a session with a live shell — the fetch
   *  answer re-measures elapsed, and keeping the original `started`
   *  stamp would add the whole away-period a second time). */
  function stampReceivedAt(list: BackgroundShellSummary[]): void {
    const now = Date.now();
    for (const s of list) {
      if (s.status === "running") {
        receivedAtWallByShell.set(s.shellSessionId, now);
      } else {
        receivedAtWallByShell.delete(s.shellSessionId);
      }
    }
  }

  /** Authoritative pull: replace the session's list with the IPC
   *  answer (re-sorted). Last-write-wins vs. concurrent events is
   *  accepted (design §3.1 / §6): a prune racing a fetch can
   *  transiently resurrect a row; the next `pruned`/`exited` event
   *  self-heals it. Failure logged + swallowed (panel just shows no
   *  shells; same degrade path as subagentRuns.fetchForSession). */
  async function fetchForSession(sessionId: string): Promise<void> {
    try {
      const rows = await transport.invoke<BackgroundShellSummary[]>(
        "list_background_shells",
        { sessionId },
      );
      const list = sortList(Array.isArray(rows) ? rows : []);
      shellsBySession.set(sessionId, list);
      stampReceivedAt(list);
    } catch (e) {
      console.error("useBackgroundShellsStore.fetchForSession failed:", e);
    }
  }

  function sortList(
    list: BackgroundShellSummary[],
  ): BackgroundShellSummary[] {
    return [...list].sort(compareShells);
  }

  /** Route one `background_shell:update` payload into the store.
   *  Exported (via the store return) as the single reducer — tests
   *  and the listener share it (cross-layer guide: one owner for the
   *  event contract). */
  function handleEvent(payload: ShellEventPayload): void {
    if (payload.kind === "pruned") {
      const list = shellsBySession.get(payload.sessionId);
      if (!list) return;
      const next = list.filter(
        (s) => s.shellSessionId !== payload.shellSessionId,
      );
      receivedAtWallByShell.delete(payload.shellSessionId);
      shellsBySession.set(payload.sessionId, next);
      return;
    }
    // started / exited must carry the summary (backend contract).
    if (!payload.shell) return;
    const list = shellsBySession.get(payload.sessionId) ?? [];
    const idx = list.findIndex(
      (s) => s.shellSessionId === payload.shellSessionId,
    );
    let next: BackgroundShellSummary[];
    if (idx >= 0) {
      next = [...list];
      next[idx] = payload.shell;
    } else {
      next = [...list, payload.shell];
    }
    next = sortList(next);
    shellsBySession.set(payload.sessionId, next);
    stampReceivedAt(next);
  }

  /** Mount the global `background_shell:update` listener. Idempotent
   *  (first call wins) — the panel calls this on mount; component
   *  unmount does NOT stop it (mirrors the subagentRuns global-
   *  listener lifecycle; `stop()` exists for tests / hot reload).
   *  A failed mount resets the guard so the next panel mount retries
   *  (a stuck flag would leave the store permanently deaf); failure
   *  is logged + swallowed (best-effort, same degrade path as
   *  `fetchForSession`). */
  async function ensureStarted(): Promise<void> {
    if (started) return;
    started = true;
    try {
      unlisten = await transport.listen<ShellEventPayload>(
        "background_shell:update",
        (payload) => {
          handleEvent(payload);
        },
      );
    } catch (e) {
      started = false;
      console.error("useBackgroundShellsStore.ensureStarted failed:", e);
    }
  }

  /** Tear down the listener (tests / hot-reload symmetry). Does NOT
   *  clear cached lists. */
  function stop(): void {
    if (unlisten) {
      unlisten();
      unlisten = null;
    }
    started = false;
  }

  /** Kill a running shell's process group. Idempotent backend-side;
   *  the row flips to its terminal state via the `exited` event —
   *  no local state fabrication. Throws on backend error (unknown
   *  shell / wrong session); the caller toasts
   *  `extractErrorMessage(e)`. */
  async function kill(sessionId: string, shellSessionId: string): Promise<void> {
    await transport.invoke("kill_background_shell", {
      sessionId,
      shellSessionId,
    });
  }

  /** Drop all state for a session (session delete). Wired next to
   *  `checklist.clearSession` in `chatSessionActions.deleteSession`. */
  function clearSession(sessionId: string): void {
    const list = shellsBySession.get(sessionId);
    if (list) {
      for (const s of list) receivedAtWallByShell.delete(s.shellSessionId);
    }
    shellsBySession.delete(sessionId);
  }

  /** Live elapsed for one shell (panel chip): monotonic snapshot +
   *  wall-clock offset for running rows (see `liveElapsedMs`).
   *  Store-level wrapper so the internal receive-timestamp map stays
   *  encapsulated; `now` is caller-supplied (the panel's 5s tick)
   *  to keep this pure per-call. */
  function elapsedOf(shell: BackgroundShellSummary, now: number): number {
    return liveElapsedMs(shell, receivedAtWallByShell, now);
  }

  return {
    shellsBySession,
    fetchForSession,
    handleEvent,
    kill,
    clearSession,
    ensureStarted,
    stop,
    elapsedOf,
  };
});
