// F6 异步 agent 任务(2026-08-27)— 轮次终结跨 session toast + busy 消解。
//
// 第一组:`buildTurnFinishedNotification` 纯函数(镜像
// `pendingNotification.test.ts` 的 Q3 门:当前 session 不弹)。
// 第二组:store 集成 —— streamEvents.handleChatEvent 的终结挂点
// (foreign done/error → toast;cancelled / 群聊中间轮 / 开关关 → 不弹)
// + finalize 公共出口清 chatStore.sessions 的 busy(serverBusy 消解,
// 评审 P3-2:不依赖 adoptForeignRequest 认领分支)。
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";

// Transport mock:finalize 的 reloadAfterFinalize 会 fire
// load_session + update_message_latency,统一回 null/[] 即可。
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
import { useConfigStore } from "./config";
import { useProjectsStore } from "./projects";
import { useStreamControllerStore } from "./streamController";
import { buildTurnFinishedNotification } from "./streamController";
import type { ChatMessage } from "./chat.types";

function mkSummary(id: string, title: string, busy?: boolean): SessionSummary {
  return {
    id,
    title,
    updated_at: "2026-08-27T00:00:00Z",
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
    session_type: "chat",
    metadata: null,
    busy,
  };
}

describe("buildTurnFinishedNotification(纯函数)", () => {
  const sessions = [
    { id: "s1", title: "前台会话" },
    { id: "s2", title: "后台会话" },
  ];

  it("当前 session 终结 → null(气泡就在眼前,不打扰)", () => {
    expect(buildTurnFinishedNotification("s1", "done", "s1", sessions)).toBeNull();
    expect(buildTurnFinishedNotification("s1", "error", "s1", sessions)).toBeNull();
  });

  it("另一 session done → 「已完成」文案 + 标题 + sessionId", () => {
    const n = buildTurnFinishedNotification("s2", "done", "s1", sessions);
    expect(n).not.toBeNull();
    expect(n!.sessionId).toBe("s2");
    expect(n!.message).toContain("后台会话");
    expect(n!.message).toContain("已完成");
  });

  it("另一 session error → 错误语义文案", () => {
    const n = buildTurnFinishedNotification("s2", "error", "s1", sessions);
    expect(n).not.toBeNull();
    expect(n!.message).toContain("出错");
  });

  it("跨 project(session 不在列表)→ 「另一项目的会话」降级", () => {
    const n = buildTurnFinishedNotification("sX", "done", "s1", sessions);
    expect(n).not.toBeNull();
    expect(n!.message).toContain("另一项目的会话");
  });
});

