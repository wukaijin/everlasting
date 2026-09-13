// ImageViewerModal — 09-13 图片路径预览弹层的组件测试。
//
// 弹层状态由 useImageViewer 模块级单例驱动(非 props),用例直接调
// open()/close() 驱动;reka-ui DialogPortal 把内容 Teleport 到
// document.body,断言走 document.body.querySelector(与组件树解耦)。
//
// Mock 策略:transport(http/auth)模块级 mock(同 MessageImages.test.ts)
// 让 imageUrl 确定性;chatStore 直接 mock 模块(组件只消费 currentCwd)
// —— 引真 store 会拖进 transport/index 的 httpTransport 初始化链,
// 与本组件无关。

import { describe, it, expect, vi, beforeEach } from "vitest";
import { mount } from "@vue/test-utils";
import { nextTick } from "vue";

vi.mock("../../transport/http", () => ({
  daemonBase: vi.fn(() => "http://localhost:7456"),
}));
vi.mock("../../transport/auth", () => ({
  currentDeviceToken: vi.fn(() => null),
}));
vi.mock("../../stores/chat", () => ({
  useChatStore: () => ({ currentCwd: "/proj/root" }),
}));

import ImageViewerModal from "./ImageViewerModal.vue";
import { useImageViewer } from "../../composables/useImageViewer";

beforeEach(() => {
  document.body.innerHTML = "";
  // 模块级单例跨用例复位。
  useImageViewer().close();
});

function mountModal() {
  return mount(ImageViewerModal, { attachTo: document.body });
}

describe("ImageViewerModal — open state", () => {
  it("renders no viewer content while closed", () => {
    const w = mountModal();
    expect(
      document.body.querySelector("[data-testid='image-viewer-img']"),
    ).toBeNull();
    w.unmount();
  });

  it("shows the img with the daemon URL (relative path resolved via cwd)", async () => {
    const w = mountModal();
    useImageViewer().open("out/ui-review/x/1.png");
    await nextTick();
    const img = document.body.querySelector("[data-testid='image-viewer-img']");
    expect(img?.getAttribute("src")).toBe(
      "http://localhost:7456/api/v1/files/image?path=" +
        encodeURIComponent("/proj/root/out/ui-review/x/1.png"),
    );
    // footer 显示解析后的完整路径;标题是文件名。
    expect(document.body.textContent).toContain(
      "/proj/root/out/ui-review/x/1.png",
    );
    expect(document.body.textContent).toContain("1.png");
    w.unmount();
  });

  it("passes ~-prefixed paths through without cwd joining", async () => {
    const w = mountModal();
    useImageViewer().open("~/.local/share/x.png");
    await nextTick();
    const img = document.body.querySelector("[data-testid='image-viewer-img']");
    expect(img?.getAttribute("src")).toContain(
      "path=" + encodeURIComponent("~/.local/share/x.png"),
    );
    w.unmount();
  });
});

describe("ImageViewerModal — interactions", () => {
  it("close button closes the viewer state", async () => {
    const w = mountModal();
    useImageViewer().open("out/x.png");
    await nextTick();
    (
      document.body.querySelector(
        "[data-testid='image-viewer-close']",
      ) as HTMLElement
    ).click();
    await nextTick();
    expect(useImageViewer().isOpen.value).toBe(false);
    w.unmount();
  });

  it("swaps to the error state when the img fails to load", async () => {
    const w = mountModal();
    useImageViewer().open("out/x.png");
    await nextTick();
    const img = document.body.querySelector("[data-testid='image-viewer-img']");
    img?.dispatchEvent(new Event("error"));
    await nextTick();
    expect(
      document.body.querySelector("[data-testid='image-viewer-error']"),
    ).not.toBeNull();
    w.unmount();
  });

  it("new-tab fallback opens the same constructed URL", async () => {
    const openSpy = vi.spyOn(window, "open").mockImplementation(() => null);
    const w = mountModal();
    useImageViewer().open("out/x.png");
    await nextTick();
    (
      document.body.querySelector(
        "[data-testid='image-viewer-open-tab']",
      ) as HTMLElement
    ).click();
    expect(openSpy).toHaveBeenCalledTimes(1);
    expect(openSpy).toHaveBeenCalledWith(
      "http://localhost:7456/api/v1/files/image?path=" +
        encodeURIComponent("/proj/root/out/x.png"),
      "_blank",
      "noreferrer",
    );
    openSpy.mockRestore();
    w.unmount();
  });
});

