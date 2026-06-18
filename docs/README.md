# Everlasting — 文档入口

> 个人使用的 vibe coding workbench 应用。基于 Tauri + 自研 agent core,WSL 优先。
> 顶层 [README.md](../README.md) 给出项目一句话介绍,本文档是设计相关的索引。

---

## 文档结构

按"是什么 → 怎么搭 → 用什么 → 怎么做 → 未来"5 维拆分:

| 文件 | 主题 | 性质 | 何时读 |
|------|------|------|--------|
| [DESIGN.md](./DESIGN.md) | 需求设计 | 已决定的项目能力边界 + 硬约束 | 第一次接触项目,看"我到底在做什么 / 不做什么" |
| [ROADMAP.md](./ROADMAP.md) | 技术路线图(单一 source of truth) | V2 4 档分类 + 已实施粗粒度归类 + 维护承诺 | 看当前在哪一步、下一步选项、什么不做 |
| [ARCHITECTURE.md](./ARCHITECTURE.md) | 架构设计 | 系统怎么搭、请求怎么流 | 写代码前,看"模块怎么分、调用怎么走" |
| [TECH.md](./TECH.md) | 技术栈 | 用什么库、为什么 | 选库/做依赖决策时 |
| [CONTEXT.md](./CONTEXT.md) | 术语表 | A4 Token 用量统计核心术语定义(glossary) | 写/改 token 统计或 cache 逻辑前对齐术语时 |
| [IMPLEMENTATION.md](./IMPLEMENTATION.md) | 决策档案 | §1 自研 agent core 决策 + §4 决策日志(ADR 性质,只追加) | 想看"为什么这么做"的历史 ADR |
| [BACKLOG.md](./BACKLOG.md) | 候选功能 | 7 个新功能的技术评估(排期归 ROADMAP) | 评估新功能技术细节时 |
| [HANDOFF.md](./HANDOFF.md) | 新 session 引导 | 5 分钟上手 + 当前任务清单 | 进新 session 第一时间读 |
| [HACKING-wsl.md](./HACKING-wsl.md) | WSL 环境坑笔记 | 10 个已知坑 + 一次性环境脚本 | 撞 WSL / 字体 / Rust 工具链 / fcitx5 输入法问题时 |
| [HACKING-llm.md](./HACKING-llm.md) | LLM API 兼容层笔记 | GLM 兼容层 3 处差异 + 实施 checklist | 写 / 改 / 调试 LLM 客户端时 |
| [HACKING-markdown.md](./HACKING-markdown.md) | 前端 markdown 渲染陷阱 | marked v18 + DOMPurify 的 XSS / 协议白名单 / 测试 fixture | 改前端 markdown 渲染 / 加 vitest fixture 时 |
| [spikes/](./spikes/) | 技术验证记录 | 5 分钟上手每个 spike 的目标 / 标准 / 结果 | 评估"某项技术能不能用"时 |
| [`_archive/`](./_archive/) | 一次性任务归档（PROPOSAL / 评审 / 收尾） | 历史任务产物，已沉淀到主目录文档 | 查阅历史决策时 |
| [`_reviews/`](./_reviews/) | 项目级设计评审快照 | 外部 LLM 评审（只读不改） | 了解项目被评审过什么 |

## 推荐阅读顺序

**第一次接触**:
1. [DESIGN.md](./DESIGN.md) §1-3 — 了解项目是什么、什么不做
2. [ROADMAP.md](./ROADMAP.md) §1-2 — 了解 V2 路线图与已实施项,准备动手
3. [ARCHITECTURE.md](./ARCHITECTURE.md) §1-2 — 了解系统怎么搭、请求生命周期

**写代码时反复查**:
- 16 关卡 → [ARCHITECTURE.md §2](./ARCHITECTURE.md#2-harness-设计从用户输入到文件变更的-16-道关卡)
- 选库依据 → [TECH.md](./TECH.md)
- 当前进度 → [ROADMAP.md §1](./ROADMAP.md#1-已实施mvp-主体--路线图外完成) + §2 V2 路线图分类
- "为什么这么做" → [IMPLEMENTATION.md §4 决策日志](./IMPLEMENTATION.md#4-决策日志)

**评估新功能时**:
- [BACKLOG.md §0](./BACKLOG.md#0-全局视角这-7-个功能落在-5-个不同的层) — 五层架构,看功能落在哪
- 对应章节 — 看具体选型

**撞到环境 / API 怪事时**:
- WSL / 字体 / Rust 工具链 → [HACKING-wsl.md](./HACKING-wsl.md)
- LLM 流式 / 错误分类 / 协议差异 → [HACKING-llm.md](./HACKING-llm.md)
- 前端 markdown 渲染 / DOMPurify / 协议白名单 → [HACKING-markdown.md](./HACKING-markdown.md)
- 前端状态管理 / streamController / Pinia 模式 → [`.trellis/spec/frontend/state-management.md`](../.trellis/spec/frontend/state-management.md)(注:此文件在 `.trellis/spec/`,不在 `docs/`)

**查阅历史决策 / 评审快照**:
- [docs/_archive/](./_archive/README.md) — 一次性任务归档（PROPOSAL / 评审 / 收尾 follow-up）
- [docs/_reviews/](./_reviews/README.md) — 项目级设计评审快照（外部 LLM 评审，只读不改）

---

## 必读参考(学习清单)

按优先级读,每个项目读透 1-2 个关键模块就行,不要通读。

### 第一梯队:必读

| 项目                           | 为什么读                                                | 看哪些文件                                    |
|--------------------------------|---------------------------------------------------------|-----------------------------------------------|
| **anthropics/claude-agent-sdk-python** | 理解 agent loop 是什么样(我们的目标是写出更好的) | `src/claude_agent_sdk/query.py`,`internal/message_parser.py` |
| **All-Hands-AI/OpenHands**     | Local GUI 几乎就是你要做的产品                          | `frontend/`,`openhands/server/`,事件流相关 |
| **0xPlaygrounds/rig**          | Rust LLM 框架的设计抽象                                  | `rig-core/src/agent/`,`rig-core/src/providers/anthropic/` |
| **modelcontextprotocol/rust-sdk** | MCP 协议 Rust 实现                                    | `examples/`,`crates/rmcp/src/service.rs`      |

### 第二梯队:挑读

| 项目                | 为什么读                                | 看哪些文件                          |
|---------------------|-----------------------------------------|-------------------------------------|
| **cline/kanban**    | 多 agent + worktree + 依赖链的实现     | worktree 管理部分,auto-commit 逻辑 |
| **Aider-AI/aider**  | repo map、commit 策略、token 优化       | `aider/repo.py`,`aider/history.py`  |
| **cline/cline**     | modes(不同 agent 角色)的状态机         | state machine 相关                  |

### 第三梯队:参考

- **OpenHands software-agent-sdk** — Python 版的 agent SDK,看它的 API 设计怎么把"定义 agent"做简单
- **Anthropic 官方文档**(platform.claude.com) — Messages API 流式协议、tool use schema
- **MCP 规范**(modelcontextprotocol.io) — 不用背,知道在哪查
