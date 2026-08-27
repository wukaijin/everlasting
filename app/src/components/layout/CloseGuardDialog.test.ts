// CloseGuardDialog — F6(2026-08-27)关闭确认的 busy 闭合计数测试。
//
// 计数口径 = 本端 streamingSessionIds ∪ 服务端 busy(chatStore.sessions
// 的 runtime 信号,含其他端发起/等闸轮次),同一 session 两态并见只计
// 一次。jsdom 无 __TAURI_INTERNALS__ → isTauriWebview() 恒 false,
// onCloseRequested 注册分支天然跳过(这正是 Web/PWA 的生产行为),
// 测试聚焦 countBusy 的并集语义。
import { describe, it, expect, beforeEach } from "vitest";
import { mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { useChatStore } from "../../stores/chat";
import { useStreamControllerStore } from "../../stores/streamController";
import type { SessionSummary } from "../../stores/chat.types";
import CloseGuardDialog from "./CloseGuardDialog.vue";

function mkSummary(id: string, busy?: boolean): SessionSummary {
  return {
    id,
    title: "t",
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

/** streamingSessionIds 是 activeRequests 的 computed —— 经注册
 * RequestState 驱动(镜像 messageQueueStream.test 的直插方式)。 */
function fakeStreamingRequest(stream: ReturnType<typeof useStreamControllerStore>, rid: string, sid: string) {
  (stream as unknown as {
    activeRequests: Map<string, Record<string, unknown>>;
  }).activeRequests.set(rid, {
    requestId: rid,
    sessionId: sid,
    projectId: "",
    userMsgId: "",
    assistantMsgId: "",
    groupChat: false,
    groupChatStarted: false,
    pendingSpeaker: null,
    history: [],
    sendAt: 0,
    firstDeltaAt: null,
    toolStartedAt: new Map<string, number>(),
    currentTurnIndex: -1,
    latencyByTurn: new Map<number, unknown>(),
    pendingTimelineText: null,
  });
}

describe("CloseGuardDialog — countBusy 闭包计数", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  function count(): number {
    const w = mount(CloseGuardDialog);
    return (w.vm as unknown as { countBusy: () => number }).countBusy();
  }

  it("空闲(无 streaming 无 busy)→ 0(放行默认关闭)", () => {
    expect(count()).toBe(0);
  });

  it("仅本端 streaming(发起/认领)→ 计数", () => {
    fakeStreamingRequest(useStreamControllerStore(), "rid-a", "s-a");
    expect(count()).toBe(1);
  });

  it("仅服务端 busy(其他端发起,本端无感知)→ 计数", () => {
    const chatStore = useChatStore();
    chatStore.sessions = [mkSummary("s-x", true)];
    expect(count()).toBe(1);
  });

  it("两态并见同一 session 只计一次 + 不同 session 相加", () => {
    const stream = useStreamControllerStore();
    fakeStreamingRequest(stream, "rid-a", "s-a");
    fakeStreamingRequest(stream, "rid-d", "s-d");
    const chatStore = useChatStore();
    chatStore.sessions = [
      mkSummary("s-a", true), // 双态,计 1
      mkSummary("s-b", true), // 仅 busy,计 1
      mkSummary("s-c", false), // 不计
    ];
    expect(count()).toBe(3);
  });
});
