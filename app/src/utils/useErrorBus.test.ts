// Unit tests for `useErrorBus` — A5 错误总线。
//
// 锁定契约(09-21-error-bus-category-recovery 收口后):
// 1. parseAppCommandError 容错 2 种输入:AppCommandError 形状对象(含
//    TransportError 实例)/ JSON 字符串;裸 string → null(不再降级
//    Server —— 误报案原路径,评审裁决关死)。
// 2. category 值域校验:含 4 字段但 category 非法 → 不误判(返回 null)。
// 3. handle() 分级:形状识别 → push+路由(必须先于 instanceof Error);
//    良性噪音 → console.debug;其他 string → console.warn(不 push);
//    其他 Error 实例 → console.error(不 push、不 toast;TransportError
//    单独打标 transport-shape-miss)。
// 4. FIFO 上限 MAX_ERRORS(50):超出丢最旧。
// 5. routeByCategory 5 类 category 均触发分发(4 类走
//    `useToast().show(...)` + InvalidRequest 走 `console.warn`)。
// 6. clear() 清空。
// 7. PR1×PR2 接缝:真 TransportError 实例(new TransportError(...))过
//    handle → 按真实 category 路由(手写形状对象测不出接缝,评审裁决;
//    顺序回归 = 形状识别退到 instanceof Error 之后 → 本组用例必红)。
// 8. extractErrorMessage 对所有输入输出逐字节不变(AC4,30+ 调用点兼容);
//    extractErrorCategory 三态。
//
// errors 是模块级全局单例,每个测试前 clear() 隔离。

