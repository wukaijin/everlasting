# Implement: N4 长会话渲染虚拟化(@tanstack/vue-virtual)

> Task: 09-19-n4-render-virtualization · 2026-09-19
> 前置:prd.md + design.md(群聊评审结论已回填,session a46158e8,12 条全 verified)。
> PR 切分按评审修订:PR0 测试基建 → PR1 骨架 → PR2 精修 → PR3 外围。

## 执行序

### PR0 测试基建(零产品代码;spike 结论是 PR1 的闸门)

- [x] **selector 迁移**:全 e2e 扫 `li`/`ul` tag 选择器 → class 选择器(新旧实现中立);对旧实现全绿
  - 实际面:tag 复合选择器只有 question-card-scroll.spec.ts 的 `ul.messages` ×3 → `.messages`(fixtures 常量 `MESSAGES`);e2e 无 `li`/`> li` 选择器;e2e/README.md 登记同步
- [x] **共享 ready helper**:`waitForListReady(page)`——scrollHeight 静默帧(连续 N 帧不变)+ 超时失败语义,单点实现于 e2e/fixtures;全 spec 换用;弃用「子元素数稳定」弱判据(含 F1 bench 判据)
  - 形态:`stableFrames=3`(帧)∧ `quietMs=250`(时长)> 现实现 stickToBottomUntilStable 的 150ms 退出窗——仅 N≈3 帧(≈50ms)时钉底循环可能仍在跑,后续 pin 会拖回测试写入的 scrollTop(question-card-scroll 冷跑 flaky 根因);超时 15s fail-loud 带观测值
- [x] **F1 bench 判据改造**:mount 稳定判据 childElementCount → 虚拟化中立判据(scrollHeight/总尺寸稳定);**用新判据对旧实现(裸 v-for)重测三档基线并落档**(AC1 双行口径的基线行)
  - 判据 = `.messages` scrollHeight 连续 3 帧 rAF 不变且 >0;数字落 `.trellis/spec/backend/perf-baseline.md` §3 F1(旧值保留 + 判据变更头注)
- [x] **§9-1 spike**:运行中动态三态翻转——建最小 Vue 测试页挂 useVirtualizer,流式 append 过程中断言 followOnAppend `false→'auto'→true` 切换真的改变跟滚行为(setOptions 响应式透传);**静态配置跑通不构成验证**;结论(U3)写回本文件:`透传 ✅/❌`,❌ 则 PR1 加手写 follow(composable 单文件)
  - 产物:`app/bench/spike-follow-options.html`(vite dev 直出 + `window.__spike` 驱动面)+ `app/e2e/spike-follow-options.spec.ts`(裸 @playwright/test,断言真实跑,e2e 门禁内)
  - 依赖:`@tanstack/vue-virtual@3.13.39`(devDependencies;virtual-core 3.17.11);vite.config.ts `optimizeDeps.include` 预热防冷启动 discovery reload 竞态
- [x] 验收:全量测试对旧实现全绿;spike 结论落档
  - `pnpm test` 1914 绿 / `pnpm test:e2e` 14 绿(13 存量 + spike)/ `pnpm build` 绿 / bench 单文件 tsc 门绿

**PR0 spike 结论(U3 拍板,2026-09-19)**:`透传 ✅`(有两条硬保留,PR1 composable 必须吃下):

1. **setOptions 响应式透传成立**:`followOnAppend false ↔ 'auto'` 运行中翻转真实改变跟滚行为(append 时 `isAtEnd` 判定 + scrollToEnd 由新值驱动);库实例 `options.followOnAppend` 同步为新值(`followSeenByLib` 侧写)——false 态 append 不跟(距底拉开 2×60px、scrollTop 不动)、'auto' 态视口在末端 append 跟滚(钉回 0)。设计 §2「锚定迁移映射」的动态 options 路线可用。
2. **「`true` = 强制跟滚」语义不存在**:virtual-core 3.17.11 的 setOptions 把 `true` 映射为 behavior:'auto',与 `'auto'` 完全同效;`isAtEnd(scrollEndThreshold)` 门对两者一视同仁——滚离末端(>80px)后 append 一律不跟(spec 腿③/③b 实证)。设计 §2 表「流式且 force → `true`(强制)」一行需修正:**force-follow(用户 10–80px 内未退、或 CH8-2a 强制回底后的流式)必须 composable 手写**(forceFollowActive 时直接 `scrollToEnd({behavior:'auto'})`,不等库的 followOnAppend)。
3. **anchorTo:'end' 的纯 resize 钉底在 Vue 适配层有缺口**:末项 grow(无 append)时钉底写入发生在 DOM scrollHeight 长高之前 → 被浏览器 clamp 丢弃;补写入口 `_retryClampedAdjustment` 只在 `_willUpdate()` 尾部触发,而 **Vue 适配层只在 options/scrollElement 变化时调 `_willUpdate`(React 适配层每 render 都调)** → 钉底卡死差一截(实测稳定差 300px)。消费侧每 render 补一次 `virtualizer._willUpdate()`(Vue `onUpdated`)即完成钉底(spike 页腿④实证)——PR1 composable 必须带这条 React-parity 补偿,否则流式末项增长钉底不成立。
4. **程序化滚动必须走库 API**(`scrollToEnd`/`scrollBy`),禁止原生 `el.scrollTop` 直写:直写与库的事件驱动内部 offset 同步竞态,随后的 append/resize 判定(`isAtEnd`/`wasAtEnd`)读到滞后 offset 而失效(实测 false-负)。PR1 §2 迁移表的 scrollToIndex/scrollToEnd 替换天然满足;e2e 若需程序化滚动同样走组件行为路径。

