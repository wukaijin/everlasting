// M4a 定时审议收官 toast(09-07-gce-m4a,评审 P2-9 双弹抑制)。
//
// 第一组:`resolveScheduledDiscussion` / `buildScheduledDiscussionNotification`
// / `scheduledStopReasonLabel` 纯函数(镜像 turnFinishedNotification.test.ts
// 的纯函数组)。
// 第二组:store 集成 —— streamEvents.handleChatEvent 终态挂点:
//   · 定时审议场终态 → **恰好一条**专用收官 toast(任务名 + 收官原因 +
//     sessionId 附着),通用 `maybeNotifyTurnFinished` 文案被抑制;
//   · 非定时场 → 通用通知(既有行为零变化);
//   · cancelled 恒不弹(用户主动停止语义)。
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
import type { ChatMessage, SessionSummary } from "./chat.types";
import { useProjectsStore } from "./projects";
import { useScheduledTasksStore } from "./scheduledTasks";
import type { ScheduledTask } from "./scheduledTasks";
import { useStreamControllerStore } from "./streamController";
import {
  buildScheduledDiscussionNotification,
  resolveScheduledDiscussion,
  scheduledStopReasonLabel,
} from "./streamController";

function mkSummary(
  id: string,
  title: string,
  opts?: { busy?: boolean; metadata?: Record<string, unknown> | null },
): SessionSummary {
  return {
    id,
    title,
    updated_at: "2026-09-07T00:00:00Z",
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
    metadata: opts?.metadata ?? null,
    busy: opts?.busy,
  };
}

describe("scheduledStopReasonLabel(纯函数)", () => {
  it("编排器边界 stop_reason → 可读收官短语", () => {
    expect(scheduledStopReasonLabel("group_chat_end")).toBe("正常收官");
    expect(scheduledStopReasonLabel("max_rounds")).toBe("达到轮次上限");
    expect(scheduledStopReasonLabel("preempted")).toBe("被打断收束");
    expect(scheduledStopReasonLabel("cancelled")).toBe("已取消");
    expect(scheduledStopReasonLabel("interrupted")).toBe("进程中断");
    expect(scheduledStopReasonLabel("error")).toBe("出错终止");
  });

  it("无 stop_reason 的 error 事件按 kind 兜底;未知值原样透出", () => {
    expect(scheduledStopReasonLabel(undefined, "error")).toBe("出错终止");
    expect(scheduledStopReasonLabel(undefined, "done")).toBe("结束");
    expect(scheduledStopReasonLabel("mystery")).toBe("mystery");
  });
});

describe("resolveScheduledDiscussion(纯函数)", () => {
  const tasks = [
    {
      target_mode: "group_chat",
      last_run_session_id: "s-gc",
      name: "每周评审",
    },
    {
      target_mode: "per_run",
      last_run_session_id: "s-pr",
      name: "巡检",
    },
  ];

  it("任务行锚点:group_chat 档的 last_run_session_id 命中 → 任务名", () => {
    expect(resolveScheduledDiscussion("s-gc", tasks, null)).toBe("每周评审");
  });

  it("per_run 行 / 未命中 session → null(走通用通知)", () => {
    expect(resolveScheduledDiscussion("s-pr", tasks, null)).toBeNull();
    expect(resolveScheduledDiscussion("s-x", tasks, null)).toBeNull();
  });

  it("metadata 兜底:created_via === 'scheduled' → 任务名(缺名降级占位)", () => {
    expect(
      resolveScheduledDiscussion("s-x", tasks, {
        created_via: "scheduled",
        scheduled_task_name: "日报审议",
      }),
    ).toBe("日报审议");
    expect(resolveScheduledDiscussion("s-x", tasks, { created_via: "scheduled" })).toBe(
      "定时审议",
    );
    expect(resolveScheduledDiscussion("s-x", tasks, { created_via: "mcp" })).toBeNull();
  });

  it("任务行优先于 metadata", () => {
    expect(
      resolveScheduledDiscussion("s-gc", tasks, {
        created_via: "scheduled",
        scheduled_task_name: "另一个名字",
      }),
    ).toBe("每周评审");
  });
});

describe("buildScheduledDiscussionNotification(纯函数)", () => {
  it("文案 = 定时审议「任务名」已收官(原因),sessionId 附着", () => {
    const n = buildScheduledDiscussionNotification("s-gc", "每周评审", "group_chat_end");
    expect(n.sessionId).toBe("s-gc");
    expect(n.message).toBe("定时审议「每周评审」已收官(正常收官)");
  });
});

