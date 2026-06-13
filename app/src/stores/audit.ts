// useAuditStore — Pinia store for the C4 audit-log query UI.
//
// Backend (PR1, 2026-06-14) exposes a single Tauri command
// `list_session_audit_events(session_id)` that returns the full
// `session_audit_events` row set for a session, sorted `ts DESC`.
// This store is the frontend-side reactive wrapper that:
//
//   1. Loads the rows on modal-open (`loadForSession(sessionId)`).
//   2. Holds the filter state (kind dropdown + critical-only
//      checkbox).
//   3. Exposes `filteredEvents` as a derived getter that respects
//      the filters.
//   4. Exposes a `refresh()` for the modal's manual refresh
//      button (MVP scope per PRD "Modal 开着期间 agent 又写新
//      事件" edge case — no live push, just re-fetch).
//
// Failure policy: any IPC failure is caught and stored in `error`;
// `events` keeps its previous value so the modal can render the
// stale state with an error banner instead of crashing.
//
// State model:
//   - `events: AuditEventRow[]` — full row set for the active
//     session, sorted `ts DESC` then `id DESC` (the backend only
//     sorts by `ts DESC`; we re-sort by `id DESC` as a secondary
//     key so same-second rows from the same turn have a stable
//     order. See `sortEvents` below.)
//   - `loading: boolean`
//   - `error: string | null`
//   - `lastSessionId: string | null`
//   - `kindFilter: string | null` — null = "全部"
//   - `onlyCritical: boolean`

import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { AuditEventRow } from "../utils/audit";

export const useAuditStore = defineStore("audit", () => {
  // -----------------------------------------------------------------------
  // State
  // -----------------------------------------------------------------------

  const events = ref<AuditEventRow[]>([]);
  const loading = ref<boolean>(false);
  const error = ref<string | null>(null);
  const lastSessionId = ref<string | null>(null);
  const kindFilter = ref<string | null>(null);
  const onlyCritical = ref<boolean>(false);

  // -----------------------------------------------------------------------
  // Fetching
  // -----------------------------------------------------------------------

  /** Stable sort: `ts DESC` (primary) + `id DESC` (secondary). The
   *  backend SQL only sorts by `ts DESC`, and SQLite `datetime('now')`
   *  is second-precision — a single agent turn that calls multiple
   *  tools in the same second produces a tie. The secondary `id DESC`
   *  makes the order deterministic (newer writes get higher ids per
   *  SQLite `AUTOINCREMENT`). */
  function sortEvents(rows: AuditEventRow[]): AuditEventRow[] {
    // Mutate a copy so the input array stays untouched (defensive
    // — `invoke` could in principle hand us a shared reference).
    return [...rows].sort((a, b) => {
      if (a.ts !== b.ts) return a.ts < b.ts ? 1 : -1;
      return b.id - a.id;
    });
  }

  /** Load all audit events for a session. Replaces `events` on
   *  success; on failure, sets `error` and leaves `events` at the
   *  previous value (defensive). Safe to call multiple times. */
  async function loadForSession(sessionId: string): Promise<void> {
    loading.value = true;
    error.value = null;
    try {
      const rows = await invoke<AuditEventRow[]>("list_session_audit_events", {
        sessionId,
      });
      events.value = sortEvents(rows);
      lastSessionId.value = sessionId;
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    } finally {
      loading.value = false;
    }
  }

  /** Re-fetch the last-loaded session. Used by the modal's manual
   *  refresh button — MVP scope does NOT include live push (the
   *  PRD lists real-time push as OOS). */
  async function refresh(): Promise<void> {
    if (!lastSessionId.value) return;
    await loadForSession(lastSessionId.value);
  }

  // -----------------------------------------------------------------------
  // Filter actions
  // -----------------------------------------------------------------------

  function setKindFilter(kind: string | null): void {
    kindFilter.value = kind;
  }

  function toggleCritical(): void {
    onlyCritical.value = !onlyCritical.value;
  }

  // -----------------------------------------------------------------------
  // Getters
  // -----------------------------------------------------------------------

  /** `true` if a row is a critical Tier 2 hard-kill denial. Reads
   *  the parsed `payload.critical` field; falls back to `false`
   *  when the payload is missing / malformed. */
  function isCritical(row: AuditEventRow): boolean {
    if (!row.payloadJson) return false;
    try {
      const p = JSON.parse(row.payloadJson);
      return !!p && typeof p === "object" && (p as { critical?: boolean }).critical === true;
    } catch {
      return false;
    }
  }

  /** Filtered view of `events` — applies the kind dropdown + the
   *  "仅 critical" checkbox. This is what the modal's list
   *  iterates over. */
  const filteredEvents = computed<AuditEventRow[]>(() => {
    let list = events.value;
    if (kindFilter.value !== null) {
      list = list.filter((e) => e.kind === kindFilter.value);
    }
    if (onlyCritical.value) {
      list = list.filter((e) => isCritical(e));
    }
    return list;
  });

  /** Total event count (no filters). Used by the modal's "事件计数"
   *  chip ("X 项"). */
  const totalCount = computed<number>(() => events.value.length);

  /** Count of critical events (no kind filter applied). Used by
   *  the "仅 critical" checkbox label. */
  const criticalCount = computed<number>(
    () => events.value.filter((e) => isCritical(e)).length,
  );

  /** Count of filtered events — the number actually shown in the
   *  list. The modal header renders this next to the filter chip. */
  const filteredCount = computed<number>(() => filteredEvents.value.length);

  return {
    // state
    events,
    loading,
    error,
    lastSessionId,
    kindFilter,
    onlyCritical,
    // actions
    loadForSession,
    refresh,
    setKindFilter,
    toggleCritical,
    // getters
    filteredEvents,
    totalCount,
    criticalCount,
    filteredCount,
    isCritical,
  };
});
