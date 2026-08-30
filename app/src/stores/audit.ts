// useAuditStore — Pinia store for the C4 audit-log query UI.
//
// Backend (RULE-PERM-001, 2026-08-30) exposes a keyset-paginated read
// `list_session_audit_events_page` (Tauri + daemon dual transport)
// returning a page object:
//
//   { events: AuditEventRow[],   // pre-sorted `ts DESC, id DESC` by SQL
//     matched: number,           // rows matching the CURRENT filter
//     totalAll: number,          // unfiltered total
//     totalCritical: number }    // critical total (NOT scoped by kind)
//
// with args `{ sessionId, limit?, beforeTs?, beforeId?, kind?,
// criticalOnly? }` (camelCase on both transports — the httpTransport
// re-snakes top-level keys for the daemon routes). The old full-pull
// command `list_session_audit_events` is untouched and still serves
// `traceStore`'s turn grouping (R6).
//
// This store is the frontend-side reactive wrapper that:
//
//   1. Loads page 1 (default 100 rows) on modal-open
//      (`loadForSession(sessionId)`), pushing the current filters
//      (kindFilter / onlyCritical) DOWN to the server — filtering is
//      no longer client-side, so list page, `matched` and the
//      loadMore continuation share one filter scope (R2).
//   2. Continues pagination via `loadMore()` — the keyset cursor is
//      the `(ts, id)` pair of the LAST accumulated row, always sent
//      together (the backend rejects a lone beforeTs). Keyset (not
//      OFFSET) keeps the "earlier page" stable while the agent keeps
//      appending rows mid-open (R5): OFFSET would shift a whole page
//      when new rows insert at the top; the cursor anchors on the
//      boundary row itself.
//   3. Re-pulls page 1 when a filter changes (`setKindFilter` /
//      `toggleCritical`) — the accumulated pages of the old filter
//      are meaningless under the new one.
//   4. Exposes the count getters (`totalCount` / `criticalCount` /
//      `filteredCount`) mapped onto the SERVER numbers, so the
//      modal's count chips stay accurate for rows never loaded (R3).
//      `hasMore = events.length < matched` gates the modal's
//      「加载更多」 button (D4 — a plain button, no infinite scroll).
//   5. Exposes `refresh()` for the modal's manual refresh button —
//      reloads page 1, re-anchoring to the newest rows.
//
// Failure policy (unchanged from the full-pull era): any IPC failure
// is caught and stored in `error`; `events` + counts keep their
// previous values so the modal can render the stale state with an
// error banner instead of crashing.
//
// State model:
//   - `events: AuditEventRow[]` — accumulated pages in SQL order
//     (newest first; `loadMore` appends strictly older rows at the
//     tail). The SQL guarantees `ts DESC, id DESC` (R4), so the old
//     client-side `sortEvents` re-sort is DELETED.
//   - `matched / totalAll / totalCritical: number` — server counts.
//   - `loading: boolean` (page-1 fetch) vs `loadingMore: boolean`
//     (cursor fetch) — separate flags so the 刷新 button and the
//     「加载更多」 button each show their own busy state.
//   - `error: string | null`, `lastSessionId: string | null`,
//   - `kindFilter: string | null` — null = "全部"; `onlyCritical:
//     boolean` — same semantics as before, but now consumed by the
//     server on the next fetch rather than by a client-side filter.

import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { transport } from "../transport";
import { extractErrorMessage } from "../utils/useErrorBus";
import type { AuditEventRow } from "../utils/audit";

/** Wire shape of `list_session_audit_events_page` — mirrors
 *  `db::AuditEventPageRow` (`#[serde(rename_all = "camelCase")]`). */
interface AuditEventPage {
  events: AuditEventRow[];
  matched: number;
  totalAll: number;
  totalCritical: number;
}

