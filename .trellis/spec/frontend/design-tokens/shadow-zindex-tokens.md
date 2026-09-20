<!-- Moved from design-tokens.md 2026-09-19 (doc-split) -->

## Shadow Scale (added 2026-06-27, PR-1)

Four elevation tiers + a focus-ring token. Alphas are
tuned for the dark `--color-bg-app` (#0a0e14) — pure-black
shadows with the same RGB channel values would have ~0.3
luminance lift on a near-black background (read as flat
"outline"), so the dark-bg alpha scale is 0.32 / 0.4 /
0.5.

| Token | Value | Use |
|---|---|---|
| `--shadow-xs` | `0 1px 2px rgba(0, 0, 0, 0.32)` | Chip hover lift, small raised chip |
| `--shadow-sm` | `0 2px 4px rgba(0, 0, 0, 0.4)` | Popover, dropdown, subagent drawer "↓ N new" floating button |
| `--shadow-md` | `0 4px 12px rgba(0, 0, 0, 0.4)` | AppShell toast, larger popover |
| `--shadow-lg` | `0 8px 24px rgba(0, 0, 0, 0.5)` | Reserved intermediate elevation tier (between md dropdowns and xl modals). Pre-PR1 some modals used this; the modal family now uses `--shadow-xl`. |
| `--shadow-xl` | `0 16px 48px rgba(0, 0, 0, 0.5)` | **Modal / large dialog (largest tier)**. Added 2026-06-27 PR1 — 8 modals (Settings / Memory / AuditLog / Diff / Yolo / DeleteWorktree / MarkdownDetail / ConfirmDialog) previously hardcoded this exact value. See `popover-pattern.md` "modal = xl". |
| `--shadow-ring` | `0 0 0 3px color-mix(in srgb, var(--color-accent) 20%, transparent)` | Focus ring (chat input focus-within, form input :focus, AuditLog select open). Uses the 20% accent mix that the pre-PR-1 chat input ring used directly as `box-shadow`. |

### Shadow exceptions (2026-06-27 PR1)

The following shadows are deliberately NOT mapped to a token —
each is a one-off value (unique alpha or offset/blur combo) where
no tier fits without a visible change. They stay inline so a
future `grep "box-shadow: 0"` doesn't read them as drift; this
table is the authoritative "these are intentional" list.

| File | Value | Why kept (not tokenized) |
|---|---|---|
| `MessageList.vue:262` (scroll-to-bottom FAB) | `0 2px 8px rgba(0,0,0,0.18)` | Extra-light float for a small FAB; lighter than `--shadow-sm` |
| `SubagentDrawer.vue:896` (↓N new pill) | `0 2px 8px rgba(0,0,0,0.25)` | Same FAB family, slightly stronger |
| `SessionList.vue:568` (ctx menu) | `0 4px 16px rgba(0,0,0,0.2)` | Wider blur + lighter alpha than `--shadow-md` |
| `HiddenProjectsMenu.vue:178` | `0 8px 24px rgba(0,0,0,0.35)` | lg offset/blur but lighter alpha than `--shadow-lg` |
| `ActivityPanel.vue:671` (floating panel shadow,值自 ChecklistCard 原样迁移 09-02) | `0 6px 24px rgba(0,0,0,0.35)` | Between md/lg for the floating activity panel |
| `ActivityPanel.vue:1012` (悬浮球空态 CTA) | `0 4px 14px rgba(0,0,0,0.3)` | Floating empty-state CTA (migrated from ChecklistCard) |
| `EmptyProjectState.vue:187` | `0 1px 0 color-mix(accent 35%)` | 1px inner highlight, not an elevation shadow |
| `MessageActionsMenu.vue:350` | `0 0 0 2px color-mix(accent 25%)` | 2px focus ring (deliberately thinner than `--shadow-ring`'s 3px) |
| `MemoryLayerItem.vue:240,249` | `0 0 0 2px color-mix(var(--color-status-success)/--warn 25%)` | Status-dot ring; colors tokenized in PR2 (`--color-status-success`/`--color-status-warn`), only the 2px ring form is non-token (vs `--shadow-ring`'s 3px) |

If a future refactor adds a `--shadow-fab` (light float) token
covering the FAB family (MessageList / SubagentDrawer), the first
two rows can retire.

**Don't add a non-ring shadow that uses the accent color**
(purple/violet glow). Pre-PR-1 some components had subtle
accent-tinted shadows; the dark-bg elevation ladder above
is more honest — the surface rises via black, not color.

---

## Z-Index Ladder (added 2026-08-23, 08-23-zindex-ladder-tokens)

13 semantic tokens in `app/src/style.css` `@theme`. **The values ARE the
contract** — they were swept 1:1 from the de-facto layers across 29 files
(42 sites), zero behavior change. Pick a tier from this table; never
invent a new raw value in a component.

| Token | Value | Band |
|---|---|---|
| `--z-raised` | 100 | Header dropdowns / side panel (TracePanel, Mode/ModelSelect, WorktreeChip) |
| `--z-drawer-overlay` | 105 | Mobile drawer overlay (AppShell) |
| `--z-drawer` | 110 | Mobile drawer body (Sidebar) |
| `--z-input-pop` | 200 | Chat-input popovers (latency, token usage, TriggerMenu) |
| `--z-sheet-overlay` | 999 | Heavy-surface overlay (SubagentDrawer) |
| `--z-sheet` | 1000 | Heavy surfaces (SubagentDrawer; DiffModal moved to the modal family 2026-09-20) |
| `--z-confirm` | 1100 | Confirm dialogs (ConfirmDialog, DeleteWorktreeConfirm, RevertConfirmModal) |
| `--z-confirm-critical` | 1200 | Critical confirm (YoloConfirmModal) |
| `--z-modal-overlay` | 2000 | Modal family backdrop (9 modals — DiffModal joined 2026-09-20) |
| `--z-modal` | 2001 | Modal family content |
| `--z-over-modal` | 3000 | Must beat modal family: in-modal reka Select portals (6 sites, `!important`), msg actions menu / latency tooltip |
| `--z-toast` | 5500 | ToastProvider |
| `--z-top` | 9999 | Ceiling: SessionList ctx menu, AppShell legacy toast. Need "above everything"? use this — never `+1`. |

**Micro-local exception**: stacking that never leaves a component (or a
documented intra-family band like the chat floating cards 50/60/70) may
keep raw values, but each site MUST carry a comment stating what it
covers and what covers it. Grep `z-index` for stragglers when adding a
new layer.

**Teleport rule (2026-09-20, jjh-mono 实测回归)**: any `position: fixed`
overlay mounted inside MessageList rows (DiffModal / RevertConfirmModal
in MessageItem) MUST `<Teleport to="body">`. The virtualized rows use
inline `transform: translateY()` positioning (`MessageList.vue`), and a
transform ancestor creates a stacking context that traps fixed
descendants — the overlay's z-index then competes only within its row
and sibling rows paint on top of it. Picking a higher ladder tier does
NOT fix this; the teleport does. Test consequence: teleported content is
invisible to `wrapper.find` — query `document.body` instead (see
`RevertConfirmModal.test.ts`).

---

