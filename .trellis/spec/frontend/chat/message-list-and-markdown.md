# MessageList Rendering & Markdown Styling

> 2026-08-14 从 chat.md doc-split 新增:消息列表动画与 markdown 容器样式的两类
> 易碎点(均来自 08-14-frontend-ux-polish-r1 实战:两个静默失效 bug)。

---

## 1. TransitionGroup enter 动画的"直接子节点"契约

> **N4 状态(09-19-n4-render-virtualization)**:TransitionGroup 已随虚拟化
> **整体退役**——run 划入动画改由 `useVirtualizedMessages` 的 enterRow 相位机
> 挂 `run-enter-from/-active` 类到虚拟项 wrapper、目标 `.msg` 子根
> (opacity+translateX 白名单,禁 scale/height);session 切换/首挂载走
> `.messages` 容器一次性 fade-in。本节保留作**历史决策记录**(enter 类落点
> 必须与选择器同元素的教训仍然成立:新动画机制里 from/active 类同挂
> wrapper、样式选择器 `> .msg` 承接,同元素契约以新形态延续)。
> **不要在新改动里恢复 TransitionGroup**(虚拟项 wrapper 非真实子节点模型)。

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
   MemoryLayerItem / FileViewerModal md 模式,六处同步) */
:deep(p)      { margin: 0 0 var(--space-3) 0; }   /* 12px,顶 0 首段贴容器顶(2026-09-13 校准:六处实现一贯形态) */
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

改节奏时六处一起改(grep `.msg__markdown` 找全消费方)。

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
跃迁**时强制回底:

> **N4 机制注记(09-19-n4-render-virtualization)**:watch 现居
> `composables/useVirtualizedMessages.ts`(集中装配点),回底动作 =
> `isAtBottom = true` + `scrollToEnd({behavior:'auto'})`(程序化滚动必须走
> 库 API,spike 约束 3)。**触发面与「仅 null→some」契约不变**。

- 触发面覆盖全部 pending 种类(question / loop_intervention / turn_limit_softcap /
  mode_change / task_state_transition)—— 都是"agent 停下来等人"的阻塞态,值得
  打断用户滚动位置;
- some → some 不触发(切到本就有 pending 的 session 时,重载路径
  `scrollAfterReload` 本就回底,重复只添抖动);
- 配套:chatSendActions `send()` 在排队路径(queueingClassic)且当前 session 有
  pending 时 warn toast 澄清"消息已排队但 Agent 在等卡片提交"(CH8-2b)——
  mock `get_pending_interaction` 的测试坑见 `../test-environment.md` §8。

---

## 4a. 虚拟化 composable 三约束(09-19-n4-render-virtualization,spike 实证)

消息列表自 N4 起由 `composables/useVirtualizedMessages.ts` 集中装配
`@tanstack/vue-virtual`。spike(`app/bench/spike-follow-options.html`,
Playwright 驱动)实证三条库层硬约束,**改 composable 前必读**——violation
都是静默行为缺陷,类型/测试不拦:

1. **`followOnAppend: true` ≠ 强制跟滚**:core 3.17.11 把 `true` 映射为
   behavior `'auto'`,与 `'auto'` 同受 isAtEnd 门控(滚离末端 append 一律
   不跟)。force-follow(F2 语义:发送后无条件跟每个 delta)必须**手写**:
   watch append + `forceFollowActive` → `scrollToEnd()`,不走 followOnAppend。
2. **Vue 适配层缺 React-parity 的 per-render `_willUpdate`**:纯 resize
   (流式末项 grow,无 append)的钉底调整写入被 clamp 后无重试入口,实测
   卡死差 ~300px。composable 必须 `onUpdated` 每 render 补
   `virtualizer._willUpdate()`。
3. **程序化滚动一律走库 API**(scrollToIndex / scrollToEnd / scrollToOffset),
   禁 `el.scrollTop` 直写——与库事件驱动 offset 同步竞态,isAtEnd 判定读
   滞后值。onScroll **读** DOM 距底(与旧 isNearBottom 同式)不违本条。

配套事实(同 spike/评审):动画属性白名单 = opacity + translateX,**禁
scale/height**——动画中间帧的测量值经 measureElement 按 getItemKey 写入
持久测量缓存,会把中间尺寸固化成 session 内永久空隙(background-color 无
几何效应,不进测量缓存,不受限)。

---

## 5. 本地路径预览链路(2026-09-13;同日扩展到非图片文件 + 工具输出面)

聊天 markdown 与工具输出里的本地路径可点击 → 应用内弹层查看。图片与文件是
**同一条四段链路上的两个平行通道**,改任一段先 grep `md-image-path` /
`md-file-path` 找全消费方:

