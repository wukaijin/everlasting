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

| 档 | f1 mount(ms) | f2 滚动帧中位(ms) | f4 流式到上屏(ms) |
|---|---|---|---|
| n100 | 59.6 | 17.2 | 137 |
| n1000 | 160.2 | 19.6 | 1,178 |
| n10000 | 491.4 | **184.8** | **21,508** |

**N4 决策读数**:10k 档滚动帧 185ms(基线 17ms 的 11 倍,远超 16.7ms 流畅线)且**流式回放(20 delta)到上屏 21.5 秒**——裸 v-for 全量 DOM 在长会话下不仅打开慢,流式期间每 delta 的全列表 patch 使会话事实不可用。虚拟化路线(content-visibility vs 真虚拟化)对比即以本表为基线同参复跑。f4 与后端 h3(10k 整轮 155.6ms)构成前后端对子:流式上屏瓶颈在渲染侧(21.5s)而非 daemon 侧。

(测量边界:fake EventSource 逐条 emit 含 Node→页面的 evaluate 往返,f4 是端到端上界;结构教训 = 10k 档必须每档独立 test()——同 renderer 内 reload 累积 8 run 会崩 WSL2 Chromium。复跑一致性:同日两跑 f4@10k 均 21.5s / 滚帧@10k 185-195ms 稳定;mount/f4@1k 抖动 ±40%——再次佐证数字不进门禁、判定走 change detection 的裁定。)

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
