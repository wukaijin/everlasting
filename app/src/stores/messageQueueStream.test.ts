// F1 消息队列 (2026-08-25) — 续轮渲染定点用例(implement.md #15)。
//
// 回归锁 review-glm P1-2:`streamEvents.handleChatEvent` 开头的
// "尾部非 assistant 即丢弃"守卫(:66-67)曾会吞掉续轮的全部事件
// (尾部是排队 user 占位)。`TurnContinuation` handler 挂在守卫之前,
// 物化占位并推新 assistant 气泡 —— 本文件锁定该契约:
// 1. `turn_continuation` 到达时尾部排队 user 占位被物化(去徽标);
// 2. 推新的 assistant 占位成为事件流新落点(subsequent delta 不丢);
// 3. 队列视图同步 shiftFront。
import { beforeEach, describe, expect, it } from "vitest";
import { createPinia, setActivePinia } from "pinia";

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
      latencyByTurn: new Map(),
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
    const q1: ChatMessage = { id: "q1", role: "user", content: "queued one", queued: { position: 1 } };
    const q2: ChatMessage = { id: "q2", role: "user", content: "queued two", queued: { position: 2 } };
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
    const q1: ChatMessage = { id: "q1", role: "user", content: "only", queued: { position: 1 } };
    const { stream, msgs } = setup([q1]);
    handle(stream, { request_id: "rid-f1-cont", kind: "turn_continuation", count: 1 });
    expect(q1.queued).toBeUndefined();
    expect(msgs[msgs.length - 1].role).toBe("assistant");
    handle(stream, { request_id: "rid-f1-cont", kind: "start" });
    handle(stream, { request_id: "rid-f1-cont", kind: "delta", text: "x" });
    expect((msgs[msgs.length - 1] as ChatMessage).content).toBe("x");
  });
});