1. **识别**(`utils/markdown.ts` `renderMarkdown` 管线,downgrade 与 sanitize
   之间插 `linkifyLocalPaths` DOM 后处理——09-13 文件通道起由
   `linkifyImagePaths` 更名):三种形态统一转锚点—— 正文裸路径、
   inline `<code>` 内路径(LLM 习惯写反引号里)、markdown 图片/链接语法
   (`![](本地)` 由 `downgradeExternalImages` 本地分支产出、`[x](本地)` 由
   linkify 给已有 `<a href>` 补 data 属性)。**命中按扩展分流**:图片扩展 →
   `<a class="md-image-path" data-image-path>`,其余文件扩展 →
   `<a class="md-file-path" data-file-path>`。**围栏代码块(pre 祖先)与
   `<a>` 内文本不动**。实现必须走 DOMParser + TreeWalker(字符串后处理分不
   清代码上下文;marked extension 优先于 codespan tokenizer 会吞 inline code)。
   路径正则 `FILE_PATH_RE`(边界捕获组 + Unicode 段字符,同
   chatInputTokens.ts FILE_RE 风格;`IMAGE_PATH_BODY`/`FILE_PATH_BODY` 由
   `pathBodyFor(extSet)` 参数化构造,段/边界语法单源):纯文件名(无 `/`)
   与 http URL 有意不识别;`/api/` 前缀排除仅作用于 href/src 形态
   (附件路由 uuid.png 会误吞)。
   另有 `linkifyPlainText(text)` 导出:给**非 markdown** 文本(工具输出
   `<pre>`)用——整体 escapeHtml → `FILE_PATH_RE` 全局替换插锚(锚文本与
   data 属性值用已转义切片,引号安全)→ `DOMPurify.sanitize`(三层防线,
   维持"所有 v-html 都过 DOMPurify"的仓库不变量)。
2. **取数**(`utils/imageUrl.ts` + daemon `GET /api/v1/files/image?path=` /
   `GET /api/v1/files/raw?path=`;契约表见 `docs/DAEMON-API.md` §7 files 域
   小节):`resolveImagePath(raw, cwd)` 把相对路径按**点击那一刻**的
   `chatStore.currentCwd` 解析(渲染时刻 cwd 会随会话切换漂移;名字沿图片
   首版保留,语义是通用本地路径解析);`/`、`~/` 原样(daemon 端展开 home)。
   `imageUrl` / `fileUrl` 三传输模式与 attachmentUrl 同构(pwa-remote 走
   proxy + `?access_token=`)。daemon 侧契约(校验全在 `commands/files.rs`,
   route 薄壳):`/files/image` 白名单 png/jpg/jpeg/gif/webp/bmp/avif/ico
   (**svg 排除**——独立文档打开时脚本会跑)+ 32 MiB;`/files/raw` 白名单
   文本类 + pdf,文本类 2 MiB(整串进 DOM 防卡死)/ pdf 32 MiB,文本类
   **一律 `text/plain; charset=utf-8` 下发**(.html/.htm 也一样——MIME 即
   闸门,不存在 text/html / image/svg+xml 下发路径)+ 严格 UTF-8 校验
   (非法 400);白名单外统一 400 不给存在性旁信道;400/404/413 分类。
   **前后端白名单有意各持一份**(跨语言共享机制成本高于收益):后端是唯一
   安全闸门,前端集偏大只会点开见 400;改动任一侧(`FILE_EXT` /
   `RAW_TEXT_EXTS`)须对照另一侧。
3. **点击委托**(`composables/useCodeBlockCopy.ts` `onMarkdownClick`):
   `closest("a[data-image-path]")` → `useImageViewer().open(原始路径)`;
   `closest("a[data-file-path]")` → `useFileViewer().open(原始路径)`
   (先 image 后 file,两个 data 属性并存时图片优先)。
   该 composable 是 markdown v-html 容器的统一委托层(代码复制 + 路径预览),
   新交互往这里加分支,容器只需根上绑 `@click="onMarkdownClick"`——**新增
   markdown 容器忘了绑 = 交互静默失效**(MessageItem 主气泡曾是唯一漏绑面,
   09-13 补上;现有绑定面:MessageItem 气泡/时间轴/摘要行、
   DiscussionSummaryCard、SubagentDrawer、MarkdownDetailModal、
   FileViewerModal md 模式、ToolOutputBody 的 pre)。
