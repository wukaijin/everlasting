// Tests for `useAuditStore` — RULE-PERM-001 (2026-08-30) keyset
// pagination rework.
//
// The store moved from "full-pull + client-side filter/sort" to
// "page-1 load + loadMore cursor continuation + server-side filter
// and counts" (command `list_session_audit_events_page`). Coverage:
//
//   1. First load fetches page 1 with the current filter pushed down
//      (kind/criticalOnly ride the invoke args) and REPLACES any
//      previously accumulated pages.
//   2. loadMore appends using the `(ts, id)` cursor of the LAST
//      accumulated row — both cursor halves asserted on the wire
//      args (the backend rejects a lone beforeTs).
//   3. Filter changes re-pull page 1 — the accumulated pages of the
//      old filter are dropped (R2; filters are server-side now).
//   4. hasMore flips false when events.length >= matched, and
//      loadMore becomes a no-op (no cursor fetch fired).
//   5. Count getters map onto the server numbers (totalAll /
//      totalCritical / matched) — chips stay accurate for rows
//      never loaded (R3).
//   6. Error paths: a failed page-1 or loadMore keeps the previous
//      events/counts and sets `error` (unchanged failure policy).
//   7. Concurrency: a duplicate loadMore while one is in flight is a
//      silent no-op; a filter switch mid-loadMore discards the stale
//      append instead of corrupting the new page 1 (fetchSeq guard).
//
// Transport is mocked at the `../transport` barrel — the canonical
// store-test pattern (test-environment.md §4, mirrors
// projects.test.ts).

import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn();