describe("F6 — 终结 toast + busy 消解(streamEvents 集成)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockClear();
    useProjectsStore().toast = null;
  });

  function handle(
    stream: ReturnType<typeof useStreamControllerStore>,
    event: Record<string, unknown>,
  ) {
    (stream as unknown as { handleChatEvent: (e: Record<string, unknown>) => void }).handleChatEvent(event);
  }

  /** 场景脚手架:当前 session = s-cur,后台 session = s-bg(侧栏
   * busy=true,来自 list_sessions);后台 session 已加载历史且尾部是
   * assistant(认领后 done 走正常渲染路径)。 */
  function setup() {
    const stream = useStreamControllerStore();
    const chatStore = useChatStore();
    chatStore.currentSessionId = "s-cur";
    chatStore.sessions = [mkSummary("s-cur", "前台会话"), mkSummary("s-bg", "后台任务", true)];
    const msgs: ChatMessage[] = [
      { id: "u0", role: "user", content: "大任务" },
      { id: "a0", role: "assistant", content: "" },
    ];
    stream.putMessages("s-bg", msgs, false);
    return { stream, chatStore };
  }

  it("foreign done → toast(标题 + sessionId 附着)+ busy 翻回 false", () => {
    const { stream, chatStore } = setup();
    handle(stream, { request_id: "rid-a", session_id: "s-bg", kind: "delta", text: "干活中" });
    handle(stream, { request_id: "rid-a", session_id: "s-bg", kind: "done", stop_reason: "end_turn" });

    const t = useProjectsStore().toast;
    expect(t?.message).toContain("后台任务");
    expect(t?.message).toContain("已完成");
    expect(t?.sessionId).toBe("s-bg");
    // finalize 公共出口消解 busy(serverBusy)。
    expect(chatStore.sessions.find((s) => s.id === "s-bg")?.busy).toBe(false);
  });

  it("foreign error → toast 错误语义文案", () => {
    const { stream } = setup();
    handle(stream, { request_id: "rid-e", session_id: "s-bg", kind: "delta", text: "x" });
    handle(stream, { request_id: "rid-e", session_id: "s-bg", kind: "error", message: "boom", category: "server" });

    const t = useProjectsStore().toast;
    expect(t?.message).toContain("出错");
    expect(t?.sessionId).toBe("s-bg");
  });

  it("当前 session 终结 → 不弹(抑制)", () => {
    const { stream } = setup();
    handle(stream, { request_id: "rid-c", session_id: "s-cur", kind: "delta", text: "x" });
    handle(stream, { request_id: "rid-c", session_id: "s-cur", kind: "done" });
    expect(useProjectsStore().toast).toBeNull();
  });

  it("stop_reason=cancelled → 不弹(用户主动停止不是「完成」)", () => {
    const { stream } = setup();
    handle(stream, { request_id: "rid-k", session_id: "s-bg", kind: "delta", text: "x" });
    handle(stream, { request_id: "rid-k", session_id: "s-bg", kind: "done", stop_reason: "cancelled" });
    expect(useProjectsStore().toast).toBeNull();
    // cancelled 仍是终结:busy 照样消解。
    const { chatStore } = { chatStore: useChatStore() };
    expect(chatStore.sessions.find((s) => s.id === "s-bg")?.busy).toBe(false);
  });

  it("开关关(turnCompleteNotify=false)→ 不弹", () => {
    useConfigStore().turnCompleteNotify = false;
    const { stream } = setup();
    handle(stream, { request_id: "rid-g", session_id: "s-bg", kind: "delta", text: "x" });
    handle(stream, { request_id: "rid-g", session_id: "s-bg", kind: "done" });
    expect(useProjectsStore().toast).toBeNull();
  });

  it("群聊中间轮 done(非终结 stop_reason)→ 不弹", () => {
    const stream = useStreamControllerStore();
    const chatStore = useChatStore();
    chatStore.currentSessionId = "s-cur";
    chatStore.sessions = [mkSummary("s-gc", "群聊", true)];
    // 群聊请求:手动注册(认领路径恒 groupChat=false),镜像
    // messageQueueStream.test 的 activeRequests 直插方式。
    (stream as unknown as {
      activeRequests: Map<string, Record<string, unknown>>;
    }).activeRequests.set("rid-gc", {
      requestId: "rid-gc",
      sessionId: "s-gc",
      projectId: "",
      userMsgId: "",
      assistantMsgId: "",
      groupChat: true,
      groupChatStarted: true,
      pendingSpeaker: null,
      history: [],
      sendAt: 0,
      firstDeltaAt: null,
      toolStartedAt: new Map<string, number>(),
      currentTurnIndex: 0,
      latencyByTurn: new Map<number, unknown>(),
      pendingTimelineText: null,
    });
    const msgs: ChatMessage[] = [
      { id: "u0", role: "user", content: "讨论" },
      { id: "a0", role: "assistant", content: "" },
    ];
    stream.putMessages("s-gc", msgs, false);

    // 逐轮 done(无 orchestrator 边界 stop_reason)→ 非终结,不弹、
    // 不消解 busy。
    handle(stream, { request_id: "rid-gc", session_id: "s-gc", kind: "done" });
    expect(useProjectsStore().toast).toBeNull();
    expect(chatStore.sessions.find((s) => s.id === "s-gc")?.busy).toBe(true);

    // group_chat_end(真终结)→ 弹 + 消解。
    handle(stream, { request_id: "rid-gc", session_id: "s-gc", kind: "done", stop_reason: "group_chat_end" });
    expect(useProjectsStore().toast?.sessionId).toBe("s-gc");
    expect(chatStore.sessions.find((s) => s.id === "s-gc")?.busy).toBe(false);
  });
});
