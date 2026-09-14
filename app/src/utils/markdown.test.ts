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

import { beforeEach, describe, it, expect } from "vitest";
import {
  createDebouncedRenderer,
  linkifyPlainText,
  renderMarkdown,
} from "./markdown";
import {
  resetExistenceForTests,
  setExistenceForTests,
} from "./pathExistence";

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
      // Child B: hljs highlights the block — "print" is wrapped in a
      // hljs span, so the bare "print(1)" substring no longer appears.
      expect(html).toContain("hljs");
      expect(html).toContain("print");
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

  // B1 (2026-08-16) R7: img src two-state allow-list. External
  // images (any src not on our attachments route) must NOT render
  // as <img> — they become a new-tab link, so no network request
  // fires when the bubble mounts. Self-hosted forms pass through.
  describe("img allow-list (B1 R7)", () => {
    it("downgrades an external markdown image to a link (no <img>, has <a>)", () => {
      const html = renderMarkdown("![](http://evil.com/x.png)");
      expect(html).not.toContain("<img");
      expect(html).toContain("<a");
      expect(html).toContain('href="http://evil.com/x.png"');
      expect(html).toContain("[图片]");
      // The opener attributes survive DOMPurify (ADD_ATTR).
      expect(html).toContain('target="_blank"');
      expect(html).toContain('rel="noreferrer"');
    });

    it("downgrades a raw HTML <img> with an external src", () => {
      const html = renderMarkdown(
        '<img src="https://cdn.example.com/pic.png" alt="ad">',
      );
      expect(html).not.toContain("<img");
      expect(html).toContain('href="https://cdn.example.com/pic.png"');
    });

    it("keeps a self-hosted relative attachment img", () => {
      const html = renderMarkdown("![](/api/v1/attachments/s1/a1b2c3d4e5f6.png)");
      expect(html).toContain("<img");
      expect(html).toContain('src="/api/v1/attachments/s1/a1b2c3d4e5f6.png"');
    });

    it("keeps a daemonBase-absolute attachment img (DEV cross origin)", () => {
      // vitest jsdom: import.meta.env.DEV is true and no ?daemonUrl
      // query is set, so daemonBase() resolves to http://localhost:7456
      // (deterministic for this fixture).
      const html = renderMarkdown(
        "![](http://localhost:7456/api/v1/attachments/s1/a1.png)",
      );
      expect(html).toContain("<img");
      expect(html).toContain(
        'src="http://localhost:7456/api/v1/attachments/s1/a1.png"',
      );
    });

    it("keeps the pwa-remote proxy form with access_token query (prefix match)", () => {
      const html = renderMarkdown(
        "![](http://localhost:7456/api/v1/proxy/api/v1/attachments/s1/a1.png?access_token=tok)",
      );
      expect(html).toContain("<img");
    });
  });

  // BUGLIST CH4-5 (2026-08-29): fenced code blocks are wrapped in a
  // `.md-code` chrome (language label + copy button). The button is
  // raw HTML — Vue listeners don't survive v-html — so it carries
  // `data-code-copy` / `data-code-block` hooks consumed by the
  // delegated handler in `composables/useCodeBlockCopy.ts`.
  describe("fenced code block chrome (CH4-5)", () => {
    it("wraps a fenced block in the md-code card with lang label + copy button", () => {
      const html = renderMarkdown("```py\nprint(1)\n```");
      expect(html).toContain('class="md-code" data-code-block');
      expect(html).toContain('class="md-code__lang">py<');
      expect(html).toContain('data-code-copy');
      expect(html).toContain(">复制</button>");
      // The highlighted code still rides inside the wrapper.
      expect(html).toContain("hljs");
      expect(html).toContain("language-py");
      expect(html).toContain("print");
    });

    it("labels a fence without a language as 'code' and skips the language class", () => {
      const html = renderMarkdown("```\nplain\n```");
      expect(html).toContain('class="md-code__lang">code<');
      expect(html).not.toContain("language-");
    });

    it("keeps only the first word of the info string as the label", () => {
      const html = renderMarkdown('```ts title="x"\nlet a = 1;\n```');
      expect(html).toContain('class="md-code__lang">ts<');
      expect(html).toContain("language-ts");
      expect(html).not.toContain('md-code__lang">ts title');
    });

    it("inline code stays bare (no chrome)", () => {
      const html = renderMarkdown("`x`");
      expect(html).not.toContain("data-code-block");
      expect(html).not.toContain("data-code-copy");
    });

    it("chrome survives DOMPurify with no event-handler attributes", () => {
      const html = renderMarkdown("```js\nalert(1)\n```");
      expect(html).toContain("<button");
      expect(html).not.toContain("onclick");
      expect(html).not.toContain("<script");
    });
  });

  // 09-13 图片路径预览:三种形态统一 linkify 成
  // `<a class="md-image-path" data-image-path>`(点击经 useCodeBlockCopy
  // 委托开 ImageViewerModal)。围栏代码块内不动;纯文件名 / URL / 我们
  // 自己的 API 路径不识别。
  describe("image path linkify (09-13)", () => {
    it("linkifies a bare relative path in prose", () => {
      const html = renderMarkdown("see out/ui-review/123/1-desktop.png here");
      expect(html).toContain('data-image-path="out/ui-review/123/1-desktop.png"');
      expect(html).toContain('class="md-image-path"');
      // 链接文本即路径本体。
      expect(html).toContain(">out/ui-review/123/1-desktop.png</a>");
    });

    it("linkifies absolute and ~ paths", () => {
      expect(renderMarkdown("at /tmp/shot.png ok")).toContain(
        'data-image-path="/tmp/shot.png"',
      );
      expect(renderMarkdown("at ~/.local/share/app/x.png ok")).toContain(
        'data-image-path="~/.local/share/app/x.png"',
      );
      expect(renderMarkdown("at ./out/x.png ok")).toContain(
        'data-image-path="./out/x.png"',
      );
      expect(renderMarkdown("at ../out/x.png ok")).toContain(
        'data-image-path="../out/x.png"',
      );
    });

    it("linkifies a path inside inline code (keeps the code wrapper)", () => {
      const html = renderMarkdown("截图在 `out/ui-review/x/1.png`");
      // inline code 保留 mono 包装,内部文本换成同文本 <a>。
      expect(html).toMatch(/<code>\s*<a[^>]*data-image-path="out\/ui-review\/x\/1\.png"[^>]*>/);
      expect(html).toContain("</a></code>");
    });

    it("does NOT touch paths inside fenced code blocks", () => {
      const html = renderMarkdown("```sh\ncat out/ui-review/x/1.png\n```");
      expect(html).not.toContain("data-image-path");
    });

    it("stamps data-image-path onto a markdown link with a local image href", () => {
      // LLM 常输出 [x.png](out/x.png) 链接形态:href 导航会被点击委托
      // 拦下走弹层;这里断言 data 属性补齐 + 原 href 保留(委托失败时
      // 不至于无路可走)。
      const html = renderMarkdown("[1-desktop.png](out/ui-review/x/1.png)");
      expect(html).toContain('data-image-path="out/ui-review/x/1.png"');
      expect(html).toContain('href="out/ui-review/x/1.png"');
    });

    it("rewrites a markdown image with a LOCAL path to a preview link", () => {
      // B1 R7 的本地分支:不再是打不开的新 tab 链接,而是预览链接。
      const html = renderMarkdown("![](out/ui-review/x/1.png)");
      expect(html).not.toContain("<img");
      expect(html).toContain('data-image-path="out/ui-review/x/1.png"');
      expect(html).toContain("[图片]");
      expect(html).not.toContain('target="_blank"');
    });

    it("supports CJK segments and paths at CJK punctuation boundaries", () => {
      expect(renderMarkdown("产物在 out/截图/首页.png。")).toContain(
        'data-image-path="out/截图/首页.png"',
      );
    });

    it("ignores bare filenames, URLs and our own api paths", () => {
      // 纯文件名(无路径分隔符):句子误伤率高,有意不识别。
      expect(renderMarkdown("generated foo.png today")).not.toContain("data-image-path");
      // http(s) URL(裸文本经 gfm autolink 成 <a>,walk 跳过 a;
      // 链接语法 href 走 isLocalImagePath 的 URL 排除)。
      expect(renderMarkdown("see https://example.com/a.png")).not.toContain("data-image-path");
      expect(renderMarkdown("[x](https://example.com/a.png)")).not.toContain("data-image-path");
      // 附件路由(uuid.png 结尾)不进预览(那是 <img> 直渲染的通道)。
      expect(renderMarkdown("[x](/api/v1/attachments/s1/a1b2c3d4.png)")).not.toContain(
        "data-image-path",
      );
    });

    it("escapes attribute values in the downgrade branch", () => {
      // 属性注入载荷:marked 把 src 里的引号 percent-encode(`%22`),
      // 属性边界不被打破 —— "onerror" 只能以 URL 文本形态存在,不能
      // 成为 <a> 的属性。断言"无事件 handler 属性",而非无子串。
      const html = renderMarkdown('![](out/x"onerror="alert(1).png)');
      expect(html).not.toMatch(/<a[^>]*\sonerror/i);
      expect(html).not.toMatch(/<a[^>]*\sonclick/i);
      expect(html).not.toContain("<img");
    });
  });

  // 09-13 同日文件通道:识别全集泛化到文本类 + pdf,非图片命中产出
  // `<a class="md-file-path" data-file-path>`(点击开 FileViewerModal;
  // pdf 由弹层 composable 分派到新标签)。段语法/边界/排除项与图片
  // 通道全同;命中按扩展分流 —— 图片扩展仍走 data-image-path 通道。
  describe("file path linkify (09-13 文件通道)", () => {
    it("linkifies a bare relative markdown path in prose", () => {
      const html = renderMarkdown("see out/report.md here");
      expect(html).toContain('data-file-path="out/report.md"');
      expect(html).toContain('class="md-file-path"');
      // 链接文本即路径本体。
      expect(html).toContain(">out/report.md</a>");
    });

    it("linkifies the four source shapes (AC1): prose / inline code / link syntax / ~-prefix", () => {
      // ① 正文裸路径。
      expect(renderMarkdown("看 src/main.rs 就知道")).toContain(
        'data-file-path="src/main.rs"',
      );
      // ② inline code 内路径(保留 code 包装)。
      const codeHtml = renderMarkdown("入口在 `src/main.rs`");
      expect(codeHtml).toMatch(
        /<code>\s*<a[^>]*data-file-path="src\/main\.rs"[^>]*>/,
      );
      expect(codeHtml).toContain("</a></code>");
      // ③ markdown 链接语法(补 data 属性,原 href 保留)。
      const linkHtml = renderMarkdown("[日志](logs/x.log)");
      expect(linkHtml).toContain('data-file-path="logs/x.log"');
      expect(linkHtml).toContain('href="logs/x.log"');
      // ④ `~/` 前缀。
      expect(renderMarkdown("笔记在 ~/notes/TODO.md 里")).toContain(
        'data-file-path="~/notes/TODO.md"',
      );
    });

    it("linkifies absolute, ./ and ../ forms across text/pdf extensions", () => {
      expect(renderMarkdown("build log at /tmp/build.log ok")).toContain(
        'data-file-path="/tmp/build.log"',
      );
      expect(renderMarkdown("see ./scripts/check.ts ok")).toContain(
        'data-file-path="./scripts/check.ts"',
      );
      expect(renderMarkdown("see ../docs/spec.pdf ok")).toContain(
        'data-file-path="../docs/spec.pdf"',
      );
      expect(renderMarkdown("产物 out/data.jsonl 落盘")).toContain(
        'data-file-path="out/data.jsonl"',
      );
    });

    it("keeps image extensions on the image channel (dispatch by extension)", () => {
      const html = renderMarkdown("see out/shot.png here");
      expect(html).toContain('data-image-path="out/shot.png"');
      expect(html).not.toContain("data-file-path");
      expect(html).not.toContain("md-file-path");
    });

    it("ignores bare filenames, URLs, /api/ links and fenced blocks (AC1 exclusions)", () => {
      // 纯文件名(无路径分隔符)不识别。
      expect(renderMarkdown("generated index.ts today")).not.toContain(
        "data-file-path",
      );
      // http(s) URL 不识别(裸文本经 autolink 成 <a>,walk 跳过 a;
      // 链接语法 href 走 isLocalFilePath 的 URL 排除)。
      expect(renderMarkdown("see https://host/x.md")).not.toContain(
        "data-file-path",
      );
      expect(renderMarkdown("[x](https://host/x.md)")).not.toContain(
        "data-file-path",
      );
      // 附件路由 /api/ 前缀不进预览(那是 <img> 直渲染的通道)。
      expect(renderMarkdown("[x](/api/v1/attachments/s1/a1b2c3d4.md)")).not.toContain(
        "data-file-path",
      );
      // 围栏代码块内不动。
      const fenced = renderMarkdown("```\ncat out/report.md\n```");
      expect(fenced).not.toContain("data-file-path");
      expect(fenced).not.toContain("data-image-path");
    });

    it("rewrites a markdown image with a LOCAL non-image path to a [文件] preview link", () => {
      // ![](本地文件) 无渲染意义;旧实现退化成相对 href 新标签链接
      // (打穿 SPA 路由的存量 wart),现在产 [文件] 预览链接。
      const html = renderMarkdown("![](out/notes.md)");
      expect(html).not.toContain("<img");
      expect(html).toContain('data-file-path="out/notes.md"');
      expect(html).toContain("[文件]");
      expect(html).not.toContain('target="_blank"');
    });

    it("keeps ![](local image) on the [图片] channel (zero regression)", () => {
      const html = renderMarkdown("![](out/ui-review/x/1.png)");
      expect(html).not.toContain("<img");
      expect(html).toContain('data-image-path="out/ui-review/x/1.png"');
      expect(html).toContain("[图片]");
      expect(html).not.toContain("data-file-path");
    });
  });

  // 09-14 存在性闸门:确认缺失(stat 404 结果落缓存)的路径不产锚点,
  // 保留原文;未知路径乐观产锚(渲染零阻塞)。测试用
  // setExistenceForTests 直播种缓存 —— vitest 下 pathExistence 默认禁网
  // (见该模块"测试隔离"注释),未知路径不会真发 fetch。
  describe("existence gating (09-14)", () => {
    beforeEach(() => resetExistenceForTests());

    it("keeps the optimistic anchor for unknown paths (default state)", () => {
      // 未知 = 乐观:这是 SSE 流式期间唯一可用的同步答案。
      const html = renderMarkdown("see /tmp/maybe/shot.png here");
      expect(html).toContain('data-image-path="/tmp/maybe/shot.png"');
    });

    it("downgrades a confirmed-missing bare-text path to plain text", () => {
      setExistenceForTests("/tmp/gone/report.md", false);
      const html = renderMarkdown("看 /tmp/gone/report.md 之前");
      expect(html).not.toContain("data-file-path");
      expect(html).not.toContain("<a");
      expect(html).toContain("/tmp/gone/report.md");
    });

    it("downgrades inside inline code too (code wrapper kept, no anchor)", () => {
      setExistenceForTests("/tmp/gone/main.rs", false);
      const html = renderMarkdown("入口在 `/tmp/gone/main.rs`");
      expect(html).toContain("<code>");
      expect(html).not.toContain("data-file-path");
      expect(html).toContain("/tmp/gone/main.rs");
    });

    it("unwraps a confirmed-missing markdown link to its label text", () => {
      setExistenceForTests("/tmp/gone/x.log", false);
      const html = renderMarkdown("[日志](/tmp/gone/x.log)");
      expect(html).not.toContain("<a");
      expect(html).not.toContain("data-file-path");
      expect(html).toContain("日志");
    });

    it("downgrades ![](local) with a confirmed-missing path to plain [图片]", () => {
      setExistenceForTests("/tmp/gone/1.png", false);
      const html = renderMarkdown("![](/tmp/gone/1.png)");
      expect(html).not.toContain("<img");
      expect(html).not.toContain("<a");
      expect(html).toContain("[图片]");
    });

    it("degrades only the missing path when existing and missing mix", () => {
      setExistenceForTests("/tmp/keep/a.md", true);
      setExistenceForTests("/tmp/gone/b.md", false);
      const html = renderMarkdown("/tmp/keep/a.md 与 /tmp/gone/b.md");
      expect(html).toContain('data-file-path="/tmp/keep/a.md"');
      expect(html).not.toContain('data-file-path="/tmp/gone/b.md"');
      expect(html).toContain("/tmp/gone/b.md");
    });
  });
});

