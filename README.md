# Everlasting

> 个人使用的 vibe coding workbench。基于 Tauri + 自研 agent core,WSL 优先。

## 这是什么

一个桌面应用,给"在 WSL 里写代码的 Windows 用户"用的 vibe coding 工作台。

不是另一个 Claude Code 替代品,而是同样的能力(聊、改代码、跑命令)加上:
- **自研 agent core** — 为了学习 harness engineering,不用 SDK 包装
- **深度 WSL 集成** — 项目放 WSL 内部,不走 `/mnt/c`
- **多项目 / 多 session / 工作流** — 不是一次性对话,是持久工作环境

## 当前状态 (2026-06-10)

权威看 `git log --oneline -20`,**路线图与下一步选项归 [docs/ROADMAP.md](./docs/ROADMAP.md)**(V2 4 档分类 + 已实施粗粒度归类,本文档不重复)。已知 issue:bug 1+2 position 在 RDP 双显示器下未完全修好。

## 代码结构

完整结构见 [STRUCTURE.md](./STRUCTURE.md)(8-PR5 创建)。简化版:

```
app/
├── src/             # Vue 3 前端 (8-PR3 拆 sub-components)
├── src-tauri/src/   # Rust 后端 (8-PR1/2 拆 state/commands/agent/db/)
├── docs/            # 设计文档 (全中文)
└── spikes/          # 技术验证记录
```

## 文档

设计文档在 [`docs/`](./docs/),按"需求/路线图/架构/技术/决策档案/候选"6 维拆分。

| 文档 | 看什么 |
|------|--------|
| [docs/README.md](./docs/README.md) | 索引 + 必读参考学习清单(参考但不抄) |
| [docs/ROADMAP.md](./docs/ROADMAP.md) | **技术路线图(单一 source of truth)** — V2 4 档分类 + 已实施归类 + 维护承诺 |
| [docs/DESIGN.md](./docs/DESIGN.md) | 项目能力边界 + 硬约束(明确不做) |
| [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) | 系统怎么搭、16 关卡请求生命周期 |
| [docs/TECH.md](./docs/TECH.md) | 锁定了哪些库(自研 Provider trait / rmcp / git2-rs / sqlx / nucleo 等) |
| [docs/IMPLEMENTATION.md](./docs/IMPLEMENTATION.md) | 决策档案 — §1 自研 agent core 决策 + §4 ADR 决策日志(只追加) |
| [docs/BACKLOG.md](./docs/BACKLOG.md) | 7 个候选功能的技术评估(排期归 ROADMAP) |

**第一次接触推荐顺序**:DESIGN §3 → ROADMAP §1-2 → ARCHITECTURE §1-2。

## 路线图

完整 V2 4 档分类 + 已实施项 + 移除项 + 维护承诺统一归 [docs/ROADMAP.md](./docs/ROADMAP.md),本文档不重复。

## 关键决策(为什么)

- **不用 SDK 包装 Claude Code / Codex** — 学不到 harness 核心
- **WSL 优先,Windows 不主动适配** — 个人使用场景就是 WSL
- **每个 session 一个 git worktree** — 多 session 并行 / 互不污染 / 切换瞬时
- **MCP 只外暴露,内部通信不绕** — 内部直接调 Rust 函数最快
- **SQLite 是唯一存储** — 单文件、零运维、FTS5 搜索
- **不做 workflow 编排 / 不做团队协作 / 不做云端部署** — 个人工具,这些是另一个产品的事

完整决策日志见 [IMPLEMENTATION §4](./docs/IMPLEMENTATION.md#4-决策日志)。

## 约束

- 仅个人使用,非商业项目
- WSL Ubuntu 22.04 优先,Windows / macOS 不主动适配
- 不做移动端 / Web 版
