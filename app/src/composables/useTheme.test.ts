// useTheme — 主题状态机单测:持久化读写、DOM 落点、往返切换。
//
// 关键裁定(与实现注释一致):
//   - 默认 aggressive(实验期);
//   - classic 必须**删除** data-theme(保证 classic 渲染路径与历史
//     一致,零覆盖参与),而非写成 data-theme="classic"。
//
// 状态是模块级单例(ref 在模块求值时以 localStorage 初始化),所以
// 每个用例先摆好存储再 vi.resetModules() + 动态 import,让 readStored
// 在用例的前置状态下重新求值。

import { describe, it, expect, vi, beforeEach } from "vitest";

describe("useTheme", () => {
  beforeEach(() => {
    localStorage.removeItem("everlasting.theme");
    delete document.documentElement.dataset.theme;
    vi.resetModules();
  });

  async function load() {
    return import("./useTheme");
  }

  it("无存储值时默认 aggressive 并落到 <html data-theme>", async () => {
    const { useTheme } = await load();
    const { theme } = useTheme();
    expect(theme.value).toBe("aggressive");
    expect(document.documentElement.dataset.theme).toBe("aggressive");
  });

  it("存储了 classic 时恢复 classic 且不携带 data-theme 属性", async () => {
    localStorage.setItem("everlasting.theme", "classic");
    const { useTheme } = await load();
    const { theme } = useTheme();
    expect(theme.value).toBe("classic");
    expect(document.documentElement.dataset.theme).toBeUndefined();
  });

  it("toggleTheme 往返切换并持久化", async () => {
    const { useTheme } = await load();
    const { theme, toggleTheme } = useTheme();
    toggleTheme();
    expect(theme.value).toBe("classic");
    expect(localStorage.getItem("everlasting.theme")).toBe("classic");
    expect(document.documentElement.dataset.theme).toBeUndefined();
    toggleTheme();
    expect(theme.value).toBe("aggressive");
    expect(localStorage.getItem("everlasting.theme")).toBe("aggressive");
    expect(document.documentElement.dataset.theme).toBe("aggressive");
  });

  it("两次 useTheme() 共享同一状态(模块级单例)", async () => {
    const { useTheme } = await load();
    const a = useTheme();
    const b = useTheme();
    a.setTheme("classic");
    expect(b.theme.value).toBe("classic");
  });

  it("非法存储值按默认 aggressive 处理", async () => {
    localStorage.setItem("everlasting.theme", "bogus");
    const { useTheme } = await load();
    const { theme, setTheme } = useTheme();
    expect(theme.value).toBe("aggressive");
    setTheme("classic");
    setTheme("classic");
    expect(theme.value).toBe("classic");
    expect(document.documentElement.dataset.theme).toBeUndefined();
  });
});