// createDebouncedRenderer 的存在性补偿重渲染(09-14):渲染跑在
// setTimeout 里无 effect scope,pathExistence 的 reactive 缓存帮不上忙,
// 靠 onPathsResolved 订阅。setExistenceForTests 变更值时走同一条
// notify 通道(微任务合并),借此驱动断言。
describe("createDebouncedRenderer existence re-render (09-14)", () => {
  beforeEach(() => resetExistenceForTests());

  it("re-renders and downgrades the anchor when a missing result lands", async () => {
    const r = createDebouncedRenderer(50);
    r.schedule("see /tmp/gone2/a.md here");
    r.flush();
    expect(r.rendered.value).toContain('data-file-path="/tmp/gone2/a.md"');
    setExistenceForTests("/tmp/gone2/a.md", false);
    await Promise.resolve(); // 等微任务合并的一拍广播
    expect(r.rendered.value).not.toContain("data-file-path");
    expect(r.rendered.value).toContain("/tmp/gone2/a.md");
    r.dispose();
  });

  it("skips re-render when the resolved path is absent from its text", async () => {
    const r = createDebouncedRenderer(50);
    r.schedule("no local paths in here");
    r.flush();
    const before = r.rendered.value;
    setExistenceForTests("/tmp/unrelated/x.md", false);
    await Promise.resolve();
    expect(r.rendered.value).toBe(before);
    r.dispose();
  });

  it("stops re-rendering after dispose", async () => {
    const r = createDebouncedRenderer(50);
    r.schedule("see /tmp/gone3/a.md here");
    r.flush();
    r.dispose();
    setExistenceForTests("/tmp/gone3/a.md", false);
    await Promise.resolve();
    expect(r.rendered.value).toContain('data-file-path="/tmp/gone3/a.md"');
  });
});

