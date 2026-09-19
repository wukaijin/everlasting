# Design: N4 长会话渲染虚拟化(@tanstack/vue-virtual)

> Task: 09-19-n4-render-virtualization · 2026-09-19
> 输入:prd.md(含用户裁定)+ research/ 两份(库能力矩阵、渲染管线事实)。
> **群聊评审已过**(2026-09-19,session `a46158e8`,review preset 四视角 14.9min / 72.7 万 token,12 条结论全 verified / 0 驳回;转录:`~/.local/share/dev.everlasting.app/discussions/2026-09-19-评审 everlasting 前端 N4 任务…-a46158e8.md`)。结构性修订:新增 PR0 测试基建(F1 childElementCount 判据在虚拟化下效度失效,评审推翻本稿 §7 原判断)、data-seq 滚到命中从 PR3 拉进 PR1(虚拟化后 querySelector 直查静默 no-op)。D1-D5 全部通过,各带补强约束,已回填正文(标注「评审补/评审修订」处)。

## 1. 架构总览

```
ChatPanel(spinner v-if 重挂载模式不变)
└─ MessageList.vue(重写主体)
   └─ .messages(仍是唯一滚动容器;ul → div 化,见 §5)
      ├─ 虚拟项 wrapper × 可见+overscan(绝对定位,data-index,measureElement ref)
      │   └─ MessageItem(li → div 化根元素,组件内部不动)
      └─ (库维护的滚动占位:container 高度 = getTotalSize(),项 translateY 定位)
```

数据流:

```
store.messages(deep reactive,delta 原位 mutate 不变)
  → visibleMessages(O(n) filter,保留)
  → buildRunGroups(纯函数,保留原输出 —— SearchModal 预览继续用)
  → flattenRunGroups(新纯函数:groups → FlatItem[],每项 {message, runFirst}(runFirst=组首标记))
  → useVirtualizer({count, getItemKey: m.id, estimateSize, anchorTo:'end',
                     followOnAppend, scrollEndThreshold:80, ...})
  → getVirtualItems() 渲染
```

**关键决策 D1:buildRunGroups 双出 + 类型约束**。保留原函数(SearchPreviewBody 零改动),新增 `flattenRunGroups` 从 groups 派生扁平列表(utils/messageFormat.ts,与 buildRunGroups 同文件同测)。**签名只收 `RunGroup[]`,禁止 ChatMessage[] 直入变体**(评审结论:防两套分组判据漂移的防线从对照单测升级为类型约束)。打平正确性另用对照单测钉:同输入下 flatten 的组首集合 = groups 的每组 items[0]。

## 2. 锚定迁移映射(旧机制 → 库语义)

| 现机制(MessageList.vue 行号) | 新语义 | 迁移判定 |
|---|---|---|
| `isAtBottom` ref + onScroll 80px 阈值(:42,:188) | `virtualizer.isAtEnd(80)`(scrollEndThreshold 同源);onScroll 保留只管回底按钮显隐与 force-follow 退出 | 直接替换 |
| `forceFollowActive`(F2,store) | options 动态切换 `followOnAppend`:非流式 false / 流式且 force → `true`(强制)/ 流式非 force → `'auto'`(阈值内才跟);用户上滚 isAtEnd 失败时 store 退 flag(逻辑不变) | 语义等价;Vue 适配 setOptions 响应式生效(PR1 验证点)。**PR0 spike 勘误(2026-09-19)**:core 3.17.11 的 `true` ≡ `'auto'`(同映射 behavior:'auto'),isAtEnd 门对两者都生效——「强制」不存在;force-follow 需 composable 手写(forceFollowActive 时直接 `scrollToEnd`),见 implement.md PR0 结论 2/3/4 |
| `scrollToBottom(smooth)`(:107) | `scrollToEnd({behavior})`;流式 delta 跟滚整体交给 followOnAppend(不再逐 delta 手动 scroll) | 删手动路径 |
| `stickToBottomUntilStable` + `stabilizing` 测试信号(:52,:149) | **退役**。mount churn 问题在虚拟化下根因消失:DOM 数量常数,无"10k 组件多帧 patch 期间 scrollHeight 抖动"。mount/session 切换落底 = onMounted + `watch(currentSessionId)` 里 `scrollToIndex(count-1)`(virtual-core 的 initialMeasurementsCache 骑过首帧)。`data-stabilizing` attr 删除,e2e/F1 的等待信号同步改(见 §7) | 净简化(评审确认成立);**PR1 e2e 必补「视口上方项异步变高不跳屏」探针用例**(退役正确性的行为证据) |
| fingerprint watch O(n)(:204) | **退役**。跟滚由 anchorTo:'end' 的末项增长钉住驱动;F5 latency badge 变高属于末项 resize,measureElement 捕获 | 删 |
| `watch(currentPendingInteraction)` CH8-2a 强制回底(:253) | 保留 watch,内部 `scrollToIndex(count-1,{align:'end'})` + force-follow 语义 | 一行改写 |
| `watch(scrollAfterReload)` F4(:262) | reloadAfterFinalize 替换 buffer 后 `scrollToIndex(last)`;虚拟化下无 churn,单次定位即稳 | 简化 |
| `jumpToBottom` 按钮(:124) | `scrollToEnd({behavior: streaming ? 'auto' : 'smooth'})` + force-follow 重挂逻辑不变 | 直接替换 |
| `setListEl` TransitionGroup $el hack(:33) | 退役,普通 ref | 删 |

