// FileViewerModal — 09-13 文件路径预览弹层的组件测试。
//
// 弹层状态由 useFileViewer 模块级单例驱动(非 props),用例直接调
// open()/close() 驱动;reka-ui DialogPortal 把内容 Teleport 到
// document.body,断言走 document.body.querySelector(与组件树解耦,
// 同 ImageViewerModal.test.ts 手法)。
//
// Mock 策略:transport(http/auth)与 chatStore 模块级 mock(同
// ImageViewerModal.test.ts);fetch 用 vi.stubGlobal 挂桩 —— 取数在
// composable 的 open() 里走全局 fetch,桩按 URL 应答,loading→ok/
// error 状态推进用 vi.waitFor 等微任务链排干。

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { mount } from "@vue/test-utils";

vi.mock("../../transport/http", () => ({
  daemonBase: vi.fn(() => "http://localhost:7456"),
}));
vi.mock("../../transport/auth", () => ({
  currentDeviceToken: vi.fn(() => null),
}));
vi.mock("../../stores/chat", () => ({
  useChatStore: () => ({ currentCwd: "/proj/root" }),
}));

import FileViewerModal from "./FileViewerModal.vue";
import { useFileViewer } from "../../composables/useFileViewer";

type FetchStub = (url: string) => { ok: boolean; text: () => Promise<string> };

/** 挂全局 fetch 桩,返回 mock 供断言调用参数。 */
function stubFetch(respond: FetchStub) {
  const fetchMock = vi.fn((url: string | URL | Request) =>
    Promise.resolve(respond(String(url))),
  );
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

/** 排干 open() 的 fetch→text→state 微任务链。 */
async function settle(): Promise<void> {
  await vi.waitFor(() => {
    const s = useFileViewer().status.value;
    expect(["ok", "error"]).toContain(s);
  });
}

beforeEach(() => {
  document.body.innerHTML = "";
  // 模块级单例跨用例复位。
  useFileViewer().close();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

function mountModal() {
  return mount(FileViewerModal, { attachTo: document.body });
}

describe("FileViewerModal — mode dispatch (AC3)", () => {
  it("renders .md content through the markdown pipeline", async () => {
    const fetchMock = stubFetch((url) =>
      url.includes("/files/raw?")
        ? { ok: true, text: async () => "# 标题\n\n正文段落" }
        : { ok: false, text: async () => "" },
    );
    const w = mountModal();
    useFileViewer().open("out/报告.md");
    await settle();
    const body = document.body.querySelector("[data-testid='file-viewer-markdown']");
    expect(body).not.toBeNull();
    // renderMarkdown 管线产物(h1 标记),不是纯文本。
    expect(body?.querySelector("h1")?.textContent).toBe("标题");
    expect(fetchMock).toHaveBeenCalledWith(
      "http://localhost:7456/api/v1/files/raw?path=" +
        encodeURIComponent("/proj/root/out/报告.md"),
    );
    w.unmount();
  });

  it("renders non-md text as a code card with copy button + ext language", async () => {
    stubFetch(() => ({ ok: true, text: async () => "fn main() {}" }));
    const w = mountModal();
    useFileViewer().open("src/main.rs");
    await settle();
    const code = document.body.querySelector("[data-testid='file-viewer-code']");
    expect(code).not.toBeNull();
    // CodeBlockPrimitive 契约:语言标签 = 扩展名,复制按钮免费获得。
    expect(code?.querySelector(".ui-prim__type")?.textContent).toBe("rs");
    expect(code?.querySelector(".ui-prim__copy")).not.toBeNull();
    expect(code?.querySelector("code.hljs")?.textContent).toContain("fn main()");
    // md 面不该出现。
    expect(document.body.querySelector("[data-testid='file-viewer-markdown']")).toBeNull();
    w.unmount();
  });

  it("shows the loading state while the fetch is in flight", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => new Promise<{ ok: boolean; text: () => Promise<string> }>(() => {})),
    );
    const w = mountModal();
    useFileViewer().open("out/slow.md");
    await Promise.resolve();
    expect(
      document.body.querySelector("[data-testid='file-viewer-loading']"),
    ).not.toBeNull();
    w.unmount();
  });

  it("swaps to the error state on a non-200 response", async () => {
    stubFetch(() => ({ ok: false, text: async () => "" }));
    const w = mountModal();
    useFileViewer().open("out/gone.md");
    await settle();
    expect(
      document.body.querySelector("[data-testid='file-viewer-error']"),
    ).not.toBeNull();
    w.unmount();
  });
});

