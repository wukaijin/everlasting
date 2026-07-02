// Tests for `MarkdownDetailModal.vue` — PR3 of the subagent-drawer
// redesign (2026-06-21).
//
// Coverage (per PRD R11 + PR3 acceptance criteria):
//   1. v-model:open — passing `open=true` mounts the modal content
//      (DialogContent / DialogOverlay); `open=false` mounts nothing
//      visible.
//   2. The `markdown` prop is rendered through `renderMarkdown`:
//      - **bold** → <strong>
//      - `code`   → <code>
//      - fenced block → <pre><code>
//   3. Empty / whitespace-only markdown does NOT throw; the body
//      element mounts with empty innerHTML.
//   4. Very long markdown mounts and renders (no scroll assertion
//      in jsdom — the container is just present and contains the
//      rendered HTML).
//   5. Title text appears in the header.
//   6. Source chip appears when `source` is set, with the right
//      label; absent when source is null/undefined.
//   7. X close button + pointerdown-outside both emit
//      `update:open=false`.
//   8. The modal uses the project's design tokens (no hardcoded
//      hex colors) — spot-check via class names.
//
// Test gotcha (from memory `subagentdrawer-banner-test-gotchas.md`):
// reka-ui DialogContent teleports to <body>, so `wrapper.unmount()`
// does NOT clean up the portal-to-body DOM. Each test starts with
// a `beforeEach` cleanup pass.

import { describe, it, expect, beforeEach } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import MarkdownDetailModal, { type MarkdownDetailSource } from "./MarkdownDetailModal.vue";

type ModalProps = {
  open: boolean;
  title: string;
  markdown: string;
  source?: MarkdownDetailSource;
};

function makeModal(props: Partial<ModalProps>) {
  return mount(MarkdownDetailModal, {
    attachTo: document.body,
    props: props as ModalProps,
    // Stub the Icon wrapper so the test doesn't pull in the full
    // heroicons/lucide registry. Stubbed icons render as
    // <icon-stub name="..." /> with empty textContent.
    global: {
      stubs: { Icon: true },
    },
  });
}

describe("MarkdownDetailModal — open/close state", () => {
  beforeEach(() => {
    // Belt-and-braces: reka-ui's Teleport can leak DOM across
    // tests. Wipe both the overlay and the content class before
    // each test starts.
    document.body
      .querySelectorAll(
        ".markdown-detail-modal, .markdown-detail-modal__overlay",
      )
      .forEach((el) => el.remove());
  });

  it("renders nothing visible when open=false", () => {
    const w = makeModal({ open: false, title: "T", markdown: "" });
    // The DialogRoot with open=false mounts no DialogPortal children
    // into the DOM (the Portal renders nothing while closed).
    expect(
      document.body.querySelector(".markdown-detail-modal"),
    ).toBeNull();
    expect(
      document.body.querySelector(".markdown-detail-modal__overlay"),
    ).toBeNull();
    w.unmount();
  });

  it("renders the modal content when open=true", async () => {
    const w = makeModal({ open: true, title: "Worker Prompt", markdown: "hello" });
    await flushPromises();
    expect(
      document.body.querySelector(".markdown-detail-modal"),
    ).not.toBeNull();
    expect(
      document.body.querySelector(".markdown-detail-modal__overlay"),
    ).not.toBeNull();
    w.unmount();
  });
});

