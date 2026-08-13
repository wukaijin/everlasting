# S6b Design — Settings 面板移动端适配

> 对应 prd:`./prd.md`。本文件只写技术设计:改动边界、关键决策、兼容性。

## 1. 架构与边界

**改动集中在 `app/src/components/settings/`**,全部是"桌面基线 + `@media (max-width: 767px)` 覆盖",不编辑桌面样式块(结构保证桌面零回归)。模板改动仅 1 处(关闭按钮加文字 label,桌面隐藏)。

| 文件 | 改动 |
|---|---|
| `SettingsModal.vue` | tab 行横向可滚动 + 边缘渐变提示;当前 tab pill 高亮(替换下划线);关闭按钮移动端显示 "Done" 文字 |
| `ProvidersTab.vue` | 卡片移动端换行堆叠(name 整词不拆、url 完整可读、key-hint 折叠成仅图标) |
| `DefaultTab.vue` | 模型条目移动端换行(name/id/tag 分层)、分组标题对比度提升、选中态加粗 |
| `style.css` | 无(44px 全局块已覆盖 settings modal button) |

**已由 S5 全局块解决、S6b 不重复**:
- 容器全屏化(style.css:361-378,`width:100vw` + `height:var(--app-height)`)
- 按钮 44px 触摸目标(style.css:383-394,`.settings-modal button` 命中 TabsTrigger 默认 button)

## 2. 关键设计

### 2.1 tab 导航(prd DEC-4,解 B1/B2/B3)

```css
@media (max-width: 767px) {
  .settings-modal__tabs {
    overflow-x: auto;              /* 6 tab 超宽可滑 */
    scrollbar-width: none;         /* 隐藏滚动条(仍可触控滑) */
    -webkit-overflow-scrolling: touch;
    /* 边缘渐变提示:右侧淡出暗示可继续滑。mask-image 覆盖内容区,
       左侧不 mask,当前区清晰。 */
    -webkit-mask-image: linear-gradient(to right, #000 88%, transparent 100%);
    mask-image: linear-gradient(to right, #000 88%, transparent 100%);
  }
  .settings-modal__tab {
    flex-shrink: 0;                /* 不被压缩成一团 */
    white-space: nowrap;
    /* 当前 tab:下划线 → 背景 pill */
    border-bottom-width: 0;
  }
  .settings-modal__tab[data-state="active"] {
    background: var(--color-accent-muted);
    color: var(--color-accent);
    border-radius: var(--radius-md);
  }
}
```

- **取舍**:`mask-image` 右侧渐隐会让最右 tab 文字边缘淡出,但 88%→100% 的窄渐变只在滚动到底时暗示还有内容;用户滑到最右,整行不 mask(可加 `mask` 两段——本任务保持简单,88% 阈值够用)
- tab 高度:桌面 `padding: 8px 16px` 不动,移动端被 `.settings-modal button` min-height 44px 撑到可点高度(已验证)
- 移动端首屏默认停在 providers(active tab 起始位置),6 个 tab 可左右滑

### 2.2 关闭按钮语义化(解 B4)

模板改动(唯一 1 处):

```vue
<button type="button" class="settings-modal__close" aria-label="Close">
  <Icon name="x" :size="14" />
  <span class="settings-modal__close-label">Done</span>
</button>
```

CSS:`.settings-modal__close-label { display: none }`,移动端 `display: inline`(icon + "Done" 并排,44px 目标已保证)。

- **取舍**:不换 `← 返回`(modal 是覆盖层,「Done」比「返回」更准确——它关闭弹窗不是返回导航)

### 2.3 Providers 卡片(解 B6/B7/B8)

```css
@media (max-width: 767px) {
  .providers-tab__row { flex-wrap: wrap; }        /* 卡片允许两行 */
  .providers-tab__row-info { flex-wrap: wrap; row-gap: 4px; }
  .providers-tab__name { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; max-width: 100%; }  /* B6: 整词不拆 */
  .providers-tab__url { white-space: normal; overflow-wrap: anywhere; max-width: 100%; }  /* B7: URL 完整可读 */
  .providers-tab__key-hint { gap: 2px; }
  .providers-tab__key-hint span + * /* 文字隐藏 */ — 做法:key-hint 模板加 <span class="providers-tab__key-hint-text">,移动端 display:none,只留 key 图标(B8)
}
```

- **B6 根因**:`.providers-tab__name` 无 nowrap,flex 压缩下 `Carlos-Api-OpenAI` 在 `-` 处断行 → 补 `white-space: nowrap` + ellipsis
- **B7**:url 桌面 `white-space: nowrap + ellipsis`(截断 `h...`),移动端改 normal + `overflow-wrap: anywhere`,完整可读
- **B8**:key-hint 文字(`已加密保存`)移动端隐藏,保留 key 图标(状态示意不丢)

### 2.4 Default 模型条目(解 B9/B10/B11)

```css
@media (max-width: 767px) {
  .default-tab__option-info { flex-wrap: wrap; row-gap: 2px; }  /* name/id/tag 分层不挤 */
  .default-tab__option-name { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; max-width: 100%; }
  .default-tab__group-header { font-size: var(--text-sm); color: var(--color-text-secondary); }  /* B10: 对比度提升 */
  .default-tab__option--selected {
    border-color: var(--color-accent);
    box-shadow: inset 3px 0 0 var(--color-accent);  /* B11: 左侧 accent 条,选中一眼可辨 */
  }
}
```

- **B9**:name(大)+ id(小)是设计意图(用户澄清过),移动端只做"允许换行不挤",不隐藏 id
- **B11**:选中态从"圆点 + 边框"加左侧 inset 条,窄屏视觉权重足够

### 2.5 其他 tab(Models/Memory/Subagents/Remote)

**不重排结构**(prd 明确不做),冒烟验证四宽度不溢出;若溢出(卡片内长文本),轻量补 `max-width` + ellipsis,不改布局。

## 3. 兼容性 / 回归

- 桌面样式块零改动 → 桌面零回归
- 模板唯一改动是关闭按钮加 `<span>` label(桌面 `display: none`,不可见)
- 44px 已全局覆盖,不新增 `!important`
- tab 行 `mask-image` 仅移动端启用,桌面 flex 平铺不受影响

## 4. 风险

| 风险 | 缓解 |
|---|---|
| `mask-image` 兼容性(webkit/blink) | 双写 `-webkit-mask-image` + `mask-image`;退化为"无渐变仅可滑"也可用 |
| tab `flex-shrink: 0` 桌面被全局 44px 规则影响 | 44px 规则是 `min-height/min-width`,不改 flex 布局;桌面无影响 |
| url `overflow-wrap: anywhere` 视觉 | 仅移动端;桌面 ellipsis 保留 |
| B8 隐藏 key-hint 文字后 a11y | aria-label 保留(已有 key 图标 + 组件语义) |
