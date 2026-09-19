// Tests for `MessageList.vue` — N4 PR1 rewrite (virtualization,
// 09-19-n4-render-virtualization).
//
// Test layering (design §7): jsdom has NO layout — these tests assert
// DECISIONS, never scroll RESULTS. The real virtualizer is swapped for
// a fixed-size fake (the 假布局 harness): `useVirtualizer` from
// @tanstack/vue-virtual is mocked to hand back an object whose
// getVirtualItems derives from the composable's own options (count +
// getItemKey), so the real composable runs end-to-end and every
// anchoring action is asserted as a CALL on the fake
// (scrollToEnd / scrollToIndex spies) — scrollTop/scrollHeight
// assertions here would be fake-green (jsdom scroll doesn't move).
//
// Coverage:
//   1. Structure: .messages is a DIV; spacer height = totalSize; rows
//      are absolutely-positioned .vrow with data-index + translateY;
//      run spacing classes (run-first 12px / run-rest 6px, index 0
//      exempt); MessageItem root is DIV (.msg) with data-seq.
//   2. Back-to-bottom button: visibility driven by isAtEnd via the
//      scroll handler; click = jumpToBottom decision (scrollToEnd with
//      smooth while idle) + button hides.
//   3. pending CH8-2a: null→some forces scrollToEnd (instant);
//      some→some does not re-trigger; some→null does nothing.
//   4. pendingScrollSeq (AC4 前半): the store command resolves to
//      scrollToIndex(align:'center') on the flattened row and resets.
//   5. Handwritten force-follow (spike conclusion 2): streaming + force
//      + append → unconditional scrollToEnd; without force → no call
//      from OUR path (library's own follow is not this code).
//   6. Mount lands at the bottom (single scrollToEnd — the retired
//      stickToBottomUntilStable's replacement).
//   7. N4 PR3: data-seq flash (`.search-hit` on the hit wrapper, cleared
//      after SEARCH_FLASH_MS) and D4 run-enter phases (from+active on a
//      newly appended run head → double-rAF release → active-only →
//      cleared; negatives: assistant fold-in and whole-array replacement
//      never animate).

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { setActivePinia, createPinia } from "pinia";
import { nextTick } from "vue";

const invokeMock = vi.fn(async (): Promise<unknown> => null);

vi.mock("../../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...(args as Parameters<typeof invokeMock>)),
    listen: async () => () => {},
  },
}));

// ---------------------------------------------------------------------------
// 假布局 harness:fixed-size fake virtualizer(60px/项,start = i*60)。
// getVirtualItems / getTotalSize 从 composable 传入的 options 派生
// (count + getItemKey),因此真实 composable 全链路可跑;锚定动作 =
// spy 调用断言。isAtEnd 由测试侧开关(按钮显隐的决策源)。
// ---------------------------------------------------------------------------
const fake = vi.hoisted(() => {
  const calls = {
    scrollToEnd: [] as Array<Record<string, unknown> | undefined>,
    scrollToIndex: [] as Array<[number, Record<string, unknown> | undefined]>,
    willUpdate: 0,
    measure: 0,
  };
  let atEnd = true;
  const ITEM_H = 60;
  const makeFake = (opts: {
    value: { count: number; getItemKey: (i: number) => string };
  }) => ({
    getVirtualItems() {
      const { count, getItemKey } = opts.value;
      const out: Array<{
        index: number;
        key: string;
        start: number;
        size: number;
      }> = [];
      for (let i = 0; i < count; i += 1) {
        out.push({ index: i, key: getItemKey(i), start: i * ITEM_H, size: ITEM_H });
      }
      return out;
    },
    getTotalSize: () => opts.value.count * ITEM_H,
    measureElement: () => {
      calls.measure += 1;
    },
    isAtEnd: () => atEnd,
    scrollToEnd: (o?: Record<string, unknown>) => {
      calls.scrollToEnd.push(o);
    },
    scrollToIndex: (i: number, o?: Record<string, unknown>) => {
      calls.scrollToIndex.push([i, o]);
    },
    _willUpdate: () => {
      calls.willUpdate += 1;
    },
  });
  return {
    calls,
    makeFake,
  };
});

