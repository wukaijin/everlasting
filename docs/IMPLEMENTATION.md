# IMPLEMENTATION — 实现讲解

> Everlasting 的"自研决策 + 决策日志"。**本文件是决策档案**,不列路线图(路线图见 [ROADMAP.md](./ROADMAP.md))。
> 需求见 [DESIGN.md](./DESIGN.md),架构见 [ARCHITECTURE.md](./ARCHITECTURE.md),技术选型见 [TECH.md](./TECH.md),路线图见 [ROADMAP.md](./ROADMAP.md)),候选功能见 [BACKLOG.md](./BACKLOG.md)。
>
> > 2026-06-04/05 项目启动期决策见 [archive/implementation-inception-2026-06-04-to-05.md](../.trellis/spec/archive/implementation-inception-2026-06-04-to-05.md) (已归档)

---

## 1. 决策:自己写 agent core,不用 SDK 包装

**背景**:Anthropic 2025-2026 年出了官方 Agent SDK(`claude-agent-sdk-python` / `-typescript`),用 `query()` 直接拿结构化消息流。OpenAI Codex CLI 是 Rust 写的(Apache 2.0)但没官方 SDK。

**为什么不用**:
1. **学习目标要求自研** — 用了 SDK 只学到"怎么调 SDK",学不到 harness 核心
2. **控制粒度** — SDK 帮你做了"消息流 → tool 调用 → 回填"的循环,你想插自定义逻辑(权限、审计、统计)就被抽象挡住了
3. **解耦厂商** — 一旦 SDK 协议变化,业务逻辑全挂

**什么时候用 SDK 合适**:赶时间、要快速出活、不在乎学习价值。本项目两个都不符合。

**自研的边界**:
- ✅ 自己写:Agent Loop、消息管理、tool 注册、流式解析、权限检查
- ✅ 自己写:Tauri IPC 事件协议、session 持久化、worktree 管理
- ❌ 不自己写:LLM HTTP 协议(用 rig)、SSE 解析(用 rig)、MCP 协议(用 rmcp)
- ❌ 不自己写:GUI 框架(Tauri 已有)、Diff 算法(用前端库)

> **演进注记(2026-07 daemon 化后,见 §4 2026-07-20 ADR)**:上面是 2026-06 项目启动期的快照,两点已演进 ——
> 1. **「Tauri IPC 事件协议」已非唯一入口**:2026-07-20~23 daemon 化后,同一批 handler 双暴露为 axum HTTP(`/api/v1/*`)+ 同源 SSE(`/api/v1/stream`),前端经 transport 抽象层默认走 `httpTransport`(浏览器模式 + Thin 模式 GUI),`tauriTransport`(`?transport=tauri`)退为 Full 模式逃生舱。核心自研的「事件协议」语义不变,只是物理通道从「单 Tauri 进程内 invoke/emit」扩到「GUI ↔ daemon 跨进程 HTTP/SSE」。详见 §4 [2026-07-20 — Agent daemon 化 + HTTP/SSE transport](#4-决策日志)。
> 2. **rig / rmcp 早已废弃**:rig-core 于 2026-06-09 弃用(TECH §2)、rmcp 于 2026-06-10 移除(TECH §3)。上面「用 rig 做 LLM HTTP / SSE」「用 rmcp 做 MCP」是**当时**的真实决策,但现状是 LLM HTTP/SSE 自研(`llm/provider/{anthropic,openai}.rs` + 自写 SSE parser)、MCP 工具走自有 `tools/` 注册(非 rmcp)。保留原文是 ADR 性质的历史档案,不代表当前做法。

---


---

## Part Index (08-07-large-file-splitting)

- [§4 决策日志(按月分卷)](./IMPLEMENTATION/decisions.md)
