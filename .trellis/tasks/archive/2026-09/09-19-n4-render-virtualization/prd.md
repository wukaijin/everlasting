# PRD: N4 长会话渲染虚拟化(@tanstack/vue-virtual)

> Task: 09-19-n4-render-virtualization · 2026-09-19 立项
> 依赖:N9 性能基准(✅ 09-19 交付)提供 F1 基线与复跑纪律;本任务为 N2(checkpoint/revert)前置。

## Goal

把 `MessageList.vue` 的裸 v-for 全量 DOM 渲染替换为 @tanstack/vue-virtual 真虚拟化(消息粒度、动态高度、聊天流锚定),使长会话(1k/10k 消息前瞻档)下滚动、流式回放、会话打开恢复可用,并消除 10k 档的 renderer 内存崩溃风险。

**用户价值**:10k 档流式回放到上屏 21.5s(会话事实不可用)、滚帧 185ms(11 倍掉帧)→ 恢复流畅;为 rewind(N2)的长会话随机跳跃铺路。

## Confirmed Facts

**裁定(用户,2026-09-19)**
- 路线 = 真虚拟化(弃 content-visibility / v-memo 递进方案)。
- 库 = `@tanstack/vue-virtual`(3.13.39,virtual-core 3.17.11,vue ^3 peer ✅,活跃维护)。

**代码/基线事实(见 research/ 两份文档)**
- F1 基线(10k 档):mount 491.4ms / 滚帧 184.8ms / 流式上屏 21,508ms;三项卡三个层(vdom JS / 浏览器渲染 / 组件实例化),虚拟化是三者共同根治面。
- 库已内建聊天场景核心语义:`anchorTo:'end'`(流式末项增长钉底)+ `followOnAppend`(视口在末端才跟滚)+ `scrollEndThreshold`(→80px)+ `scrollToEnd` + `measureElement`。
- 七难点判定:4 库内建、1 净简化(旧滚动机制退役)、2 自实现(run-group 打平 = 主要结构工作;enter 动画重做 = 中风险)。
- 真实画像:P50<10 条、最大 156;1k/10k 是前瞻档(N2 rewind 面向)。

## Requirements

- **R1 虚拟化渲染**:MessageList 以 `useVirtualizer`(消息粒度扁平列表,`getItemKey=m.id`)渲染;`.messages` 仍是唯一滚动容器;DOM 数量与 session 长度解耦(可见窗口 + overscan 常数)。
- **R2 锚定与流式语义迁移**:`anchorTo:'end'` + `followOnAppend` + `scrollEndThreshold=80` 覆盖现有 isAtBottom/force-follow/80px 阈值语义;`stickToBottomUntilStable` rAF 稳定器与 O(n) fingerprint watch **退役**(由库语义替代);F5 latency badge 变高、reload/session 切换钉底、pending 强制回底(CH8-2a)行为保持。
- **R3 run 分组视觉保持**:buildRunGroups 纯函数保留(输出打平为消息级 + 组首元数据);交错思考 run 的视觉流(组内 6px / 组间 12px、speaker 对齐)不回退;SearchModal 预览侧共用面同步适配。
- **R4 `data-seq` 搜索定位**:SearchModal「在主窗口打开」滚动到命中消息改走 store 命令(`pendingScrollSeq` → scrollToIndex align:center;现状 querySelector 直查在虚拟化后静默 no-op,评审拉进 PR1);flash 高亮保留(PR3)。
- **R5 动画**:现状 TransitionGroup 提供两类动画——新 run 划入(组首挂类重做,仅"新 append 且可见"触发)与 session 切换/首挂载 appear(降级为 `.messages` 容器一次性 fade-in);`prefers-reduced-motion` 契约保留。
- **R6 回归门**:既有 vitest(MessageList/MessageItem 相关)+ e2e 13 例全绿(question-card-scroll 的 scrollTop/scrollHeight 行为断言重点);F1 同参复跑(perf-baseline.md §6 纪律)。

## Acceptance Criteria

- **AC1(F1 复跑,10k 档,同判据双行)**:三指标达到「回到 100 档体验线」——**f4 流式上屏 ≤ 500ms / f2 滚帧 ≤ 30ms / f1 mount ≤ 150ms**(用户裁定 2026-09-19;基线 491.4 / 184.8 / 21,508ms)。**口径(评审修订)**:mount 判据换虚拟化中立场(scrollHeight 稳定,childElementCount 在虚拟化下首帧即常数已失效),报告双行 = 新判据旧实现基线行 + 新实现行;PR0 落基线行。1k 档同向改善。数字不进 CI 门禁(N9 裁定不变),此线仅作本任务完成判定。
- **AC2(锚定回归,e2e)**:流式中用户上滚 → 不抢滚动条 + 回底按钮出现;点回底 → 瞬跳并重挂跟滚;pending 卡片 null→some 强制回底;reload/session 切换落底无弹跳。
- **AC3(打平正确性,vitest)**:打平渲染输出与现 run-group 分组在相同 store.messages 输入下视觉等价(组边界/组首标记);SearchModal 预览不回归。
- **AC4(搜索定位,拆半)**:PR1 = 滚到命中(e2e:跨会话搜索「在主窗口打开」滚动到命中消息视口);PR3 = flash 高亮。
- **AC5(内存,定性)**:F1 10k 档复跑无 renderer 崩溃(现状:同 renderer 累积 reload 必崩)。
- **AC6(动效,人工/e2e)**:新 run 划入动画存在且不打扰滚动位置;reduced-motion 下即时出现。

## Out of Scope

- content-visibility / v-memo 局部优化(裁定不走;若虚拟化后有残余热点另行立项)。
- SubagentDrawer 列表、SearchPreviewBody 预览的虚拟化(短列表)。
- 分页加载 / 无限 prepend(向后翻历史加载更早消息)——真实画像最大 156 条,一次性渲染;库的 prepend 稳定性留作 N2 面。
- 移动端专项手势优化(仅回归不退化)。

## Open Questions

- **Q2(流程)**:是否按惯例跑群聊评审(推荐:PR1 实施前对 design.md 评一场)。
