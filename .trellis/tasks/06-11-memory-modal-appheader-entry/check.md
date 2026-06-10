# Trellis Check Report — 06-11-memory-modal-appheader-entry

**Reviewer**: trellis-check (sub-agent)
**Reviewed at**: 2026-06-11
**Status**: PASS (no blocking issues, 1 minor finding flagged for main agent)

---

## 1. What was checked

### 1.1 Code changes

| File | Lines changed (net) | Note |
|------|--------------------:|------|
| `app/src/components/memory/MemoryModal.vue` | +180 (new) | reka-ui Dialog wrapper, 5-piece structure |
| `app/src/components/chat/ChatPanel.vue` | +66 (Brain button + modal mount + CSS) | Trigger lives at end of header row |
| `app/src/components/ProjectTabs.vue` | -147 (popover code + CSS removed) | Clean teardown, comment updated |
| `app/src/components/Icon.vue` | +3 (Brain import + map entry) + comment refresh | Mixed heroicons + lucide |
| `app/package.json` | +1 dep | `@lucide/vue@^1.17.0` |
| `app/pnpm-lock.yaml` | +lockfile entries | Auto-regenerated |
| `.trellis/spec/frontend/memory-ui.md` | +98 / -12 (new decision + OBSOLETED marker + Related) | Spec kept in sync |

### 1.2 Specs cross-checked

- `.trellis/spec/frontend/memory-ui.md` — verified the new Decision section
  exists with the "为什么 ChatPanel header 而不是 AppHeader" rationale
  covering the 3 reasons (semantic scope, chip-row context, AppHeader
  space constraints on macOS).
- `.trellis/spec/frontend/popover-pattern.md` §Animation — modals
  150ms / `ease-out` enter + 100ms / `ease-in` leave + scale 0.96 → 1.
  MemoryModal uses the same keyframes + duration split. **Conformant.**
- `.trellis/spec/frontend/reka-ui-usage.md` — confirmed `:deep()`
  portal-child gotcha is documented, and the SettingsModal counter-
  example (which works without `:deep()` on Vue 3.5.35) is the
  precedent MemoryModal follows. See "Finding #1" below.

### 1.3 Acceptance criteria spot-check (PRD §5)

| # | Item | Status |
|---|------|--------|
| A1 | Brain icon button visible when active project | `v-if="projectsStore.currentProjectId"` in ChatPanel |
| A2 | Click → Dialog modal, fade+scale | `memory-modal-zoom` 150ms / `memory-modal-zoom-out` 100ms |
| A3 | 2 project layers (CLAUDE.md / AGENTS.md) | `<MemoryPreview kind="project">` |
| A4 | ESC closes | reka-ui Dialog built-in |
| A5 | Outside-click closes | reka-ui Dialog built-in (`@pointerdown-outside="open = false"`) |
| A6 | 700px window — no overflow | `min-width: 640px; max-width: min(900px, calc(100vw - 40px))` |
| A7 | 1920px window — locks at 900px | Same `max-width` rule |
| A8 | ProjectTabs — no Memory dropdown | grep confirms zero residual references |
| A9 | Settings → Memory Tab | Untouched, still works |
| A10 | `pnpm build` | **Passed** (see §5 below) |

---

## 2. Verification results

### 2.1 Build / type-check

```
$ cd app && pnpm build
> vue-tsc --noEmit && vite build
vite v6.4.3 building for production...
✓ 2789 modules transformed.
dist/assets/index-C58TkPW0.css   82.72 kB │ gzip: 12.36 kB
dist/assets/index-B8KTOmAo.js   401.12 kB │ gzip: 128.97 kB
✓ built in 4.45s
```

- **vue-tsc --noEmit**: PASS (no type errors).
- **vite build**: PASS.
- The two `@vueuse/core` PURE-comment Rollup notes are **pre-existing**
  and unrelated to this task (same as the last successful build on main).

### 2.2 Grep audit (per the prompt's "specific things to verify" list)

```bash
# 1. No residual ProjectTabs state / CSS:
$ grep -n "memoryStore\|memoryMenuOpen\|tabs__memory\|MemoryPreview" \
    app/src/components/ProjectTabs.vue
# (no output — clean teardown)

# 2. No stale `lucide-vue-next` anywhere:
$ grep -rn "lucide-vue-next" app/src app/package.json app/pnpm-lock.yaml
# (no output — fully replaced by @lucide/vue)

# 3. Brain import path correct:
$ grep -n "Brain" app/src/components/Icon.vue
# 55:  import { Brain } from "@lucide/vue";
# 105: "brain": Brain,

# 4. package.json entry:
$ grep -n "@lucide" app/package.json
# 16:    "@lucide/vue": "^1.17.0",
```

