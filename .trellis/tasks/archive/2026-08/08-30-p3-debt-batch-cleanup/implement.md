# 执行计划

> 顺序原则:先纯文档(零编译面)→ 后端 wire/注册删改 → 摘伞(编译驱动)→ 前端 → 全量回归 → 销账提交。每步后跑增量验证,失败即停。

## Step 1 — 文档类(RULE-DOC-002 残件 / DOC-001 残件)

- [x] subagent/mod.rs:28:`max_turns: Some(20)` → `Some(SUBAGENT_MAX_TURNS)`(= 200)
- [x] drive.rs 任务名注释 sweep:grep 命中行(84/244/376/640/996/1180/1450/1492/1525/1606/2041/2652)逐处审 —— 保留功能/不变量句,删「RULE-xxx (task-slug) / WP2 / 08-20-xxx」历史前缀;空洞则整行删
- 验证:`cargo check -p everlasting`(注释改动也应过编译)

## Step 2 — RULE-HEALTH-001 删哨兵

- [x] health.rs:HealthResponse 删 `session_count` 字段 + 两段 P2.2/P2.5 注释收敛(保留 Q1 stateless 设计依据)+ health() 删 `session_count: -1` + 内联 tests 同步
- [x] everlasting-daemon.rs:35 文档行删 sessionCount
- [x] health.ts:40 删 `sessionCount?: number`;health.test.ts:37 删 fixture 行
- 验证:`cargo test -p everlasting --lib "daemon::routes::health"`;`pnpm test health`

## Step 3 — RULE-SHIM-001 删两个 deprecated IPC

- [x] question.rs:删 `get_pending_question` 命令壳(:530);评估 `get_pending_question_inner` 调用方(routes/question.rs:18,93)→ daemon 路由删后 inner 无调用方则同删;模块头注释(:29 提及)同步
- [x] routes/question.rs:删 handler(:89-93)+ 路由注册(:127)+ use 导入
- [x] lib.rs:447 删注册 + 相邻注释;lib.rs:275 删 test_provider 注册
- [x] providers.rs:删 `test_provider`(:377)+ 头注释(:2-3,:326,:460-462 提及处修正)
- [x] http.ts:147 删映射;questionCards.types.ts:99 删 `GET_PENDING_QUESTION_CMD` + :91 注释修正
- [x] messageTimeline.ts:20-23 注释修正(实时态已填 contentBlocks;回退 = 旧 DB 行兼容,下线条件 backfill)
- 验证:`cargo test -p everlasting --lib`;grep 全仓零残留;`pnpm test` + vue-tsc

## Step 4 — RULE-ALLOW-001 摘伞

- [x] 摘 db/memories.rs:42 伞 + 「Dead-code policy」注释段重写为逐项策略
- [x] 摘 db/memories/types.rs:7 伞
- [x] 摘 agent/workflow/task.rs:79 伞
- [x] `cargo check -p everlasting 2>&1 | grep dead_code` 逐项定性:有调用者 → 消除;真预留 → 逐项 allow + 理由;孤儿 → 删
- 验证:`cargo test -p everlasting --lib`(全量)

## Step 5 — RULE-FE-002 toast

- [x] confirmYolo resolve-after-success catch + cancelYolo catch:lazy-import showToast + extractErrorMessage,warn 级文案(「模式已切换,通知 agent 失败」/「已取消,通知 agent 失败」语义)
- [x] vitest 用例 ×2(镜像既有 chatModeActions 测试 mock:transport invoke fail 注入)
- 验证:`pnpm test chatMode`(或对应文件)

## Step 6 — RULE-TEST-001 残件注释

- [x] tests_request_mode_change.rs:32-38 TODO 更新:指向 commands/tests_resolve_mode_change.rs,删除"future task"措辞

## Step 7 — 全量回归

- [x] `cargo test -p everlasting --lib`(PKG_CONFIG_PATH 见 AGENTS.md)
- [x] `cargo fmt --check` + `cargo clippy -p everlasting --lib`
- [x] `pnpm test` + `pnpm build`(零 chunk 告警,BUILD-001 销账证据)

## Step 8 — 销账 + 收尾

- [x] DEBT.md:删 7 条(BUILD-001/DOC-002/DOC-001/HEALTH-001/FE-002/SHIM-001/ALLOW-001;TEST-001 改写为仅剩 playwright 评估),尾表 9→2 修正
- [x] spec 沉淀(若有契约级新知:如 health wire 字段删除的兼容原则、deprecated IPC 下线流程)
- [x] journal(add_session.py)+ commit + task.py archive

## 回滚点

- 每 Step 独立可回滚(纯文档/wire 删改互不依赖);Step 4 摘伞若 fallout 超预期(>30 项难定性),恢复三把伞中的 workflow/task.rs 并在 DEBT 重登记缩窄条目。
