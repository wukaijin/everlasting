// GCE-M3 (09-06-gce-m3-control-plane) — chat store `preemptGroupChat` action:
//   - wire shape: invoke("preempt_group_chat", { sessionId }) —— 顶层 key 的
//     camelCase 由各 transport 自行扳正(Tauri 纯透传 command;http → body
//     session_id),本测试锁 store 层调用形状;
//   - outcome 分派:preempted=true → info toast;false → warn;reject → error。
//
// Canonical transport-barrel mock(test-environment.md §4,同
// GroupChatConfigModal.test.ts):mock "../transport" barrel,
// 全 store 经 pinia 实例化。

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

function setupStore() {
  setActivePinia(createPinia());
  const chat = useChatStore();
  const projects = useProjectsStore();
  return { chat, projects };
}

beforeEach(() => {
  invokeMock.mockReset();
  // store 建立期间若有杂散拉取,给良性应答(list → 空,其余 null)
  invokeMock.mockImplementation(async (cmd: string) =>
    cmd === "list_sessions" ? [] : null,
  );
});

describe("chat store preemptGroupChat(收束打断,与 API 同权同语义)", () => {
  it("invoke 形状 ('preempt_group_chat', { sessionId});preempted → info toast", async () => {
    const { chat, projects } = setupStore();
    chat.currentSessionId = "sess-1";
    invokeMock.mockResolvedValue({ preempted: true });

    await chat.preemptGroupChat();

    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledWith("preempt_group_chat", {
      sessionId: "sess-1",
    });
    expect(projects.toast?.kind).toBe("info");
    expect(projects.toast?.message).toContain("收束打断已请求");
  });

  it("preempted=false → warn toast(讨论可能已结束)", async () => {
    const { chat, projects } = setupStore();
    chat.currentSessionId = "sess-1";
    invokeMock.mockResolvedValue({ preempted: false });

    await chat.preemptGroupChat();

    expect(projects.toast?.kind).toBe("warn");
    expect(projects.toast?.message).toContain("未受理");
  });

  it("invoke 拒绝 → error toast(端点报错原样透传)", async () => {
    const { chat, projects } = setupStore();
    chat.currentSessionId = "sess-1";
    invokeMock.mockRejectedValue(new Error("该会话当前没有进行中的群聊讨论"));

    await chat.preemptGroupChat();

    expect(projects.toast?.kind).toBe("error");
    expect(projects.toast?.message).toContain("没有进行中的群聊讨论");
  });

  it("无 current session → 早退零调用", async () => {
    const { chat } = setupStore();
    chat.currentSessionId = null;

    await chat.preemptGroupChat();

    expect(invokeMock).not.toHaveBeenCalled();
  });
});
