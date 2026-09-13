// Markdown rendering for assistant chat messages.
//
// Why a dedicated module (instead of inline in MessageItem.vue):
//   - Testable in isolation (vitest can drive the XSS fixtures without
//     spinning up a component)
//   - Single source of truth for the marked + DOMPurify configuration
//     (any future UI surface that needs markdown reuses the same path)
//   - Trims leading whitespace *before* parsing so the markdown parser
//     never eats the first character as syntax (e.g. leading `*` would
//     otherwise start a list mid-asterisk)
//
// XSS story (locked):
//   marked v8+ REMOVED its `sanitize` option — there is no safe built-in
//   way to get sanitized output from marked alone. Every call into
//   `renderMarkdown` MUST pass through `DOMPurify.sanitize`. The
//   `vitest` fixture suite in `markdown.test.ts` asserts this on a
//   representative set of attack vectors; CI gates the suite.
//
// Streaming story (locked):
//   ChatPanel feeds `displayContent` (already trimmed) into
//   `createDebouncedRenderer` so a 50ms quiet window collapses the
//   torrent of SSE deltas into one render. On `streaming=false` the
//   caller invokes `flush()` to render the final frame immediately.

import { marked, type Tokens } from "marked";
import { markedHighlight } from "marked-highlight";
import DOMPurify, { type Config as DOMPurifyConfig } from "dompurify";
import { ref, type Ref } from "vue";
import { renderCodeHtml } from "./highlight";
import { daemonBase } from "../transport/http";

// --- marked configuration ----------------------------------------------
// Configure once at module load. `marked.setOptions` mutates the
// singleton; subsequent calls in the same process inherit these
// options. `gfm: true` enables tables / strikethrough / task lists /
// autolinks (we want all of those). `breaks: true` turns single
// newlines into <br> — matches the previous `white-space: pre-wrap`
// behavior the bubble had before markdown landed.
marked.setOptions({
  gfm: true,
  breaks: true,
});

// B9 Child B (2026-07-02): wire hljs into the markdown pipeline so
// ```lang fenced code blocks in assistant prose get syntax highlighting.
// marked-highlight calls the shared `renderCodeHtml` (the same helper
// `<CodeBlockPrimitive>` uses), so language support never diverges
// between the two entry points. The highlighted HTML then goes through
// the existing DOMPurify pass in `renderMarkdown` — hljs emits escaped
// `<span class="hljs-*">` which the default html profile keeps (the
// markdown.test.ts XSS fixtures guard against regression).
marked.use(
  markedHighlight({
    langPrefix: "hljs language-",
    emptyLangClass: "hljs",
    highlight(code: string, lang: string) {
      return renderCodeHtml(code, (lang ?? "").toLowerCase());
    },
  }),
);

// CH4-5 (2026-08-29): fenced code block chrome. marked-highlight leaves
// the highlighted HTML in `token.text` with `escaped: true`; the default
// `code` renderer would emit a bare `<pre><code>`. We wrap it in a
// `.md-code` card carrying a language label + a copy button. The copy
// button is raw HTML (no Vue listeners survive v-html) — the click is
// handled by delegation in `composables/useCodeBlockCopy.ts` via the
// `data-code-block` / `data-code-copy` hooks. Per-container chrome CSS
// is mirrored next to each container's `:deep(pre)` block (see
// .trellis/spec/frontend/chat/message-list-and-markdown.md §2).
marked.use({
  renderer: {
    code({ text, lang, escaped }: Tokens.Code): string {
      const langString = (lang ?? "").match(/^\S*/)?.[0] ?? "";
      const label = langString || "code";
      const codeHtml = escaped ? text : escapeHtml(text);
      const langClass = langString ? ` language-${escapeHtml(langString)}` : "";
      return (
        `<div class="md-code" data-code-block>` +
        `<div class="md-code__head">` +
        `<span class="md-code__lang">${escapeHtml(label)}</span>` +
        `<button type="button" class="md-code__copy" data-code-copy>复制</button>` +
        `</div>` +
        `<pre><code class="hljs${langClass}">${codeHtml}</code></pre>` +
        `</div>`
      );
    },
  },
});

