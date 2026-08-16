// B1 (2026-08-16) image-multimodal R2a — component tests for
// `MessageImages.vue`, the user-message thumbnail strip.
//
// Coverage:
//   1. Both entry shapes render: a `file` ref resolves through
//      `attachmentUrl(sessionId, file)` (daemon GET route); a
//      `localUrl`-only entry (blob objectURL) renders verbatim.
//   2. Clicking a thumbnail opens the resolved URL via
//      `window.open` (new tab) — `vi.spyOn(window, "open")`.
//
// `attachmentUrl`'s transport deps are module-mocked (same pattern
// as `utils/attachmentUrl.test.ts`) so the expected URLs are
// deterministic instead of depending on vitest's `import.meta.env`.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { mount } from "@vue/test-utils";

vi.mock("../../transport/http", () => ({
  daemonBase: vi.fn(() => "http://localhost:7456"),
}));
vi.mock("../../transport/auth", () => ({
  getDeviceToken: vi.fn(() => null),
}));

import MessageImages from "./MessageImages.vue";

const openSpy = vi.spyOn(window, "open").mockImplementation(() => null);

function mountStrip(
  images: Array<{ file?: string; localUrl?: string; mediaType: string }>,
) {
  // No element stubs: jsdom doesn't fetch <img> resources, so the
  // real element renders and the src attribute is assertable.
  return mount(MessageImages, {
    props: { sessionId: "sess-1", images },
  });
}

beforeEach(() => {
  openSpy.mockClear();
});

describe("MessageImages — render", () => {
  it("renders a thumbnail per entry, file ref via attachmentUrl", () => {
    const w = mountStrip([
      { file: "a1b2c3d4e5f6.png", mediaType: "image/png" },
      { localUrl: "blob:local-1", mediaType: "image/jpeg" },
    ]);
    const items = w.findAll(".message-images__item");
    expect(items.length).toBe(2);
    // file form → daemon GET route URL.
    const src0 = items[0].get("img").attributes("src");
    expect(src0).toBe(
      "http://localhost:7456/api/v1/attachments/sess-1/a1b2c3d4e5f6.png",
    );
    // localUrl form → blob URL verbatim.
    const src1 = items[1].get("img").attributes("src");
    expect(src1).toBe("blob:local-1");
  });

  it("renders nothing when the images array is empty", () => {
    const w = mountStrip([]);
    expect(w.findAll(".message-images__item").length).toBe(0);
  });
});

describe("MessageImages — click opens the full-size image", () => {
  it("opens the attachmentUrl for a file entry", async () => {
    const w = mountStrip([{ file: "f.png", mediaType: "image/png" }]);
    await w.find("[data-testid='message-image-0']").trigger("click");
    expect(openSpy).toHaveBeenCalledTimes(1);
    expect(openSpy).toHaveBeenCalledWith(
      "http://localhost:7456/api/v1/attachments/sess-1/f.png",
    );
  });

  it("opens the localUrl for a localUrl-only entry", async () => {
    const w = mountStrip([{ localUrl: "blob:local-2", mediaType: "image/png" }]);
    await w.find("[data-testid='message-image-0']").trigger("click");
    expect(openSpy).toHaveBeenCalledWith("blob:local-2");
  });

  it("prefers the file ref over localUrl when both are present", async () => {
    const w = mountStrip([
      { file: "both.png", localUrl: "blob:stale", mediaType: "image/png" },
    ]);
    await w.find("[data-testid='message-image-0']").trigger("click");
    expect(openSpy).toHaveBeenCalledWith(
      "http://localhost:7456/api/v1/attachments/sess-1/both.png",
    );
  });
});
