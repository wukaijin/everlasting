# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Everlasting — 个人 vibe coding 工作台。Tauri 2 + Vue 3 + Rust，自研 agent core（非 SDK 包装），WSL-first 设计。目标：与 Claude Code 同等能力（聊天、编辑代码、运行命令），但用自研的 agent harness 实现以学习 harness 工程。

**进程模型**：agent core 跑在独立 `everlasting-daemon` 进程（axum HTTP server），Tauri GUI 进程作为瘦客户端，spawn daemon 为 sidecar 并经同源 HTTP/SSE 通信（默认 `httpTransport`，daemon 同时用 ServeDir 服务前端 SPA）。前端也可脱离 Tauri 用纯浏览器访问 daemon（浏览器模式）。`?transport=tauri` + Full 模式是 daemon 故障时的逃生舱（回退到一体化 Tauri IPC）。另有云端 `everlasting-remote` 服务端（独立 crate，跑国内 2C2G 服务器，仅中继不存 agent 数据），手机 PWA 经它 + WSS 隧道反向访问 PC daemon。详见 [docs/REMOTE-ACCESS-ROADMAP.md](./docs/REMOTE-ACCESS-ROADMAP.md) + [docs/ARCHITECTURE.md §1](./docs/ARCHITECTURE.md)。

**路线图 / 排期 / 维护承诺**:**[docs/ROADMAP.md](./docs/ROADMAP.md)** 是单一 source of truth(V2 4 档分类 + 已实施粗粒度归类),状态类内容(当前做到哪 / 排哪档)一律查它;决策历史走 [docs/IMPLEMENTATION/ 决策日志(按月分卷)](./docs/IMPLEMENTATION/)。本文档只留"项目是什么 + 去哪查",不重复路线图细节与决策历史。

## Common Commands

```bash
# 开发
cd app && pnpm tauri dev        # 启动 Vite dev server + Tauri 窗口

# 构建
cd app && pnpm tauri build      # 前端 type-check + build，然后 Rust 编译 + 打包

# 仅前端
cd app && pnpm dev              # 只跑 Vite dev server（无 Tauri）
cd app && pnpm build            # vue-tsc --noEmit + vite build

# Rust（workspace 翻转后：根目录有 workspace Cargo.toml，members = app/src-tauri + crates/everlasting-remote(-protocol)；
# 根目录裸 cargo build/test 只作用于 default-members（两个 remote crate），不会碰 app）
cargo check -p everlasting          # 从根目录快速编译检查 app（等价 cd app/src-tauri && cargo check）
cargo test -p everlasting --lib     # 运行 app 的 Rust 单元测试
cargo test -p everlasting-remote    # remote crate 测试（零系统库依赖，无需 PKG_CONFIG_PATH）
cd app/src-tauri && cargo check     # 或 cd 进成员目录后裸命令（--bin 可解析到 daemon）
cd app/src-tauri && cargo build --bin everlasting-daemon  # 构建 daemon（根目录则用 cargo build -p everlasting --bin everlasting-daemon）

# WSL 环境（linuxbrew pkg-config 覆盖系统路径——见 HACKING-wsl 坑 1）：
# cargo check / cargo test 撞到 gdk-pixbuf-2.0 / webkit2gtk-4.1 等"系统库 not found"
# 时，最短路径是给 PKG_CONFIG_PATH 加系统 pkgconfig 目录（不要去改 tauri config）：
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
# 注：完整 gtk/webkit 依赖（Tauri runtime）需要 `pnpm tauri dev/build` 走 .cargo/config 路径，
#     `cargo test` 和 `cargo test --lib` 都需要 PKG_CONFIG_PATH，否则撞 gdk-pixbuf not found；
#     remote 两 crate 零系统库依赖，不需要 PKG_CONFIG_PATH。

# 日志控制
RUST_LOG=debug pnpm tauri dev   # tracing 输出级别

# Daemon / 浏览器模式（纯浏览器模式：daemon 同源服务前端 SPA，浏览器开 http://localhost:7456/）
# 子命令：start / bg / stop / restart / rebuild / status / logs；选项 --port N（默认 7456）/ --no-build
./scripts/daemon.sh start      # 编译 release + 前台启动（日志打终端）
./scripts/daemon.sh bg         # 同 start 但后台（日志写 /tmp/everlasting-daemon.log）
./scripts/daemon.sh restart    # stop + bg（改前端后重新 serve dist 的最常用工作流）
./scripts/daemon.sh rebuild    # 只重新编译 release 二进制（不重启）
./scripts/daemon.sh status     # 进程状态 + GET /api/v1/health
./scripts/daemon.sh logs       # tail -f 后台日志
# GUI sidecar 模式：正常 `pnpm tauri dev/build` 即可，GUI 进程自动 spawn daemon 子进程并经 httpTransport 通信。
# 逃生舱：URL 加 ?transport=tauri + GUI 在 Full 模式（EVERLASTING_GUI_FULL_STATE=1）回退到一体化 Tauri IPC。
# ⚠️ 不要同时跑两个 daemon（会撞端口 + 数据分裂；sidecar 模式由 RunEvent::Exit 钩子自动回收）。

# Remote daemon（everlasting-remote，云服务器端；本地开发/联调用 remote.sh 管理）
./scripts/remote.sh start   # 本地起 remote 服务端（默认端口 7457，--shared-secret 必传；零系统库依赖）
./scripts/remote.sh status  # 进程状态 + health
# 部署：scripts/deploy-remote.sh（国内 2C2G 服务器，见 docs/REMOTE-DEPLOY.md）
# E2E 冒烟：scripts/remote-e2e-smoke.mjs（remote 链路端到端验证，见 docs/REMOTE-ACCESS-E2E.md）
```

