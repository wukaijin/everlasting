# `_reviews` — 设计评审 & 代码审计快照

> 项目设计文档和关键模块实现的外部评审快照。**只读不改**,保留作为历史参考——评审当时的设计可能已演进,以主目录文档和当前代码为准。

## 约定

- 目录:`docs/_reviews/`,与 `_archive/` 平行的下划线前缀
- 文件命名:`REVIEW-<topic>-<date>.md`(评审类型 + 日期,避免同名覆盖)
- 评审范围:设计评审覆盖文档;代码审计覆盖实现模块(稳定性 / 错误处理 / 并发 / 边界条件)
- 多份独立评审对照看更全面(不同模型角度不同)

## 当前条目

| 文件 | 类型 | 评审模型 | 评审日期 | 评审范围 |
|---|---|---|---|---|
| [`REVIEW-tool-comparison-2026-06-12.md`](./REVIEW-tool-comparison-2026-06-12.md) | 竞品调研 | — | 2026-06-12 | Tool 横向对比: Everlasting vs Claude Code / Open Code / Codex CLI / Cursor / Cline; 现有 7 tool 差距分析 + 缺失 tool 优先级 |
| [`REVIEW-sse-agent-loop-2026-06-12.md`](./REVIEW-sse-agent-loop-2026-06-12.md) | 代码审计 | Reasonix | 2026-06-12 | SSE 解析 → Provider → Agent Loop → 前端消费全链路: 稳定性 / 正确性 / 取消安全 / 错误处理 / 并发; 整体 ★★★★½ |
| [`REVIEW-b5-memory-grill-2026-06-10.md`](./REVIEW-b5-memory-grill-2026-06-10.md) | 设计复审 | Reasonix (grill-me) | 2026-06-10 | B5 Memory 设计: 概念混淆 / 注入频率 & 位置 / 命名策略 / 前端文本对齐; 9 题全决议 |
| [`REVIEW-glm-5.1.md`](./REVIEW-glm-5.1.md) | 设计评审 | GLM 5.1 | 2026-06 | 整体设计 / 架构 16 关卡 / 技术选型 / 实施路线图 / 风险管理 / 文档质量 |
| [`REVIEW-deepseek-v4-pro.md`](./REVIEW-deepseek-v4-pro.md) | 设计评审 | deepseek v4 pro | 2026-06 | 同上(两份独立评审,对照看更全面) |
| [`REVIEW-claude-opus-2026-06-09.md`](./REVIEW-claude-opus-2026-06-09.md) | 代码审计 | Claude Opus 4.8 | 2026-06-09 | 大文件重构计划 / 文档审阅 / 技术路线决策 / 项目结构全景图 |
