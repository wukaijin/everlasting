# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Everlasting — 个人 vibe coding 工作台。Tauri 2 + Vue 3 + Rust，自研 agent core（非 SDK 包装），WSL-first 设计。目标：与 Claude Code 同等能力（聊天、编辑代码、运行命令），但用自研的 agent harness 实现以学习 harness 工程。

**当前状态(2026-06-13)**:MVP 主体 + 多 Provider + Step 8 代码重构已全部完成;memory/指令文件系统（4 文件加载 + cache_control 注入）+ per-session token usage + C3 context 压缩(token 硬卡 + MAX_TURNS 50)+ A2+B7 权限系统(⑨ 关 5-tier path-based 决策层 + 3 档 Mode `edit`/`plan`/`yolo` + ⑯ 审计日志 10 类 AuditKind)已全部落地;V2 路线图第一档收口 + 第二档 2/7 进 §1 已实施,剩余 5 项在第二档,详见 [docs/ROADMAP.md §1.2](./docs/ROADMAP.md#12-路线图外完成)。position bug(2026-06-14 ✅ 已解决,A7 收尾):根因是 Wayland 协议禁止客户端设置窗口位置(WSLg/Weston 下 `setPosition()` 被合成器忽略,Tauri issue #14913,非 Tauri bug),故 `TitleBar.vue` 放弃手动 setSize+setPosition 铺满整屏,全平台改原生 `toggleMaximize()`;RDP 双屏已验证通过,详见 [IMPLEMENTATION §4 2026-06-14](./docs/IMPLEMENTATION.md#4-决策日志)。

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

前端测试用 **vitest**（`app/vitest.config.ts`，覆盖 `app/src/**/*.test.ts`：streamController / lru / markdown / messageFormat / path / permissions / chatMode / duration / useKeyboard 等 store 与 utils）；类型安全另靠 `vue-tsc --noEmit`。Rust 单元测试走 `cargo test`（`#[cfg(test)]` 内联于各模块）。

## Architecture

> 完整结构见 [STRUCTURE.md](./STRUCTURE.md)(8-PR5 创建)。

```
app/
├── src/                    # Vue 3 前端 (8-PR3 拆分后;06-23 续拆 3 组件 + 1 composable + 3 store 模块)
│   ├── components/
│   │   ├── layout/         # AppShell / AppHeader / Sidebar / TitleBar / AppLogo
│   │   ├── chat/           # ChatPanel / MessageList / ChatInput / MessageItem / ToolCallCard / DiffView / SubagentDrawer 等
│   │   │                   # (06-23 拆:MessageItemEdit/Footer + SubagentDrawerHeader/ErrorCard + ChatInputLatencyPopover/HintRow)
│   │   ├── memory/         # MemoryPreview / MemoryModal / MemoryLayerItem
│   │   ├── settings/       # SettingsModal / ModelRow / ProvidersTab / MemoryTab 等
│   │   ├── audit/          # C4 审计日志查询 UI (AuditLogModal / AuditLogItem)
│   │   ├── common/         # 通用组件 (TriggerMenu 等 @文件/命令触发器)
│   │   ├── ChatWindow.vue  # 顶层容器(纯组合)
│   │   ├── SessionList.vue / ProjectTabs.vue / Icon.vue
│   ├── stores/             # Pinia stores
│   │   ├── chat.ts         # facade: sessions 列表 + currentSessionId + currentCwd + CRUD 委托
│   │   ├── chat.types.ts   # (06-23 拆)~310 行纯类型 + 强绑定 const(MODE_CYCLE 等)
│   │   ├── streamController.ts # SSE 单源 + LRU 20 + activeRequests (8-PR3 拆分)
│   │   ├── subagentRuns.ts # (06-23 拆)store 主体 + coerceStatus(~547 行)
│   │   ├── subagentRuns.types.ts # (06-23 拆)~354 行
│   │   ├── runAccumulator.ts # (06-23 拆)~537 行 RunAccumulator + parseTranscriptJson
│   │   ├── config.ts / models.ts / providers.ts / projects.ts
│   │   ├── memory.ts       # memory/指令文件 UI 状态
│   │   ├── permissions.ts   # A2+B7 权限 / Mode (edit/plan/yolo) 状态
│   │   ├── audit.ts         # C4 审计日志查询 store
│   │   └── checklist.ts     # B12 agent 自跟踪 checklist store
│   └── utils/              # path / markdown / messageFormat / tokenUsage / lru / audit / colorTag / duration / useKeyboard / chatInputCodeMirror (06-23 拆 composable)
├── src-tauri/              # Rust 后端 (8-PR1/2 拆分后;06-23 续拆 subagent/ + chat_loop + tests)
│   └── src/
│       ├── lib.rs          # Tauri 入口(纯 init + 命令注册)
│       ├── state.rs        # AppState 共享状态
│       ├── main.rs         # Windows 子系统入口
│       ├── resource_loader.rs  # Markdown + frontmatter 通用加载 (Skill/Role/B3 /command 资源,parse_frontmatter 手写)
│       ├── files.rs        # 文件操作辅助
│       ├── db/             # SQLite 持久化(8-PR2 拆分, CRUD 函数分散到子模块)
│       │   ├── mod.rs / migrations.rs / types.rs / models.rs / config.rs
│       │   ├── providers.rs / projects.rs / sessions.rs / subagent_runs.rs / permissions.rs
│       │   ├── tests.rs    # (06-23 拆)6 个 `*_tests.rs` 按 SQL 域(无 common,test_pool 6 份复制)
│       ├── llm/            # LLM 客户端模块 + 自研 Provider trait
│       │   ├── provider/   # Provider trait + AnthropicProvider + OpenAIProvider + wire.rs + mock.rs
│       │   ├── sse.rs      # SseParser — 状态机式 SSE 行解析
│       │   ├── error.rs    # LlmError 5 类错误分类、中文用户消息
│       │   └── types.rs    # ContentBlock、MessageContent、ChatMessage、ToolDef、ChatEvent
│       ├── memory/         # Memory/指令文件系统(4 文件加载 + cache_control 注入)
│       │   ├── loader.rs / file.rs / watcher.rs / tokens.rs / types.rs
│       ├── agent/          # Agent Loop(8-PR1 拆分;06-23 续拆 subagent/ + chat_loop + tests)
│       │   ├── chat.rs / chat_loop.rs  # (06-23 抽 run_subagent 后)主循环 ~2064 行
│       │   ├── subagent/   # (06-23 拆 4 文件 + dispatch.rs)
│       │   │   ├── mod.rs / sink.rs / transcript.rs / truncate_summary.rs
│       │   │   └── dispatch.rs  # (06-23 抽自 chat_loop.rs)run_subagent + resolve_project_id + SUBAGENT_MAX_TURNS
│       │   ├── permissions/  # (06-23 拆 mod.rs → 8 模块 + 6 tests_*.rs)
│       │   │   ├── mod.rs (纯 re-exports) / types.rs / store.rs / payload.rs
│       │   │   ├── mode.rs / audit.rs / check.rs / ask.rs
│       │   │   ├── dangerous.rs / shell_trust.rs (sibling 不动)
│       │   │   └── tests_*.rs (6 个 + tests_common.rs)
│       │   ├── tests_*.rs  # (06-23 拆 tests.rs → 5 域文件 + tests_common.rs)
│       ├── skill/          # Skill 系统(资源加载 + 注册,/skill + use_skill tool)
│       ├── commands/       # Tauri commands(8-PR1 拆分: sessions/projects/config/cancel/providers/worktree/memory/permissions/command_palette/panel/files/subagent_runs 等)
│       ├── projects/       # Project 数据模型 + boundary 校验
│       ├── git/            # git2-rs worktree + diff
│       └── tools/          # Tool 定义与执行
│           ├── mod.rs      # builtin_tools()、execute_tool() 分发
│           ├── read_file.rs / write_file.rs / edit_file.rs / grep.rs / glob.rs / list_dir.rs / shell.rs
│           ├── web_fetch.rs   # P1 web 抓取(SSRF 拦截 + 5 MiB body cap)
│           ├── use_skill.rs   # Skill 调用 tool
│           ├── update_checklist.rs # B12 agent 自跟踪 checklist tool
│           └── read_guard.rs  # session 隔离的已读文件校验(edit_file 前置 3 道 check)
docs/                       # 设计文档(全中文,spikes/ 在 docs/ 下而非项目根)
```

### 核心数据流

前端 `ChatWindow.vue`（侧边栏 + chat 区）→ Pinia `chat.ts send()` → Tauri IPC `invoke("chat", { requestId, sessionId, messages })` → Rust `chat` 命令 **Agent Loop**（max 50 turns）→ 每轮开头通过 `build_instructions_blocks()` 构造带 `cache_control` 的 synthetic user message（4 个指令文件: User CLAUDE.md / User AGENTS.md / Project CLAUDE.md / Project AGENTS.md）→ `chat_stream_with_tools()` 请求 LLM API → SSE 流式解析（BlockState 状态机处理 text/tool_use）→ 高频事件 `chat-event`（delta/start/done/error）+ 低频独立事件 `tool:call` / `tool:result` → 如果 tool_use 则执行 tool → 构造 tool_result 回填 → 再发 LLM → 直到 text-only 响应或 max turns。**Turn 边界**调 `db::persist_turn` 落 SQLite，session 列表从 DB 读。前端 Pinia store 多 listener 监听，增量更新消息 + 工具卡片。

### 关键架构决策

- **自研 agent core**：不使用 Anthropic Agent SDK / Codex SDK，自己实现 Agent Loop、消息管理、tool 注册、权限检查（见 `docs/IMPLEMENTATION.md §1`）
- **步骤 1 用手写 SSE 解析**：不用 eventsource-stream crate，`llm/sse.rs` 是自研状态机（已通过 spike-002 验证）
- **自研 Provider trait（多 Provider 抽象）**：`llm/provider/` 定义 `Provider` trait，`AnthropicProvider` / `OpenAIProvider` 两个实现 + `wire.rs` WireMessage 跨协议中间层（2026-06-08/09 落地，取代早期 rig-core 计划）
- **16 阶段请求生命周期**：完整的 agent 请求处理管线，定义在 `docs/ARCHITECTURE.md`
- **Memory/指令文件系统**：4 个指令文件（User/Project × CLAUDE.md/AGENTS.md）固定路径加载 + notify 监听 + `build_instructions_blocks()` 构造带 `cache_control: ephemeral` 的 synthetic user message，实现 prompt caching（2026-06-11 B5 重构落地）
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
| HTTP/LLM | reqwest + 手写 SSE + 自研 Provider trait (Anthropic / OpenAI) |
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