describe("FileViewerModal — pdf dispatch (AC4)", () => {
  it("opens a new tab to /files/raw instead of the modal", async () => {
    const openSpy = vi.spyOn(window, "open").mockImplementation(() => null);
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    const w = mountModal();
    useFileViewer().open("~/docs/spec.pdf");
    await Promise.resolve();
    expect(openSpy).toHaveBeenCalledTimes(1);
    expect(String(openSpy.mock.calls[0]?.[0])).toContain("/api/v1/files/raw?path=");
    expect(String(openSpy.mock.calls[0]?.[0])).toContain(
      encodeURIComponent("~/docs/spec.pdf"),
    );
    // 不开弹层:单例状态仍关闭,无任何弹层内容,零 fetch。
    expect(useFileViewer().isOpen.value).toBe(false);
    expect(document.body.querySelector("[data-testid='file-viewer-code']")).toBeNull();
    expect(document.body.querySelector("[data-testid='file-viewer-markdown']")).toBeNull();
    expect(fetchMock).not.toHaveBeenCalled();
    openSpy.mockRestore();
    w.unmount();
  });
});

describe("FileViewerModal — interactions", () => {
  it("close button closes the viewer state", async () => {
    stubFetch(() => ({ ok: true, text: async () => "x" }));
    const w = mountModal();
    useFileViewer().open("out/x.md");
    await settle();
    (
      document.body.querySelector(
        "[data-testid='file-viewer-close']",
      ) as HTMLElement
    ).click();
    await Promise.resolve();
    expect(useFileViewer().isOpen.value).toBe(false);
    w.unmount();
  });

  it("new-tab fallback opens the same constructed URL", async () => {
    stubFetch(() => ({ ok: true, text: async () => "x" }));
    const openSpy = vi.spyOn(window, "open").mockImplementation(() => null);
    const w = mountModal();
    useFileViewer().open("out/x.md");
    await settle();
    (
      document.body.querySelector(
        "[data-testid='file-viewer-open-tab']",
      ) as HTMLElement
    ).click();
    expect(openSpy).toHaveBeenCalledWith(
      "http://localhost:7456/api/v1/files/raw?path=" +
        encodeURIComponent("/proj/root/out/x.md"),
      "_blank",
      "noreferrer",
    );
    openSpy.mockRestore();
    w.unmount();
  });

  it("nested paths in rendered markdown keep the delegation alive (AC3)", async () => {
    const fetchMock = stubFetch((url) => {
      if (url.includes(encodeURIComponent("out/inner.md"))) {
        return { ok: true, text: async () => "内层内容" };
      }
      return { ok: true, text: async () => "外层引用 out/inner.md 就看" };
    });
    const w = mountModal();
    useFileViewer().open("out/outer.md");
    await settle();
    // 第一帧渲染出的 markdown 里有嵌套文件路径锚点。
    const nested = document.body.querySelector<HTMLAnchorElement>(
      "[data-testid='file-viewer-markdown'] a[data-file-path='out/inner.md']",
    );
    expect(nested).not.toBeNull();
    nested!.click();
    await vi.waitFor(() => {
      expect(useFileViewer().rawPath.value).toBe("out/inner.md");
      expect(useFileViewer().status.value).toBe("ok");
    });
    // 第二次取数用的是内层路径;内容换成了内层文件。
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(document.body.querySelector("[data-testid='file-viewer-markdown']")?.textContent).toContain(
      "内层内容",
    );
    w.unmount();
  });
});
