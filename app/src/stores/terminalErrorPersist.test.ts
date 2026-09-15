// 09-15-n1(daemon E2E 实证):error 终态的 `last.error` 会被
// finalizeRequest → reloadAfterFinalize 的 DB 权威替换冲掉(DB 只存
// 「[生成出错中断]」占位文本,不存错误态)→ 错误行(retry /「测试连接」)
// 只闪现一瞬。修复:RequestState.terminalError 暂存 + reloadAfterFinalize
// 挂回重载后的最后一条 assistant 行。本测试钉住该通路。
// 脚手架镜像 budgetStopStream.test.ts(transport mock + 直插 sessions +
// handleChatEvent 驱动)。
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";

const invokeMock: any = vi.fn();
vi.mock("../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

import { useChatStore } from "./chat";
import type { ChatMessage, SessionSummary } from "./chat.types";
import { useProjectsStore } from "./projects";
import { useStreamControllerStore } from "./streamController";

function mkSummary(id: string): SessionSummary {
  return {
    id,
    title: "会话",
    updated_at: "2026-09-15T00:00:00Z",
    preview: "",
    project_id: "p1",
    current_cwd: "/tmp",
    worktree_path: null,
    worktree_state: "none",
    last_worktree_path: null,
    model_id: "m1",
    input_tokens_total: null,
    output_tokens_total: null,
    cache_creation_total: null,
    cache_read_total: null,
    last_context_input_tokens: null,
    last_input_tokens: null,
    last_output_tokens: null,
    last_cache_creation_total: null,
    last_cache_read_total: null,
    color_tag: null,
    mode: "edit",
    workflow_enabled: false,
    plugin_name: null,
    session_type: "classic",
    metadata: null,
    busy: true,
  } as unknown as SessionSummary;
}

describe("error 终态在 reloadAfterFinalize 后存续(terminalError 挂回)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
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
    chatStore.currentSessionId = "s-1";
    chatStore.sessions = [mkSummary("s-1")];
    // 会话缓冲已有 user 行(start 认领时在其后建 assistant 占位)。
    const userMsg = { id: "u1", seq: 0, role: "user", content: "你好" } as unknown as ChatMessage;
    (stream as unknown as {
      putMessages: (sid: string, msgs: ChatMessage[], pinned: boolean) => void;
    }).putMessages("s-1", [userMsg], false);
    // reloadAfterFinalize 的权威重拉:assistant 行只有占位文本,无错误态。
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "load_session") {
        return {
          session: { id: "s-1", session_type: "classic", stop_reason: null },
          messages: [
            { id: 1, session_id: "s-1", seq: 0, role: "user", content: [{ type: "text", text: "你好" }] },
            {
              id: 2,
              session_id: "s-1",
              seq: 1,
              role: "assistant",
              content: [{ type: "text", text: "[生成出错中断]" }],
            },
          ],
        };
      }
      return null;
    });
    return { stream, chatStore };
  }

  it("error → finalize+DB 重载后,错误仍挂在最后 assistant 行", async () => {
    const { stream } = setup();
    handle(stream, { request_id: "rid-e1", session_id: "s-1", kind: "start" });
    handle(stream, { request_id: "rid-e1", session_id: "s-1", kind: "delta", text: "部分" });
    handle(stream, {
      request_id: "rid-e1",
      session_id: "s-1",
      kind: "error",
      message: "服务器错误 (HTTP 502)",
      category: "server",
    });

    // reloadAfterFinalize 是 fire-and-forget 异步 —— 等缓冲被 DB 形状替换。
    await vi.waitFor(() => {
      const msgs = (
        stream as unknown as { messagesBySession: Map<string, ChatMessage[]> }
      ).messagesBySession.get("s-1")!;
      expect(msgs.some((m) => m.id === "s-1-1")).toBe(true);
    });

    const msgs = (
      stream as unknown as { messagesBySession: Map<string, ChatMessage[]> }
    ).messagesBySession.get("s-1")!;
    const lastAssistant = [...msgs].reverse().find((m) => m.role === "assistant")!;
    expect(lastAssistant.id).toBe("s-1-1"); // DB 权威替换确实发生
    expect(lastAssistant.error).toEqual({
      message: "服务器错误 (HTTP 502)",
      category: "server",
    });
  });

  it("done(正常收官)不携带 terminalError(对照组)", async () => {
    const { stream } = setup();
    handle(stream, { request_id: "rid-e2", session_id: "s-1", kind: "start" });
    handle(stream, { request_id: "rid-e2", session_id: "s-1", kind: "delta", text: "ok" });
    handle(stream, { request_id: "rid-e2", session_id: "s-1", kind: "done" });

    await vi.waitFor(() => {
      const msgs = (
        stream as unknown as { messagesBySession: Map<string, ChatMessage[]> }
      ).messagesBySession.get("s-1")!;
      expect(msgs.some((m) => m.id === "s-1-1")).toBe(true);
    });
    const msgs = (
      stream as unknown as { messagesBySession: Map<string, ChatMessage[]> }
    ).messagesBySession.get("s-1")!;
    const lastAssistant = [...msgs].reverse().find((m) => m.role === "assistant")!;
    expect(lastAssistant.error).toBeUndefined();
  });
});