export const useAuditStore = defineStore("audit", () => {
  // -----------------------------------------------------------------------
  // State
  // -----------------------------------------------------------------------

  /** Accumulated pages (SQL-ordered; page 1 first, older pages
   *  appended at the tail by `loadMore`). */
  const events = ref<AuditEventRow[]>([]);
  const matched = ref<number>(0);
  const totalAll = ref<number>(0);
  const totalCritical = ref<number>(0);
  const loading = ref<boolean>(false);
  const loadingMore = ref<boolean>(false);
  const error = ref<string | null>(null);
  const lastSessionId = ref<string | null>(null);
  const kindFilter = ref<string | null>(null);
  const onlyCritical = ref<boolean>(false);

  /** Monotonic fetch token for the stale-response race: a page-1
   *  reload that starts while a `loadMore` is in flight must win —
   *  the loadMore's response (old-filter rows) would otherwise
   *  append into the NEW filter's page 1. Every fetch captures the
   *  token at start and bails at resolution when a newer fetch has
   *  superseded it. */
  let fetchSeq = 0;

  // -----------------------------------------------------------------------
  // Fetching
  // -----------------------------------------------------------------------

  /** Apply a page payload to the state. Split from the fetch helpers
   *  so the stale-token guard reads once instead of four times. */
  function applyPage(page: AuditEventPage): void {
    events.value = page.events;
    matched.value = page.matched;
    totalAll.value = page.totalAll;
    totalCritical.value = page.totalCritical;
  }

  /** Map an IPC error to the user-facing message (same rule as
   *  every store here: real Error.message wins, otherwise the
   *  error-bus extractor). */
  function toErrorMessage(e: unknown): string {
    return e instanceof Error ? e.message : extractErrorMessage(e);
  }

  /** Common filter args pushed down on EVERY fetch (R2 — the list
   *  page and the counts must share one filter scope). `kind: null`
   *  deserializes to `Option::None` / `Option<String>::None` on both
   *  transports. */
  function filterArgs(): { kind: string | null; criticalOnly: boolean } {
    return { kind: kindFilter.value, criticalOnly: onlyCritical.value };
  }

  /** Load page 1 for a session, replacing whatever pages are
   *  accumulated. Used by:
   *   - the modal's open watcher (`loadForSession`),
   *   - the manual 刷新 button (`refresh` — re-anchors to newest),
   *   - the filter setters (a filter change invalidates the
   *     accumulated pages of the old filter).
   *  On failure, sets `error` and leaves `events`/counts at the
   *  previous value (defensive — unchanged policy). Safe to call
   *  multiple times. */
  async function loadForSession(sessionId: string): Promise<void> {
    const seq = ++fetchSeq;
    loading.value = true;
    error.value = null;
    try {
      const page = await transport.invoke<AuditEventPage>(
        "list_session_audit_events_page",
        { sessionId, ...filterArgs() },
      );
      if (seq !== fetchSeq) return; // superseded — the newer fetch owns the state
      applyPage(page);
      lastSessionId.value = sessionId;
    } catch (e) {
      if (seq !== fetchSeq) return;
      error.value = toErrorMessage(e);
    } finally {
      if (seq === fetchSeq) loading.value = false;
    }
  }

  /** Append the next page, cursoring on the `(ts, id)` of the LAST
   *  accumulated row (both halves always travel together — the
   *  backend rejects a partial cursor). Guards:
   *   - `hasMore` false (everything already loaded): no-op;
   *   - `loadingMore` / `loading`: one in-flight fetch at a time per
   *     direction (a duplicate click or a page-1 reload mid-flight
   *     must not stack a second cursor fetch on a stale cursor);
   *   - `lastSessionId` / empty `events`: nothing to continue from.
   *  On failure: `error` set, accumulated pages untouched. */
  async function loadMore(): Promise<void> {
    if (events.value.length >= matched.value) return;
    if (loadingMore.value || loading.value) return;
    const sid = lastSessionId.value;
    const last = events.value[events.value.length - 1];
    if (!sid || !last) return;
    const seq = ++fetchSeq;
    loadingMore.value = true;
    error.value = null;
    try {
      const page = await transport.invoke<AuditEventPage>(
        "list_session_audit_events_page",
        {
          sessionId: sid,
          beforeTs: last.ts,
          beforeId: last.id,
          ...filterArgs(),
        },
      );
      if (seq !== fetchSeq) return; // a filter switch re-anchored meanwhile
      // Append, don't replace — the response holds only the rows
      // strictly older than the cursor (SQL order preserved).
      events.value = [...events.value, ...page.events];
      matched.value = page.matched;
      totalAll.value = page.totalAll;
      totalCritical.value = page.totalCritical;
    } catch (e) {
      if (seq !== fetchSeq) return;
      error.value = toErrorMessage(e);
    } finally {
      loadingMore.value = false;
    }
  }

  /** Re-fetch page 1 of the last-loaded session. Used by the modal's
   *  manual refresh button — live push is still OOS; this re-anchors
   *  to the newest rows. */
  async function refresh(): Promise<void> {
    if (!lastSessionId.value) return;
    await loadForSession(lastSessionId.value);
  }

  // -----------------------------------------------------------------------
  // Filter actions
  // -----------------------------------------------------------------------

  /** Arm the kind filter AND re-pull page 1 under it (R2 — the
   *  filter now lives server-side, so the accumulated pages of the
   *  old filter are stale). Fire-and-forget reload keeps the `void`
   *  signature the modal's `SelectRoot` v-model setter binds to.
   *  When no session is loaded yet, just arms the filter for the
   *  next `loadForSession`. */
  function setKindFilter(kind: string | null): void {
    kindFilter.value = kind;
    if (lastSessionId.value) void loadForSession(lastSessionId.value);
  }

  /** Mirror image of `setKindFilter` for the 仅 critical checkbox —
   *  same re-pull semantics, same `void` signature (the modal's
   *  `CheckboxRoot` v-model setter). */
  function toggleCritical(): void {
    onlyCritical.value = !onlyCritical.value;
    if (lastSessionId.value) void loadForSession(lastSessionId.value);
  }

  // -----------------------------------------------------------------------
  // Getters
  // -----------------------------------------------------------------------

  /** `true` while un-fetched filtered rows remain
   *  (`events.length < matched`). Gates the modal's 「加载更多」
   *  button — it disappears at the end of the list (R1). */
  const hasMore = computed<boolean>(() => events.value.length < matched.value);

  /** The rows the modal's list iterates. Server-side filtering
   *  (R2) means this IS `events` — every accumulated row already
   *  satisfies the active filter, and the SQL order is the display
   *  order (R4). The name stays so the modal's bindings are
   *  untouched. */
  const filteredEvents = computed<AuditEventRow[]>(() => events.value);

  /** Total event count (no filters) — server-computed, accurate for
   *  rows never loaded. Feeds the modal's count chip ("X 项"). */
  const totalCount = computed<number>(() => totalAll.value);

  /** Count of critical events (NOT scoped by the kind filter) —
   *  server-computed. Feeds the 仅 critical checkbox label. */
  const criticalCount = computed<number>(() => totalCritical.value);

  /** Count of rows matching the CURRENT filter — server-computed.
   *  Feeds the count chip's "X / Y 项" filtered numerator. */
  const filteredCount = computed<number>(() => matched.value);

  /** `true` if a row is a critical Tier 2 hard-kill denial. Reads
   *  the parsed `payload.critical` field; falls back to `false`
   *  when the payload is missing / malformed. Kept for per-row
   *  badge rendering — the COUNTING job moved server-side
   *  (`criticalCount` above). */
  function isCritical(row: AuditEventRow): boolean {
    if (!row.payloadJson) return false;
    try {
      const p = JSON.parse(row.payloadJson);
      return !!p && typeof p === "object" && (p as { critical?: boolean }).critical === true;
    } catch {
      return false;
    }
  }

  return {
    // state
    events,
    matched,
    totalAll,
    totalCritical,
    loading,
    loadingMore,
    error,
    lastSessionId,
    kindFilter,
    onlyCritical,
    // actions
    loadForSession,
    loadMore,
    refresh,
    setKindFilter,
    toggleCritical,
    // getters
    hasMore,
    filteredEvents,
    totalCount,
    criticalCount,
    filteredCount,
    isCritical,
  };
});
