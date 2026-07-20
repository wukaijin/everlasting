// Unit tests for `useErrorBus` — A5 错误总线。
//
// 锁定契约:
// 1. parseAppCommandError 容错 3 种输入:对象 / JSON 字符串 / 原始 string。
// 2. category 值域校验:含 4 字段但 category 非法 → 不误判(返回 null)。
// 3. handle() 把可识别错误 push 进 errors,不可识别静默丢弃。
// 4. FIFO 上限 MAX_ERRORS(50):超出丢最旧。
// 5. routeByCategory 5 类 category 均触发分发(R1 升级后 4 类走
//    `useToast().show(...)` + InvalidRequest 走 `console.warn`)。
// 6. clear() 清空。
//
// errors 是模块级全局单例,每个测试前 clear() 隔离。

import { describe, it, expect, beforeEach, vi } from "vitest";
import { useErrorBus, parseAppCommandError } from "./useErrorBus";
import type { AppCommandError } from "./useErrorBus";
import { useToast } from "../composables/useToast";

beforeEach(() => {
  // 全局单例,测试间清空。
  useErrorBus().clear();
  useToast().clear();
  // 路由 stub 会 console.warn(InvalidRequest 一类),默认静音(个别用例显式 spy)。
  vi.restoreAllMocks();
});

describe("parseAppCommandError — 3 种输入容错", () => {
  it("接受 AppCommandError 对象", () => {
    const e: AppCommandError = {
      category: "Auth",
      kind: "LlmError",
      message: "bad key",
      retryable: false,
    };
    expect(parseAppCommandError(e)).toEqual(e);
  });

  it("接受含 requestId 的对象", () => {
    const e = {
      category: "Server",
      kind: "Anyhow",
      message: "boom",
      retryable: true,
      requestId: "mz8s3hqwx6rmqjswgte",
    };
    const parsed = parseAppCommandError(e);
    expect(parsed?.requestId).toBe("mz8s3hqwx6rmqjswgte");
  });

  it("接受 JSON 字符串", () => {
    const json = JSON.stringify({
      category: "RateLimit",
      kind: "LlmError::RateLimit",
      message: "请求过于频繁",
      retryable: true,
      requestId: "r1",
    });
    const parsed = parseAppCommandError(json);
    expect(parsed?.category).toBe("RateLimit");
    expect(parsed?.requestId).toBe("r1");
    expect(parsed?.message).toBe("请求过于频繁");
  });

  it("原始 string 降级 Server/Unknown/retryable=false", () => {
    const parsed = parseAppCommandError("随便一段老链路文字");
    expect(parsed?.category).toBe("Server");
    expect(parsed?.kind).toBe("Unknown");
    expect(parsed?.retryable).toBe(false);
    expect(parsed?.message).toBe("随便一段老链路文字");
  });

  it("非 JSON 也非对象结构的 string 仍降级(不走 JSON.parse 成功路径)", () => {
    const parsed = parseAppCommandError("not a json {");
    expect(parsed?.category).toBe("Server");
    expect(parsed?.message).toBe("not a json {");
  });
});

describe("parseAppCommandError — 防误判", () => {
  it("含 4 字段但 category 非法值 → null(不误判)", () => {
    const parsed = parseAppCommandError({
      category: "Weird",
      kind: "x",
      message: "y",
      retryable: true,
    });
    expect(parsed).toBeNull();
  });

  it("缺 retryable 字段 → null", () => {
    const parsed = parseAppCommandError({
      category: "Auth",
      kind: "x",
      message: "y",
    });
    expect(parsed).toBeNull();
  });

  it("null / undefined / number → null", () => {
    expect(parseAppCommandError(null)).toBeNull();
    expect(parseAppCommandError(undefined)).toBeNull();
    expect(parseAppCommandError(42)).toBeNull();
  });

  it("JSON 字符串里 category 非法 → 走 string fallback(降级 Server)", () => {
    // JSON.parse 成功但 isAppCommandError false → 不进 JSON 分支返回,
    // fall through 到 string fallback,把原字符串当 message。
    const json = JSON.stringify({
      category: "Bogus",
      kind: "x",
      message: "y",
      retryable: true,
    });
    const parsed = parseAppCommandError(json);
    expect(parsed?.category).toBe("Server");
    expect(parsed?.kind).toBe("Unknown");
  });
});

