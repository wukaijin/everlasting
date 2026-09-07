// C1.2 token 预算硬停(09-08-gc-c1-stoploss)—— streamEvents 两处
// isTerminal 白名单都认 `stop_reason: "budget"`:
//   · 头部早判(handleChatEvent ~151):终态事件触发收官通知路由;
//   · done 处理器(~644):finalizeRequest 把 rid 从 activeRequests
//     摘除(漏了 = 请求悬挂、后续事件全丢)。
// 镜像 scheduledDiscussionToast.test.ts 的脚手架(transport mock +
// 直插 sessions + handleChatEvent 驱动)。非定时场(metadata 兜底
// 不命中)→ 走通用收官 toast。
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";

const invokeMock: any = vi.fn();
invokeMock.mockImplementation(async () => null);
vi.mock("../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

import { useChatStore } from "./chat";
import type { SessionSummary } from "./chat.types";
import { useProjectsStore } from "./projects";
import { useStreamControllerStore } from "./streamController";

function mkSummary(id: string, opts?: { busy?: boolean }): SessionSummary {
  return {
    id,
    title: "讨论场",
    updated_at: "2026-09-08T00:00:00Z",
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
    workflow_enabled: false,
    plugin_name: "dev",
    session_type: "group_chat",
    metadata: { participants: [] },
    busy: opts?.busy ?? false,
  } as unknown as SessionSummary;
}

describe("C1.2 — stop_reason=budget 是终态(两处白名单)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockClear();
    useProjectsStore().toast = null;
  });

  function handle(
    stream: ReturnType<typeof useStreamControllerStore>,
    event: Record<string, unknown>,
  ) {
    (stream as unknown as { handleChatEvent: (e: Record<string, unknown>) => void }).handleChatEvent(
      event,
    );
  }

  function setup() {
    const stream = useStreamControllerStore();
    const chatStore = useChatStore();
    chatStore.currentSessionId = "s-cur";
    chatStore.sessions = [mkSummary("s-gc", { busy: true })];
    /** 注入的 start 事件经 unknown-request 认领路径建条目,groupChat
     *  恒 false(streamEvents:114);真实发送路径由 startStream args
     *  带真值。测试直改内部条目,使群聊白名单分支真正被走到。 */
    const markGroupChat = (rid: string) => {
      handle(stream, { request_id: rid, session_id: "s-gc", kind: "start" });
      const req = (stream as unknown as { activeRequests: Map<string, { groupChat: boolean }> })
        .activeRequests.get(rid);
      if (req) req.groupChat = true;
    };
    return { stream, markGroupChat };
  }

  it("done(budget) → 请求 finalize(rid 摘除)+ 收官 toast", () => {
    const { stream, markGroupChat } = setup();
    markGroupChat("rid-b1");
    handle(stream, { request_id: "rid-b1", session_id: "s-gc", kind: "delta", text: "讨论中" });
    expect((stream as unknown as { activeRequests: Map<string, unknown> }).activeRequests.has("rid-b1")).toBe(true);

    handle(stream, {
      request_id: "rid-b1",
      session_id: "s-gc",
      kind: "done",
      stop_reason: "budget",
    });

    // done 处理器的 isTerminal(第二处白名单):finalize 执行。
    expect(
      (stream as unknown as { activeRequests: Map<string, unknown> }).activeRequests.has("rid-b1"),
    ).toBe(false);
    // 头部早判(第一处白名单):终态通知路由走了(非定时场 → 通用 toast)。
    expect(useProjectsStore().toast).not.toBeNull();
  });

  it("done(end_turn) 等非终态群聊轮不 finalize(对照组)", () => {
    const { stream, markGroupChat } = setup();
    markGroupChat("rid-b2");
    handle(stream, { request_id: "rid-b2", session_id: "s-gc", kind: "delta", text: "x" });
    handle(stream, {
      request_id: "rid-b2",
      session_id: "s-gc",
      kind: "done",
      stop_reason: "end_turn",
    });
    expect(
      (stream as unknown as { activeRequests: Map<string, unknown> }).activeRequests.has("rid-b2"),
    ).toBe(true);
  });
});
