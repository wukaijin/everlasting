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
| [CONTEXT.md](./CONTEXT.md) | 术语表 | 项目 glossary(Token 用量 / Checklist / Subagent / AuditKind / daemon 化进程模型 等) | 写/改跨模块共享概念前对齐术语时 |
| [IMPLEMENTATION.md](./IMPLEMENTATION.md) | 决策档案 | §1 自研 agent core 决策 + 决策日志(ADR 性质,只追加,按月分卷,见 [IMPLEMENTATION/decisions.md](./IMPLEMENTATION/decisions.md)) | 想看"为什么这么做"的历史 ADR |
| [REMOTE-ACCESS-ROADMAP.md](./REMOTE-ACCESS-ROADMAP.md) | daemon 化编排 | remote-access epic(transport 抽象 / axum daemon / sidecar / httpTransport / ServeDir)的 Phase 编排 + 状态 | 看 daemon 化怎么分阶段落地 / 当前到哪个 Phase |
| [REMOTE-DEPLOY.md](./REMOTE-DEPLOY.md) | remote 云服务器部署手册 | everlasting-remote 服务端部署(国内 2C2G 服务器 + nginx + remote.sh / deploy-remote.sh) | 部署 remote daemon / 排查部署问题时 |
| [REMOTE-ACCESS-E2E.md](./REMOTE-ACCESS-E2E.md) | 远程访问 E2E 部署与验收手册 | S1+S2+S4+S5 全链路(E2E 隧道管线 / 配对 / PWA)逐场景验收步骤 | 端到端验证远程访问 / 回滚排查时 |
| [BACKLOG.md](./BACKLOG.md) | 候选功能 | 7 个新功能的技术评估(排期归 ROADMAP) | 评估新功能技术细节时 |
| [BUGLIST.md](./BUGLIST.md) | 缺陷跟踪 | 2026-08-29 WebUI 全量测试的甄别结论 + 待修复清单(状态跟踪,修一个勾一个) | 领缺陷修复 / 看某测试问题是否设计如此时 |
| [HACKING-wsl.md](./HACKING-wsl.md) | WSL 环境坑笔记 | 11 个已知坑 + 一次性环境脚本 | 撞 WSL / 字体 / Rust 工具链 / fcitx5 输入法问题时 |
| [HACKING-llm.md](./HACKING-llm.md) | LLM API 兼容层笔记 | GLM 兼容层 3 处差异 + 实施 checklist | 写 / 改 / 调试 LLM 客户端时 |
| [HACKING-markdown.md](./HACKING-markdown.md) | 前端 markdown 渲染陷阱 | marked v18 + DOMPurify 的 XSS / 协议白名单 / 测试 fixture | 改前端 markdown 渲染 / 加 vitest fixture 时 |
| [DEBUG_DB.md](./DEBUG_DB.md) | SQLite 直连调试指引 | DB 路径 / schema / sqlite3 速查 | 直连查 DB / 排查数据问题时 |
| [spikes/](./_history/spikes/) | 技术验证记录 | 5 分钟上手每个 spike 的目标 / 标准 / 结果 | 评估"某项技术能不能用"时 |
| [`_history/`](./_history/) | 统一历史归档 | 已消费文档 / 设计回顾 / 调研 / 评审 / 验证(含归档后的 [A2-SHELL](./_history/2026-08-28-a2-shell-classification.md) / [INTERLEAVED-THINKING](./_history/2026-08-28-interleaved-thinking-design.md) / [WORKFLOW-INTEGRATION](./_history/2026-08-28-workflow-integration.md) 设计回顾 + 13 part 子目录) | 查阅历史决策 / 设计回顾时 |

## 推荐阅读顺序

**按场景速查**:
- **第一次接触**:CLAUDE.md → DESIGN.md → ROADMAP.md → ARCHITECTURE.md(看"做什么 / 不做 / 当前在哪步 / 怎么搭")
- **写代码时反复查**:ARCHITECTURE.md §2 18+ 关卡 / TECH.md 选库 / IMPLEMENTATION/decisions.md ADR
- **评估新功能**:BACKLOG.md §0 五层架构 → 对应章节
- **撞环境/API 怪事**:HACKING-wsl.md / HACKING-llm.md / HACKING-markdown.md / `.trellis/spec/frontend/state-management.md`

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