vi.mock("@tanstack/vue-virtual", async () => {
  const { shallowRef } = await import("vue");
  return {
    useVirtualizer: (opts: never) => shallowRef(fake.makeFake(opts)),
  };
});

import MessageList from "./MessageList.vue";
import {
  RUN_ENTER_ACTIVE_MS,
  SEARCH_FLASH_MS,
} from "../../composables/useVirtualizedMessages";
import { useChatStore } from "../../stores/chat";
import { useStreamControllerStore } from "../../stores/streamController";
import { useQuestionCardsStore } from "../../stores/questionCards";
import * as messageFormat from "../../utils/messageFormat";
import type { PendingInteraction } from "../../stores/questionCards.types";

const { calls } = fake;

function makePending(sessionId: string, toolUseId: string): PendingInteraction {
  return {
    kind: "question",
    payload: {
      session_id: sessionId,
      tool_use_id: toolUseId,
      ts: Date.now(),
      questions: [
        {
          question: "继续吗?",
          options: [{ label: "继续" }, { label: "停止" }],
          multi_select: false,
        },
      ],
    },
  };
}

/** jsdom 无布局 —— 断言不读滚动结果值(设计 §7);但 onScroll 的按钮
 *  显隐 / force 退出判定以 DOM 距底读数为**输入**,这里把输入喂进元素
 *  (defineProperty + scrollTop),再派发 scroll 事件。断言仍全部落在
 *  决策(按钮出现与否 / store flag / fake 库调用记录)上。 */
function seedScrollInputs(w: ReturnType<typeof mount>, scrollTop: number): void {
  const el = w.get(".messages").element as HTMLElement;
  Object.defineProperty(el, "scrollHeight", { value: 1000, configurable: true });
  Object.defineProperty(el, "clientHeight", { value: 400, configurable: true });
  el.scrollTop = scrollTop; // 1000 - scrollTop - 400 < 80 → near
  el.dispatchEvent(new Event("scroll"));
}
async function mountList() {
  const w = mount(MessageList, {
    attachTo: document.body,
    global: { stubs: { MessageItem: false, Icon: true } },
  });
  await flushPromises();
  await nextTick();
  return w;
}

function seedSessionWithMessages(sessionId: string) {
  const store = useChatStore();
  store.sessions = [
    {
      id: sessionId,
      title: "t",
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
    } as never,
  ];
  store.currentSessionId = sessionId;
  // `store.messages` is a computed over the controller's LRU map —
  // seed the map, not the computed. Two runs (u1 opens run 1, u2
  // opens run 2): exercises both spacing classes.
  useStreamControllerStore().messagesBySession.set(sessionId, [
    { id: "u1", role: "user", content: "第一条", seq: 1 } as never,
    { id: "a1", role: "assistant", content: "回答一", seq: 2 } as never,
    { id: "u2", role: "user", content: "第二条", seq: 3 } as never,
    { id: "a2", role: "assistant", content: "回答二", seq: 4 } as never,
  ]);
  return store;
}

