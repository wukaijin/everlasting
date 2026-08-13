// httpTransport tests(远程访问 Phase 2.3 C10)。
//
// Locks four invariants:
//   1. `invoke` → POST `/api/v1/{domain}/{cmd}`,args **顶层 key**
//      camelCase→snake_case(`requestId`→`request_id`),嵌套值原样透传
//      (Tauri/daemon 对 Rust struct 同 serde,前端读法一致)。
//   2. `invoke` 在未知 cmd / HTTP !ok 时抛 `TransportError`,透传 daemon
//      error body;空响应 body → `null`(chat / cancel_chat 返 `Json(())`)。
//   3. `listen` lazy 创建单全局 `EventSource(/api/v1/stream)` + 按 event
//      name 分发;unlisten 移除单个 handler(不 close EventSource)。
//   4. handler 可在分发中 unlisten 自己,不丢兄弟 handler(对应 http.ts
//      的 `[...handlers]` 复制遍历)。
//
// http.ts 持有 module-level 的 `handlersByEvent` / `eventSource`,故每个
// test 用 `vi.resetModules()` + 动态 `import("./http")` 拿干净 module,
// 避免跨 test 状态泄漏。

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

// ---------------------------------------------------------------------------
// mock 全局 fetch + EventSource
// ---------------------------------------------------------------------------
const fetchMock = vi.fn();
let lastFetchCall: { url: string; init: RequestInit } | null = null;

interface MockMessageEvent {
  data: string;
}

class MockEventSource {
  static last: MockEventSource | null = null;
  readonly url: string;
  private readonly listeners = new Map<
    string,
    Set<(e: MockMessageEvent) => void>
  >();
  onerror: ((e: unknown) => void) | null = null;

  constructor(url: string) {
    this.url = url;
    MockEventSource.last = this;
  }

  addEventListener(event: string, cb: (e: MockMessageEvent) => void): void {
    let set = this.listeners.get(event);
    if (!set) {
      set = new Set();
      this.listeners.set(event, set);
    }
    set.add(cb);
  }

  removeEventListener(event: string, cb: (e: MockMessageEvent) => void): void {
    this.listeners.get(event)?.delete(cb);
  }

  close(): void {
    /* no-op for tests */
  }

  /** Test helper:deliver a named SSE frame to every listener of `event`. */
  emit(event: string, data: unknown): void {
    const payload = typeof data === "string" ? data : JSON.stringify(data);
    for (const cb of this.listeners.get(event) ?? []) {
      cb({ data: payload });
    }
  }
}

beforeEach(async () => {
  vi.resetModules();
  // default:200 + empty body(→ invoke returns null)。具体 test 用
  // mockResolvedValueOnce 覆盖。
  fetchMock.mockImplementation(async (url: string, init?: RequestInit) => {
    lastFetchCall = { url, init: init ?? {} };
    return { ok: true, status: 200, text: async () => "" } as Response;
  });
  fetchMock.mockClear();
  lastFetchCall = null;
  vi.stubGlobal("fetch", fetchMock);
  vi.stubGlobal("EventSource", MockEventSource);
  MockEventSource.last = null;
  // daemonBase() 读 location.search —— 每个 test 起干净 URL。
  window.history.replaceState(null, "", "/");
});

afterEach(() => {
  vi.unstubAllGlobals();
});

async function loadTransport() {
  const mod = await import("./http");
  return mod.httpTransport;
}

