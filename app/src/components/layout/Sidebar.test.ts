// Tests for the Sidebar density toggle → SessionList wiring.
//
// 历史坑(2026-08-30 修):density 曾在 SessionList 内部自持 ref、
// 只在 setup 时读一次 localStorage,而按钮在 Sidebar 里写
// localStorage —— localStorage 写入不响应,两边不联通,点了按钮
// 列表毫无反应、要刷新页面才生效。修复后 Sidebar 持有状态 +
// 持久化,SessionList 经 prop 接收;本组测试把这条响应式契约钉住:
//   - 点击头部按钮 → 列表 class 立即翻转(不需要 reload)
//   - compact 下项目名 + 「·」分隔符隐藏
//   - localStorage 持久化 + 首屏读取(重载路径)

import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";
import { mount, VueWrapper } from "@vue/test-utils";

const invokeMock = vi.fn();
vi.mock("../../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

import Sidebar from "./Sidebar.vue";
import { useChatStore } from "../../stores/chat";
import type { SessionSummary } from "../../stores/chat.types";

const DENSITY_LS_KEY = "everlasting:sessionDensity";

function makeSession(id: string, title: string): SessionSummary {
  return {
    id,
    title,
    // 真实时间戳:groupSessions 按 updated_at 分桶,空串会让会话
    // 落不进任何桶 → 列表项不渲染,断言不到项目名显隐。
    updated_at: new Date().toISOString(),
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
    workflow_enabled: false,
    plugin_name: "",
    mode: "edit",
    session_type: "chat",
    metadata: null,
  };
}

describe("Sidebar density toggle", () => {
  let wrapper: VueWrapper | null = null;

  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    localStorage.clear();
    wrapper = null;
  });

  function mountSidebar(): VueWrapper {
    const chat = useChatStore();
    chat.sessions = [makeSession("s1", "hello")];
    wrapper = mount(Sidebar, {
      global: {
        stubs: {
          ProjectTabs: true,
          SettingsModal: true,
          GroupChatConfigModal: true,
        },
      },
    });
    return wrapper;
  }

  it("默认舒适密度:列表无 --compact,项目名可见", () => {
    const w = mountSidebar();
    const list = w.find(".session-list");
    expect(list.classes()).toContain("session-list--comfortable");
    expect(list.classes()).not.toContain("session-list--compact");
    expect(w.find(".session-item__project").exists()).toBe(true);
  });

  it("点击密度按钮 → 列表立即变紧凑(项目名隐藏 + localStorage 持久化)", async () => {
    const w = mountSidebar();
    const btn = w.find('button[title="切换为紧凑密度"]');
    expect(btn.exists()).toBe(true);
    await btn.trigger("click");
    const list = w.find(".session-list");
    expect(list.classes()).toContain("session-list--compact");
    expect(w.find(".session-item__project").exists()).toBe(false);
    expect(localStorage.getItem(DENSITY_LS_KEY)).toBe("compact");
  });

  it("再点一次切回舒适密度", async () => {
    const w = mountSidebar();
    await w.find('button[title="切换为紧凑密度"]').trigger("click");
    await w.find('button[title="切换为舒适密度"]').trigger("click");
    expect(w.find(".session-list").classes()).not.toContain(
      "session-list--compact",
    );
    expect(w.find(".session-item__project").exists()).toBe(true);
    expect(localStorage.getItem(DENSITY_LS_KEY)).toBe("comfortable");
  });

  it("localStorage 存过 compact → 首屏即紧凑(重载路径)", () => {
    localStorage.setItem(DENSITY_LS_KEY, "compact");
    const w = mountSidebar();
    expect(w.find(".session-list").classes()).toContain(
      "session-list--compact",
    );
  });
});
