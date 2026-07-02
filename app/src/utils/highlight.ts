// highlight.ts — shared syntax-highlighting helper for B9 code_block
// primitive + the markdown pipeline (Child B of 07-02-b9-generative-ui,
// 2026-07-02; parent D6 = hljs).
//
// Two call sites share this ONE helper so language support can never
// diverge between them:
//   1. `utils/markdown.ts` via `marked-highlight` (every ```lang fenced
//      block in assistant prose gets highlighted)
//   2. `<CodeBlockPrimitive>` directly (the use_ui `code_block` primitive
//      is a structured card, NOT markdown text, so it calls this itself)
//
// Language set = `highlight.js/lib/common` (~30 mainstream languages:
// js/ts/py/rust/go/java/c/c++/json/bash/md/...). NOT the full bundle
// (~900KB) — the project is a personal tool and common covers the
// realistic tail. Promote to full only if a real gap appears.

import hljs from "highlight.js/lib/common";

/**
 * Render `code` to hljs-highlighted HTML.
 *
 * - Known `language` (hljs recognizes it) → `hljs.highlight(code, {language})`
 * - Unknown / missing `language` → `hljs.highlightAuto` (best-effort
 *   detection; never throws on weird input)
 *
 * Always returns a string (hljs escapes the input, so the result is safe
 * to bind via `v-html` after the usual DOMPurify pass for the markdown
 * path; the CodeBlockPrimitive path trusts hljs's escaping directly since
 * the input is a structured primitive, not arbitrary prose).
 */
export function renderCodeHtml(code: string, language: string): string {
  const lang = language?.trim().toLowerCase();
  if (lang && hljs.getLanguage(lang)) {
    try {
      return hljs.highlight(code, { language: lang }).value;
    } catch {
      // fall through to auto-detect (rare: hljs throws on some inputs)
    }
  }
  return hljs.highlightAuto(code).value;
}