describe("M4a — 收官 toast 双弹抑制(streamEvents 集成)", () => {
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

  /** 场景脚手架:当前 session = s-cur;定时审议场 s-gc(busy 来自
   *  list_sessions);scheduledTasks store 直插任务行(fire 落账锚点)。 */
  function setup(opts?: {
    metadata?: Record<string, unknown> | null;
    seedTask?: boolean;
  }) {
    const stream = useStreamControllerStore();
    const chatStore = useChatStore();
    chatStore.currentSessionId = "s-cur";
    chatStore.sessions = [
      mkSummary("s-cur", "前台会话"),
      mkSummary("s-gc", "每周评审", {
        busy: true,
        metadata: opts?.metadata ?? null,
      }),
    ];
    if (opts?.seedTask ?? true) {
      const task = {
        id: "t1",
        project_id: "p1",
        target_session_id: null,
        target_mode: "group_chat",
        model_id: null,
        last_run_session_id: "s-gc",
        name: "每周评审",
        prompt: "复盘",
        schedule: { kind: "weekly", weekday: "fri", at: "18:00" },
        enabled: true,
        created_by: "user",
        created_at: 1,
        last_fired_at: null,
        next_fire_at: 9,
        run_count: 0,
        max_runs: null,
        ends_at: null,
        group_chat_config: null,
        last_fire_outcome: "started",
      } as ScheduledTask;
      useScheduledTasksStore().tasks = [task];
    }
    const msgs: ChatMessage[] = [
      { id: "u0", role: "user", content: "议题" },
      { id: "a0", role: "assistant", content: "" },
    ];
    stream.putMessages("s-gc", msgs, false);
    return { stream, chatStore };
  }

  it("定时审议场终态 → 恰好一条专用收官 toast(通用通知被抑制)", () => {
    const { stream } = setup();
    handle(stream, { request_id: "rid-1", session_id: "s-gc", kind: "delta", text: "讨论中" });
    handle(stream, {
      request_id: "rid-1",
      session_id: "s-gc",
      kind: "done",
      stop_reason: "group_chat_end",
    });
    const t = useProjectsStore().toast;
    expect(t).not.toBeNull();
    expect(t?.message).toBe("定时审议「每周评审」已收官(正常收官)");
    expect(t?.sessionId).toBe("s-gc");
    // 双弹抑制:通用文案(「任务已完成」)不出现。
    expect(t?.message).not.toContain("已完成");
  });

  it("max_rounds / error 终态 → 收官原因随 toast 透出", () => {
    const { stream } = setup();
    handle(stream, { request_id: "rid-2", session_id: "s-gc", kind: "delta", text: "x" });
    handle(stream, {
      request_id: "rid-2",
      session_id: "s-gc",
      kind: "done",
      stop_reason: "max_rounds",
    });
    expect(useProjectsStore().toast?.message).toContain("达到轮次上限");

    useProjectsStore().toast = null;
    handle(stream, { request_id: "rid-3", session_id: "s-gc", kind: "error", message: "boom" });
    expect(useProjectsStore().toast?.message).toContain("出错终止");
  });

  it("cancelled → 恒不弹(用户主动停止语义)", () => {
    const { stream } = setup();
    handle(stream, { request_id: "rid-4", session_id: "s-gc", kind: "delta", text: "x" });
    handle(stream, {
      request_id: "rid-4",
      session_id: "s-gc",
      kind: "done",
      stop_reason: "cancelled",
    });
    expect(useProjectsStore().toast).toBeNull();
  });

  it("任务行缓存未命中时 metadata 兜底(当前 project 的 session)", () => {
    const { stream } = setup({
      seedTask: false,
      metadata: { created_via: "scheduled", scheduled_task_name: "日报审议" },
    });
    handle(stream, {
      request_id: "rid-5",
      session_id: "s-gc",
      kind: "done",
      stop_reason: "group_chat_end",
    });
    expect(useProjectsStore().toast?.message).toBe(
      "定时审议「日报审议」已收官(正常收官)",
    );
  });

  it("两级判定都不命中(foreign 普通 session)→ 通用通知,零行为变化", () => {
    const { stream } = setup({ seedTask: false });
    handle(stream, { request_id: "rid-6", session_id: "s-gc", kind: "delta", text: "x" });
    handle(stream, {
      request_id: "rid-6",
      session_id: "s-gc",
      kind: "done",
      stop_reason: "group_chat_end",
    });
    const t = useProjectsStore().toast;
    expect(t?.message).toContain("每周评审");
    expect(t?.message).toContain("已完成");
    expect(t?.message).not.toContain("已收官");
  });
});