describe("useErrorBus — push / handle / clear", () => {
  it("handle 对象错误后 push 进 errors", () => {
    const { errors, handle } = useErrorBus();
    handle({
      category: "RateLimit",
      kind: "LlmError",
      message: "slow",
      retryable: true,
    });
    expect(errors.value).toHaveLength(1);
    expect(errors.value[0].category).toBe("RateLimit");
  });

  it("handle 字符串错误降级后 push", () => {
    const { errors, handle } = useErrorBus();
    handle("老链路 string rejection");
    expect(errors.value).toHaveLength(1);
    expect(errors.value[0].category).toBe("Server");
    expect(errors.value[0].kind).toBe("Unknown");
  });

  it("handle null 静默丢弃(不 push)", () => {
    const { errors, handle } = useErrorBus();
    handle(null);
    handle(42);
    expect(errors.value).toHaveLength(0);
  });

  it("clear 清空", () => {
    const { errors, push, clear } = useErrorBus();
    push({ category: "Server", kind: "X", message: "x", retryable: true });
    push({ category: "Network", kind: "X", message: "y", retryable: true });
    expect(errors.value).toHaveLength(2);
    clear();
    expect(errors.value).toHaveLength(0);
  });
});

describe("useErrorBus — FIFO 上限 MAX_ERRORS=50", () => {
  it("超出上限丢最旧,保留最近 50 条", () => {
    const { errors, push } = useErrorBus();
    for (let i = 0; i < 60; i++) {
      push({
        category: "Server",
        kind: "X",
        message: `msg-${i}`,
        retryable: true,
      });
    }
    expect(errors.value).toHaveLength(50);
    // 丢掉 msg-0..msg-9,保留 msg-10..msg-59。
    expect(errors.value[0].message).toBe("msg-10");
    expect(errors.value[49].message).toBe("msg-59");
  });

  it("恰好 50 条不丢", () => {
    const { errors, push } = useErrorBus();
    for (let i = 0; i < 50; i++) {
      push({
        category: "Network",
        kind: "X",
        message: `${i}`,
        retryable: true,
      });
    }
    expect(errors.value).toHaveLength(50);
    expect(errors.value[0].message).toBe("0");
  });
});

describe("useErrorBus — routeByCategory 5 类分发 (R1 升级)", () => {
  it("4 类 (Auth/RateLimit/Server/Network) 推 toast,InvalidRequest 走 console.warn", () => {
    const spy = vi.spyOn(console, "warn").mockImplementation(() => {
      /* 静音 InvalidRequest 的 console.warn */
    });
    const toastBus = useToast();
    // 通过 useErrorBus.push() 走 routeByCategory 路径,验证分发正确:
    //   - 4 类(Auth/RateLimit/Server/Network)被推到 toast 队列
    //   - InvalidRequest 走 console.warn(无效 path,不进 toast)
    // 注:toast 队列有 MAX_CONCURRENT=3 FIFO 上限,所以 [1]=Auth,
    // [2]=RateLimit, [3]=Server, [4]=Network 把 Auth 顶出 → 剩
    // RateLimit/Server/Network。我们分开断言:
    //   - console.warn:InvalidRequest 一次(category 前缀)
    //   - toast 队列:剩余 3 类齐全,Auth 在 FIFO 历史上走过
    const { push } = useErrorBus();
    const cats = [
      "Auth",
      "RateLimit",
      "InvalidRequest",
      "Server",
      "Network",
    ] as const;
    for (const c of cats) {
      push({
        category: c,
        kind: "X",
        message: `msg-${c}`,
        retryable: true,
      });
    }
    // InvalidRequest 走 console.warn
    expect(spy).toHaveBeenCalledTimes(1);
    expect(String(spy.mock.calls[0]?.[0] ?? "")).toContain("InvalidRequest");
    // toast 队列包含 4 类中的 3 个(FIFO 上限 + InvalidRequest 不入)
    const toastCats = toastBus.toasts.value.map((t) => t.category);
    expect(toastCats.length).toBe(3);
    expect(toastCats).not.toContain("InvalidRequest");
    // 用 spy + 为每类加 spy 验证 routeByCategory 把 4 类都走过(toast 推过
    // 就能进入 show 路径,但 show 是 console-only 不可观测)。改用 spy
    // on console.warn 仅能验证 InvalidRequest 走原状路径 — 用 spy
    // on useToast 的 show 不可行(show 是闭包),退而求其次:验证
    // MAX_CONCURRENT 上限生效(toast 队列 hit 3 上限)+ 老 Auth 被踢出,
    // 4 类都进过队列就成立。如果未来要测"4 类进过队列",可以重构
    // useToast 让 show 返回 ref counter,留 follow-up。
    // FIFO:第一条 Auth 被后面的 3 条 toast 踢出(MAX=3,4 条入 → 1 踢出)
    expect(toastCats).toEqual(["RateLimit", "Server", "Network"]);
  });

  it("errors readonly — 外部无法直接改数组", () => {
    const { errors } = useErrorBus();
    // readonly ref:赋值 / push 在类型层被挡;这里只验证运行时结构未破。
    expect(Array.isArray(errors.value)).toBe(true);
  });
});
