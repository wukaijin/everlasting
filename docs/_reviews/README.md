# `_reviews` — 设计评审 & 代码审计快照

> 项目设计文档和关键模块实现的外部评审快照。**只读不改**,保留作为历史参考——评审当时的设计可能已演进,以主目录文档和当前代码为准。

## 约定

- 目录:`docs/_reviews/`(原 `docs/review/` + `docs/_review/` + `docs/_reviews/` 三目录 2026-06-25 合并)
- 文件命名:`REVIEW-<topic>-<date>.md`(评审类型 + 日期,避免同名覆盖);少数早期文件保留原名(无 REVIEW- 前缀)
- 评审范围:设计评审覆盖文档;代码审计覆盖实现模块(稳定性 / 错误处理 / 并发 / 边界条件)
- 多份独立评审对照看更全面(不同模型角度不同)

## 当前条目

| 文件 | 类型 | 评审模型 | 评审日期 | 评审范围 |
|---|---|---|---|---|
| [`REVIEW-agent-loop-full-audit-2026-06-14.md`](./REVIEW-agent-loop-full-audit-2026-06-14.md) | 代码审计 | glm-5.2 (5 路并行深挖) | 2026-06-14 | Agent Loop 全链路 + 5 辅助功能(Permission/Mode · Memory · Provider/Model/SSE · Worktree/Git/Tools)全盘审视; 综合 ★★★★; 发现 3 安全 P0(shell env泄漏/不kill进程组/web_fetch SSRF绕过) + 2 C3 P0(orphan tool_result/超窗静默) + 跨子系统取消语义隐性依赖 + REVIEW-sse 债务 0 落地 |
| [`REVIEW-a2-b7-permission-mode-plan-2026-06-13.md`](./REVIEW-a2-b7-permission-mode-plan-2026-06-13.md) | 设计评审 | Reasonix | 2026-06-13 | A2+B7 权限系统 + 多模式计划审查; 发现 ⑨ 关 5 道顺序不一致 (blocker) + Per-Mode Tool List 遗漏 + IPC 异常路径缺失 |
| [`REVIEW-b5-memory-grill-2026-06-10.md`](./REVIEW-b5-memory-grill-2026-06-10.md) | 设计复审 | Reasonix (grill-me) | 2026-06-10 | B5 Memory 设计: 概念混淆 / 注入频率 & 位置 / 命名策略 / 前端文本对齐; 9 题全决议 |
| [`REVIEW-glm-5.1.md`](./REVIEW-glm-5.1.md) | 设计评审 | GLM 5.1 | 2026-06 | 整体设计 / 架构 16 关卡 / 技术选型 / 实施路线图 / 风险管理 / 文档质量 |
| [`REVIEW-deepseek-v4-pro.md`](./REVIEW-deepseek-v4-pro.md) | 设计评审 | deepseek v4 pro | 2026-06 | 同上(两份独立评审,对照看更全面) |
| [`REVIEW-claude-opus-2026-06-09.md`](./REVIEW-claude-opus-2026-06-09.md) | 代码审计 | Claude Opus 4.8 | 2026-06-09 | 大文件重构计划 / 文档审阅 / 技术路线决策 / 项目结构全景图 |
| [`FINDINGS-b5-cache-wire-validation.md`](./FINDINGS-b5-cache-wire-validation.md) | 实施验证 | — | 2026-06-11 | B5 cache wire 验证: 4 文件 cache_control 注入实测 + Anthropic prompt caching 行为确认 |
| [`b6-subagent-prd-review.md`](./b6-subagent-prd-review.md) | 设计评审 | Carlos (grill) | 2026-06-19 | B6 subagent PRD + 调研 review; PR1/2/3 实施前的设计复审 |
| [`b6-subagent-assessment.md`](./b6-subagent-assessment.md) | 代码审计 | Carlos | 2026-06-20/21 | B6 subagent 系统评估: PR1+PR2+PR3 + 2026-06-21 fix 全部落地代码; 工具配给 / Mode 结合 / 持久化 / 并发 / 降级债 5 维度评级 |

> 已移至 [`_deprecated/`](../_deprecated/) 的早期评审快照见该目录 README。