> 备注:spike 页含诊断句柄 `window.__lib`(库实例)与 `waitSynced` 钩子;spec 的③腿断言按实证现实写(不跟),④腿先等内容长高再断言钉底(防止在旧末端空转通过)。

### PR1 骨架替换(核心)

- [x] `app/`:`pnpm add @tanstack/vue-virtual`(锁 ^3.13.x)
  - PR0 装在 devDependencies,PR1 挪到 dependencies(产品依赖)
- [x] `utils/messageFormat.ts`:`flattenRunGroups(groups: RunGroup[])`(签名禁 ChatMessage[] 直入,评审 D1);FlatItem = {message, runFirst};对照单测(组首集合 = groups[].items[0])
  - 测试落 `utils/buildRunGroups.test.ts`(3 新用例);泛型 `FlatItem<T>`,`ChatMessageLike` 结构约束
- [x] 锚定决策纯函数 `followModeOf({isStreaming, forceFollow})` + 单测
  - 落 `composables/useVirtualizedMessages.ts`,返回 `AnchorDecision{followOnAppend, forceFollow}` 双路径决策(spike 勘误后「强制」不是 followOnAppend 取值,是 composable 第二条路径);4 用例含「永不返回 true」回归钉
- [x] **虚拟化 composable 集中装配**(`useVirtualizedMessages`,单文件——spike 证伪时手写 follow 的落点)
  - `app/src/composables/useVirtualizedMessages.ts`:三条 spike 约束全部内嵌(①手写 force-follow watch append;②onUpdated 每 render `_willUpdate()` React-parity 补偿;③程序化滚动全走库 API);§2 迁移表全部 watch(jumpToBottom/pending CH8-2a/reload F4/session 切换+mount 落底/pendingScrollSeq)集中于此
  - 实施修订:onScroll 的按钮显隐/force 退出判定用 DOM 距底读数(与旧 isNearBottom 同式),不读 `virtualizer.isAtEnd()` —— 库内 offset 在它自己的 scroll 监听器更新,Vue @scroll 可能先触发读到滞后值(e2e 实证);阈值常量 `SCROLL_END_THRESHOLD=80` 与 options.scrollEndThreshold 同源共享
  - estimateSize 三态粗估 text 260 / tool 140/卡(toolResults 同价) / thinking 60/块,兜底下限 48
- [x] `MessageList.vue` 重写:
  - [x] 退役:TransitionGroup / setListEl / stickToBottomUntilStable / stabilizing(data-stabilizing)/ fingerprint watch / scrollToBottom 手动路径
  - [x] useVirtualizer(anchorTo:'end', followOnAppend 动态(followModeOf), scrollEndThreshold:80, getItemKey:m.id, estimateSize 三态粗估:text 260 / tool 140/卡 / thinking 60/块)
  - [x] 虚拟项 wrapper:absolute 定位,项内 padding 间距(D3:run-first 12px / run-rest 6px / 首项 index===0 免);.messages flex+gap 删净
  - [x] §2 迁移表逐行落地(jumpToBottom / pending CH8-2a / reload F4 / session 切换 → scrollToIndex / scrollToEnd)
  - [x] ul/li → div 化(D5:.messages tag + MessageItem 根;wrapper 需保留 flex column 上下文,.msg 的 align-self 对齐依赖它)
