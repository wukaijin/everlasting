// F1 消息队列 (2026-08-25) — 续轮渲染 + 占位视图生命周期定点用例。
//
// 第一组(implement.md #15)回归锁 review-glm P1-2:
// `streamEvents.handleChatEvent` 开头的"尾部非 assistant 即丢弃"守卫
// (:66-67)曾会吞掉续轮的全部事件(尾部是排队 user 占位)。
// `TurnContinuation` handler 挂在守卫之前,物化占位并推新 assistant
// 气泡 —— 锁定该契约:
// 1. `turn_continuation` 到达时尾部排队 user 占位被物化(去徽标);
// 2. 推新的 assistant 占位成为事件流新落点(subsequent delta 不丢);
// 3. 队列视图同步 shiftFront。
//
// 第二组(评审 Round 2 P1/P2 修复)锁定:
// 4. `dropQueuedPlaceholder` 删除撤销/退回成功的占位并重排位次,
//    拒绝删非排队行;
// 5. `materializeQueuedPlaceholders` 水合后物化占位(按 queued.id
//    去重),恢复刷新/驱逐/第二端后的可见性;
// 6. R8 撤销/退回按 uuid 寻址直达 IPC(不再 position 推数组下标)。
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";

// Transport mock —— store 的 revoke/recall/hydrate 走 `transport.invoke`;
// 按 command 名分流,断言用 invokeMock。
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const invokeMock: any = vi.fn();
invokeMock.mockImplementation(async (cmd: string) => {
  if (cmd === "list_queued_messages") return [];
  if (cmd === "recall_queued_message") {
    return { id: "id-x", message: { content: "recalled text" }, enqueued_at: 0 };
  }
  return null;
});
vi.mock("../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

import { useMessageQueueStore } from "./messageQueueStore";
import { useStreamControllerStore } from "./streamController";
import type { ChatMessage } from "./chat.types";

function usr(seq: number, text: string): ChatMessage {
  return { id: `u${seq}`, seq, role: "user", content: text };
}
function asst(seq: number, text: string): ChatMessage {
  return { id: `a${seq}`, seq, role: "assistant", content: text };
}
// 注:不需要 rehydrateMessages —— 本文件只关心视图态数组形态。

describe("F1 message queue — TurnContinuation rendering boundary", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockClear();
  });

  function setup(tail: ChatMessage[]) {
    const stream = useStreamControllerStore();
    const sid = "f1-continuation-sid";
    const req = {
      requestId: "rid-f1-cont",
      sessionId: sid,
      projectId: null,
      userMsgId: "u0",
      assistantMsgId: "a-live",
      history: [],
      sendAt: 0,
      firstDeltaAt: null,
      toolStartedAt: new Map<string, number>(),
      currentTurnIndex: -1,
      latencyByTurn: new Map<string, number>(),
    };
    (stream as unknown as { activeRequests: Map<string, typeof req> })
      .activeRequests.set(req.requestId, req);
    // 历史两行 + 尾部"上一轮的 assistant 气泡 + 排队 user 占位"
    // —— 真实续轮形态:第一轮已产出回复,用户连发两条在队。
    const msgs: ChatMessage[] = [usr(0, "go"), asst(1, "first reply"), ...tail];
    stream.putMessages(sid, msgs, false);
    return { stream, sid, msgs, req };
  }

  function handle(stream: ReturnType<typeof useStreamControllerStore>, event: Record<string, unknown>) {
    (
      stream as unknown as {
        handleChatEvent: (e: Record<string, unknown>) => void;
      }
    ).handleChatEvent(event);
  }

  it("materializes queued placeholders and opens a new assistant bubble", () => {
    const q1: ChatMessage = { id: "q1", role: "user", content: "queued one", queued: { id: "id-1", position: 1 } };
    const q2: ChatMessage = { id: "q2", role: "user", content: "queued two", queued: { id: "id-2", position: 2 } };
    const { stream, sid, msgs } = setup([asst(9, ""), q1, q2]);

    // 队列视图预置两条(与占位对齐)。
    const queue = useMessageQueueStore();
    queue.queuedBySession.set(sid, [
      { id: "id-1", text: "queued one", position: 1 },
      { id: "id-2", text: "queued two", position: 2 },
    ]);

    // 后端本轮注入 count=1(只消费队首一条 —— 上轮结束后的正常续轮)。
    handle(stream, { request_id: "rid-f1-cont", kind: "turn_continuation", count: 1 });

    // ① q1 被物化(去徽标),q2 保持排队;
    expect(q1.queued).toBeUndefined();
    expect(q2.queued).toBeDefined();
    // ② 新 assistant 占位成为尾部 → :66-67 守卫恢复,后续事件不再被丢;
    const last = msgs[msgs.length - 1];
    expect(last.role).toBe("assistant");
    expect(last.content).toBe("");
    // ③ 队列视图同步弹走 1 条。
    expect(queue.entriesFor(sid).map((e) => e.id)).toEqual(["id-2"]);

    // 续轮的 start/delta 落在新气泡上(不被 :67 吞掉)。
    handle(stream, { request_id: "rid-f1-cont", kind: "start" });
    handle(stream, { request_id: "rid-f1-cont", kind: "delta", text: "continuation!" });
    expect((msgs[msgs.length - 1] as ChatMessage).content).toBe("continuation!");
  });

  it("keeps deltas flowing when the tail is a queued user placeholder (P1-2 regression)", () => {
    // 极端形态:没有上一轮 assistant 气泡(理论不该出现,防御性),
    // 全尾是排队占位 —— handler 必须仍能恢复不变量。
    const q1: ChatMessage = { id: "q1", role: "user", content: "only", queued: { id: "id-1", position: 1 } };
    const { stream, msgs } = setup([q1]);
    handle(stream, { request_id: "rid-f1-cont", kind: "turn_continuation", count: 1 });
    expect(q1.queued).toBeUndefined();
    expect(msgs[msgs.length - 1].role).toBe("assistant");
    handle(stream, { request_id: "rid-f1-cont", kind: "start" });
    handle(stream, { request_id: "rid-f1-cont", kind: "delta", text: "x" });
    expect((msgs[msgs.length - 1] as ChatMessage).content).toBe("x");
  });
});

