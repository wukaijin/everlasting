# 统一 modal 动画节奏与退出动画

## 背景

排查发现 app 内所有 modal 共用一套动画参数（`DiffModal.vue:173` 注释明确写为「modal 动画 convention」），但该 convention 的**进入缓动过于激进 + 时长偏短**，导致整体节奏「不自然」（用户反馈：弹射到位 → 急停 → 蹭完）。同时 `PermissionGrantsModal` 因缺失退出 keyframes，关闭时硬切、无淡出（确凿 bug）。

现状参数（全 app modal 一致）：

| | 进入 | 退出 |
|---|---|---|
| 时长 | `--duration-base` = 150ms | `--duration-fast` = 100ms |
| 缓动 | `--ease-out` = `cubic-bezier(0.16, 1, 0.3, 1)`（expo-out） | `ease-in` |
| scale | 0.96 → 1 | 1 → 0.96 |

## 目标

把进入/退出的**时长与缓动**调成更自然的组合，全 app modal 统一，并补齐 `PermissionGrantsModal` 退出动画。纯 CSS 调参 + 少量 keyframe 补全 + 可能新增 motion token，低风险。

## 最终交付（2026-07-02，覆盖下方初版规划）

经多轮实机迭代，最终参数（与下方"目标参数"初版不同，以本节为准）：

| 维度 | 最终值 |
|---|---|
| mask（overlay/backdrop） | **无动画**，瞬间出现/消失（A 类不写 overlay animation；B 类 backdrop 仅保留 transition-duration 作 Vue 计时，opacity 始终 1） |
| content 进入 | `scale 0.1→1` + opacity，`var(--duration-modal-in)` 200ms + `var(--ease-modal-in)` |
| content 退出 | `scale 1→0.1` + opacity，`var(--duration-modal-out)` 150ms + `var(--ease-accelerate)` |
| 缓动曲线 | 进入/退出统一 `ease` = `cubic-bezier(0.25, 0.1, 0.25, 1)`（`--ease-modal-in` 与 `--ease-accelerate` 同值，保留两个 token 便于将来分开调） |

新增 token（`style.css`）：`--duration-modal-in: 200ms`、`--duration-modal-out: 150ms`、`--ease-modal-in`、`--ease-accelerate`（后两个 = ease）。`PermissionGrantsModal` 退出动画 bug 顺带修复（补 zoom-out keyframe + `[data-state="closed"]`）。

## 目标参数

| | 现在 | 目标 | 理由 |
|---|---|---|---|
| 进入时长 | 150ms | **200ms** | 大 modal 需给眼睛聚焦时间，150ms 偏仓促 |
| 进入缓动 | expo-out `(0.16,1,.3,1)` | **复用 `--ease-decelerate` `(0,0,0.2,1)`** | 起手平缓→中段加速→末段缓，比 expo-out 更"跟手"（token 已存在，drawer/toast 同款） |
| 退出时长 | 100ms | **150ms** | 让 scale 收缩看得见，不再「啪」地没 |
| 退出缓动 | `ease-in` | **accelerate `(0.4,0,1,1)`** | 与进入对称的物理感 |
| scale 幅度 | 0.96 → 1 | **不变** | 幅度没问题，问题在缓动/时长 |

进入复用现有 `--ease-decelerate`（`(0,0,0.2,1)`，Material standard decelerate，drawer/toast 同款）；退出新增 `--ease-accelerate`（`(0.4,0,1,1)`，emphasized accelerate）。

## Requirements

