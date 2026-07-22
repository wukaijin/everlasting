// health handshake tests (P2.4 D3.3, 2026-07-22).
//
// Locks the Q5 layered validation + the poll-until-ready behavior:
//   1. protocol mismatch (api_versions lacks "v1") → rejects immediately
//      with a Chinese user-facing message (no retry).
//   2. build drift (daemon_version ≠ app) → warn-only, resolves with health.
//   3. non-200 / network error → keeps polling until success or timeout.
//   4. timeout → rejects with a timeout message containing the base URL.
//   5. happy path → resolves with the validated DaemonHealth.
//
// Uses an injected `fetchImpl` + `base` so no real network / global
// state is touched. `awaitDaemonHealthy` reads `daemonBase()` only when
// `base` is omitted, so passing `base` explicitly short-circuits the
// module-level resolver entirely.
//
// We use REAL timers (not fake) with tiny intervals (5ms poll / ~120ms
// timeout) — the mock fetch resolves synchronously-enough that the
// whole suite runs in well under a second, and real timers avoid the
// `await promise` × `advanceTimersByTimeAsync` race that fake timers
// introduce for poll loops.

import { describe, it, expect, vi } from "vitest";
import {
  awaitDaemonHealthy,
  EXPECTED_API_VERSION,
  type DaemonHealth,
} from "./health";

const BASE = "http://test-daemon:9999";

function makeHealth(overrides: Partial<DaemonHealth> = {}): DaemonHealth {
  return {
    daemonId: "test-id",
    daemonVersion: "0.1.0",
    apiVersions: ["v1"],
    uptimeSeconds: 100,
    sessionCount: 3,
    ...overrides,
  };
}

/** Build a mock fetch that returns a sequence of responses (one per
 *  call). The Nth call returns responses[N]; calls beyond the sequence
 *  throw (test bug — we should have provided enough responses). */
function fetchSequence(
  responses: Array<
    | { ok: true; health: DaemonHealth }
    | { ok: false; status: number; body?: unknown }
  >,
): { fetchImpl: typeof fetch; calls: number } {
  let calls = 0;
  const fetchImpl = vi.fn(async () => {
    const r = responses[calls];
    calls++;
    if (!r) throw new Error("fetchSequence exhausted (test bug)");
    if (r.ok) {
      return {
        ok: true,
        status: 200,
        json: async () => r.health,
      } as unknown as Response;
    }
    return {
      ok: false,
      status: r.status,
      json: async () => r.body ?? {},
    } as unknown as Response;
  }) as unknown as typeof fetch;
  return { fetchImpl, get calls() { return calls; } };
}

/** Build a mock fetch that always rejects the fetch promise (network
 *  error — daemon not up yet). */
function fetchAlwaysError(): typeof fetch {
  return vi.fn(async () => {
    throw new Error("connect ECONNREFUSED");
  }) as unknown as typeof fetch;
}

describe("awaitDaemonHealthy", () => {
  it("happy path: resolves on first 200 with valid api_versions", async () => {
    const health = makeHealth();
    const { fetchImpl } = fetchSequence([{ ok: true, health }]);
    const result = await awaitDaemonHealthy({
      base: BASE,
      fetchImpl,
      timeoutMs: 1000,
      intervalMs: 5,
    });
    expect(result).toEqual(health);
  });

  it("protocol mismatch (no v1): rejects immediately without retry", async () => {
    let calls = 0;
    const fetchImpl = vi.fn(async () => {
      calls++;
      return {
        ok: true,
        status: 200,
        json: async () => makeHealth({ apiVersions: ["v2"] }),
      } as unknown as Response;
    }) as unknown as typeof fetch;
    await expect(
      awaitDaemonHealthy({
        base: BASE,
        fetchImpl,
        timeoutMs: 5000,
        intervalMs: 5,
      }),
    ).rejects.toThrow(/协议版本不兼容/);
    // Must NOT have retried (protocol mismatch is a hard fail).
    expect(calls).toBe(1);
    // Sanity: the expected-version constant flows through correctly.
    expect(EXPECTED_API_VERSION).toBe("v1");
  });

  it("build drift: warns but resolves (warn-only, not a hard fail)", async () => {
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    const health = makeHealth({ daemonVersion: "9.9.9-different" });
    const { fetchImpl } = fetchSequence([{ ok: true, health }]);
    const result = await awaitDaemonHealthy({
      base: BASE,
      fetchImpl,
      timeoutMs: 1000,
      intervalMs: 5,
    });
    expect(result).toEqual(health);
    expect(warnSpy).toHaveBeenCalledWith(
      expect.stringMatching(/build version drift/),
    );
    warnSpy.mockRestore();
  });

  it("matching build version: no drift warning", async () => {
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    // __APP_VERSION__ is "0.1.0" per package.json; daemon matches.
    const health = makeHealth({ daemonVersion: "0.1.0" });
    const { fetchImpl } = fetchSequence([{ ok: true, health }]);
    await awaitDaemonHealthy({
      base: BASE,
      fetchImpl,
      timeoutMs: 1000,
      intervalMs: 5,
    });
    expect(warnSpy).not.toHaveBeenCalledWith(
      expect.stringMatching(/build version drift/),
    );
    warnSpy.mockRestore();
  });

  it("non-200 then 200: polls until success", async () => {
    const health = makeHealth();
    let calls = 0;
    const responses: Array<
      { ok: true; health: DaemonHealth } | { ok: false; status: number }
    > = [
      { ok: false, status: 503 },
      { ok: false, status: 503 },
      { ok: true, health },
    ];
    const fetchImpl = vi.fn(async () => {
      const r = responses[calls];
      calls++;
      if (!r) throw new Error("exhausted");
      if (r.ok) {
        return {
          ok: true,
          status: 200,
          json: async () => r.health,
        } as unknown as Response;
      }
      return { ok: false, status: r.status, json: async () => ({}) } as unknown as Response;
    }) as unknown as typeof fetch;
    const result = await awaitDaemonHealthy({
      base: BASE,
      fetchImpl,
      timeoutMs: 5000,
      intervalMs: 5,
    });
    expect(result).toEqual(health);
    expect(calls).toBe(3);
  });

  it("timeout: rejects with a message naming the base URL + escape hint", async () => {
    const fetchImpl = fetchAlwaysError();
    await expect(
      awaitDaemonHealthy({
        base: BASE,
        fetchImpl,
        timeoutMs: 80,
        intervalMs: 20,
      }),
    ).rejects.toThrow(new RegExp(`${BASE.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}|transport=tauri`));
  });

  it("network errors then success: retries through ECONNREFUSED", async () => {
    const health = makeHealth();
    let calls = 0;
    const fetchImpl = vi.fn(async () => {
      calls++;
      if (calls < 3) throw new Error("connect ECONNREFUSED");
      return {
        ok: true,
        status: 200,
        json: async () => health,
      } as unknown as Response;
    }) as unknown as typeof fetch;
    const result = await awaitDaemonHealthy({
      base: BASE,
      fetchImpl,
      timeoutMs: 5000,
      intervalMs: 5,
    });
    expect(result).toEqual(health);
    expect(calls).toBe(3);
  });
});
