# Research: @tanstack/vue-virtual 能力面与难点覆盖(2026-09-19)

> N4 路线裁定:真虚拟化 + @tanstack/vue-virtual(用户 2026-09-19 裁定,弃 content-visibility / v-memo 递进方案)。
> 本文档 = 库状态核实 + 项目七难点 × 库 API 匹配矩阵 + 风险清单。代码事实见同目录 `render-pipeline-facts.md`。

## 1. 库状态(npm registry 实查,2026-09-19)

- `@tanstack/vue-virtual` latest **3.13.39**(2026-09-14 发布,5 天前);近三月月度级更新(07-22 / 07-28 / 08-18 / 09-07 / 09-11 / 09-14)——活跃维护。
- peerDeps `vue ^2.7.0 || ^3.0.0`(项目 vue ^3.5.13 ✅);deps 精确锁 `@tanstack/virtual-core 3.17.11`。
- 架构:headless core(virtual-core,全部逻辑)+ 薄 Vue 适配(`useVirtualizer` = options 塞进 Vue 响应式,官方模式 watchEffect/setOptions,见 TanStack Table Vue 虚拟化指南)。核心逻辑与 React 兄弟包(3.14.x,周更)共享生产验证,Vue 层薄到没有多少可出 bug 的面。
- React 版 LogRocket 教程(2025-11)有完整的 livestream chat 场景实践(动态行高 + append + 跟滚),API 与 Vue 适配几乎一致。

## 2. 聊天场景 API(Virtualizer API 文档,对应 core 3.17.x)

| API | 行为 | 项目语义映射 |
|---|---|---|
| `anchorTo: 'start' \| 'end'`(默认 start) | `'end'` = 聊天/日志流模式:旧项 prepend 时保持当前可见项稳定;**末项流式增长时保持 end-pinned 视口钉住**。要求 `getItemKey` 持久 key | 项目 `m.id` 天然满足;替代 stickToBottomUntilStable 的"骑过 mount churn"职责 |
| `followOnAppend: boolean \| 'auto' \| 'smooth' \| 'instant'`(默认 false) | append 后仅当视口本来就在末端 `scrollEndThreshold` 内才跟滚;上滚读历史不抢滚动条。传 `true` 强制 | = `isAtBottom(80px) + shouldFollow` 的库内建版;'auto' 强制形态对应 force-follow |
| `scrollEndThreshold: number`(默认 1) | `isAtEnd()` 判定阈值(px) | 直接映射现有 80px 阈值 |
| `scrollToIndex(i, {align, behavior})` / `scrollToEnd({behavior})` | 定位;smooth 滚动中只测目标 buffer 内项防漂移 | jumpToBottom 按钮 + data-seq 搜索定位 |
| `isAtEnd(threshold?)` / `getDistanceFromEnd()` | 末端检测 | isAtBottom ref 的库等价物 |
| `measureElement(el)`(默认 getBoundingClientRect + ResizeObserver) | 动态高度测量 | 工具卡/思考块展开折叠、流式增长 |
| `estimateSize(i) => number` | 必填初始估高(建议估大不估小) | 按消息形态三态估(thinking/tool/text,真实画像有均值) |
| `shouldAdjustScrollPositionOnItemSizeChange` | 视口上方项 resize 时的滚动位置校正控制(默认反向滚动时跳过);**iOS WebKit 把此类写推迟到滚动稳定** | 库对 WebKit 引擎坑有专门兜底 → 对 Tauri WKWebView/WebKitGTK 利好 |
| `resizeItem(i, size)` / `measure()` | 手动尺寸 / 重置测量 | 备用 |
| `takeSnapshot()` + `initialMeasurementsCache` + `initialOffset` | 测量缓存与位置持久化 | session 切换 / 组件隐藏 |
| `useCachedMeasurements` | display:none 前置开,防测量清零 | session 切换(spinner v-if 交换重挂载) |
| `getVirtualItems()` / `getTotalSize()` | 可见项集 / spacer 总高 | 渲染循环 + e2e scrollHeight 断言适配 |
| `getItemKey` | 持久 key(prepend 稳定性前提) | `m.id` |

## 3. 七难点 × 库能力判定

| # | 难点 | 判定 | 依据 |
|---|---|---|---|
| ① | 动态高度 + 流式末项增长 | **库内建** | `measureElement` + `anchorTo:'end'` 流式钉住 + `shouldAdjustScrollPositionOnItemSizeChange` |
| ② | TransitionGroup / enter 动画 | **自实现,中风险** | 虚拟化接管 children,TransitionGroup 弃用;followOnAppend:'smooth' 承担跟滚平滑;新 run 划入动画 → 对"新 append 且可见"组首挂 CSS 类;session 切换重挂载本就无 appear;prefers-reduced-motion 契约保留 |
| ③ | stickToBottomUntilStable / isNearBottom / force-follow | **库内建,净简化** | anchorTo:'end' + followOnAppend + scrollEndThreshold(80)+ scrollToEnd;**rAF 稳定器 + O(n) fingerprint watch 整个退役** |
| ④ | data-seq 搜索定位(SearchModal→主窗口) | 低成本适配 | scrollToIndex(i,{align:'center'}) + seq→index 映射;flash 类渲染后挂 |
| ⑤ | run-group 嵌套结构 | **主要结构工作** | 虚拟化粒度必须扁平(消息级);buildRunGroups 纯函数保留但输出打平(组首元数据);run-group li 容器消失,组间视觉靠消息级样式(组首 padding-top / 组尾 padding-bottom 类) |
| ⑥ | pending 强制回底(CH8-2a) | 库内建 | scrollToIndex(last);followOnAppend 强制形态 |
| ⑦ | e2e scrollTop/scrollHeight 断言(3 spec) | 中低适配量 | .messages 仍是滚动容器,scrollTop 语义不变;scrollHeight = getTotalSize()+padding(库 spacer);小会话近似全渲染;question-card-scroll 行为断言跑一遍验证 |

另:虚拟化后 DOM 常数化 → 10k 档 WSL2 Chromium 崩溃场景(N9 F1 实测教训)消失;f1 mount 预期从 491ms 转常数级。

## 4. 风险清单(设计/实施阶段处理)

1. **动画重做的视觉回归**(②):静态截图看不见动效(ui-review 方法局限已有实证)——验收形态待定(e2e 或人工)。
2. **anchorTo/followOnAppend 为较新 API**:文档与 core 3.17.11 精确对应,但需在 PR1 骨架里核对 Vue 适配层透传完整性;Linux WebKitGTK 实机验证(滚动校正的 WebKit 延迟写行为是库已知处理项,方向利好)。
3. **estimateSize 初始估高**:按形态三态估;首滚滚动条长度有收敛过程(测后即准)。
4. **流式高频测量**:20 delta/s 的 ResizeObserver 回调 + 测量缓存写频率,F1 实测定(数字不进门禁,change detection 判定)。
5. 既有 13 例 e2e + 移动端 390px 动量滚动回归面。
6. **v-memo 不采用**(用户裁定 B):若 tanstack 落地后 f4 仍有残余(如虚拟项外的高频重算),再做局部 memo——不在本任务范围。

## 5. 参考

- Virtualizer API:https://tanstack.com/virtual/latest/docs/api/virtualizer
- TanStack Table Vue 虚拟化指南(useVirtualizer + watchEffect 模式):https://tanstack.com
- LogRocket: TanStack Virtual 聊天流实践(React,API 同构):https://blog.logrocket.com(2025-11)
- npm registry 实查(2026-09-19):@tanstack/vue-virtual 3.13.39 / virtual-core 3.17.11
