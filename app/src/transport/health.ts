// daemon health handshake (P2.4 D3.3, 2026-07-22, task
// `07-20-remote-access-daemon-split`).
//
// The GUI's `httpTransport` default (P2.4 D3.1) is an **irreversible
// switch** — once the frontend picks httpTransport, every store /
// controller reaches the agent core through the daemon. If the daemon
// isn't up (sidecar spawn race, crash, port squatter), the GUI is
// totally non-functional. This module closes that race by polling
// `GET {daemonBase}/api/v1/health` BEFORE the app mounts, with a
// layered Q5 validation:
//
//   - protocol mismatch (`api_versions` lacks `"v1"`) → hard fail
//     (the daemon speaks a different protocol; do NOT silently fall
//     through to a broken UX).
//   - build mismatch (`daemon_version` ≠ this build) → `console.warn`
//     only (dev/edge mixing is tolerated; protocol-compatible).
//   - timeout → hard fail (the daemon never came up).
//
// The caller (`main.ts`) renders a full-screen error overlay on hard
// fail and never mounts the app — fail-loud, no silent degradation
// (Q1 + R-1 decision).

import { daemonBase } from "./http";

/** Expected API protocol version. The daemon's `api_versions` MUST
 *  contain this or the handshake hard-fails (Q5 protocol-version gate).
 *  Sourced from the daemon's `SUPPORTED_API_VERSIONS` constant
 *  (`daemon/routes/health.rs`); duplicated here as a literal to avoid
 *  a Rust→TS codegen dependency (Q4 decision — no ts-rs). */
export const EXPECTED_API_VERSION = "v1";

/** Health response shape (mirrors `daemon::routes::health::HealthResponse`,
 *  serde `rename_all = "camelCase"`). Subset — we only validate the
 *  fields the handshake cares about. */
export interface DaemonHealth {
  daemonId: string;
  daemonVersion: string;
  apiVersions: string[];
  uptimeSeconds?: number;
}

/** Outcome of a single health probe — either the daemon responded
 *  with a parseable body, or it didn't (network error / non-200 /
 *  unparseable JSON). */
type ProbeResult =
  | { ok: true; health: DaemonHealth }
  | { ok: false; reason: string };

/** One `GET /api/v1/health` attempt via an injected fetch. Short
 *  timeout isn't enforced here (the poll loop's `timeoutMs` is the
 *  ceiling); the daemon typically answers within 50-200ms of the
 *  sidecar spawn.
 *
 *  Accepts the fetch as a parameter so tests can inject a mock
 *  without touching `globalThis.fetch`. Production wires
 *  `globalThis.fetch.bind(globalThis)` in `awaitDaemonHealthy`. */
async function probeOnceWith(
  base: string,
  fetchImpl: typeof fetch,
): Promise<ProbeResult> {
  try {
    const resp = await fetchImpl(`${base}/api/v1/health`, { method: "GET" });
    if (!resp.ok) return { ok: false, reason: `HTTP ${resp.status}` };
    const health = (await resp.json()) as DaemonHealth;
    return { ok: true, health };
  } catch (e) {
    return {
      ok: false,
      reason: e instanceof Error ? e.message : String(e),
    };
  }
}

/** Q5 layered validation of a successful health response. Returns
 *  `null` when the daemon is acceptable, or a user-facing Chinese
 *  message describing why it's not.
 *
 *  - `api_versions` MUST contain `EXPECTED_API_VERSION` (hard fail —
 *    protocol incompatibility means the frontend can't talk to this
 *    daemon at all).
 *  - `daemon_version` vs `__APP_VERSION__`: warn-only (logged), never
 *    blocks. Dev/edge mixing is expected during upgrades. */
function validateHealth(health: DaemonHealth): string | null {
  if (!Array.isArray(health.apiVersions) || !health.apiVersions.includes(EXPECTED_API_VERSION)) {
    return (
      `daemon 协议版本不兼容:期望 api_versions 含 "${EXPECTED_API_VERSION}",` +
      `实际 ${JSON.stringify(health.apiVersions)}。\n` +
      `请重启 GUI 与 daemon 使用同一版本构建(可能 daemon 是旧版残留)。`
    );
  }
  // Build-version drift: warn only.
  const appVersion =
    typeof __APP_VERSION__ === "string" && __APP_VERSION__ !== "0.0.0"
      ? __APP_VERSION__
      : null;
  if (
    appVersion &&
    health.daemonVersion &&
    appVersion !== health.daemonVersion
  ) {
    console.warn(
      `[health] daemon build version drift: frontend=${appVersion} daemon=${health.daemonVersion} (warn-only; protocol compatible)`,
    );
  }
  return null;
}

/** Options for `awaitDaemonHealthy`. */
export interface AwaitDaemonHealthyOptions {
  /** Total budget in ms. Default 15s — the sidecar binary is 100MB+ and
   *  cold-starts the SQLite pool + migrations on first run, so a
   *  generous ceiling avoids spurious failures on slow disks. */
  timeoutMs?: number;
  /** Poll interval in ms. Default 250ms — tight enough to feel instant
   *  once the daemon is up, loose enough to avoid hammering during the
   *  race window. */
  intervalMs?: number;
  /** Injected for tests (defaults to the module-level `daemonBase`). */
  base?: string;
  /** Injected fetch for tests. */
  fetchImpl?: typeof fetch;
}

/** Poll the daemon health endpoint until it responds with a
 *  protocol-compatible body, or until `timeoutMs` elapses. Resolves
 *  with the validated `DaemonHealth`; rejects with a Chinese
 *  user-facing message on timeout or protocol mismatch.
 *
 *  Called from `main.ts` BEFORE `app.mount("#app")` so a broken daemon
 *  never produces a half-rendered, non-functional UI. */
export async function awaitDaemonHealthy(
  opts: AwaitDaemonHealthyOptions = {},
): Promise<DaemonHealth> {
  const timeoutMs = opts.timeoutMs ?? 15_000;
  const intervalMs = opts.intervalMs ?? 250;
  const base = opts.base ?? daemonBase();
  // The probe closures capture the injected fetch if provided (tests);
  // production uses the global.
  const fetchImpl = opts.fetchImpl ?? globalThis.fetch.bind(globalThis);

  const deadline = Date.now() + timeoutMs;
  let lastReason = "(no response yet)";
  while (Date.now() < deadline) {
    const result = await probeOnceWith(base, fetchImpl);
    if (result.ok) {
      const validationError = validateHealth(result.health);
      if (validationError) {
        // Protocol mismatch — retrying won't help; bail immediately.
        throw new Error(validationError);
      }
      return result.health;
    }
    lastReason = result.reason;
    await sleep(intervalMs);
  }
  throw new Error(
    `daemon 未在 ${timeoutMs / 1000}s 内就绪(最后一次探测:${lastReason})。\n` +
      `可能原因:sidecar 启动失败 / 端口被占 / daemon 崩溃。\n` +
      `排查:\n` +
      `  1) 检查 GUI 控制台是否有 sidecar spawn 错误;\n` +
      `  2) 手动 curl ${base}/api/v1/health 确认 daemon 存活;\n` +
      `  3) 紧急逃生:在窗口 URL 加 ?transport=tauri 走 Full 模式。`,
  );
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
