# Design — 统一 modal 动画节奏与退出动画

> 纯 CSS 调参任务，无数据流/架构变更。本文只记关键决策点与两类实现的对应改法。

## 最终决策（2026-07-02，覆盖下方 D1/D2/D3 的迭代过程）

实机多轮迭代后，最终落地参数与下方 D1 初版（复用 `--ease-decelerate`、scale 0.96）不同：

1. **mask 不动画**（用户反馈"mask 不要加动画"）：A 类 overlay 不写 animation；B 类 backdrop 仅保留 `transition-duration` 作 Vue leave 计时（opacity 始终 1）。下方 D2/D3 未涉及此点（迭代新增）。
2. **缓动统一 `ease`**（用户反馈"曲线都改成平滑"）：`--ease-modal-in` 与 `--ease-accelerate` 都 = `cubic-bezier(0.25, 0.1, 0.25, 1)`，不再是 D1 的 emphasized decelerate / accelerate。
3. **scale `0.1↔1`**（用户反馈"进入/退出都 scale 1→0.1"）：进入 from `scale(0.1)`、退出 to `scale(0.1)`，不再是 D2/D3 的 0.96。

下方 D1/D2/D3 保留作迭代过程记录。

## 决策

### D1. 时长与缓动 token 化（新增 3 个 token + 复用现有 `--ease-decelerate`，集中在 `style.css` motion 区）

现状 `--duration-*` ladder（80/100/150/240ms）无 200ms 档位。进入缓动**复用现有 `--ease-decelerate`（`style.css:151` = `cubic-bezier(0, 0, 0.2, 1)`）**——它已是"内容浮现进入"语义（subagent drawer / toast 同款），`(0,0,0.2,1)` 起手平缓→中段加速→末段缓，比 expo-out 更"跟手"，正是本任务要解决的痛点。退出缓动无现成 token，新增 `--ease-accelerate`。时长新增 modal 专用档位：

```css
/* motion 区，紧邻现有 --duration-* / --ease-*。
   进入复用现有 --ease-decelerate (0,0,0.2,1) — drawer/toast 同款，同属
   "内容浮现进入"语义；只新增退出用的 --ease-accelerate。 */
--duration-modal-in: 200ms;
--duration-modal-out: 150ms;   /* 与 --duration-base 同值，但语义独立，便于将来单独调 modal 退出 */
--ease-accelerate: cubic-bezier(0.4, 0, 1, 1);   /* modal/overlay 退出：emphasized accelerate */
```

`--duration-modal-out` 故意不直接复用 `--duration-base`：语义独立后，将来若要把 modal 退出调成 120ms 而不影响其他用 `--duration-base` 的组件（hover/按钮等），改一处即可。

### D2. A 类（reka-ui `animation`）— 5 个 modal

每个 modal 4 处 shorthand 替换时长+缓动（keyframe 内容不变）：

| 选择器 | 现在 | 目标 |
|---|---|---|
| `__overlay`（enter） | `animation: {n}-fade 150ms var(--ease-out)` | `…var(--duration-modal-in) var(--ease-decelerate) both` |
| `__overlay[data-state=closed]`（exit） | `…{n}-fade-out 100ms ease-in forwards` | `…var(--duration-modal-out) var(--ease-accelerate) forwards` |
| `.modal`（enter） | `animation: {n}-zoom 150ms var(--ease-out)` | `…var(--duration-modal-in) var(--ease-decelerate) both` |
| `.modal[data-state=closed]`（exit） | `…{n}-zoom-out 100ms ease-in forwards` | `…var(--duration-modal-out) var(--ease-accelerate) forwards` |

**配套修复 — enter 加 `both` fill-mode**：A 类 enter 现在用默认 `fill-mode: none`，理论上元素 mount 第一帧会以 base style（opacity:1）闪现，再跳回 keyframe `from`（opacity:0）重播。150ms 时不易察觉，**延长到 200ms 后这个首帧闪现会更容易被看到**。加 `both`（= backwards + forwards）让 from 状态在 mount 首帧立即生效，彻底消除闪现。这是延长时长必要的配套，非额外范围。

**`PermissionGrantsModal` 额外补 exit**（修 bug）：当前只有 `{n}-fade` / `{n}-zoom`（enter），需新增：
```css
.grant-modal__overlay[data-state="closed"] {
  animation: grant-modal-fade-out var(--duration-modal-out) var(--ease-accelerate) forwards;
}
.grant-modal[data-state="closed"] {
  animation: grant-modal-zoom-out var(--duration-modal-out) var(--ease-accelerate) forwards;
}
@keyframes grant-modal-fade-out { from { opacity: 1; } to { opacity: 0; } }
@keyframes grant-modal-zoom-out {
  from { opacity: 1; transform: translate(-50%, -50%) scale(1); }
  to   { opacity: 0; transform: translate(-50%, -50%) scale(0.96); }
}
```
不补时，`usePresence` 检测到 close 时 `animation-name` 未变（`isAnimating=false`），直接 `UNMOUNT` → 硬切。

### D3. B 类（Vue `<Transition>` + `transition`）— 3 个 modal

替换 enter-active 的 transition shorthand 时长+缓动，leave-active 的 longhand override 同步：

| 选择器 | 现在 | 目标 |
|---|---|---|
| `__enter-active` / `__enter-active .modal` | `transition: … var(--duration-base) var(--ease-out)` | `… var(--duration-modal-in) var(--ease-decelerate)` |
| `__leave-active` / `__leave-active .modal` override | `transition-duration:100ms; transition-timing-function:ease-in` | `transition-duration: var(--duration-modal-out); transition-timing-function: var(--ease-accelerate)` |

B 类用 `transition` + Vue Transition class，enter-from 在插入首帧就生效，**无 fill-mode 问题**，不需要 D2 的 `both` 配套。

## 风险与回滚

- **风险**：极低。纯 CSS 值替换，无逻辑/结构变更。最大潜在问题是 WebKitGTK 下 200ms + backdrop-filter 是否引入新闪烁，但 blur 值未动、opacity 范围未动，理论上不新增渲染压力。
- **回滚**：纯 token 值改回（200→150、新增缓动 token 删除、PermissionGrants 补的 keyframe 删除），git revert 单 PR 即可。
- **`prefers-reduced-motion`**：`style.css:179` 通配 `*` 降级到 0.01ms 仍覆盖所有 modal，无需改动，验证时确认。

## 不做（候选债）

- 8 个 modal 抽象为共享 `<BaseModal>` 组件 / 共享动画 CSS module — 本次只对齐参数，结构统一留后续。
- A 类 5 套近乎相同的 keyframe（`{n}-fade` / `{n}-zoom` 名不同行为一致）合并为全局 `modal-fade` / `modal-zoom` — 同上，留后续重构。