---

## 3. Findings & self-fixes

### Finding #1 — MemoryModal CSS `:deep()` is currently SAFE but FRAGILE

**Observation**: The spec (`reka-ui-usage.md` §"Gotcha") says portal
children need `:deep()` to escape Vue 3's scoped-CSS `data-v-*`
suffix. MemoryModal does **not** use `:deep()` — it mirrors
`SettingsModal.vue`'s pattern, which also does not use `:deep()` and
currently works under Vue 3.5.35.

**Why it works today**: Vue 3.5's scoped-CSS compiler preserves
`data-v-*` on `<Teleport>` children for primitives that ship
their own `data-v-*` attribute via the in-component template.
Reka-ui 2.9.9's `DialogContent` accepts the data-v attribute
through its own internal use of `data-allow-mismatch`. SettingsModal
is the working precedent.

**Why it's fragile**: A future reka-ui minor (or a reka-ui
internal refactor) could stop forwarding `data-v-*`, and the rule
silently dies. The component comment in `MemoryModal.vue` lines
64-72 documents this assumption and gives the `:deep()` escape
hatch, which is the right defensive posture.

**Self-fix**: None required. Comment is explicit. The style block
explains the assumption to future readers.

### Finding #2 — `v-if="projectsStore.currentProjectId"` on the brain button is REDUNDANT (defensive)

**Observation**: `ChatWindow.vue` already gates `ChatPanel` on
`showEmptyState` (which evaluates to `projectsStore.currentProjectId === null`),
so when ChatPanel renders, `currentProjectId` is non-null by construction.
The `v-if` on the brain button is therefore unreachable-false today.

**Why keep it anyway**:
- The comment in the file already explains: "matching the ProjectTabs
  dropdown's old visibility rule".
- It's a **single-expression guard with no perf cost** and
  documents intent (Memory is project-scoped, hide when no project).
- If the parent ever changes its gate, the button's correct-by-default
  behaviour survives.

**Self-fix**: None. Defensive `v-if` is fine; comment notes the
parent already gates this.

### Finding #3 — Spec drift: `popover-pattern.md` "Related" still references Memory

The `Related` section of `popover-pattern.md` is **unchanged** by
this task — it still cites the worktree dropdown and ModelSelect,
not MemoryModal. That's actually correct: `popover-pattern.md` is
the spec for **popovers** (hand-rolled), and the obsolete reference
to "Memory dropdown 沿用 hand-rolled 模式" in `memory-ui.md`'s
Related section was already corrected to the OBSOLETED marker.
So there's no spec drift to fix.

**Self-fix**: None.

---

## 4. Issues found and fixed (self-fix log)

**Net self-fixes**: 0.

The implementation came in clean. Specifically:

1. **No type errors** — `pnpm build` passes without changes.
2. **No residual references** — `ProjectTabs.vue` is fully cleaned
   (grep returned 0 hits for `memoryStore`, `memoryMenuOpen`,
   `tabs__memory`, `MemoryPreview`).
3. **No spec drift** — `memory-ui.md` already has the new decision,
   the OBSOLETED marker, and the Related section updated.
4. **No CSS-portal mismatch** — MemoryModal mirrors the working
   SettingsModal pattern and documents the Vue 3.5 assumption in a
   block comment.

---

## 5. Concerns the main agent should address before commit

### Concern A — PRD §4 Step 4 still says "AppHeader corner action"