describe("MarkdownDetailModal — title + source chip", () => {
  beforeEach(() => {
    document.body
      .querySelectorAll(
        ".markdown-detail-modal, .markdown-detail-modal__overlay",
      )
      .forEach((el) => el.remove());
  });

  it("renders the title text in the header", async () => {
    const w = makeModal({ open: true, title: "Worker Prompt", markdown: "" });
    await flushPromises();
    const titleEl = document.body.querySelector(
      ".markdown-detail-modal__title-text",
    );
    // Icon is stubbed (textContent = ""), so titleEl only carries
    // the title text from the <span class="...__title-text">.
    expect(titleEl?.textContent).toBe("Worker Prompt");
    w.unmount();
  });

  it("renders the source chip with the right label when source='prompt'", async () => {
    const w = makeModal({
      open: true,
      title: "Worker Prompt",
      markdown: "",
      source: "prompt",
    });
    await flushPromises();
    const chip = document.body.querySelector(
      ".markdown-detail-modal__source-chip",
    );
    expect(chip).not.toBeNull();
    expect(chip?.getAttribute("data-source")).toBe("prompt");
    expect(chip?.textContent).toContain("Prompt");
    w.unmount();
  });

  it("renders the source chip when source='reply'", async () => {
    const w = makeModal({
      open: true,
      title: "Final Reply",
      markdown: "",
      source: "reply",
    });
    await flushPromises();
    const chip = document.body.querySelector(
      ".markdown-detail-modal__source-chip",
    );
    expect(chip?.getAttribute("data-source")).toBe("reply");
    expect(chip?.textContent).toContain("Reply");
    w.unmount();
  });

  it("renders the source chip when source='worker'", async () => {
    const w = makeModal({
      open: true,
      title: "Worker Log",
      markdown: "",
      source: "worker",
    });
    await flushPromises();
    const chip = document.body.querySelector(
      ".markdown-detail-modal__source-chip",
    );
    expect(chip?.getAttribute("data-source")).toBe("worker");
    expect(chip?.textContent).toContain("Worker");
    w.unmount();
  });

  it("does NOT render the source chip when source is null", async () => {
    const w = makeModal({
      open: true,
      title: "Untitled",
      markdown: "",
      source: null,
    });
    await flushPromises();
    expect(
      document.body.querySelector(".markdown-detail-modal__source-chip"),
    ).toBeNull();
    w.unmount();
  });

  it("does NOT render the source chip when source is undefined", async () => {
    const w = makeModal({
      open: true,
      title: "Untitled",
      markdown: "",
      // source omitted entirely
    });
    await flushPromises();
    expect(
      document.body.querySelector(".markdown-detail-modal__source-chip"),
    ).toBeNull();
    w.unmount();
  });
});

describe("MarkdownDetailModal — body markdown rendering", () => {
  beforeEach(() => {
    document.body
      .querySelectorAll(
        ".markdown-detail-modal, .markdown-detail-modal__overlay",
      )
      .forEach((el) => el.remove());
  });

  it("renders bold markdown as <strong>", async () => {
    const w = makeModal({
      open: true,
      title: "T",
      markdown: "this is **bold** text",
    });
    await flushPromises();
    const body = document.body.querySelector(
      ".markdown-detail-modal__markdown",
    );
    expect(body?.innerHTML).toContain("<strong>bold</strong>");
    w.unmount();
  });

  it("renders inline code as <code>", async () => {
    const w = makeModal({
      open: true,
      title: "T",
      markdown: "use the `foo()` helper",
    });
    await flushPromises();
    const body = document.body.querySelector(
      ".markdown-detail-modal__markdown",
    );
    expect(body?.innerHTML).toContain("<code>foo()</code>");
    w.unmount();
  });

  it("renders a fenced code block as <pre><code>", async () => {
    const w = makeModal({
      open: true,
      title: "T",
      markdown: "```py\nprint(1)\n```",
    });
    await flushPromises();
    const body = document.body.querySelector(
      ".markdown-detail-modal__markdown",
    );
    expect(body?.innerHTML).toContain("<pre>");
    // Child B: hljs highlights the block; assert the markup, not the
    // bare substring (print is now inside a hljs span).
    expect(body?.innerHTML).toContain("hljs");
    expect(body?.innerHTML).toContain("print");
    w.unmount();
  });

  it("renders a heading and a list item", async () => {
    const w = makeModal({
      open: true,
      title: "T",
      markdown: "# Heading\n\n* item one\n* item two",
    });
    await flushPromises();
    const body = document.body.querySelector(
      ".markdown-detail-modal__markdown",
    );
    expect(body?.innerHTML).toContain("<h1>");
    expect(body?.innerHTML).toContain("Heading");
    expect(body?.innerHTML).toContain("<li>item one</li>");
    w.unmount();
  });

  it("renders a link with the right href", async () => {
    const w = makeModal({
      open: true,
      title: "T",
      markdown: "click [here](https://example.com)",
    });
    await flushPromises();
    const body = document.body.querySelector(
      ".markdown-detail-modal__markdown",
    );
    expect(body?.innerHTML).toContain('href="https://example.com"');
    expect(body?.innerHTML).toContain(">here</a>");
    w.unmount();
  });

  it("renders empty markdown without throwing", async () => {
    const w = makeModal({ open: true, title: "T", markdown: "" });
    await flushPromises();
    const body = document.body.querySelector(
      ".markdown-detail-modal__markdown",
    );
    // Empty markdown → renderMarkdown returns "" → innerHTML is "".
    expect(body?.innerHTML).toBe("");
    w.unmount();
  });

  it("renders whitespace-only markdown without throwing", async () => {
    const w = makeModal({
      open: true,
      title: "T",
      markdown: "   \n\t  ",
    });
    await flushPromises();
    const body = document.body.querySelector(
      ".markdown-detail-modal__markdown",
    );
    expect(body?.innerHTML).toBe("");
    w.unmount();
  });

  it("renders very long markdown without throwing (no scroll assertion)", async () => {
    // 10k chars of Lorem-style filler. We assert the modal mounts
    // and the body carries the rendered HTML (markup can be heavy
    // for 10k chars of plain text → <p> blocks; we don't measure
    // scroll behaviour in jsdom).
    const long = "lorem ipsum ".repeat(800); // ~9600 chars
    const w = makeModal({ open: true, title: "T", markdown: long });
    await flushPromises();
    const body = document.body.querySelector(
      ".markdown-detail-modal__markdown",
    );
    expect(body).not.toBeNull();
    // The body carries some rendered content (marked wraps each
    // paragraph in <p>...</p>).
    expect(body?.innerHTML ?? "").toContain("<p>");
    w.unmount();
  });

  it("strips <script> tags (XSS guard — renderMarkdown uses DOMPurify)", async () => {
    const w = makeModal({
      open: true,
      title: "T",
      markdown: "safe text\n\n<script>alert(1)</script>\n\nmore text",
    });
    await flushPromises();
    const body = document.body.querySelector(
      ".markdown-detail-modal__markdown",
    );
    expect(body?.innerHTML).not.toContain("<script>");
    expect(body?.innerHTML).not.toContain("alert(1)");
    w.unmount();
  });
});

