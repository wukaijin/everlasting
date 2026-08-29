# P3 技术债打包清理(08-30)

## 背景

DEBT.md open 集合 9 条 P3(2026-08-27 盘点登记)。本批目标是把台账清到只剩确需独立任务的条目。2026-08-30 逐条源码考证后发现 **4 条已自愈或大幅缩水**,登记时的描述与现状不符:

| 条目 | 登记时 | 2026-08-30 考证现状 |
|---|---|---|
| RULE-BUILD-001 | manualChunks 方案搁置 | **已实现**:vite.config.ts 六分包(vendor-vue/reka/editor/markdown/icons/misc);实测 build 零告警,最大 chunk ChatView 403 kB < 800 kB 限 |
| RULE-DOC-002 | 三处漂移 | 两处自愈:state-management.md 08-27 重写后旧 :517 引用失效、无内联编辑残留;docs/TECH.md §1.4 已描述手写 parser 与实现一致。**只剩 subagent/mod.rs:28**(注释 `max_turns: Some(20)` vs 实际 `SUBAGENT_MAX_TURNS = 200`,dispatch.rs:79) |
| RULE-DOC-001 | drive.rs 注释 + CLAUDE.md 状态段 | CLAUDE.md 状态段已由 b400ba6(叙事文档治理 WP1)删除。**只剩 drive.rs 任务名+日期注释**(RULE-PERSIST-001/WP2/08-20-* 等 ~15 处) |
| RULE-TEST-001 | mode change DB 链路零覆盖 | pure core `resolve_mode_change_internal` 抽取时已落 `commands/tests_resolve_mode_change.rs` 6+ 用例(allow/deny/root-guard/unknown-session/unregister)。**只剩 playwright 选型评估 + 一条过时 TODO 注释**(tests_request_mode_change.rs:32 还说"commands/ tests 是 future task") |

## 范围(6 项实事 + 3 项纯销账)

1. **RULE-DOC-002 残件**:subagent/mod.rs:28 模块文档注释 20 → 200(与 `SUBAGENT_MAX_TURNS` 一致)。
2. **RULE-DOC-001 残件**:drive.rs 内「任务名+日期(+WP)」前缀注释收敛 —— 保留功能性/不变量说明,历史归属信息删除(git log 是只读稳定副本);若删前缀后无实质内容则整行删。
3. **RULE-HEALTH-001**:删除 `session_count` 哨兵(定案:**删而非接线**)。考证:`session_count` 零消费方(仅 health.ts:40 接口声明 + health.test.ts:37 fixture);health handler 的 stateless 设计是 Q1 端口探测的刻意选择,接线 AppState 违背该设计且无人需要 detailed 变体。改动面:HealthResponse 删字段 + health() 删 -1 + 两段 TODO(P2.5) 注释收敛 + health.rs 内联 tests + everlasting-daemon.rs:35 文档行 + health.ts:40 + fixture。
4. **RULE-FE-002**:confirmYolo resolve-after-success catch(:225 附近)与 cancelYolo catch(:268 附近)从 console-only 接入提示通道 —— 镜像同函数第一个 catch 的 lazy-import(`useErrorBus.extractErrorMessage` + `projectsStore.showToast`)模式,warn 级(模式已切成功/cancel 已关弹窗,resolve 失败只意味着 agent 侧 oneshot 悬挂至超时)。Session 46 曾倾向 by-design 销账,但「agent 等待卡可能滞留且用户无感知」值得一枚非阻塞 warn toast;两处同修。
5. **RULE-SHIM-001**:
   - 删 `get_pending_question` deprecated IPC 全链:Tauri 注册(lib.rs:447)+ daemon 路由(routes/question.rs:89-93,:127)+ http.ts:147 映射 + `GET_PENDING_QUESTION_CMD` 常量(questionCards.types.ts:99)+ 命令壳(question.rs:530)。`get_pending_question_inner` 若因此失去调用方则一并删。
   - 删 `test_provider` 命令壳 + lib.rs:275 注册(无 daemon 路由,默认 httpTransport 下本就不可达;前端 ProvidersTab 注释自证改走 test_model)。
   - messageTimeline 双渲染路径**定性不删**:考证 streamController.ts:715-733 实时态已就地填 contentBlocks,回退路径只剩旧 DB 行(无 contentBlocks 的历史消息)数据兼容;`:20-23` 的「实时流式态为 false」注释已过时需修正,并注明回退的下线条件(需 DB backfill,非本批)。
6. **RULE-ALLOW-001**:摘三处模块级 `#![allow(dead_code)]` 伞 —— db/memories.rs:42(伞注释自承诺「P2 落地后替换为逐项」,P2-P5 已全部上线)、db/memories/types.rs:7、agent/workflow/task.rs:79。编译驱动逐项定性:有调用者自然消除;真预留面逐项 allow + 真实理由注释;孤儿删除。subagent/event_sink 等既有逐项 allow 只审计调用方现状,不重复劳动。
7. **RULE-TEST-001 残件(注释)**:tests_request_mode_change.rs:32-38 TODO 更新为指向已存在的 tests_resolve_mode_change.rs。
8. **销账×3 + 台账修正**:RULE-BUILD-001(附 build 实测证据)、DEBT.md 尾表「优先级分布」stale 修正(当前写着 P2:1/P3:10/Total 11,实际 P2=0)。

## 非目标

- RULE-PERM-001(审计事件分页,1-2 天)—— 留 open,单独任务。
- playwright 选型评估(RULE-TEST-001 真残件)—— 留 open 或降级。
- messageTimeline 回退路径移除(需旧数据 backfill,产品决策)。
- 全仓任务名注释清理(本批只动 DEBT 点名的 drive.rs)。

## 验收标准

- AC1:`cargo test -p everlasting --lib` 全绿;`cargo clippy -p everlasting --lib` 零新增告警;fmt 干净。
- AC2:`pnpm test` 全绿 + `vue-tsc --noEmit` 绿;`pnpm build` 零 chunk 告警。
- AC3:HEALTH-001 后全仓 `session_count` 引用仅剩语义无关命中;health 内联测试同步更新。
- AC4:SHIM-001 后前端 `GET_PENDING_QUESTION_CMD`/`test_provider` 零引用,后端注册表与 daemon 路由同步删除。
- AC5:ALLOW-001 摘伞后,memories/workflow 域内每一处保留的 `allow(dead_code)` 均带真实理由;删除项无悬空引用。
- AC6:FE-002 两个新 toast 分支有 vitest 用例(镜像既有 chatMode 测试的 mock 模式)。
- AC7:DEBT.md open 集合 9 → 2(PERM-001 + TEST-001 残件),尾表与正文一致;已闭合条目按维护说明从文件删除,闭合说明进 git log。
