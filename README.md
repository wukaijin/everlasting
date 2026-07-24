# Everlasting

[![CI](https://github.com/wukaijin/everlasting/actions/workflows/ci.yml/badge.svg)](https://github.com/wukaijin/everlasting/actions/workflows/ci.yml)

> 个人 vibe coding 桌面工作台。Tauri + 自研 agent core,WSL 优先。

## 这是什么

一个桌面应用,给"在 WSL 里写代码的 Windows 用户"用的 vibe coding 工作台。

不是另一个 Claude Code 替代品 —— 是同样的能力(聊、改代码、跑命令)加上三件事:

- **自研 agent core** — 自己实现 Agent Loop / Tool Calling / 流式 SSE / 16 关卡请求生命周期,不用 SDK 包装。学习 harness engineering,完全可控,不被厂商牵着走。
- **深度 WSL 集成** — 项目放 WSL 内部(`~/projects`),不走 `/mnt/c`;GUI 进程通过 WSLg / Wayland 渲染到 Windows 桌面。
- **多项目 / 多 session / 工作流** — 不是一次性对话,是一个持久的工作环境。每个 session 一个 git worktree,可并行、可互不污染、可瞬时切换。

> 这三件事哪天不再重要,这个项目就失去了存在理由。详见 [docs/DESIGN.md §2.3](./docs/DESIGN.md#23-核心差异点)。

## 状态

当前:2026-07-24。MVP 主体 + V2 第一/二/三档 25 项已全部落地 + **daemon 化(remote-access epic,07-20~23)**;详见 [docs/ROADMAP.md §1](./docs/ROADMAP.md#1-已实施mvp-主体--路线图外完成)。

完整提交历史:`git log --oneline -20`。最近里程碑(daemon 化 remote-access / E2 trace viewer / B9+ 生成式 UI 收尾 / V2-2+ 自主记忆面板 / C2+ 循环检测主动干预 / A2+ shell 分类 / A5+ 网络健壮性 / B6+ subagent 多模型 / B8 workflow 编排)见 git log,本文档不重复。

## 5 分钟上手

**前提**:
- WSL 2 + Ubuntu 22.04(Windows 11);macOS / 纯 Linux 可跑但非主目标
- Node 22 + pnpm 10
- Rust stable + 系统 webkit2gtk-4.1 依赖(WSL 装法见 [docs/HACKING-wsl.md](./docs/HACKING-wsl.md))
- 一个 LLM API key(Anthropic / OpenAI / GLM 等任意 Anthropic 兼容协议)

**5 步**:

```bash
# 1. clone
git clone https://github.com/wukaijin/everlasting.git && cd everlasting

# 2. 安装前端依赖
cd app && pnpm install

# 3. 启动(同时启动 Vite dev server + Tauri 窗口)
pnpm tauri dev

# 4. 窗口里:Settings → 添加 provider(Anthropic / OpenAI / 任意兼容协议)
#    填 api_key,选 default model

# 5. 新建项目 / 新建 session / 聊
```

> API key 在 UI Settings 里配置(落盘 DB catalog),**不**走 env 变量。

WSL 环境踩坑(中文输入法、linuxbrew pkg-config、Rust 工具链、字体等)走 [docs/HACKING-wsl.md](./docs/HACKING-wsl.md),**不要在 README 复述**。

## 能力矩阵

### 读写 & 文件
`read_file` / `write_file` / `edit_file`(ReadGuard 三道 check 前置) / `grep` / `glob` / `list_dir`

### Shell
`shell`(Bash 落盘 + cat -n) / `run_background_shell` / `shell_status` / `shell_kill`(L1a,24h 后台,APPEND user message 保 memory cache breakpoint)

### 联网
`web_fetch`(SSRF 拦截 + 5 MiB body cap + attribution prefix)

### 技能 / 记忆 / UI
`use_skill`(B4 三层渐进披露,workflow-aware) / `use_ui`(B9 生成式 UI,non-blocking) / `update_checklist`(B12 loop-local + workflow 分支同步 task.json.items) / `remember`(V2 2 期自主记忆写入)

### 跨 turn 交互
`ask_user_question`(QuestionStore + selector 复用 B9) / `request_mode_change`(LLM 申请切 mode,用户 inline card 授权,Yolo 走二次 modal 守门)

### 工作流编排(B8,07-08~10 完整落地)
`create_task` / `request_task_state_transition`(task.json 四态状态机 Planning → Implement → Check → Done;workflow_enabled session 可见;set_task_state → Done 触发 spec distillation 沉淀到 `.everlasting/spec/`)

### Subagent(B6 + L3a-d)
`dispatch_subagent` / `merge_worker` / `discard_worker`(L3b worker worktree 隔离 + branch 前缀 + libgit2 merge + 启动 sweep)

### Git 集成
worktree 解耦 + opt-in attach / detach / delete;每个 session 一个 worktree

### LLM Provider(自研 trait)
Anthropic / OpenAI 双 Provider(rig-core 2026-06-09 弃用,改自研 `Provider` trait 走双 Provider + retry 包装)

### 运行形态(daemon 化后,07-20~23)
agent core 跑在独立 `everlasting-daemon` 进程(axum HTTP server),两种形态共享同一份 agent core:
- **Tauri GUI + sidecar daemon**(默认):GUI 作为瘦客户端,自动 spawn daemon 子进程,前端经 `httpTransport`(同源 HTTP/SSE)通信。`?transport=tauri` + Full 模式是 daemon 故障逃生舱(回退 legacy in-process IPC)。
- **纯浏览器模式**:daemon 用 ServeDir 同源服务前端 SPA,任意浏览器开 `http://localhost:7456/` 即可用(WSL 内 daemon 经 localhost 转发可达 Windows 宿主浏览器);前端 `isTauriWebview()`=false 时用 `BrowserHeader` 替代 `TitleBar`。管理脚本 `scripts/daemon.sh`。
- 详见 [docs/REMOTE-ACCESS-ROADMAP.md](./docs/REMOTE-ACCESS-ROADMAP.md) + [docs/ARCHITECTURE.md §1](./docs/ARCHITECTURE.md#1-系统架构)。

### 横切
- **A2+B7 权限系统**:⑨ 关 5-tier path-based 决策层 + 3 档 Mode(`edit` / `plan` / `yolo`) + ⑯ 审计日志 25 类 AuditKind + `ToolKind::GitMutation`(L3b PR3+,避免 Shell 串扰)
- **C3 Context 压缩**:`context_window * 0.80` → `0.50` 触发,B5 memory 永远保护,MAX_TURNS=200 兜底
- **C2/C2+ 循环检测**:L1 精确签名硬触发 N=3 + L2 Jaccard 软提示 N=5/0.85;连续 3 次触发走主动干预(QuestionStore 复用 + 用户决策)
- **A5+ 网络健壮性**:`retry_open` wrapper + Full Jitter + retry-after advisory + 首字节前重试 + 双向熔断
- **V2 2 期 自主记忆**:agent 自主产生 + 跨 session 召回 + 状态机 candidate→active→verified + 异步卫生 job

## 文档索引

设计文档在 [`docs/`](./docs/),按"是什么 → 怎么搭 → 用什么 → 怎么做 → 未来"5 维拆分,**详细看 [docs/README.md](./docs/README.md)**。本文档不重复设计细节。

| 场景 | 文档 |
|---|---|
| 第一次接触 / 看项目边界 | [docs/DESIGN.md](./docs/DESIGN.md) §2-3 |
| 看当前在哪步 / 下一步选项 | [docs/ROADMAP.md](./docs/ROADMAP.md) |
| 写代码前看模块怎么分 / 调用怎么走 | [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) §2 16 关卡 |
| 选库 / 做依赖决策 | [docs/TECH.md](./docs/TECH.md) |
| 看"为什么这么做"的历史 ADR | [docs/IMPLEMENTATION.md §4](./docs/IMPLEMENTATION.md#4-决策日志) |
| 评估新功能技术细节 | [docs/BACKLOG.md](./docs/BACKLOG.md) |
| 撞 WSL / LLM API / markdown 渲染怪事 | [docs/HACKING-wsl.md](./docs/HACKING-wsl.md) / [docs/HACKING-llm.md](./docs/HACKING-llm.md) / [docs/HACKING-markdown.md](./docs/HACKING-markdown.md) |
| SQLite 直连调试 | [docs/DEBUG_DB.md](./docs/DEBUG_DB.md) |
| 术语(glossary) | [docs/CONTEXT.md](./docs/CONTEXT.md) |

## 约束(明确不做)

- 仅个人使用,非商业项目
- WSL Ubuntu 22.04 优先,Windows / macOS 不主动适配
- 不做移动端 / 云端部署 / 托管型 Web 版(注:本地浏览器模式 —— localhost 访问本机 daemon —— 是 daemon 化的副产物,不算"Web 版";跨设备云端访问见 [BACKLOG §4](./docs/BACKLOG.md#4-跨设备),未做)
- 不包装 Claude Code / Codex SDK(自研是学习目标)
- 不做通用 agent 框架(Cline / OpenHands 已在做)
- 不做 in-app 自动升级(走包管理器或手动)
- 不做云端触发器 / 云端触发回写本机(主动权必须在本地用户)

完整约束见 [docs/DESIGN.md §3.2](./docs/DESIGN.md#32-明确不做硬约束)。