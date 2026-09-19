<!-- Moved from popover-pattern.md 2026-09-19 (doc-split) -->

## Animation

> Added 2026-06-09 in the UI polish PR. All modal / popover
> instances in the project now have enter/leave transitions.
> Conventions captured here so future dropdowns / modals
> follow the same pattern.
>
> **Updated 2026-06-27 (PR-3d)**: the `150ms / 100ms` bare
> ms values are now expressed as `var(--duration-base)` /
> `var(--duration-fast)` tokens. The semantic
> `ease-out` / `ease-in` keywords are kept (the new
> `--ease-out` is a custom cubic-bezier that's slightly
> "harder" than CSS default; using the token vs keyword
> is a deliberate visual choice — see the Modal: fade +
> scale example below). The toast moves from
> `200ms` to `var(--duration-slow) var(--ease-out)` (240ms;
> a 40ms bump to match the project's motion vocabulary).
>
> **Updated 2026-07-02 (task 07-02-modal-motion-rhythm)**: the
> **modal** convention below is rewritten. Modal **mask**
> (overlay / backdrop) no longer animates — instant appear/
> disappear; only the modal **content** animates. New modal-
> specific tokens `--duration-modal-in/out` (200/150ms),
> `--ease-modal-in`, `--ease-accelerate` (both
> `cubic-bezier(0.25, 0.1, 0.25, 1)` = CSS `ease`, symmetric
> S-curve). Scale amplitude changed from `0.96↔1` to `0.1↔1`
> (enter grows from 10%, leave shrinks to 10%). Popover /
> Toast / Drawer rows below are unchanged.

### Convention: token-based durations

| Trigger | Enter | Leave |
|---|---|---|
| **Modal content** (8 modals；mask 不动画) | `var(--duration-modal-in) var(--ease-modal-in)` | `var(--duration-modal-out) var(--ease-accelerate)` |
| Popover (anchored) | `var(--duration-base) var(--ease-out)` | `var(--duration-fast) ease-in` |
| Toast (AppShell) | `var(--duration-slow) var(--ease-out)` | (same) |
| Subagent drawer (`SubagentDrawer.vue`) | `var(--duration-slow) var(--ease-decelerate)` | (same) |

Modal 是 200ms-in / 150ms-out + `ease` 平滑曲线 + scale `0.1↔1`（详见下节
"Modal: scale"）；**mask（overlay/backdrop）不参与动画，瞬间出现/消失**，
只有 content 做 scale+opacity。Popover 仍是 150/100ms + `--ease-out`/`ease-in`
（起手更利落，适合小尺寸弹出）。Toast 用 240ms 双向（引人注意，不作为其他
弹窗的先例）。Subagent drawer 是更重的右锚定全高面板，slide-in 更慢 +
`--ease-decelerate` 更"物理"。

### Modal: scale（mask 无动画）

Modal **content** 用 **scale `0.1 ↔ 1` + opacity**；**mask（overlay/
backdrop）不动画**——瞬间出现/消失。A 类（reka-ui Dialog）的 overlay 不写
animation，reka-ui 的 `usePresence` 检测到 overlay 无 animation-name → 立即
unmount；B 类（Vue `<Transition>`）的 backdrop 保留 `transition-duration`
仅作 Vue leave 计时，opacity 始终 1（否则 active class 提前移除会中断 content
过渡）。content keyframe（A 类用 `translate(-50%,-50%)` 居中，B 类用 flex
居中、无 translate）：

```css
@keyframes modal-zoom {
  from { opacity: 0; transform: translate(-50%, -50%) scale(0.1); }
  to   { opacity: 1; transform: translate(-50%, -50%) scale(1); }
}
@keyframes modal-zoom-out {
  from { opacity: 1; transform: translate(-50%, -50%) scale(1); }
  to   { opacity: 0; transform: translate(-50%, -50%) scale(0.1); }
}
```

**Trigger mechanism differs by component**:

- **reka-ui DialogContent**（A 类，5 个 modal：Settings/Memory/Audit/
  MarkdownDetail/PermissionGrants）: reka-ui 在 content 上设
  `data-state="open|closed"`，overlay **不写 animation**（mask 无动画），
  content 用 `[data-state]` 选择器：
  ```css
  .xxx-modal { animation: modal-zoom var(--duration-modal-in) var(--ease-modal-in) both; }
  .xxx-modal[data-state="closed"] { animation: modal-zoom-out var(--duration-modal-out) var(--ease-accelerate) forwards; }
  ```
  enter 加 `both` 避免 `fill-mode: none` 的首帧闪现。reka-ui 的 `Presence`
  靠 `animation-name` 切换检测 exit（close 时 name 变 → `unmountSuspended`
  → 等 `animationend` 卸载）；overlay 无 animation → close 时立即卸载。

- **Vue `<Transition>`**（B 类，3 个：ConfirmDialog/YoloConfirmModal/DiffModal）:
  包 `<Transition name="xxx">`，backdrop（Transition 根元素）只设
  `transition: opacity var(--duration-modal-in/out)`（opacity 不变，仅作
  计时）；content 的 `.xxx-enter-active .modal` / `.xxx-leave-to .modal` 设
  scale+opacity 过渡。见 `ConfirmDialog.vue` 参考实现。

**Modal shadow** (2026-06-27 PR1): all modal content surfaces use
`box-shadow: var(--shadow-xl)` (the largest tier). Pre-PR1 eight
modals hardcoded `0 16px 48px rgba(0,0,0,0.5)`; that value is now
the `--shadow-xl` token. Dropdown / popover / tooltip surfaces use
`--shadow-md` (`0 4px 12px rgba(0,0,0,0.4)`), not `--shadow-sm`.

### Reduced Motion (added 2026-06-27, PR-1)

`app/src/style.css` includes a top-level `@media
(prefers-reduced-motion: reduce)` block that collapses
all `animation-duration` and `transition-duration` to
`0.01ms` for users with the OS setting on. The convention
above applies in full only to users WITHOUT that setting;
reduced-motion users see instant appear/disappear (no fade,
no scale, no slide). Required for WCAG 2.3.3 accessibility.

### Confirmation Dialog Pattern (added 2026-06-11, 体验优化 PR `0140502`)

> **Use `app/src/components/common/ConfirmDialog.vue` for all
> destructive / confirmable actions.** This component supersedes
> the older per-action `DeleteWorktreeConfirm` /
> `DeleteModelConfirm` copies. When adding a new "are you sure?"
> dialog in the app, always reach for `ConfirmDialog` first.

**Props**:

| Prop | Type | Default | Notes |
|---|---|---|---|
| `open` | `boolean` | — | v-model binding; `v-if` mounts/unmounts the dialog |
| `title` | `string` | — | Header title (renders the warn icon automatically when `variant === "danger"`) |
| `variant` | `"danger" \| "warning" \| "default"` | `"danger"` | Drives the confirm-button color (red / accent-muted / default) |
| `confirmText` | `string` | `"确认"` | Confirm button label |

**Slot**: `body` — arbitrary content (use `<p>` for short messages,
nested markup for richer warnings).

**Emits**: `cancel` (Escape, backdrop click, ✕ button, "取消" button)
and `confirm` (Enter or confirm button click).

**Built-in behavior**:

- **Esc closes** (emits `cancel`).
- **Enter confirms** (emits `confirm`).
- **Backdrop click cancels** (`@click.self` on `.confirm-backdrop`).
- **Focus** is auto-moved to the confirm button on `open` (via
  `setTimeout(..., 0)` after the v-if mount), so Enter works
  without a prior Tab.
- **Transition** uses `name="confirm-modal"`；backdrop 无视觉动画，
  content 走 `--duration-modal-in/out` + `--ease-modal-in/accelerate`
  + scale `0.1↔1`（见上节 "Modal: scale"）。

**Why the component exists**: see the "Don't" section below
about `window.confirm()` in Tauri webview. The whole reason
`ConfirmDialog` is hand-rolled is that the native dialog
silently no-ops in this environment.

**Example usage** (session delete with body content):

```vue
<script setup lang="ts">
import ConfirmDialog from "../common/ConfirmDialog.vue";
const showConfirm = ref(false);
const sessionIdToDelete = ref<string | null>(null);

function askDelete(id: string) {
  sessionIdToDelete.value = id;
  showConfirm.value = true;
}
async function onConfirm() {
  if (sessionIdToDelete.value) await doDelete(sessionIdToDelete.value);
  showConfirm.value = false;
}
</script>

<template>
  <ConfirmDialog
    :open="showConfirm"
    title="删除 session"
    variant="danger"
    confirm-text="删除"
    @cancel="showConfirm = false"
    @confirm="onConfirm"
  >
    <p>该 session 包含 <strong>{{ messageCount }}</strong> 条消息,删除后无法恢复。</p>
  </ConfirmDialog>
</template>
```

**Convention: skip the dialog for empty containers.** A
"delete this empty session" / "delete this fresh worktree" /
"remove this unused provider" should NOT pop a confirm — the
destructive cost is zero. Only show the dialog when there is
real content the user might regret losing. The current rule in
`SessionList.vue` is: a session is "empty" iff its message
count is 0; non-empty sessions always go through
`ConfirmDialog`.

**Migration path** (optional, not blocking): the older
`DeleteWorktreeConfirm` and `DeleteModelConfirm` components
can be replaced with `ConfirmDialog` calls. The PRD
(`.trellis/tasks/06-11-session-loading/prd.md`) marked this
as "可选，不改也行" — defer until a third call site appears
or visual drift is reported.

### Popover: fade + slide (direction matches position)

Popover instances use **fade + slide**, where the slide direction
**MUST match the popover's open position**:

| Popover open direction | Slide keyframe |
|---|---|
| Upward (e.g. `ModelSelect` — `bottom: calc(100% + 4px)`) | `translateY(4px → 0)` (slides up from below) |
| Downward (e.g. worktree dropdown — `top: calc(100% + 4px)`) | `translateY(-4px → 0)` (slides down from above) |

This makes the popover feel like it's "emerging from" the trigger
button. Sliding the wrong direction (e.g. upward popover slides
*upward* from `translateY(0 → -4px)`) reads as the popover
"running away" from the trigger.

**Implementation**: wrap the popover in Vue `<Transition>` and
define scoped CSS. Reference ModelSelect.vue (upward) and
ChatPanel.vue worktree popover (downward).

### Don't: Animate the popover's parent container

Animate the popover element itself, not its parent. If the
parent (e.g. `.chat-panel__worktree`) has `transition` set on
itself, the trigger button next to the popover may shimmer or
shift during the animation. This is subtle but breaks the
illusion of a "floating" popover.

### Don't: Use `transition-delay` on popover

The popover should appear in sync with the user's click. A
delay (even 50ms) feels sluggish. The 150ms enter is the full
duration, not "150ms after a 100ms delay".

### Don't: Animate `width` / `height` of the popover

Size animations look broken at small sizes (4-8px change is
invisible) and create reflow on the rest of the page. The
popover should snap to its final size and only animate
`opacity` + `transform`.

---

