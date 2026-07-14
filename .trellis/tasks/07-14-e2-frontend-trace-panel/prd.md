# E2 frontend trace panel (child-2)

## Goal

前端独立 trace 面板:useTraceStore(live+回看同构)+ streamController 3 新 event case + `<TracePanel>` / `<TurnTimeline>` / `<TurnCard>` + AppShell drawer 挂载 + 渲染复用 `parseAuditPayload` / icon family + 清理 UI。child-2,依赖 child-1 的 3 新 ChatEvent + `list_turn_traces` IPC + `TurnTraceRow` 数据结构。

## 权威引用

- **parent 决策 + AC**:`../07-14-e2-harness-trace-viewer/prd.md`(R2 前端 scope + AC5/AC6/AC7 + Decisions 1-6)
- **parent 技术设计**:`../07-14-e2-harness-trace-viewer/design.md`(§3.3 前端 live+回看同构 + §5 前端面板挂载/组件结构/渲染复用)
- **parent 实施 checklist F1-F9**:`../07-14-e2-harness-trace-viewer/implement.md §3`
- **child-1 产出**:3 新 ChatEvent 变体(`ContextCompacted` / `LoopHint` / `WorkflowBreadcrumb`,wire snake_case) + `list_turn_traces` + `clear_session_trace` IPC + `TurnTraceRow` 数据结构(camelCase)+ audit `turn_seq` 列

## Scope(本 child = parent prd R2 + AC5/AC6/AC7)

- R2.1 `<TracePanel>` 组件 + AppShell drawer 挂载(右滑入可折叠,默认收起)
- R2.2 Pinia `useTraceStore`:`currentSessionTraces: Map<seq, TurnTrace>`(live)+ `loadHistory(sessionId)`(回看)+ `clearSessionTrace` + `panelOpen`
- R2.3 `streamController.handleChatEvent` 加 3 case(`context_compacted` / `loop_hint` / `workflow_breadcrumb`)+ `startRequest` 清空
- R2.4 `<TurnTimeline>`(seq 主轴)+ `<TurnCard>`(latency + token 分布 + compaction + loop + breadcrumb + tool calls 点开)
- R2.5 复用 `parseAuditPayload`(`utils/audit.ts:191`)+ 13 类 icon family
- R2.6 失败高亮:`tool_executed.exit_code != 0` / `compaction.degradation == "still_over"` → 红边(复用 `audit-item--critical` class)
- R2.7 清理 UI:TracePanel header 清理按钮 → `clear_session_trace` IPC + 刷新

## Acceptance Criteria(本 child)

- [ ] AC5 `<TracePanel>` live 模式:当前 session 进行中实时显示每轮 trace(C3 压缩 / C2 soft hint / workflow breadcrumb / token)增量更新
- [ ] AC6 `<TracePanel>` 回看模式:打开历史 session 显示其完整 turn 时间线(经 `list_turn_traces` IPC + `list_session_audit_events` 按 turn_seq 归组)
- [ ] AC7 清理入口能删指定 session 的 trace 数据(TracePanel header 清理按钮 → `clear_session_trace` IPC + 刷新)
- [ ] AC8(局部)`pnpm build` + `pnpm vitest run` + `pnpm vue-tsc --noEmit` 全绿;现有 audit / subagent drawer 行为零回归

## Out of Scope(本 child)

- 筛选(按维度 / turn / 工具过滤)— Phase 2
- 导出(JSON)— Phase 2
- 4 类 `WorkerAsk*` 死代码审计变体
- `is_worker` 列隔离(worker trace 混入 parent session_id 已接受)
- worker trace 在主 chat live 面板的额外可视化(SubagentBufferSink 隔离已够)
- C2 `loop_window` 滑动窗口中间态可视化(只 emit hit_count + verdict_kind 摘要)
- 独立 trace 数据导出 / 外部工具消费

## 边界 / 不动

- 不动 child-1 已交付的 ChatEvent 变体 / IPC / 数据结构
- 不动 `parseAuditPayload` / `AuditLogItem.vue`(只复用)
- 不动 `usePermissionsStore` / `useChatStore` 主流程(只读 `currentSessionId`)
- 不引入新依赖(reka-ui drawer / framer / 等都不加,纯 Vue 3 transition + 原生 <Transition>)

## 依赖

- child-1 已完成并 commit(✓ commit `6120267`):3 ChatEvent 变体 + 2 IPC + TurnTraceRow

## 风险(详见 parent design §9 + implement §5)

| 文件 | 风险 | 缓解 |
|---|---|---|
| `AppShell.vue` | 布局改 drawer | 复原 `Sidebar + main`(机械 revert) |
| `streamController.ts` | handleChatEvent 加 case | 删 case |
| `useTraceStore` | Map<seq, TurnTrace> 状态 | 同 `recallHitsBySession` 模式,session 切换/startRequest 清空 |
| AuditEventRow turn_seq 字段 | child-1 已加 + 测试 pin | 直接读 `r.turnSeq` |
| wire snake_case vs camelCase | 后端 ChatEvent wire snake_case(`context_compacted`)/ TurnTraceRow camelCase(`tokenUsageJson`) | 类型按字段声明为准,不要混淆 |

## start 前 follow-up

- [x] child-1 已完成(commit `6120267` + spec 4 文档更新)
- [x] sub-agent dispatch:`implement.jsonl` / `check.jsonl` 已 curate 真实 spec/research 入口
- [x] 实施时 `trellis-implement` prompt 以 `Active task: .trellis/tasks/07-14-e2-frontend-trace-panel` 开头
- [ ] 实施时按本 child `implement.md` 的 F1-F9 有序推进
- [ ] 完成后 `trellis-check` 验 AC5/AC6/AC7/AC8 局部 + 零回归,再交 parent 统一收口