前端测试用 **vitest**（`app/vitest.config.ts`，覆盖 `app/src/**/*.test.ts`：streamController / lru / markdown / messageFormat / path / permissions / chatMode / duration / useKeyboard 等 store 与 utils）；类型安全另靠 `vue-tsc --noEmit`。Rust 单元测试走 `cargo test`（`#[cfg(test)]` 内联于各模块）。

## Architecture

> 目录树 / 模块归属 / 全景图见 [STRUCTURE.md](./STRUCTURE.md)(单一来源)。此处只留骨架 + 关键数据流。

### 核心数据流

前端 `ChatWindow.vue`（侧边栏 + chat 区）→ Pinia `chat.ts send()` → `transport.invoke("chat", { requestId, sessionId, messages })`（**默认 `httpTransport`**：fetch POST 到 daemon `/api/v1/...`；`?transport=tauri` 逃生时走 Tauri IPC 同进程）→ Rust `chat` handler（daemon 进程的 axum 路由，或 Full 模式下的 Tauri command；两者共享同一 `#[tauri::command]`/REST 双暴露 handler）→ **Agent Loop**（max 200 turns）→ 每轮开头通过 `build_instructions_blocks()` 构造带 `cache_control` 的 synthetic user message（4 个指令文件: User CLAUDE.md / User AGENTS.md / Project CLAUDE.md / Project AGENTS.md）+ 工具前 `memory_recall` 召回 + context 压缩降级 → `chat_stream_with_tools()` 请求 LLM API → SSE 流式解析（BlockState 状态机处理 text/tool_use）→ 高频事件 `chat-event`（delta/start/done/error）+ 低频独立事件 `tool:call` / `tool:result` → 经 daemon 的 `HttpSseSink`（`daemon/sse.rs`）同源 SSE 广播给前端（Full 模式则经 Tauri event）→ 只读工具集 `FuturesUnordered` 批量执行 + 写类 / shell 串行 → 构造 tool_result 回填 → 再发 LLM → 直到 text-only 响应或 max turns。**Turn 边界**调 `db::persist_turn` 落 SQLite（daemon 进程持有 DB pool），session 列表从 DB 读。前端 Pinia store 多 listener 监听（`transport.listen`），增量更新消息 + 工具卡片。

