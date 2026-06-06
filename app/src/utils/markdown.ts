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

import { marked } from "marked";
import DOMPurify, { type Config as DOMPurifyConfig } from "dompurify";
import { ref, type Ref } from "vue";

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
  return DOMPurify.sanitize(rawHtml, PURIFY_CONFIG);
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