/** Minimal HTML escaper for the renderer's own interpolations (lang
 *  label; un-highlighted fallback code). Marked's internal `escape` is
 *  not part of the public API. */
function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

// --- DOMPurify configuration -------------------------------------------
// The defaults already strip <script>, on* handlers, and javascript:
// URLs, so the XSS fixtures in markdown.test.ts pass without extra
// config. We allow `target` and `rel` so future renderer hooks (or
// hand-authored links inside chat messages) can opt into opening in a
// new tab without re-running the sanitizer against an allow-list.
// `RETURN_TRUSTED_TYPE: false` is the default but we pin it so the
// TypeScript overload that returns `TrustedHTML` doesn't get
// selected, which would force a cast at every call site.
const PURIFY_CONFIG: DOMPurifyConfig = {
  USE_PROFILES: { html: true },
  ADD_ATTR: ["target", "rel"],
  RETURN_TRUSTED_TYPE: false,
};

// --- B1 (2026-08-16) R7: img src two-state allow-list --------------------
// LLM output can carry `![](url)` images. DOMPurify's default html
// profile KEEPS <img>, so an external image URL would fire a network
// request the moment the bubble renders — closing the pre-existing
// leak channel (BACKLOG §3.3 "不渲染 LLM 之外的图" was aspirational
// until this gate). Any <img> whose src is NOT one of our own
// attachment-route forms is rewritten BEFORE the DOMPurify pass:
//
//   ① relative `/api/v1/attachments/…` (browser-local PROD — the
//      SPA is same-origin with the daemon) passes through untouched;
//   ② absolute `${daemonBase()}/api/v1/attachments/…` (DEV cross
//      origin, incl. the pwa-remote proxy form) passes through;
//   ③ LOCAL image path form (09-13) → preview link (`data-image-path`,
//      click opens the in-app viewer; see the linkify block below);
//   ④ everything else (external http/data URLs) → plain new-tab
//      opener link `[图片]`.
//
// Implementation note: replacing the node inside a DOMPurify
// `uponSanitizeAttribute` hook is awkward (hooks see attribute
// strings, not owning nodes). The regex pass below runs on
// marked's OUTPUT instead — marked always emits quoted attribute
// values, so matching `<img … src="…" …>` is reliable — and
// DOMPurify still sanitizes everything afterward (the replacement
// <a>'s href survives because href is on the default allow-list;
// target/rel via ADD_ATTR above; data-image-path via the default
// ALLOW_DATA_ATTR).
// --- 09-13 本地路径预览(图片+文件):linkify 本地路径 ------------------------
// LLM 输出的本地路径(ui-review 截图、`out/报告.md`、`src/main.rs` 等)
// 在正文/inline code 里是纯文本,无法查看。这里把三种形态统一转成可点击
// 的 `<a class="md-image-path" data-image-path="原始路径">`(图片,点击经
// useCodeBlockCopy 的委托开 ImageViewerModal 弹层,<img> 直连 daemon 的
// `GET /api/v1/files/image`)或 `<a class="md-file-path" data-file-path=
// "原始路径">`(其余文件,点击开 FileViewerModal;pdf 由弹层 composable
// 分派到新标签,不走弹层):
//
//   ① 裸文本路径(正文段落里 `out/ui-review/x/1.png`);
//   ② inline `<code>` 内路径(LLM 习惯把路径写进反引号)——code 内保留
//      mono 字体,文本替换为同文本的 <a>;**围栏代码块(pre 祖先)不动**
//      (延续 wrapAtFileTokensOutsideCode 的"代码上下文不改写"理念,
//      只是这里按用户决策放宽到 inline code 也识别);
//   ③ markdown 图片语法 `![](本地路径)` 与链接语法 `[x](out/x.png)`:
//      前者经下方 downgradeExternalImages 的本地分支,后者在此处给
//      已有 <a href> 补 data 属性。
//
// 实现为 marked 输出之后的 **DOM 后处理**(DOMParser + TreeWalker):
// 字符串级后处理分不清"是否在 code/pre 内"(marked 已把代码内容转义成
// 文本),marked extension 则优先于 codespan tokenizer、会把 inline code
// 内文本先吞掉;DOM walk 能精确按祖先元素跳过。安全性:插入的 <a> 全部
// 经 DOM API 构造(textContent/setAttribute 自动转义),随后仍过
// DOMPurify(data-* 默认放行,ALLOW_DATA_ATTR)。
//
// 路径正则与 chatInputTokens.ts 的 FILE_RE 同风格(边界捕获组 + Unicode
// 段字符)。纯文件名(x.png,无路径分隔符)有意不识别 —— 英文句子里
// 误伤率高;`https://host/x.png` 由前边界(不含 `/`)自然排除。
const IMAGE_EXT = String.raw`png|jpe?g|gif|webp|bmp|avif|ico`;
// 09-13 同日文件通道:文本类 + pdf。与 daemon `/files/raw` 白名单
// (commands/files.rs `RAW_TEXT_EXTS`)有意各持一份 —— 后端是唯一安全
// 闸门,前端集偏大只会点开见 400;两份名单的对齐约定记录在
// .trellis/spec/frontend/chat/message-list-and-markdown.md §5,改动任一
// 侧须对照另一侧。
const TEXT_EXT = String.raw`md|markdown|txt|log|json|jsonl|csv|tsv|yaml|yml|toml|ini|conf|cfg|xml|html|htm|css|js|mjs|cjs|jsx|ts|tsx|vue|svelte|py|rs|go|java|kt|kts|c|h|cpp|hpp|cc|cs|rb|php|sh|bash|zsh|fish|sql|proto|graphql|gql|diff|patch`;
const PDF_EXT = String.raw`pdf`;
/** 前端识别全集 = 图片 ∪ 文本 ∪ pdf(单正则统一识别,命中后按扩展分流)。 */
const FILE_EXT = String.raw`${IMAGE_EXT}|${TEXT_EXT}|${PDF_EXT}`;
/** 段字符:Unicode 字母/数字 + `.` `_` `-`(dotfile、kebab 文件名)。 */
const IMAGE_PATH_SEG = String.raw`[\p{L}\p{N}._-]`;
/** 路径本体(无边界组):带前缀(`/`、`~/`、`./`、`../`)任意段数;
 *  无前缀(裸相对)必须至少含一个 `/` 段,否则就是纯文件名。扩展集
 *  参数化(图片通道与文件通道共用同一套段/边界语法)。 */
