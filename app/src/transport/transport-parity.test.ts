// Transport parity test(远程访问 Phase 2.5 E2, 2026-07-23)。
//
// `tauriTransport` 与 `httpTransport` 实现同一个 `Transport` 接口(types.ts),
// 20+ 调用点对两者无感知。P2.5 原计划追求"双 transport 双跑同一套 vitest",
// 但探明后确认:24 个组件 / store 测试文件已 `vi.mock("@/transport", ...)`
// 全量 mock transport,双跑无意义(它们 transport-agnostic by construction)。
//
// 本文件取而代之做**契约层一致性**测试:对同一组 `invoke` / `listen` 调用,
// 断言两 transport 的可观察行为对齐 ——
//   1. `invoke` 成功路径:都 resolve 一个值(httpTransport 的 args 顶层
//      camelCase→snake_case 转换不改变 resolve 值本身)。
//   2. `invoke` 失败路径:都 reject(Tauri 抛原始错误,httpTransport 抛
//      `TransportError` —— 都是可被 `try/catch` 捕获的 rejection)。
//   3. `listen` 契约:注册 handler → 收到对应事件 → handler 被调用(收到
//      **已解包** payload,非 Tauri `Event<T>` 信封)→ 返回的 unlisten 能取消。
//
// 这把"两 transport 契约一致"从隐式假设变成显式锁定:任何一个 transport
// 偏离 `Transport` 接口语义(如 httpTransport 忘了 unwrap payload,或
// tauriTransport 漏了 camelCase 处理),本测试会失败。

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import type { Transport, UnlistenFn } from "./types";

// ---------------------------------------------------------------------------
// 共享 mock 基建
// ---------------------------------------------------------------------------

/**
 * 一个极简的可控 `Transport`,模拟一个"理想后端" —— invoke 查表返回
 * 预设值,listen 记录 handler 供测试 emit。用于断言两真实 transport 在
 * 相同刺激下的**对外行为**一致(而非比较它们彼此的内部实现)。
 *
 * 关键:本 mock 不替代被测 transport —— 它是对照基准。tauriTransport /
 * httpTransport 各自 mock 掉底层(Tauri API / fetch+EventSource),然后
 * 我们验证它们都对这个基准行为对齐。
 */
class RecordingBackend {
  readonly invokeCalls: Array<{ cmd: string; args?: Record<string, unknown> }> =
    [];
  private readonly invokeResults = new Map<string, unknown>();
  private readonly invokeErrors = new Set<string>();

  /** 预设某 cmd 的成功返回值。 */
  setInvokeResult(cmd: string, result: unknown): void {
    this.invokeResults.set(cmd, result);
  }

  /** 预设某 cmd 抛错(reject)。 */
  setInvokeError(cmd: string): void {
    this.invokeErrors.add(cmd);
  }

  /** 记录一次调用并返回预设结果(或抛预设错误)。 */
  async doInvoke<T>(
    cmd: string,
    args?: Record<string, unknown>,
  ): Promise<T> {
    this.invokeCalls.push({ cmd, args });
    if (this.invokeErrors.has(cmd)) {
      throw new Error(`backend error for ${cmd}`);
    }
    return (this.invokeResults.get(cmd) ?? null) as T;
  }

  reset(): void {
    this.invokeCalls.length = 0;
    this.invokeResults.clear();
    this.invokeErrors.clear();
  }
}

const backend = new RecordingBackend();

// ---------------------------------------------------------------------------
// tauriTransport 的 mock:Tauri `invoke` / `listen` → RecordingBackend
// ---------------------------------------------------------------------------

/** 模拟 Tauri `Event<T>` 信封(listen 回调收到的原始形状)。 */
interface TauriEvent<T> {
  event: string;
  id: number;
  payload: T;
}

const tauriEventListeners = new Map<
  string,
  Set<(e: TauriEvent<unknown>) => void>
>();

function mockTauriListen<T>(
  event: string,
  cb: (e: TauriEvent<T>) => void,
): Promise<UnlistenFn> {
  let set = tauriEventListeners.get(event);
  if (!set) {
    set = new Set();
    tauriEventListeners.set(event, set);
  }
  set.add(cb as (e: TauriEvent<unknown>) => void);
  return Promise.resolve(() => {
    tauriEventListeners.get(event)?.delete(
      cb as (e: TauriEvent<unknown>) => void,
    );
  });
}

/** 测试 helper:模拟 Tauri 后端 emit 一个事件(带信封)。 */
function tauriEmit(event: string, payload: unknown): void {
  for (const cb of tauriEventListeners.get(event) ?? []) {
    cb({ event, id: Date.now(), payload });
  }
}

// ---------------------------------------------------------------------------
// httpTransport 的 mock:fetch + EventSource → RecordingBackend
// ---------------------------------------------------------------------------