describe("ImageViewerModal — zoom / pan", () => {
  function zoomLabel(): string {
    return (
      document.body.querySelector("[data-testid='image-viewer-zoom-label']")
        ?.textContent ?? ""
    );
  }
  function imgStyle(): string {
    return (
      document.body
        .querySelector("[data-testid='image-viewer-img']")
        ?.getAttribute("style") ?? ""
    );
  }
  async function openAndMount() {
    const w = mountModal();
    useImageViewer().open("out/x.png");
    await nextTick();
    return w;
  }

  it("starts at 100% with identity transform", async () => {
    const w = await openAndMount();
    expect(zoomLabel()).toBe("100%");
    expect(imgStyle()).toContain("scale(1)");
    w.unmount();
  });

  it("zoom-in button steps to 125% and writes the img transform", async () => {
    const w = await openAndMount();
    (
      document.body.querySelector(
        "[data-testid='image-viewer-zoom-in']",
      ) as HTMLElement
    ).click();
    await nextTick();
    expect(zoomLabel()).toBe("125%");
    expect(imgStyle()).toContain("scale(1.25)");
    w.unmount();
  });

  it("zoom-out is disabled at 100%; works after zooming in", async () => {
    const w = await openAndMount();
    const out = document.body.querySelector(
      "[data-testid='image-viewer-zoom-out']",
    ) as HTMLButtonElement;
    expect(out.disabled).toBe(true);
    (
      document.body.querySelector(
        "[data-testid='image-viewer-zoom-in']",
      ) as HTMLElement
    ).click();
    await nextTick();
    expect(out.disabled).toBe(false);
    out.click();
    await nextTick();
    expect(zoomLabel()).toBe("100%");
    w.unmount();
  });

  it("reset button restores 100% after zoom + pan", async () => {
    const w = await openAndMount();
    (
      document.body.querySelector(
        "[data-testid='image-viewer-zoom-in']",
      ) as HTMLElement
    ).click();
    await nextTick();
    // 拖一点位移(放大后 canPan)。
    const stage = document.body.querySelector(".image-viewer__stage") as HTMLElement;
    stage.dispatchEvent(
      new MouseEvent("pointerdown", { bubbles: true, clientX: 0, clientY: 0 }),
    );
    stage.dispatchEvent(
      new MouseEvent("pointermove", { bubbles: true, clientX: 40, clientY: 20 }),
    );
    stage.dispatchEvent(
      new MouseEvent("pointerup", { bubbles: true, clientX: 40, clientY: 20 }),
    );
    await nextTick();
    expect(imgStyle()).not.toContain("translate(calc(-50% + 0px)");
    (
      document.body.querySelector(
        "[data-testid='image-viewer-zoom-reset']",
      ) as HTMLElement
    ).click();
    await nextTick();
    expect(zoomLabel()).toBe("100%");
    expect(imgStyle()).toContain("translate(calc(-50% + 0px), calc(-50% + 0px))");
    w.unmount();
  });

  it("wheel zooms around the pointer (jsdom rect is 0×0 → center anchor)", async () => {
    const w = await openAndMount();
    const stage = document.body.querySelector(".image-viewer__stage") as HTMLElement;
    stage.dispatchEvent(
      new WheelEvent("wheel", { bubbles: true, cancelable: true, deltaY: -100 }),
    );
    await nextTick();
    // factor = exp(0.15) ≈ 1.162 → round(116%)。
    expect(zoomLabel()).toBe("116%");
    w.unmount();
  });

  it("double-click zooms in, then double-click resets", async () => {
    const w = await openAndMount();
    const stage = document.body.querySelector(".image-viewer__stage") as HTMLElement;
    stage.dispatchEvent(
      new MouseEvent("dblclick", { bubbles: true, clientX: 0, clientY: 0 }),
    );
    await nextTick();
    expect(zoomLabel()).toBe("250%");
    stage.dispatchEvent(
      new MouseEvent("dblclick", { bubbles: true, clientX: 0, clientY: 0 }),
    );
    await nextTick();
    expect(zoomLabel()).toBe("100%");
    w.unmount();
  });

  it("drag does not pan at 100% (canPan gate)", async () => {
    const w = await openAndMount();
    const stage = document.body.querySelector(".image-viewer__stage") as HTMLElement;
    stage.dispatchEvent(
      new MouseEvent("pointerdown", { bubbles: true, clientX: 0, clientY: 0 }),
    );
    stage.dispatchEvent(
      new MouseEvent("pointermove", { bubbles: true, clientX: 40, clientY: 20 }),
    );
    stage.dispatchEvent(
      new MouseEvent("pointerup", { bubbles: true, clientX: 40, clientY: 20 }),
    );
    await nextTick();
    expect(imgStyle()).toContain("translate(calc(-50% + 0px), calc(-50% + 0px))");
    w.unmount();
  });

  it("switching to another image resets the zoom state", async () => {
    const w = await openAndMount();
    (
      document.body.querySelector(
        "[data-testid='image-viewer-zoom-in']",
      ) as HTMLElement
    ).click();
    await nextTick();
    expect(zoomLabel()).toBe("125%");
    useImageViewer().open("out/y.png");
    await nextTick();
    expect(zoomLabel()).toBe("100%");
    w.unmount();
  });
});
