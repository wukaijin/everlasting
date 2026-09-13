// Tests for `useCodeBlockCopy` — BUGLIST CH4-5 (2026-08-29): the
// delegated click handler for the fenced-code chrome emitted by the
// markdown pipeline into v-html. Locks the contract:
//   1. Click on [data-code-copy] → clipboard gets the sibling
//      <pre><code> text, button flips to "已复制", reverts after 2s.
//   2. Clicks that don't land on the button are ignored.
//   3. Missing clipboard API (jsdom default / non-secure context)
//      degrades silently — no throw, no label change.
// 09-13 图片路径预览分支:
//   4. Click on a[data-image-path] → preventDefault + useImageViewer
//      open(原始路径);普通 <a> 与其它落点不开弹层。
// 09-13 文件路径预览分支:
//   5. Click on a[data-file-path] → preventDefault + useFileViewer
//      open(原始路径)(open 内部会 useChatStore/fetch,顶部按仓库
//      惯例模块级 mock transport + store,fetch 挂全局桩防真网络)。

import { describe, it, expect, vi, afterEach } from "vitest";

vi.mock("../transport/http", () => ({
  daemonBase: vi.fn(() => "http://localhost:7456"),
}));
vi.mock("../transport/auth", () => ({
  currentDeviceToken: vi.fn(() => null),
}));
vi.mock("../stores/chat", () => ({
  useChatStore: () => ({ currentCwd: "/proj/root" }),
}));

import { useCodeBlockCopy } from "./useCodeBlockCopy";
import { useImageViewer } from "./useImageViewer";
import { useFileViewer } from "./useFileViewer";

function buildBlock(): { root: HTMLElement; btn: HTMLElement } {
  const root = document.createElement("div");
  root.setAttribute("data-code-block", "");
  const head = document.createElement("div");
  const btn = document.createElement("button");
  btn.setAttribute("data-code-copy", "");
  btn.textContent = "复制";
  head.appendChild(btn);
  const pre = document.createElement("pre");
  const code = document.createElement("code");
  code.textContent = "const x = 1;";
  pre.appendChild(code);
  root.appendChild(head);
  root.appendChild(pre);
  document.body.appendChild(root);
  return { root, btn };
}

function clickEvent(target: EventTarget): MouseEvent {
  const e = new MouseEvent("click", { bubbles: true });
  Object.defineProperty(e, "target", { value: target });
  return e;
}

/** jsdom's `navigator.clipboard` is undefined; install a stub and
 *  return it so tests can assert calls. Restored after each test. */
function stubClipboard() {
  const writeText = vi.fn<(text: string) => Promise<void>>(async () => {});
  Object.defineProperty(navigator, "clipboard", {
    value: { writeText },
    configurable: true,
  });
  return writeText;
}

afterEach(() => {
  document.body.innerHTML = "";
  // @ts-expect-error — test-only teardown of the stub
  delete navigator.clipboard;
  vi.unstubAllGlobals();
  useImageViewer().close();
  useFileViewer().close();
});

describe("useCodeBlockCopy (CH4-5)", () => {
  it("copies the block's code text and flips the label to 已复制 → 复制", async () => {
    vi.useFakeTimers();
    const writeText = stubClipboard();
    const { btn } = buildBlock();
    const { onMarkdownClick } = useCodeBlockCopy();

    await onMarkdownClick(clickEvent(btn));

    expect(writeText).toHaveBeenCalledWith("const x = 1;");
    expect(btn.textContent).toBe("已复制");

    await vi.advanceTimersByTimeAsync(2000);
    expect(btn.textContent).toBe("复制");
    vi.useRealTimers();
  });

  it("ignores clicks that don't land on the copy button", async () => {
    const writeText = stubClipboard();
    const { root } = buildBlock();
    const { onMarkdownClick } = useCodeBlockCopy();

    await onMarkdownClick(clickEvent(root));

    expect(writeText).not.toHaveBeenCalled();
  });

  it("degrades silently when the clipboard API is missing", async () => {
    const { btn } = buildBlock();
    const { onMarkdownClick } = useCodeBlockCopy();

    await expect(onMarkdownClick(clickEvent(btn))).resolves.toBeUndefined();
    expect(btn.textContent).toBe("复制");
  });

  it("no target element (synthetic event) is a safe no-op", async () => {
    const writeText = stubClipboard();
    const { onMarkdownClick } = useCodeBlockCopy();
    const e = new MouseEvent("click");

    await expect(onMarkdownClick(e)).resolves.toBeUndefined();
    expect(writeText).not.toHaveBeenCalled();
  });
});

describe("useCodeBlockCopy — image path preview (09-13)", () => {
  function buildImageLink(path: string): HTMLAnchorElement {
    const a = document.createElement("a");
    a.className = "md-image-path";
    a.setAttribute("data-image-path", path);
    a.textContent = path;
    document.body.appendChild(a);
    return a;
  }

  it("opens the viewer with the raw path and prevents navigation", async () => {
    const a = buildImageLink("out/ui-review/x/1.png");
    const { onMarkdownClick } = useCodeBlockCopy();
    const e = clickEvent(a);
    const preventSpy = vi.spyOn(e, "preventDefault");

    await onMarkdownClick(e);

    expect(preventSpy).toHaveBeenCalledTimes(1);
    expect(useImageViewer().path.value).toBe("out/ui-review/x/1.png");
  });

  it("does not open the viewer for plain links or stray clicks", async () => {
    const writeText = stubClipboard();
    const plain = document.createElement("a");
    plain.setAttribute("href", "https://example.com");
    plain.textContent = "x";
    document.body.appendChild(plain);
    const { onMarkdownClick } = useCodeBlockCopy();

    await onMarkdownClick(clickEvent(plain));
    expect(useImageViewer().isOpen.value).toBe(false);
    expect(writeText).not.toHaveBeenCalled();
  });
});

describe("useCodeBlockCopy — file path preview (09-13)", () => {
  function buildFileLink(path: string): HTMLAnchorElement {
    const a = document.createElement("a");
    a.className = "md-file-path";
    a.setAttribute("data-file-path", path);
    a.textContent = path;
    document.body.appendChild(a);
    return a;
  }

  it("opens the file viewer with the raw path and prevents navigation", async () => {
    // open() 会发起 fetch —— 挂全局桩,防止 jsdom 里打出真网络请求。
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({ ok: true, text: async () => "x" })),
    );
    const a = buildFileLink("out/a.md");
    const { onMarkdownClick } = useCodeBlockCopy();
    const e = clickEvent(a);
    const preventSpy = vi.spyOn(e, "preventDefault");

    await onMarkdownClick(e);

    expect(preventSpy).toHaveBeenCalledTimes(1);
    expect(useFileViewer().rawPath.value).toBe("out/a.md");
    expect(useFileViewer().isOpen.value).toBe(true);
  });

  it("does not open the file viewer for image links (channel separation)", async () => {
    const a = buildFileLink("out/x.png");
    // 混淆面:把同一锚点同时塞两种 data 属性 —— 委托按优先级只认
    // data-image-path,文件查看器不得被图片命中触发。
    a.setAttribute("data-image-path", "out/x.png");
    const { onMarkdownClick } = useCodeBlockCopy();

    await onMarkdownClick(clickEvent(a));

    expect(useImageViewer().isOpen.value).toBe(true);
    expect(useFileViewer().isOpen.value).toBe(false);
  });
});
