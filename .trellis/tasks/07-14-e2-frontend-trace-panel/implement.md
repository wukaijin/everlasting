# E2 frontend trace panel — 执行计划(child-2)

> parent 权威技术设计在 `../07-14-e2-harness-trace-viewer/design.md` §3.3 + §5;checklist 详见 `../07-14-e2-harness-trace-viewer/implement.md §3`(F1-F9)。本文件为子 task 视角的执行入口 + 验证 + 风险。

## 执行 checklist(有序,F1-F9)

- [ ] **F1 类型 + store**(`app/src/types/turnTrace.ts` 新 + `app/src/stores/traceStore.ts` 新):
  - `TurnTrace` 前端统一类型:`{ id, sessionId, seq, tokenUsage?, compaction?, loopHint?, breadcrumb?, auditEvents?: AuditEventRow[] }`(tokenUsage/compaction/loopHint/breadcrumb 解析对应 `*_json` 字段为强类型子对象)
  - `useTraceStore`(Pinia):`currentSessionTraces: Map<number, TurnTrace>` + `loadHistory(sessionId)`(invoke list_turn_traces + list_session_audit_events,审计按 turnSeq 归组)+ `clearSessionTrace(sessionId)`(invoke clear_session_trace + 清 local state)+ `panelOpen: boolean` + `setPanelOpen(open: boolean)`
  - 类型与 TurnTraceRow(camelCase)+ AuditEventRow(turnSeq 字段)+ ChatEvent 3 变体(wire snake_case)对齐
- [ ] **F2 live 增量**(`app/src/stores/streamController.ts` `handleChatEvent`):
  - 加 3 case:`context_compacted` → upsert TurnTrace.compaction(seq 取 event.seq);`loop_hint` → upsert TurnTrace.loopHint;`workflow_breadcrumb` → upsert TurnTrace.breadcrumb
  - `startRequest` 清空 `currentSessionTraces`(同 `recallHitsBySession` 模式)
  - session 切换 watcher 清空
- [ ] **F3 回看**(`useTraceStore.loadHistory`):
  - 调 `list_turn_traces(sessionId)` 拉 turn_trace rows,逐行 parse `*_json` 字段为子对象 → `Map<number, TurnTrace>`
  - 调 `list_session_audit_events(sessionId)` 拉 audit rows,按 `turnSeq` 归组到对应 `TurnTrace.auditEvents`(null 归到"未分组"虚拟 seq)
  - 回看 vs live 共享同一 `TurnTrace` 类型,统一 `currentSessionTraces`
- [ ] **F4 组件**(`app/src/components/trace/` 新):
  - `TracePanel.vue`(drawer shell,reka-ui Dialog 或原生 transition,右滑入可折叠)
  - `TurnTimeline.vue`(seq 主轴,按 seq ASC 渲染)
  - `TurnCard.vue`(latency + token 分布 + compaction + loop + breadcrumb + tool calls 点开;失败高亮红边)
  - `TraceEventItem.vue`(复用 AuditLogItem 渲染 + 13 类 icon family;不重写)
  - 复用 `parseAuditPayload`(`utils/audit.ts:191`)判别 + icon 渲染
- [ ] **F5 挂载**(`app/src/components/layout/AppShell.vue`):
  - body 从 `Sidebar + main` 扩为 `Sidebar + main + <TracePanel>(drawer)`(同 SubagentDrawer 模式)
  - `app/src/components/layout/AppHeader.vue` 加 trace toggle 按钮(放审计入口旁)
  - 状态落 `useTraceStore.panelOpen`
- [ ] **F6 渲染复用**:
  - `parseAuditPayload` + 13 类 icon family 分桶(`AuditLogItem.vue` 已有,直接 import 渲染子组件或抽 props)
  - 失败高亮:`tool_executed.exit_code != 0` / `compaction.degradation == "still_over"` → 红边(复用 `audit-item--critical` class)
  - token 5 字段迷你条形图(纯 CSS + `utils/tokenUsage.ts` 色阶,不要引图表库)
- [ ] **F7 清理 UI**(`TracePanel.vue` header):
  - "清理" 按钮 → `useTraceStore.clearSessionTrace(currentSessionId)` → invoke clear_session_trace IPC → 清 local `currentSessionTraces` + 刷新(再次 loadHistory 或 trigger pull)
  - 二次确认用 ConfirmDialog(`popover-pattern.md` 已有,2026-06-11 加)
- [ ] **F8 测试**(`app/src/stores/traceStore.test.ts` 新 + `app/src/components/trace/*.test.ts`):
  - vitest store 测试:live 增量 3 case / 回看归组(turnSeq 存在 + 不存在)/ 清理 / session 切换清空
  - vitest 组件 smoke test:TracePanel 折叠 / 展开 / 空态 / loading
  - vue-tsc 0 err
- [ ] **F9 验证**:
  - `cd app && pnpm build`(vue-tsc --noEmit + vite build)
  - `cd app && pnpm vitest run`
  - 现有 audit / subagent drawer 行为零回归(vitest 全绿)

## 验证命令

```bash
# 前端
cd app && pnpm build               # vue-tsc --noEmit + vite build
cd app && pnpm vitest run          # vitest 全绿

# 后端配套验证(IPC 类型对齐)
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check
```

## 风险 / 回滚点(详见 parent implement §5)

| 文件 | 风险 | 回滚 |
|---|---|---|
| `app/src/components/layout/AppShell.vue` | 布局改 drawer | 复原 `Sidebar + main`(机械 revert) |
| `app/src/stores/streamController.ts` | handleChatEvent 加 3 case | 删 case |
| `app/src/stores/traceStore.ts` | 新 store | 删文件 + AppShell 复原 |
| `app/src/components/trace/*.vue` | 5 个新组件 | 删目录 + AppHeader 复原 |

## start 前 follow-up

- [x] child-1 已完成(commit `6120267`):3 ChatEvent 变体 + 2 IPC + TurnTraceRow 已就位
- [x] sub-agent dispatch:`implement.jsonl` / `check.jsonl` 已 curate 真实 spec/research 入口
- [x] 实施时 trellis-implement prompt 以 `Active task: .trellis/tasks/07-14-e2-frontend-trace-panel` 开头
- [x] 行号:parent design §5 给出 `parseAuditPayload` 位置 `utils/audit.ts:191` + AuditLogItem.vue + audit-item--critical class;实施前以代码为准复核
- [ ] 完成后 trellis-check 验 AC5/AC6/AC7/AC8 局部 + 零回归,再交 parent 统一收口
