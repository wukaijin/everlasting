# Everlasting — 文档入口

> 个人使用的 vibe coding workbench 应用。基于 Tauri + 自研 agent core,WSL 优先。
> 顶层 [README.md](../README.md) 给出项目一句话介绍,本文档是设计相关的索引。

---

## 文档结构

按"是什么 → 怎么搭 → 用什么 → 怎么做 → 未来"5 维拆分:

| 文件 | 主题 | 性质 | 何时读 |
|------|------|------|--------|
| [DESIGN.md](./DESIGN.md) | 需求设计 | 已决定的项目边界 | 第一次接触项目,看"我到底在做什么" |
| [ARCHITECTURE.md](./ARCHITECTURE.md) | 架构设计 | 系统怎么搭、请求怎么流 | 写代码前,看"模块怎么分、调用怎么走" |
| [TECH.md](./TECH.md) | 技术栈 | 用什么库、为什么 | 选库/做依赖决策时 |
| [IMPLEMENTATION.md](./IMPLEMENTATION.md) | 实现讲解 | 路线图、决策记录、待办 | 动手时,看"下一步做什么" |
| [BACKLOG.md](./BACKLOG.md) | 候选功能 | 7 个新功能的技术评估 | 评估新功能时(优先级未定) |

## 推荐阅读顺序

**第一次接触**:
1. [DESIGN.md](./DESIGN.md) §1-2 — 了解项目是什么、什么不做
2. [ARCHITECTURE.md](./ARCHITECTURE.md) §1-2 — 了解系统怎么搭、请求生命周期
3. [IMPLEMENTATION.md](./IMPLEMENTATION.md) §2 — 了解路线图,准备动手

**写代码时反复查**:
- 16 关卡 → [ARCHITECTURE.md §2](./ARCHITECTURE.md#2-harness-设计从用户输入到文件变更的-16-道关卡)
- 选库依据 → [TECH.md](./TECH.md)
- 当前进度 → [IMPLEMENTATION.md §3](./IMPLEMENTATION.md#3-待办与下一步)

**评估新功能时**:
- [BACKLOG.md §0](./BACKLOG.md#0-全局视角这-7-个功能落在-5-个不同的层) — 五层架构,看功能落在哪
- 对应章节 — 看具体选型

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