describe("MessageList — virtualized render structure (N4 PR1)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockClear();
    document.body.innerHTML = "";
    calls.scrollToEnd.length = 0;
    calls.scrollToIndex.length = 0;
    calls.willUpdate = 0;
    calls.measure = 0;
  });

  it("renders a div list with absolute rows, data-index, translateY and measureElement wiring", async () => {
    seedSessionWithMessages("s1");
    const w = await mountList();

    // D5: the scroll container is a real DIV (ul retired with the
    // TransitionGroup).
    const list = w.get(".messages");
    expect(list.element.tagName).toBe("DIV");

    // Spacer carries the library's total height (fake: count × 60).
    const spacer = w.get(".messages-spacer");
    expect(spacer.element.tagName).toBe("DIV");
    expect((spacer.element as HTMLElement).style.height).toBe("240px");

    // Every flattened message is a .vrow (fake shows the full window)
    // with data-index + translateY positioning.
    const rows = w.findAll(".vrow");
    expect(rows).toHaveLength(4);
    expect(rows[0]!.attributes("data-index")).toBe("0");
    expect((rows[2]!.element as HTMLElement).style.transform).toBe(
      "translateY(120px)",
    );
    // measureElement ref callbacks ran for every row (library-owned
    // measurement — the wrapper is the measured element).
    expect(calls.measure).toBe(4);
    w.unmount();
  });

  it("spacing classes: index 0 exempt, group heads run-first, others run-rest", async () => {
    seedSessionWithMessages("s1");
    const w = await mountList();
    const rows = w.findAll(".vrow");
    // u1 opens run 1 at index 0 → NO padding class (D3 补强①: 判定用
    // index,不用组首标记)。
    expect(rows[0]!.classes()).not.toContain("run-first");
    expect(rows[0]!.classes()).not.toContain("run-rest");
    // a1 folds into run 1 → run-rest(6px)。
    expect(rows[1]!.classes()).toContain("run-rest");
    expect(rows[1]!.classes()).not.toContain("run-first");
    // u2 opens run 2 → run-first(12px)。
    expect(rows[2]!.classes()).toContain("run-first");
    expect(rows[3]!.classes()).toContain("run-rest");
    w.unmount();
  });

  it("MessageItem root is a DIV carrying the data-seq hook", async () => {
    seedSessionWithMessages("s1");
    const w = await mountList();
    const first = w.get(".msg");
    expect(first.element.tagName).toBe("DIV");
    // data-seq fallthrough (CH12-1b hook; PR3 flash will key off it).
    expect(w.find('.msg[data-seq="2"]').exists()).toBe(true);
    w.unmount();
  });
});

describe("MessageList — back-to-bottom button (isAtEnd driven)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockClear();
    document.body.innerHTML = "";
    calls.scrollToEnd.length = 0;
    calls.scrollToIndex.length = 0;
    calls.willUpdate = 0;
    calls.measure = 0;
  });

  it("hidden while at end; appears when the user scrolls away; click scrolls back (smooth while idle)", async () => {
    seedSessionWithMessages("s1");
    const w = await mountList();
    expect(w.find(".scroll-to-bottom").exists()).toBe(false);

    // User scrolls away: DOM distance-from-bottom input > 80 → button
    // appears (decision, not a scroll value).
    seedScrollInputs(w, 100);
    await nextTick();
    expect(w.find(".scroll-to-bottom").exists()).toBe(true);

    calls.scrollToEnd.length = 0;
    await w.get(".scroll-to-bottom").trigger("click");
    await flushPromises();
    // idle → smooth behavior (streaming path asserts behavior auto in
    // the force-follow describe below).
    expect(calls.scrollToEnd).toEqual([{ behavior: "smooth" }]);
    // jumpToBottom pre-emptively resets the flag → button hides at
    // once instead of waiting for the scroll to cross the threshold.
    expect(w.find(".scroll-to-bottom").exists()).toBe(false);
    w.unmount();
  });

  it("force-follow exit: scrolling away while force-follow clears the flag (store decision)", async () => {
    const store = seedSessionWithMessages("s1");
    store.forceFollowActive = true;
    const w = await mountList();

    seedScrollInputs(w, 100);
    await nextTick();
    expect(store.forceFollowActive).toBe(false);
    w.unmount();
  });

  it("near-bottom scroll does NOT clear force-follow (80px threshold)", async () => {
    const store = seedSessionWithMessages("s1");
    store.forceFollowActive = true;
    const w = await mountList();

    seedScrollInputs(w, 560); // 1000 - 560 - 400 = 40 < 80 → near
    await nextTick();
    expect(store.forceFollowActive).toBe(true);
    w.unmount();
  });
});