describe("F1 message queue — placeholder view lifecycle (Round 2 fixes)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockClear();
  });

  function seedStream(sid: string, msgs: ChatMessage[]) {
    const stream = useStreamControllerStore();
    stream.putMessages(sid, msgs, false);
    return stream;
  }

  it("dropQueuedPlaceholder removes the queued row and renumbers siblings", () => {
    const stream = seedStream("s1", [
      usr(0, "go"),
      asst(1, "reply"),
      { id: "qa", role: "user", content: "A", queued: { id: "id-a", position: 1 } },
      { id: "qb", role: "user", content: "B", queued: { id: "id-b", position: 2 } },
    ]);

    stream.dropQueuedPlaceholder("s1", "qa");

    const msgs = stream.getMessages("s1")!;
    expect(msgs.map((m) => m.id)).toEqual(["u0", "a1", "qb"]);
    // 剩余占位位次重排(撤销后徽标不显示陈旧位次)。
    expect(msgs[2]!.queued).toEqual({ id: "id-b", position: 1 });
  });

  it("dropQueuedPlaceholder refuses non-queued rows (persisted messages stay)", () => {
    const stream = seedStream("s1", [usr(0, "persisted")]);
    stream.dropQueuedPlaceholder("s1", "u0");
    expect(stream.getMessages("s1")!.map((m) => m.id)).toEqual(["u0"]);
  });

  it("materializeQueuedPlaceholders restores visibility after refresh, dedupes by id", () => {
    // 刷新形态:DB 重建的普通历史 + 一条仍在队的占位(id-b,发送路径
    // 的占位已随内存丢失)。水合 entries 两条。
    const stream = seedStream("s1", [
      usr(0, "go"),
      asst(1, "reply"),
      { id: "qb", role: "user", content: "B", queued: { id: "id-b", position: 1 } },
    ]);

    stream.materializeQueuedPlaceholders("s1", [
      { id: "id-a", text: "A" },
      { id: "id-b", text: "B" },
    ]);

    const msgs = stream.getMessages("s1")!;
    // id-b 已有占位(去重不重复物化);id-a 新物化 append 到尾部。
    const queued = msgs.filter((m) => m.queued);
    expect(queued.map((m) => m.queued!.id)).toEqual(["id-b", "id-a"]);
    // 位次按数组序重排。
    expect(queued.map((m) => m.queued!.position)).toEqual([1, 2]);
    // 非排队行不动。
    expect(msgs[0]!.id).toBe("u0");
    expect(msgs[1]!.id).toBe("a1");
  });

  it("revoke addresses the backend by uuid, not array position", async () => {
    const queue = useMessageQueueStore();
    queue.queuedBySession.set("s1", [
      { id: "id-a", text: "A", position: 1 },
      { id: "id-b", text: "B", position: 2 },
    ]);

    const ok = await queue.revoke("s1", "id-a");

    expect(ok).toBe(true);
    expect(invokeMock).toHaveBeenCalledWith("remove_queued_message", {
      sessionId: "s1",
      id: "id-a",
    });
    // 视图同步移除 + 重排。
    expect(queue.entriesFor("s1")).toEqual([
      { id: "id-b", text: "B", position: 1 },
    ]);
  });

  it("recallToComposer fetches the original text and publishes a draft", async () => {
    const queue = useMessageQueueStore();
    queue.queuedBySession.set("s1", [{ id: "id-a", text: "A", position: 1 }]);

    const ok = await queue.recallToComposer("s1", "id-a");

    expect(ok).toBe(true);
    expect(invokeMock).toHaveBeenCalledWith("recall_queued_message", {
      sessionId: "s1",
      id: "id-a",
    });
    // 草稿携带后端返回的原文(不是视图里的预览文本),供 ChatInput
    // watch recallDraft 立即回填当前 session。
    expect(queue.recallDraft).toEqual({ sessionId: "s1", text: "recalled text" });
    expect(queue.entriesFor("s1")).toEqual([]);
  });

  it("takeRecallDraft consumes once, only for the matching session", () => {
    const queue = useMessageQueueStore();
    queue.recallDraft = { sessionId: "s1", text: "draft" };

    expect(queue.takeRecallDraft("s2")).toBeNull();
    expect(queue.takeRecallDraft("s1")).toBe("draft");
    expect(queue.takeRecallDraft("s1")).toBeNull();
  });
});
