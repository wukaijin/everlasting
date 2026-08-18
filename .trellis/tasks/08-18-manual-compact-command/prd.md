# 手动 /compact 命令入口

## Goal

给用户一个主动收窗入口:`/compact` 命令(B3 command palette 内置命令框架先例:`/help` `/clear` `/new`),触发与自动路径同源的 LLM 摘要压缩(C3+,08-18 已落地)。

## Background / 前置

- 父任务(已归档):`.trellis/tasks/archive/2026-08/08-18-llm-context-compaction/`——摘要生成(`attempt_summary_compaction` / `build_compaction_prompt` / `compute_preservation_region` / `insert_compaction_summary`)与水位替换(`apply_compaction_watermark`)全部现成,手动入口 = 复用 + 触发点接线。
- 自动触发线 0.85×window;手动入口不受阈值限制(Claude Code `/compact` 同款:可带 focus instructions,不受熔断限制)。

## Requirements(草案,brainstorm 细化)

- R1 `/compact` 触发一次摘要压缩(复用自动路径的保留区/摘要/落库/回填);阈值无视;
- R2 可选 focus 参数(`/compact 聚焦 API 变更`)注入 prompt(Claude Code `/compact focus on ...` 先例);
- R3 压缩时已有自动摘要(水位)在场 → 增量合并(prior-summary)而非重摘要全历史;
- R4 熔断关系:手动触发不受连续失败熔断限制(Claude Code 同款),但仍走降级链(失败 → 机械丢组 → 报错给用户);
- R5 前端:摘要行最低渲染已存在(MessageItem kind 判定);本任务补命令入口 + 压缩中状态提示(可选);
- R6 低阈值场景:context 未超线时手动压缩也允许(用户主动收窗)。

## Non-Goals

- MAX_TURNS 软卡化(独立任务 `08-18-max-turns-softcap`);
- handoff 接力(独立任务 `08-18-handoff-mechanism`);
- chat 流内摘要 UI 卡片(自动+手动共用的展示升级,可与本任务同做或后置)。

## Acceptance Criteria(草案)

- [ ] AC1:`/compact` 在 palette 触发,无超线限制;
- [ ] AC2:带 focus 参数时 prompt 含定向指令(测试锁定);
- [ ] AC3:已有水位时走增量合并(prior_summary_seq 非空);
- [ ] AC4:摘要失败 → 机械丢组 → 用户可见反馈(非静默);
- [ ] AC5:前端命令分发(B3 内置命令分发链)+ 状态提示;
- [ ] AC6:mock 端到端 + live 验证(手动触发前后 turn_trace 对比)。

## Notes

- 依赖:需先 `task.py start`;brainstorm 决议(命令名 / focus 语法 / 触发点选 IPC 还是内置命令 body)。
- 关联 spec:agent-loop-architecture(pattern-llm-compaction)+ frontend chat(command palette 分发)。