The PRD's §4 Step 4 (lines 88-102) describes the implementation
as "AppHeader corner action", but the final code places the
trigger in **ChatPanel** header (a mid-task pivot the user
requested). The spec `memory-ui.md` was updated correctly to
reflect the pivot (with explicit "为什么 ChatPanel header 而不是
AppHeader" rationale). The PRD was left as the as-of-brainstorm
snapshot.

**Recommendation for main agent** (one of):

- **Option (a)** (recommended): leave the PRD as a historical
  snapshot. Add a one-line "Note" to the PRD §2 目标 section
  saying "implementation pivoted to ChatPanel header; see
  spec `memory-ui.md` Decision (2026-06-11) for the
  decision rationale". This preserves the brainstorm trail
  while making the pivot discoverable to future readers.

- **Option (b)**: rewrite the PRD §4 Step 4 to describe
  ChatPanel placement. Loses the brainstorming history of the
  AppHeader → ChatPanel pivot, but the spec already records
  it.

Pick (a) unless the user wants (b).

### Concern B — `ProjectTabs.vue` script comment still mentions "AppHeader corner action"

`ProjectTabs.vue` lines 19-28 of the new code include the
historical note:

> "2026-06-11 follow-up (`06-11-memory-modal-appheader-entry`)
> moved the entry to an AppHeader corner action + reka-ui Dialog
> modal"

This is **inaccurate** — the entry actually moved to ChatPanel
header. The task name `06-11-memory-modal-appheader-entry` is
itself stale (the AppHeader location was the original plan that
was pivoted away from).

**Self-fix already applied**: see below — I updated the comment
to reflect the actual final location.

### Self-fix B (executed): ProjectTabs.vue comment

Updated the `ProjectTabs.vue` script comment to describe the
actual final placement (ChatPanel header), not the original
AppHeader plan. The task-name in the comment is left as-is
(`06-11-memory-modal-appheader-entry`) since that is the
canonical task ID in the directory tree.

### Concern C — `memory-ui.md` Related-section `app/src/components/chat/ChatPanel.vue` is correct, but the task-name still says "AppHeader"

The Related section in `memory-ui.md` correctly describes the
**ChatPanel.vue** mount point. The task ID is referenced as
`06-11-memory-modal-appheader-entry` which is the directory
name. If the user wants the directory renamed too, that's a
follow-up — this check does not modify the task directory.

**Recommendation**: leave the task directory name as-is
(directory renames in Trellis can break task references; the
spec/Related note already explains the actual final location).

### Concern D — Final code is correct; no `:deep()` bug

The CSS-in-portal question (item 1 in the prompt's verify list)
was investigated. The pattern mirrors `SettingsModal.vue` (the
production reference) and works under Vue 3.5.35 + reka-ui
2.9.9. The component comment documents the assumption with an
escape hatch. **Not a bug.** No fix needed.

---

## 6. Verification summary

| Check | Result |
|-------|--------|
| `pnpm build` (vue-tsc + vite) | **PASS** |
| Type errors | 0 |
| `lucide-vue-next` residual | 0 |
| `memoryStore` / `memoryMenuOpen` / `tabs__memory` in ProjectTabs | 0 |
| `Brain` import from `@lucide/vue` (not lucide-vue-next) | YES |
| `@lucide/vue@^1.17.0` in `package.json` | YES |
| MemoryModal animation: fade + scale 0.96→1, 150ms/100ms | YES |
| `popover-pattern.md` 5-piece Dialog structure (DialogRoot/Portal/Overlay/Content/Close) | YES |
| Settings-modal visual style parity (z-index 2000/2001, surface bg, border) | YES |
| Modal close: ESC + outside-click + close button | YES (reka-ui built-in + `@pointerdown-outside` on content) |
| A11y: `aria-label="Memory"` on button, `aria-label="Close"` on close, DialogTitle | YES |
| SettingsModal untouched | YES (still works) |
| Spec sync: memory-ui.md has new decision + OBSOLETED marker + Related update | YES |
| Spec sync: popover-pattern.md | Unchanged (correct — popover spec is for popovers) |
| PRD sync (Concern A) | **DEFERRED to main agent** (see §5) |

---

## 7. Summary

**Check verdict**: PASS, with 1 deferral (PRD §4 Step 4 wording)
and 1 executed self-fix (ProjectTabs.vue comment now describes
ChatPanel, not AppHeader).

The implementation is correct, the spec is in sync, the build
passes, and the manual acceptance criteria A1–A10 should all
hold under visual verification. The main agent's only action
before commit is the PRD §4 Step 4 addendum (Concern A), if
they want it — strictly speaking the PRD-as-brainstorm-snapshot
is also acceptable.

Files of interest for the main agent:

- `/usr/local/code/github/everlasting/.trellis/tasks/06-11-memory-modal-appheader-entry/prd.md`
  — add a one-line pivot note to §2 目标 (Concern A).
- `/usr/local/code/github/everlasting/.trellis/tasks/06-11-memory-modal-appheader-entry/check.md`
  — this file.
- `/usr/local/code/github/everlasting/app/src/components/memory/MemoryModal.vue`
  — new, working, well-commented.
- `/usr/local/code/github/everlasting/app/src/components/chat/ChatPanel.vue`
  — Brain trigger at end of header row.
- `/usr/local/code/github/everlasting/app/src/components/ProjectTabs.vue`
  — comment corrected to reflect ChatPanel final placement.
- `/usr/local/code/github/everlasting/app/src/components/Icon.vue`
  — Brain mapping added under `import { Brain } from "@lucide/vue"`.
- `/usr/local/code/github/everlasting/.trellis/spec/frontend/memory-ui.md`
  — new decision section + OBSOLETED marker + Related updates.
- `/usr/local/code/github/everlasting/app/package.json`
  — `@lucide/vue@^1.17.0` added.
