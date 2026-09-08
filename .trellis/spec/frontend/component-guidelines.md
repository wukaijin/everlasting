# Component Guidelines

> How components are built in this project.

---

## Overview

<!--
Document your project's component conventions here.

Questions to answer:
- What component patterns do you use?
- How are props defined?
- How do you handle composition?
- What accessibility standards apply?
-->

(To be filled by the team)

---

## Component Structure

<!-- Standard structure of a component file -->

(To be filled by the team)

---

## Props Conventions

<!-- How props should be defined and typed -->

(To be filled by the team)

---

## Styling Patterns

<!-- How styles are applied (CSS modules, styled-components, Tailwind, etc.) -->

(To be filled by the team)

---

## Accessibility

<!-- A11y requirements and patterns -->

(To be filled by the team)

---

## Common Mistakes

<!-- Component-related mistakes your team has made -->

(To be filled by the team)

## Common Mistakes

### flex 卡片横排被 nowrap 长文本挤成纵排满宽(min-width:auto 陷阱)

**Symptom**(实证 2026-09-08,gce-m4c preset 卡):flex row + wrap + `flex: 1 1 160px`
的三张卡,实测逐张满宽纵排——设计意图三卡同行。

**Cause**:flex item 的 `min-width: auto` = min-content;卡内 `white-space: nowrap` 的
描述行把 min-content 撑到 ~450px,假想主尺寸(hypothetical main size = flex-basis 被
min-width 钳制)超过行宽 → 单卡即换行,grow 因子把它拉满整行。子级文本容器写
`min-width: 0` 救不了——钳制发生在**卡本身**这一层。

**Fix**:

```css
/* Wrong:只给文本容器 min-width:0 */
.preset-card { flex: 1 1 160px; }        /* min-width:auto 仍按内容钳制 */
.preset-text { min-width: 0; }

/* Correct:卡(= flex item)显式归零 */
.preset-card { flex: 1 1 160px; min-width: 0; }
.preset-text { min-width: 0; }
.preset-desc { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
```

**Prevention**:flex/wrap 布局里放 nowrap / 长文本的卡,卡这一层一律 `min-width: 0`。
排查手法:VLM 截图评审会说「纵排」但给不出机制;**DOM 实测才是裁决**——
`getBoundingClientRect()` 对比各卡 x/y,再读 `getComputedStyle` 的
`flexBasis`/`minWidth` 定位钳制层。
