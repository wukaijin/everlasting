# Perf Baseline — N9 性能基准基线与口径

> 任务 `09-19-n9-perf-benchmark`(2026-09-19)。本文件 = 首批基线 + 复跑口径 + 更新纪律。
> bench 源码:`app/src-tauri/benches/`(harness / db_bench / sse_bench + support + profile.json)。

## 1. 口径(不变量)

- **同机自比较**:所有数字只在同一台机、同一负载条件下对比;绝对值跨机无意义。每份新基线必须带 §5 环境头。
- **统计语义**:P50/P99 列只做指示值;**回归判定一律用 criterion change detection**(斜率 + 置信区间,`--save-baseline` 后复跑自动对比),不用裸阈值。
- **tempdir 只落 ext4**(/tmp);TMPDIR 指向 /mnt/*(9p 跨文件系统)时 bench fail-loud 拒跑。
- **数字不进 CI blocking 门禁**(用户裁定);CI 只跑**编译门**:`cargo check -p everlasting --lib --features bench --benches`(bench 代码防腐烂)。
- **迭代隔离**:h2/h3 等 true-落库组必须 `iter_batched(PerIteration)`(每 iteration 测量外重建 session,防行累积与 `UNIQUE(session_id, seq)` 撞)。
- **依赖方向收口**:benches 只准 `use everlasting_lib::bench_api`;F1 前端基准只准依赖 `app/e2e/fixtures.ts` world。
- 跑基准前:插电 / 无重负载 / 记录后台状况;`cargo bench` 走 release(bench profile)。

## 2. 复跑命令

```bash
cd app/src-tauri
PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo bench --features bench --bench harness -- --save-baseline <名>
# 同款 --bench db_bench / --bench sse_bench
# 首批基线名:n9-first(2026-09-19);后续复跑不传 --save-baseline 即与既有 baseline 自动对比
```

## 3. 首批基线(2026-09-19,baseline `n9-first`)

### B1 harness 开销(run_chat_loop 整轮,MockProvider 零网络,内存库)

| 组 | P50 | 场景 |
|---|---|---|
| h1_cold_text_turn | 20.7 ms | 空 session 纯文本一轮(冷启动) |
| h2_1k_history_text_turn | 31.2 ms | 预种 1k 消息后一轮 |
| h3_10k/history_text_turn | 155.6 ms | 预种 10k 消息后一轮(长会话启动) |
| h3_10k/history_text_turn_compaction_on | 159.7 ms | 同上 + llm_compaction_enabled(摘要路径含 1 次 mock LLM 调用) |
| h4_tool_round_trip | 36.9 ms | tool_use(list_dir 真执行)→ tool_result → 文本收尾 |

**归因**:h3(155.6)− b1_mem_10k(9.6)≈ **146 ms = context 组装 + tools schema + rehydrate 净开销**(10k 历史);DB 读只占 ~6%。h2−h1 ≈ 10.5ms(1k 历史增量)。压缩开关在 10k 档下成本 +4ms(未过触发阈值时主要走 token 估算)。

### B2 DB 读写(双档分表;N2 推算只认 disk 档)

| 组 | mem P50 | disk P50 |
|---|---|---|
| b1_load_session_1000 | 7.6 ms | 132.2 ms |
| b1_load_session_10000 | 9.6 ms | 139.1 ms |
| b2_persist_turn | 264 µs | 1.42 ms |
| b3_finalize_turn_persist(UPSERT) | 249 µs | 1.36 ms |
| b5_delete_suffix(50 行,1k/10k 表) | — | 1.79 / 1.68 ms |

**关键事实**:disk 档 `load_session` 是内存档的 **17 倍**(WAL 读);表 1k→10k 读成本只 +5%(固定开销主导)。**N2 成本模型**:turn 边界 auto-commit ≈ disk b2/b3 的 1.4ms/轮(每 turn 1-2 行)——完全可接受;revert 的后缀删除 <2ms 且与表大小无关。

### B3 SSE(降级形态:oneshot 内存面,无 TCP/网络栈,数字是下界)

| 组 | P50 | 度量 |
|---|---|---|
| b3a_sse_handshake | 4.8 µs | 路由 dispatch + handler + SSE 响应建立(不读 body——空 replay 首帧是 30s KeepAlive) |
| b3b_sse_replay_ttfb_1000 | 4.0 µs | 预填 1000 帧 + Last-Event-ID:0 → body 首块字节 |

**边界(如实标注)**:路径 B(live 首事件)未测——探针坐实 `build_provider` 字符串 dispatch 无 mock 臂,scripted MockProvider 无法经 providers 表/catalog 注入;给生产开 mock 注入机制超出测量任务边界。真 TCP/TLS/网络传输开销不在本 bench 面(F5 生产观测覆盖端到端)。

### F1 前端渲染(Playwright 真 Chromium,`pnpm bench:fe`,median;100/1k 档 8 run、10k 档 3 run)

> **判据变更(N4 PR0,2026-09-19)**:f1 mount 稳定判据 childElementCount 连续帧不变 → `.messages` scrollHeight 连续 3 帧 rAF 不变且 >0(虚拟化中立;childElementCount 在虚拟化下首帧即常数,评审推翻其效度)。10k 档结构同步改为**每 run 独立 test()**:旧「单 test 3 run + reload」结构在新判据下 2/2 崩(新判据使 renderer 存活 >4min,Execution context destroyed 死点漂移于 f2/f4;N9 时 3 run 尚可勉强存活是旧判据早退的假象)。f2/f4 度量定义不变,测量时点仍在 f1 稳定之后 —— 同序复跑即同参。

**旧判据基线(n9-first,2026-09-19,保留)**:

| 档 | f1 mount(ms) | f2 滚动帧中位(ms) | f4 流式到上屏(ms) |
|---|---|---|---|
| n100 | 59.6 | 17.2 | 137 |
| n1000 | 160.2 | 19.6 | 1,178 |
| n10000 | 491.4 | **184.8** | **21,508** |

**PR0 新判据旧实现基线(2026-09-19,同机同环境 §5;AC1 双行口径的基线行)**:

| 档 | f1 mount(ms) | f2 滚动帧中位(ms) | f4 流式到上屏(ms) |
|---|---|---|---|
| n100 | 54.7 | 17.0 | 146 |
| n1000 | 723.4 | 16.7 | 813 |
| n10000 | **67,912.6** | **82.5** | **17,742** |

**f1@10k = 67.9s 的机制注记(新判据揭出的旧实现真实布局成本,旧判据不可见)**:

- DOM 节点数在 mount 后即恒定(全页 162,937 节点、li.msg 8,000、run-group 3,000)——旧 childElementCount 判据因此首帧即收敛(491ms 的来源)。
- 但**高度是渐进实体化的**:mount 后存在一段波动长度的稳定窗(0–18s 实测),随后 scrollHeight 以 ~1k px/s 逐段增长(543 次变化、~600px/次),~50–70s 后收敛于 1,433,749 px(onset 时点有波动,收敛值三跑一致:67,912.6 / 67,926.7 / 67,912.6)。期间视图被钉底跟随(scrollTop 贴着 scrollHeight 长,Chrome scroll anchoring),Σ子元素高度与 scrollHeight 同步增长、溢出 delta 恒定。机制归因在 Chromium 对 16 万节点 / 1.4M px 页面的布局行为,属裸 v-for 实现面;虚拟化后 DOM 数量与列表长度解耦,该面整体消失(PR1 复跑同尺验证)。f1@1k 的 723ms 是同现象的小尺度版。
- f2@10k 从 184.8 变 82.5 是**测量时点效应**非改善:f1 判据换尺后,f2 落在完全实体化 + 布局缓存热的页面上(旧判据下 f2 跑在半实体化布局,首滚含未实体化内容的布局成本)。PR1 复跑同序测量,双行对比仍同参。f4 差异(21,508 → 17,742)同机同日波动范围内。
- 10k 档每 run 独立 test 后单 run 墙钟 ~2.3min(f1 68s + f4 18s),10k 档三 run ≈ 7min;数字不进 CI 门禁的裁定不变。

**N4 决策读数**:10k 档滚动帧 185ms(基线 17ms 的 11 倍,远超 16.7ms 流畅线)且**流式回放(20 delta)到上屏 21.5 秒**——裸 v-for 全量 DOM 在长会话下不仅打开慢,流式期间每 delta 的全列表 patch 使会话事实不可用。虚拟化路线(content-visibility vs 真虚拟化)对比即以本表为基线同参复跑。f4 与后端 h3(10k 整轮 155.6ms)构成前后端对子:流式上屏瓶颈在渲染侧(21.5s)而非 daemon 侧。

(测量边界:fake EventSource 逐条 emit 含 Node→页面的 evaluate 往返,f4 是端到端上界;结构教训 = 10k 档必须每档独立 test()——同 renderer 内 reload 累积 8 run 会崩 WSL2 Chromium。复跑一致性:同日两跑 f4@10k 均 21.5s / 滚帧@10k 185-195ms 稳定;mount/f4@1k 抖动 ±40%——再次佐证数字不进门禁、判定走 change detection 的裁定。)

**PR1 新实现行(N4,2026-09-19,@tanstack/vue-virtual 3.13.39 / virtual-core 3.17.11,同机同环境同尺)**:

| 档 | f1 mount(ms) | f2 滚动帧中位(ms) | f4 流式到上屏(ms) |
|---|---|---|---|
| n100 | 63.7 | 39.0 | 119 |
| n1000 | 69.7 | 37.9 | 186 |
| n10000 | **70.8** | **44.7** | **859** |

**AC1 判定(@10k 线 f4≤500 / f2≤30 / f1≤150)**:f1 ✅(70.8;旧实现同尺 67.9s,虚拟化后 10k mount 与 100 档同量级——「DOM 数量与长度解耦」的数字证据);f4 ❌ 859(线 500;对旧实现同尺 17,742 改善 95%,对旧判据 21,508 改善 96%);f2 ❌ 44.7(线 30;对旧实现同尺 82.5 改善 46%)。

- **f4 残余构成(临时探针实测)**:单 delta 两帧墙钟中位 ~59ms(其中 ~33ms 是 2×rAF 帧等待),实际主线程功 ~20-25ms/delta × 20 delta + 逐条 emit 的 evaluate 往返。归因与 design §9-6(U4)预判一致:visibleMessages→buildRunGroups→flatten 每拍全链重算(10k,虚拟项外的高频重算)+ 增长行 ResizeObserver 触发的测量版本失效,非 virtualizer 本身——PR2「流式跟滚手感」与 U4 立项面。
- **f2 残余构成**:虚拟化下滚帧成本 = 滚动触发的渲染窗口重挂(~10-13 个 MessageItem 实例,fixture 行高大,视口内可见 2-3 行 + overscan 5)。100 档同为 39ms → 成本与列表长度解耦(虚拟化生效),但窗口重挂常数高于旧全量 DOM 的布局缓存热路径(17ms@100)。overscan / MessageItem 轻量化 / 滚动条收敛属 PR2 面(estimateSize 调优 + shouldAdjustScrollPositionOnItemSizeChange 旋钮已列)。
- PR0 机制注记的验证:f1@10k 旧实现 67.9s 的「渐进实体化布局」面在新实现下整体消失(mount 70.8ms,三档同量级),f2 双行对比同序同参成立。

**PR2 行(N4,2026-09-19,同机同环境同尺;overscan 5→3 + estimateSize 实测回归式 + 可见性缓存链)**:

| 档 | f1 mount(ms) | f2 滚动帧中位(ms) | f4 流式到上屏(ms) |
|---|---|---|---|
| n100 | 58.7 | 35.5 | 99 |
| n1000 | 67.2 | 36.2 | 96 |
| n10000 | **95.5** | **39.1** | **293** |

**AC1 判定(@10k 线 f4≤500 / f2≤30 / f1≤150)**:f4 ✅(293;PR1 859 → −66%,对旧实现同尺 17,742 改善 98%);f1 ✅(95.5;单 run 出过 601.9/670.6 的离群值,PR1 无此现象,疑 vite dev 冷模块转换 / WSL2 调度尾流,median 判定不受影响);f2 ❌(39.1;PR1 44.7 → −13%,对旧实现同尺 82.5 改善 53%,30 线判定不可达,见分解)。

- **f4 859 → 293 的去向(主刀 = 可见性缓存链)**:PR1 形态 flatItems 依赖每条消息全部可见性字段,delta 原位追加 content 即 O(n) 全链(visibleMessages filter → buildRunGroups → flatten)重算,实测主线程 ~20-25ms/delta × 20 delta。PR2 改单调核心缓存(可见性输入分「单调核心」(WeakMap 缓存,filter 主循环对已可见行零字段读取)+「尾行结构签名」(tailSig computed,纯文本 delta 期间取值恒定 → Vue computed 值稳定语义不向下游传播)):流式 delta 期间整链静默(输出数组同引用,buildRunGroups/flatten 零重跑;vitest 有 spy 重算计数断言钉住)。可见性翻转事件(首 delta / tool push / error)仍精确各触发一次重判。残余 293ms = 23 次 emit 的 Node→页面往返地板(n100 同协议 99ms ≈ 4.3ms/事件)+ 每 delta 局部流式必要功(末行 markdown 重渲、ResizeObserver 测量、spacer 高度 + anchorTo:'end' 钉底调整),10k−100 差值 ≈ 9.7ms/delta,已与 session 长度基本解耦,非 O(n) 残余。
- **f2 39.1 的常数成本分解(30 线不可达,如实记录)**:f2 = 3 个瞬跳帧,每帧成本 = 渲染窗口整体重挂(overscan 3 下 ~9 个 MessageItem mount,各含 markdown/工具卡/思考块)≈ 3ms/行 + RO 测量 + 绝对定位布局。n100 35.5 ≈ n10000 39.1 → 与长度解耦的纯常数。overscan 5→3 收益 −15%(44.7→39.1);继续逼近 30 只剩 overscan 2/1(快速滚动露白风险,违「视觉正确性优先」裁定,不做)或 MessageItem 本体减重(组件树级工程,另行立项)。旧实现同尺 82.5(PR0 判据)/17(全量 DOM 布局缓存热,零重挂)——虚拟化以「滚动常数换 mount/流式解耦」的结构性代价在 f2 上如实呈现。
- **estimateSize 调优(滚动条首滚收敛)**:PR1 三态粗估(text 260 / tool 140 / thinking 60)Σest = 2,526,796px,对真值 Σ = 1,445,917px **+74.8%**(首滚 scrollbar 长度漂移的来源);PR2 换实测回归式(10k fixture 全列表逐窗采样 + 子元素解剖:markdown 22px/行 × ceil(len/88) + 座 28 + user 气泡 +16 + 紧凑 rocard 卡体 26/张 + 折叠思考 28/块 + ghost user 残根 6px;timeline 形态行的文本在 contentBlocks 的 text 块,m.content 为空,须计入)→ Σest = 1,432,560 = **−0.9%**。方法论教训:markdown 异步填充使「首见采样」读到骨架高(42px),行高真值必须静默后复测;卡行文本块不含在 wire `text` 字段/rehydrate 后的 `m.content` 里。

**PR3 行(N4 终跑,2026-09-20,同机同环境同尺;flash + D4 动画落地,零热路径改动的收尾 PR)**:

| 档 | f1 mount(ms) | f2 滚动帧中位(ms) | f4 流式到上屏(ms) |
|---|---|---|---|
| n100 | 61.8 | 28.4 | 92 |
| n1000 | 61.6 | 32.2 | 104 |
| n10000 | **81.7** | **39.6** | **283** |

**AC1 判定(@10k 线 f4≤500 / f2≤30 / f1≤150)**:f1 ✅(81.7)/ f4 ✅(283)/ f2 ❌(39.6;与 PR2 的 39.1 持平,30 线不可达的分解见 PR2 行,PR3 不改热路径,判定不变)。

- **对 PR2 无回归(逐档 ±噪声)**:f1 95.5→81.7(PR2 行已注明其单 run 离群,中位回到 60-80 带内)、f2 39.1→39.6(+0.5,run 间波动)、f4 293→283;100/1k 档同向持平。PR3 新增面均不在测量热路径上:flash = 单行 background-color 一次性动画(1400ms,无几何效应,不进 measureElement 缓存)、run-enter = 组首行 opacity/translateX 一次性过渡(240ms,合成器/非布局属性,白名单内)、容器 fade-in = 挂载一次 opacity 动画。f2@n100 本跑 28.4 越过 30 线属单跑噪声(线只判 @10k,PR2 分解结论不变)。
- **任务收口口径(N4 全程)**:10k 档 f1 67,912.6(旧实现同尺)→ 70.8(PR1)→ 95.5(PR2)→ 81.7(PR3);f2 82.5 → 44.7 → 39.1 → 39.6;f4 17,742 → 859 → 293 → 283。AC1 三线 @10k 终态:f1 ✅ / f4 ✅ / f2 ❌(结构性常数成本,已有分解,另行立项面)。

### F5 对照(harness 开销在真实端到端中的占比)

真实负载画像(见 §4):assistant 轮平均 total_ms = 12,965(n=865)。h1(20.7ms)/ 12,965ms ≈ **0.16%** —— LLM 网络+生成占绝对大头,harness 开销在单轮体感中可忽略;N9 数字的价值在**回归可见性**与**长会话场景**(h3 155ms / b1_disk 132ms 会在用户体感内叠加)。

## 4. 真实负载画像(2026-09-19,sqlite3 -readonly,94 session / 1720 消息)

- **Session 规模**:均值 18.3 条,P50<10(53 个 ≤10 / 40 个 11-100 / 1 个 101-1k / **0 个 >1k**,最大 156)。→ 种子档 100 贴近现实;1k/10k 是 N4/N2 前瞻档。
- **形态混合比**:thinking 43.5% / tool_use+result 对 40% / 纯 text 16.5%(→ `benches/profile.json` 已按此校准;此前草稿偏差大)。
- **文本长度**:均值 399 字符,最大 4,850。**tool_result content**:均值 2,709 字符,最大 126k。
- **F5 时延**:ttfb 均值 12.3s / total 均值 13.0s / max 162s。**tool duration**(n=209):均值 13.7s,max 555s(真实工具=长 shell/agent 任务,远重于 mock 场景)。

## 5. 环境头(首批基线)

| 项 | 值 |
|---|---|
| CPU | AMD Ryzen 5 5600(12 线程) |
| 内存 | 11 GiB |
| 内核 | 5.15.153.1-microsoft-standard-WSL2(x86_64) |
| 磁盘 | /tmp(ext4,WSL2 VHD) |
| 日期 | 2026-09-19 |

## 6. 更新纪律(触发器)

动以下路径的任务,check 阶段必须复跑对应 bench 面并与本文件基线对比(criterion change detection 报告截进任务 journal):

| 改动面 | 复跑 |
|---|---|
| `agent/chat_loop*` / `agent/context.rs` / `agent/chat.rs`(context 组装、turn 结构) | harness |
| `db/sessions/messages.rs` / migrations(消息读写路径) | db_bench |
| `daemon/sse.rs` / `daemon/routes/stream.rs`(SSE 通道) | sse_bench |
| `llm/provider` 构造/dispatch | harness + sse_bench |
| `MessageList.vue` / 消息渲染链 | F1(Playwright,PR3 落地后) |

数字显著回归(置信区间不含 0 且斜率 >+20%)时:先归因再合入;本文件随修随更新并保留旧值一行(日期标注)。

## 7. 种子 profile 单一出处

`app/src-tauri/benches/profile.json`(B2 `seed_session` 与 F1 `gen-fixtures.mjs` 同读);更新只改数字不改结构,改后复跑受影响面并更新 §3。
