// pathExistence — 09-14 存在性闸门的状态机单测。
//
// 覆盖:乐观 consult 默认态、200/404 落缓存、400/5xx/网络失败不落缓存
// (保持乐观)、inFlight 去重、positive 永久缓存、negative TTL 过期重查、
// 值未变不广播、通知微任务合并、相对路径无 pinia 不猜。
//
// 网络:vitest 默认禁网(见模块注释"测试隔离"),本文件用
// __enableNetworkForTests + vi.stubGlobal("fetch") 打开真链路;
// daemonBase/currentDeviceToken 模块级 mock 保确定性(与 imageUrl.test.ts
// 同构)。fake timers 只 fake Date(TTL 时间旅行),微任务/宏任务保持真实,
// fetch mock 的 promise 解析不受影响。

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

vi.mock("../transport/http", () => ({
  daemonBase: vi.fn(() => "http://localhost:7456"),
  // pathExistence → stores/chat → transport/index 在模块级取 httpTransport
  // 绑定(resolveTransport 只返回不调用),mock 必须补上这个导出。
  httpTransport: {},
}));
vi.mock("../transport/auth", () => ({
  currentDeviceToken: vi.fn(() => null),
}));

import {
  __enableNetworkForTests,
  onPathsResolved,
  pathKnownMissing,
  resetExistenceForTests,
  schedulePathCheck,
} from "./pathExistence";

/** 排空微任务(结果落地 + notify 合并都在微任务里)。 */
const flush = () => new Promise((r) => setTimeout(r, 0));

/** 真 stat 404 的哨兵 body(daemon `stat_file` 的 NotFound 臂)。 */
const STAT_NOT_FOUND = "stat: file not found";

function stubFetch(status: number, body?: string): ReturnType<typeof vi.fn> {
  const fn = vi.fn(async () => new Response(body ?? null, { status }));
  vi.stubGlobal("fetch", fn);
  return fn;
}

beforeEach(() => {
  resetExistenceForTests();
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("pathExistence", () => {
  it("fires no fetch in vitest until the network seam is enabled", async () => {
    // 测试隔离默认态:markdown/组件测试的乐观路径零网络副作用。
    const fn = vi.fn();
    vi.stubGlobal("fetch", fn);
    schedulePathCheck("/tmp/never-fetched/a.md");
    await flush();
    expect(fn).not.toHaveBeenCalled();
    expect(pathKnownMissing("/tmp/never-fetched/a.md")).toBe(false);
  });

  it("keeps the optimistic default until a 404 lands, then reports missing", async () => {
    __enableNetworkForTests();
    const fetchMock = stubFetch(404, STAT_NOT_FOUND);
    expect(pathKnownMissing("/tmp/gone/a.md")).toBe(false);
    schedulePathCheck("/tmp/gone/a.md");
    expect(fetchMock).toHaveBeenCalledWith(
      "http://localhost:7456/api/v1/files/stat?path=%2Ftmp%2Fgone%2Fa.md",
    );
    await flush();
    expect(pathKnownMissing("/tmp/gone/a.md")).toBe(true);
  });

  it("treats a sentinel-less 404 as unknown (stale daemon without /stat)", async () => {
    // 陈旧 daemon 的路由 fallback 404(无哨兵 body)≠ 文件不存在:
    // 按未知处理,否则旧 daemon + 新前端会杀光所有路径链接。
    __enableNetworkForTests();
    let notified = 0;
    const unsub = onPathsResolved(() => notified++);
    stubFetch(404); // 无 body
    schedulePathCheck("/tmp/stale/a.md");
    await flush();
    expect(pathKnownMissing("/tmp/stale/a.md")).toBe(false);
    expect(notified).toBe(0);
    unsub();
  });

  it("caches a positive result forever (one fetch, no TTL re-check)", async () => {
    __enableNetworkForTests();
    const fetchMock = stubFetch(200);
    schedulePathCheck("/tmp/keep/a.md");
    await flush();
    expect(pathKnownMissing("/tmp/keep/a.md")).toBe(false);
    schedulePathCheck("/tmp/keep/a.md");
    schedulePathCheck("/tmp/keep/a.md");
    await flush();
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("dedupes concurrent checks for the same path (in-flight set)", async () => {
    __enableNetworkForTests();
    let release!: () => void;
    const fetchMock = vi.fn(
      () =>
        new Promise<Response>((r) =>
          (release = () => r(new Response(STAT_NOT_FOUND, { status: 404 }))),
        ),
    );
    vi.stubGlobal("fetch", fetchMock);
    schedulePathCheck("/tmp/slow/a.md");
    schedulePathCheck("/tmp/slow/a.md");
    schedulePathCheck("/tmp/slow/a.md");
    expect(fetchMock).toHaveBeenCalledTimes(1);
    release();
    await flush();
    expect(pathKnownMissing("/tmp/slow/a.md")).toBe(true);
  });

  it("treats 5xx / network failure as unknown (no cache write, optimistic stays)", async () => {
    __enableNetworkForTests();
    const fetchMock = stubFetch(500);
    schedulePathCheck("/tmp/err/a.md");
    await flush();
    expect(pathKnownMissing("/tmp/err/a.md")).toBe(false);
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new TypeError("fetch failed");
      }),
    );
    schedulePathCheck("/tmp/err/b.md");
    await flush();
    expect(pathKnownMissing("/tmp/err/b.md")).toBe(false);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("re-checks a stale negative after the TTL (file may have been created)", async () => {
    __enableNetworkForTests();
    const fetchMock = stubFetch(404, STAT_NOT_FOUND);
    schedulePathCheck("/tmp/late/a.md");
    await flush();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    // TTL 内:重复 schedule 不再发请求。
    schedulePathCheck("/tmp/late/a.md");
    await flush();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(pathKnownMissing("/tmp/late/a.md")).toBe(true);
    // 时间旅行 16s:下一次渲染 schedule 重查(仍是 404)。
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(Date.now() + 16_000);
    schedulePathCheck("/tmp/late/a.md");
    await flush();
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(pathKnownMissing("/tmp/late/a.md")).toBe(true);
  });

  it("notifies listeners once per microtask batch, only on value changes", async () => {
    __enableNetworkForTests();
    const calls: string[][] = [];
    const unsubscribe = onPathsResolved((raws) => calls.push(raws));
    stubFetch(404, STAT_NOT_FOUND);
    schedulePathCheck("/tmp/batch/a.md");
    schedulePathCheck("/tmp/batch/b.md");
    await flush();
    expect(calls).toEqual([[ "/tmp/batch/a.md", "/tmp/batch/b.md" ]]);
    // 广播之外,负面结果本身也要落缓存(防"因错而绿":值算反时广播
    // 仍会发生,这里锁死方向)。
    expect(pathKnownMissing("/tmp/batch/a.md")).toBe(true);
    // TTL 过期后的重查若值未变(404→404):刷新 checkedAt,不再广播。
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(Date.now() + 16_000);
    schedulePathCheck("/tmp/batch/a.md");
    await flush();
    expect(calls).toHaveLength(1);
    unsubscribe();
    schedulePathCheck("/tmp/batch/c.md");
    await flush();
    expect(calls).toHaveLength(1);
  });

  it("does not guess for relative paths without an active pinia", async () => {
    __enableNetworkForTests();
    const fetchMock = stubFetch(200);
    schedulePathCheck("out/relative/a.md");
    await flush();
    expect(fetchMock).not.toHaveBeenCalled();
    expect(pathKnownMissing("out/relative/a.md")).toBe(false);
  });
});
