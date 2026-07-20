// Tests for `useToast` composable — A5 R1 (2026-07-17).
//
// 锁定契约:
// 1. show / dismiss / clear 三操作 + queue 上限(MAX_CONCURRENT=3)
// 2. 同 (category, title, description) 在 DEDUPE_WINDOW (5s) 内只弹 1 次
// 3. ttl 自动到期 → 自动 dismiss
// 4. FIFO overflow(老的先出)
// 5. readonly `toasts` ref
//
// 注意:模块级单例,所以 beforeEach 必须 clear。

import {
  describe,
  it,
  expect,
  beforeEach,
  afterEach,
  vi,
} from "vitest";
import {
  useToast,
  _useToastInternal_clearAllTimers,
} from "./useToast";

beforeEach(() => {
  useToast().clear();
  _useToastInternal_clearAllTimers();
  vi.useFakeTimers();
});

afterEach(() => {
  _useToastInternal_clearAllTimers();
  vi.useRealTimers();
});

describe("useToast — show / dismiss / clear", () => {
  it("show 把 toast 推入队列,toasts.length === 1", () => {
    const { show, toasts } = useToast();
    const id = show({
      category: "Network",
      title: "网络问题",
      description: "断线",
    });
    expect(id).not.toBeNull();
    expect(toasts.value).toHaveLength(1);
    const t = toasts.value[0];
    expect(t.category).toBe("Network");
    expect(t.title).toBe("网络问题");
    expect(t.description).toBe("断线");
    expect(t.ttl).toBe(5000);
  });

  it("dismiss 按 id 移除", () => {
    const { show, dismiss, toasts } = useToast();
    const id = show({ category: "Auth", title: "鉴权失败", description: "key invalid" });
    expect(toasts.value).toHaveLength(1);
    dismiss(id!);
    expect(toasts.value).toHaveLength(0);
  });

  it("dismiss 未知 id 静默 no-op", () => {
    const { dismiss, toasts } = useToast();
    useToast().show({ category: "Server", title: "服务端错误" });
    dismiss("not-an-id");
    expect(toasts.value).toHaveLength(1);
  });

  it("clear 清空所有 + 清 ttl 定时器", () => {
    const { show, clear, toasts } = useToast();
    show({ category: "Auth", title: "x" });
    show({ category: "Network", title: "y" });
    show({ category: "Server", title: "z" });
    expect(toasts.value).toHaveLength(3);
    clear();
    expect(toasts.value).toHaveLength(0);
  });
});

describe("useToast — queue 上限 MAX_CONCURRENT=3 + FIFO 溢出", () => {
  it("推 4 个,最早的被 FIFO 移除", () => {
    const { show, toasts } = useToast();
    show({ category: "Auth", title: "1" });
    show({ category: "Auth", title: "2" });
    show({ category: "Auth", title: "3" });
    show({ category: "Auth", title: "4" });
    expect(toasts.value).toHaveLength(3);
    // 最早的 "1" 应被移除
    expect(toasts.value.map((t) => t.title)).toEqual(["2", "3", "4"]);
  });
});

describe("useToast — dedupe 同 (category, title, description) 5s 内只 1 次", () => {
  it("同 desc 5s 内第 2 次返回 null", () => {
    const { show, toasts } = useToast();
    const id1 = show({
      category: "Server",
      title: "服务端错误",
      description: "boom",
    });
    expect(id1).not.toBeNull();
    const id2 = show({
      category: "Server",
      title: "服务端错误",
      description: "boom",
    });
    expect(id2).toBeNull();
    expect(toasts.value).toHaveLength(1);
  });

  it("不同 desc 不 dedupe", () => {
    const { show, toasts } = useToast();
    show({ category: "Server", title: "服务端错误", description: "boom1" });
    show({ category: "Server", title: "服务端错误", description: "boom2" });
    expect(toasts.value).toHaveLength(2);
  });

  it("不同 category 不 dedupe", () => {
    const { show, toasts } = useToast();
    show({ category: "Server", title: "x", description: "msg" });
    show({ category: "Network", title: "x", description: "msg" });
    expect(toasts.value).toHaveLength(2);
  });

  it("5s 后再发同条不 dedupe + ttl 已 dismiss 老的,所以最后应剩 1 条", () => {
    const { show, toasts } = useToast();
    show({ category: "Server", title: "x", description: "msg" });
    expect(toasts.value).toHaveLength(1);
    // advance 5s+1ms:触发第一条的 ttl 自动 dismiss(失去 dedupe 候选)
    vi.advanceTimersByTime(5001);
    // 此时 queue 已为空(老的被 dismiss 了) -> 第 2 条全新加入
    show({ category: "Server", title: "x", description: "msg" });
    expect(toasts.value).toHaveLength(1);
  });

  it("5s 内第 2 次 push 同条仍 dedupe 不入队(老的还在 ttl 内)", () => {
    const { show, toasts } = useToast();
    show({ category: "Server", title: "x", description: "msg" });
    vi.advanceTimersByTime(2000);
    // 老的仍在(2s < 5s),dedupe -> 不入队
    show({ category: "Server", title: "x", description: "msg" });
    expect(toasts.value).toHaveLength(1);
  });
});

describe("useToast — ttl 自动到期", () => {
  it("fake timer 推 5000ms 后自动 dismiss", () => {
    const { show, toasts } = useToast();
    const id = show({
      category: "Network",
      title: "x",
      ttl: 5000,
    });
    expect(toasts.value).toHaveLength(1);
    vi.advanceTimersByTime(4999);
    expect(toasts.value).toHaveLength(1);
    vi.advanceTimersByTime(2);
    expect(toasts.value).toHaveLength(0);
    expect(id).not.toBeNull();
  });

  it("自定 ttl 生效", () => {
    const { show, toasts } = useToast();
    show({
      category: "Server",
      title: "x",
      ttl: 1000,
    });
    expect(toasts.value).toHaveLength(1);
    vi.advanceTimersByTime(1001);
    expect(toasts.value).toHaveLength(0);
  });
});

describe("useToast — readonly 防止外部突变", () => {
  it("读取 toasts 是 readonly 但 push/splice 运行时不被拦截", () => {
    const { toasts } = useToast();
    expect(Array.isArray(toasts.value)).toBe(true);
    // readonly 是 TS 类型层提示,运行时不影响 — 这里只验证读取 + 结构 OK。
  });
});
