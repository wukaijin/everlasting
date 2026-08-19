# MAX_TURNS 软卡化 — 撞线询问替代硬终断(去硬卡)

## Goal

去掉单聊 200-turn 硬终断:撞线时不再无条件 `stop_reason="max_turns"` 硬停,改为**软卡**(checkpoint)——复用 C2+ 主动干预(06-24)的 `QuestionStore` + 浮动询问卡机制,询问用户"继续 / 压缩后续跑 / 停止"。前提已满足:C3+ 摘要压缩(08-18,已归档)保证 context 可语义无损无限续。

## Brainstorm 决议(2026-08-19,用户确认)

| 决策点 | 决议 |
|---|---|
| 撞线阈值 | **保持 200**(`MAX_TURNS` 不动;软卡化后它只是首次询问边界) |
| 「继续」粒度 | **+200**(再给一整个预算,计数归零可再跑;400/600… 每次撞线再问,均有 audit) |
| 无响应兜底 | **10 分钟超时 → 停止**(与今日 max_turns 行为等价;unattended 不烧钱;超时期间可通过 Stop 取消) |
| 压缩 gate 关闭时 | **隐藏「压缩后续跑」选项**(条件构建 payload:gate 开三支 / gate 关两支,卡片不展示选了也无效的选项) |

## Background / 前置

- 父任务(已归档):`.trellis/tasks/archive/2026-08/08-18-llm-context-compaction/`(摘要压缩 + 水位 + 保留区存活,spec 见 agent-loop-architecture "pattern-llm-compaction")。
- 硬卡现状:`MAX_TURNS = 200`(agent/mod.rs:91-92),硬终断 `stop_reason="max_turns"`(chat_loop.rs:1055-1084);worker `SUBAGENT_MAX_TURNS = 200`(软终态 + C1 resume,不动);群聊 `MAX_ORCHESTRATION_ROUNDS = 30`(不动,后续独立评估)。
- 模板先例:C2+ 的 per-run-local `loop_hit_count` N=3 → `emit_tool_question` → 三臂 select(`token.cancelled()` / oneshot rx)(drive.rs:1597-1960)——本任务把同一结构挂到 `turn_limit` 撞线处,并增加超时臂。
- 撞线时序:上一轮的 tool results 已由 `finalize_turn` 落库,DB 尾部干净(无孤儿 tool_use);「停止」收尾与今日等价,用户发新消息即可续。

## Requirements

- R1 撞线语义:主 loop 撞线 → 询问卡片(继续 / 压缩后续跑 / 停止),不再无条件终止;worker 与群聊保持硬卡。
- R2 兜底:询问 10 分钟无响应 → 停止(与今日 `max_turns` 终态等价);pending 期间 Stop → `cancelled` 干净退出;oneshot dropped → 同 cancel 处理。
- R3 死循环防护不变:C2/C2+ 检测链、烧钱兜底层级不动。
- R4 范围:仅单聊主 loop;worker(有 resume)与群聊(30 轮编排)零回归。
- R5 观测:新增 `AuditKind`(参照 `LoopIntervention` 无 migration 先例),记录 asked / continued / compacted_continued / stopped / timeout_stopped / cancelled。
- R6 压缩联动:「压缩后续跑」→ 下一 turn **强制触发自动摘要压缩路径**(force flag 穿进 `drive_turn`,绕过 token 触发线但保留全部 gate);手动 `/compact` 入口是空闲期专用(seq=MAX+1 契约),loop 活跃时不可用,不复用。

## Non-Goals

- worker / 群聊上限调整;
- 手动 `/compact`(独立任务 `08-18-manual-compact-command`,已完成);
- handoff 接力(独立任务 `08-18-handoff-mechanism`);
- C2/C2+ 检测链与干预文案改动。

## Acceptance Criteria

- [ ] AC1:主 loop 200 轮撞线 → 弹浮动询问卡而非硬停;继续(+200)/ 压缩后续跑(force 压缩后 +200)/ 停止(今日 max_turns 等价终态)三分支行为正确;gate 关闭时卡片只有继续/停止两支。
- [ ] AC2:继续 → 预算 +200 可再跑(计数归零语义);停止 → 与今日 `max_turns` 终态等价的干净收尾(Done{max_turns} 一次、cwd/session 落库)。
- [ ] AC3:询问无响应 10 分钟超时 → 停止(有测试;超时经 env 可调以便测试);pending 期间 Stop → cancelled;oneshot dropped → cancelled。
- [ ] AC4:worker / 群聊路径零回归(硬卡原样;现有测试全绿)。
- [ ] AC5:mock 端到端(cargo 集成测试覆盖五分支)+ live 验证(turn-smoke 链路,撞线边界可经 env 调低实跑询问卡)。
- [ ] AC6:软卡事件有 audit 留痕(新 AuditKind,前端 audit log UI 可见)。

## Open Questions

(无 —— 四项决议已全部确认,见顶部决议表。)
