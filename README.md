# Everlasting

> 个人使用的 vibe coding workbench。基于 Tauri + 自研 agent core,WSL 优先。

## 这是什么

一个桌面应用,给"在 WSL 里写代码的 Windows 用户"用的 vibe coding 工作台。

不是另一个 Claude Code 替代品,而是同样的能力(聊、改代码、跑命令)加上:
- **自研 agent core** — 为了学习 harness engineering,不用 SDK 包装
- **深度 WSL 集成** — 项目放 WSL 内部,不走 `/mnt/c`
- **多项目 / 多 session / 工作流** — 不是一次性对话,是持久工作环境

## 当前状态 (2026-06-10)

权威看 `git log --oneline -20`,本节仅列关键 milestone:

- ✅ MVP 步骤 1 / 2 / 3a / 3b-1 / 4(Git 集成)/ 6a-多 Provider 已完成
- ✅ 路线图外完成 Anthropic extended thinking + spike-005 follow-up 7 PR + 字体栈 + 6 UI/状态 bug 修复 + 工具集扩展批次(edit_file / grep / glob)
- ✅ **当前进行**:Step 8 代码重构(5 PR: 8-PR1 lib.rs 拆分 / 8-PR2 db.rs 拆分 / 8-PR3 前端拆 sub-components 已落地,8-PR4 文档更新 + spec 清理 本次,8-PR5 STRUCTURE.md 待办)
- ⏸ MCP 暴露未开始;3b-2(完整三栏 UI + rig-core 迁移)已废弃
- 🐛 已知 issue:bug 1+2 position 在 RDP 双显示器下未完全修好

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

设计文档在 [`docs/`](./docs/),按"需求/架构/技术/实现/候选"5 维拆分。

| 文档 | 看什么 |
|------|--------|
| [docs/README.md](./docs/README.md) | 索引 + 必读参考学习清单(参考但不抄) |
| [docs/DESIGN.md](./docs/DESIGN.md) | 项目是什么、什么不做、Scope(MVP/v1/v2/v3+) |
| [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) | 系统怎么搭、16 关卡请求生命周期 |
| [docs/TECH.md](./docs/TECH.md) | 锁定了哪些库(自研 Provider trait / rmcp / git2-rs / sqlx / nucleo 等) |
| [docs/IMPLEMENTATION.md](./docs/IMPLEMENTATION.md) | 8 步路线图 + 决策日志 + 下一步待办 |
| [docs/BACKLOG.md](./docs/BACKLOG.md) | 7 个候选功能的技术评估(优先级未定) |

**第一次接触推荐顺序**:DESIGN §2-3 → ARCHITECTURE §1-2 → IMPLEMENTATION §2。

## 路线图(8 步,不写时间)

> 实时状态以 `git log --oneline -20` 为准;路线图语义在 2026-06-09 校准:rig-core 弃用,3b-2 标记废弃。

| 阶段 | 步骤 | 目标 | 状态 |
|------|------|------|------|
| MVP | 1 | Tauri 骨架 + LLM 直连 + 流式显示 | ✅ |
| MVP | 2 | Tool Calling(改文件 / 跑 shell) | ✅ |
| MVP | 3a | SQLite + Session 持久化 | ✅ |
| MVP | 3b-1 | 项目基础结构 + 顶部 Tabs UI | ✅ |
| — | 3b-2 | ~~完整三栏 UI + rig-core 迁移~~ | ⛔ 废弃 (2026-06-09,rig-core 弃用) |
| MVP | 4 | Git 集成(worktree + auto commit) | ✅ (worktree 解耦 + opt-in attach) |
| MVP | 5 | WSL 体验(WSLg 显示,无跨边界) | ✅ (spike-001 验证) |
| v1  | 6a | 多 Provider(Anthropic / OpenAI) | ✅ |
| v1  | 6b | MCP 暴露 + 嵌入式终端 + 权限系统 | ⏸ 未开始 |
| 跨  | 8 | 代码重构与文档清理(5 PR) | 🔄 当前进行 |

详见 [IMPLEMENTATION §2](./docs/IMPLEMENTATION.md#2-实施路线图)。

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