4. **弹层**(`composables/useImageViewer.ts` + `useFileViewer.ts` 模块级单例,
   `components/common/ImageViewerModal.vue` + `FileViewerModal.vue` 全局唯一
   实例挂 App.vue):reka-ui Dialog 六件套(MarkdownDetailModal 模式)。
   图片版:img onerror 统一错误态(daemon 400/404/413 都长这样),「新标签
   打开」兜底走同一 `imageUrl`;缩放/平移(09-13 同日增强):数学核心在
   `composables/useImagePanZoom.ts`(锚点公式 `t' = a − (s'/s)·(a − t)`、
   clamp [1,8]、回 1 清平移——**坐标模型与 ImageViewerModal 的 stage CSS
   成对**(img 绝对居中 + transform-origin: center,改一处必须同步另一处);
   舞台 overflow:hidden,pan/zoom 全由 img transform 承载不走滚动条)。
   **stage 高度陷阱(09-14 实证)**:stage(flex:1)的内容全是绝对定位
   (img/error 脱离文档流),auto 高度的 flex 容器解析不出它的内容高度 →
   坍缩成 padding 一条;弹层根必须给**确定 height**(86vh),只给 max-height
   不够。jsdom 测不出布局,此类弹层改动要真开一次眼看;
   交互 = wheel(光标锚点,`.prevent`)、pointer 拖拽(canPan 门控 +
   setPointerCapture,jsdom 缺位 try/catch 降级)、双击 2.5×↔复位、header
   控件;换图(src watch)复位缩放态。触摸 pinch 有意不做(见 composable
   尾注)。文件版:open() 里**点击时刻**解析 cwd(pdf 必须在 open 时刻拿到
   绝对 URL 去新标签,故解析比图片版提前);`.md`/`.markdown` →
   renderMarkdown 管线(根绑 onMarkdownClick,嵌套路径递归可点)+ §2 镜像块
   第六处;其余文本类 → 构造 `{type:'code_block'}` 复用 CodeBlockPrimitive
   (hljs 别名按扩展名,复制按钮免费);**pdf 不开弹层**,open() 直接
   `window.open(fileUrl)` 浏览器原生 viewer(2026-09-13 用户决议 Q1);
   文本类 fetch → loading/ok/error 单例状态,连续 open 以序号作废旧响应。
5. **工具输出面**(`components/chat/ToolOutputBody.vue`,2026-09-13):工具
   结果是路径最密集的面但**不走 markdown 管线**。`<pre>` 从文本插值改
   `v-html="linkifyPlainText(truncated)"` + 根绑 `onMarkdownClick`——
   截断契约不变(先 `truncateOutput(display, 500)` 后 linkify):被 500 字
   边界切断的路径缺扩展名尾,正则不匹配,**不产生半截链接**;纯插值改
   v-html 后 XSS 面由 linkifyPlainText 的转义→插锚→sanitize 三层承担
   (AC5 夹具锁死)。组件引入的是 composable 单例、非 store,不违反
   FT-F-001 D3。主面板 `ToolCallCard` 与 `SubagentDrawer`
   `DrawerToolCallCard` 共用本组件,一处改动同时生效。
6. **存在性闸门**(`utils/pathExistence.ts` + daemon `GET /api/v1/files/stat`,
   2026-09-14):LLM 会写不存在的路径(幻觉/未生成的产物),死链接长期
   高亮误导点击。**乐观渲染,确认缺失才降级**——渲染是同步纯函数、存在性
   是异步事实,乐观序让存在的文件(常态)永不闪烁,缺失路径只有一次
   短暂高亮(stat 一个往返)后回纯文本;悲观序会让常态路径闪成链接,
   弃。四个产锚点处(linkify 走查臂/href 补 data 臂/downgrade 本地臂/
   linkifyPlainText)先 consult `pathKnownMissing` 再产锚,未知照产 +
   `schedulePathCheck` 异步确认;href 臂确认缺失时**解包锚点回子节点**
   (不留裸 href 打穿 SPA 路由)。SSE 零阻塞:渲染路径只做 Map 查询,
   fetch 全在渲染外;`createDebouncedRenderer` 的 50ms 节流不变,结果经
   `onPathsResolved` 订阅补偿重渲染(过滤:pending 节流帧让路 + raw 路径
   子串不在文本里零开销跳过 + dispose 退订)。computed 消费方
   (ToolOutputBody 等)靠 reactive Map 依赖追踪免费重算。缓存键 =
   `resolveImagePath(raw, currentCwd)` 同源解析(与点击基准一致);
   positive 永久缓存(文件消失点击走弹层错误态),negative 15s TTL 静默
   重查自愈(重查期间 consult 仍按缺失,**不闪回链接**);只信 200/404,且 404 须
   带**哨兵 body `stat: file not found`**(陈旧 daemon 路由 fallback 的 404 按
   未知处理,防"旧 daemon + 新前端"静默杀光链接;字面量两侧成对,同白名单
   配对约定);400/5xx/网络失败不写缓存保持乐观。测试隔离:vitest 默认禁网
   (`import.meta.env.MODE`,防本机 daemon 真响应注入破坏确定性),
   `pathExistence.test.ts` 用 `__enableNetworkForTests` + stubbed fetch
   测真链路,其余测试用 `setExistenceForTests` 直播种缓存。
   daemon 侧 `/files/stat`(白名单 = image ∪ raw 并集,与前端 `FILE_EXT`
   对齐;metadata O(1) 不读内容无大小上限;`no-store`)契约表在
   `docs/DAEMON-API.md` §7 files 域小节。

**样式边界**:`.md-image-path` / `.md-file-path` 的交互态样式(cursor:pointer
——无 href 的 a UA 不给指针;word-break:break-all——长绝对路径防撑爆气泡)
放**全局 style.css**(两类共持一份);颜色/下划线等排版仍由各容器的
`:deep(a)` 承载(不进 §2 的镜像块——那是排版节奏,这是横切交互态)。