function pathBodyFor(extSet: string): string {
  return String.raw`(?:(?:/|~/|\.{1,2}/)${IMAGE_PATH_SEG}+(?:/${IMAGE_PATH_SEG}+)*|${IMAGE_PATH_SEG}+/${IMAGE_PATH_SEG}+(?:/${IMAGE_PATH_SEG}+)*)\.(?:${extSet})`;
}
/** 图片本体(既有通道;spec §5 引用名保留)。 */
export const IMAGE_PATH_BODY = pathBodyFor(IMAGE_EXT);
/** 文件本体(全集;`linkifyPlainText` 的说明见函数注释)。 */
export const FILE_PATH_BODY = pathBodyFor(FILE_EXT);
/** 文本内的图片路径(带前后边界组;`m[2]` 是路径本体)。 */
export const IMAGE_PATH_RE = new RegExp(
  `(^|[\\s(\\[{"'<（【「『“：，])(${IMAGE_PATH_BODY})(?=$|[\\s.,;:!?)\\]}>"'’」』】”。！？；…])`,
  "giu",
);
/** 文本内的本地路径(图片+文件全集;边界组逻辑与 IMAGE_PATH_RE 全同)。 */
export const FILE_PATH_RE = new RegExp(
  `(^|[\\s(\\[{"'<（【「『“：，])(${FILE_PATH_BODY})(?=$|[\\s.,;:!?)\\]}>"'’」』】”。！？；…])`,
  "giu",
);
/** 属性值形态(href/src 整体就是一个路径,无需边界组)。 */
const LOCAL_FILE_PATH_RE = new RegExp(`^(?:${FILE_PATH_BODY})$`, "iu");
/** 命中路径按扩展分流:尾巴是图片扩展 → 既有图片通道,否则文件通道。
 *  仅断尾部(非 global,无 lastIndex 状态),配合 match 结束于扩展名的
 *  正则形态(`x.png.bak` 命中 `x.png` 尾 → 归图片通道,与识别一致)。 */
