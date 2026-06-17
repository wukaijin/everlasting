// chatInputTokens.ts — PR1.5 PR-B: token highlighting for the ChatInput editor.
//
// Adds visual coloring to three token kinds so the user can tell at a
// glance what's a reference vs. plain text:
//   - `/command`  → accent blue (matches the B3 `/command` palette family
//                   and the project's primary-action color).
//   - `@file`     → read-tool cyan (matches the read_file tool family
//                   per `.trellis/spec/frontend/design-tokens.md`).
//   - `skill`     → violet (matches extended-thinking family). The match
//                   rule is RESERVED for B4 — this file ships the CSS
//                   class hook (`cm-token-skill`) and a slot in
//                   `TOKEN_KINDS` so B4 only needs to add a regex, not
//                   restructure the plugin.
//
// Implementation notes (research/codemirror-token-highlight-migration.md §1):
//   - `Decoration.mark` only — no `Decoration.widget` / `Decoration.replace`,
//     because widget decorations have known IME interaction bugs (CM forum
//     /t/9729, /t/9799). `mark` adds a CSS class to an inline `<span>` over
//     the matched range and is IME-safe.
//   - `RangeSetBuilder` requires strictly-sorted, non-overlapping ranges
//     or `.finish()` throws. The three token regexes are mutually
//     exclusive per character position (`/` `@` and a future skill marker
//     can't start on the same offset), so no overlap.
//   - `update()` only rebuilds on `docChanged` / `viewportChanged` —
//     cursor moves don't change coloring, so we skip the rebuild to
//     avoid wasted work.
//   - Pure decoration: NO dispatch, NO doc mutation, NO selection
//     change. The plugin is a read-only projection of the doc →
//     DecorationSet. This is the critical safety property: highlighting
//     must never interfere with input / IME / caret position.

import {
  Decoration,
  type DecorationSet,
  ViewPlugin,
  type EditorView,
  type ViewUpdate,
} from "@codemirror/view";
import { RangeSetBuilder } from "@codemirror/state";

/** Token kind descriptor. Adding a new kind (B4 skill) is a single
 *  entry here + a `.cm-token-<class>` CSS rule in ChatInput.vue. */
interface TokenKind {
  /** CSS class applied via `Decoration.mark({ class })`. The matching
   *  color rule lives in `ChatInput.vue`'s `<style scoped>` block as
   *  `:deep(.cm-token-<class>)`. */
  className: string;
  /** Match function: given the full doc text, return all token spans
   *  as `{ from, to }` pairs (absolute doc offsets). Returning an
   *  empty array is fine. */
  match: (doc: string) => Array<{ from: number; to: number }>;
}

/**
 * Command token: `/` followed by a letter (avoids matching URL paths
 * like `http://` and bare division slashes) + command-name chars
 * `[A-Za-z0-9_-]`. Must be preceded by whitespace OR start of doc/line
 * so we don't pick up `/` inside a word (e.g. `path/to` stays uncolored,
 * but `/help` anywhere on its own is highlighted as a command hint).
 *
 * Why "preceded by whitespace/line-start": Claude Code and the existing
 * `detectCommandTrigger` treat `/` at line start as the palette trigger;
 * coloring is more permissive (any whitespace-boundary `/word`) for a
 * gentle visual hint without opening the panel. The leading-boundary
 * rule also kills the `//` in `http://` because the first `/` follows
 * a `:`.
 */
const COMMAND_RE = /(^|[\s])(\/[A-Za-z][\w-]*)/g;

/**
 * File token: `@` followed by a path-shaped run `[\w/.-]+`. The first
 * char after `@` MUST be a word char OR a `.` (i.e. `[.\w]`) so we
 * cover hidden / dotfiles like `.gitignore`, `.env`, `.babelrc`,
 * `.eslintrc`. The first char still MUST NOT be another `@` (the
 * `[.\w]` class excludes `@`), which guards against email-style runs
 * like `user@@host`. Must be preceded by whitespace or line start so
 * `name@host.com` emails stay uncolored — the `(^|[\s])` boundary
 * kills the `name@host` shape because `@` follows a word char, not
 * whitespace.
 */
