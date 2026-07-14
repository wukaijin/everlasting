# E2 backend trace pipeline (child-1)

## Goal

后端 trace 数据管道:emit always-on + 落盘 + v7 migration + IPC。补齐 ROADMAP E2 的 4 维 trace 信号缺口(C3 压缩 / per-turn token / C2 soft hint / workflow breadcrumb)+ 审计 turn 对齐。parent = `07-14-e2-harness-trace-viewer`。

## 权威引用

- **技术设计**:`../07-14-e2-harness-trace-viewer/design.md`(§1 架构 / §2 数据模型 / §3 数据流 / §4 契约 / §6 兼容性 / §7 tradeoff / §9 风险)。
- **全局决策 + AC**:`../07-14-e2-harness-trace-viewer/prd.md`(Decisions 1-3 + R1 + AC1-AC4/AC8)。
- **执行 checklist**:`../07-14-e2-harness-trace-viewer/implement.md` §2(B1-B9)。
- **调研事实(写点/签名/schema)**:`./research/trace-data-gap-audit.md`。

## Scope(本 child = parent prd R1)

- R1.1 新增 3 `ChatEvent` 变体(emit always-on):`ContextCompacted` / `LoopHint` / `WorkflowBreadcrumb`。
- R1.2 `turn_trace` 表(v7)+ per-turn token 落盘(UPSERT 累积各列)。
- R1.3 `session_audit_events.turn_seq` 列(v7)+ `record_audit_event` 扩 turn_seq 参数(21 类调用点传 seq)。
- R1.4 `list_turn_traces` + `clear_session_trace` IPC。
- R1.5 3 新 event 也落 turn_trace 对应列(live/回看同构)。

**边界**:只动后端。不动 agent 决策逻辑(C3 降级/C2 干预/breadcrumb 注入行为不变,trace 旁路观测)。前端面板 = child-2,不在本 child。

## 依赖

- 无前置依赖(本 child 先做)。
- child-2(前端)依赖本 child 的 3 新 ChatEvent + list_turn_traces IPC + TurnTraceRow 数据结构。

## Acceptance Criteria(本 child)

- [ ] AC1 3 新 ChatEvent 变体在对应写点 emit,always-on(面板未开也 emit + 落盘)。
- [ ] AC2 `turn_trace` 表 v7 创建,per-turn token 不再因 OVERWRITE 丢失(回看 N turn 前的 token 可见)。
- [ ] AC3 `session_audit_events.turn_seq` 填充,审计行能按 turn 分组(不再需前端反推)。
- [ ] AC4 `list_turn_traces` IPC 返回历史 session 全 turn trace。
- [ ] AC8 `cargo test --lib` + 现有审计/loop/compaction 零回归;`cargo fmt` clean。

## Out of Scope(本 child)

- 前端面板 / store / 渲染(child-2)。
- 筛选 / 导出(parent Phase 2)。
- worker trace 隔离(is_worker 列,Phase 2;MVP 接受混入 parent session_id)。
- 4 类 WorkerAsk* 死代码审计变体。