**远程链路（remote-control epic）**：手机 PWA → HTTPS → `everlasting-remote` 云服务端（配对码 redeem 换 device_token → 反向代理 `proxy.rs` 转发请求原文）→ WSS 隧道（`tunnel_registry` + PC 侧 `daemon/tunnel/client.rs` 长连接）→ PC daemon **loopback 转发**（`dispatcher.rs` 打本地 agent core，agent 零改动）→ 流式响应经 `sse_bridge.rs` 桥回 remote 再回手机。前端 `transport/auth.ts` 注入 device_token（`/api/v1/proxy` 前缀 + Bearer；SSE 走 `?access_token=`），配对流由 `stores/pairing.ts` 驱动，vue-router `isRemoteContext()` 守卫仅在 remote-served 语境 gate 配对页。

### 关键架构决策

- **自研 agent core**：不使用 Anthropic Agent SDK / Codex SDK，自己实现 Agent Loop、消息管理、tool 注册、权限检查（见 `docs/IMPLEMENTATION.md §1`）
- **步骤 1 用手写 SSE 解析**：不用 eventsource-stream crate，`llm/sse.rs` 是自研状态机（已通过 spike-002 验证）
- **自研 Provider trait（多 Provider 抽象）**：`llm/provider/` 定义 `Provider` trait，`AnthropicProvider` / `OpenAIProvider` 两个实现 + `wire.rs` WireMessage 跨协议中间层（2026-06-08/09 落地，取代早期 rig-core 计划）
- **16 阶段请求生命周期**：完整的 agent 请求处理管线，定义在 `docs/ARCHITECTURE.md`
- **Memory/指令文件系统**：4 个指令文件（User/Project × CLAUDE.md/AGENTS.md）固定路径加载 + mtime fence 新鲜度校验（RULE-C-001,notify 已移除）+ `build_instructions_blocks()` 构造带 `cache_control: ephemeral` 的 synthetic user message，实现 prompt caching（2026-06-11 B5 重构落地）
- **daemon 化（已落地，remote-access Phase 2）**：agent core 从 Tauri GUI 进程拆出为独立 `everlasting-daemon` 进程（axum HTTP server），GUI 作为瘦客户端 spawn daemon 为 sidecar，经**同源 HTTP + SSE** 通信（`httpTransport` 默认）；daemon 用 `tower-http::ServeDir` 同源服务前端 SPA，支持纯浏览器访问（浏览器模式）。`?transport=tauri` + Full 模式是 daemon 故障逃生舱（回退一体化 Tauri IPC）。决策动机见 [docs/IMPLEMENTATION/ 决策日志](./docs/IMPLEMENTATION/)，编排放 [docs/REMOTE-ACCESS-ROADMAP.md](./docs/REMOTE-ACCESS-ROADMAP.md)
- **远程控制（已落地，remote-control epic S1~S6b）**：Cargo workspace 翻转（根 `Cargo.toml`，default-members 只含 remote 两 crate）；新增 `everlasting-remote` 云服务端（axum 0.7 ws + 自研 WSS 隧道协议 + 配对码 60s 一次性 + per-IP 限速 + 反向代理，**仅中继不存 agent 数据**，PC daemon 权威不变）；PC daemon 增加 tunnel client（WSS 长连接 + loopback 转发，**agent core 零改动**）；前端 vue-router + PWA 壳 + 配对/节点视图 + pwa-remote transport 模式；移动端适配（S5/S6a/S6b 含真机迭代）。HTTPS 用户自理（nginx），不做主动推送/多用户/跨节点同步。部署见 [docs/REMOTE-DEPLOY.md](./docs/REMOTE-DEPLOY.md)，E2E 验收见 [docs/REMOTE-ACCESS-E2E.md](./docs/REMOTE-ACCESS-E2E.md)

## Environment Variables

项目**不读任何 LLM 相关 env 变量**。provider / model / api_key / base_url 全部通过 UI Settings 配置，落盘到 DB catalog（`providers` / `models` / `app_config` 表）。历史上曾有 `ANTHROPIC_API_KEY` / `LLM_MODEL` 等 env 兜底路径，已在多 Provider catalog 架构落地后移除。

