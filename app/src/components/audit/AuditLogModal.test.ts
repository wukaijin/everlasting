// Tests for `AuditLogModal.vue` — RULE-PERM-001 (2026-08-30) keyset
// pagination UI.
//
// Coverage (implement.md PR3 / PRD AC5):
//   1. 「加载更多」 button renders iff `store.hasMore` (disappears at
//      the end of the list — R1).
//   2. Click fires `store.loadMore()` — cursor args asserted on the
//      invoke (beforeTs/beforeId from the LAST rendered row) and the
//      list grows without a modal re-open.
//   3. In-flight loadMore shows the button's busy state (加载中…,
//      disabled).
//   4. Count chip text: "X 项" unfiltered, "X / Y 项" with a filter
//      active — values come from the SERVER counts (R3), so the
//      mock page's matched/totalAll drive the chip directly.
//
// Mirror of PermissionGrantsModal.test.ts: reka DialogContent
// teleports to <body>, so every test queries the body and
// `beforeEach` wipes leaked portal DOM. The store is NOT stubbed —
// the real audit store runs against the mocked transport (the
// precedent for store-coupled modal tests), so the open-watcher →
// loadForSession → invoke chain is exercised end to end.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn(async (): Promise<unknown> => null);

vi.mock("../../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...(args as Parameters<typeof invokeMock>)),
    listen: async () => () => {},
  },
}));

import AuditLogModal from "./AuditLogModal.vue";
import { useChatStore } from "../../stores/chat";
import { useAuditStore } from "../../stores/audit";
import type { AuditEventRow } from "../../utils/audit";

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

/** Wire-shaped page (`db::AuditEventPageRow` camelCase) — the invoke
 *  answer for `list_session_audit_events_page`. */
function auditPage(over: Partial<{
  events: AuditEventRow[];
  matched: number;
  totalAll: number;
  totalCritical: number;
}> = {}) {
  return {
    events: [] as AuditEventRow[],
    matched: 0,
    totalAll: 0,
    totalCritical: 0,
    ...over,
  };
}

/** Hold an invoke in flight (button busy-state test). */
function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

/** Seed the chat store so `boundSessionId` resolves and the modal
 *  loads page 1 on open. (Same shape as PermissionGrantsModal's
 *  seedChat — `as never` because only the fields the modal reads
 *  are real.) */
function seedChat(sessionId: string | null) {
  const chat = useChatStore();
  chat.currentSessionId = sessionId;
  if (sessionId) {
    chat.sessions.push({
      id: sessionId,
      title: "长会话",
      updated_at: "",
      preview: "",
      project_id: "p1",
      current_cwd: "/tmp",
      worktree_path: null,
      worktree_state: "none",
      last_worktree_path: null,
      model_id: null,
      input_tokens_total: null,
      output_tokens_total: null,
      cache_creation_total: null,
      cache_read_total: null,
      last_context_input_tokens: null,
      last_input_tokens: null,
      last_output_tokens: null,
      last_cache_creation: null,
      last_cache_read: null,
      color_tag: null,
      mode: "edit",
      session_type: "chat",
      busy: false,
    } as never);
  }
}

function mountModal(open: boolean) {
  return mount(AuditLogModal, {
    attachTo: document.body,
    props: { open },
    global: { stubs: { Icon: true } },
  });
}

/** The modal loads on the open transition (`watch(open)`), so tests
 *  mount closed and then flip the prop. */
async function openModal() {
  const w = mountModal(false);
  await w.setProps({ open: true });
  await flushPromises();
  return w;
}

function moreButton(): HTMLButtonElement | null {
  return document.body.querySelector<HTMLButtonElement>(".audit-modal__more-btn");
}

function countChip(): string {
  return document.body.querySelector(".audit-modal__count")?.textContent?.trim() ?? "";
}