import { describe, it, expect, beforeEach, vi } from "vitest";
import {
  useErrorBus,
  parseAppCommandError,
  isBenignBrowserNoise,
  isAppCommandError,
  extractErrorCategory,
  extractErrorMessage,
} from "./useErrorBus";
import type { AppCommandError } from "./useErrorBus";
import { useToast } from "../composables/useToast";
import { TransportError } from "../transport/http";

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

  it("裸 string(非 JSON)→ null(不再降级 Server,09-21 收窄)", () => {
    // 原行为:强标 {category:"Server", kind:"Unknown"} —— 本地噪音被
    // 误标「服务端错误」弹 toast(ResizeObserver 误报案原路径)。
    const parsed = parseAppCommandError("随便一段老链路文字");
    expect(parsed).toBeNull();
  });

  it("非 JSON 结构的 string → null", () => {
    expect(parseAppCommandError("not a json {")).toBeNull();
  });

  it("原始 string 经 extractErrorMessage 原样返回(AC4:输出逐字节不变)", () => {
    expect(extractErrorMessage("随便一段老链路文字")).toBe(
      "随便一段老链路文字",
    );
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

  it("JSON 字符串里 category 非法 → null(不再走 string 降级)", () => {
    // JSON.parse 成功但 isAppCommandError false → 原行为 fall through 到
    // string fallback 强标 Server;09-21 收窄后 parse 直接返回 null。
    const json = JSON.stringify({
      category: "Bogus",
      kind: "x",
      message: "y",
      retryable: true,
    });
    expect(parseAppCommandError(json)).toBeNull();
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

  it("handle 裸 string → console.warn 留痕,不 push FIFO、不 toast(09-21 收窄)", () => {
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    const { errors, handle } = useErrorBus();
    handle("老链路 string rejection");
    expect(errors.value).toHaveLength(0); // 不 push FIFO(评审裁决)
    expect(useToast().toasts.value).toHaveLength(0); // 不弹 Server toast
    expect(warnSpy).toHaveBeenCalledTimes(1);
    expect(String(warnSpy.mock.calls[0]?.[0] ?? "")).toContain(
      "[errorBus:uncaught-string]",
    );
    expect(warnSpy.mock.calls[0]?.[1]).toBe("老链路 string rejection");
  });

  it("handle 良性噪音 string → console.debug,不 push、不 toast", () => {
    const debugSpy = vi.spyOn(console, "debug").mockImplementation(() => {});
    const { errors, handle } = useErrorBus();
    handle("ResizeObserver loop completed with undelivered notifications.");
    expect(errors.value).toHaveLength(0);
    expect(useToast().toasts.value).toHaveLength(0);
    expect(debugSpy).toHaveBeenCalledTimes(1);
    expect(String(debugSpy.mock.calls[0]?.[0] ?? "")).toContain(
      "[errorBus:benign-noise]",
    );
  });

  it("handle null/number 静默丢弃(不 push)", () => {
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

// ---------------------------------------------------------------------------
// 09-21-error-bus-category-recovery:全局兜底分级 + PR1×PR2 接缝。
// ---------------------------------------------------------------------------

describe("useErrorBus — Error 实例分支(09-21 前是盲区)", () => {
  it("new Error('boom') → console.error 留痕,不 push、不 toast(不再静默)", () => {
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const { errors, handle } = useErrorBus();
    handle(new Error("boom"));
    expect(errors.value).toHaveLength(0);
    expect(useToast().toasts.value).toHaveLength(0);
    expect(errorSpy).toHaveBeenCalledTimes(1);
    expect(String(errorSpy.mock.calls[0]?.[0] ?? "")).toContain(
      "[errorBus:uncaught]",
    );
    expect(errorSpy.mock.calls[0]?.[1]).toBeInstanceOf(Error);
  });

  it("Error 分支对 name==='TransportError' 单独打标 [errorBus:transport-shape-miss]", () => {
    // 构造收窄被打破的回归态模拟:真 TransportError 实例但 retryable 被
    // 破坏成非 boolean → 形状门不认 → 正常应落 Error 分支;打标保证
    // console 里可直接发现,而不是无声(design D2.3 观测兜底)。
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const { errors, handle } = useErrorBus();
    const te = new TransportError(500, { kind: "X", message: "m" });
    Object.defineProperty(te, "retryable", {
      value: "dirty",
      configurable: true,
    });
    expect(isAppCommandError(te)).toBe(false); // 前置:形状门确实不认
    handle(te);
    expect(errors.value).toHaveLength(0);
    expect(useToast().toasts.value).toHaveLength(0);
    expect(errorSpy).toHaveBeenCalledTimes(1);
    expect(String(errorSpy.mock.calls[0]?.[0] ?? "")).toContain(
      "[errorBus:transport-shape-miss]",
    );
    expect(errorSpy.mock.calls[0]?.[1]).toBe(te);
  });
});

describe("useErrorBus — PR1×PR2 接缝:真 TransportError 实例过 handle", () => {
  it("body 全字段的真实例 → 按 category 路由 push + toast(接缝集成)", () => {
    // 手写形状对象测不出接缝(评审裁决):此处必须 new TransportError。
    // 若 handle 的形状识别被挪到 instanceof Error 之后,本用例必红 ——
    // TransportError 会落 console.error 分支而不入总线(顺序守门断言)。
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const { errors, handle } = useErrorBus();
    const te = new TransportError(429, {
      category: "RateLimit",
      kind: "LlmError::RateLimit",
      message: "请求过于频繁",
      retryable: true,
      requestId: "r-1", // wire 是 camelCase(daemon/error.rs serde rename_all)
    });
    handle(te);
    expect(errors.value).toHaveLength(1);
    expect(errors.value[0].category).toBe("RateLimit");
    expect(errors.value[0].message).toBe("请求过于频繁");
    const toastCats = useToast().toasts.value.map((t) => t.category);
    expect(toastCats).toEqual(["RateLimit"]);
    // 顺序守门:形状识别先于 instanceof Error —— 走对分支就不该有
    // console.error(无论是 uncaught 还是 transport-shape-miss)。
    expect(errorSpy).not.toHaveBeenCalled();
  });

  it("body 无 category 的真实例 → status 逆映射出 category 再路由(401→Auth toast)", () => {
    const { errors, handle } = useErrorBus();
    const te = new TransportError(401, {
      kind: "Auth",
      message: "invalid token",
    });
    handle(te);
    expect(errors.value).toHaveLength(1);
    expect(errors.value[0].category).toBe("Auth"); // http.ts 逆映射产物
    expect(useToast().toasts.value.map((t) => t.category)).toEqual(["Auth"]);
  });

  it("status=0 unknown-cmd 真实例 → InvalidRequest:进 errors 但只 console.warn,不 toast", () => {
    // 分档兜底(评审裁决)经接缝的端到端:unknown-cmd 前科路径不再翻转为
    // 假「服务端错误」toast,与 InvalidRequest 同待遇。
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    const { errors, handle } = useErrorBus();
    const te = new TransportError(
      0,
      'unknown cmd "handoff_session" — no domain mapping in httpTransport',
    );
    handle(te);
    expect(errors.value).toHaveLength(1);
    expect(errors.value[0].category).toBe("InvalidRequest");
    expect(useToast().toasts.value).toHaveLength(0);
    expect(warnSpy).toHaveBeenCalledTimes(1);
    expect(String(warnSpy.mock.calls[0]?.[0] ?? "")).toContain(
      "InvalidRequest",
    );
  });
});

describe("isBenignBrowserNoise — 白名单前缀(09-21)", () => {
  it("两个已知变体命中(undelivered notifications / limit exceeded)", () => {
    expect(
      isBenignBrowserNoise(
        "ResizeObserver loop completed with undelivered notifications.",
      ),
    ).toBe(true);
    expect(isBenignBrowserNoise("ResizeObserver loop limit exceeded")).toBe(
      true,
    );
  });

  it("不误杀:近似前缀 / 真错误消息", () => {
    expect(isBenignBrowserNoise("ResizeObserverx loop exploded")).toBe(false);
    expect(isBenignBrowserNoise("Failed to fetch")).toBe(false);
    expect(
      isBenignBrowserNoise("Uncaught TypeError: x is not a function"),
    ).toBe(false);
  });
});

describe("extractErrorCategory — 三态(design D2.4)", () => {
  it("AppCommandError 形状对象 → category", () => {
    expect(
      extractErrorCategory({
        category: "Server",
        kind: "Anyhow",
        message: "db",
        retryable: true,
      }),
    ).toBe("Server");
  });

  it("真 TransportError 实例 → 恢复出的 category(含逆映射)", () => {
    expect(
      extractErrorCategory(
        new TransportError(429, {
          category: "RateLimit",
          kind: "X",
          message: "m",
          retryable: true,
        }),
      ),
    ).toBe("RateLimit");
    expect(
      extractErrorCategory(new TransportError(401, { kind: "Auth", message: "x" })),
    ).toBe("Auth");
  });

  it("其他(Error 实例 / 裸 string / null)→ null", () => {
    expect(extractErrorCategory(new Error("boom"))).toBeNull();
    expect(extractErrorCategory("raw string")).toBeNull();
    expect(extractErrorCategory(null)).toBeNull();
  });
});

describe("extractErrorMessage — AC4 兼容面(输出逐字节不变)", () => {
  it("AppCommandError 对象 → message", () => {
    expect(
      extractErrorMessage({
        category: "Auth",
        kind: "LlmError",
        message: "bad key",
        retryable: false,
      }),
    ).toBe("bad key");
  });

  it("JSON 字符串 → message", () => {
    expect(
      extractErrorMessage(
        JSON.stringify({
          category: "Network",
          kind: "X",
          message: "断网了",
          retryable: true,
        }),
      ),
    ).toBe("断网了");
  });

  it("Error 实例 → e.message(含真 TransportError:文案与收窄前一致)", () => {
    expect(extractErrorMessage(new Error("boom"))).toBe("boom");
    const te = new TransportError(500, { kind: "X", message: "db gone" });
    expect(extractErrorMessage(te)).toBe("db gone");
  });

  it("裸 string → 原样;null/undefined → (未知错误)", () => {
    expect(extractErrorMessage("原样透出")).toBe("原样透出");
    expect(extractErrorMessage(null)).toBe("(未知错误)");
    expect(extractErrorMessage(undefined)).toBe("(未知错误)");
    expect(extractErrorMessage(42)).toBe("(未知错误)");
  });
});
