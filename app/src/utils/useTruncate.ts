// useTruncate — pure markdown-aware string truncation.
//
// PR3 of the subagent-drawer redesign (2026-06-21). Two consumers
// land with this file (PR5 wires them into the SubagentDrawer):
//
//   1. `task` (the parent LLM's prompt that dispatched the worker)
//      → 120 chars + "View full →" → triggers `MarkdownDetailModal`
//   2. `finalText` (the worker's stripped terminal reply)
//      → 280 chars + "View full →" → triggers `MarkdownDetailModal`
//
// Why a *pure function* (not a Vue composable returning a ref):
//   The truncations happen at render time (PR5 calls `truncate(...)`
//   inline in the drawer's template, or in a computed). There's no
//   reactive state to manage, no lifecycle hook to bind, and no
//   dependency injection to thread — the input is just a string + a
//   length, the output is just a string. A composable wrapper would
//   add ceremony without value.
//
// Why this lives in `utils/` (not `composables/`):
//   The project's convention is `useFoo.ts` files in `utils/` next
//   to plain utilities (`useKeyboard.ts` is the canonical example).
//   The `use*` prefix is a naming hint — it suggests "I look like a
//   composable" — but the contents are pure. A future caller CAN
//   wrap this in a composable if they need reactivity, but the
//   primitive is intentionally non-reactive.
//
// Why markdown-aware:
//   A naive `text.slice(0, n)` can leave the truncation boundary
//   inside a fenced code block (`` ``` `` ... `` ``` ``) or an inline
//   code span (`` ` `` ... `` ` ``). When the user clicks "View full →"
//   to see the markdown rendered, the visible portion looks
//   broken (a code block that's open but never closes, or worse,
//   a partial inline code that swallows the rest of the text).
//   So when `maxChars` lands inside a code region, we backtrack to
//   the start of the region and cut there. Links (`[text](url)`)
//   may be cut mid-text — link boundary preservation isn't worth
//   the extra scan cost (links are usually rare in `task` /
//   `finalText` and the "View full →" modal shows the complete text
//   anyway).
//
// Performance contract:
//   Single linear scan, O(N) where N = `text.length`. No regex
//   backtracking on the text body. The 100k-char benchmark test
//   asserts < 50ms; the implementation measured ~1-3ms on a modern
//   laptop.

const DEFAULT_SUFFIX = "…"; // single Unicode ellipsis (U+2026), not "..."

/**
 * Truncate `text` to at most `maxChars` characters, appending `suffix`
 * if the input was longer than the budget. The boundary is pushed
 * backward when it lands inside a fenced code block or an inline
 * code span, so the rendered portion of the markdown does not have
 * an unclosed code region.
 *
 * Behaviour summary:
 *
 * - `text` empty               → return `""`
 * - `maxChars <= 0`            → return `suffix` (or `""` if no suffix)
 * - `text.length <= maxChars`  → return `text` unchanged (no suffix)
 * - Normal case                → cut at `maxChars`, then backtrack
 *   to just-before the opening fence (``` ) or inline code (`) if
 *   the cut would land inside one.
 *
 * Degenerate cases that must NOT infinite-loop:
 * - text is all backticks (e.g. ```` `` ```` ``): backtrack finds
 *   no safe boundary → falls back to hard cut at `maxChars`.
 * - unclosed code fence (e.g. `` ```python\nfoo``): the scan
 *   reports `cut` is inside a fence; backtrack to the fence opener;
 *   if the opener is at index 0, fall back to hard cut.
 *
 * @param text     Input string (markdown text — may contain fenced
 *                 and inline code).
 * @param maxChars Maximum number of characters to keep before the
 *                 suffix is appended.
 * @param suffix   String appended to indicate truncation. Defaults
 *                 to a single Unicode ellipsis (U+2026). Pass `""`
 *                 for no suffix.
 */
