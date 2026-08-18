# MAX_TURNS 软卡化 — 撞线询问替代硬终断(去硬卡)

## Goal

去掉单聊 200-turn 硬终断:撞线时不再 `stop_reason="max_turns"` 硬停,改为**软卡**(checkpoint)——复用 C2+ 主动干预(06-24)的 `QuestionStore` + `<AskUserQuestionCard>` 三分支机制,询问用户"继续 / 压缩后续跑 / 停止"。前提已满足:C3+ 摘要压缩(08-18,已归档)保证 context 可语义无损无限续。

## Background / 前置

- 父任务(已归档):`.trellis/tasks/archive/2026-08/08-18-llm-context-compaction/`(摘要压缩 + 水位 + 保留区存活,spec 见 agent-loop-architecture "pattern-llm-compaction")。
- 硬卡现状:`MAX_TURNS = 200`(agent/mod.rs:91),硬终断 `stop_reason="max_turns"`;worker `SUBAGENT_MAX_TURNS = 200`(软终态 + C1 resume,不动);群聊 `MAX_ORCHESTRATION_ROUNDS = 30`(不动,后续独立评估)。
- 模板先例:C2+ 的 per-run-local `loop_hit_count` N=3 → `emit_tool_question` → 三分支(`Done{loop_terminated}` / 继续+清零 / `Done{cancelled}`),`tokio::select!{token.cancelled(), oneshot}` 结构在 chat_loop 顶层——本任务把同一结构挂到 `turn_limit` 撞线处。

## Requirements(草案,brainstorm 细化)

- R1 撞线语义:MAX_TURNS 从硬终断 → 询问卡片(继续 / 压缩后续跑 / 停止),不再无条件终止;
- R2 兜底保留:询问超时/无响应 → 默认安全行为(倾向停止或续 N 轮再问);防"永远问不完";
- R3 死循环防护不变:C2/C2+ 检测链、烧钱兜底层级不动;
- R4 范围:仅单聊主 loop;worker(有 resume)与群聊(30 轮编排)不动;
- R5 观测:`AuditKind` 或 trace 记录软卡事件(参照 `LoopIntervention` 无 migration 先例);
- R6 压缩联动:选"压缩后续跑"→ 触发 C3+ 摘要压缩(复用 `attempt_summary_compaction` 路径或手动触发入口)。

## Non-Goals

- worker / 群聊上限调整;
- 手动 `/compact`(独立任务 `08-18-manual-compact-command`);
- handoff 接力(独立任务 `08-18-handoff-mechanism`)。

## Acceptance Criteria(草案)

- [ ] AC1:200 轮撞线 → 弹询问卡片而非硬停;三分支行为正确;
- [ ] AC2:继续 → 计数归零可再跑;停止 → 与今日 `max_turns` 终态等价的干净收尾;
- [ ] AC3:询问无响应兜底行为有测试;
- [ ] AC4:worker/群聊路径零回归(硬卡保持原样);
- [ ] AC5:mock 端到端 + live 验证(长跑不中断)。

## Notes

- 依赖:需先 `task.py start`;brainstorm 决议(撞线阈值是否保持 200 / 继续的粒度 / 默认行为)。
- 关联 spec:agent-loop-architecture(pattern-llm-compaction 同文件扩展或新增 pattern)。