// linkifyPlainText — 工具输出 `<pre>` 面的非 markdown linkify(2026-09-13):
// 整体转义 → 路径插锚(已转义切片)→ DOMPurify。契约:
//   1. 路径转锚点(图片扩展归 data-image-path,其余 data-file-path);
//   2. 任何 HTML(含 <script>)只以转义文本存在,绝不产生可执行标记;
//   3. 产出仍过 DOMPurify(仓库不变量,v-html 消费面)。
describe("linkifyPlainText", () => {
  it("turns a local file path into an anchor", () => {
    const html = linkifyPlainText("wrote out/a.md");
    expect(html).toContain('data-file-path="out/a.md"');
    expect(html).toContain(">out/a.md</a>");
  });

  it("routes image extensions to the image channel", () => {
    expect(linkifyPlainText("saved out/x/shot.png today")).toContain(
      'data-image-path="out/x/shot.png"',
    );
  });

  it("escapes raw HTML so <script> never becomes executable markup", () => {
    const html = linkifyPlainText('<script>alert(1)</script> out/a.md');
    expect(html.toLowerCase()).not.toContain("<script");
    expect(html.toLowerCase()).not.toContain("onerror");
    // 路径插锚不受相邻 HTML 文本影响。
    expect(html).toContain('data-file-path="out/a.md"');
  });

  it("leaves plain text without paths as pure escaped text (no anchors)", () => {
    const html = linkifyPlainText("plain exit 0");
    expect(html).not.toContain("<a");
    expect(html).toContain("plain exit 0");
  });

  it("handles paths at CJK punctuation boundaries", () => {
    expect(linkifyPlainText("产物 out/报告.md。后续")).toContain(
      'data-file-path="out/报告.md"',
    );
  });

  // 09-14 存在性闸门(同 renderMarkdown 的 existence gating describe,
  // 契约一致:确认缺失不产锚,未知乐观产锚)。
  describe("existence gating (09-14)", () => {
    beforeEach(() => resetExistenceForTests());

    it("keeps the optimistic anchor for an unknown path", () => {
      expect(linkifyPlainText("wrote /tmp/maybe/a.md")).toContain(
        'data-file-path="/tmp/maybe/a.md"',
      );
    });

    it("returns the escaped original match for a confirmed-missing path", () => {
      setExistenceForTests("/tmp/gone/a.md", false);
      const html = linkifyPlainText("wrote /tmp/gone/a.md today");
      expect(html).not.toContain("<a");
      expect(html).toContain("/tmp/gone/a.md");
    });
  });
});
