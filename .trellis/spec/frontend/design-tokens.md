# Frontend Design Tokens

> CSS variable system for the frontend visual language.
> Captures the color, spacing, radius, and font tokens that
> every component should reference — never hardcode hex
> values or px magic numbers in component CSS.

---

## Status

Filled (2026-06-09). Token definitions live in
`app/src/style.css` (single global stylesheet imported by
`main.ts`). Components reference tokens via `var(--name)`.

**Theme override layer (2026-09-05, aggressive)**: `style.css`'s `@theme`
block is the **classic** default. A second stylesheet
`app/src/theme-aggressive.css` overrides the same `var(--color-*)` set
under `:root[data-theme="aggressive"]` (sharp corners 2-4px, visible
grid with brightened borders, single volt lime accent `#a3e635`-family;
see its header comment for the design rulings). `composables/useTheme.ts`
owns switching (`ThemeName = "classic" | "aggressive"`, **default
aggressive** during the experiment; classic = attribute **deleted** so the
classic path renders byte-identical to history); entry point = Sidebar
footer toggle; persistence = localStorage (`everlasting.theme`), **not**
backend `app_config`. Component CSS must keep consuming `var(--color-*)`
only — the theme swap is pure token-layer, components are untouched.
Tokens tables below list the **classic** values; when auditing aggressive
rendering, read `theme-aggressive.css` for the override values (this spec
predates the layer and its color/radius tables are classic-scoped).

---

## Token Family Index(2026-09-19 doc-split)

> **分篇**:本文保留 Status、两条 Don't 核心规则与 Related;各 token 族参考表与 4 条编年 Decision 已按 tool-contract 模式拆至 `design-tokens/` 子目录(一族一文件,原锚点以本索引替代)。

- [color-tokens.md](./design-tokens/color-tokens.md) — Color Tokens + State Tints
- [decisions.md](./design-tokens/decisions.md) — 编年 Decision:`--color-text-muted` 提亮 `#7c8aa0`(06-09)/ secondary·muted 上提一档 slate(08-05)/ 填充 500·文字 400 拆分 + 彩底文字规则(08-15)/ design system token expansion(06-27 PR-1)
- [typography-tokens.md](./design-tokens/typography-tokens.md) — Typography Tokens
- [spacing-radius-border-tokens.md](./design-tokens/spacing-radius-border-tokens.md) — Spacing + Radius + Border Tokens
- [motion-vocabulary.md](./design-tokens/motion-vocabulary.md) — Motion Vocabulary
- [shadow-zindex-tokens.md](./design-tokens/shadow-zindex-tokens.md) — Shadow Scale + Z-Index Ladder
- [button-icon-modal-tokens.md](./design-tokens/button-icon-modal-tokens.md) — Button Family + Icon Sizing + Modal Tokens

---

## Don't: Hardcode color / spacing / radius / motion / shadow / type values in component CSS

Component CSS MUST reference the tokens. Hardcoded
`#131822`, `8px`, `150ms`, `0 4px 12px rgba(0,0,0,0.4)`, or
`font-size: 14px` in a component file will silently drift
if the token is ever updated.

**Wrong**:

```css
.my-card {
  background: #131822; /* ❌ hardcoded, will not follow --color-bg-surface updates */
}
```

**Correct**:

```css
.my-card {
  background: var(--color-bg-surface); /* ✅ tracks the token */
}
```

**Exception**: the `app/src/style.css` file itself, where
the token values are defined.

---

## Don't: Add a new `--color-*` token for a one-off use

The token system is intentionally small. Before adding a
new color token, ask:

1. Will this color appear in 3+ unrelated components?
2. Is it a "primary" use case (action, surface, text) or
   a one-off accent?

If the answer to (1) is "no" or (2) is "one-off", put the
hex value in the component CSS with a comment explaining
why it can't be a token.

---

## Related

- `app/src/style.css` — token definitions.
- `.trellis/spec/frontend/cjk-fonts.md` — `--font-sans`
  bundling (where HarmonyOS Sans SC comes from).
- `.trellis/spec/frontend/popover-pattern.md` — popover
  styling conventions (use these tokens).
- `.trellis/spec/frontend/reka-ui-usage.md` — reka-ui
  primitives are themed against these tokens.