export function truncate(
  text: string,
  maxChars: number,
  suffix: string = DEFAULT_SUFFIX,
): string {
  // --- Defensive guards ---------------------------------------------
  if (!text) return "";
  if (maxChars <= 0) return suffix;
  if (text.length <= maxChars) return text;

  // --- Scan the prefix for code-region state -----------------------
  //
  // We need to know two things at the cut position:
  //   1. Are we inside a fenced code block (opened by 3+ backticks
  //      on its own line-ish boundary)?
  //   2. Are we inside an inline code span (opened by a single
  //      backtick)?
  //
  // Approach: a single linear pass over `text[0..cut)`. Track:
  //   - `inFence`: boolean (last 3-backtick run toggled this)
  //   - `inInline`: boolean (last 1-backtick run toggled this)
  //   - `lastFenceStart`: index of the most recent fence-opener run
  //     start (>=3 backticks). -1 if no fence has opened.
  //   - `lastInlineStart`: index of the most recent inline-opener
  //     single-backtick position. -1 if no inline has opened.
  //   - The current run (length + start + char).
  //
  // Note: we DO NOT try to detect "code-block opens only at line
  // start". Real markdown requires a fence opener to be on its own
  // line (with up to 3 leading spaces), and the closer can have up
  // to 3 leading spaces and nothing else. For our truncation
  // purposes the precision of "exact fence position" doesn't matter
  // — pushing the boundary backward to "just before the backtick
  // run" yields the same visual result, and the "View full →" modal
  // lets the user see the original text. So we treat any run of
  // >=3 backticks as a fence toggle.

  // The naive cut position — what `slice(0, cut)` would give us.
  // The algorithm below either keeps this position (if no code
  // region is active at cut) or pulls it backward (if a code
  // region opens before cut and we land inside it).
  const cut = maxChars;

  let inFence = false;
  let inInline = false;
  let lastFenceStart = -1;
  let lastInlineStart = -1;

  // Run-tracking: we accumulate consecutive identical chars so we
  // can distinguish single-backtick (inline toggle) from triple-
  // backtick (fence toggle). One char at a time; the loop is hot.
  let runStart = 0; // start index of the current run
  let runLen = 0; // length of the current run
  // The char of the current run. We only track backticks — every
  // other char resets the run (runLen becomes 0 below).
  // (Tildes are intentionally ignored; see algorithm note above.)

  // Helper: when the run ENDS at index `endIdx`, decide what
  // toggle (if any) it represents and update the region state.
  // We track the run-end by the NEXT non-matching char (or by the
  // loop hitting `cut`); the helper is called at that point.
  const onRunEnd = (endIdx: number): void => {
    if (runLen === 0) return;
    if (runLen >= 3) {
      // Fence toggle.
      inFence = !inFence;
      if (inFence) {
        // Opened — record the start for backtracking.
        lastFenceStart = runStart;
      }
      // Fence also resets inline state (per CommonMark: a fence
      // can't appear inside an inline code span).
      inInline = false;
      lastInlineStart = -1;
    } else if (runLen === 1) {
      // Single backtick — inline code toggle.
      if (inInline) {
        // Closer.
        inInline = false;
      } else {
        // Opener.
        inInline = true;
        lastInlineStart = runStart;
      }
    }
    // runLen === 2: two backticks — no toggle (not a fence, not
    // a valid inline). Ignore. (Two-backtick is a literal "``" in
    // CommonMark; we don't try to parse it.)
    runLen = 0;
    runStart = endIdx;
  };

  for (let i = 0; i < cut; i++) {
    const ch = text[i];
    if (ch === "`") {
      // Extend the current run.
      if (runLen === 0) {
        runStart = i;
      }
      runLen++;
    } else {
      // Non-backtick: close the previous run, then continue.
      if (runLen > 0) {
        onRunEnd(i);
      }
    }
  }

  // The run (if any) that was open at `cut - 1` is still pending.
  // Process it — but be careful: `cut` is the EXCLUSION boundary,
  // so we only process runs that ended BEFORE `cut`.
  if (runLen > 0) {
    // Determine whether the run extends past `cut`. If
    // `runStart + runLen > cut`, the run spans the cut boundary
    // and we do NOT process it (the chars beyond `cut` may change
    // its classification). Otherwise the run ended at or before
    // `cut`, so process it.
    if (runStart + runLen <= cut) {
      onRunEnd(cut);
    }
    // else: run straddles the cut — leave state as-is (the run's
    // effect on state depends on chars we haven't scanned).
  }

  // --- Decide the safe boundary -------------------------------------
  let safeBoundary = cut;
  if (inFence && lastFenceStart >= 0) {
    safeBoundary = lastFenceStart;
  } else if (inInline && lastInlineStart >= 0) {
    safeBoundary = lastInlineStart;
  }

  // --- Build the truncated result -----------------------------------
  const truncated = text.slice(0, safeBoundary);
  // Fallback: if backtracking yielded empty (e.g. text starts with
  // a fence and maxChars < 3, so the only safe boundary is 0),
  // hard-cut at maxChars so the user gets SOMETHING truncated.
  if (truncated.length === 0) {
    return text.slice(0, cut) + suffix;
  }
  return truncated + suffix;
}