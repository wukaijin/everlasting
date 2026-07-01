# Implement — 统一 modal 动画节奏与退出动画

> 参照 `design.md` 的 D1/D2/D3。纯 CSS，按文件类型分阶段推进。

## 阶段 1：新增 motion token（`app/src/style.css`）

- [ ] 在 motion token 区（`:138-151` 附近，现有 `--ease-decelerate` 之后）新增 3 个 token：
  ```css
  --duration-modal-in: 200ms;
  --duration-modal-out: 150ms;
  --ease-accelerate: cubic-bezier(0.4, 0, 1, 1);
  ```
  - 进入缓动**复用现有** `--ease-decelerate`（`style.css:151`，`(0,0,0.2,1)`），不新增。
  - 注释说明：`--duration-modal-in/out` 为 modal 专用时长档位（语义独立于 `--duration-base`）；`--ease-accelerate` 为 modal 退出专用，与进入的 `--ease-decelerate` 对称。

## 阶段 2：A 类 — reka-ui `animation`（5 个）

每个 modal 改 4 处 shorthand（keyframe 内容不动），enter 加 `both`（见 design D2 配套修复）：

| 选择器 | 目标 |
|---|---|
| `__overlay` | `animation: {n}-fade var(--duration-modal-in) var(--ease-decelerate) both` |
| `__overlay[data-state="closed"]` | `animation: {n}-fade-out var(--duration-modal-out) var(--ease-accelerate) forwards` |
| `.modal` 根 | `animation: {n}-zoom var(--duration-modal-in) var(--ease-decelerate) both` |
| `.modal[data-state="closed"]` | `animation: {n}-zoom-out var(--duration-modal-out) var(--ease-accelerate) forwards` |

- [ ] `app/src/components/settings/SettingsModal.vue`（`:73/:77/:98/:102`）
- [ ] `app/src/components/memory/MemoryModal.vue`（`:80/:84/:112/:116`）
- [ ] `app/src/components/audit/AuditLogModal.vue`（`:322/:326/:361/:365`）
- [ ] `app/src/components/common/MarkdownDetailModal.vue`（`:200/:204/:233/:237`）
- [ ] `app/src/components/permissions/PermissionGrantsModal.vue`（`:146-185`）— **额外补 exit**：
  - 加 `.grant-modal__overlay[data-state="closed"]` 规则 + `.grant-modal[data-state="closed"]` 规则
  - 加 `@keyframes grant-modal-fade-out` + `@keyframes grant-modal-zoom-out`（见 design D2）

**阶段 2 review gate**：5 个文件改完后，`grep -nE "animation:" ` 这 5 个文件，确认每个都有 4 处且 enter 都带 `both`、exit 都带 `forwards`、无残留 `var(--ease-out)` / `100ms` / `ease-in`。

## 阶段 3：B 类 — Vue `<Transition>`（3 个）

- [ ] `app/src/components/common/ConfirmDialog.vue`（`:234-259`）
- [ ] `app/src/components/chat/YoloConfirmModal.vue`（`:345-370`）
- [ ] `app/src/components/chat/DiffModal.vue`（`:173-200`）

改法（见 design D3）：
- `__enter-active` / `__enter-active .modal`：`transition` shorthand 里 `var(--duration-base) var(--ease-out)` → `var(--duration-modal-in) var(--ease-decelerate)`
- `__leave-active` / `__leave-active .modal` override：`transition-duration:100ms` → `var(--duration-modal-out)`；`transition-timing-function:ease-in` → `var(--ease-accelerate)`

**阶段 3 review gate**：3 个文件改完后，`grep -nE "transition|ease-in|100ms|var\(--ease-out\)"` 确认无残留旧值。

## 阶段 4：验证

- [ ] `cd app && pnpm build` — `vue-tsc --noEmit && vite build` 通过
- [ ] `cd app && pnpm test` — `vitest run` 全绿（至少 `YoloConfirmModal.test.ts` / `MarkdownDetailModal.test.ts` 不挂）
- [ ] 散落检查：`grep -rnE "cubic-bezier\(0\.16, 1, 0\.3, 1\)|ease-in\b|100ms" app/src/components/{settings,memory,audit,permissions,common,chat}` 在 modal 动画行无命中（token 定义处除外）
- [ ] `prefers-reduced-motion`：确认 `style.css:179` 的通配降级未被破坏
- [ ] **肉眼验证**（`cd app && pnpm tauri dev`）：逐个开关 8 个 modal —— Settings / Memory / 审计日志 / 权限放行 / Markdown 详情 / 会话删除 Confirm / Yolo 确认 / Diff 详情
  - 进入：200ms 平稳浮现，无「弹射/急停」
  - 退出：150ms 淡出 + 微缩放清晰可见
  - **重点**：权限放行 modal 关闭时**有淡出**（修了 bug），不再硬切
  - 8 个节奏一致

## 回滚点

每阶段独立可回滚。若肉眼验证发现 WebKitGTK 下 200ms 引入新闪烁，单独把 `--duration-modal-in` 改回 150ms 即可全局回退，无需逐文件 revert。

## 提交（按 Trellis 四段式，task.py archive auto-commit）

fix → docs(debt 若有) → archive → journal。纯 UI 调参，DEBT.md 多半无新增。
