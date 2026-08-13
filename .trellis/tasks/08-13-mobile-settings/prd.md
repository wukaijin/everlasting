# S6b Settings 面板移动端适配:tab 导航 / 卡片信息密度 / 选中态

> parent(S6 总任务)见 `../08-13-mobile-polish/prd.md`(痛点 SoT)。本任务细化执行。

## Goal

S5 只做了 Settings 的**容器全屏化**,内部零适配。本任务把 Settings 面板内部(6 个 tab 导航 + 各 tab 卡片列表)做成 320–430px 手机可用:
- 6 个 tab 横向可滚动,当前 tab 高亮明显
- Provider / Model 卡片信息密度降到手机可读
- 选中状态、分组标题在窄屏可辨识

**适配宽度范围:320px – 430px**(对齐 S6 跨子任务验收)。

## 痛点清单(B 组,来自 parent SoT)

### 共通问题(每个 tab 都有)

- `B1` **顶部 tab 全部平铺挤死** —— 6 个 tab 一字排开,字压成一团
- `B2` **缺水平滚动提示** —— 用户不知 Subagents/Remote 在屏外
- `B3` **当前 tab 仅用下划线高亮** —— 视觉反馈太弱
- `B4` **关闭 × 在右上角** —— 弹窗全屏下应语义化(`Done` / `← 返回`)
- `B5` **整体未做移动端适配** —— 桌面 Settings 面板硬塞手机

### Providers tab

- `B6` **Provider 卡片标题断行丑** —— `Carlos-Api-OpenAI` 被拆三行
- `B7` **卡片内信息密度过高** —— 徽章 + URL + 状态 + 操作图标挤一行,URL 被截断
- `B8` **`已加密保存` 系统状态文字占空间**

### Default Model tab

- `B9` **模型条目字号层级混乱** —— name(大)+ id(小)+ `thinking` 徽章紧挨
- `B10` **分组标题 `CARLOS-API-XXX` 对比度低**
- `B11` **选中状态视觉反馈不足** —— 圆点 + 边框变化对比度不够

## 决策记录(已确认)

- `B12` **tab 导航方案:横向可滚动 tab + 背景 pill 高亮** —— tab 行 `overflow-x: auto` + 隐藏滚动条 + 边缘渐变提示可滑动;当前 tab 从下划线改为背景 pill(对比度提升)。iOS 设置/App Store 分类栏的标准移动端模式,改动集中在 `SettingsModal.vue` CSS
- 断点沿用 S5 `@media (max-width: 767px)` 叠加约定;窄屏降级(<360px)对 tab 同样生效

## 验收标准

- [ ] 320px / 360px / 393px / 430px 下 Settings 无横向滚动 / 无元素裁剪
- [ ] 6 个 tab 横向可滚动,边缘渐变提示可滑动,当前 tab 背景 pill 高亮明显
- [ ] 关闭按钮在手机端为语义化(`Done` / `← 返回`),点击返回
- [ ] Provider 卡片标题不拆行(`Carlos-Api-OpenAI` 整词显示);卡片内 URL 完整可读或可点开
- [ ] 模型条目 name + id 两级字号清晰,选中态(圆点/背景)在窄屏可辨识;分组标题对比度提升
- [ ] 桌面(≥768px)Settings 布局回归零变化

## Notes

- **依赖 S6 跨子任务验收**:320–430px 无横向滚动(见 parent prd Acceptance Criteria)
- **CSS 优先** —— 桌面样式是基线,移动端全部放 `@media (max-width: 767px)` 内,不改桌面块,结构保证桌面零回归(spec: `frontend/responsive-mobile.md`)
- 代码位置锚点:
  - SettingsModal: `app/src/components/settings/SettingsModal.vue`(DialogRoot + TabsRoot `:22-78`;6 tab `:36-55`;tab 行 CSS `:166-173`;下划线高亮 `:191-194`;桌面宽 640px `:95`)
  - 6 个 tab 内容: `app/src/components/settings/{ProvidersTab,ModelsTab,DefaultTab,MemoryTab,SubagentsTab,RemoteTab}.vue`
  - Provider 卡片: `ProvidersTab.vue:300+`(卡片内徽章/URL/状态/操作图标挤一行)
  - Model 条目 + 分组标题: `DefaultTab.vue`
- **reka-ui portal 陷阱**:Dialog/Tooltip portal 到 body,scoped style 选不中 → 全局覆盖须写 style.css 或用 `:deep()`,`!important` 只用于 reka-ui 覆盖且集中在 style.css 一处(见 `frontend/reka-ui-usage.md`)
- 测试: `cd app && pnpm test`(Vitest);`pnpm build`(vue-tsc + vite)
- **不做的事**:不重排各 tab 内容结构(那是 V2 紧凑视图),只做信息密度降级与可读性