describe("httpTransport.invoke", () => {
  it("POSTs to /api/v1/{domain}/{cmd} with camelCase→snake_case top-level keys", async () => {
    const t = await loadTransport();
    await t.invoke("chat", { requestId: "r1", sessionId: "s1" });

    expect(lastFetchCall?.url).toBe("http://localhost:7456/api/v1/agent/chat");
    expect(lastFetchCall?.init.method).toBe("POST");
    expect(lastFetchCall?.init.headers).toEqual({
      "Content-Type": "application/json",
    });
    expect(JSON.parse(lastFetchCall!.init.body as string)).toEqual({
      request_id: "r1",
      session_id: "s1",
    });
  });

  it("passes nested values through verbatim (no deep transform)", async () => {
    const t = await loadTransport();
    await t.invoke("chat", {
      requestId: "r1",
      messages: [{ role: "user", contentBlocks: [{ type: "text" }] }],
    });
    const body = JSON.parse(lastFetchCall!.init.body as string);
    expect(body.request_id).toBe("r1");
    // nested objects keep their camelCase keys untouched
    expect(body.messages).toEqual([
      { role: "user", contentBlocks: [{ type: "text" }] },
    ]);
  });

  it("throws TransportError(status 0) on unknown cmd (no domain mapping)", async () => {
    const t = await loadTransport();
    await expect(t.invoke("totally_unknown_cmd")).rejects.toMatchObject({
      name: "TransportError",
      status: 0,
    });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("throws TransportError with status + parsed body on HTTP !ok", async () => {
    fetchMock.mockResolvedValueOnce({
      ok: false,
      status: 403,
      json: async () => ({ kind: "PermissionDenied", message: "forbidden" }),
      text: async () => '{"kind":"PermissionDenied","message":"forbidden"}',
    } as Response);
    const t = await loadTransport();
    await expect(t.invoke("list_sessions")).rejects.toMatchObject({
      name: "TransportError",
      status: 403,
      body: { kind: "PermissionDenied", message: "forbidden" },
    });
  });

  it("returns null on empty body (chat / cancel_chat return Json(()))", async () => {
    const t = await loadTransport();
    const r = await t.invoke("cancel_chat", { sessionId: "s1" });
    expect(r).toBeNull();
  });

  it("returns parsed JSON on non-empty body", async () => {
    fetchMock.mockResolvedValueOnce({
      ok: true,
      status: 200,
      text: async () => '{"homeDir":"/home/c"}',
    } as Response);
    const t = await loadTransport();
    const r = await t.invoke("get_home_dir");
    expect(r).toEqual({ homeDir: "/home/c" });
  });

  it("honors ?daemonUrl= query for the base URL", async () => {
    window.history.replaceState(null, "", "/?daemonUrl=http://my-host:1234");
    const t = await loadTransport();
    await t.invoke("chat", { requestId: "r" });
    expect(lastFetchCall?.url).toBe("http://my-host:1234/api/v1/agent/chat");
  });

  it("strips trailing slashes from ?daemonUrl", async () => {
    window.history.replaceState(null, "", "/?daemonUrl=http://h:1////");
    const t = await loadTransport();
    await t.invoke("get_home_dir");
    expect(lastFetchCall?.url).toBe("http://h:1/api/v1/config/get_home_dir");
  });
});

describe("httpTransport.listen", () => {
  it("lazily creates a single EventSource on /api/v1/stream", async () => {
    const t = await loadTransport();
    expect(MockEventSource.last).toBeNull();
    await t.listen("chat-event", () => {});
    expect(MockEventSource.last).not.toBeNull();
    expect(MockEventSource.last?.url).toBe(
      "http://localhost:7456/api/v1/stream",
    );
  });

  it("dispatches a named SSE event to the handler as parsed payload", async () => {
    const t = await loadTransport();
    const received: unknown[] = [];
    await t.listen<{ x: number }>("chat-event", (p) => received.push(p));
    MockEventSource.last!.emit("chat-event", { x: 42 });
    expect(received).toEqual([{ x: 42 }]);
  });

  it("passes a raw string payload through when it is not JSON", async () => {
    const t = await loadTransport();
    const received: unknown[] = [];
    await t.listen("chat-event", (p) => received.push(p));
    MockEventSource.last!.emit("chat-event", "not-json{");
    expect(received).toEqual(["not-json{"]);
  });

  it("delivers to every handler registered under the same event name", async () => {
    const t = await loadTransport();
    const a: unknown[] = [];
    const b: unknown[] = [];
    await t.listen("tool:call", (p) => a.push(p));
    await t.listen("tool:call", (p) => b.push(p));
    MockEventSource.last!.emit("tool:call", { id: 1 });
    expect(a).toEqual([{ id: 1 }]);
    expect(b).toEqual([{ id: 1 }]);
  });

  it("does not cross-dispatch between different event names", async () => {
    const t = await loadTransport();
    const chat: unknown[] = [];
    const tool: unknown[] = [];
    await t.listen("chat-event", (p) => chat.push(p));
    await t.listen("tool:call", (p) => tool.push(p));
    MockEventSource.last!.emit("chat-event", { n: 1 });
    expect(chat).toEqual([{ n: 1 }]);
    expect(tool).toEqual([]);
  });

  it("unlisten removes only that handler (sibling + EventSource stay)", async () => {
    const t = await loadTransport();
    const a: unknown[] = [];
    const b: unknown[] = [];
    const unA = await t.listen("chat-event", (p) => a.push(p));
    await t.listen("chat-event", (p) => b.push(p));
    unA();
    MockEventSource.last!.emit("chat-event", { n: 1 });
    expect(a).toEqual([]);
    expect(b).toEqual([{ n: 1 }]);
  });

  it("a handler unlistening itself mid-dispatch does not skip its siblings", async () => {
    const t = await loadTransport();
    const seen: string[] = [];
    await t.listen("chat-event", () => seen.push("a"));
    const unB = await t.listen("chat-event", () => {
      seen.push("b");
      unB(); // remove self during dispatch
    });
    void unB;
    await t.listen("chat-event", () => seen.push("c"));

    MockEventSource.last!.emit("chat-event", {});
    // first round:all three run (snapshot copy); b removes itself
    expect(seen).toEqual(["a", "b", "c"]);

    MockEventSource.last!.emit("chat-event", {});
    // second round:b is gone
    expect(seen).toEqual(["a", "b", "c", "a", "c"]);
  });
});

// ---------------------------------------------------------------------------
// S4 pwa-remote 模式(design §2.2 / D3):localStorage 有 device token →
// invoke 加 `/api/v1/proxy` 前缀 + Bearer 头;EventSource 加 proxy + query。
// 无 token → 行为完全不变(回归)。401 + token → 清 token + 关 ES + 回调。
// ---------------------------------------------------------------------------
describe("httpTransport pwa-remote mode (device token)", () => {
  const TOKEN_KEY = "everlasting_device_token";

  beforeEach(() => {
    try {
      localStorage.removeItem(TOKEN_KEY);
    } catch {
      // jsdom always has localStorage; defensive.
    }
  });

  afterEach(() => {
    try {
      localStorage.removeItem(TOKEN_KEY);
    } catch {
      // fail silently
    }
  });

  it("with token: invoke URL gets /api/v1/proxy prefix + Authorization header", async () => {
    localStorage.setItem(TOKEN_KEY, "tok-123");
    const t = await loadTransport();
    await t.invoke("list_sessions");

    expect(lastFetchCall?.url).toBe(
      "http://localhost:7456/api/v1/proxy/api/v1/sessions/list_sessions",
    );
    expect(lastFetchCall?.init.method).toBe("POST");
    expect(lastFetchCall?.init.headers).toEqual({
      "Content-Type": "application/json",
      Authorization: "Bearer tok-123",
    });
    // body transform unaffected by token
    expect(JSON.parse(lastFetchCall!.init.body as string)).toEqual({});
  });

  it("without token: invoke URL has no proxy prefix and no Authorization (regression)", async () => {
    const t = await loadTransport();
    await t.invoke("list_sessions");

    expect(lastFetchCall?.url).toBe(
      "http://localhost:7456/api/v1/sessions/list_sessions",
    );
    expect(lastFetchCall?.init.headers).toEqual({
      "Content-Type": "application/json",
    });
  });

  it("with token: invoke still applies camelCase→snake_case top-level keys", async () => {
    localStorage.setItem(TOKEN_KEY, "tok");
    const t = await loadTransport();
    await t.invoke("chat", { requestId: "r1", sessionId: "s1" });

    expect(JSON.parse(lastFetchCall!.init.body as string)).toEqual({
      request_id: "r1",
      session_id: "s1",
    });
  });

  it("with token: EventSource URL has proxy prefix + access_token query (encoded)", async () => {
    localStorage.setItem(TOKEN_KEY, "tok secret"); // space exercises encoding
    const t = await loadTransport();
    await t.listen("chat-event", () => {});

    expect(MockEventSource.last?.url).toBe(
      "http://localhost:7456/api/v1/proxy/api/v1/stream?access_token=tok%20secret",
    );
  });

  it("without token: EventSource URL unchanged (regression)", async () => {
    const t = await loadTransport();
    await t.listen("chat-event", () => {});

    expect(MockEventSource.last?.url).toBe(
      "http://localhost:7456/api/v1/stream",
    );
  });

  it("on 401 + token: clears device token, closes EventSource, fires onAuthFailed, still throws", async () => {
    localStorage.setItem(TOKEN_KEY, "tok-123");
    const mod = await import("./http");
    const t = mod.httpTransport;

    const fired: number[] = [];
    mod.setOnAuthFailed(() => fired.push(1));

    // Start an EventSource first so resetEventSource has something to close.
    await t.listen("chat-event", () => {});
    const es = MockEventSource.last;
    expect(es).not.toBeNull();
    const closeSpy = vi.spyOn(es!, "close");

    fetchMock.mockResolvedValueOnce({
      ok: false,
      status: 401,
      json: async () => ({ kind: "Auth", message: "invalid token" }),
      text: async () => '{"kind":"Auth","message":"invalid token"}',
    } as Response);

    await expect(t.invoke("list_sessions")).rejects.toMatchObject({
      name: "TransportError",
      status: 401,
    });

    expect(localStorage.getItem(TOKEN_KEY)).toBeNull();
    expect(closeSpy).toHaveBeenCalled();
    expect(fired).toEqual([1]);

    mod.setOnAuthFailed(null);
  });

  it("on 401 without token: does NOT fire onAuthFailed (browser-local, no token to invalidate)", async () => {
    const mod = await import("./http");
    const t = mod.httpTransport;

    const fired: number[] = [];
    mod.setOnAuthFailed(() => fired.push(1));

    fetchMock.mockResolvedValueOnce({
      ok: false,
      status: 401,
      json: async () => ({ kind: "Auth", message: "no" }),
      text: async () => '{"kind":"Auth","message":"no"}',
    } as Response);

    await expect(t.invoke("list_sessions")).rejects.toMatchObject({
      status: 401,
    });
    expect(fired).toEqual([]);
    mod.setOnAuthFailed(null);
  });

  it("resetEventSource() closes the current EventSource and clears the ref", async () => {
    const t = await loadTransport();
    await t.listen("chat-event", () => {});
    const first = MockEventSource.last;
    const closeSpy = vi.spyOn(first!, "close");

    const mod = await import("./http");
    mod.resetEventSource();

    expect(closeSpy).toHaveBeenCalled();
    // next listen rebuilds a fresh EventSource (different instance)
    await t.listen("tool:call", () => {});
    expect(MockEventSource.last).not.toBe(first);
  });
});