describe("MessageList — pending-interaction force scroll (CH8-2a)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockClear();
    document.body.innerHTML = "";
    calls.scrollToEnd.length = 0;
    calls.scrollToIndex.length = 0;
    calls.willUpdate = 0;
    calls.measure = 0;
  });

  it("pending appearing (null→some) force-scrolls to bottom (instant)", async () => {
    seedSessionWithMessages("s1");
    const w = await mountList();
    // Mount itself lands at the bottom first (single scrollToEnd —
    // stickToBottomUntilStable's replacement); drop it from the count.
    calls.scrollToEnd.length = 0;

    // A blocking question registers for the current session.
    useQuestionCardsStore().addPending("s1", makePending("s1", "toolu-1"));
    await flushPromises();

    // Decision assertion: ONE instant scroll-to-end. (The old test
    // pinned scrollTop=scrollHeight — a result assertion that jsdom
    // faked; design §7 retires it.)
    expect(calls.scrollToEnd).toEqual([{ behavior: "auto" }]);
    w.unmount();
  });

  it("some→some (pending replaced) does not re-trigger the forced scroll", async () => {
    seedSessionWithMessages("s1");
    useQuestionCardsStore().addPending("s1", makePending("s1", "toolu-1"));
    const w = await mountList();
    calls.scrollToEnd.length = 0;

    // A new pending object (backend overwrite semantics) — identity
    // changes but it is NOT a null→some transition: no forced scroll.
    useQuestionCardsStore().addPending("s1", makePending("s1", "toolu-2"));
    await flushPromises();

    expect(calls.scrollToEnd).toEqual([]);
    w.unmount();
  });

  it("some→null (pending resolved) does not force anything", async () => {
    seedSessionWithMessages("s1");
    useQuestionCardsStore().addPending("s1", makePending("s1", "toolu-1"));
    const w = await mountList();
    calls.scrollToEnd.length = 0;

    useQuestionCardsStore().removePending("s1");
    await flushPromises();

    expect(calls.scrollToEnd).toEqual([]);
    w.unmount();
  });
});

describe("MessageList — pendingScrollSeq command (AC4 前半, N4 PR1)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockClear();
    document.body.innerHTML = "";
    calls.scrollToEnd.length = 0;
    calls.scrollToIndex.length = 0;
    calls.willUpdate = 0;
    calls.measure = 0;
  });

  it("resolves the seq to scrollToIndex(align:center) on the flattened row and resets", async () => {
    const store = seedSessionWithMessages("s1");
    const w = await mountList();
    calls.scrollToIndex.length = 0;

    // seq 3 is the second run head → flattened index 2.
    store.pendingScrollSeq = 3;
    await flushPromises();

    expect(calls.scrollToIndex).toEqual([[2, { align: "center" }]]);
    // One-shot command: consumed → null so re-issuing the SAME seq
    // re-triggers.
    expect(store.pendingScrollSeq).toBeNull();
    w.unmount();
  });

  it("re-issuing the same seq triggers again (reset contract)", async () => {
    const store = seedSessionWithMessages("s1");
    const w = await mountList();

    store.pendingScrollSeq = 4;
    await flushPromises();
    store.pendingScrollSeq = 4;
    await flushPromises();

    expect(calls.scrollToIndex).toEqual([
      [3, { align: "center" }],
      [3, { align: "center" }],
    ]);
    w.unmount();
  });

  it("unknown seq: no scroll, command still consumed (no stale re-entry)", async () => {
    const store = seedSessionWithMessages("s1");
    const w = await mountList();
    calls.scrollToIndex.length = 0;

    store.pendingScrollSeq = 999;
    await flushPromises();

    expect(calls.scrollToIndex).toEqual([]);
    expect(store.pendingScrollSeq).toBeNull();
    w.unmount();
  });
});

