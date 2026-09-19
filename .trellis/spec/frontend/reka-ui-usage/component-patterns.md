<!-- Moved from reka-ui-usage.md 2026-09-19 (doc-split) -->

## Pattern: Tooltip for hover affordances (added 2026-06-10, A4 token-usage)

Use reka-ui's `Tooltip` primitive for **hover-only static
information** — e.g. breaking down a single number into
its components, or providing a one-line hint that doesn't
need its own click target.

**Production instance** (2026-06-10, A4; updated 2026-08-21):
`app/src/components/chat/ChatInputHintRow.vue`'s
`chat-input__token-usage` chip (e.g. "14.2K · 7% / 200K") was
the original A4 instance — hovering out a 4-line breakdown
(`input / cache_read / cache_creation / output`). On 2026-08-21
(quota panel relocation follow-up to
`08-20-turn-usage-event-quota-view`) that chip became a **click
popover** (`ChatInputTokenUsage.vue`, hand-rolled pattern — see
`popover-pattern.md`) because its content grew into a full usage
dashboard. The remaining Tooltip production user is
`app/src/components/chat/MessageItemFooter.vue` — consult it for
the six-piece structure in situ before writing a new one.

**Six-piece structure** (always required, in this order):

```vue
<TooltipProvider>
  <TooltipRoot :delay-duration="150">
    <TooltipTrigger as-child>
      <span class="my-chip">14.2K</span>
    </TooltipTrigger>
    <TooltipPortal>
      <TooltipContent class="my-chip__tooltip" :side-offset="4">
        <TooltipArrow class="my-chip__tooltip-arrow" />
        <!-- tooltip body -->
      </TooltipContent>
    </TooltipPortal>
  </TooltipRoot>
</TooltipProvider>
```

**Why all six pieces**:

- **`TooltipProvider`** — top-level context provider. reka-ui 2.9.9's
  `TooltipRoot` is **not** self-contained: it relies on a
  `TooltipProviderContext` (Vue's Symbol-based `provide`/`inject`)
  that the Provider `provide`s. Rendering `TooltipRoot` without a
  `TooltipProvider` ancestor throws at runtime with
  `Injection Symbol(TooltipProviderContext) not found` and the
  entire Vue tree (here: `ChatWindow`) goes blank. TypeScript
  / `pnpm build` does NOT catch this because the inject is
  runtime-only. **Always wrap TooltipRoot in TooltipProvider.**
  Add it as a local wrapper at the consumer site (one provider
  per Tooltip instance, NOT app-root) — lifting to app root is
  YAGNI for a single consumer.
- **`TooltipRoot`** — context provider; `delay-duration` (ms) defers the open so quick mouse-passes don't trigger.
- **`TooltipTrigger as-child`** — merges trigger props onto the existing child (`<span>` / `<button>`) so the chip's own class + click handler are preserved. **Without `as-child` the trigger renders as a `<button>` that wraps the chip — you lose styling and get a nested clickable.**
- **`TooltipPortal`** — portals to `<body>` to escape overflow containers. **Required for the portal-child styling to work** (see gotcha above).
- **`TooltipContent`** — receives `data-state="delayed-open|closed"` for animation; `side-offset` is the gap between trigger and tooltip (4px is the project default, matches `popover-pattern.md`).
- **`TooltipArrow`** — the little triangle pointing at the trigger. Optional but conventional; users expect it.

**Don't**: wrap `TooltipContent` in `v-if`. reka-ui
manages the open/close lifecycle itself; `v-if` will fight
it and the tooltip will flicker or fail to open.

**Don't**: set `delay-duration="0"`. Even 100-150ms
defer prevents "tooltip pops up on every mouse pass" —
annoying for dense chip rows. The project's default
delay-duration is 150ms.

**Styling**: see `<style scoped>` gotcha above —
`TooltipContent` portals to body, so the rule MUST be
wrapped in `:deep()` to match. Trigger styles stay scoped.

**Example CSS** (from ChatInput.vue A4):

```css
/* trigger — stays scoped, in-component */
.chat-input__token-usage { ... }

/* tooltip content — :deep() required */
:deep(.chat-input__token-usage-tooltip) {
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  padding: 8px 12px;
  font-size: 12px;
  z-index: 1000;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
}

@keyframes tooltip-enter {
  from { opacity: 0; transform: translateY(2px); }
  to   { opacity: 1; transform: translateY(0); }
}

:deep(.chat-input__token-usage-tooltip[data-state="delayed-open"]) {
  animation: tooltip-enter 150ms ease-out;
}
```

**Don't use reka-ui Tooltip** for click-triggered
dropdowns / menus. Those are the hand-rolled popover
pattern (see `popover-pattern.md`).

---

## D3 PR2 (2026-06-17): `DropdownMenu` for per-message actions

D3 PR2 added the first production use of reka-ui's
`DropdownMenu` primitive — for the per-message ⋯ menu on
chat rows (`<MessageActionsMenu>` mounted in
`MessageItem.vue`). The dropdown has three items: Edit,
Resend (disabled, PR3), Copy.

### Why reka-ui `DropdownMenu` (not the hand-rolled popover)

`popover-pattern.md` documents the project's hand-rolled
popover pattern (used by `ModelSelect`, `ModeSelect`,
`TriggerMenu`, worktree dropdown). It's a stable
`onDocumentClick` + `Esc` close pair. For the message
hover menu, reka-ui is the right primitive because:

- The trigger is per-row ephemeral (appears on `:hover`,
  hides on `:mouseleave`). The hand-rolled pattern
  assumes a stable trigger element; binding a
  document-level click handler that re-checks the
  hover state on every render is awkward.
- Reka-ui `DropdownMenu` ships keyboard a11y out of the
  box: arrow up/down navigation, `Enter` to select,
  `Esc` to close, focus-return to the trigger. The
  hand-rolled pattern would need ~50 lines of keydown
  handler to match.
- The trade-off (acknowledged in `popover-pattern.md`):
  we now have two popover implementations in the
  codebase. Future work could extract a `usePopover`
  composable to consolidate. Out of scope for D3.

### Component shape (six pieces, in order)

```vue
<DropdownMenuRoot>
  <DropdownMenuTrigger as-child>
    <button class="msg-actions__trigger">…</button>
  </DropdownMenuTrigger>
  <DropdownMenuPortal>
    <DropdownMenuContent
      class="msg-actions__content"
      :side-offset="4"
      align="end"
    >
      <DropdownMenuItem
        class="msg-actions__item"
        :disabled="!canEdit()"
        @select="onEdit"
      >
        <Icon name="pencil" :size="14" />
        <span>编辑</span>
      </DropdownMenuItem>
      <DropdownMenuItem
        class="msg-actions__item"
        :disabled="true"
        @select.prevent
      >
        <Icon name="refresh" :size="14" />
        <span>重发</span>
        <span class="msg-actions__item-hint">PR3 待实施</span>
      </DropdownMenuItem>
      <DropdownMenuSeparator class="msg-actions__separator" />
      <DropdownMenuItem
        class="msg-actions__item"
        @select="onCopy"
      >
        <Icon name="copy" :size="14" />
        <span>复制</span>
      </DropdownMenuItem>
    </DropdownMenuContent>
  </DropdownMenuPortal>
</DropdownMenuRoot>
```

### Required pieces (and why)

- **`DropdownMenuRoot`** — the context provider. reka-ui
  2.9.9's `DropdownMenuContent` is **not** self-contained:
  it relies on a `DropdownMenuContext` (Vue's Symbol-based
  `provide`/`inject`) that the Root `provide`s. Rendering
  `DropdownMenuContent` without a `DropdownMenuRoot`
  ancestor throws at runtime with
  `Injection Symbol(DropdownMenuContext) not found`.
  TypeScript / `pnpm build` does NOT catch this because
  the inject is runtime-only. **Always wrap
  `DropdownMenuContent` in a `DropdownMenuRoot`.**
- **`DropdownMenuTrigger as-child`** — merges trigger
  props onto the existing child (the `<button>`) so the
  trigger's own class + click handler are preserved.
  **Without `as-child` the trigger renders as a default
  `<button>` that wraps the child — you lose the styling
  and get a nested clickable.**
- **`DropdownMenuPortal`** — portals to `<body>` to
  escape overflow containers. **Required for the
  portal-child styling to work** (see the `:deep()`
  gotcha above). The dropdown can otherwise be clipped
  by the `.msg` row's `overflow` (none today, but the
  portal is cheap insurance against future changes).
