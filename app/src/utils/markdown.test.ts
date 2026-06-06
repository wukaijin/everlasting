// XSS + correctness fixtures for `renderMarkdown`.
//
// Why fixtures (not snapshot tests):
//   The contract we're protecting is "marked output goes through
//   DOMPurify, and the sanitizer strips XSS vectors". An exact
//   string-match snapshot test would break every time marked
//   tweaks whitespace or attribute order; a targeted `expect(...).not.toContain("script")`
//   test pins the *security property* and stays green across cosmetic
//   marked upgrades. Add a fixture here for every new XSS vector you
//   want protected — `pnpm test` gates the suite.

import { describe, it, expect } from "vitest";
import { renderMarkdown } from "./markdown";

describe("renderMarkdown", () => {
  describe("empty / whitespace input", () => {
    it("returns empty string for empty input", () => {
      expect(renderMarkdown("")).toBe("");
    });

    it("returns empty string for whitespace-only input", () => {
      expect(renderMarkdown("   \n\t  ")).toBe("");
    });

    it("returns empty string for null-ish values coerced to empty", () => {
      // The signature is `string`; this case is just a guard against
      // callers that pass an unexpected falsy. Should never hit in
      // production but cheap to assert.
      expect(renderMarkdown("" as string)).toBe("");
    });
  });

  describe("basic markdown", () => {
    it("renders **bold** as <strong>", () => {
      expect(renderMarkdown("**bold**")).toContain("<strong>bold</strong>");
    });

    it("renders `code` as <code>", () => {
      expect(renderMarkdown("`code`")).toContain("<code>code</code>");
    });

    it("renders a fenced python code block as <pre><code>", () => {
      const html = renderMarkdown("```py\nprint(1)\n```");
      expect(html).toContain("<pre>");
      expect(html).toContain("<code");
      expect(html).toContain("print(1)");
    });

    it("renders a link with a safe href", () => {
      const html = renderMarkdown("[x](https://example.com)");
      expect(html).toContain('href="https://example.com"');
      expect(html).toContain(">x</a>");
    });

    it("trims leading whitespace before parsing", () => {
      // Without the trim, leading `*` would be eaten by the parser
      // and produce an empty <em></em> artifact; with the trim the
      // bullet list renders normally.
      const html = renderMarkdown("\n\n* item one\n* item two");
      expect(html).toContain("<li>item one</li>");
      expect(html).toContain("<li>item two</li>");
    });
  });

  describe("XSS protection (DOMPurify is mandatory)", () => {
    it("strips raw <script> tags", () => {
      const html = renderMarkdown('<script>alert("XSS")</script>');
      expect(html).not.toContain("<script");
      expect(html).not.toContain("alert(");
    });

    it("strips <img onerror=...> handlers", () => {
      const html = renderMarkdown('<img src=x onerror=alert(1)>');
      expect(html.toLowerCase()).not.toContain("onerror");
    });

    it("strips javascript: URLs from <a> href", () => {
      const html = renderMarkdown('<a href="javascript:alert(1)">x</a>');
      // DOMPurify either drops the href entirely or replaces it with
      // `about:blank`; both outcomes are safe. The only unsafe one is
      // the literal `javascript:` scheme, so we assert against that
      // (case-insensitive — some browsers treat `JaVaScRiPt:` as
      // dangerous too).
      expect(html.toLowerCase()).not.toContain("javascript:");
    });

    it("strips javascript: URLs in markdown link syntax", () => {
      const html = renderMarkdown("[click me](javascript:alert(1))");
      expect(html.toLowerCase()).not.toContain("javascript:");
    });

    it("strips inline event handlers on nested elements", () => {
      const html = renderMarkdown(
        '<div onclick="alert(1)">hi <span onmouseover="alert(2)">there</span></div>',
      );
      expect(html.toLowerCase()).not.toContain("onclick");
      expect(html.toLowerCase()).not.toContain("onmouseover");
    });

    it("strips <iframe> entirely", () => {
      const html = renderMarkdown(
        '<iframe src="https://evil.example"></iframe>',
      );
      expect(html.toLowerCase()).not.toContain("<iframe");
    });
  });
});
