# Research: 现有消息渲染管线代码事实与瓶颈归因(2026-09-19)

> N9 F1 基线(10k 档:mount 491.4ms / 滚帧 184.8ms / 流式上屏 21,508ms)的代码级归因。
> 基线出处:`.trellis/spec/backend/perf-baseline.md` §3 F1;bench 源码 `app/bench/render.bench.ts`。

## 1. 渲染链结构(MessageList.vue,463 行)

```
TransitionGroup(tag="ul", :ref="setListEl", appear, name="msg")
  └─ v-for g in renderGroups          ← 外层 run-group <li class="run-group">(TransitionGroup 直接子节点)
       └─ v-for m in g.items           ← 内层 MessageItem(2137 行/组件,~20 子组件导入)
            data-seq fallthrough(SearchModal 定位钩)
```

- `visibleMessages` computed:O(n) filter(store.messages),可见谓词 = content/toolCalls/error/thinkingBlocks/redactedThinkingData 非空([MessageList.vue:54](../../../../app/src/components/chat/MessageList.vue#L54))。
- `renderGroups` computed:O(n) `buildRunGroups(visibleMessages)`(utils/messageFormat.ts,纯函数,D2 起与 SearchModal 预览共用;run = 真·用户输入开启,ghost user/orphan-repair 归前 run)。
- 滚动容器:`.messages`(ul 自身,overflow-y:auto,scrollbar-gutter:stable,overflow-x:hidden 裁剪动画偏移)。
- 布局:run-group 间 gap 12px(ul flex column),组内 gap 6px。

## 2. 滚动/跟底机制清单(全部要面对虚拟化迁移)

| 机制 | 位置 | 语义 |
|---|---|---|
| `isAtBottom` ref + `onScroll`(passive) | MessageList.vue:42,188 | near-bottom 80px 阈值;驱动回底按钮显隐;force-follow 用户上滚 >80px 退出 |
| `forceFollowActive` | chat store(F2) | 发送后无条件跟每个 delta;stream done/error 复位 |
| `scrollToBottom(smooth)` | :107 | 流式 delta 跟滚必须 instant(smooth 会叠动画卡顿);按钮回底 idle 时 smooth |
| `jumpToBottom` | :124 | 流式中瞬跳 + 重挂 force-follow;idle 平滑 |
| `stickToBottomUntilStable(deadline 1000ms, quiet 150ms)` | :149 | rAF 循环钉底,骑过 reloadAfterFinalize / session 切换的多帧 mount churn(scrollHeight 静默 quiet 才退出);`stabilizing` ref 是 e2e 测试信号(data-stabilizing attr) |
| fingerprint watch | :204 | O(n) `messages.map(指纹).join("\|")` 逐 delta 重算,决定 shouldFollow 后 scrollToBottom;含 latency/thinkingDurationMs(F5 badge 变高也要跟) |
| `watch(currentSessionId)` | :232 | 切 session → stickToBottomUntilStable(ChatPanel spinner v-if 重挂载,onMounted 也跑) |
| `watch(currentPendingInteraction)` | :253 | CH8-2a:pending interaction null→some 强制回底(loop 阻塞等人,全 UI 唯一值得打断滚动位置的状态);some→some 不触发 |
| `watch(scrollAfterReload)` | :262 | F4 reload 后钉底 |
| 回底按钮 | :319 | absolute 于 .messages-wrap;移动端 44px 避让(responsive-mobile HIG) |

## 3. 瓶颈归因(三指标三层次)

**f4 流式 21.5s —— Vue vdom 层(JS)。** delta 处理 `last.content += event.text` 原位 mutate([streamEvents.ts:323](../../../../app/src/stores/streamEvents.ts#L323));每 delta 触发:visibleMessages O(n) 重算 + fingerprint watch O(n) join + renderGroups 重建 + TransitionGroup 全量 ~10k MessageItem vnode 重建与 keyed diff(内容未变也逃不掉)。每 delta 成本超线性放大(100 档 ~7ms → 1k 档 ~59ms → 10k 档 ~1000ms),keyed diff O(n) + GC 特征。→ content-visibility 治不了(JS 层);v-memo 可治但被裁定不走;虚拟化(可见项常数)根治。

**f2 滚帧 185ms —— 浏览器渲染层。** programmatic scroll 无 JS patch(onScroll 只算 isNearBottom);成本全在浏览器对 10k 组件 DOM(数十万节点)的 layout/paint/hit-test。

**f1 mount 491ms —— 组件实例化层。** 10k 个 MessageItem 组件实例 + DOM 全量构建。虚拟化是唯一解;附带 10k 档 WSL2 Chromium 崩溃风险(F1 实测:同 renderer 累积 reload 崩,必须每档独立 test())随之消失。

## 4. e2e 依赖面(13 例存量 + bench)

- `question-card-scroll.spec.ts`:scrollTop/scrollHeight **行为值断言**(jsdom scrollTo mock 滚不动,必须真 Chromium);mock 种子小会话;pending null→some 回底联动 = CH8-2a 的回归面。
- `tool-card-compact.spec.ts`:read 族卡片高度门 ≤34px(截图/几何断言)。
- `chat-input-keys.spec.ts`:输入键路由(滚动面轻)。
- F1 bench(`app/bench/render.bench.ts`):mount 稳定判据 = `.messages` 子元素数连续 3 帧不变——**虚拟化后子元素数=可见项+spacer,判据仍成立但含义变化(稳定性收敛更快)**;f4 判据 body 文本含末 delta(不变);复跑纪律 = perf-baseline.md §6(改 MessageList.vue 必复跑 F1)。

## 5. 真实画像口径(perf-baseline.md §4,2026-09-19)

94 session / 1720 消息:规模 P50<10(53 个 ≤10 / 40 个 11-100 / 1 个 101-1k / 0 个 >1k,最大 156)。形态:thinking 43.5% / tool 对 40% / 纯 text 16.5%;文本均值 399 字符、tool_result 均值 2,709 字符最大 126k。→ 100 档贴近现实,1k/10k 前瞻档;种子 profile 单一出处 `benches/profile.json`。

## 6. 相关但不在本任务面的镜像渲染

- SearchModal 的 SearchPreviewBody 复用 `buildRunGroups`(D2 提取共用)——搜索预览天然短列表,不虚拟化;但 buildRunGroups 签名若变(打平输出),预览侧要同步适配或保留旧包装。
- SubagentDrawer 的 DrawerToolCallCard 是独立列表,不在本任务面。