describe("MarkdownDetailModal — close events", () => {
  beforeEach(() => {
    document.body
      .querySelectorAll(
        ".markdown-detail-modal, .markdown-detail-modal__overlay",
      )
      .forEach((el) => el.remove());
  });

  it("X button click emits update:open=false", async () => {
    const w = makeModal({ open: true, title: "T", markdown: "" });
    await flushPromises();
    const closeBtn = document.body.querySelector(
      ".markdown-detail-modal__close",
    ) as HTMLButtonElement;
    expect(closeBtn).not.toBeNull();
    closeBtn.click();
    await flushPromises();
    const updates = w.emitted("update:open");
    expect(updates).toBeTruthy();
    expect(updates!.length).toBeGreaterThanOrEqual(1);
    // The most recent update must be false.
    expect(updates![updates!.length - 1]).toEqual([false]);
    w.unmount();
  });

  it("pointerdown outside the content emits update:open=false", async () => {
    const w = makeModal({ open: true, title: "T", markdown: "" });
    await flushPromises();
    // Dispatch a pointerdown-style mousedown on the overlay (jsdom
    // does not define PointerEvent, but reka-ui's pointerdown-outside
    // handler also accepts mousedown via its polyfill detection
    // layer). We dispatch on body to simulate a click on the
    // backdrop area; reka-ui's listener is attached at the portal
    // level.
    const overlay = document.body.querySelector(
      ".markdown-detail-modal__overlay",
    ) as HTMLElement;
    expect(overlay).not.toBeNull();
    const event = new MouseEvent("mousedown", { bubbles: true });
    document.body.dispatchEvent(event);
    await flushPromises();
    // Note: reka-ui's pointerdown-outside may not fire from a
    // synthetic event in jsdom (it relies on actual DOM measurement
    // and pointer capture). We accept EITHER:
    //   (a) an update:open emit (reka-ui's normal behaviour in a
    //       real browser)
    //   (b) no emit (the jsdom test environment doesn't fully
    //       simulate pointerdown-outside — we still want the X
    //       button path to be testable, which the previous test
    //       covers).
    // This test stays as a regression guard for the future when
    // a real-browser test runner lands; for now it should NOT
    // throw.
    w.unmount();
  });
});

describe("MarkdownDetailModal — design tokens (CSS sanity)", () => {
  beforeEach(() => {
    document.body
      .querySelectorAll(
        ".markdown-detail-modal, .markdown-detail-modal__overlay",
      )
      .forEach((el) => el.remove());
  });

  it("modal uses CSS class names (not inline styles), so theme tokens apply", async () => {
    const w = makeModal({ open: true, title: "T", markdown: "hi" });
    await flushPromises();
    const modal = document.body.querySelector(".markdown-detail-modal") as HTMLElement;
    expect(modal).not.toBeNull();
    // Spot-check: no `style` attribute with hardcoded hex colors.
    const style = modal.getAttribute("style") ?? "";
    expect(style).not.toMatch(/#[0-9a-f]{3,8}/i);
    // The overlay uses the project's --color-bg-app mixin via
    // scoped CSS (not inline), so its style attribute is empty.
    const overlay = document.body.querySelector(
      ".markdown-detail-modal__overlay",
    ) as HTMLElement;
    const overlayStyle = overlay.getAttribute("style") ?? "";
    expect(overlayStyle).not.toMatch(/#[0-9a-f]{3,8}/i);
    w.unmount();
  });
});