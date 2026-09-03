// F3 磁盘治理(2026-09-03, task 09-03-f3-disk-governance)PR1 —
// configStore 两个新开关的接线测试:fail-open 缺省 ref、key 常量与
// 后端 SETTABLE_APP_FLAGS 白名单严格一致、写成功才更新本地 ref
// (失败抛错给调用方,本地保持 DB 现状 —— 与既有开关同款约定)。
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

describe("configStore — F3 磁盘治理开关", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockClear();
    invokeMock.mockImplementation(async () => null);
  });

  it("缺省 fail-open:两个开关都是 true(旧 daemon 未回字段也如此)", () => {
    const cfg = useConfigStore();
    expect(cfg.diskGovernorEnabled).toBe(true);
    expect(cfg.outputsAgeCleanupEnabled).toBe(true);
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

  it("写失败 → 抛错给调用方,本地 ref 保持不变", async () => {
    invokeMock.mockRejectedValue(new Error("transport boom"));
    const cfg = useConfigStore();
    await expect(cfg.setDiskGovernorEnabled(false)).rejects.toThrow();
    await expect(cfg.setOutputsAgeCleanupEnabled(false)).rejects.toThrow();
    expect(cfg.diskGovernorEnabled).toBe(true);
    expect(cfg.outputsAgeCleanupEnabled).toBe(true);
  });
});
