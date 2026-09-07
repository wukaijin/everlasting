# planning review — 09-08-gc-c1-stoploss-regression-gate

> 评审门:Checker 子代理(跨模型规划评审),2026-09-08,对 prd/design/implement 三件套逐锚点核码。
> **总 verdict:PASS-with-notes** —— 核心机制(ask-free 单入口短路、TallySink 轮头检查、HaltReason::Budget、测试策略)经源码逐项验证健全可行;4 项 P1 事实错误/设计缺口 + 9 项 P2 已于当日全部折回三件套(见各文件「评审修正」标注)。

## P1(已折回)

1. **M4a「零改动透传」断言为假**:`GroupChatTaskConfig` 闭结构(scheduled_tasks.rs:64-68,未知键静默丢弃)+ fire 建群枚举白名单重建 metadata(scheduler/mod.rs:1032-1048)。→ R2/Out of Scope 改为「M4a 透传归 M4」。
2. **前端终态白名单有两处**:streamEvents.ts:151-61(handleChatEvent 早判,驱动定时场 toast + 跨客户端 finalize)与 644-650(done 处理器)。只改一处会致定时场无 toast / 跨客户端 activeRequests 悬挂。→ design §2.4 / Step 3 改「两处都加 budget」。
3. **R4 第一项清扫是已完成工作**:group_chat_loop.rs:60-66 round-robin 注释已由 `66ef6fa4`(2026-09-06)修复,PRD 引的是讨论进行时快照。→ Step 0 收缩,AC4 措辞收缩。
4. **background_escalation.rs:370 在 tests mod**:生产侧经 drive.rs:832 `permission_ctx.clone()` 自动继承 init ctx(群聊场继承 ask_free=true 恰是想要的)。→ design §1.1 改「零改动,仅测试 harness 补字段」。
5. **ChatEventSink 实为 10 方法**(4 必实现 + 6 默认,非「六个」);`has_live_observer` 必须显式转发(唯一生产覆写 = daemon HttpSseSink,漏转发破 GC3 观察者感知)。→ design §2.2 / Step 2a 改并加转发断言。

## P2(已折回)

锚点漂移修正(is_worker 767-768 / sink.clone 791 / 终态集 644-650);quality-guidelines.md 非空模板(4 条既填约定,改「追加」);GUI 编辑路径 updateGroupChatConfig 全量替换 metadata 会抹 token_budget(改合并保键 + 断言);AC2 措辞改「越线后下一轮头确定性终止」;overshoot 精确化(participant ≤20 / moderator ≤1);MCP/script 通道白名单措辞修正(加参归 M4);interrupted-resume 边界注记(SIGKILL 续跑理论可达 2×budget,M4 可选 checkpoint consumed 字段);clippy 门对齐 --lib;scheduledStopReasonLabel 可选 case。

## 核验通过(明确声明)

ask_path 唯一 Tier 4 入口(softcap/模式变更/问题卡片对群聊均不可达);worker 三调用点 is_worker=false;TallySink 对 ChatEventPayload(serde flatten)内省可行;三处内层 sink 传入点完备(编排器自身通知 usage=None 零贡献);resume 链路 ctx 重建、budget 声明跨 resume 存活;ToolDenied 多源复用语义(PermissionTimeout 反失真);turn_trace 孤儿列佐证;HaltReason/finalize/checkpoint 既有语义兼容(budget 终态自动删 checkpoint 行,不误入 preempt 收束轮);共识四项映射完整、无擅自扩大(Q1 为有记录的用户裁定);M4 待定决策(声明位置)无预占;基线数字(1652 精确 / 2343+ 合理)。

---

# implementation review — Checker 验收(2026-09-08)

> 对 4 个实施 commits(a54da415 / f7c56c2b / 7401de66 / a0ac1530)的逐项核码验收。
> **总 verdict:ACCEPT-with-notes**;1 P1 + 2 P2 + 1 P3 已于当日全部修复(commit `dc3660a2`)。

## 修复记录

- **P1-1**:TallySink 结构体 doc 仍写被否掉的 Done 数据源(实施修正漏改 doc,自违本任务 RULE)→ 改 TurnUsage 表述(group_chat_loop.rs:235)。
- **P2-1**:补 `tally_sink_forwards_has_live_observer` + `tally_sink_counts_turn_usage_not_done` 两单测,锁透传与「TurnUsage 单计 / Done 不计」语义。
- **P2-2**:AuditKind::ToolDenied doc 补「Tier 4 ask-free — group chat」来源。
- **P3**:checkpoint keep-or-delete 注释的终态枚举补 `budget`。

## AC 终判(Checker 核码 + 主代理全量门)

- AC1 ask-free ✅(短路零往返、契约文案、经典路径零改动)
- AC2 budget ✅(三破坏剧本 + 落库/Done 双断言 + 缺省零变更)
- AC3 前端 ✅(两处白名单 + notice/label + GUI 输入与合并保键,vitest 覆盖)
- AC4 RULE ✅(P1-1 修复后闭环)
- AC5 门 ✅(后端 2350 passed / clippy -D warnings 净 / 前端 1657 passed / vue-tsc 净 / fmt pre-commit 强制)

## 实施期重要发现(已折回文档)

**Done 不达 sink**:drive.rs 对工具中间轮不转发 Done,每 run_chat_loop 仅终态 Done 且只带末轮 usage——按 Done 累计会漏计「狂调工具」面并双计末轮。C1.2 数据源改为 `TurnUsage`(每内层 LLM 轮恰一次,与 turn_trace 同点同值),prd/design 已同步修正。
