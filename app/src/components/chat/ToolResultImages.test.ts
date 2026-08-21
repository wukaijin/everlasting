// ToolResultImages — 08-21-b1-image-followups R6 tests:
//   1. tool-returned image refs render thumbnails via
//      `attachmentUrl(sessionId, file)` (daemon GET route);
//   2. no sessionId → renders nothing (search-preview callers);
//   3. empty images → renders nothing.
// `attachmentUrl`'s transport deps are module-mocked (same pattern
// as `MessageImages.test.ts`).
import { describe, it, expect, vi } from "vitest";
import { mount } from "@vue/test-utils";

vi.mock("../../transport/http", () => ({
  daemonBase: vi.fn(() => "http://localhost:7456"),
}));
vi.mock("../../transport/auth", () => ({
  getDeviceToken: vi.fn(() => null),
}));

import ToolResultImages from "./ToolResultImages.vue";

const IMGS = [
  { file: "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6.png", media_type: "image/png", source: "read_file", tokens_est: 640 },
];

describe("ToolResultImages", () => {
  it("渲染工具返回图的缩略图(attachmentUrl 路由)", () => {
    const wrapper = mount(ToolResultImages, {
      props: { images: IMGS, sessionId: "sess12345678" },
    });
    const img = wrapper.find("img");
    expect(img.exists()).toBe(true);
    expect(img.attributes("src")).toBe(
      "http://localhost:7456/api/v1/attachments/sess12345678/a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6.png",
    );
  });

  it("无 sessionId(搜索预览等无会话上下文)不渲染", () => {
    const wrapper = mount(ToolResultImages, {
      props: { images: IMGS, sessionId: "" },
    });
    expect(wrapper.find("img").exists()).toBe(false);
  });

  it("空列表不渲染", () => {
    const wrapper = mount(ToolResultImages, {
      props: { images: [], sessionId: "sess12345678" },
    });
    expect(wrapper.find(".tool-result-images").exists()).toBe(false);
  });
});