- **`DropdownMenuContent`** — receives
  `data-state="open|closed"` for animation. `side-offset`
  is the gap between trigger and dropdown (4px matches
  the project default). `align="end"` aligns the right
  edge of the dropdown with the right edge of the
  trigger — the typical pattern for a top-right ⋯
  trigger.
- **`DropdownMenuItem`** — receives `data-highlighted`
  (focus / hover) and `data-disabled` (the `:disabled`
  prop) as CSS selectors. The `@select` event fires
  on click / Enter; on any `:disabled` item bind
  `@select.prevent` (not a bare `@select`) so a stray
  click / Enter doesn't dismiss the menu with nothing
  happening (misleading).
- **`DropdownMenuSeparator`** — a thin horizontal rule
  between the action groups. The CSS class on the
  separator follows the same BEM-style convention as
  every other component in the codebase.

### Don't: wrap `DropdownMenuTrigger` in `TooltipTrigger` (as-child nesting)

Never nest a `TooltipTrigger as-child` around a
`DropdownMenuTrigger as-child` that shares the same
`<button>`. Both `as-child` wrappers merge their
listeners onto the one element, and reka-ui's Tooltip
registers a `pointerdown` handler that **swallows the
click** — the DropdownMenu never receives its open
signal. Hover-reveal still works (it's `:hover`-driven),
which masks the bug during casual testing.

Root cause of the D3 MessageActionsMenu "click 没反应"
bug (fixed 2026-06-17). Symptom: hover shows the ⋯
button, click is dead-silent, no console error.

**Fix**: for a hint on the trigger, use the native
`:title` attribute — it doesn't participate in the DOM
event flow, so zero conflict. (`MessageActionsMenu.vue`
uses `:title` after the fix.) Same underlying rule as
the §"Don't use reka-ui Tooltip for click-triggered
dropdowns" gotcha above, from the inverse direction.

### Don't: forget `DropdownMenuPortal`

Without the portal, the dropdown renders inline next to
the trigger. If the trigger is inside an `overflow:
hidden` ancestor (e.g. a future `.msg__tools` clipping
its content), the dropdown is clipped and invisible.
The portal is the default in the reka-ui docs for a
reason; mirror the pattern in `SelectContent` /
`DialogContent` etc. (see the portal gotcha above).

### Don't: bind `@select` (without `.prevent`) on a `:disabled` item

On a `:disabled` `DropdownMenuItem`, the `@select`
event can still fire on Enter / click in some reka-ui
versions even with `:disabled` set, if a handler is
bound. Bind `@select.prevent` instead (no-op handler
+ prevented menu close) so a disabled item genuinely
does nothing. (D3 historical: the Resend item was a
disabled placeholder in PR2 and used this `.prevent`
guard; PR3 made Resend a real action with
`@select="onResend"`, so the guard is no longer on
Resend — but the rule still applies to any future
disabled item.)

### Don't: re-style items via inline `style=""`

The items use BEM classes (`.msg-actions__item`,
`.msg-actions__item-icon`, `.msg-actions__item-hint`).
Inline `style` would bypass the design tokens and
break future theme swaps.

### Reference

- `app/src/components/chat/MessageActionsMenu.vue` —
  production instance.
- `app/src/components/chat/MessageItem.vue` — parent
  that mounts it, plus the inline edit-mode UI.
- `.trellis/spec/frontend/state-management.md` §
  "D3 PR2 (2026-06-17): inline message edit" — the
  store API + flow.

---

## Date/Time primitives (2026-08-29, task `08-29-sched-datetime-pickers`)

ScheduledTasksTab 的日期/时间输入换用 reka-ui 2.9.9 的 `DatePicker*` /
`TimeField*`(包装组件 `app/src/components/common/AppDatePicker.vue` /
`AppTimeField.vue`,字符串 v-model 契约)。四个版本钉死约束,升 reka 时复查:

### 1. `DatePickerPortal` 在 2.9.9 包根不存在

`dist/DatePicker/` 里有 `DatePickerPortal.js` 文件,但包根 **不导出** 它
(`import { DatePickerPortal } from "reka-ui"` 得到 `undefined`),渲染成
`Invalid vnode type` 且无报错弹层直接不出现。教训:**确认原语存在要看
包根导出,不能只看 dist 文件名**(`node -e "import('reka-ui').then(m =>
console.log(typeof m.DatePickerPortal))"`)。DatePickerContent 自带
portal,直接 `DatePickerRoot > DatePickerTrigger / DatePickerContent`。

### 2. DatePicker 弹层样式:scoped / `:deep()` 都够不到,用非 scoped 块

`DatePickerContent` 经 DatePickerPortal(Teleport)+ popper 渲染链后,
元素**不带任何 data-v scope attr**(与 SelectContent 不同——后者的
包裹层根会拿父组件 scope attr,所以 Select 的 `:deep()` 生效)。实证:
`:deep(.adp__pop)` 与 plain scoped 的 computed background 都是
transparent。修法:弹层样式放组件的**第二个非 `<style scoped>` 块**,
类名 `adp__` 前缀命名空间化防泄漏。

### 3. 弹层 z-index 要提 reka 的 fixed popper 包裹层

popper 包裹层 `position: fixed; z-index: auto`,settings modal 内容是
z 2001 → 弹层被整个盖住(截图实证:trigger 有 data-state=open 高亮但
看不见面板)。内容元素是 static,z-index 写它身上无效。修法:

```css
body > div:has(> .adp__pop) { z-index: var(--z-over-modal); }
```

### 4. TimeField 的 `hourCycle` 是数字 `12 | 24`

类型声明写成 `HourCycle`(`@internationalized/date` 的字符串 union
"h23" 等)但 reka 实现 `normalizeHourCycle` 只认数字,传字符串被静默
忽略落回 locale 默认。另外 `locale="zh-CN"` 的 24h 格式还会多出一个
**空白 literal 分段**(dayPeriod 槽位残留),需按 `value.trim() === ""`
过滤(AppTimeField 的 `visibleSegments`)。分段键入在 vitest+jsdom 可
直接 `trigger("keydown", { key: "9" })` 驱动。

## Pattern: roving tabindex keyboard nav inside a Dialog(2026-09-03,task `09-03-dirbrowser-desktop-unify`)

**Problem**:DirBrowserModal 这类「行列表 + 路径输入框」的 Dialog 要补方向键
导航,但不能绕开 reka-ui Dialog 的焦点陷阱(Esc 关窗、Tab 循环),也不能让
方向键在输入框聚焦时被列表劫持。

**Solution**(三根支柱,全部落在组件内,零全局监听):

1. **roving tabindex**:列表所有行(`..` + entries)中恰一行 `tabindex="0"`
   (选中行锚 `activeIndex`),其余 `tabindex="-1"`。数学上互斥——`..` 行
   `activeIndex===0` 与 entry 行 `(parent?1:0)+i` 不可能同时为 0。
2. **keydown 挂列表容器**(`.dir-browser__list`),不是 document/window:
   `ArrowDown/ArrowUp` 用 `Math.min/max` 钳边移动 focus(不环绕),
   `preventDefault()` 只在这两个键上;**Enter 不写 JS handler**——focus 落在
   原生 `<button>` 上,浏览器自己激活 click。路径输入框在容器之外,天然不
   被劫持(方向键留在输入框内移动光标)。
3. **导航发起源区分焦点策略**:`navigate(path, { fromList })`——行点击/`..`
   行(列表发起)完成后 `nextTick` 把 focus 复位到新列表首行(失败保留旧
   列表时锚回首行供重试);「前往」/底部「上一步」/隐藏开关发起的不抢焦点。
   `focusActiveRow` 用 `:not(:disabled)` 选择器避开 busy 行。

**Why**:reka-ui 的焦点陷阱管理 Tab/Esc;在容器级挂 keydown 与之正交,不
抢占。Enter 走原生激活意味着 jsdom(vitest)测不了完整激活链路——断言策略
见 `test-environment.md` §9。

**Related**:`DirBrowserModal.vue` 头注释;e2e
`app/e2e/projects-add-dirbrowser.spec.ts`(真 Chromium 锁 Enter 原生链路)。
