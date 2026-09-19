<!-- Moved from design-tokens.md 2026-09-19 (doc-split) -->

## Typography Tokens

| Token | Value | Use |
|---|---|---|
| `--font-sans` | `"HarmonyOS Sans SC", -apple-system, BlinkMacSystemFont, "Microsoft YaHei UI", sans-serif` | Default body font |
| `--font-mono` | `ui-monospace, "SF Mono", "Cascadia Code", "Source Code Pro", Menlo, Consolas, monospace` | Monospace (chip labels, hint text, status bar, code blocks) |

The sans stack starts with `HarmonyOS Sans SC` (bundled,
472KB woff2 subset) — see `.trellis/spec/frontend/cjk-fonts.md`
for why. Mono falls back to system fonts only; we don't
bundle a CJK mono font.

### Type Scale (added 2026-06-27, PR-1)

| Token | Value | Use |
|---|---|---|
| `--text-2xs` | `10px` | Sub-caption / 角标 metadata (added 2026-06-27 PR2): `(edited)` label, MemoryPreview/DiffView metadata, ModeSelect/TriggerMenu/ModelSelect captions — 21 ad-hoc 10px values were swept into this token |
| `--text-xs` | `11px` | Mono metadata: chip labels, hint text, sidebar header (`SESSIONS`), status bar, caption |
| `--text-sm` | `12px` | Caption / hint / small button / form help |
| `--text-base` | `13px` | Form input, message body, dropdown option |
| `--text-md` | `14px` | Default app font (`:root font-size`), ChatInput editor body |
| `--text-lg` | `16px` | Section title, card title, chat input send button |
| `--text-xl` | `20px` | Empty-state hero title (`EmptyProjectState`, `chat-panel__empty`) |
| `--text-2xl` | `24px` | Reserved (not used yet — landing page / hero title) |

**Convention**: 11-12px is the "metadata" tier (mono, hints,
chips). 13-14px is the "content" tier (body, message
bubble, form input). 16-20px is the "title" tier (section
header, empty-state hero).

### Line Heights (added 2026-06-27, PR-1)

| Token | Value | Use |
|---|---|---|
| `--leading-tight` | `1.3` | Headings, dense chip rows |
| `--leading-normal` | `1.5` | Default body, form input |
| `--leading-relaxed` | `1.6` | Chat message bubble (was hard-coded `1.6` in `MessageItem.vue`) |
| `--leading-loose` | `1.75` | Reserved for long-form content (not used yet) |

The 1.6 (vs Tailwind v4's default 1.625) is intentional —
chat message rhythm is slightly tighter to keep adjacent
user/assistant turns visually grouped.

### Font Weights (added 2026-06-27, PR-1)

| Token | Value | Use |
|---|---|---|
| `--weight-regular` | `400` | Default body, message text |
| `--weight-medium` | `500` | Button label, chip emphasis, sidebar project path |
| `--weight-semibold` | `600` | Title, section header, strong button |
| `--weight-bold` | `700` | Reserved (not used — `--weight-semibold` carries all current emphasis needs) |

---

