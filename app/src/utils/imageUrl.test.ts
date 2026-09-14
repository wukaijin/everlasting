// imageUrl — 09-13 本地路径预览(图片 + 同日文件通道)的单测:
//   - resolveImagePath:相对路径按会话 cwd 解析;`/`、`~/` 原样透传
//     (`~` 由 daemon 展开);cwd 未知时不猜。
//   - imageUrl / fileUrl:三传输模式(URL 形态与 attachmentUrl.test.ts
//     同构,daemonBase/currentDeviceToken 模块级 mock 保确定性)。

import { describe, it, expect, vi } from "vitest";

vi.mock("../transport/http", () => ({
  daemonBase: vi.fn(() => "http://localhost:7456"),
}));
vi.mock("../transport/auth", () => ({
  currentDeviceToken: vi.fn(() => null),
}));

import { currentDeviceToken } from "../transport/auth";
import { fileUrl, imageUrl, resolveImagePath, statUrl } from "./imageUrl";

describe("resolveImagePath", () => {
  it("joins a relative path onto cwd", () => {
    expect(resolveImagePath("out/x.png", "/proj/root")).toBe(
      "/proj/root/out/x.png",
    );
  });

  it("strips a leading ./ before joining", () => {
    expect(resolveImagePath("./out/x.png", "/proj/root")).toBe(
      "/proj/root/out/x.png",
    );
  });

  it("keeps ../ segments for the daemon fs to resolve", () => {
    expect(resolveImagePath("../out/x.png", "/proj/root/sub")).toBe(
      "/proj/root/sub/../out/x.png",
    );
  });

  it("passes absolute and ~-prefixed paths through untouched", () => {
    expect(resolveImagePath("/tmp/shot.png", "/proj/root")).toBe("/tmp/shot.png");
    expect(resolveImagePath("~/.local/share/x.png", "/proj/root")).toBe(
      "~/.local/share/x.png",
    );
  });

  it("does not guess when cwd is unknown (passes raw through)", () => {
    expect(resolveImagePath("out/x.png", "")).toBe("out/x.png");
  });

  it("collapses duplicate trailing slashes on cwd", () => {
    expect(resolveImagePath("out/x.png", "/proj/root//")).toBe(
      "/proj/root/out/x.png",
    );
  });
});

describe("imageUrl", () => {
  it("builds the direct daemon URL without a device token", () => {
    expect(imageUrl("/tmp/shot.png")).toBe(
      "http://localhost:7456/api/v1/files/image?path=%2Ftmp%2Fshot.png",
    );
  });

  it("routes through the pwa-remote proxy with the query token", () => {
    vi.mocked(currentDeviceToken).mockReturnValueOnce("tok en&1");
    expect(imageUrl("~/.local/share/x.png")).toBe(
      "http://localhost:7456/api/v1/proxy/api/v1/files/image" +
        "?path=~%2F.local%2Fshare%2Fx.png&access_token=tok%20en%261",
    );
  });

  it("percent-encodes the path so query structure cannot be injected", () => {
    expect(imageUrl("/a b&c=d.png")).toBe(
      "http://localhost:7456/api/v1/files/image?path=%2Fa%20b%26c%3Dd.png",
    );
  });
});

// fileUrl — 文件通道(/files/raw)的三传输模式,镜像 imageUrl describe。
describe("fileUrl", () => {
  it("builds the direct daemon URL against /files/raw", () => {
    expect(fileUrl("/proj/root/out/report.md")).toBe(
      "http://localhost:7456/api/v1/files/raw?path=" +
        encodeURIComponent("/proj/root/out/report.md"),
    );
  });

  it("routes through the pwa-remote proxy with the query token (AC7)", () => {
    vi.mocked(currentDeviceToken).mockReturnValueOnce("tok en&1");
    expect(fileUrl("~/docs/spec.pdf")).toBe(
      "http://localhost:7456/api/v1/proxy/api/v1/files/raw" +
        "?path=~%2Fdocs%2Fspec.pdf&access_token=tok%20en%261",
    );
  });

  it("percent-encodes the path so query structure cannot be injected", () => {
    expect(fileUrl("/a b&c=d.md")).toBe(
      "http://localhost:7456/api/v1/files/raw?path=%2Fa%20b%26c%3Dd.md",
    );
  });
});

// statUrl — 存在性探针(/files/stat,09-14 存在性闸门)的三传输模式,
// 镜像 fileUrl describe;path 契约同 fileUrl(解析后的绝对/~ 形态)。
describe("statUrl", () => {
  it("builds the direct daemon URL against /files/stat", () => {
    expect(statUrl("/tmp/maybe/shot.png")).toBe(
      "http://localhost:7456/api/v1/files/stat?path=%2Ftmp%2Fmaybe%2Fshot.png",
    );
  });

  it("routes through the pwa-remote proxy with the query token", () => {
    vi.mocked(currentDeviceToken).mockReturnValueOnce("tok en&1");
    expect(statUrl("~/docs/spec.md")).toBe(
      "http://localhost:7456/api/v1/proxy/api/v1/files/stat" +
        "?path=~%2Fdocs%2Fspec.md&access_token=tok%20en%261",
    );
  });
});
