# F6 异步 agent 任务 — 执行计划

对应 design.md R1-R5。4 个 PR 串行(仅 PR2 依赖 PR1 字段),每个 PR 独立可回滚。

## PR1 — busy 字段后端(R1 后端半)

- [x] `db::SessionSummary` 加 `#[serde(default)] pub busy: bool`;grep 全部构造点补字段(测试构造优先 fixture 化,防 70 处机械补参的 ARGS-001 教训重演)
- [x] 抽 `enrich_busy(sessions: &mut [SessionSummary], state)` helper;daemon `routes/sessions.rs:39` 与 Tauri command `list_sessions` 双挂载
- [x] 集成测试:daemon route 层断言活跃 session busy=true、闲 session false、终结后 false(mock loop 起停)
- [x] wire 兼容测试:busy 字段序列化含 default(AC6)

验证:`cargo test -p everlasting --lib`(PKG_CONFIG_PATH 见 AGENTS.md)

## PR2 — 前端可见性 + 完成 toast(R1 前端半 + R2)

- [x] sessions store 挂 `serverBusy`(list_sessions 映射)+ **finalize 公共出口**处 `serverBusy.delete(sessionId)`(stale-busy 消解;不依赖 adoptForeignRequest 认领分支——评审 P3-2)
- [x] `SessionList.vue` 红点条件扩展(两处模板槽位 `:484`/`:577`)
- [x] `buildTurnFinishedNotification(sessionId, kind)` 纯函数(镜像 buildPendingNotification);挂 streamEvents 终结判定处;`error`/`done` 文案区分;群聊走既有 isTerminal
- [x] `turn_complete_notify_enabled` app_config 读出口:`routes/config.rs` 新增 `get_app_config` POST 路由(照具体命令先例)+ configStore 启动拉取一次缓存(评审 P3-3 定案)+ 关闭短路
- [x] vitest:冷启动红点(serverBusy)/foreign 完成清 busy/非当前 session toast/当前 session 抑制/error 文案/开关关/群聊中间轮不触发(AC1+AC2 全分支)

验证:`cd app && pnpm test` + `pnpm build`(vue-tsc)

## PR3 — F3 并发闸(R3)

- [x] `AppState.loop_permits`(启动读 `max_concurrent_loops`,缺省 4,parse 回退 4;注释钉死「改配置需重启」)
- [x] `chat_inner` spawn 闭包头 acquire;select! cancelled 早退臂按 pre-flight 回滚模板(chat.rs:429-432)完整回滚注册(cancellations / session_active_request(rid 守卫)/ inflight_exits)+ 补 cancelled 终态事件
- [x] **注释钉死两条不变量**:任何 `session_active_request.insert` 发生时本请求必在等闸或持闸(claim 即注册,等闸也亮 busy——有意语义);任何许可 drop 时对应注册必已注销。**禁止**把 acquire 挪进路由临界区(持全局 message_queues 锁 await = 队头阻塞 + 与 drain_all 死锁,见 design R3)
- [x] 集成测试:permits=2 + mock provider 三连发(不同 session),断言①三个 ChatAcceptance 均**立返** started;②等闸 session busy **即亮**(claim 即注册);③第 3 个 Start 延迟至首个许可释放且 FIFO(断言用事件序,不用 sleep);④等闸中取消 → cancelled 终态 + 注册全清(session 不被假在途卡死,可直接再发)
- [x] 回归:F1-A 队列用例、群聊、legacy 3a 顶替路径既有测试不红

验证:`cargo test -p everlasting --lib`;clippy/fmt

## PR4 — 关闭确认 + 文档(R4 + R5)

- [x] main.ts Tauri 初始化分支挂 `onCloseRequested`(守卫 `isTauriWebview()`,**非 transport 种类**——评审 P2;handler 外捕获 window 句柄,确认时复用 `destroy()`):busy 计数 = streamingSessionIds ∪ serverBusy(含等闸轮次);ConfirmDialog 复用
- [x] vitest:有 busy 弹窗/无 busy 直关/取消不退(PWA 分支跳过逻辑)
- [x] ROADMAP §1.2 F6 行 + §2/§4 状态更新(F3 最小档落地、F6 编排面落地、F1-C 归 F2 裁定并注明两个消费者:F2 cron + LLM detached dispatch);REMOTE-DEPLOY/HACKING「detach 边界」小节——两种关闭语义分开写清:Web/PWA 关闭(Ctrl+W/杀 App)不影响任务(standalone daemon 存活) vs Thin GUI 窗口关闭终止全部(有确认弹窗)
- [x] 全量收口:cargo 全量 + pnpm 全量 + vue-tsc + clippy + fmt + lefthook pre-commit 过

## Live 验证(全 PR 合入后)

- [x] `scripts/turn-smoke.sh` 一轮 LLM 实跑通过(改了 chat_inner spawn 链必须 live 验)
- [ ] AC1 手动:PC 发长任务 → 手机 PWA 冷启动见红点;AC2:切别的 session 收 toast、点击跳转;AC4:Thin GUI 关闭弹确认
- [ ] daemon 重启恢复:杀 daemon → 重启 → 在跑轮次标 interrupted(既有 recover 链路不受闸/字段影响)

## 风险点与回滚

| 风险 | 缓解 |
|------|------|
| `chat.rs` spawn/routing 临界区是 F1-A 刚落的热区 | PR3 单独成 PR;permits 调大即降级;时间断言用事件序 |
| SessionSummary 构造点漏补 | 编译器强保证(grep + 全量编译);fixture 化 |
| stale-busy 长静默悬挂 | MVP 接受(reload 兜底);live 验证观察,不行再评估焦点刷新 |
| 双 transport enrich 漂移 | 单 helper 双挂载 + 双侧测试(F1-A「路由口径统一」教训) |

## Spec 沉淀(收口时)

- `.trellis/spec/backend/agent-loop-architecture/` 新 pattern:`pattern-global-loop-semaphore`(信号量 + 排队取消终态契约)
- `.trellis/spec/backend/database-guidelines.md` 或 daemon spec:SessionSummary.busy 运行时态 enrich 模式(DB 恒 false,transport 层 enrich)
- frontend chat/state spec:serverBusy 消解契约(终结事件清 + reload 兜底)

## 流程注

- Inline 工作流(主线直做),Phase 2 经 trellis-before-dev 装载 spec;implement.jsonl/check.jsonl JSONL gate 不适用
- 复杂度:中(PR1/PR3 后端各 ~半天,PR2/PR4 前端各 ~半天 + live 验证)