// ---------------------------------------------------------------------------
// PR2(f4 降本):流式 delta 不触发 flatten 链重算的决策断言。
//
// PR1 形态 flatItems 依赖每条消息的全部可见性字段,delta 原位追加
// content 即 O(n) 全链重算(每 delta 主线程 ~20-25ms,f4@10k 859ms 主
// 构成)。PR2 缓存链成立的行为证据 = spy 重算计数:纯增长零重算,翻转
// 事件(可见性 false→true / 结构数组增长)恰好一次。
// ---------------------------------------------------------------------------
describe("MessageList — 流式 delta 不触发 flatten 重算(PR2)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockClear();
    document.body.innerHTML = "";
    calls.scrollToEnd.length = 0;
    calls.scrollToIndex.length = 0;
    calls.willUpdate = 0;
    calls.measure = 0;
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  function lastMsg(): { content: string } & Record<string, unknown> {
    const msgs = useStreamControllerStore().messagesBySession.get("s1")!;
    return msgs[msgs.length - 1] as never;
  }

  it("可见尾行原位追加 content(流式 delta)→ buildRunGroups/flatten 零重算", async () => {
    seedSessionWithMessages("s1");
    const w = await mountList();
    const flattenSpy = vi.spyOn(messageFormat, "flattenRunGroups");
    const groupsSpy = vi.spyOn(messageFormat, "buildRunGroups");

    // 尾行 a2 已可见(content 非空):20 个原位 delta 只增长文本,
    // 可见性与组结构均不变 → 缓存链必须完全静默。
    for (let i = 0; i < 20; i += 1) {
      lastMsg().content += ` delta-${i}`;
      await flushPromises();
    }

    expect(flattenSpy).not.toHaveBeenCalled();
    expect(groupsSpy).not.toHaveBeenCalled();
    // 渲染面同参:行数不变(4 条种子)。
    expect(w.findAll(".vrow")).toHaveLength(4);
    w.unmount();
  });

  it("可见性翻转(空占位首 delta)恰好重算一次;后续 delta 不再重算", async () => {
    seedSessionWithMessages("s1");
    const w = await mountList();
    const msgs = useStreamControllerStore().messagesBySession.get("s1")!;
    // 真实 send 路径同形态:占位行(空 content)不可见。
    msgs.push({ id: "a-x", role: "assistant", content: "", seq: 9 } as never);
    await flushPromises();

    const flattenSpy = vi.spyOn(messageFormat, "flattenRunGroups");
    // 首 delta:content 空 → 非空,可见性翻转 → 恰一次重算。
    lastMsg().content += "首段";
    await flushPromises();
    expect(flattenSpy).toHaveBeenCalledTimes(1);
    // 占位行此刻入列表(5 行)。
    expect(w.findAll(".vrow")).toHaveLength(5);

    // 后续 delta:纯增长,零重算。
    for (let i = 0; i < 5; i += 1) {
      lastMsg().content += ` 续${i}`;
      await flushPromises();
    }
    expect(flattenSpy).toHaveBeenCalledTimes(1);
    w.unmount();
  });

  it("结构数组增长(尾行 toolCalls push)触发重判 —— 工具卡必须入列表", async () => {
    seedSessionWithMessages("s1");
    const w = await mountList();
    const msgs = useStreamControllerStore().messagesBySession.get("s1")!;
    const flattenSpy = vi.spyOn(messageFormat, "flattenRunGroups");

    (lastMsg() as { toolCalls?: unknown[] }).toolCalls = [
      { id: "tu-1", name: "read_file", input: { path: "a.rs" } },
    ];
    await flushPromises();

    // 重算发生(单调缓存对「同对象结构翻转」的重判入口 = tailSig)。
    expect(flattenSpy).toHaveBeenCalledTimes(1);
    // 行数不变(同一条消息),断言经 store 面确认消息仍在列表。
    expect(msgs).toHaveLength(4);
    expect(w.findAll(".vrow")).toHaveLength(4);
    w.unmount();
  });
});