const IMAGE_TAIL_RE = new RegExp(`\\.(?:${IMAGE_EXT})$`, "i");
/** 便宜预检:文本里出现任一识别扩展名字样才进 DOMParser(多数消息
 *  不含,别为它们付 parse + walk 的钱)。宽松无妨,误报只是多跑一次
 *  walk。 */
const FILE_PATH_HINT = new RegExp(`\\.(?:${FILE_EXT})`, "i");

/** src/href 是否是"本地路径"形态(图片+文件全集)。排除 http(s)/data/
 *  mailto/锚点,以及我们自己的 API 路径(`/api/v1/attachments/...` 的
 *  uuid 文件名以 .png 结尾,会被裸形态误吞)。 */
function isLocalFilePath(src: string): boolean {
  if (/^(?:https?:|data:|mailto:|#|\/api\/)/i.test(src)) return false;
  return LOCAL_FILE_PATH_RE.test(src);
}

/** marked 会把链接目标 percent-encode(空格 → %20);daemon 读的是解码
 *  后的文件路径,尽力解码,畸形序列(裸 `%`)原样返回。 */
function tryDecodeUri(s: string): string {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}

const IMG_TAG_RE = /<img\b[^>]*>/gi;
const SRC_ATTR_RE = /\bsrc\s*=\s*(?:"([^"]*)"|'([^']*)')/i;

function isOwnAttachmentSrc(src: string): boolean {
  if (src.startsWith("/api/v1/attachments/")) return true;
  const base = daemonBase().replace(/\/+$/, "");
  return (
    src.startsWith(`${base}/api/v1/attachments/`) ||
    src.startsWith(`${base}/api/v1/proxy/api/v1/attachments/`)
  );
}

/** Replace non-allow-listed `<img>` tags. Allow-listed tags (our
 *  attachments route) pass through untouched; LOCAL paths become a
 *  preview link — 图片扩展 → `[图片]`(data-image-path,现状),其余
 *  文件扩展 → `[文件]`(data-file-path,顺带修掉 `![](out/x.md)` 退化
 *  成相对 href 新标签链接打穿 SPA 路由的存量 wart); everything else
 *  degrades to a new-tab opener link as before. */
function downgradeExternalImages(html: string): string {
  return html.replace(IMG_TAG_RE, (tag) => {
    const m = SRC_ATTR_RE.exec(tag);
    const src = m ? m[1] ?? m[2] ?? "" : "";
    if (!src || isOwnAttachmentSrc(src)) return tag;
    if (isLocalFilePath(src)) {
      const p = tryDecodeUri(src);
      if (IMAGE_TAIL_RE.test(p)) {
        return `<a class="md-image-path" data-image-path="${escapeHtml(p)}">[图片]</a>`;
      }
      return `<a class="md-file-path" data-file-path="${escapeHtml(p)}">[文件]</a>`;
    }
    return `<a href="${src}" target="_blank" rel="noreferrer">[图片]</a>`;
  });
}

/** DOM 后处理:见上方 linkify 块注释。pre(围栏)与 a(防嵌套)内
 *  的文本节点跳过;inline code 内的文本节点照常处理。命中按扩展分流:
 *  图片 → md-image-path/data-image-path(现状不变),其余 →
 *  md-file-path/data-file-path。 */
function linkifyLocalPaths(html: string): string {
  if (!FILE_PATH_HINT.test(html)) return html;
  const doc = new DOMParser().parseFromString(html, "text/html");
  // ③ markdown 链接语法:给本地路径形态的 <a href> 补 data 属性
  // (点击委托按 data-* 拦截,preventDefault 后不走 href 导航 ——
  // 相对路径 href 会打穿 SPA 路由)。
  for (const a of Array.from(
    doc.body.querySelectorAll<HTMLAnchorElement>("a[href]"),
  )) {
    const href = a.getAttribute("href") ?? "";
    if (isLocalFilePath(href)) {
      const p = tryDecodeUri(href);
      if (IMAGE_TAIL_RE.test(p)) {
        a.classList.add("md-image-path");
        a.dataset.imagePath = p;
      } else {
        a.classList.add("md-file-path");
        a.dataset.filePath = p;
      }
    }
  }
  // ①② 文本节点替换。
  const walker = doc.createTreeWalker(doc.body, NodeFilter.SHOW_TEXT);
  const targets: Text[] = [];
  while (walker.nextNode()) {
    const t = walker.currentNode as Text;
    if (!FILE_PATH_HINT.test(t.data)) continue;
    let el: Element | null = t.parentElement;
    let skip = false;
    while (el && el !== doc.body) {
      if (el.tagName === "PRE" || el.tagName === "A") {
        skip = true;
        break;
      }
      el = el.parentElement;
    }
    if (!skip) targets.push(t);
  }
  for (const node of targets) {
    const text = node.data;
    const owner = node.ownerDocument;
    if (!owner) continue;
    const frag = owner.createDocumentFragment();
    let last = 0;
    for (const m of text.matchAll(FILE_PATH_RE)) {
      const pathStart = (m.index ?? 0) + (m[1]?.length ?? 0);
      const path = m[2];
      if (pathStart < last) continue; // 防御:边界组理论不重叠,兜底
      frag.appendChild(owner.createTextNode(text.slice(last, pathStart)));
      const a = owner.createElement("a");
      if (IMAGE_TAIL_RE.test(path)) {
        a.className = "md-image-path";
        a.dataset.imagePath = path;
      } else {
        a.className = "md-file-path";
        a.dataset.filePath = path;
      }
      a.textContent = path;
      frag.appendChild(a);
      last = pathStart + path.length;
    }
    if (last === 0) continue;
    frag.appendChild(owner.createTextNode(text.slice(last)));
    node.replaceWith(frag);
  }
  return doc.body.innerHTML;
}

/** 非 markdown 纯文本的 linkify(工具输出 `<pre>` 面用,2026-09-13):
 *  整体 escapeHtml → FILE_PATH_RE 全局替换插锚(锚文本与 data 属性值
 *  都用**已转义**切片 —— 引号已成 &quot;,属性边界天然安全)→
 *  DOMPurify.sanitize(双保险,维持"所有 v-html 都过 DOMPurify"的仓库
 *  不变量)。无命中时纯转义文本也照走 sanitize,约定单一好审计。
 *  注意工具输出先经 truncateOutput 截断:被切断的路径缺扩展名尾,
 *  正则不匹配,不会产生半截链接。 */
export function linkifyPlainText(text: string): string {
  const escaped = escapeHtml(text ?? "");
  const html = escaped.replace(
    FILE_PATH_RE,
    (match: string, lead: string, path: string): string => {
      // replacer 函数形态:(match, 边界组, 路径组);返回值不做 $ 模板
      // 解释,路径/边界字符零歧义。
      void match;
      const attr = IMAGE_TAIL_RE.test(path) ? "data-image-path" : "data-file-path";
      const cls = attr === "data-image-path" ? "md-image-path" : "md-file-path";
      return `${lead}<a class="${cls}" ${attr}="${path}">${path}</a>`;
    },
  );
  return DOMPurify.sanitize(html, PURIFY_CONFIG);
}

/**
 * Render a markdown string to a sanitized HTML string.
 *
 * Returns `""` for empty or whitespace-only input so the bubble
 * doesn't render an empty `<p></p>` artifact. Always trims leading
 * whitespace before parsing — this is the *only* trim call in the
 * rendering pipeline; callers should pass raw LLM text.
 */
export function renderMarkdown(text: string): string {
  if (!text || !text.trim()) return "";
  const trimmed = text.replace(/^\s+/, "");
  // Cast to string: `marked.parse` is overloaded to return either
  // `string` or `Promise<string>` depending on options. We never set
  // `async: true` (singleton `marked` is sync), so the runtime value
  // is always a string. The cast keeps TypeScript from widening to
  // `string | Promise<string>` and forcing downstream casts.
  const rawHtml = marked.parse(trimmed) as string;
  return DOMPurify.sanitize(
    linkifyLocalPaths(downgradeExternalImages(rawHtml)),
    PURIFY_CONFIG,
  );
}

export interface DebouncedRenderer {
  /** Reactive ref of the latest sanitized HTML. Bind with `v-html`. */
  rendered: Ref<string>;
  /** Schedule a render. Bursts of calls within `debounceMs` collapse
   *  into one parse + sanitize pass. */
  schedule: (text: string) => void;
  /** Render the most recent scheduled text immediately, cancelling
   *  any pending debounce timer. Call on stream end so the final
   *  frame doesn't wait out the timer. */
  flush: () => void;
  /** Cancel any pending timer and drop retained text. Wire this to
   *  `onUnmounted` to avoid leaking the closure across rapid
   *  message-list churn. */
  dispose: () => void;
}

/**
 * A reactive debounced markdown renderer.
 *
 * Why a factory (and not a plain computed):
 *   The 50ms debounce needs to live across the streaming lifecycle,
 *   including a final flush on stream end. A `computed` would re-run
 *   on every change synchronously; a `schedule` with setTimeout lets
 *   us coalesce bursts of `displayContent` updates and also expose a
 *   `flush()` for the terminal frame.
 *
 * The returned `rendered` is a `Ref<string>` — wire it directly into
 * the template with `v-html="rendered"` (script setup auto-unwraps
 * refs, so `v-html="rendered"` works in `<template>`).
 *
 * Memory: call `dispose()` from `onUnmounted` to clear any pending
 * timer. Without it, a message unmounted mid-debounce would leak the
 * closure (and indirectly the old `text` string) until the timer
 * fired.
 */
export function createDebouncedRenderer(
  debounceMs = 50,
): DebouncedRenderer {
  const rendered = ref<string>("");
  let pendingText: string | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let lastScheduled: string | null = null;

  const apply = (text: string) => {
    rendered.value = renderMarkdown(text);
    pendingText = null;
  };

  const schedule = (text: string) => {
    pendingText = text;
    // Cheap no-op fast path: identical to the last scheduled text
    // (e.g. watcher firing on a reactive ref that didn't change in
    // value). Avoids re-running marked + DOMPurify on noise.
    if (text === lastScheduled) return;
    lastScheduled = text;
    if (timer !== null) clearTimeout(timer);
    timer = setTimeout(() => {
      timer = null;
      if (pendingText !== null) apply(pendingText);
    }, debounceMs);
  };

  const flush = () => {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
    if (pendingText !== null) apply(pendingText);
  };

  const dispose = () => {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
    pendingText = null;
    lastScheduled = null;
  };

  return { rendered, schedule, flush, dispose };
}
