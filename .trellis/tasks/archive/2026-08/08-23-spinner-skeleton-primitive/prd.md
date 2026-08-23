# Spinner/Skeleton 共享原语:统一 10 处自写 spin 与 2 套 shimmer

## Goal

把散落在 10 个组件里的自写 spin keyframes 与 2 套各自命名的 skeleton shimmer,
收敛为 style.css 里的 canonical 原语(单一 keyframes + 标准类),消灭"每处一个
keyframe 名、周期 0.6~1.4s 各拍脑袋"的漂移;顺带清掉两处已确认的死代码 spinner。

## 现状清点(2026-08-23 grep 实证)

### A. 环形 border spinner ×6(活代码)

| 文件 | 类名 | 尺寸/边框 | 轨道+高亮 | 周期 |
|---|---|---|---|---|
| SearchModal.vue | `.search-modal__spinner` | 14px / 2px | bg-border + accent top | 0.8s |
| SubagentsTab.vue | `.subagents-tab__spinner` | 14px / 2px | bg-border + accent top | 0.8s |
| WorkerTurnTraceList.vue | `.worker-turn-trace__spinner` | 10px / 2px | bg-border + accent top | 0.8s |
| WorkerMergeControls.vue | `.worker-merge-controls__spinner` | 12px / 1.5px | currentColor,顶段 transparent | 0.6s |
| DrawerSection.vue | `.drawer-section__live-spinner` | 8px / 1.5px | currentColor,顶段 transparent | 0.8s |
| ChatPanel.vue | `.chat-panel__spinner`(+`.chat-panel__loading`+`@keyframes spin`) | 20px / 2px | bg-border + accent top | 0.6s |

两种合法视觉变体:**track 型**(灰轨 + accent 高亮弧,独立加载面)与
**inline 型**(currentColor 单色弧,嵌在按钮/badge 内不加灰轨)。

### B. 图标自转 ×3(活代码)

| 文件 | 载体 | 周期 |
|---|---|---|
| MessageItem.vue | `Icon name="refresh"` 经 icon-class 挂到 svg,重试中 | 1.4s |
| ChecklistCard.vue | marker span 内 `:deep(svg)`,需 `transform-box: fill-box` 防偏心摆动 | 1.0s |
| SubagentDrawer.vue | 32px 圆底容器包 `Icon name="loader"`,空态 | 1.2s |

### C. Skeleton shimmer ×2(同一手法、两套命名)

- ChatPanel.vue `@keyframes skeleton-shimmer`:elevated→border-strong→elevated,1.5s
- TurnTimeline.vue `@keyframes skeleton-pulse`(实为同款 background-position
  shimmer,名字误导):surface→elevated→surface,1.4s

### D. 死代码

- ChatPanel.vue `.chat-panel__loading/.chat-panel__spinner/@keyframes spin`
  ——注释自述 v-if 已移除、"remove in a follow-up if no test references"
- ChatInput.vue `.chat-input__spinner/@keyframes chat-input-spin`
  ——样式块存在但模板零引用(grep 实证)

测试侧:grep *.test.ts 仅注释提及 spinner(行为断言),无类名/keyframes 锁定。

## Requirements

- **R1 原语落地(style.css)**:
  - `@keyframes app-spin`(唯一旋转 keyframes)+ `@keyframes skeleton-shimmer`(唯一 shimmer);
  - `.app-spinner` 基类(track 型默认:14px/2px,bg-border 轨 + accent 弧,0.8s linear)
    + 尺寸修饰 `--xs`(10px/1.5px)/`--sm`(12px/1.5px)/`--lg`(20px/2px)
    + 变体 `--inline`(currentColor 单色弧、顶段 transparent);
  - `.icon-spin`(图标/svg 自转,1s linear);
  - 注释注明 prefers-reduced-motion 全局折叠已有(style.css PR-1 media block),原语不重复处理。
- **R2 迁移 6 处环形 spinner**:本地 keyframes 全删,模板挂 `.app-spinner[--xs/--sm/--lg][--inline]`,
  组件 scoped 样式只保留定位性声明(flex-shrink/margin 等);周期统一 0.8s。
- **R3 迁移 3 处图标自转**:引用共享 `app-spin`;ChecklistCard 保留 fill-box 几何修正
  (那是几何约束不是动画定义);MessageItem/SubagentDrawer 改用 `.icon-spin` 或等价引用。
- **R4 skeleton 统一**:TurnTimeline 删 `skeleton-pulse`,并入共享 `skeleton-shimmer`;
  渐变色阶是否统一到 elevated→border-strong 以截图对比度为准(surface 底面板上若
  border-strong 对比过强则允许保留自己的 stops,只共享 keyframes——记录裁决理由)。
- **R5 死代码清除**:确认无测试/模板引用后删 ChatPanel 三件套与 ChatInput 两件套。

## Acceptance Criteria

- [ ] `grep -rn "@keyframes" app/src --include="*.vue"` 结果中不含任何 `spin` 命名
      (app-spin/skeleton-shimmer 只存在于 style.css);
- [ ] `.app-spinner` 在 dist 构建产物中可见且活站点 computed animation-name=`app-spin`、
      duration=0.8s(运行时探针取证,至少覆盖 SearchModal/SubagentsTab 两处可达面);
- [ ] 死代码删除后 pnpm test 全绿(app 目录,vitest)+ vue-tsc 零错;
- [ ] 视觉抽查:迁移前后 spinner 渲染形态不变(尺寸/颜色逐项对照上表),
      用 ui-review scratch 截图或 DOM 探针双证;
- [ ] TurnTimeline skeleton 仍渲染且动画名为 `skeleton-shimmer`。

## Notes

- 命名沿用 style.css 现有共享层惯例(button/input reset 同区);不引入新 Vue 组件——
  10 个站点全是 scoped BEM 局部定位 + 共享形态,CSS 类是最小 diff 方案,组件化收益不足。
- 周期治理原则:环统一 0.8s(现网多数派),图标自转统一 1s;不再保留 per-site 时长。