`ANTHROPIC_API_KEY` 仍作为**敏感变量名**出现在 `tools/shell.rs` 的 shell 命令环境变量脱敏清单里（执行 shell 命令前擦除），与 LLM 配置无关。

**例外（remote crate）**：`crates/everlasting-remote` 是独立服务端，读自己的 env（`EVERLASTING_REMOTE_PORT` / `EVERLASTING_REMOTE_DB_PATH` / `EVERLASTING_REMOTE_SECRET`，见 `crates/everlasting-remote/src/config.rs`），与 LLM 配置同样无关。GUI/daemon 进程仍不读任何 LLM env。

## WSL 环境注意

项目在 WSL 2 + Ubuntu 22.04 上开发。环境踩坑记录在 `docs/HACKING-wsl.md`（中文输入法、linuxbrew pkg-config、pnpm 代理、Rust 版本、cargo cache 锁、WSLg 字体等）。**新机器或怀疑环境问题时先读 HACKING-wsl**。

## Tech Stack (Locked)

| 层 | 技术 |
|---|---|
| 桌面框架 | Tauri 2（GUI 进程；daemon 化后为瘦客户端，agent core 已拆出） |
| 前端 | Vue 3 (`<script setup>`) + Vite + Pinia + reka-ui + vue-router 4 + vite-plugin-pwa（PWA 壳，远程手机访问） |
| 后端 | Rust (edition 2021) + tokio（Cargo workspace：app/src-tauri + crates/everlasting-remote(-protocol)） |
| Agent daemon | axum 0.7 + tower 0.5 + tower-http 0.6（ServeDir 同源 SPA）+ clap；`everlasting-daemon` bin，GUI 经 tauri-plugin-shell spawn 为 sidecar，默认 `httpTransport`（同源 HTTP/SSE）|
| Remote daemon（云） | `crates/everlasting-remote`：axum 0.7（ws feature）+ sqlx + dashmap + subtle + 自研 WSS 隧道协议（tokio-tungstenite 0.24）；零系统库依赖；独立 SQLite（nodes/devices/pairing_codes）|
| 前端 transport 抽象 | `app/src/transport/`（httpTransport 默认 / tauriTransport `?transport=tauri` 逃生 / pwa-remote 模式 auth.ts）|
| HTTP/LLM | reqwest + 手写 SSE + 自研 Provider trait (Anthropic / OpenAI) |
| 错误处理 | anyhow（边界）+ thiserror（领域） |
| 日志 | tracing + tracing-subscriber |
| 包管理 | pnpm（前端）、cargo（Rust） |

## Documentation

所有设计文档在 `docs/` 目录，全中文。入口与分工：
- **状态 / 排期查 [docs/ROADMAP.md](./docs/ROADMAP.md)**（技术路线图，单一 source of truth：V2 4 档分类 + 已实施粗粒度归类）
- **决策历史查 [docs/IMPLEMENTATION/](./docs/IMPLEMENTATION/)**（`decisions.md` 索引 + `decisions-2026-{06,07,08}.md` 按月分卷，只追加）
- `ARCHITECTURE.md` — 系统架构、16 阶段请求生命周期、核心决策
- `DESIGN.md` — 项目能力边界 + 硬约束(明确不做)
- `TECH.md` — 技术选型决策（锁定/候选/不用）
- `IMPLEMENTATION.md` — 决策档案(§1 自研 agent core 决策 + §4 ADR 决策日志)
- `REMOTE-ACCESS-ROADMAP.md` — 远程访问实施路线图（Phase 1/2 已落地；Phase 3 已由 remote-control epic 实施）
- `REMOTE-DEPLOY.md` — remote 云服务器部署手册（国内 2C2G + nginx + remote.sh/deploy-remote.sh）
- `REMOTE-ACCESS-E2E.md` — 远程访问 E2E 部署与验收手册
- `HACKING-llm.md` — LLM API 兼容层笔记
- `HACKING-wsl.md` — WSL 环境坑笔记
- `BACKLOG.md` — 候选功能技术评估(排期归 ROADMAP)
- `DEBUG_DB.md` — SQLite 直连调试指引(DB 路径 / schema / sqlite3 速查)