describe("AuditLogModal — 加载更多 (RULE-PERM-001)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockClear();
    invokeMock.mockImplementation(async () => auditPage());
    // reka DialogContent teleports to body; wipe leaks between tests.
    document.body
      .querySelectorAll(".audit-modal, .audit-modal__overlay")
      .forEach((el) => el.remove());
  });

  it("hasMore=true → 加载更多 可见;载满 matched → 消失(R1)", async () => {
    // Page 1 of 3: 2 rows loaded, 1 more filtered row on the server.
    invokeMock.mockImplementation(async () =>
      auditPage({
        events: [makeRow({ id: 2 }), makeRow({ id: 1 })],
        matched: 3,
        totalAll: 3,
        totalCritical: 0,
      }),
    );
    seedChat("s1");
    const w = await openModal();

    expect(document.body.querySelector(".audit-modal__list")).not.toBeNull();
    expect(document.body.querySelectorAll(".audit-item").length).toBe(2);
    expect(moreButton()).not.toBeNull();
    w.unmount();

    // Same session fully loaded: hasMore false → button gone.
    const w2 = mountModal(false);
    invokeMock.mockImplementation(async () =>
      auditPage({
        events: [makeRow({ id: 2 }), makeRow({ id: 1 })],
        matched: 2,
        totalAll: 2,
        totalCritical: 0,
      }),
    );
    await w2.setProps({ open: true });
    await flushPromises();
    expect(moreButton()).toBeNull();
    w2.unmount();
  });

  it("点击加载更多:以最后一行 (ts,id) 为游标调 loadMore,列表原地追加", async () => {
    invokeMock.mockImplementation(async () =>
      auditPage({
        events: [
          makeRow({ id: 2, ts: "2026-08-30 10:00:05" }),
          makeRow({ id: 1, ts: "2026-08-30 10:00:05" }),
        ],
        matched: 3,
        totalAll: 3,
        totalCritical: 0,
      }),
    );
    seedChat("s1");
    const w = await openModal();

    // Page 2: rows strictly older than the cursor row (id 1).
    invokeMock.mockImplementation(async () =>
      auditPage({
        events: [makeRow({ id: 0, ts: "2026-08-30 09:59:00" })],
        matched: 3,
        totalAll: 3,
        totalCritical: 0,
      }),
    );
    moreButton()!.click();
    await flushPromises();

    // Both cursor halves on the wire, taken from the LAST rendered
    // row (not the first) — the keyset anchor.
    expect(invokeMock).toHaveBeenLastCalledWith(
      "list_session_audit_events_page",
      {
        sessionId: "s1",
        beforeTs: "2026-08-30 10:00:05",
        beforeId: 1,
        kind: null,
        criticalOnly: false,
      },
    );
    // Appended in place — 3 rows now, and the button is gone
    // (events.length === matched).
    expect(document.body.querySelectorAll(".audit-item").length).toBe(3);
    expect(moreButton()).toBeNull();
    w.unmount();
  });

  it("加载中:按钮禁用 + 文案切换「加载中…」", async () => {
    invokeMock.mockImplementation(async () =>
      auditPage({
        events: [makeRow({ id: 2 }), makeRow({ id: 1 })],
        matched: 3,
        totalAll: 3,
        totalCritical: 0,
      }),
    );
    seedChat("s1");
    const w = await openModal();

    const gate = deferred<ReturnType<typeof auditPage>>();
    invokeMock.mockImplementation(async () => gate.promise);
    moreButton()!.click();
    await flushPromises();

    const busy = moreButton();
    expect(busy).not.toBeNull();
    expect(busy!.disabled).toBe(true);
    expect(busy!.textContent).toContain("加载中");

    gate.resolve(
      auditPage({
        events: [makeRow({ id: 0, ts: "2026-08-30 09:59:00" })],
        matched: 3,
        totalAll: 3,
        totalCritical: 0,
      }),
    );
    await flushPromises();
    expect(document.body.querySelectorAll(".audit-item").length).toBe(3);
    w.unmount();
  });

  it("计数 chip:无过滤「X 项」,有过滤「X / Y 项」(服务端数值,R3)", async () => {
    // Unfiltered: chip shows the server total only.
    invokeMock.mockImplementation(async () =>
      auditPage({
        events: [makeRow({ id: 2 }), makeRow({ id: 1 })],
        matched: 2,
        totalAll: 5,
        totalCritical: 1,
      }),
    );
    seedChat("s1");
    const w = await openModal();
    expect(countChip()).toBe("5 项");
    // critical checkbox label carries the server critical count.
    expect(document.body.querySelector(".audit-modal__check-label")?.textContent)
      .toContain("(1)");
    w.unmount();

    // Filtered: the audit store still holds lastSessionId="s1" from
    // the open above, so setKindFilter fires its re-pull immediately
    // — the filtered page must be the live mock BEFORE arming.
    invokeMock.mockImplementation(async () =>
      auditPage({
        events: [makeRow({ id: 9, kind: "tool_denied" })],
        matched: 1,
        totalAll: 5,
        totalCritical: 1,
      }),
    );
    const audit = useAuditStore();
    audit.setKindFilter("tool_denied");
    await flushPromises(); // drain the setter's fire-and-forget reload
    const w2 = mountModal(false);
    await w2.setProps({ open: true });
    await flushPromises();

    expect(countChip()).toBe("1 / 5 项");
    // The open load carried the filter down to the server.
    expect(invokeMock).toHaveBeenCalledWith("list_session_audit_events_page", {
      sessionId: "s1",
      kind: "tool_denied",
      criticalOnly: false,
    });
    w2.unmount();
  });

  it("空态文案:totalAll=0 →「暂无审计事件」;totalAll>0 过滤无命中 →「无匹配事件」", async () => {
    // 服务端过滤下 store.events 只含命中行,「会话没有事件」与「过滤
    // 无命中」必须由 totalAll 区分(回归:曾用 events.length === 0
    // 判断,过滤无命中时误报「暂无审计事件」)。
    seedChat("s1");

    // 过滤无命中:matched 0,但会话本身有 175 条事件。
    invokeMock.mockImplementation(async () =>
      auditPage({
        events: [],
        matched: 0,
        totalAll: 175,
        totalCritical: 0,
      }),
    );
    const w = await openModal();
    const placeholder = document.body.querySelector(".audit-modal__placeholder");
    expect(placeholder).not.toBeNull();
    expect(placeholder!.textContent).toContain("无匹配事件");
    expect(placeholder!.textContent).not.toContain("暂无审计事件");
    w.unmount();

    // 会话真的没有事件:totalAll = 0。
    const w2 = mountModal(false);
    invokeMock.mockImplementation(async () => auditPage());
    await w2.setProps({ open: true });
    await flushPromises();
    const placeholder2 = document.body.querySelector(".audit-modal__placeholder");
    expect(placeholder2).not.toBeNull();
    expect(placeholder2!.textContent).toContain("暂无审计事件");
    w2.unmount();
  });
});
