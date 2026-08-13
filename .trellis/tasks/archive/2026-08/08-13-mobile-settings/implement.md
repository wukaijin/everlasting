# S6b Implement — Settings 面板移动端适配

> 对应 prd + design。改动面集中在 `app/src/components/settings/`,桌面基线零改动。

## 实施清单(有序)

### 1. SettingsModal tab 导航(解 B1/B2/B3)

- [ ] `.settings-modal__tabs` 移动端:`overflow-x: auto` + `scrollbar-width: none` + `-webkit-overflow-scrolling: touch` + 右侧 `mask-image` 渐变提示
- [ ] `.settings-modal__tab` 移动端:`flex-shrink: 0` + `white-space: nowrap` + 去掉下划线(border-bottom-width: 0)
- [ ] `.settings-modal__tab[data-state="active"]` 移动端:背景 pill(`--color-accent-muted`)+ `--color-accent` 文字 + `border-radius: var(--radius-md)`

### 2. 关闭按钮语义化(解 B4)

- [ ] 模板:`<DialogClose>` 按钮内加 `<span class="settings-modal__close-label">Done</span>`(icon + 文字)
- [ ] CSS:label 桌面 `display: none`,移动端 `display: inline`

### 3. Providers 卡片(解 B6/B7/B8)

- [ ] `.providers-tab__row` 移动端 `flex-wrap: wrap`
- [ ] `.providers-tab__row-info` 移动端 `flex-wrap: wrap` + `row-gap: 4px`
- [ ] `.providers-tab__name` 移动端 `white-space: nowrap` + ellipsis(整词不拆)
- [ ] `.providers-tab__url` 移动端 `white-space: normal` + `overflow-wrap: anywhere`(完整可读)
- [ ] key-hint 文字包 `<span class="providers-tab__key-hint-text">`,移动端隐藏文字留图标

### 4. Default 模型条目(解 B9/B10/B11)

- [ ] `.default-tab__option-info` 移动端 `flex-wrap: wrap`
- [ ] `.default-tab__option-name` 移动端 `white-space: nowrap` + ellipsis
- [ ] `.default-tab__group-header` 移动端 `font-size: var(--text-sm)` + `color: var(--color-text-secondary)`
- [ ] `.default-tab__option--selected` 移动端 `box-shadow: inset 3px 0 0 var(--color-accent)`

### 5. 其他 tab 溢出检查

- [ ] Models/Memory/Subagents/Remote 四宽度冒烟:若卡片长文本溢出,轻量补 `max-width`/ellipsis(不改布局)

## 验证命令

```bash
cd app
pnpm test            # Vitest 全量(settings 有组件测试)
pnpm build           # vue-tsc + vite
```

## 验证检查单(dev server + DevTools 移动模拟)

- [ ] 320 / 360 / 393 / 430px:Settings 打开无 body 横向滚动
- [ ] 6 个 tab 可左右滑到 Subagents/Remote,边缘渐隐提示存在
- [ ] 当前 tab 背景 pill 高亮明显(非下划线)
- [ ] 关闭按钮显示 "Done"(icon + 文字),点击关闭
- [ ] Provider 卡片:name 不拆行、url 完整可读、key-hint 只剩图标
- [ ] Default 模型条目:name/id/tag 分层不挤,分组标题可读,选中项左侧 accent 条
- [ ] 桌面(≥768px)Settings 与改动前逐项对比:tab 平铺不变、关闭只 icon、卡片单行不变

## 风险回滚点

- 若 `mask-image` 导致最右 tab 文字看不清 → 去掉 mask,保留 overflow-x(功能完整,仅少提示)
- 若 tab pill 与 44px 全局规则冲突 → 回退仅保下划线,优先可滚动

## 收尾

- [ ] `pnpm test` + `pnpm build` 全绿
- [ ] 桌面回归对比(零变化)
- [ ] 更新 spec:`frontend/reka-ui-usage.md` 或 `responsive-mobile.md` 补 settings tab 导航约定(若有新约定)
