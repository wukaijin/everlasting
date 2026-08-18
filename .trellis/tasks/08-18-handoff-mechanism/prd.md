# handoff 跨 session 接力

## Goal

长任务跨 session 接续:把当前会话的语义压缩产物(摘要 + 状态)接力到新 session 继续跑——"压缩的落点变成新会话起点"而非原地收窗。与 C3+ 摘要压缩共享 90% 管道(摘要生成 / 保留区 / 增量合并)。

## Background / 前置

- 父任务(已归档):`.trellis/tasks/archive/2026-08/08-18-llm-context-compaction/`——摘要 prompt(handoff 话术,Codex "another LLM will resume" 前缀已在模板里)、`build_compaction_prompt`、`SummaryAnchor` 增量合并现成。
- 既有素材:worker C1 resume 机制(`Incomplete` 软终态 + 续跑,07-28);自动标题;memory 系统(跨 session 召回)。Amp 2025-10 曾主张"别压缩、开新线程接力"后又回归 —— 调研见 archived task research/04。
- 与手动 /compact 的关系:压缩产物原地落(`/compact`)/ 写成接力起点(handoff)是同一个管道的两个落点。

## Requirements(草案,brainstorm 细化)

- R1 接力动作:当前 session → 生成摘要(复用自动路径)→ 新建 session(继承 project/worktree)以摘要作为首条 context 继续;
- R2 摘要质量:handoff 目标下摘要必须含"当前工作状态 + 下一步"(模板已有 Work State / Next Step 段;接力时校验这两段非空,缺失则重试或提示);
- R3 来源可溯:接力会话标记 parent session 关联(metadata 或新列?);D2 搜索/审计可查;
- R4 用户驱动为主:MVP 不做自动接力(继续跑 >200 轮由软卡化任务决定);
- R5 原始会话保留:接力不清空原 session(落库无损原则沿用)。

## Non-Goals

- 自动接力 / 资源自动回收;
- 跨 project / 跨机器接力(跨设备同步是远期,BACKLOG §4);
- 群聊 / worker 接力(MVP 主 loop 单聊,scope gate 同 C3+)。

## Acceptance Criteria(草案)

- [ ] AC1:接力产出新 session,首条 context = 前缀 + 摘要,能正常续跑(mock 端到端);
- [ ] AC2:摘要含 Work State / Next Step(缺失 → 重试或明确报错);
- [ ] AC3:parent 关联可查(DB 断言);
- [ ] AC4:原 session 完好(行数不变);
- [ ] AC5:live 验证一次真实接力(重编 daemon 后)。

## Notes

- 依赖:需先 `task.py start`;建议排在软卡化 / 手动 compact 之后(共享管道先各自稳定)。
- 关联 spec:agent-loop-architecture(pattern-llm-compaction)+ database-guidelines(如加 parent 关联列需 migration)。