describe("MessageList — handwritten force-follow (spike conclusion 2)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockClear();
    document.body.innerHTML = "";
    calls.scrollToEnd.length = 0;
    calls.scrollToIndex.length = 0;
    calls.willUpdate = 0;
    calls.measure = 0;
  });

  function seedStreaming(controller: ReturnType<typeof useStreamControllerStore>, sessionId: string): void {
    // streamingSessionIds 是 activeRequests 的 computed —— 经注册
    // request 声明流式态(chatSend.test.ts 先例)。
    controller.activeRequests.set("rid-test-1", {
      requestId: "rid-test-1",
      sessionId,
      projectId: "p1",
      userMsgId: "u-x",
      assistantMsgId: "a-x",
      groupChat: false,
      groupChatStarted: false,
      pendingSpeaker: null,
      terminalError: null,
      history: [],
    } as never);
  }

  it("streaming + force + append → unconditional scrollToEnd (auto)", async () => {
    const store = seedSessionWithMessages("s1");
    seedStreaming(useStreamControllerStore(), "s1");
    store.forceFollowActive = true;
    const w = await mountList();
    calls.scrollToEnd.length = 0;

    // Append a new message (new assistant turn mid-stream).
    useStreamControllerStore().messagesBySession.get("s1")!.push({
      id: "a3",
      role: "assistant",
      content: "流式增量",
      seq: 5,
    } as never);
    await flushPromises();

    // Force-follow is OUR path (library `true` ≡ 'auto' — it cannot
    // force): unconditional, bypassing the isAtEnd gate.
    expect(calls.scrollToEnd).toEqual([{ behavior: "auto" }]);
    w.unmount();
  });

  it("streaming + append WITHOUT force → our handwritten path stays silent", async () => {
    seedSessionWithMessages("s1");
    seedStreaming(useStreamControllerStore(), "s1");
    const w = await mountList();
    calls.scrollToEnd.length = 0;

    useStreamControllerStore().messagesBySession.get("s1")!.push({
      id: "a3",
      role: "assistant",
      content: "流式增量",
      seq: 5,
    } as never);
    await flushPromises();

    // The library's followOnAppend('auto') owns the at-end append case
    // inside core (not observable through the fake); this asserts our
    // handwritten path does NOT double-drive.
    expect(calls.scrollToEnd).toEqual([]);
    w.unmount();
  });

  it("force append while NOT streaming → no unconditional follow", async () => {
    const store = seedSessionWithMessages("s1");
    store.forceFollowActive = true;
    const w = await mountList();
    calls.scrollToEnd.length = 0;

    useStreamControllerStore().messagesBySession.get("s1")!.push({
      id: "u3",
      role: "user",
      content: "排队刷新",
      seq: 5,
    } as never);
    await flushPromises();

    expect(calls.scrollToEnd).toEqual([]);
    w.unmount();
  });
});

