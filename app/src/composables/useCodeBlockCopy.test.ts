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

import { describe, it, expect, vi, afterEach } from "vitest";
import { useCodeBlockCopy } from "./useCodeBlockCopy";
import { useImageViewer } from "./useImageViewer";

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

  afterEach(() => {
    useImageViewer().close();
  });

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
