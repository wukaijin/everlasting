# N9 性能基准(criterion + Playwright 真浏览器)

## Goal

为应用的三条「用户直接感知延迟」主链路钉一组**可重复、受控、同机可比较**的性能基线,并固化成 bench 套件,使后续性能变化可对比、可归因。直接目的:为 N4(长会话渲染虚拟化)的路线决策(content-visibility vs 真虚拟化)供数;长期目的:回归可见性(现状性能退化完全静默)。

来源:BACKLOG 附录 B N9 候选(2026-09-05 群聊共识,P2,与 N4/N7 联动);N9 → N4 → N2(checkpoint 前置)依赖链的链头。

## Background

- **与 F5 时延追踪的边界**(勘察实证):F5(2026-06)已有生产观测——`messages.ttfb_ms/gen_ms/total_ms/thinking_ms` 四列 + per-tool `duration_ms`,但测量的是**端到端含真实 LLM 网络延迟**;harness 自身开销(context 组装 / tools[] schema 序列化 / memory 加载 / DB 读写)无任何受控测量,`load_session` 耗时无记录。N9 测 F5 测不到的 harness 开销面,互补不重叠。
- **现状零 bench 基建**(勘察实证):无 criterion 依赖、无 `benches/`;vitest 2.1.9 自带 bench 但 config 只收 `*.test.ts`;turn_trace 只有 token 维度无时延字段。
- **规划评审**:session `448f6601`(2026-09-19,review preset 四视角,14 分钟)12 条结论全采纳,重大修正 = bench 可见性方案重写(bench_api 再导出)、B2 双档(内存/disk)、B3 双路径重定义;裁决记录见 design.md §10。

## Requirements

**R1 B1 harness 开销 bench**(cargo criterion,`benches/harness.rs`):MockProvider 驱动 `run_chat_loop` 整轮;场景 h1(空)/h2(1k)/h3(10k)/h3b(单函数拆组:组装/rehydrate,MVP 必做)/h4(工具回路),×`llm_compaction_enabled` off/on 档;iter_batched 每 iteration 测量外重建 session(防累积污染与 UNIQUE(seq) 撞)。

**R2 B2 DB 读写 bench**(`benches/db_bench.rs`):内存档(查询/映射 CPU)+ **disk 档**(init_pool + ext4 tempdir,含 WAL/fsync)**分表**;量 load_session(1k/10k)、persist/finalize 写入、**b5 按 seq 删除后缀曲线**(N2 revert 成本);种子 = 入库 profile JSON 单一出处;**N2 推算只认 disk 档**(内存档明文禁用)。

**R3 B3 SSE 双路径 bench**(`benches/sse_bench.rs`):路径 A(replay TTFB)+ 路径 B(live 首事件);归因式 `B3(路径B) − B1(h1) = daemon HTTP 净开销` 落档;MockProvider config 可选性探针不通过则降级为 A + 差值分解并如实标注。

**R4 F1 前端渲染基准**(Playwright 真浏览器,`app/bench/` 独立目录 + config + `pnpm bench:fe`):f1 mount(终点 = MessageList stick-to-bottom 收敛)/ f2 滚动帧+longtask / f3(可选)JS heap / f4 流式回放(10k,与 h3 同源成对);median+P75+max,10-15 run;报告带人工检查列;种子与 B2 同读 profile JSON。

**R5 画像与 profile**:sqlite3 -readonly 拉真实分布(messages/session、F5 列、tool duration、内容形态混合比)固化为入库 profile JSON;B2/F1 双侧单一出处;PR1 草稿 → PR2 只改数字不改结构。

**R6 落档**:`.trellis/spec/backend/perf-baseline.md`(口径/环境头/统计语义【P99 指示值 + criterion change detection 判定】/基线表【内存 disk 分表】/归因式/复跑法/更新纪律触发器);spec 索引登记。

**R7 防腐烂**:CI 编译门(`cargo check --lib --features bench --benches` + bench 文件 typecheck;**数字不进门禁**,编译面进);依赖方向收口(bench 只 use bench_api;F1 只依赖 e2e/fixtures.ts world)。

## Acceptance Criteria

- [ ] `cargo bench -p everlasting` 一键复跑,B1/B2/B3 分布产出;iter_batched 隔离生效(h3 无 UNIQUE 撞/无行累积)
- [ ] 10k 消息档 load_session 有基线数字,内存/disk 分表;N2 推算口径写明只认 disk 档
- [ ] B3 双路径数字 + 归因式落档(或降级形态如实标注边界)
- [ ] `pnpm bench:fe` 产出三量级 + f4 报告,数字为真 Chromium(median+P75+max);vitest/e2e 门禁 include 零触碰
- [ ] profile JSON 入库且 B2/F1 同源消费;画像数字进落档附录
- [ ] CI 编译门绿(本地预演通过);默认构建(`cargo check` 无 feature)零变化
- [ ] perf-baseline.md 含环境头、统计语义、复跑命令;spec 索引登记
- [ ] `cargo test -p everlasting --lib` 全量回归绿(测试树零改动)

## Out of Scope

- 任何优化本身(归 N4 与后续任务;N9 只测量不修)
- 压测 / 混沌注入(N8 候选)
- 性能数字进 CI blocking 门禁(用户裁定不变;编译门不是数字门)
- LLM 真实网络延迟测量(F5 生产观测已覆盖)
- vitest bench 承担 N4 决策职责(jsdom 无布局引擎,已裁定 Playwright)

## Decisions(已裁定)

- **D1 前端基准形态 = Playwright 真浏览器**(2026-09-19 用户裁定):content-visibility 在 jsdom 不生效,字面「vitest bench」方案测不出 N4 要对比的差异。
- **D2 规划评审已跑**(session `448f6601`,review preset):12 条结论全采纳,三件套已按结论修订(见 design §10)。
- **D3 基线落档 = `.trellis/spec/backend/perf-baseline.md`**(新文件,仓库惯例)。
- **D4 N2 checkpoint bench 对象 = `finalize_turn_persist`**(2026-09-19 用户裁定,采纳推荐):N2 落地路径 = turn 边界 auto-commit + revert=reset,写路径即每 turn persist/finalize 落库;`upsert_group_chat_checkpoint` 是群聊 P1a 专属 UPSERT,与 N2 per-turn 基线语义无关。b 组对象 = persist_turn(b2)/ finalize_turn_persist(b3)/ 删除后缀(b5)。

## Open Questions

(无 —— 规划闭环,全部裁定完毕)
