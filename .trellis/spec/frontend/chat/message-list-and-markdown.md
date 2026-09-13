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

---

## 4. pending interaction 强制回底契约(BUGLIST CH8-2a,2026-08-29)

MessageList watch `questionCardsStore.getPending(currentSessionId)`,**仅 null → some
跃迁**时强制回底(`isAtBottom = true` + 瞬时 scrollToBottom):

- 触发面覆盖全部 pending 种类(question / loop_intervention / turn_limit_softcap /
  mode_change / task_state_transition)—— 都是"agent 停下来等人"的阻塞态,值得
  打断用户滚动位置;
- some → some 不触发(切到本就有 pending 的 session 时,重载路径
  `scrollAfterReload` 本就回底,重复只添抖动);
- 配套:chatSendActions `send()` 在排队路径(queueingClassic)且当前 session 有
  pending 时 warn toast 澄清"消息已排队但 Agent 在等卡片提交"(CH8-2b)——
  mock `get_pending_interaction` 的测试坑见 `../test-environment.md` §8。

---

## 5. 图片路径预览链路(2026-09-13)

聊天 markdown 里的本地图片路径可点击 → 应用内弹层查看。全链四段,**改任一段
先 grep `md-image-path` 找全消费方**:

1. **识别**(`utils/markdown.ts` `renderMarkdown` 管线,downgrade 与 sanitize
   之间插 `linkifyImagePaths` DOM 后处理):三种形态统一转
   `<a class="md-image-path" data-image-path="原始路径">` —— 正文裸路径、
   inline `<code>` 内路径(LLM 习惯写反引号里)、markdown 图片/链接语法
   (`![](本地)` 由 `downgradeExternalImages` 本地分支产出、`[x](本地)` 由
   linkify 给已有 `<a href>` 补 data 属性)。**围栏代码块(pre 祖先)与 `<a>`
   内文本不动**。实现必须走 DOMParser + TreeWalker(字符串后处理分不清代码
   上下文;marked extension 优先于 codespan tokenizer 会吞 inline code)。
   路径正则 `IMAGE_PATH_RE`(边界捕获组 + Unicode 段字符,同
   chatInputTokens.ts FILE_RE 风格):纯文件名(无 `/`)与 http URL 有意
   不识别;`/api/` 前缀排除(附件路由 uuid.png 会误吞)。
2. **取数**(`utils/imageUrl.ts` + daemon `GET /api/v1/files/image?path=`):
   `resolveImagePath(raw, cwd)` 把相对路径按**点击那一刻**的
   `chatStore.currentCwd` 解析(渲染时刻 cwd 会随会话切换漂移,故解析
   推迟到弹层 computed);`/`、`~/` 原样(daemon 端展开 home)。URL 三传输
   模式与 attachmentUrl 同构(pwa-remote 走 proxy + `?access_token=`)。
   daemon 侧契约:扩展白名单(png/jpg/jpeg/gif/webp/bmp/avif/ico,**svg
   排除**——独立文档打开时脚本会跑)+ 32 MiB 上限 + 只收绝对/`~/` 路径,
   400/404/413 分类(`commands/files.rs` `read_image_at_inner`)。
3. **点击委托**(`composables/useCodeBlockCopy.ts` `onMarkdownClick`):
   `closest("a[data-image-path]")` → preventDefault → `useImageViewer().open(原始路径)`。
   该 composable 是 markdown v-html 容器的统一委托层(代码复制 + 图片预览),
   新交互往这里加分支,容器只需根上绑 `@click="onMarkdownClick"`——**新增
   markdown 容器忘了绑 = 交互静默失效**(MessageItem 主气泡曾是唯一漏绑面,
   09-13 补上;现有绑定面:MessageItem 气泡/时间轴/摘要行、
   DiscussionSummaryCard、SubagentDrawer、MarkdownDetailModal)。
4. **弹层**(`composables/useImageViewer.ts` 模块级单例 + `components/common/
   ImageViewerModal.vue` 全局唯一实例挂 App.vue):reka-ui Dialog 六件套
   (MarkdownDetailModal 模式),img onerror 统一错误态(daemon 400/404/413
   都长这样),「新标签打开」兜底走同一 `imageUrl`。缩放/平移(09-13 同日
   增强):数学核心在 `composables/useImagePanZoom.ts`(锚点公式
   `t' = a − (s'/s)·(a − t)`、clamp [1,8]、回 1 清平移——**坐标模型与
   ImageViewerModal 的 stage CSS 成对**(img 绝对居中 + transform-origin:
   center,改一处必须同步另一处);舞台 overflow:hidden,pan/zoom 全由
   img transform 承载不走滚动条);交互 = wheel(光标锚点,`.prevent`)、
   pointer 拖拽(canPan 门控 + setPointerCapture,jsdom 缺位 try/catch
   降级)、双击 2.5×↔复位、header −/倍率/+/复位 控件;换图(src watch)
   复位缩放态。触摸 pinch 有意不做(见 composable 尾注)。

**样式边界**:`.md-image-path` 的交互态样式(cursor:pointer——无 href 的 a
UA 不给指针;word-break:break-all——长绝对路径防撑爆气泡)放**全局
style.css**;颜色/下划线等排版仍由各容器的 `:deep(a)` 承载(不进 §2 的
五处镜像块——那是排版节奏,这是横切交互态)。