- [x] **data-seq 滚到命中(PR1 必做,评审拉前)**:store 加 `pendingScrollSeq` 命令(照 scrollAfterReload 先例);MessageList 侧 findIndex + scrollToIndex(align:'center');flash 留 PR3;AC4 前半 e2e
  - store `pendingScrollSeq: Ref<number|null>`(消费后清零,同 seq 可再触发);SearchModal.locateMessage 改写命令(nextTick+nextPaint 后置值),CH12-1b vitest 断言同步迁移(scrollIntoView spy → 命令置值断言);消费 watch 带 immediate(spinner 重挂竞态窗口补消费)
- [x] MessageList.test.ts 重写:决策断言(spy scrollToIndex/scrollToEnd,**不碰 scrollTop/scrollHeight**)+ 假布局 harness(固定尺寸假 virtualizer)
  - 假 harness = vi.mock("@tanstack/vue-virtual") 返回固定 60px/项的假 virtualizer,getVirtualItems/getTotalSize 从 composable 传入的 options(count+getItemKey)派生 → 真实 composable 全链可跑;14 用例(结构/间距类/按钮/CH8-2a 三态/pendingScrollSeq 三例/手写 force-follow 三例);按钮与 force 退出用例以 defineProperty 喂 DOM 读数**输入**(断言仍全在决策侧)
- [x] e2e 新增 `virtualized-list.spec.ts`:10k 种子滚动底/顶/中段、展开工具卡高度校正不跳屏、**视口上方项异步变高不跳屏探针**(D2 退役证据)、data-seq 滚到命中
  - 4 用例;种子复用 bench/fixtures(readFixture(10000))+ world;两个「不跳屏」探针以压缩摘要行(compaction_summary,点击展开)为确定性变高源;实测教训:①滚后 estimate→actual 校正有尾流,探针基线必须取自静默布局(waitForReadingQuiet helper);②哨兵用 data-seq 固定身份,按位置取 rows[0] 会因窗口重排换行产生假稳定
  - **CI 缺口(检查阶段补,2026-09-20)**:种子是 gitignore 生成物,CI 原不跑 gen → `pnpm test:e2e` 在 CI 必挂(readFixture ENOENT)。修法 = ci.yml Playwright 步骤前加 `pnpm bench:fe:gen`(profile.json 已入库,LCG 确定性);design §6「CI 面无新步骤」已同步勘误
- [x] 全量:`pnpm test` / `pnpm test:e2e` / `pnpm build` / **F1 新判据复跑三档**(AC1 双行报告:PR0 基线行 vs 新实现行,对线 f4≤500 / f2≤30 / f1≤150 @10k)
  - vitest 1936 绿(144 文件)/ e2e 18 绿(14 存量+spike+4 新增)/ build 绿 / bench 三档绿
  - **F1(10k median):f1 70.8 ✅(≤150)/ f2 44.7 ❌(线 30,旧实现同尺 82.5 → −46%)/ f4 859 ❌(线 500,旧实现同尺 17,742 → −95%)**;100/1k 档:f1 63.7/69.7,f2 39.0/37.9,f4 119/186;残余构成与归因见 perf-baseline.md §3 F1 PR1 行(f4=U4 预判链路实测坐实,f2=窗口重挂常数,均 PR2/另行立项面)
- [x] perf-baseline.md 追加 N4 后基线行(保留旧值 + 日期标注)
- [ ] **三场真人冒烟**:流式跟滚 / 上滚读历史不被抢 / 回底按钮(记录到任务 journal)——**留主会话人工**(e2e 已有同语义自动化覆盖:spike 腿④ + virtualized-list 探针,人工场补体感验收)

### PR2 语义精修

- [x] estimateSize 调优(滚动条首滚收敛;profile.json 数字支撑)
  - 三态粗估(260/140/60)换**实测回归式**:临时探针(已删)对 10k fixture 全列表逐窗采样 + 子元素解剖得真值锚点(markdown 22px/行 × ceil(len/88)、座 28、user 气泡 +16、rocard 卡体 26/张、折叠思考 28/块、ghost user 残根 6px;timeline 行文本在 contentBlocks 的 text 块,m.content 为空须计入)。Σest 对真值 Σ:PR1 +74.8% → PR2 **−0.9%**(1,432,560 vs 1,445,917)。方法论陷阱:markdown 异步填充,首见采样读到骨架高(42px),真值须静默复测
- [x] `shouldAdjustScrollPositionOnItemSizeChange` 作为正式回退位接入调节(校正异常第一旋钮)
  - 选项槽位已接入 composable 的 useVirtualizer options(显式 undefined = 库默认策略,注释写明何时调与「不许 patch 库」约束);PR1 两个「不跳屏」探针实测默认策略正确,维持 undefined