**关键决策 D2:非流式期消息 append 的跟滚**。followOnAppend 覆盖 append;非流式场景(排队 flush、外部 adopt)走同一 followOnAppend('auto')+ isAtEnd 阈值,不再有独立 watch。

## 3. 虚拟项与间距实现

**关键决策 D3:间距用项内 padding,不用 margin**。tanstack 项定位 = translateY(start) + measureElement(getBoundingClientRect,**不含 margin**);margin 会导致相邻虚拟项视觉重叠。flatten 项 wrapper 样式:`.run-first { padding-top: 12px }`(首个虚拟项除外)`.run-rest { padding-top: 6px }` —— 视觉等价现 run-group 组间 12px / 组内 6px,且计入测量高。评审补两条显式约束:①首项免 padding 判定用 `item.index === 0`(不用组首标记排除,防 flatten 边界漂移);②wrapper 定位 = `position: absolute`,`.messages` 的 flex column + gap **删净**,间距唯一来源 = 项内 padding。

**estimateSize 初版策略(PR1 粗估,PR2 视滚动条收敛调)**:三态常量按画像 —— 纯 text ≈ 260px(399 字符均值)、tool 消息 ≈ 140px/卡、thinking ≈ 60px/块(折叠态),`estimateSize(i)` 按 FlatItem 形态线性组合,宁可估大(首滚略过冲,测后收敛)。测量持久化:`getItemKey=m.id` 稳定,session 内重滚零重测;`useCachedMeasurements` 处理 spinner 重挂载。

**measureElement 挂载**:虚拟项 wrapper 的 `:ref` 回调 → `virtualizer.measureElement(el)` + `data-index`;ResizeObserver 自动跟流式增长 / 工具卡与思考块展开折叠 / 图片加载。

## 4. 动画(R5)

**关键决策 D4:动画降级方案**。TransitionGroup 弃用后:
- **新 run 划入**(现状:整 run-group translateX(24px)+fade):降级为**组首消息**(runFirst 虚拟项)insert 时挂 `run-enter-from` CSS 类(同 from 态参数),下帧移除。触发判据:该 key 上一渲染窗口不存在 && append 于末端 && 可见。仅新 run 划入,流式中追加的 assistant turn 不动画(与现状"归入已有 run 不触发"语义一致)。
- **session 切换 / 首挂载 appear**(现状:整列表划入):降级为 `.messages` 容器一次性 fade-in(CSS animation,挂载触发)。reduced-motion 全局 @media 兜底已有,契约保留。
- **动画属性白名单(评审补)**:只允许 `opacity` + `translateX`,**禁 scale / height**——动画中间帧的测量值经 measureElement 写入按 `getItemKey` 持久的测量缓存,scale/height 动画会把中间尺寸固化为 session 内永久空隙。现 from 态(translateX(24px)+opacity)本就合规。
- 视觉验收:ui-review 静态截图覆盖不了动效(方法局限已有实证),AC6 走人工 + e2e 辅助(类挂载断言)。

