# E2 turn-level harness trace viewer

## Goal

把 agent harness 每轮(turn)的决策过程串成一条可视化时间线,既是 debug 工具(以后调 loop / 压缩 / 权限问题能看时间线),也是 harness 学习教具(项目灵魂="自研 agent core 学习 harness 工程")。ROADMAP 第三档 E2。

## Background

ROADMAP §2 第三档 E2 原文:"per-turn 决策时间线(context 构造 / C3 压缩触发 / token 分布 / C2 循环检测);复用审计日志 + token 用量。既是调试器也是 harness 学习教具"。

调研后确认:ROADMAP 画的 4 维度里,只有"工具执行 + 延迟"这一维有完整 per-turn 持久化数据,其余三维几乎都是缺口。这把 E2 的核心矛盾变成 scope 决策——动不动后端 trace 数据管道。

## Confirmed Facts(代码调研,2026-07-14)

### 已有 per-turn 持久化骨架(可直接用)

- **`messages` 表**(`db/sessions.rs:692` `persist_turn`):每轮 assistant 行带 `seq`(per-session 计数器)+ latency 4 列(`ttfb_ms/gen_ms/total_ms/thinking_ms`)+ `content` JSON(含 `ContentBlock::ToolUse{id,name,input}` / `ToolResult` / `Thinking` blocks,有 `tool_use_id` 配对)+ `has_tool_calls/has_tool_results` flag。turn 定义 = 一次 provider.send + 0..N tool execute + 一次 persist_turn。
- **`session_audit_events` 表**(`db/migrations.rs:465`,v5):`id(PK autoinc) / session_id / ts(TEXT 秒精度) / kind / payload_json`,索引 `(session_id, ts DESC)`。**无 turn_seq 列**。AuditKind 实际 25 变体(文档说 21 滞后),21 类落表、4 类 `WorkerAsk*` 死代码占位。写入全 best-effort。
- **`tool_executed` 审计**(`audit.rs:278`):payload `{tool_name, tool_input, duration_ms, exit_code}` — 唯一带工具完成指标的 variant。
- **查询 IPC**:`list_session_audit_events(session_id) -> Vec<AuditEventRow>`(`commands/permissions.rs:322`),全量下推,无分页/无 turn filter。
- **现有 `AuditLogModal.vue`**:session 级全量流(reka-ui Dialog),已有 `parseAuditPayload`(`utils/audit.ts:191`)判别联合 + 13 类 icon family(`AuditLogItem.vue`)。绑 `chatStore.currentSessionId`,非 turn 级。
- **`ChatEvent` 枚举**(`llm/types.rs:341`):现有变体都带"是否持久化"doc(如 `Retrying`/`Recall` 标 read-only non-persistent,`TurnComplete`/`FileInjections` 标持久化)。`TurnComplete{seq, ttfb_ms, gen_ms, total_ms, thinking_ms}` **不带 token**。
- **前端 in-memory per-turn**(`streamController.ts`):`RequestState.latencyByTurn: Map<turnIndex, TurnLatency>`、`recallHitsBySession`、`toolStartedAt` — 均不持久化,刷新即丢。
- **migration 最新 v6**(`db/migrations.rs:486`,2026-06-13 Mode 3 档化 backfill);下一个 v7。`run_migrations` 在 `migrations.rs:50`。
- **AppShell 布局**(`components/layout/AppShell.vue`):`AppHeader + body(Sidebar + main slot)`;独立面板挂这层。

### 数据缺口(ROADMAP 4 维度里缺的)

1. **C3 压缩触发 — 零持久化**。`compact_messages` 返回 `CompactResult{tokens_before, tokens_after, dropped_count, degradation}`(`context.rs:93`),只在 `chat_loop.rs:1261` `tracing::info!` + `StillOver` 分支 `tracing::error!`,不进 wire/DB/审计。`DegradationKind` 三态:None / NoCandidates / StillOver(后者必发 `ChatEvent::Error` 终止 turn)。
2. **per-turn token — 永久丢失**。`update_last_turn_usage`(`sessions.rs:411`)在每个 `ChatEvent::Done{usage}` 触发时 OVERWRITE session 表 5 个 snapshot 列,非累加;前几 turn 的 token 用量不可逆。session 表 4 个累计列已冻结(非破坏性 migration,production 不写)。
3. **C2 循环检测 — soft hint 不可见**。`loop_hit_count`/`loop_window` 是 `run_chat_loop` 局部变量(`chat_loop.rs:1205/1195`),跨 turn 累积,不在 struct/DB/event。1-2 连击 soft hint 只塞进下一轮 `tool_result` block(`chat_loop.rs:2181`);仅 ≥3 连击主动干预落 `loop_intervention` 审计(payload `{hit_count, verdict_kind, action, run_id?}`)+ `tool:question` 通道。
4. **workflow breadcrumb — 注入后丢弃**。`append_workflow_breadcrumb`(`inject.rs:343`)每轮把 `<workflow-task-meta>` Text block 注入 `messages[0]`,无 dedicated audit row / 无 chat-event / 无 per-turn snapshot。`WorkflowCtx.current_task` 在 IPC entry 缓存一次(`build_workflow_ctx`,`inject.rs:143`),loop 内不刷新。workflow tool 落 `tool_executed`;`request_task_state_transition` 还多落 3 个 `TaskStateTransition*` 审计 + `task:state:transition:request` event。from→to hook 不落审计,只在 `task.json.summary` 留 marker。
5. **审计表无 turn_seq 列**。turn 切片需前端从 `messages.seq` 或 `tool_use_id↔tool_result` 反推,或后端补列。

## Decisions(2026-07-14 brainstorm)

