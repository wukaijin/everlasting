<!-- Moved from design-tokens.md 2026-09-19 (doc-split) -->

---

## Decision: `--color-text-muted` bumped to `#7c8aa0` (2026-06-09)

**Context**: the original `--color-text-muted: #64748b`
(slate-500) was too dim for 11px mono text. Sidebar headers,
status bar, chat input hint, and form labels were all nearly
invisible against the dark surface.

**Decision**: bumped to `#7c8aa0` (slate-450, ~6% luminance
lift) as part of the UI polish PR.

**Rationale**:

- Lifts 11px mono gray text to readable weight without
  crossing into "primary text" territory.
- Maintains a 7% luminance gap from `--color-text-secondary: #8b95a7`
  so the two are still distinguishable.
- Matches the 11px mono text in AppShell toast, sidebar
  header (`SESSIONS`), and chat input hint (now more
  readable).

**When to revisit**: if the contrast between `--color-text-muted`
and `--color-text-secondary` ever feels too small (or too
large), bump both in step, keeping the relative gap.

---

## Decision: `--color-text-secondary` / `--color-text-muted` lifted one slate tier (2026-08-05)

**Context**: secondary text (`--color-text-secondary: #8b95a7`,
slate-400-equivalent) read as too dim across 40+ component
call sites (sidebar captions, model/mode select labels, tool
card metadata, message footer, chat input hint, audit log
rows, ...). User reported the overall text felt under-lit even
though `--color-text-primary` (#cbd5e1) is already at
slate-300. The fix is to lift the secondary + muted tier one
slate step (slate-400 → slate-350, slate-450 → slate-400)
without disturbing `--color-text-primary` — that anchor is
where the "primary vs secondary" hierarchy starts, and pulling
it up would compress the gap to the primary tier.

**Decision**:

- `--color-text-secondary`: `#8b95a7` → `#9aa3b8` (slate-350,
  +6% luminance)
- `--color-text-muted`: `#7c8aa0` → `#8a93a8` (slate-400, in
  step with secondary to preserve the documented 7% luminance
  gap)
- `--color-text-primary`: untouched (`#cbd5e1`, slate-300)
- `--color-text-on-accent`: untouched (`#ffffff`)

**Rationale**:

- The two tokens are lifted together so the relative
  secondary↔muted contrast stays at the previously-tuned
  ~7% luminance. Bumping only `--color-text-secondary` would
  invert the gap and make `--color-text-muted` look dimmer
  than it used to — visible regression for the 11px mono
  caption tier that the 2026-06-09 bump was supposed to
  rescue.
- Keeping `--color-text-primary` at `#cbd5e1` preserves the
  ~10% luminance gap between primary and secondary — the
  hierarchy still reads "primary headline, secondary
  metadata" at a glance. A more aggressive lift (e.g.
  `--color-text-secondary: #a8b0c0`) would narrow this to
  ~4% and start to flatten the visual hierarchy the design
  depends on.
- No component CSS needs to change — all 40+ call sites
  already reference `var(--color-text-secondary)`, so the
  lift cascades automatically. The `PluginSelect.vue` line
  that uses `--color-text-secondary` for a border + a
  background (not just text) inherits the same lift; the
  lift is small enough that the visual impact on those
  decorations is the right direction (a slightly lighter
  border on a 1px chip edge) rather than a regression.

**When to revisit**:

- If components that *should* be dim (inactive placeholder
  text, very subtle hint text) start to read as too bright,
  consider adding a dedicated `--color-text-faint` below
  `--color-text-muted` rather than dimming muted back down.
- If the gap to primary ever feels too tight, lift primary
  to `#d1d8e3` (slate-280) — but only if a real readability
  audit (e.g. axe-core WCAG AA pass on body text) flags
  primary itself. Don't lift primary speculatively.

---

## Decision: 填充 500 / 文字 400 拆分 + 彩底文字规则 + hover 提强(2026-08-15, contrast-color-r1)

**Context**: 对比度专项评审(token 样本页 + mmx vision + WCAG 数值三方交叉,
方法见 `.agents/skills/ui-review/`,证据见 `.trellis/tasks/08-15-contrast-color-r1/research/`)
发现灰阶文字全过 AA,但"彩色当文字用"存在系统性 FAIL:accent #3b5bdb 作文字
3.14:1(surface)、tool-error #ef4444 作文字 4.32(elevated)/3.60(accent-muted)。
另发现 hover 6% 叠加与 default 肉眼不可辨(此前被误归因为静态截图盲区)。

**Decision**:

1. 新增 `--color-accent-text: #7c9aff` 与 `--color-tool-error-text: #f87171`。
   **原则:填充 500 档 / 文字 400 档** — Tailwind-500 量级的饱和色在暗底上够
   图形对比(3:1)但不够文字(AA 4.5:1);彩色 token 作 ink 一律用 400 档兄弟
   token,禁止把填充 token 复用作文字色。
2. **彩底文字规则**:彩色/低亮度底(accent-muted、tool 色块底)上文字最低
   `--color-text-secondary`(muted 在 accent-muted 上仅 4.41),且禁同色系
   文字:accent 蓝字在 accent-muted 上 2.39、紫字(thinking)4.98 过线但与
   蓝底同色相"沉底"(亮度算不出的色相贴近问题,VLM 观感证实)。
