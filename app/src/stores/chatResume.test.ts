// GCE-P1a(09-06-gc-p1a-checkpoint-resume)— chat store `resumeGroupChat`
// action + 终态合并回写(reloadAfterFinalize)的组合锁定:
//   - wire 形状:invoke("resume_group_chat", { sessionId })(顶层 key
//     camelCase 由各 transport 自行扳正;Tauri 纯透传 command,
//     http CMD_TO_DOMAIN → agent 域);
//   - 受理分派:status=started → info toast + 返回 true;其他 → warn +
//     false;reject → error + false;
//   - 终态合并:finalize 后 load_session 的 session.stop_reason 写回
//     sessions[](「续跑」按钮的数据通路,评审 P0-1B)。

import { describe, it, expect, vi, beforeEach } from "vitest";
import { createPinia, setActivePinia } from "pinia";

const invokeMock = vi.fn();

vi.mock("../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

import { useChatStore } from "./chat";
import { useProjectsStore } from "./projects";
import { useStreamControllerStore } from "./streamController";

function setupStore() {
  setActivePinia(createPinia());
  const chat = useChatStore();
  const projects = useProjectsStore();
  return { chat, projects };
}

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockImplementation(async (cmd: string) =>
    cmd === "list_sessions" ? [] : null,
  );
});

describe("chat store resumeGroupChat(续跑中断讨论,与 API 同权同语义)", () => {
  it("invoke 形状 ('resume_group_chat', { sessionId});started → info toast + true", async () => {
    const { chat, projects } = setupStore();
    chat.currentSessionId = "sess-1";
    invokeMock.mockResolvedValue({ status: "started" });

    const ok = await chat.resumeGroupChat();

    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledWith("resume_group_chat", {
      sessionId: "sess-1",
    });
    expect(ok).toBe(true);
    expect(projects.toast?.kind).toBe("info");
    expect(projects.toast?.message).toContain("已从断点续跑");
  });

  it("非 started 受理 → warn toast + false", async () => {
    const { chat, projects } = setupStore();
    chat.currentSessionId = "sess-1";
    invokeMock.mockResolvedValue({ status: "queued" });

    const ok = await chat.resumeGroupChat();

    expect(ok).toBe(false);
    expect(projects.toast?.kind).toBe("warn");
    expect(projects.toast?.message).toContain("未受理");
  });

  it("invoke 拒绝(五类校验任一)→ error toast + false", async () => {
    const { chat, projects } = setupStore();
    chat.currentSessionId = "sess-1";
    invokeMock.mockRejectedValue(
      new Error("无可续跑的断点(该会话没有中断的讨论)"),
    );

    const ok = await chat.resumeGroupChat();

    expect(ok).toBe(false);
    expect(projects.toast?.kind).toBe("error");
    expect(projects.toast?.message).toContain("无可续跑的断点");
  });

  it("无 current session → 早退零调用", async () => {
    const { chat } = setupStore();
    chat.currentSessionId = null as unknown as string;

    const ok = await chat.resumeGroupChat();

    expect(ok).toBe(false);
    expect(invokeMock).not.toHaveBeenCalled();
  });
});

describe("终态 stop_reason 合并回写(续跑按钮数据通路,评审 P0-1B)", () => {
  it("finalize 路径的 load_session 结果把 stop_reason/discussion_summary 写回 sessions[]", async () => {
    const { chat } = setupStore();
    // 侧栏 list 里有一个群聊 session。
    chat.sessions = [
      {
        id: "sess-gc",
        title: "群聊",
        updated_at: "2026-09-06T00:00:00Z",
        preview: "",
        project_id: "p1",
        current_cwd: "/tmp",
        worktree_state: "none",
        worktree_path: null,
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
        metadata: null,
        busy: true,
        stop_reason: null,
        discussion_summary: null,
      },
    ];
    chat.currentSessionId = "sess-gc";

    // finalize 后的权威重拉:session 行带终局值。
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "load_session") {
        return {
          session: {
            id: "sess-gc",
            title: "群聊",
            created_at: "2026-09-06T00:00:00Z",
            updated_at: "2026-09-06T00:10:00Z",
            model: "GLM-4.7",
            project_id: "p1",
            current_cwd: "/tmp",
            worktree_state: "none",
            worktree_path: null,
            last_worktree_path: null,
            model_id: null,
            input_tokens_total: null,
            output_tokens_total: null,
            cache_creation_total: null,
            cache_read_total: null,
            session_type: "group_chat",
            metadata: null,
            stop_reason: "cancelled",
            discussion_summary: null,
          },
          messages: [],
        };
      }
      return null;
    });

    // 经 store 公开的 finalize 钩子触发(controller 内部走
    // reloadAfterFinalize;这里直接调 streamEvents 的公共入口之一 —
    // finalizeRequest 由 done 事件路由,测试经 controller API 驱动)。
    const controller = useStreamControllerStore();
    await controller.reloadAfterFinalize("sess-gc", "rid-1");

    const summary = chat.sessions.find((s) => s.id === "sess-gc")!;
    expect(summary.stop_reason).toBe("cancelled");
    expect(summary.busy).toBe(true); // busy 由 finalizeRequest 翻,本路径不动
  });
});
