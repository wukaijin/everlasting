# MessageList Rendering & Markdown Styling

> 2026-08-14 从 chat.md doc-split 新增:消息列表动画与 markdown 容器样式的两类
> 易碎点(均来自 08-14-frontend-ux-polish-r1 实战:两个静默失效 bug)。

---

## 1. TransitionGroup enter 动画的"直接子节点"契约

**契约**:`<TransitionGroup>` 的 enter/appear/leave 类落在**真实直接子节点**上。
自 5b1fc81(07-30 run 分组重构)起直接子节点是 run-group `<li>`,
不再是 `.msg--user` / `.msg--assistant` 消息元素本身。

**Common Mistake(2026-08-14 修复,静默失效两周+)**:

- **Symptom**:新消息无划入动画、切会话列表无动画;无报错、测试全绿。
- **Cause**:重构把 TransitionGroup 直接子节点从消息元素换成 run-group li,
  但旧选择器 `.msg--user.msg-enter-from` 要求方向类与 enter 类在**同一元素**,
  于是 enter 类落在 li 上、方向类在子元素上,选择器永不命中。
- **Fix**:from 态重定向到 run-group li(整 run 划入,方向词汇沿用)。
- **Prevention**:任何动 MessageList 结构的改动,先 grep `msg-enter` /
  `run-group` 选择器确认 enter 类落点与选择器目标同元素;devtools 里
  给新消息打断点看 class 列表里是否真出现 `*-enter-from`。

```css
/* Wrong: enter 类在 run-group li 上,方向类在 .msg 子元素上 → 永不命中 */
.msg--user.msg-enter-from { ... }

/* Correct: 直接子节点(run-group li)承载 enter 类 */
:deep(.run-group.msg-enter-from .msg--user) { opacity: 0; transform: translateX(24px); }
```

**相关**:容器级 key fade 不做(与 run 级 enter 双重动画,且 out-in 延迟
重挂载与 `stickToBottomUntilStable` 滚动锚定时序耦合)——决策记录在
08-14-frontend-ux-polish-r1/implement.md 4.2。

---

## 2. 跨组件复用类名 ≠ 继承 scoped 样式

**Gotcha**:scoped `<style>` 的 `:deep()` 只在本组件的 scope id 下生效。
另一个组件复用同一类名(如 `MessageItem` 的 `.msg__markdown` 被 DiscussionSummaryCard
复用渲染 v-html)**不会**继承 MessageItem 的段落节奏——preflight 把 margin
清零后,复用方的正文就是零间距(2026-08-14 前的 DiscussionSummaryCard 实况)。

**Convention**:复用 `.msg__markdown` 渲染 markdown 的容器,必须镜像以下
节奏块(或将其提升到全局 style.css——目前选择镜像,避免全局类泄漏):

```css
/* 镜像块(MessageItem.vue / DiscussionSummaryCard.vue /
   MarkdownDetailModal.vue / SubagentDrawer reply 区 /
   MemoryLayerItem,五处同步) */
:deep(p)      { margin: var(--space-3) 0; }   /* 12px */
:deep(li)     { margin: var(--space-1) 0; }   /* 4px */
:deep(ul, ol) { margin: var(--space-1) 0 var(--space-3); }  /* 4/12 */
:deep(h1..h4) { margin: var(--space-4) 0 var(--space-1); }  /* 16/4 */
line-height: var(--leading-relaxed);           /* 1.6,长文容器统一 */
/* list marker 必须显式补回(BUGLIST CH4-2,2026-08-29):Tailwind
   v4 preflight 全局 `ul, ol { list-style: none }`,只镜像 margin/
   padding 的话列表符号直接消失(截图实证)。 */
:deep(ul)     { list-style: disc; }
:deep(ol)     { list-style: decimal; }
```

改节奏时五处一起改(grep `.msg__markdown` 找全消费方)。

---

## 3. 排版相关决策

- **行高**:长文容器统一 `--leading-relaxed`(1.6);代码块 pre 1.45、
  chip/角标 1.4、mono 数据块 1.4 是允许的例外。VLM 视觉评审反复把 1.6
  正文判为"行高过低"——该反馈应转译为**段落节奏**问题排查,不要直接调
  line-height(08-14 两轮评审实证)。
- **盘古之白**:`text-autospace: ideograph-alpha` 已加在 markdown 容器
  (2026-08-14)。Chromium ≥140 原生支持,WebKit/Firefox 忽略声明 =
  零风险渐进增强。`text-spacing-trim` 会改变标点宽度,不启用。
