// B1 attachmentUrl — the three transport modes (design §3.3):
//   1. browser-local PROD (no token, daemonBase = location.origin)
//   2. DEV browser (no token, daemonBase = http://localhost:7456)
//   3. pwa-remote (device token present → remote proxy path +
//      `?access_token=` query, the SSE EventSource auth channel)
//
// `daemonBase` / `currentDeviceToken` are module-mocked so each mode is
// driven explicitly instead of relying on vitest's `import.meta.env`
// defaults.

import { describe, it, expect, vi, beforeEach } from "vitest";

vi.mock("../transport/http", () => ({ daemonBase: vi.fn() }));
vi.mock("../transport/auth", () => ({ currentDeviceToken: vi.fn() }));

import { attachmentUrl } from "./attachmentUrl";
import { daemonBase } from "../transport/http";
import { currentDeviceToken } from "../transport/auth";

const daemonBaseMock = vi.mocked(daemonBase);
const getTokenMock = vi.mocked(currentDeviceToken);

describe("attachmentUrl", () => {
  beforeEach(() => {
    daemonBaseMock.mockReset();
    getTokenMock.mockReset();
    getTokenMock.mockReturnValue(null);
  });

  it("PROD browser-local: absolute URL under location.origin", () => {
    daemonBaseMock.mockReturnValue("https://pc.example.com");
    expect(attachmentUrl("s1", "a1b2c3d4e5f6.png")).toBe(
      "https://pc.example.com/api/v1/attachments/s1/a1b2c3d4e5f6.png",
    );
  });

  it("DEV browser: absolute URL against the daemon port (not relative)", () => {
    daemonBaseMock.mockReturnValue("http://localhost:7456");
    const url = attachmentUrl("s1", "a1b2c3d4e5f6.webp");
    expect(url).toBe(
      "http://localhost:7456/api/v1/attachments/s1/a1b2c3d4e5f6.webp",
    );
  });

  it("strips a trailing slash from daemonBase", () => {
    daemonBaseMock.mockReturnValue("http://localhost:7456/");
    expect(attachmentUrl("s1", "f.png")).toBe(
      "http://localhost:7456/api/v1/attachments/s1/f.png",
    );
  });

  it("pwa-remote: routes through the remote proxy with access_token query", () => {
    daemonBaseMock.mockReturnValue("https://remote.example.com");
    getTokenMock.mockReturnValue("tok en+1=");
    expect(attachmentUrl("s1", "f.png")).toBe(
      "https://remote.example.com/api/v1/proxy/api/v1/attachments/s1/f.png?access_token=tok%20en%2B1%3D",
    );
  });

  it("encodes path components defensively", () => {
    daemonBaseMock.mockReturnValue("http://localhost:7456");
    expect(attachmentUrl("s 1", "a b.png")).toBe(
      "http://localhost:7456/api/v1/attachments/s%201/a%20b.png",
    );
  });
});