## 5. HTML 语义:ul/li → div 化

**关键决策 D5**。MessageItem 根是 `<li class="msg">`;虚拟项 wrapper 若保持 li → li 嵌套 li 非法(浏览器不容器 reparent,但 WebKit 解析行为在动态 insertBefore 下有怪风险);wrapper 用 div 则 ul 直接子节点非 li 也非法。裁定:**聊天消息流放弃 list 语义,全 div 化**——`.messages` ul→div(list-style/gap 原本就重置),虚拟项 wrapper div,MessageItem 根 li→div(`.msg` 类不动,样式选择器基本不受影响)。同步回归三处:e2e 选择器(检查是否 `li`/`ul` tag 选择器)、SearchModal 定位(`[data-seq]` 属性选择器,tag 无关)、a11y(无既有 a11y 门禁)。**评审确认零补偿**:不搭车 aria-live(无读屏消费方实证,出现需求另行立项)。

**data-seq「滚到命中」的修法(评审结构性修订,PR3 → PR1)**:现状 SearchModal 经 querySelector 跨组件直查 DOM 节点(`app/src/components/chat/SearchModal.vue` 附近),虚拟化后离屏消息不在 DOM,**直查静默 no-op**——这是必改项,不能留到 PR3。修法照 `scrollAfterReload` 先例走 **store 命令**:`pendingScrollSeq` → MessageList 侧 `findIndex`(flatten 数组内 seq 匹配)→ `scrollToIndex(i, {align:'center'})`;flash 高亮效果留 PR3。

## 6. 依赖引入

`app/package.json` + `@tanstack/vue-virtual ^3.13.39`(peer vue ^3 ✅;pnpm add)。锁 major;virtual-core 随包精确锁 3.17.11。零传递依赖风险(仅 core)。CI 面原判「无新步骤」有一处例外(检查阶段修正 2026-09-20):虚拟化专项 e2e 复用 bench/fixtures 的 10k 种子(gitignore 生成物),CI 需在 Playwright 步骤前加 `pnpm bench:fe:gen`(确定性 LCG 再生,profile.json 已入库)。

## 7. 测试策略分层(评审后按「决策 vs 结果」重划)

- **vitest(jsdom 无布局——只测决策,不断言结果)**:① `flattenRunGroups` 纯函数对照单测(D1,类型约束 + 组首集合);② 锚定决策纯函数(`followModeOf({isStreaming, forceFollow})` → followOnAppend 值)单测;③ MessageList 组件测:渲染了可见窗口 + wrapper 结构 + data-index/measureElement 挂载,**锚定动作用 spy 断言 scrollToIndex/scrollToEnd 被调用——不断言 scrollTop/scrollHeight**(jsdom 假布局,结果断言 = 假绿);④ 组件测需**假布局 harness**(喂固定 item 尺寸的假 virtualizer),实现方式写进 implement.md。现有 MessageList.test.ts 的 settle 等待逻辑随 stickToBottom 退役重写。
- **e2e(真 Chromium——落底/不跳屏/不抢滚动条等一切结果断言)**:① question-card-scroll 全量回归;② 新增虚拟化专项 spec:10k 种子(复用 `bench/fixtures` 与 `e2e/fixtures` world)滚动到底→顶→中段、**中途展开工具卡高度校正不跳屏**、滚到顶再回底、**视口上方项异步变高不跳屏探针**(D2 退役证据);③ tool-card-compact / chat-input-keys 原样回归;④ data-seq 滚到命中(AC4 前半,PR1)。
- **F1 bench 判据改造(评审推翻原判据)**:`childElementCount` 连续帧不变在虚拟化下**首帧即常数**——f1 会被系统性低估,"仍成立"不成立。PR0 把 mount 稳定判据改为虚拟化中立的新判据(如 rAF 序列上 scrollHeight/总尺寸稳定),**并用新判据对旧实现(裸 v-for)重测一版基线**;AC1 报告同判据双行(新判据旧实现基线行 + 新实现行),150ms 线不动。
- **ready 信号收敛(评审)**:等待用 **scrollHeight 静默帧 helper**(连续 N 帧不变 + 超时失败语义,全 spec 单点实现共享);断言用 **isAtEnd**(短列表首帧即真,只作钉底态断言);「子元素数稳定」弱判据弃用。`stabilizing`/`data-stabilizing` 删除。