// ---------------------------------------------------------------------------
// N4 PR3:flash 高亮(AC4 后半)+ D4 新 run 划入(design §4)。断言全部
// 落在**类挂载/移除决策**(jsdom 无布局,视觉结果断言不在此层;真浏览器
// 的命中定位/类存在由 e2e virtualized-list.spec 覆盖)。白名单约束
// (只许 opacity/translateX)由 CSS 审查守住,测试锁相位时序。
// ---------------------------------------------------------------------------
describe("MessageList — PR3 flash + D4 run-enter", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockClear();
    document.body.innerHTML = "";
    calls.scrollToEnd.length = 0;
    calls.scrollToIndex.length = 0;
    calls.willUpdate = 0;
    calls.measure = 0;
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  it("flash: consuming a known seq flags the hit wrapper (search-hit) and clears it after the window", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    const store = seedSessionWithMessages("s1");
    const w = await mountList();

    store.pendingScrollSeq = 3;
    await flushPromises();

    // 类落在**wrapper**(.vrow)上:seq 3 = u2 = 打平 index 2;其余行不沾。
    const rows = w.findAll(".vrow");
    expect(rows[2]!.classes()).toContain("search-hit");
    expect(rows[0]!.classes()).not.toContain("search-hit");
    expect(rows[3]!.classes()).not.toContain("search-hit");
    // 同一 wrapper 不该挂 enter 类(flash 与 enter 是两套状态)。
    expect(rows[2]!.classes()).not.toContain("run-enter-from");

    await vi.advanceTimersByTimeAsync(SEARCH_FLASH_MS + 100);
    await nextTick();
    expect(w.findAll(".vrow.search-hit")).toHaveLength(0);

    // 动画结束摘类后,重复下令可重新点亮(与 reset 契约配套)。
    store.pendingScrollSeq = 3;
    await flushPromises();
    expect(w.findAll(".vrow.search-hit")).toHaveLength(1);
    w.unmount();
  });

  it("enter: new run appended at end + visible mounts from+active, double-rAF releases to active, window clears", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    const rafQueue: FrameRequestCallback[] = [];
    vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
      rafQueue.push(cb);
      return rafQueue.length;
    });
    const flushRaf = (): void => {
      while (rafQueue.length > 0) {
        for (const cb of rafQueue.splice(0)) cb(0);
      }
    };

    seedSessionWithMessages("s1");
    const w = await mountList();
    // 首次触发 = 基线(整体替换守卫),mount 不动画。
    expect(w.findAll(".run-enter-from")).toHaveLength(0);

    // 新用户消息 = 新 run 组首,append 于末端且在(假)渲染窗口内。
    useStreamControllerStore().messagesBySession.get("s1")!.push({
      id: "u3",
      role: "user",
      content: "新 run 组首",
      seq: 5,
    } as never);
    await flushPromises();

    const rows = w.findAll(".vrow");
    const last = rows[rows.length - 1]!;
    expect(last.classes()).toContain("run-first");
    // from 相位:from+active 同挂(与 Vue TransitionGroup enter 同式)。
    expect(last.classes()).toContain("run-enter-from");
    expect(last.classes()).toContain("run-enter-active");

    // 双 rAF → from 摘除(active 过渡窗保留)。
    flushRaf();
    await nextTick();
    expect(last.classes()).not.toContain("run-enter-from");
    expect(last.classes()).toContain("run-enter-active");

    // 过渡窗结束 → 全摘。
    await vi.advanceTimersByTimeAsync(RUN_ENTER_ACTIVE_MS + 100);
    await nextTick();
    expect(last.classes()).not.toContain("run-enter-from");
    expect(last.classes()).not.toContain("run-enter-active");
    w.unmount();
  });

  it("enter negative: appended assistant turn folds into the existing run — no animation", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    seedSessionWithMessages("s1");
    const w = await mountList();

    useStreamControllerStore().messagesBySession.get("s1")!.push({
      id: "a3",
      role: "assistant",
      content: "流式续写",
      seq: 5,
    } as never);
    await flushPromises();

    const rows = w.findAll(".vrow");
    expect(rows).toHaveLength(5);
    const last = rows[rows.length - 1]!;
    // 归入已有 run(run-rest)——「流式中追加的 assistant turn 不动画」。
    expect(last.classes()).toContain("run-rest");
    expect(last.classes()).not.toContain("run-enter-from");
    expect(last.classes()).not.toContain("run-enter-active");
    w.unmount();
  });

  it("enter negative: whole-array replacement (session switch / reload) never animates", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    seedSessionWithMessages("s1");
    const w = await mountList();

    // 整体替换且**变长**(store.messages 换引用):切换/reload 语义,
    // 进场由容器 fade-in 承担,不逐行动画。
    useStreamControllerStore().messagesBySession.set("s1", [
      { id: "r1", role: "user", content: "重载一", seq: 1 } as never,
      { id: "r2", role: "assistant", content: "重载二", seq: 2 } as never,
      { id: "r3", role: "user", content: "重载三", seq: 3 } as never,
      { id: "r4", role: "assistant", content: "重载四", seq: 4 } as never,
      { id: "r5", role: "user", content: "重载五", seq: 5 } as never,
    ]);
    await flushPromises();

    expect(w.findAll(".vrow")).toHaveLength(5);
    expect(w.findAll(".run-enter-from")).toHaveLength(0);
    expect(w.findAll(".run-enter-active")).toHaveLength(0);
    w.unmount();
  });
});
