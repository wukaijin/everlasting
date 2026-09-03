// F3 磁盘治理(2026-09-03, task 09-03-f3-disk-governance)PR1 —
// configStore 两个新开关的接线测试:fail-open 缺省 ref、key 常量与
// 后端 SETTABLE_APP_FLAGS 白名单严格一致、写成功才更新本地 ref
// (失败抛错给调用方,本地保持 DB 现状 —— 与既有开关同款约定)。
// 2026-09-03 (task 09-03-ask-no-timeout):加 `ask_no_timeout` ——
// 与 kill-switch 方向相反,enable 语义 fail-closed 缺省 false。
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

import { useConfigStore } from "./config";

describe("configStore — F3 磁盘治理开关 + ask_no_timeout", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockClear();
    invokeMock.mockImplementation(async () => null);
  });

  it("缺省 fail-open:两个磁盘开关都是 true(旧 daemon 未回字段也如此)", () => {
    const cfg = useConfigStore();
    expect(cfg.diskGovernorEnabled).toBe(true);
    expect(cfg.outputsAgeCleanupEnabled).toBe(true);
  });

  it("缺省 fail-closed:askNoTimeout 是 false(enable 语义,与 kill-switch 反向)", () => {
    const cfg = useConfigStore();
    expect(cfg.askNoTimeout).toBe(false);
  });

  it("setDiskGovernorEnabled:key 与后端白名单一致,写成功才更新本地 ref", async () => {
    const cfg = useConfigStore();
    await cfg.setDiskGovernorEnabled(false);
    expect(cfg.diskGovernorEnabled).toBe(false);
    expect(invokeMock).toHaveBeenCalledWith("set_app_config_flag", {
      key: "disk_governor_enabled",
      value: false,
    });
  });

  it("setOutputsAgeCleanupEnabled:同款接线", async () => {
    const cfg = useConfigStore();
    await cfg.setOutputsAgeCleanupEnabled(false);
    expect(cfg.outputsAgeCleanupEnabled).toBe(false);
    expect(invokeMock).toHaveBeenCalledWith("set_app_config_flag", {
      key: "outputs_age_cleanup_enabled",
      value: false,
    });
  });

  it("setAskNoTimeout:key ask_no_timeout 与后端白名单一致,写 true 才更新 ref", async () => {
    const cfg = useConfigStore();
    await cfg.setAskNoTimeout(true);
    expect(cfg.askNoTimeout).toBe(true);
    expect(invokeMock).toHaveBeenCalledWith("set_app_config_flag", {
      key: "ask_no_timeout",
      value: true,
    });
  });

  it("写失败 → 抛错给调用方,本地 ref 保持不变", async () => {
    invokeMock.mockRejectedValue(new Error("transport boom"));
    const cfg = useConfigStore();
    await expect(cfg.setDiskGovernorEnabled(false)).rejects.toThrow();
    await expect(cfg.setOutputsAgeCleanupEnabled(false)).rejects.toThrow();
    await expect(cfg.setAskNoTimeout(true)).rejects.toThrow();
    expect(cfg.diskGovernorEnabled).toBe(true);
    expect(cfg.outputsAgeCleanupEnabled).toBe(true);
    expect(cfg.askNoTimeout).toBe(false);
  });
});