## 8. PR 切分与验收门(评审修订:四 PR,新增 PR0 测试基建)

| PR | 内容 | 验收门 |
|---|---|---|
| **PR0 测试基建**(零产品代码) | 四件钉死:① selector 迁移(ul/li tag 选择器改 class,新旧实现中立);② 共享 ready helper(scrollHeight 静默帧 + 超时失败,单点实现);③ **F1 bench 判据改造 + 用新判据对旧实现重测基线**(AC1 双行口径的基线行);④ §9-1 spike:运行中动态三态翻转(followOnAppend `false→'auto'→true` 断言跟滚切换)——**静态配置跑通不构成验证** | 全量测试在新 selector/判据下对旧实现全绿;spike 出透传结论(U3 条件决策点在此拍板,不能等到 PR1 写一半) |
| **PR1 骨架替换**(核心) | 依赖引入;flatten + D1/D3/D5;MessageList 重写为 useVirtualizer(**虚拟化调用集中装配在单一 composable**——spike 证伪时补丁 = 单文件手写 follow);§2 表全量迁移(旧机制退役);estimateSize 粗估;**data-seq 滚到命中(store 命令,AC4 前半)**;e2e 虚拟化专项 + 等待信号迁移 | vitest + e2e 全绿(含新增)+ **F1 新判据复跑**(AC1 双行报告)+ **三场真人冒烟**(流式跟滚/上滚读历史/回底按钮) |
| **PR2 语义精修** | estimateSize 调优(滚动条收敛);**shouldAdjustScrollPositionOnItemSizeChange 升格为正式回退位**(校正行为异常时的第一调节旋钮);流式跟滚手感;**慢滚 10k 全程锚定漂移 ≤ 一行高**验收;F5 badge / reload / pending 边界补强;AC2 逐项 e2e | e2e + F1 复跑对比 PR1(不回归) |
| **PR3 外围收尾** | data-seq flash 高亮;动画 D4(白名单约束);SearchModal/预览侧回归确认;移动端 390px + ui-review 视觉回归 + **IME/键盘/容器 resize 人工清单**(U2);AC6 人工动效验收 | 全量回归 + ui-review + AC6 |

**回滚 = 可运行证明**(评审升级):显式否决双渲染 feature flag;revert PR1 后,**PR0 的实现中立用例同套再绿即回滚验收**(不靠口头"回到现状")。PR1 失败回退后回到裸 v-for(n9-first 与 PR0 新判据基线双参照)。

## 9. 风险与开放验证点(评审后)

1. **Vue 适配层对 anchorTo/followOnAppend 的 setOptions 响应式透传**(较新 API,文档对应 core 3.17.11 与锁版一致)——**验证形态 = 运行中动态三态翻转**(followOnAppend false→'auto'→true 断言跟滚切换),静态配置跑通不构成验证;**前置到 PR0 spike**,证伪则 PR1 加手写 follow 路径(落点 = 集中装配 composable,单文件补丁)。
2. **estimateSize 估小 → 首滚跳动**:估大策略缓解;PR2 调优(计划内非风险,U5)。
3. **测量回调频率**(流式 20 delta/s × 末项 ResizeObserver):F1 实测;必要时 `shouldAdjustScrollPositionOnItemSizeChange` 定制(PR2 正式回退位)。
4. **WebKitGTK 实机**(U1,WSL2 web 面无法覆盖):PR3 人工清单一行。
5. **IME 组合输入 / 移动端键盘 / 容器 resize 下的钉底行为**(U2):存量未知(现状代码零处理,非本任务引入的回归面)——PR3 人工清单一行,实测有回归另行立项。
6. **f4 残余的第一嫌疑人**(U4,PR1 F1 复跑观察项):visibleMessages→buildRunGroups→flatten 每拍全链重算(虚拟项外的高频重算,非 virtualizer 本身);超线另行立项。