vi.mock("../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

import { useAuditStore } from "./audit";
import type { AuditEventRow } from "../utils/audit";

/** Wire-shaped audit row factory. `ts`/`id` pairs are what the
 *  loadMore cursor is asserted against — keep them realistic
 *  (same-second ties with descending ids, like the real SQL order). */
function makeRow(overrides: Partial<AuditEventRow> = {}): AuditEventRow {
  return {
    id: 1,
    sessionId: "s1",
    ts: "2026-08-30 10:00:00",
    kind: "tool_executed",
    payloadJson: null,
    turnSeq: null,
    ...overrides,
  };
}

/** Wire-shaped page (`db::AuditEventPageRow` camelCase). */
function makePage(
  over: Partial<{
    events: AuditEventRow[];
    matched: number;
    totalAll: number;
    totalCritical: number;
  }> = {},
) {
  return {
    events: [],
    matched: 0,
    totalAll: 0,
    totalCritical: 0,
    ...over,
  };
}

/** Manual promise so tests can hold an invoke in flight (concurrency
 *  cases). */
function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

/** The filter setters reload page 1 fire-and-forget (`void` signature
 *  — the modal's v-model setters can't await). A macrotask hop drains
 *  every pending microtask deterministically; bare `await
 *  Promise.resolve()` chains would be hop-count-fragile. */
function drain(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

describe("useAuditStore — keyset pagination (RULE-PERM-001)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    invokeMock.mockImplementation(async () => makePage());
  });

  // -------------------------------------------------------------------
  // 1. First load — reset + page-1 args
  // -------------------------------------------------------------------

  it("首屏:filter 先行 arm(无 session 不发 IPC),page-1 参数携带过滤下推", async () => {
    const store = useAuditStore();
    // Arm filters BEFORE any session load (no lastSessionId → the
    // setters must not fire an IPC, just arm).
    store.setKindFilter("tool_denied");
    store.toggleCritical();
    expect(invokeMock).not.toHaveBeenCalled();

    invokeMock.mockImplementation(async () =>
      makePage({
        events: [makeRow({ id: 2, kind: "tool_denied" })],
        matched: 1,
        totalAll: 9,
        totalCritical: 2,
      }),
    );
    await store.loadForSession("s1");

    // Wire contract pinned exactly: camelCase args, filters pushed
    // down, no limit (server default 100), no cursor on page 1.
    expect(invokeMock).toHaveBeenCalledWith("list_session_audit_events_page", {
      sessionId: "s1",
      kind: "tool_denied",
      criticalOnly: true,
    });
    expect(store.events.map((r) => r.id)).toEqual([2]);
    expect(store.lastSessionId).toBe("s1");
    expect(store.error).toBeNull();
  });

  it("首屏:成功后替换旧累计页(不残留上一 session 的行)", async () => {
    const store = useAuditStore();
    invokeMock.mockImplementation(async () =>
      makePage({
        events: [makeRow({ id: 5 }), makeRow({ id: 4 })],
        matched: 2,
        totalAll: 2,
      }),
    );
    await store.loadForSession("s1");
    expect(store.events.length).toBe(2);

    // Re-open with a different session: the page must be REPLACED,
    // not appended — a stale row from s1 would corrupt the list and
    // the loadMore cursor (it anchors on the LAST row).
    invokeMock.mockImplementation(async () =>
      makePage({
        events: [makeRow({ id: 30, sessionId: "s2" })],
        matched: 1,
        totalAll: 1,
      }),
    );
    await store.loadForSession("s2");

    expect(store.events.map((r) => r.id)).toEqual([30]);
    expect(store.lastSessionId).toBe("s2");
    expect(store.totalCount).toBe(1);
  });

  // -------------------------------------------------------------------
  // 2. loadMore — cursor continuation
  // -------------------------------------------------------------------

  it("loadMore:游标取自已载最后一行的 (ts,id),结果 append 不替换", async () => {
    const store = useAuditStore();
    // Two same-second rows (a single turn's tool calls) — the tie is
    // broken by id, so the cursor MUST carry both halves.
    invokeMock.mockImplementation(async () =>
      makePage({
        events: [
          makeRow({ id: 3, ts: "2026-08-30 10:00:00" }),
          makeRow({ id: 2, ts: "2026-08-30 10:00:00" }),
        ],
        matched: 3,
        totalAll: 3,
        totalCritical: 1,
      }),
    );
    await store.loadForSession("s1");

    invokeMock.mockImplementation(async () =>
      makePage({
        events: [makeRow({ id: 1, ts: "2026-08-30 10:00:00" })],
        matched: 3,
        totalAll: 3,
        totalCritical: 1,
      }),
    );
    await store.loadMore();

    // Both cursor halves on the wire (a lone beforeTs is a backend
    // 400 — contract: always both or neither).
    expect(invokeMock).toHaveBeenLastCalledWith(
      "list_session_audit_events_page",
      {
        sessionId: "s1",
        beforeTs: "2026-08-30 10:00:00",
        beforeId: 2,
        kind: null,
        criticalOnly: false,
      },
    );
    // Appended at the tail in SQL order — page 1 rows stay in front.
    expect(store.events.map((r) => r.id)).toEqual([3, 2, 1]);
    expect(store.loadingMore).toBe(false);
  });

  it("loadMore:携带当前过滤下推(续拉行口径与列表一致,R2)", async () => {
    const store = useAuditStore();
    invokeMock.mockImplementation(async () =>
      makePage({
        events: [makeRow({ id: 2, kind: "tool_denied" })],
        matched: 2,
      }),
    );
    await store.loadForSession("s1");
    store.setKindFilter("tool_denied");
    await drain(); // settle the setter's fire-and-forget reload
    invokeMock.mockClear();

    invokeMock.mockImplementation(async () =>
      makePage({
        events: [makeRow({ id: 1, kind: "tool_denied" })],
        matched: 2,
      }),
    );
    await store.loadMore();

    expect(invokeMock).toHaveBeenLastCalledWith(
      "list_session_audit_events_page",
      {
        sessionId: "s1",
        beforeTs: "2026-08-30 10:00:00",
        beforeId: 2,
        kind: "tool_denied",
        criticalOnly: false,
      },
    );
  });

  // -------------------------------------------------------------------
  // 3. Filter changes re-pull page 1
  // -------------------------------------------------------------------

  it("setKindFilter:重拉第一页并丢弃旧过滤的累计页", async () => {
    const store = useAuditStore();
    invokeMock.mockImplementation(async () =>
      makePage({
        events: [makeRow({ id: 2 }), makeRow({ id: 1 })],
        matched: 2,
        totalAll: 2,
      }),
    );
    await store.loadForSession("s1");

    invokeMock.mockImplementation(async () =>
      makePage({
        events: [makeRow({ id: 9, kind: "tool_denied" })],
        matched: 1,
        totalAll: 2,
      }),
    );
    store.setKindFilter("tool_denied");
    await drain();

    expect(invokeMock).toHaveBeenCalledWith("list_session_audit_events_page", {
      sessionId: "s1",
      kind: "tool_denied",
      criticalOnly: false,
    });
    // Old pages gone — the 2 unfiltered rows are replaced by the
    // filtered page 1.
    expect(store.events.map((r) => r.id)).toEqual([9]);
    expect(store.filteredCount).toBe(1);
  });

  it("toggleCritical:翻转状态并重拉第一页", async () => {
    const store = useAuditStore();
    invokeMock.mockImplementation(async () =>
      makePage({ events: [makeRow({ id: 2 })], matched: 1, totalAll: 5 }),
    );
    await store.loadForSession("s1");

    invokeMock.mockImplementation(async () =>
      makePage({
        events: [makeRow({ id: 3, payloadJson: '{"critical":true}' })],
        matched: 1,
        totalAll: 5,
        totalCritical: 1,
      }),
    );
    store.toggleCritical();
    await drain();

    expect(store.onlyCritical).toBe(true);
    expect(invokeMock).toHaveBeenCalledWith("list_session_audit_events_page", {
      sessionId: "s1",
      kind: null,
      criticalOnly: true,
    });
    expect(store.events.map((r) => r.id)).toEqual([3]);
  });

  // -------------------------------------------------------------------
  // 4. hasMore termination
  // -------------------------------------------------------------------

  it("hasMore:载满 matched 后翻 false,loadMore 变 no-op(不发 IPC)", async () => {
    const store = useAuditStore();
    invokeMock.mockImplementation(async () =>
      makePage({
        events: [makeRow({ id: 2 }), makeRow({ id: 1 })],
        matched: 2,
        totalAll: 2,
      }),
    );
    await store.loadForSession("s1");
    expect(store.hasMore).toBe(false);

    invokeMock.mockClear();
    await store.loadMore();
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("hasMore:未载满时为 true", async () => {
    const store = useAuditStore();
    invokeMock.mockImplementation(async () =>
      makePage({ events: [makeRow({ id: 2 }), makeRow({ id: 1 })], matched: 5 }),
    );
    await store.loadForSession("s1");
    expect(store.hasMore).toBe(true);
  });

  it("空首屏:matched=0 → hasMore false;无游标行 → loadMore no-op", async () => {
    const store = useAuditStore();
    invokeMock.mockImplementation(async () => makePage());
    await store.loadForSession("s9");
    expect(store.hasMore).toBe(false);

    await store.loadMore(); // empty page 1 → no cursor row → no-op
    expect(invokeMock).toHaveBeenCalledTimes(1); // only the page-1 load
  });

  // -------------------------------------------------------------------
  // 5. Count getters map onto the server numbers
  // -------------------------------------------------------------------

  it("count getters:totalCount/criticalCount/filteredCount 接服务端三值", async () => {
    const store = useAuditStore();
    // Server says: 9 rows total, 3 critical, 4 match the current
    // filter — while only 2 rows are actually loaded. The chips must
    // show the SERVER numbers (R3), not the loaded-page counts.
    invokeMock.mockImplementation(async () =>
      makePage({
        events: [makeRow({ id: 2 }), makeRow({ id: 1 })],
        matched: 4,
        totalAll: 9,
        totalCritical: 3,
      }),
    );
    await store.loadForSession("s1");

    expect(store.totalCount).toBe(9);
    expect(store.criticalCount).toBe(3);
    expect(store.filteredCount).toBe(4);
    expect(store.events.length).toBe(2); // loaded ≠ counted
    // isCritical stays for the per-row badge (payload parsing); the
    // counting job itself moved server-side.
    expect(store.isCritical(makeRow({ payloadJson: '{"critical":true}' }))).toBe(true);
    expect(store.isCritical(makeRow({ payloadJson: "not json" }))).toBe(false);
    expect(store.isCritical(makeRow({ payloadJson: null }))).toBe(false);
  });

  // -------------------------------------------------------------------
  // 6. Error paths — keep previous state, set error
  // -------------------------------------------------------------------

  it("loadForSession 失败:保旧 events/counts,set error,loading 复位", async () => {
    const store = useAuditStore();
    invokeMock.mockImplementation(async () =>
      makePage({ events: [makeRow()], matched: 1, totalAll: 1 }),
    );
    await store.loadForSession("s1");

    invokeMock.mockImplementation(async () => {
      throw new Error("daemon gone");
    });
    await store.refresh();

    expect(store.error).toBe("daemon gone");
    expect(store.events.map((r) => r.id)).toEqual([1]);
    expect(store.totalCount).toBe(1);
    expect(store.loading).toBe(false);
  });

  it("loadMore 失败:保累计页,set error,loadingMore 复位", async () => {
    const store = useAuditStore();
    invokeMock.mockImplementation(async () =>
      makePage({
        events: [makeRow({ id: 2 }), makeRow({ id: 1 })],
        matched: 4,
      }),
    );
    await store.loadForSession("s1");

    invokeMock.mockImplementation(async () => {
      throw new Error("cursor fetch failed");
    });
    await store.loadMore();

    expect(store.error).toBe("cursor fetch failed");
    expect(store.events.map((r) => r.id)).toEqual([2, 1]);
    expect(store.loadingMore).toBe(false);
    expect(store.hasMore).toBe(true); // still 2/4 — retryable via 加载更多
  });

  // -------------------------------------------------------------------
  // 7. Concurrency guards
  // -------------------------------------------------------------------

  it("并发 loadMore:进行中重复调用是 no-op,只发一次游标 IPC", async () => {
    const store = useAuditStore();
    invokeMock.mockImplementation(async () =>
      makePage({
        events: [makeRow({ id: 2 }), makeRow({ id: 1 })],
        matched: 3,
      }),
    );
    await store.loadForSession("s1");

    const gate = deferred<ReturnType<typeof makePage>>();
    invokeMock.mockImplementation(async () => gate.promise);
    const first = store.loadMore();
    const second = store.loadMore(); // duplicate click mid-flight
    await second; // resolves immediately, no second cursor IPC
    expect(invokeMock).toHaveBeenCalledTimes(2); // page 1 + ONE cursor call

    gate.resolve(makePage({ events: [makeRow({ id: 0 })], matched: 3 }));
    await first;
    expect(store.events.map((r) => r.id)).toEqual([2, 1, 0]);
  });

  it("loadMore 进行中切换过滤:过期的 append 被丢弃,不污染新过滤的页", async () => {
    const store = useAuditStore();
    invokeMock.mockImplementation(async () =>
      makePage({
        events: [makeRow({ id: 2 }), makeRow({ id: 1 })],
        matched: 4,
      }),
    );
    await store.loadForSession("s1");

    // Hold the cursor fetch, then switch the filter mid-flight.
    const gate = deferred<ReturnType<typeof makePage>>();
    invokeMock.mockImplementation(async () => gate.promise);
    const pending = store.loadMore();

    invokeMock.mockImplementation(async () =>
      makePage({
        events: [makeRow({ id: 9, kind: "tool_denied" })],
        matched: 1,
      }),
    );
    store.setKindFilter("tool_denied");
    // Drain the filter's page-1 reload before releasing the stale
    // cursor response.
    await drain();
    expect(store.events.map((r) => r.id)).toEqual([9]);

    // Late stale response: old-filter rows arriving AFTER the new
    // page 1 — must be discarded, not appended.
    gate.resolve(makePage({ events: [makeRow({ id: 0 })], matched: 4 }));
    await pending;

    expect(store.events.map((r) => r.id)).toEqual([9]);
    expect(store.filteredCount).toBe(1); // the filter page's matched
  });

  it("refresh:重拉第一页(重锚到最新),游标累积页被替换", async () => {
    const store = useAuditStore();
    invokeMock.mockImplementation(async () =>
      makePage({
        events: [makeRow({ id: 2 }), makeRow({ id: 1 })],
        matched: 4,
      }),
    );
    await store.loadForSession("s1");
    invokeMock.mockImplementation(async () =>
      makePage({ events: [makeRow({ id: 1 })], matched: 4 }),
    );
    await store.loadMore(); // appends the cursor page → [2,1,1]
    expect(store.events.length).toBe(3);

    // Manual refresh re-anchors: back to a fresh page 1 (the mock's
    // page now holds NEWER rows the cursor pages never had).
    invokeMock.mockImplementation(async () =>
      makePage({
        events: [
          makeRow({ id: 5 }),
          makeRow({ id: 4 }),
          makeRow({ id: 3 }),
        ],
        matched: 6,
        totalAll: 6,
      }),
    );
    await store.refresh();

    expect(invokeMock).toHaveBeenLastCalledWith(
      "list_session_audit_events_page",
      { sessionId: "s1", kind: null, criticalOnly: false },
    );
    expect(store.events.map((r) => r.id)).toEqual([5, 4, 3]);
    expect(store.hasMore).toBe(true); // 3 / 6
  });
});