- [x] **慢滚 10k 全程锚定漂移 ≤ 一行高**验收(e2e)
  - virtualized-list.spec.ts 新用例:rAF 节流慢滚(6000px/帧)顶→底→顶全程,固定哨兵(data-seq="5";ghost 行 .msg 高 0 不可作哨兵)静默基线 vs 回程后读数,|漂移| ≤ 320px(一行高)绿
- [x] 流式跟滚手感:force-follow 强制形态、上滚退出、AC2 逐项 e2e;F5 badge / reload / pending 边界补强
  - AC2 逐项(全部 e2e 绿):①流式中上滚不被抢 + 回底按钮出现(scrollTop 恒 0 + 按钮可见);②点回底瞬跳(behavior auto,PR1 单测钉)+ 视口外增长的段文本可见 + 后续 delta 重挂跟滚;③pending null→some 强制回底 = question-card-scroll.spec.ts#1 存量用例(虚拟化下语义完整,套内绿);④reload 落底无弹跳(done → reloadAfterFinalize 静默窗持续贴底)+ 会话切换落底 + 按钮复位(「今天」分组种子,更早分组默认折叠的坑);⑤F5 badge 变高跟滚:尾行带 ms 字段种子(活行 badge 在 done 与 reload 后都成立)
- [x] **AC1 达标增补(implement 外追加,f4 主刀)**:可见性单调核心缓存(WeakMap 缓存 + 尾行结构签名 tailSig,delta 期间 flatten 链全静默)+ vitest spy 重算计数断言(纯增长零重算 / 翻转恰好一次 / 结构数组增长触发重判)+ overscan 5→3
- [x] `pnpm test` / `pnpm test:e2e` / F1 复跑对 PR1 不回归
  - vitest 1943 绿(144 文件,+7)/ e2e 22 绿(18 存量 + 4 新增)/ build 绿 / bench 三档绿
  - **F1(10k median):f1 95.5 ✅(≤150;单 run 有 ~670ms 离群,疑 dev 冷模块/WSL2 尾流)/ f2 39.1 ❌(线 30;PR1 44.7 → −13%,常数成本分解落档,不再砍 overscan——露白风险)/ f4 293 ✅(≤500;PR1 859 → −66%,U4 链路已消,残余 = emit 往返地板 + 局部流式必要功)**;100/1k 档:f1 58.7/67.2,f2 35.5/36.2,f4 99/96;数字与归因见 perf-baseline.md §3 F1 PR2 行

### PR3 外围收尾

- [x] data-seq flash 高亮(AC4 后半)
  - 形态:主窗口复刻 CH12-1b 的 WAAPI 视觉(accent 22% → transparent,1400ms ease-out 单次;SearchPreviewBody 的 14%×3 是弹层内形态,不采用)—— 由 composable 消费 `pendingScrollSeq` 时记录命中 `message.id`(`flashKey`,SEARCH_FLASH_MS=1500ms 后自动清零)→ MessageList 在 wrapper 挂 `.search-hit` 类 → CSS keyframes(`msg-hit-flash`)承载,目标元素 = `.msg` 子根(与旧实现同元素;wrapper 带 inline translateY 定位,transform 不可占用)。background-color 无几何效应、不进测量缓存,不在 D4 白名单管辖面(白名单禁的是测量面几何属性)。e2e:virtualized-list data-seq 用例补 flash 类存在 + 4s 内摘除断言;vitest 补挂载/摘除/重复下令重点亮三断言
- [x] 动画 D4:run-enter-from 类(白名单 opacity+translateX,禁 scale/height)+ 容器 fade-in;reduced-motion 契约
  - 触发判据(composable 内 watch(virtualItems)):该 key 上窗不存在 && flatItems 增长(append) && 末项 runFirst && 在渲染窗口内;基线坑:`store.messages` 引用基线须 setup 期即取(watch 求值基线同时刻建立),否则热挂载后首个 append 被「整体替换」守卫误吞(首轮测试抓出)。整体替换(session 切换 / reload,数组换引用)与 assistant turn 归入已有 run(runFirst=false)均不动画,各有 vitest 负例
  - 相位机:from+active 同挂 → 双 rAF 释放(Vue TransitionGroup 内部同式)→ RUN_ENTER_ACTIVE_MS=300ms 全摘;参数沿用旧 TransitionGroup(--duration-slow 240ms / --ease-out / translateX(24px) 用户侧词汇)
  - 容器 fade-in:`.messages` 挂载触发一次性 opacity 动画(ChatPanel spinner v-if 使会话切换走重挂路径);reduced-motion 由 style.css 顶层 @media 兜底(时长压 0.01ms),两处动画即时呈现,组件侧零处理
  - AC6 断言状态:类挂载/移除决策 = vitest 4 例 + 相位时序 ✅;视觉动效本身 = 人工(ui-review 静态截图覆盖不了动效,方法局限既有实证)