3. `--color-bg-hover` 6%→10%、`--color-bg-active` 10%→14%,并立**最小可辨
   ΔL 规范**:交互态叠加必须相对 default 产生可辨亮度差,目标 hover≈1.2:1
   相对底色;当前 10%(≈1.2)在静态截图下仍被判"可辨但极弱",以真机裁决
   (R4.2),过弱上调 12%/16%,过强回落 8%/12%。
4. 11px mono 高频元数据位(侧栏分组头、消息耗时、memory 卡时间戳、常驻
   提示)由 muted 升 secondary——"字号惩罚"真实存在(数值过 AA 但小字感知
   发灰,双 VLM 独立判读);muted 保留给一次性角标,禁止 blanket replace。

**Rationale**: 三方证据链(数值 FAIL + VLM 点名 + 截图目验)成立才动 token;
VLM 对比度"估算值"不可信(实测把 7:1 判成 <3:1),裁决一律"数值管亮度、
VLM 管色相观感"。灰阶三档不动(2026-08-05 刚调,层级锚点)。

**When to revisit**:

- 真机复核 R2.3(error-text 纯深底荧光感)与 R4.2(hover 10% 强度),量级
  可在上述区间微调。
- elevated/border 层级感知(border × elevated 仅 1.05:1)与"填充色暗底"
  图形对比(accent 填充在 elevated 2.87 < 3:1,trace 进度条被点名)另立
  专题,未在本轮处理。

---

## Decision: Design system token expansion (2026-06-27, PR-1)

**Context**: the pre-2026-06-27 token system had color +
typography covered, but spacing / radius / type-scale / motion
/ shadow values were scattered as raw px / ms values across
the 44 components. PR-1 + PR-2 + PR-3d swept 438 raw values
into token form, formalizing the project's de-facto scale
(4-based spacing, 4/6/8/12 radius, font-size ladder) and
adding the missing motion vocabulary (durations + easings).

**Decision**: added 6 new token families to
`app/src/style.css` `@theme` block:

- **Spacing scale** (8 tokens): `--space-0..8` = 0/4/8/12/16/20/24/32/48 px
- **Radius scale** (5 tokens): `--radius-sm/md/lg/xl/pill` = 4/6/8/12/999px
- **Type scale** (7 sizes + 4 leading + 4 weights): `--text-xs..2xl`, `--leading-tight/normal/relaxed/loose`, `--weight-regular/medium/semibold/bold`
- **Motion** (6 durations + 3 easings): `--duration-instant/fast/base/slow/pulse/blink` + `--ease-out/spring/decelerate`
- **Shadow scale** (4 + ring): `--shadow-xs/sm/md/lg` + `--shadow-ring`
- **3 state tints** (above section): `--color-bg-hover/active/selected`

**Rationale**:

- The 4-based spacing scale matches the existing project
  convention (the same 4/8/12/16/20/24/32 px values were
  used ad-hoc in 20+ files). The token layer is additive —
  visual rhythm is unchanged.
- The 100/150/240ms motion duration split aligns with the
  pre-existing `popover-pattern.md` modal/popover 150ms-enter
  / 100ms-leave convention. The added `--duration-slow`
  (240ms) absorbs the toast (was 200ms ad-hoc) and the
  subagent drawer slide (was 180ms ad-hoc). `--duration-pulse`
  (1800ms) absorbs the subagent breathing animation that
  was hard-coded twice (tool card left bar, drawer section
  spinner). **Modal** 另有专用档位 `--duration-modal-in/out`
  (200/150ms) + `--ease-modal-in` / `--ease-accelerate`
  (2026-07-02, task 07-02-modal-motion-rhythm)；popover/drawer
  仍用 `--duration-base/fast`。
- `--ease-out` is `cubic-bezier(0.16, 1, 0.3, 1)` — a
  Linear-style snappy decel, replacing the bare CSS
  `ease-out` keyword. Slightly "harder" feel (faster
  initial deceleration), but the popover-pattern.md spec
  doesn't document the exact curve, so the visual delta
  is minimal.
- The 3 state tints are deliberately `color-mix` at use
  time (not precomputed hex) so a future `--color-accent`
  or `--color-text-primary` change propagates without
  re-balancing the tints.

**Migration path** (consumed in PR-2): the 438 raw values
in components were swept by sed in a follow-up PR
(`PR-2`); component CSS now references tokens verbatim.
The `:where(button)` baseline transition (PR-3d) is the
only global CSS rule that consumes the motion tokens
outside component scope — it provides a fast-color
fallback for buttons without explicit transitions.

**When to revisit**:
- If the spacing scale needs a half-step (`6px`, `10px`,
  `14px`) for a niche component, do NOT add `--space-1-5`
  tokens — add the raw value with a comment explaining
  why (per the "Don't add a token for a one-off use"
  rule below).
- If the project ever migrates to Tailwind v4 utility
  classes for spacing, the `--space-*` tokens will collide
  with Tailwind's built-in `--spacing-*` (note the
  missing `ing`). The token names were chosen to AVOID
  the collision so this is a non-issue today.

---

