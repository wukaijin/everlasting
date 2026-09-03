// Tests for `DiskTab.vue` — Settings「磁盘」分类(F3 磁盘治理 PR3,
// 2026-09-03,design §7)。
//
// 契约:
//   1. 挂载自动拉 get_disk_usage;条目 label + 人类可读大小 + 总量行渲染。
//   2. 开关 toggle → configStore setter(写成功才更新本地 ref 的 IPC 形状)。
//   3. 清理按钮:pending 期间禁点 → run_disk_cleanup 成功 → toast 含回收
//      字节数与逐项摘要 → 自动重新 get_disk_usage(AC7 数字下降闭环)。
//   4. kill-switch 关闭(diskGovernorEnabled=false)→ 按钮仍可用 + 说明
//      文案在场(AC9 手动语义)。
//
// transport / projects store mock(ScheduledTasksTab.test.ts 同款);config
// store 用真 pinia(缺省两开关 true)。

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn();
vi.mock("../../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

const showToastMock = vi.fn();
vi.mock("../../stores/projects", () => ({
  useProjectsStore: () => ({
    showToast: showToastMock,
  }),
}));

import DiskTab from "./DiskTab.vue";
import { useConfigStore } from "../../stores/config";

function usageReport(totalBytes = 4096) {
  return {
    entries: [
      { key: "db", label: "数据库(主库 + WAL)", bytes: 2048 },
      { key: "backups", label: "数据库备份", bytes: 1024 },
      { key: "outputs", label: "工具输出 spill", bytes: 1024 },
      { key: "worktrees", label: "Git worktrees", bytes: 0 },
      { key: "attachments", label: "图片附件", bytes: 0 },
      { key: "logs", label: "daemon 日志", bytes: 0 },
      { key: "webkit_cache", label: "WebKit 缓存", bytes: 0 },
    ],
    totalBytes,
  };
}

const zeroOutcome = () => ({
  workerWorktrees: { items: 0, reclaimedBytes: 0 },
  orphanSessionWorktrees: { items: 0, reclaimedBytes: 0 },
  outputs: { items: 0, reclaimedBytes: 0 },
  backups: { items: 0, reclaimedBytes: 0 },
});

/** 组件挂载即拉一次 get_disk_usage;缺省 stub 满足之。 */
function stubBackend(overrides: Record<string, unknown> = {}) {
  invokeMock.mockImplementation(async (cmd: string) => {
    if (cmd === "get_disk_usage") return usageReport();
    if (cmd === "run_disk_cleanup") return zeroOutcome();
    if (cmd === "set_app_config_flag") return null;
    if (overrides[cmd]) return overrides[cmd];
    return null;
  });
}

function mountTab() {
  return mount(DiskTab, {
    global: {
      plugins: [],
    },
  });
}

beforeEach(() => {
  setActivePinia(createPinia());
  invokeMock.mockClear();
  showToastMock.mockClear();
  stubBackend();
});

describe("DiskTab — 占用概览", () => {
  it("挂载自动拉取;渲染条目 label + 人类可读大小 + 总量行", async () => {
    const wrapper = mountTab();
    await flushPromises();

    const text = wrapper.text();
    expect(text).toContain("数据库(主库 + WAL)");
    expect(text).toContain("2.0 KB");
    expect(text).toContain("1.0 KB");
    expect(text).toContain("合计");
    expect(text).toContain("4.0 KB");
    // 调用形状:进入 tab 一次 get_disk_usage,无参。
    expect(invokeMock).toHaveBeenCalledWith("get_disk_usage");
  });

  it("刷新按钮重新拉取", async () => {
    const wrapper = mountTab();
    await flushPromises();
    invokeMock.mockClear();
    await wrapper.find(".disk-tab__refresh").trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("get_disk_usage");
  });
});

describe("DiskTab — 回收开关", () => {
  it("toggle 走 configStore setter:key 与后端白名单一致,成功后本地 ref 翻转", async () => {
    const wrapper = mountTab();
    await flushPromises();
    const config = useConfigStore();
    expect(config.diskGovernorEnabled).toBe(true);

    const switches = wrapper.findAll('button[role="switch"]');
    expect(switches).toHaveLength(2);
    await switches[0].trigger("click");
    await flushPromises();

    expect(invokeMock).toHaveBeenCalledWith("set_app_config_flag", {
      key: "disk_governor_enabled",
      value: false,
    });
    expect(config.diskGovernorEnabled).toBe(false);
  });

  it("第二个开关写 outputs_age_cleanup_enabled", async () => {
    const wrapper = mountTab();
    await flushPromises();
    const switches = wrapper.findAll('button[role="switch"]');
    await switches[1].trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("set_app_config_flag", {
      key: "outputs_age_cleanup_enabled",
      value: false,
    });
  });
});

describe("DiskTab — 立即清理", () => {
  it("成功:toast 含总回收量与逐项摘要;自动重新拉概览(AC7 闭环)", async () => {
    let usageCalls = 0;
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "get_disk_usage") {
        usageCalls += 1;
        // 清理后总量下降:第一次 4096,刷新后 2048。
        return usageReport(usageCalls === 1 ? 4096 : 2048);
      }
      if (cmd === "run_disk_cleanup") {
        return {
          workerWorktrees: { items: 0, reclaimedBytes: 0 },
          orphanSessionWorktrees: { items: 2, reclaimedBytes: 3 * 1024 * 1024 },
          outputs: { items: 1, reclaimedBytes: 512 * 1024 },
          backups: { items: 0, reclaimedBytes: 0 },
        };
      }
      return null;
    });

    const wrapper = mountTab();
    await flushPromises();
    expect(wrapper.text()).toContain("4.0 KB");

    await wrapper.find(".disk-tab__cleanup").trigger("click");
    await flushPromises();

    // toast:总量 3.5 MB + 逐项摘要(仅 items>0 的两项)。
    expect(showToastMock).toHaveBeenCalledTimes(1);
    const msg = showToastMock.mock.calls[0][0] as string;
    expect(msg).toContain("3.5 MB");
    expect(msg).toContain("孤儿 session worktree 2 项");
    expect(msg).toContain("工具输出 spill 1 项");
    // AC7:resolve 成功后自动重新 get_disk_usage,总量数字同步下降。
    expect(usageCalls).toBe(2);
    expect(wrapper.text()).toContain("2.0 KB");
    expect(wrapper.text()).not.toContain("4.0 KB");
  });

  it("无可回收项:toast 走「没有可回收项」文案,仍刷新概览", async () => {
    const wrapper = mountTab();
    await flushPromises();
    await wrapper.find(".disk-tab__cleanup").trigger("click");
    await flushPromises();
    const msg = showToastMock.mock.calls[0][0] as string;
    expect(msg).toContain("没有可回收项");
    // 挂载一次 + 清理后刷新一次。
    expect(invokeMock.mock.calls.filter(([c]) => c === "get_disk_usage")).toHaveLength(2);
  });

  it("失败:toast 错误,按钮恢复可点", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "get_disk_usage") return usageReport();
      if (cmd === "run_disk_cleanup") throw new Error("boom");
      return null;
    });
    const wrapper = mountTab();
    await flushPromises();

    await wrapper.find(".disk-tab__cleanup").trigger("click");
    await flushPromises();

    expect(showToastMock).toHaveBeenCalledTimes(1);
    expect((showToastMock.mock.calls[0][0] as string)).toContain("清理失败");
    expect((wrapper.find(".disk-tab__cleanup").element as HTMLButtonElement).disabled).toBe(false);
  });
});

describe("DiskTab — kill-switch 关闭时(AC9 手动语义)", () => {
  it("按钮仍可用,且出现说明文字", async () => {
    const config = useConfigStore();
    config.diskGovernorEnabled = false;
    const wrapper = mountTab();
    await flushPromises();

    expect(wrapper.text()).toContain("自动回收已关闭");
    const btn = wrapper.find(".disk-tab__cleanup").element as HTMLButtonElement;
    expect(btn.disabled).toBe(false);
  });
});