1. **进入动画**：8 个 modal 的进入统一为 200ms + 复用现有 `--ease-decelerate`（`(0,0,0.2,1)`）。
2. **退出动画**：8 个 modal 的退出统一为 150ms + accelerate `(0.4, 0, 1, 1)`。
3. **补 `PermissionGrantsModal` 退出动画**：新增 `grant-modal-fade-out` / `grant-modal-zoom-out` keyframes + `[data-state="closed"]` 规则，使其余 4 个 A 类 modal 一致（修 bug）。
4. **scale 幅度**保持 0.96 → 1（进入）/ 1 → 0.96（退出），不动。
5. **两套实现统一参数**：A 类（reka-ui `animation`，5 个）与 B 类（Vue `<Transition>` + `transition`，3 个）落到同一组时长/缓动。
6. **token 化**：进入复用现有 `--ease-decelerate`，退出引用新增 `--ease-accelerate`；进入/退出时长新增 `--duration-modal-in/out`。避免 `cubic-bezier` 字面值散落在 8 个组件里。
7. **不动**：颜色、尺寸、z-index 体系、reka-ui 组件结构、TS 逻辑、scale 幅度、`backdrop-filter` 值。

## 涉及文件

**A 类（reka-ui Dialog + CSS `animation`）— 5 个**：
- `app/src/components/settings/SettingsModal.vue`（`:73/:77` overlay, `:98/:102` content, `:105-123` keyframes）
- `app/src/components/memory/MemoryModal.vue`（`:80/:84`, `:112/:116`, `:119-137`）
- `app/src/components/audit/AuditLogModal.vue`（`:322/:326`, `:361/:365`, `:368-386`）
- `app/src/components/permissions/PermissionGrantsModal.vue`（`:146-185`，**缺 exit，需补全**）
- `app/src/components/common/MarkdownDetailModal.vue`（`:200/:204`, `:233/:237`, `:240-258`）

**B 类（Vue `<Transition>` + CSS `transition`）— 3 个**：
- `app/src/components/common/ConfirmDialog.vue`（`:234-259`）
- `app/src/components/chat/YoloConfirmModal.vue`（`:345-370`）
- `app/src/components/chat/DiffModal.vue`（`:173-200`）

**Token**：`app/src/style.css`（motion token 区，`:138-150` 附近）

## Acceptance Criteria

- [ ] 8 个 modal 打开动画均为 200ms + standard decel，观感「平稳浮现、无急停/弹射感」
- [ ] 8 个 modal 关闭动画均为 150ms + accelerate，淡出 + 微缩放清晰可见
- [ ] `PermissionGrantsModal` 关闭时有完整退出动画（淡出 + scale 收缩），不再硬切消失
- [ ] A 类（reka-ui）与 B 类（Vue Transition）节奏一致，开关体验统一
- [ ] 进入复用 `--ease-decelerate`、退出用新增 `--ease-accelerate`，时长用 `--duration-modal-in/out`；modal 组件内无散落 `cubic-bezier` 字面值
- [ ] `prefers-reduced-motion` 全局降级（`style.css:179` 将 transition/animation-duration 降到 0.01ms）仍生效
- [ ] `pnpm build`（含 `vue-tsc --noEmit`）通过；`vitest` 全绿（不引入新失败）
- [ ] WSLg/WebKitGTK 下肉眼验证无 backdrop-filter 闪烁回退

## Constraints

- 纯前端 CSS 改动，不动 TS 逻辑、不动 reka-ui 组件结构（不换 `forceMount`、不重构为单一 base 组件）
- 保持现有 z-index 体系（1100 / 1200 / 2000 / 2001 / 3000）
- 保持 reka-ui `Presence` 驱动的退出机制（`usePresence` 靠 `animation-name` 切换检测 exit）
- 不引入新依赖
- 改动需同时覆盖 A、B 两类，不能只改一类导致新的不一致

## Out of Scope

- 不改 backdrop-filter 的 blur 值 / 不改 overlay 透明度（`color-mix 70%`）
- 不重构 8 个 modal 为共享基础组件（本次只对齐动画参数；结构统一是后续候选债）
- 不调整 TriggerMenu / WorktreeChip / HiddenProjectsMenu 等非 modal 的弹出层动画（它们是 popover/dropdown，节奏可以不同）