1. **Scope = 完整版**。后端补全 trace 数据管道 + 前端 viewer,补齐 4 维度全部缺口。跨层 + DB migration(v7)+ 动 agent core 多写点。
2. **Viewer = 独立面板(非 modal),live 跟踪当前 session + 事后回看历史 session**。约束后端:必须实时 emit ChatEvent(live 要求)+ 必须落盘(回看要求)。复用 `parseAuditPayload` + icon family;时间线主轴用 `messages.seq`。
3. **落盘 = always-on + 清理入口**。所有 session 都写 trace 数据,任何时候出问题都能回看。SQLite 本地写入开销可忽略;DB 膨胀靠清理入口。emit event 也 always-on,live 面板随时可用。
4. **[自定,review 可调] 粒度 = turn 级时间线为主轴 + tool-call 级点开细节**。turn 卡片汇总该轮 token/延迟/压缩/循环/breadcrumb;点开看该轮 tool calls(从 audit + messages content)。
5. **[自定,review 可调] MVP 边界**:后端管道全(4 维 emit + 落盘 + 审计 turn_seq)+ 前端面板(live + 回看全量时间线 + 维度分组渲染 + 失败高亮)。**筛选(按维度/turn/工具过滤)+ 导出(JSON)= Phase 2 OOS**。live 含在 MVP(emit always-on,前端监听新 event 增量更新即可,数据结构与回看统一)。
6. **[自定,review 可调] 任务结构 = parent + 2 child**:parent 本 task 拥有跨 child 验收 + 集成;child-1 后端 trace 管道(先做,child-2 依赖其 event + IPC + 数据结构);child-2 前端 trace 面板。

## Requirements

### R1 后端:4 维 trace 信号 emit + 落盘

- R1.1 新增 3 个 `ChatEvent` 变体(emit always-on,live 面板用):
  - `ContextCompacted { seq, tokens_before, tokens_after, dropped_count, degradation }` — 写点 `chat_loop.rs:1261`(现有 tracing 旁)。
  - `LoopHint { seq, hit_count, verdict_kind }` — soft hint(1-2 连击)写点 `chat_loop.rs:2181` 附近。
  - `WorkflowBreadcrumb { seq, task_slug, status, breadcrumb_text }` — 写点 `inject.rs:343` `append_workflow_breadcrumb`。
- R1.2 per-turn token 落盘:新表 `turn_trace`(每 turn 一行,统一多维 trace),含 `seq / session_id / token_usage_json / compaction_json / loop_hint_json / breadcrumb_json / created_at`。`Done{usage}` 时写 token 列,`persist_turn` 时以 seq 落行。
- R1.3 审计 `turn_seq` 列:`session_audit_events` 加 `turn_seq INTEGER NULL`(v7 migration),`record_audit_event` 传入当前 seq;历史行回填 NULL。
- R1.4 新 IPC `list_turn_traces(session_id) -> Vec<TurnTraceRow>`(回看历史)。
- R1.5 emit 的 3 个新 event 也落进 `turn_trace` 对应列(回看时与 live 同构)。

### R2 前端:独立 trace 面板(live + 回看)

- R2.1 新组件 `<TracePanel>`,挂 AppShell 层(可切换视图或第三栏,design 定)。
- R2.2 Pinia `useTraceStore`:currentSessionTraces(live,监听 3 新 event 增量)+ loadHistory(sessionId)(回看,调 `list_turn_traces` + `list_session_audit_events`)。
- R2.3 时间线主轴 turn seq,每 turn 一卡片:latency(已有)+ token 分布 + compaction(若有)+ loop(若有)+ breadcrumb + tool calls 点开。
- R2.4 复用 `parseAuditPayload` + icon family 渲染事件;失败高亮(`exit_code != 0` / `degradation=StillOver`)。
- R2.5 清理入口:按 session / 按时间删 turn_trace + 审计(R1.3 后审计带 turn_seq 可级联)。

## Acceptance Criteria

- [ ] AC1 后端 3 新 ChatEvent 变体在对应写点 emit,且 always-on(live 面板未开也 emit + 落盘)。
- [ ] AC2 `turn_trace` 表 v7 migration 创建,per-turn token 不再因 OVERWRITE 丢失历史(回看 N turn 前的 token 可见)。
- [ ] AC3 `session_audit_events.turn_seq` 列填充,审计行能按 turn 分组(不再需前端反推)。
- [ ] AC4 `list_turn_traces` IPC 返回历史 session 全 turn trace。
- [ ] AC5 `<TracePanel>` live 模式:当前 session 进行中实时显示每轮 trace(压缩/循环/breadcrumb/token)。
- [ ] AC6 `<TracePanel>` 回看模式:打开历史 session 显示其完整 turn 时间线。
- [ ] AC7 清理入口能删指定 session 的 trace 数据。
- [ ] AC8 `cargo test --lib` + `vitest` + `vue-tsc --noEmit` 全绿;现有审计/loop/compaction 行为零回归。

## Out of Scope(MVP)

- 筛选(按维度/turn/工具过滤)— Phase 2。
- 导出(JSON)— Phase 2。
- 4 类 `WorkerAsk*` 死代码审计变体(production 永不写,不补)。
- workflow from→to hook(spec distillation / preflight)独立审计行 — 维持 task.json.summary marker 现状。
- C2 循环 `loop_window` 滑动窗口中间态可视化 — 只 emit hit_count + verdict_kind 摘要。
- 独立 trace 数据导出/外部工具消费 — 内部 viewer 为主。

## Open Questions

- 无(genuinely blocking 已清零;自定项 D4-D6 标注 review 可调)。