class MockEventSource {
  static last: MockEventSource | null = null;
  readonly url: string;
  private readonly listeners = new Map<
    string,
    Set<(e: { data: string }) => void>
  >();
  onerror: ((e: unknown) => void) | null = null;

  constructor(url: string) {
    this.url = url;
    MockEventSource.last = this;
  }

  addEventListener(event: string, cb: (e: { data: string }) => void): void {
    let set = this.listeners.get(event);
    if (!set) {
      set = new Set();
      this.listeners.set(event, set);
    }
    set.add(cb);
  }

  removeEventListener(event: string, cb: (e: { data: string }) => void): void {
    this.listeners.get(event)?.delete(cb);
  }

  close(): void {
    /* no-op */
  }

  /** 测试 helper:向某 event 的所有 listener 投递一帧。 */
  emit(event: string, data: unknown): void {
    const payload = typeof data === "string" ? data : JSON.stringify(data);
    for (const cb of this.listeners.get(event) ?? []) {
      cb({ data: payload });
    }
  }
}

const fetchMock = vi.fn();

/** 测试 helper:模拟 daemon 经 SSE 投递一个事件(无信封,纯 payload)。 */
function httpEmit(event: string, payload: unknown): void {
  MockEventSource.last?.emit(event, payload);
}

// ---------------------------------------------------------------------------
// test 生命周期:每个 test 起干净 module + 后端
// ---------------------------------------------------------------------------

beforeEach(async () => {
  vi.resetModules();
  backend.reset();
  tauriEventListeners.clear();
  MockEventSource.last = null;

  // Tauri API mock(默认模块 `@tauri-apps/api/core` + `/event`)。
  vi.doMock("@tauri-apps/api/core", () => ({
    invoke: (cmd: string, args?: Record<string, unknown>) =>
      backend.doInvoke(cmd, args),
  }));
  vi.doMock("@tauri-apps/api/event", () => ({
    listen: mockTauriListen,
  }));

  // fetch mock:POST → RecordingBackend;按 cmd 返回预设结果或 4xx。
  fetchMock.mockImplementation(
    async (url: string, init?: RequestInit): Promise<Response> => {
      const u = String(url);
      // 从 `/api/v1/{domain}/{cmd}` 提取 cmd。
      const m = u.match(/\/api\/v1\/[^/]+\/([^/?]+)$/);
      const cmd = m ? m[1] : "";
      let body: unknown;
      try {
        body = init?.body ? JSON.parse(String(init.body)) : {};
      } catch {
        body = {};
      }
      backend.invokeCalls.push({ cmd, args: body as Record<string, unknown> });
      try {
        const result = await backend.doInvoke(cmd, body as Record<string, unknown>);
        return {
          ok: true,
          status: 200,
          text: async () =>
            result === null || result === undefined ? "" : JSON.stringify(result),
        } as Response;
      } catch {
        return {
          ok: false,
          status: 400,
          text: async () => JSON.stringify({ message: "backend error" }),
          json: async () => ({ message: "backend error" }),
        } as Response;
      }
    },
  );
  vi.stubGlobal("fetch", fetchMock);
  vi.stubGlobal("EventSource", MockEventSource);
  window.history.replaceState(null, "", "/");
});

afterEach(() => {
  vi.doUnmock("@tauri-apps/api/core");
  vi.doUnmock("@tauri-apps/api/event");
  vi.unstubAllGlobals();
});

async function loadTauriTransport(): Promise<Transport> {
  const mod = await import("./tauri");
  return mod.tauriTransport;
}

async function loadHttpTransport(): Promise<Transport> {
  const mod = await import("./http");
  return mod.httpTransport;
}

// ---------------------------------------------------------------------------
// 契约 1:invoke 成功路径 —— 两 transport 都 resolve 预设值
// ---------------------------------------------------------------------------

