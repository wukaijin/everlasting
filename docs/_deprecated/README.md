# `_deprecated` — 失去时效的评审与快照

> 历史评审、smoke test 等当时有用的产物,使命已结束或被新版本取代。**只读不改**,作为考古层保留—— git 历史和 git log 仍是权威来源。

## 约定

- 目录:`docs/_deprecated/`,与 `_archive/` / `_reviews/` 平行的下划线前缀
- 与 `_reviews/` 的区别:`_reviews/` 是仍被主文档引用作为决策依据的评审;`_deprecated/` 是已无入站引用的快照
- 保留原因:可能仍有历史回溯价值(例如对比新旧方案),但已不构成"现行 source of truth"

## 当前条目

| 文件 | 类型 | 日期 | 归档原因 |
|---|---|---|---|
| [`PR3-SMOKE-TEST.md`](./PR3-SMOKE-TEST.md) | 手动冒烟测试 | 2026-06-15 | PR3 已合并,测试场景已固化进 vitest |
| [`REVIEW-sse-agent-loop-2026-06-12.md`](./REVIEW-sse-agent-loop-2026-06-12.md) | 代码审计 | 2026-06-12 | 已被 [`REVIEW-agent-loop-full-audit-2026-06-14.md`](../_reviews/REVIEW-agent-loop-full-audit-2026-06-14.md) 全盘覆盖 |
| [`REVIEW-tool-comparison-2026-06-12.md`](./REVIEW-tool-comparison-2026-06-12.md) | 竞品调研 | 2026-06-12 | 结论已被 `_reviews/` 内的 A2+B7 / 工具增强任务吸收 |