const FILE_RE = /(^|[\s])(@[.\w][\w/.-]*)/g;

/** Build a match function from a global regex with a leading capture
 *  group for the boundary char (so we can offset `from` past it). */
function matchFrom(regex: RegExp): (doc: string) => Array<{ from: number; to: number }> {
  return (doc: string) => {
    const out: Array<{ from: number; to: number }> = [];
    // `regex` is a module-level constant with the `g` flag; reset state
    // before each scan (defensive — matchAll creates its own state but
    // we use the same regex instance so lastIndexOf-based reset is moot
    // here; `matchAll` doesn't mutate the regex's lastIndex).
    for (const m of doc.matchAll(regex)) {
      const prefix = m[1] ?? "";
      const token = m[2];
      if (!token) continue;
      const from = (m.index ?? 0) + prefix.length;
      const to = from + token.length;
      if (to > from) out.push({ from, to });
    }
    return out;
  };
}

/** Token kind table. Order doesn't matter — spans are sorted before
 *  being handed to `RangeSetBuilder`. */
const TOKEN_KINDS: TokenKind[] = [
  {
    className: "cm-token-command",
    match: matchFrom(COMMAND_RE),
  },
  {
    className: "cm-token-file",
    match: matchFrom(FILE_RE),
  },
  // B4 skill token — regex intentionally absent. When B4 lands, add a
  // `{ className: "cm-token-skill", match: matchFrom(SKILL_RE) }` entry
  // here and a `:deep(.cm-token-skill)` rule in ChatInput.vue. The CSS
  // rule is already pre-staged below in the ChatInput style block.
];

/** Build the DecorationSet for the current doc. Collects all token
 *  spans from every `TokenKind`, sorts by `from` (asc) then `to`
 *  (asc), and feeds them to `RangeSetBuilder`. `RangeSetBuilder.add`
 *  requires strict ascending order and non-overlapping ranges or
 *  `.finish()` throws — our regexes never produce overlapping spans
 *  (a single char can't start two different token kinds), but we
 *  sort anyway so the contract holds even if a future kind's regex
 *  yields out-of-order matches. */
function buildDecorations(view: EditorView): DecorationSet {
  const doc = view.state.doc.toString();
  type Span = { from: number; to: number; deco: Decoration };
  const spans: Span[] = [];
  for (const kind of TOKEN_KINDS) {
    const mark = Decoration.mark({ class: kind.className });
    for (const { from, to } of kind.match(doc)) {
      spans.push({ from, to, deco: mark });
    }
  }
  spans.sort((a, b) => a.from - b.from || a.to - b.to);
  const builder = new RangeSetBuilder<Decoration>();
  for (const s of spans) {
    // `RangeSetBuilder.add(from, to, value)` — `value` is the
    // `Decoration` itself (NOT `Decoration.range(...)`, which returns
    // a `Range<Decoration>` and is meant for the `Decoration.set([...])`
    // alternate API). The builder enforces ascending `from` order; our
    // sort above satisfies that contract.
    builder.add(s.from, s.to, s.deco);
  }
  return builder.finish();
}

/** The ViewPlugin. `ViewPlugin.fromClass` is the canonical shape; the
 *  class holds the `DecorationSet` and rebuilds it only when the doc
 *  or viewport changes (NOT on pure cursor moves — those don't affect
 *  coloring). The `decorations` field is exposed via the plugin spec's
 *  `decorations: (plugin) => plugin.decorations` so CM can read it
 *  every update without re-running the build. */
export const tokenHighlightPlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = buildDecorations(view);
    }
    update(u: ViewUpdate): void {
      if (u.docChanged || u.viewportChanged) {
        this.decorations = buildDecorations(u.view);
      }
    }
  },
  { decorations: (p) => p.decorations },
);
