# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Everlasting — 个人 vibe coding 工作台。Tauri 2 + Vue 3 + Rust，自研 agent core（非 SDK 包装），WSL-first 设计。目标：与 Claude Code 同等能力（聊天、编辑代码、运行命令），但用自研的 agent harness 实现以学习 harness 工程。

**进程模型（2026-07-20 daemon 化后）**：agent core 跑在独立 `everlasting-daemon` 进程（axum HTTP server），Tauri GUI 进程作为瘦客户端，spawn daemon 为 sidecar 并经同源 HTTP/SSE 通信（默认 `httpTransport`，daemon 同时用 ServeDir 服务前端 SPA）。前端也可脱离 Tauri 用纯浏览器访问 daemon（浏览器模式）。`?transport=tauri` + Full 模式是 daemon 故障时的逃生舱（回退到一体化 Tauri IPC）。详见 [docs/REMOTE-ACCESS-ROADMAP.md](./docs/REMOTE-ACCESS-ROADMAP.md) + [docs/ARCHITECTURE.md §1](./docs/ARCHITECTURE.md)。

**当前状态(2026-07-23)**:MVP 主体 + 多 Provider + Step 8 代码重构已全部完成;memory/指令文件系统(4 文件加载 + cache_control 注入)+ per-session token usage + C3 context 压缩(token 硬卡 + MAX_TURNS 200)+ A2+B7 权限系统(⑨ 关 5-tier path-based 决策层 + 3 档 Mode `edit`/`plan`/`yolo` + ⑯ 审计日志 17 类 AuditKind)已全部落地;V2 路线图第一档 + 第二档 7/7 全部完成,第三档已落地 B4 Skill / B6 Subagent(06-21 收尾)/ B12 Checklist / C2 循环检测 / L1 后台 shell / L2 并行只读 / L3a-d subagent 全套 / RULE-D-001 api_key 加密 / V2 2 期自主记忆(06-29)/ B9 生成式 UI(07-02 部分)/ B9+ 收尾(07-13)/ E2 harness trace viewer(07-14);**daemon 化(remote-access Phase 2,07-20~07-23)**:transport 抽象层 + axum daemon(`everlasting-daemon` bin)+ sidecar spawn + httpTransport 默认 + ServeDir 浏览器模式 + E2E 测试已落地,详见 [docs/ROADMAP.md §1.2](./docs/ROADMAP.md#12-路线图外完成)。position bug(2026-06-14 ✅ 已解决,A7 收尾):根因是 Wayland 协议禁止客户端设置窗口位置(WSLg/Weston 下 `setPosition()` 被合成器忽略,Tauri issue #14913,非 Tauri bug),故 `TitleBar.vue` 放弃手动 setSize+setPosition 铺满整屏,全平台改原生 `toggleMaximize()`;RDP 双屏已验证通过,详见 [IMPLEMENTATION §4 2026-06-14](./docs/IMPLEMENTATION.md#4-决策日志)。

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
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
# 注：完整 gtk/webkit 依赖（Tauri runtime）需要 `pnpm tauri dev/build` 走 .cargo/config 路径，
#     `cargo test` 和 `cargo test --lib` 都需要 PKG_CONFIG_PATH，否则撞 gdk-pixbuf not found。

# 日志控制
RUST_LOG=debug pnpm tauri dev   # tracing 输出级别

# Daemon / 浏览器模式（daemon 化后，2026-07-20 起）
# 详见 docs/HACKING-wsl.md §远程访问 daemon 部署。
cd app/src-tauri && cargo build --bin everlasting-daemon   # 只编译 daemon bin（GUI sidecar 模式由 build.rs 自动 staging）
./scripts/daemon.sh start      # 前台启 daemon（release 二进制，同源服务前端 dist/）
./scripts/daemon.sh bg         # 后台启 daemon（PID 文件 + 日志 /tmp/everlasting-daemon.log）
./scripts/daemon.sh stop       # 停 daemon（SIGTERM graceful 8s → SIGKILL 兜底）
./scripts/daemon.sh restart    # 重启（改前端后重新 serve 新 dist 的最常用工作流）
./scripts/daemon.sh status     # health 检查（GET /api/v1/health）
./scripts/daemon.sh logs       # tail -f daemon 日志
# 浏览器模式：daemon 起来后用浏览器开 http://localhost:7456/（daemon ServeDir 同源服务 SPA）。
# GUI sidecar 模式：正常 `pnpm tauri dev/build` 即可，GUI 进程自动 spawn daemon 子进程并经 httpTransport 通信。
# 逃生舱：URL 加 ?transport=tauri + GUI 在 Full 模式（EVERLASTING_GUI_FULL_STATE=1）回退到一体化 Tauri IPC。
# ⚠️ 不要同时跑两个 daemon（会撞端口 + 数据分裂；sidecar 模式由 RunEvent::Exit 钩子自动回收）。
```

前端测试用 **vitest**（`app/vitest.config.ts`，覆盖 `app/src/**/*.test.ts`：streamController / lru / markdown / messageFormat / path / permissions / chatMode / duration / useKeyboard 等 store 与 utils）；类型安全另靠 `vue-tsc --noEmit`。Rust 单元测试走 `cargo test`（`#[cfg(test)]` 内联于各模块）。

## Architecture

> 完整结构见 [STRUCTURE.md](./STRUCTURE.md)(8-PR5 创建)。

```
app/
├── src/                    # Vue 3 前端 (8-PR3 拆分后;06-23 续拆 3 组件 + 1 composable + 3 store 模块)
│   ├── components/
│   │   ├── layout/         # AppShell / AppHeader / Sidebar / TitleBar / AppLogo / BrowserHeader(★ 浏览器模式顶栏,07-23)
│   │   ├── chat/           # ChatPanel / MessageList / ChatInput / MessageItem / ToolCallCard / DiffView / SubagentDrawer / UiCard(B9)/ WorkerBranchBadge + WorkerMergeControls(L3b PR4) 等
│   │   │                   # (06-23 拆:MessageItemEdit/Footer + SubagentDrawerHeader/ErrorCard + ChatInputLatencyPopover/HintRow;B9:UiCard + primitive registry;L3b PR4:WorkerBranchBadge/WorkerMergeControls)
│   │   ├── memory/         # MemoryPreview / MemoryModal / MemoryLayerItem
│   │   ├── settings/       # SettingsModal / ModelRow / ProvidersTab / MemoryTab 等
│   │   ├── audit/          # C4 审计日志查询 UI (AuditLogModal / AuditLogItem)
│   │   ├── trace/          # E2 harness trace viewer (TracePanel / TurnTimeline / TurnCard / TraceEventItem)
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
│   │   ├── trace.ts         # E2 harness trace store (live+回看同构 TurnTrace)
│   │   └── checklist.ts     # B12 agent 自跟踪 checklist store
│   └── utils/              # path / markdown / messageFormat / tokenUsage / lru / audit / colorTag / duration / useKeyboard / chatInputCodeMirror (06-23 拆 composable)
├── transport/             # ★ NEW (07-20 remote-access) — 前端 transport 抽象层(invoke/listen 与载体解耦)
│   ├── index.ts           # resolveTransport():默认 httpTransport;?transport=tauri 逃生 → tauriTransport
│   ├── http.ts            # httpTransport(fetch POST + SSE EventSource,连 everlasting-daemon 同源)
│   ├── tauri.ts           # tauriTransport(@tauri-apps/api invoke/listen 透传,Full 模式逃生舱)
│   ├── health.ts          # daemon health 轮询 + 自动降级(httpTransport ↔ tauriTransport)
│   ├── env.ts             # isTauriWebview():Tauri webview vs 纯浏览器检测(浏览器无 Tauri runtime)
│   └── types.ts           # Transport trait(invoke/listen 签名)+ UnlistenFn
├── src-tauri/              # Rust 后端 (8-PR1/2 拆分后;06-23 续拆 subagent/ + chat_loop + tests)
│   └── src/
│       ├── lib.rs          # Tauri 入口(纯 init + 命令注册 + sidecar spawn + RunEvent::Exit 回收)
│       ├── state.rs        # AppState 共享状态(load_inner / load_from_dir;daemon 侧也用)
│       ├── sidecar.rs      # ★ NEW (07-22 P2.4) — GuiMode{Thin,Full} + spawn_and_manage(everlasting-daemon sidecar via tauri-plugin-shell)
│       ├── main.rs         # Windows 子系统入口
│       ├── resource_loader.rs  # Markdown + frontmatter 通用加载 (Skill/Role/B3 /command 资源,parse_frontmatter 手写)
│       ├── files.rs        # 文件操作辅助
│       ├── db/             # SQLite 持久化(8-PR2 拆分, CRUD 函数分散到子模块)
│       │   ├── mod.rs / migrations.rs / types.rs / models.rs / config.rs
│       │   ├── providers.rs / projects.rs / sessions.rs / subagent_runs.rs / permissions.rs / trace.rs  # E2 turn_trace CRUD
│       │   ├── tests.rs    # (06-23 拆)6 个 `*_tests.rs` 按 SQL 域(无 common,test_pool 6 份复制)
│       ├── llm/            # LLM 客户端模块 + 自研 Provider trait + A5+ 网络健壮性
│       │   ├── provider/   # Provider trait + AnthropicProvider + OpenAIProvider + wire.rs + mock.rs
│       │   ├── retry.rs    # A5+ retry_open wrapper(Full Jitter + 首字节前重试 + retry-after 解析;07-05)
│       │   ├── sse.rs      # SseParser — 状态机式 SSE 行解析
│       │   ├── error.rs    # LlmError 5 类错误分类、中文用户消息
│       │   └── types.rs    # ContentBlock、MessageContent、ChatMessage、ToolDef、ChatEvent
│       ├── memory/         # Memory/指令文件系统(4 文件加载 + cache_control 注入)
│       │   ├── loader.rs / file.rs / watcher.rs / tokens.rs / types.rs
│       ├── agent/          # Agent Loop(8-PR1;06-23 subagent/ + chat_loop + tests;06-24 loop_detection / memory_*;07-07 question_store;07-08~10 workflow/)
│       │   ├── chat.rs / chat_loop.rs    # 主循环 + run_subagent 串联
│       │   ├── trace.rs                 # E2 trace pipeline(3 record_* 双写:emit + upsert turn_trace)
│       │   ├── context.rs               # C3 context 压缩(token 阈值 + 降级 + B5 保护)
│       │   ├── loop_detection.rs        # C2/C2+ 循环检测分级触发 + 主动干预(per-run-local count + QuestionStore)
│       │   ├── system_prompt.rs / behavior_prompt.rs / thinking.rs # prompt 与 thinking 块处理
│       │   ├── auto_reflect.rs / memory_recall.rs / memory_hygiene.rs # V2 2 期自主记忆:反思 / 召回 / 卫生 job
│       │   ├── question_store.rs        # ask_user_question / request_mode_change 跨 turn 状态(PendingInteraction tagged enum)
│       │   ├── helpers.rs / provider.rs / at_file.rs # 工具级辅助
│       │   ├── subagent/   # (06-23 拆 4 文件 + dispatch.rs)
│       │   │   ├── mod.rs / sink.rs / transcript.rs / truncate_summary.rs
│       │   │   └── dispatch.rs  # (06-23 抽自 chat_loop.rs)run_subagent + resolve_project_id + SUBAGENT_MAX_TURNS
│       │   ├── workflow/   # ★ NEW (07-08~10) — workflow 系统核心(workflow.json 外置 + task 状态机 + breadcrumb 注入)
│       │   │   ├── mod.rs (re-exports) / def.rs (WorkflowDef + 4 访问函数 + default_workflow)
│       │   │   ├── builtin.rs (builtin dev workflow plugin loader)
│       │   │   ├── inject.rs (per-turn breadcrumb + bootstrap hint + resolve_current_task 即时读盘)
│       │   │   ├── state.rs (TaskStatus state machine helpers)
│       │   │   └── task.rs (TaskJson schema + read_task lenient + create_task_init + archive_task_init)
│       │   ├── permissions/  # (06-23 拆 mod.rs → 8 模块 + 6 tests_*.rs)
│       │   │   ├── mod.rs (纯 re-exports) / types.rs / store.rs / payload.rs
│       │   │   ├── mode.rs / audit.rs / check.rs / ask.rs
│       │   │   ├── dangerous.rs / shell_trust.rs (sibling 不动)
│       │   │   └── tests_*.rs (6 个 + tests_common.rs)
│       │   ├── tests_*.rs  # (06-23 拆 tests.rs → 5 域文件 + tests_common.rs;后续追加 tests_agent_loop / tests_ask_user_question / tests_c2plus / tests_cancellation / tests_envelope / tests_prompts / tests_request_mode_change / tests_subagent)
│       ├── background_shell/  # ★ NEW (06-19 L1a) — BackgroundShellRegistry trait + InMemory impl(tokio 后台 task + drain_notifications)
│       ├── daemon/         # ★ NEW (07-20~23 remote-access) — axum HTTP daemon(everlasting-daemon bin 的核心)
│       │   ├── mod.rs      # re-exports + daemon_version
│       │   ├── server.rs   # build_router + serve_daemon(TcpListener 0.0.0.0:7456 + ServeDir 同源 SPA fallback + graceful shutdown)
│       │   ├── sse.rs      # HttpSseSink — agent loop 的 SSE 事件流广播(chat-event/tool:call/tool:result)
│       │   ├── error.rs    # DaemonError(axum IntoResponse)
│       │   └── routes/     # 79 个 #[tauri::command] 镜像为 REST 路由(同 handler 双暴露 IPC+HTTP)
│       │       └── sessions/projects/config/providers/memory/permissions/.../stream(SSE)/health 等 20 文件
│       ├── bin/            # ★ NEW (07-20) — cargo bin targets
│       │   └── everlasting-daemon.rs  # daemon 进程入口(resolve_data_dir + clap --port/--data-dir + serve_daemon)
│       ├── skill/          # Skill 系统(资源加载 + 注册,/skill + use_skill tool)
│       ├── commands/       # Tauri commands(8-PR1 拆分;07-03 subagents / 07-07 question / 07-08~10 task)
│       │   ├── task.rs # ★ NEW (07-08~10) — task.json CRUD IPC(create_task / read_task / set_task_state / archive_task)
│       │   ├── subagents.rs # ★ NEW (07-03) — subagent model override IPC(list_subagents_with_model + set_subagent_model)
│       │   ├── question.rs # ★ NEW (07-07) — get_pending_interaction + resolve_mode_change
│       │   └── 其他: sessions/projects/config/cancel/providers/worktree/memory/permissions/command_palette/panel/files/subagent_runs/audit/checklist
│       ├── projects/       # Project 数据模型 + boundary 校验
│       ├── git/            # git2-rs worktree + diff
│       └── tools/          # Tool 定义与执行(21 个 builtin,mod.rs::builtin_tools() 注册;filter_tools_for_mode/subagent/workflow 三层过滤)
│           ├── mod.rs       # builtin_tools()、execute_tool() 分发、ToolKind/GitMutation(WebFetch式 tool-level grant)
│           ├── read_file.rs / write_file.rs / edit_file.rs / grep.rs / glob.rs / list_dir.rs  # L2 并发只读集
│           ├── shell.rs / run_background_shell.rs / shell_status.rs / shell_kill.rs  # L1a 后台 shell(tokio Child,无 PTY)
│           ├── web_fetch.rs   # P1 web 抓取(SSRF 拦截 + 5 MiB body cap)
│           ├── use_skill.rs   # B4 Skill 调用 tool(三层渐进披露 L0/L1/L2,workflow-aware)
│           ├── use_ui.rs      # B9 生成式 UI(non-blocking execute + UiPrimitive registry)
│           ├── update_checklist.rs # B12 agent 自跟踪 checklist tool(loop-local + workflow 分支同步 task.json.items)
│           ├── remember.rs    # V2 2 期自主记忆写入 tool
│           ├── ask_user_question.rs  # 跨 turn 问题(selector 复用,query_store 配对)
│           ├── dispatch_subagent.rs # B6 派 worker agent
│           ├── merge_worker.rs / discard_worker.rs  # L3b worker worktree 收口(ToolKind::GitMutation)
│           ├── create_task.rs / request_task_state_transition.rs  # ★ NEW (07-08/10) workflow tools(workflow_enabled session 可见,filter_tools_for_workflow 白名单)
│           ├── request_mode_change.rs  # ★ NEW (07-07 B6+ A) LLM 申请切 mode(用户 inline card 授权)
│           └── read_guard.rs  # session 隔离的已读文件校验(edit_file 前置 3 道 check)
docs/                       # 设计文档(全中文,spikes/ 在 docs/ 下而非项目根)
```

### 核心数据流

前端 `ChatWindow.vue`（侧边栏 + chat 区）→ Pinia `chat.ts send()` → `transport.invoke("chat", { requestId, sessionId, messages })`（**默认 `httpTransport`**：fetch POST 到 daemon `/api/v1/...`；`?transport=tauri` 逃生时走 Tauri IPC 同进程）→ Rust `chat` handler（daemon 进程的 axum 路由，或 Full 模式下的 Tauri command；两者共享同一 `#[tauri::command]`/REST 双暴露 handler）→ **Agent Loop**（max 200 turns）→ 每轮开头通过 `build_instructions_blocks()` 构造带 `cache_control` 的 synthetic user message（4 个指令文件: User CLAUDE.md / User AGENTS.md / Project CLAUDE.md / Project AGENTS.md）+ 工具前 V2 2 期 `memory_recall` 召回 + C3 context 压缩降级 → `chat_stream_with_tools()` 请求 LLM API → SSE 流式解析（BlockState 状态机处理 text/tool_use）→ 高频事件 `chat-event`（delta/start/done/error）+ 低频独立事件 `tool:call` / `tool:result` → 经 daemon 的 `HttpSseSink`（`daemon/sse.rs`）同源 SSE 广播给前端（Full 模式则经 Tauri event）→ L2 并发只读集 `FuturesUnordered` 批量执行 + 写类 / shell 串行 → 构造 tool_result 回填 → 再发 LLM → 直到 text-only 响应或 max turns。**Turn 边界**调 `db::persist_turn` 落 SQLite（daemon 进程持有 DB pool），session 列表从 DB 读。前端 Pinia store 多 listener 监听（`transport.listen`），增量更新消息 + 工具卡片。

### 关键架构决策

- **自研 agent core**：不使用 Anthropic Agent SDK / Codex SDK，自己实现 Agent Loop、消息管理、tool 注册、权限检查（见 `docs/IMPLEMENTATION.md §1`）
- **步骤 1 用手写 SSE 解析**：不用 eventsource-stream crate，`llm/sse.rs` 是自研状态机（已通过 spike-002 验证）
- **自研 Provider trait（多 Provider 抽象）**：`llm/provider/` 定义 `Provider` trait，`AnthropicProvider` / `OpenAIProvider` 两个实现 + `wire.rs` WireMessage 跨协议中间层（2026-06-08/09 落地，取代早期 rig-core 计划）
- **16 阶段请求生命周期**：完整的 agent 请求处理管线，定义在 `docs/ARCHITECTURE.md`
- **Memory/指令文件系统**：4 个指令文件（User/Project × CLAUDE.md/AGENTS.md）固定路径加载 + notify 监听 + `build_instructions_blocks()` 构造带 `cache_control: ephemeral` 的 synthetic user message，实现 prompt caching（2026-06-11 B5 重构落地）
- **daemon 化（已落地，2026-07-20~23 remote-access Phase 2）**：agent core 从 Tauri GUI 进程拆出为独立 `everlasting-daemon` 进程（axum HTTP server），GUI 作为瘦客户端 spawn daemon 为 sidecar，经**同源 HTTP + SSE** 通信（`httpTransport` 默认）；daemon 用 `tower-http::ServeDir` 同源服务前端 SPA，支持纯浏览器访问（浏览器模式）。`?transport=tauri` + Full 模式是 daemon 故障逃生舱（回退一体化 Tauri IPC）。决策动机见 [docs/IMPLEMENTATION.md §4](./docs/IMPLEMENTATION.md)，编排放 [docs/REMOTE-ACCESS-ROADMAP.md](./docs/REMOTE-ACCESS-ROADMAP.md)

## Environment Variables

项目**不读任何 LLM 相关 env 变量**。provider / model / api_key / base_url 全部通过 UI Settings 配置，落盘到 DB catalog（`providers` / `models` / `app_config` 表）。历史上曾有 `ANTHROPIC_API_KEY` / `LLM_MODEL` 等 env 兜底路径，已在多 Provider catalog 架构落地后移除。

`ANTHROPIC_API_KEY` 仍作为**敏感变量名**出现在 `tools/shell.rs` 的 shell 命令环境变量脱敏清单里（执行 shell 命令前擦除），与 LLM 配置无关。

## WSL 环境注意

项目在 WSL 2 + Ubuntu 22.04 上开发。环境踩坑记录在 `docs/HACKING-wsl.md`（中文输入法、linuxbrew pkg-config、pnpm 代理、Rust 版本、cargo cache 锁、WSLg 字体等）。**新机器或怀疑环境问题时先读 HACKING-wsl**。

## Tech Stack (Locked)

| 层 | 技术 |
|---|---|
| 桌面框架 | Tauri 2（GUI 进程；daemon 化后为瘦客户端，agent core 已拆出） |
| 前端 | Vue 3 (`<script setup>`) + Vite + Pinia + reka-ui |
| 后端 | Rust (edition 2021) + tokio |
| Agent daemon | axum 0.7 + tower 0.5 + tower-http 0.6（ServeDir 同源 SPA）+ clap；`everlasting-daemon` bin，GUI 经 tauri-plugin-shell spawn 为 sidecar，默认 `httpTransport`（同源 HTTP/SSE）|
| 前端 transport 抽象 | `app/src/transport/`（httpTransport 默认 / tauriTransport `?transport=tauri` 逃生）|
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
- `HACKING-llm.md` — LLM API 兼容层笔记
- `HACKING-wsl.md` — WSL 环境坑笔记
- `BACKLOG.md` — 候选功能技术评估(排期归 ROADMAP)
- `DEBUG_DB.md` — SQLite 直连调试指引(DB 路径 / schema / sqlite3 速查)
