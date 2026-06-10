# `_reviews` — 项目级设计评审快照

> 项目整体设计(`DESIGN.md` / `ARCHITECTURE.md` / `TECH.md` / `IMPLEMENTATION.md` / `BACKLOG.md`)的**外部 LLM 评审快照**。
> **只读不改**,保留作为历史参考——评审当时的设计可能已演进,以主目录文档为准。

## 约定

- 目录:`docs/_reviews/`,与 `_archive/` 平行的下划线前缀
- 文件命名:`REVIEW-<model>.md`(模型名带版本,避免未来同模型升级后混淆)
- 评审范围通常覆盖:整体设计 / 架构 / 技术选型 / 实施路线图 / 风险管理 / 文档质量
- 多份独立评审对照看更全面(不同模型角度不同)

## 当前条目

| 文件 | 评审模型 | 评审日期 | 评审范围 |
|---|---|---|---|
| [`REVIEW-b5-memory-grill-2026-06-10.md`](./REVIEW-b5-memory-grill-2026-06-10.md) | Reasonix (grill-me interview) | 2026-06-10 | B5 Memory 设计复审: 概念混淆 / 注入频率 & 位置 / 命名策略 / 前端文本对齐; 9 题全决议 |
| [`REVIEW-glm-5.1.md`](./REVIEW-glm-5.1.md) | GLM 5.1 | 2026-06 | 整体设计 / 架构 16 关卡 / 技术选型 / 实施路线图 / 风险管理 / 文档质量 |
| [`REVIEW-deepseek-v4-pro.md`](./REVIEW-deepseek-v4-pro.md) | deepseek v4 pro | 2026-06 | 同上(两份独立评审,对照看更全面) |
| [`REVIEW-claude-opus-2026-06-09.md`](./REVIEW-claude-opus-2026-06-09.md) | Claude Opus 4.8 | 2026-06-09 | 代码审计: 大文件重构计划 / 文档审阅 / 技术路线决策 / 项目结构全景图 |
