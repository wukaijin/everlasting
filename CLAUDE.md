# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Everlasting — 个人 vibe coding 工作台。Tauri 2 + Vue 3 + Rust，自研 agent core（非 SDK 包装），WSL-first 设计。目标：与 Claude Code 同等能力（聊天、编辑代码、运行命令），但用自研的 agent harness 实现以学习 harness 工程。

当前状态：MVP 步骤 1（骨架 + LLM 直连）已完成。详见 `docs/IMPLEMENTATION.md` 的 8 步路线图。

## Common Commands

```bash
# 开发
cd app && pnpm tauri dev        # 启动 Vite dev server + Tauri 窗口

# 构建
cd app && pnpm tauri build      # 前端 type-check + build，然后 Rust 编译 + 打包

# 仅前端
cd app && pnpm dev              # 只跑 Vite dev server（无 Tauri）
cd app && pnpm build            # vue-tsc --noEmit + vite build

# Rust
cd app/src-tauri && cargo check # 快速编译检查
cd app/src-tauri && cargo test  # 运行 Rust 单元测试（sse.rs / error.rs 有 #[cfg(test)]）

# 日志控制
RUST_LOG=debug pnpm tauri dev   # tracing 输出级别
```

项目没有配置前端测试框架（无 vitest/jest），类型安全靠 `vue-tsc --noEmit`。

## Architecture

```
app/
├── src/                    # Vue 3 前端
│   ├── components/         # ChatWindow.vue（IME 安全输入框 + 消息列表 + 流式光标）
│   └── stores/             # Pinia stores
│       ├── chat.ts         # useChatStore: 消息列表、流式状态、listen("chat-event")
│       └── config.ts       # useConfigStore: LLM 配置（model/baseUrl/configured）
├── src-tauri/              # Rust 后端
│   └── src/
│       ├── lib.rs          # Tauri 入口: AppState、chat 命令、get_llm_config 命令
│       ├── main.rs         # Windows 子系统入口
│       └── llm/            # LLM 客户端模块
│           ├── client.rs   # LlmConfig::from_env()、chat_stream()
│           ├── sse.rs      # SseParser — 状态机式 SSE 行解析（处理 GLM ping 心跳）
│           ├── error.rs    # LlmError 5 类错误分类、中文用户消息
│           └── types.rs    # Role、ChatMessage、ChatRequest、ChatEvent、LlmErrorCategory
docs/                       # 设计文档（全中文）
└── spikes/                 # 技术验证记录
```

### 核心数据流

前端 `ChatWindow.vue` → Pinia `chat.ts send()` → Tauri IPC `invoke("chat", ...)` → Rust `chat` 命令 `spawn` 异步任务 → `chat_stream()` 用 reqwest 请求 LLM API → SSE 流式解析 → 每个事件通过 `emit("chat-event", payload)` 推回前端 → Pinia store 监听并增量更新消息。

### 关键架构决策

- **自研 agent core**：不使用 Anthropic Agent SDK / Codex SDK，自己实现 Agent Loop、消息管理、tool 注册、权限检查（见 `docs/IMPLEMENTATION.md §1`）
- **步骤 1 用手写 SSE 解析**：不用 eventsource-stream crate，`llm/sse.rs` 是自研状态机（已通过 spike-002 验证）
- **步骤 3b 切到 rig-core**：LLM 客户端从 reqwest 迁移到 rig-core（保留为后续步骤）
- **16 阶段请求生命周期**：完整的 agent 请求处理管线，定义在 `docs/ARCHITECTURE.md`
- **daemon 化**：后期 Tauri GUI 进程与 Agent Daemon 进程分离，通过 Unix socket / WebSocket IPC

## Environment Variables

```bash
ANTHROPIC_API_KEY=xxx        # 或 ANTHROPIC_AUTH_TOKEN（必需，用于真实 LLM）
ANTHROPIC_BASE_URL=xxx       # 默认 https://api.anthropic.com
LLM_MODEL=xxx                # 默认 GLM-4.7
LLM_MAX_TOKENS=1024          # 默认 1024
```

## WSL 环境注意

项目在 WSL 2 + Ubuntu 22.04 上开发。环境踩坑记录在 `docs/HACKING-wsl.md`（中文输入法、linuxbrew pkg-config、pnpm 代理、Rust 版本、cargo cache 锁、WSLg 字体等）。**新机器或怀疑环境问题时先读 HACKING-wsl**。

## Tech Stack (Locked)

| 层 | 技术 |
|---|---|
| 桌面框架 | Tauri 2 |
| 前端 | Vue 3 (`<script setup>`) + Vite + Pinia + reka-ui |
| 后端 | Rust (edition 2021) + tokio |
| HTTP/LLM | reqwest + 手写 SSE（步骤 1）→ rig-core（步骤 3b） |
| 错误处理 | anyhow（边界）+ thiserror（领域） |
| 日志 | tracing + tracing-subscriber |
| 包管理 | pnpm（前端）、cargo（Rust） |

## Documentation

所有设计文档在 `docs/` 目录，全中文：
- `ARCHITECTURE.md` — 系统架构、16 阶段请求生命周期、核心决策
- `DESIGN.md` — 项目范围、约束、排除项
- `TECH.md` — 技术选型决策（锁定/候选/不用）
- `IMPLEMENTATION.md` — 8 步路线图、决策日志
- `HANDOFF.md` — session 交接指南
- `HACKING-llm.md` — LLM API 兼容层笔记
- `HACKING-wsl.md` — WSL 环境坑笔记
- `BACKLOG.md` — 候选功能评估