- [x] SearchModal / SearchPreviewBody 回归确认(buildRunGroups 未动,预期零改,跑用例确认)
  - `utils/messageFormat.ts` PR3 零改动;`pnpm vitest run src/components/search/ src/utils/buildRunGroups.test.ts` = 20 用例绿(SearchModal 12 含 CH12-1b pendingScrollSeq 迁移断言 + buildRunGroups/flatten 8);SearchPreviewBody 无独立测试文件,其共用的 buildRunGroups 语义由上 8 例钉住,flash/动画改动全部落在主窗口 MessageList 一侧
- [x] 移动端 390px + `scripts/ui-review.sh --screenshots-only` 视觉对比 + WebKitGTK 桌面人工(U1)
  - e2e:virtualized-list 新增 390×844 用例(页面无横向滚动 + 渲染窗口内行右缘不越容器 + 回底按钮 44×44 + 避让位 right 8 / bottom 64 对 `.messages-wrap` 坐标系断言 + 点击回底落底);ui-review --screenshots-only 七张全出(20260920-023954),脚本零修改(选择器均在 MessageList 之外,虚拟化无失效面),dist 为 PR3 后构建
- [x] **人工清单**(实测回归另行立项):
  - WebKitGTK 桌面实机 U1(WSL2 web 面无法覆盖)—— **主会话/用户执行**
  - IME 组合输入 / 移动端键盘 / 容器 resize 钉底 U2(现状代码零处理,存量未知)—— **主会话/用户执行**
  - 三场真人冒烟(流式跟滚 / 上滚读历史不被抢 / 回底按钮;PR1 起挂账,e2e 已有同语义自动化覆盖,人工补体感验收)—— **主会话/用户执行**
- [x] AC6 动效人工验收;全量门:`pnpm test` / `pnpm test:e2e` / `pnpm build` / F1 终跑
  - vitest 1947 绿(144 文件,+4:flash 1 + enter 3)/ e2e 23 绿(+1 移动端)/ build 绿 / bench 三档绿
  - **F1 终跑(10k median):f1 81.7 ✅(≤150)/ f2 39.6 ❌(线 30,与 PR2 39.1 持平,结构性常数已有分解)/ f4 283 ✅(≤500)**;100/1k 档:f1 61.8/61.6,f2 28.4/32.2,f4 92/104;逐档对 PR2 ±噪声无回归(动画均一次性、不在热路径),数字与收口口径见 perf-baseline.md §3 F1 PR3 行
  - AC6 人工动效验收(新 run 划入观感 / 不打扰滚动位置 / reduced-motion 即时呈现)—— **主会话/用户执行**(自动化已钉:类相位 vitest + e2e 命中/避让几何)

## 验证命令速查

```bash
cd app && pnpm test                # vitest 全量
cd app && pnpm test:e2e            # Playwright(13 例 + PR0/PR1 新增)
cd app && pnpm bench:fe            # F1 三档(10k 档 3 run,每档独立 test 结构教训)
cd app && pnpm build               # vue-tsc + vite
```

## 风险文件与回滚点

| 文件 | 风险 | 回滚 |
|---|---|---|
| `app/src/components/chat/MessageList.vue`(重写) | 最高——滚动语义全换 | revert PR1;**回滚验收 = PR0 实现中立用例同套再绿**(评审定,否决双渲染 flag) |
| `app/src/components/chat/MessageItem.vue`(根 li→div) | 选择器回归面 | 同上 |
| `app/src/utils/messageFormat.ts`(增量) | 低(纯函数新增) | 保留无害 |
| `app/bench/render.bench.ts`(PR0 判据改造) | 判据变更换尺——必须留旧实现基线行 | 判据中立,无需回滚 |
| `app/e2e/question-card-scroll.spec.ts`(selector/ready) | 断言语义漂移 | e2e 单文件回滚 |

## Start 前检查

- [x] prd.md(裁定齐:路线/库/达标线/评审流程;AC1 双行口径 + AC4 拆半待同步)
- [x] design.md(评审结论回填:D1-D5 补强 + PR0-PR3 切分 + §9 U1-U5)
- [x] implement.md(本文件)
- [x] implement.jsonl / check.jsonl 真实条目(7+5 条)
- [x] 群聊评审收官(12 结论全 verified,已回填)
- [ ] 用户放行 → `task.py start`