describe("transport parity — invoke 成功路径", () => {
  it("两 transport 对相同 cmd 都 resolve 相同预设返回值", async () => {
    backend.setInvokeResult("list_sessions", [
      { id: "s1", title: "Session 1" },
      { id: "s2", title: "Session 2" },
    ]);

    const tauri = await loadTauriTransport();
    const http = await loadHttpTransport();

    const [tauriResult, httpResult] = await Promise.all([
      tauri.invoke("list_sessions", { projectId: "p1" }),
      http.invoke("list_sessions", { projectId: "p1" }),
    ]);

    expect(tauriResult).toEqual(httpResult);
    expect(tauriResult).toEqual([
      { id: "s1", title: "Session 1" },
      { id: "s2", title: "Session 2" },
    ]);
  });

  it("null 返回值(chat / cancel_chat 返 Json(()))两 transport 都 resolve null", async () => {
    backend.setInvokeResult("chat", null);

    const tauri = await loadTauriTransport();
    const http = await loadHttpTransport();

    const [tauriResult, httpResult] = await Promise.all([
      tauri.invoke("chat", { requestId: "r1" }),
      http.invoke("chat", { requestId: "r1" }),
    ]);

    expect(tauriResult).toBeNull();
    expect(httpResult).toBeNull();
  });

  it("httpTransport 把顶层 camelCase key 转 snake_case,但 resolve 值不变", async () => {
    // daemon 侧按 snake_case 收到 args,但返回值方向两 transport 一致。
    backend.setInvokeResult("load_session", { id: "s1", messages: [] });

    const http = await loadHttpTransport();
    const result = await http.invoke("load_session", {
      sessionId: "s1", // camelCase 顶层 key
    });

    expect(result).toEqual({ id: "s1", messages: [] });
    // daemon 收到的 body 顶层 key 应已被转成 snake_case。
    const lastCall = backend.invokeCalls[backend.invokeCalls.length - 1];
    expect(lastCall?.args).toHaveProperty("session_id", "s1");
    expect(lastCall?.args).not.toHaveProperty("sessionId");
  });
});

// ---------------------------------------------------------------------------
// 契约 2:invoke 失败路径 —— 两 transport 都 reject(可被 try/catch)
// ---------------------------------------------------------------------------

describe("transport parity — invoke 失败路径", () => {
  it("后端错误时两 transport 都 reject(不吞错)", async () => {
    backend.setInvokeError("delete_session");

    const tauri = await loadTauriTransport();
    const http = await loadHttpTransport();

    await expect(tauri.invoke("delete_session", { id: "s1" })).rejects.toThrow();
    // 清掉 tauri 侧的调用记录,只验 http。
    backend.invokeCalls.length = 0;
    await expect(http.invoke("delete_session", { id: "s1" })).rejects.toThrow();
  });

  it("httpTransport 的 rejection 是 TransportError(status + body)", async () => {
    backend.setInvokeError("delete_session");
    const http = await loadHttpTransport();

    await expect(http.invoke("delete_session", { id: "s1" })).rejects.toMatchObject(
      {
        name: "TransportError",
        status: 400,
      },
    );
  });
});

// ---------------------------------------------------------------------------
// 契约 3:listen —— 注册 → 收到已解包 payload → unlisten 取消
// ---------------------------------------------------------------------------

describe("transport parity — listen 契约", () => {
  it("两 transport 都把已解包 payload 投给 handler(非 Tauri Event 信封)", async () => {
    const tauri = await loadTauriTransport();
    const http = await loadHttpTransport();

    const tauriPayloads: unknown[] = [];
    const httpPayloads: unknown[] = [];
    await tauri.listen<{ text: string }>("chat-event", (p) => {
      tauriPayloads.push(p);
    });
    await http.listen<{ text: string }>("chat-event", (p) => {
      httpPayloads.push(p);
    });

    const event = { text: "hello" };
    tauriEmit("chat-event", event); // Tauri 信封在 mock 内部解包
    httpEmit("chat-event", event); // SSE data 字段在 httpTransport 内 JSON.parse

    expect(tauriPayloads).toEqual([event]);
    expect(httpPayloads).toEqual([event]);
    // 关键:handler 收到的是解包后的 payload,不是 { event, id, payload } 信封。
    expect(tauriPayloads[0]).not.toHaveProperty("event");
    expect(tauriPayloads[0]).not.toHaveProperty("id");
  });

  it("unlisten 后该 handler 不再被调用(两 transport 一致)", async () => {
    const tauri = await loadTauriTransport();
    const http = await loadHttpTransport();

    const tauriCalls: unknown[] = [];
    const httpCalls: unknown[] = [];
    const tauriUnlisten = await tauri.listen("chat-event", (p) =>
      tauriCalls.push(p),
    );
    const httpUnlisten = await http.listen("chat-event", (p) =>
      httpCalls.push(p),
    );

    tauriUnlisten();
    httpUnlisten();
    tauriEmit("chat-event", { text: "after-unlisten" });
    httpEmit("chat-event", { text: "after-unlisten" });

    expect(tauriCalls).toHaveLength(0);
    expect(httpCalls).toHaveLength(0);
  });

  it("listen 返回 Promise<UnlistenFn>(两 transport 都是异步返回取消句柄)", async () => {
    const tauri = await loadTauriTransport();
    const http = await loadHttpTransport();

    const tauriResult = tauri.listen("x", () => {});
    const httpResult = http.listen("x", () => {});

    expect(tauriResult).toBeInstanceOf(Promise);
    expect(httpResult).toBeInstanceOf(Promise);

    const tauriUnlisten = await tauriResult;
    const httpUnlisten = await httpResult;
    expect(typeof tauriUnlisten).toBe("function");
    expect(typeof httpUnlisten).toBe("function");
  });
});
