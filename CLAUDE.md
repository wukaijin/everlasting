# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Everlasting — 个人 vibe coding 工作台。Tauri 2 + Vue 3 + Rust，自研 agent core（非 SDK 包装），WSL-first 设计。目标：与 Claude Code 同等能力（聊天、编辑代码、运行命令），但用自研的 agent harness 实现以学习 harness 工程。

**当前状态(2026-06-10)**:MVP 主体 + 多 Provider + Step 8 代码重构已全部完成;V2 路线图已重排,🟢 第一档准备开始。已知 issue:bug 1+2 position 在 RDP 双显示器下未完全修好。

**路线图 / 排期 / 维护承诺**:**[docs/ROADMAP.md](./docs/ROADMAP.md)** 是单一 source of truth(V2 4 档分类 + 已实施粗粒度归类)。本文档不重复路线图细节;决策历史见 [docs/IMPLEMENTATION.md §4](./docs/IMPLEMENTATION.md#4-决策日志)。

## Common Commands

```bash
# 开发
cd app && pnpm tauri dev        # 启动 Vite dev server + Tauri 窗口

# 构建
cd app && pnpm tauri build      # 前端 type-check + build，然后 Rust 编译 + 打包

# 仅前端
cd app && pnpm dev              # 只跑 Vite dev server（无 Tauri）
cd app && pnpm build            # vue-tsc --noEmit + vite build

# Rust（必须 cd 到 app/src-tauri，项目根目录没有 Cargo.toml）
cd app/src-tauri && cargo check # 快速编译检查
cd app/src-tauri && cargo test  # 运行 Rust 单元测试（sse.rs / error.rs 有 #[cfg(test)]）

# WSL 环境（linuxbrew pkg-config 覆盖系统路径——见 HACKING-wsl 坑 1）：
# cargo check / cargo test 撞到 gdk-pixbuf-2.0 / webkit2gtk-4.1 等"系统库 not found"
# 时，最短路径是给 PKG_CONFIG_PATH 加系统 pkgconfig 目录（不要去改 tauri config）：
#   cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check
#   cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test
#   cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
# 注：完整 gtk/webkit 依赖（Tauri runtime）需要 `pnpm tauri dev/build` 走 .cargo/config 路径，
#     `cargo test` 和 `cargo test --lib` 都需要 PKG_CONFIG_PATH，否则撞 gdk-pixbuf not found。

# 日志控制
RUST_LOG=debug pnpm tauri dev   # tracing 输出级别
```

项目没有配置前端测试框架（无 vitest/jest），类型安全靠 `vue-tsc --noEmit`。

## Architecture

> 完整结构见 [STRUCTURE.md](./STRUCTURE.md)(8-PR5 创建)。

```
app/
├── src/                    # Vue 3 前端 (8-PR3 拆分后)
│   ├── components/
│   │   ├── layout/         # AppShell / AppHeader / Sidebar / TitleBar / AppLogo
│   │   ├── chat/           # ChatPanel / MessageList / ChatInput / ToolCallCard / DiffView 等子组件
│   │   ├── settings/       # ModelRow 等
│   │   ├── ChatWindow.vue  # 顶层容器(69 行,纯组合)
│   │   ├── SessionList.vue
│   │   ├── ProjectTabs.vue
│   │   └── Icon.vue
│   ├── stores/             # Pinia stores
│   │   ├── chat.ts         # facade: sessions 列表 + currentSessionId + currentCwd + CRUD 委托
│   │   ├── streamController.ts # SSE 单源 + LRU 20 + activeRequests (8-PR3 拆分)
│   │   ├── config.ts       # useConfigStore: LLM 配置
│   │   ├── models.ts       # models catalog
│   │   ├── providers.ts    # providers 配置
│   │   └── projects.ts     # projects 列表
│   └── utils/              # path / markdown / messageFormat / lru
├── src-tauri/              # Rust 后端 (8-PR1/2 拆分后)
│   └── src/
│       ├── lib.rs          # Tauri 入口(94 行,纯 init + 命令注册)
│       ├── state.rs        # AppState 共享状态
│       ├── main.rs         # Windows 子系统入口
│       ├── db/             # SQLite 持久化(8-PR2 拆分, 8 个 CRUD 函数分散到子模块)
│       │   ├── mod.rs / migrations.rs / types.rs / models.rs / config.rs
│       │   ├── providers.rs / projects.rs / sessions.rs / tests.rs
│       ├── llm/            # LLM 客户端模块 + 自研 Provider trait
│       │   ├── client.rs   # LlmConfig::from_env()、chat_stream_with_tools()、BlockState 状态机
│       │   ├── provider.rs # Provider trait + AnthropicProvider + OpenAIProvider
│       │   ├── wire.rs     # WireMessage 跨协议中间层
│       │   ├── sse.rs      # SseParser — 状态机式 SSE 行解析
│       │   ├── error.rs    # LlmError 5 类错误分类、中文用户消息
│       │   └── types.rs    # ContentBlock、MessageContent、ChatMessage、ToolDef、ChatEvent
│       ├── agent/          # Agent Loop(8-PR1 拆分)
│       ├── commands/       # Tauri commands(8-PR1 拆分,sessions/projects/config/cancel/providers/worktree)
│       ├── projects/       # Project 数据模型 + boundary 校验
│       ├── git/            # git2-rs worktree + diff
│       └── tools/          # Tool 定义与执行
│           ├── mod.rs      # builtin_tools()、execute_tool() 分发
│           ├── read_file.rs / write_file.rs / edit_file.rs / grep.rs / glob.rs / list_dir.rs / shell.rs
docs/                       # 设计文档(全中文)
└── spikes/                 # 技术验证记录
```

### 核心数据流

前端 `ChatWindow.vue`（侧边栏 + chat 区）→ Pinia `chat.ts send()` → Tauri IPC `invoke("chat", { requestId, sessionId, messages })` → Rust `chat` 命令 **Agent Loop**（max 20 turns）→ 每轮：`chat_stream_with_tools()` 请求 LLM API → SSE 流式解析（BlockState 状态机处理 text/tool_use）→ 高频事件 `chat-event`（delta/start/done/error）+ 低频独立事件 `tool:call` / `tool:result` → 如果 tool_use 则执行 tool → 构造 tool_result 回填 → 再发 LLM → 直到 text-only 响应或 max turns。**Turn 边界**调 `db::persist_turn` 落 SQLite，session 列表从 DB 读。前端 Pinia store 多 listener 监听，增量更新消息 + 工具卡片。

### 关键架构决策

- **自研 agent core**：不使用 Anthropic Agent SDK / Codex SDK，自己实现 Agent Loop、消息管理、tool 注册、权限检查（见 `docs/IMPLEMENTATION.md §1`）
- **步骤 1 用手写 SSE 解析**：不用 eventsource-stream crate，`llm/sse.rs` 是自研状态机（已通过 spike-002 验证）
- **自研 Provider trait（多 Provider 抽象）**：`llm/provider.rs` 定义 `Provider` trait，`AnthropicProvider` / `OpenAIProvider` 两个实现 + `llm/wire.rs` WireMessage 跨协议中间层（2026-06-08/09 落地，取代早期 rig-core 计划）
- **16 阶段请求生命周期**：完整的 agent 请求处理管线，定义在 `docs/ARCHITECTURE.md`
- **daemon 化**：后期 Tauri GUI 进程与 Agent Daemon 进程分离，通过 Unix socket / WebSocket IPC

## Environment Variables

```bash
ANTHROPIC_API_KEY=xxx        # 或 ANTHROPIC_AUTH_TOKEN(必需,用于真实 LLM)
ANTHROPIC_BASE_URL=xxx       # 默认 https://api.anthropic.com
OPENAI_API_KEY=xxx           # 多 Provider 模式下使用(可选)
OPENAI_BASE_URL=xxx          # 默认 https://api.openai.com/v1
LLM_MODEL=xxx                # 默认 GLM-4.7 (与 HACKING-llm.md 一致)
LLM_MAX_TOKENS=1024          # 默认 1024
```

**多 Provider 提示**:Anthropic / OpenAI 双 Provider 已落地(2026-06-08/09,4 PR + 1 follow-up),完整设计、wire shape、catalog schema 详见 `.trellis/tasks/archive/2026-06/06-08-multi-model-llm-provider-planning/prd.md` + `.trellis/spec/backend/llm-contract.md` "Scenario: Multi-Provider Abstraction" section。

## WSL 环境注意

项目在 WSL 2 + Ubuntu 22.04 上开发。环境踩坑记录在 `docs/HACKING-wsl.md`（中文输入法、linuxbrew pkg-config、pnpm 代理、Rust 版本、cargo cache 锁、WSLg 字体等）。**新机器或怀疑环境问题时先读 HACKING-wsl**。

## Tech Stack (Locked)

| 层 | 技术 |
|---|---|
| 桌面框架 | Tauri 2 |
| 前端 | Vue 3 (`<script setup>`) + Vite + Pinia + reka-ui |
| 后端 | Rust (edition 2021) + tokio |
| HTTP/LLM | reqwest + 手写 SSE + 自研 Provider trait (Anthropic / OpenAI) | rig-core 0.38.1 **未采用**(2026-06-09 决策弃用,见 IMPLEMENTATION §4) |
| 错误处理 | anyhow（边界）+ thiserror（领域） |
| 日志 | tracing + tracing-subscriber |
| 包管理 | pnpm（前端）、cargo（Rust） |

## Documentation

所有设计文档在 `docs/` 目录，全中文：
- `ROADMAP.md` — **技术路线图(单一 source of truth)**,V2 4 档分类 + 已实施粗粒度归类
- `ARCHITECTURE.md` — 系统架构、16 阶段请求生命周期、核心决策
- `DESIGN.md` — 项目能力边界 + 硬约束(明确不做)
- `TECH.md` — 技术选型决策（锁定/候选/不用）
- `IMPLEMENTATION.md` — 决策档案(§1 自研 agent core 决策 + §4 ADR 决策日志)
- `HANDOFF.md` — session 交接指南
- `HACKING-llm.md` — LLM API 兼容层笔记
- `HACKING-wsl.md` — WSL 环境坑笔记
- `BACKLOG.md` — 候选功能技术评估(排期归 ROADMAP)
