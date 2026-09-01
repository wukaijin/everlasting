# docs 开发规范

> 本目录沉淀 docs(`docs/` + 根 `STRUCTURE.md` + 决策日志 + `_history`)的维护规范。

## 规范索引

| 指南 | 描述 | 状态 |
|------|------|------|
| [number-snapshot-grep](./number-snapshot-grep.md) | 数值快照(IPC/AuditKind/表数)改动必须全仓 grep 旧值,先实测再落笔 | 2026-09-01 沉淀 |

## 何时读本目录

- 改 docs 数值、同步新特性文档时,先读本目录避免漏改
- done → 用 `wf-update-spec` skill 把本次决策 / 坑 / 新 pattern 写进来