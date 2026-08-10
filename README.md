# Everlasting

[![CI](https://github.com/wukaijin/everlasting/actions/workflows/ci.yml/badge.svg)](https://github.com/wukaijin/everlasting/actions/workflows/ci.yml)

> 个人 vibe coding 桌面工作台。Tauri + 自研 agent core，WSL 优先。

## 这是什么

一个桌面应用，给"在 WSL 里写代码的 Windows 用户"用的 vibe coding 工作台。

不是另一个 Claude Code 替代品 —— 是同样的能力（聊、改代码、跑命令）加上三件事：

- **自研 agent core** — 自己实现 Agent Loop / Tool Calling / 流式 SSE / 16 关卡请求生命周期，不用 SDK 包装。学习 harness engineering，完全可控，不被厂商牵着走。
- **深度 WSL 集成** — 项目放 WSL 内部（`~/projects`），不走 `/mnt/c`；GUI 进程通过 WSLg / Wayland 渲染到 Windows 桌面。
- **多项目 / 多 session / 工作流** — 不是一次性对话，是一个持久的工作环境。每个 session 一个 git worktree，可并行、可互不污染、可瞬时切换。

> 这三件事哪天不再重要，这个项目就失去了存在理由。详见 [docs/DESIGN.md §2.3](./docs/DESIGN.md#23-核心差异点)。

## 状态

MVP 主体 + V2 路线图主体已落地；daemon 化（agent core 拆出独立进程，GUI 作为瘦客户端）已于 2026-07 收官，近期主线是**群聊（group chat）多参与者编排**与 review 可视化。

完整路线 / 排期 / 维护承诺见 [docs/ROADMAP.md](./docs/ROADMAP.md)（单一 source of truth）；决策历史见 [docs/IMPLEMENTATION/decisions.md](./docs/IMPLEMENTATION/decisions.md)。本文档不重复细节。

## 5 分钟上手

**前提**：
- WSL 2 + Ubuntu 22.04（Windows 11）；macOS / 纯 Linux 可跑但非主目标
- Node 22 + pnpm 10
- Rust stable + 系统 webkit2gtk-4.1 依赖（WSL 装法见 [docs/HACKING-wsl.md](./docs/HACKING-wsl.md)）
- 一个 LLM API key（Anthropic / OpenAI / GLM 等任意 Anthropic 兼容协议）

**5 步**：

```bash
# 1. clone
git clone https://github.com/wukaijin/everlasting.git && cd everlasting

# 2. 安装前端依赖
cd app && pnpm install

# 3. 启动（同时启动 Vite dev server + Tauri 窗口）
pnpm tauri dev

# 4. 窗口里：Settings → 添加 provider（Anthropic / OpenAI / 任意兼容协议）
#    填 api_key，选 default model

# 5. 新建项目 / 新建 session / 聊
```

> API key 在 UI Settings 里配置（落盘 DB catalog），**不**走 env 变量。

WSL 环境踩坑（中文输入法、linuxbrew pkg-config、Rust 工具链、字体等）走 [docs/HACKING-wsl.md](./docs/HACKING-wsl.md)，**不要在 README 复述**。

## 开发启动 & daemon 管理

日常开发用上面的 `pnpm tauri dev`（GUI sidecar 模式，自动 spawn daemon）。**纯浏览器模式**（daemon 同源服务前端 SPA，Windows 宿主浏览器访问 `http://localhost:7456/`）用 [`scripts/daemon.sh`](./scripts/daemon.sh) 管理：

```bash
./scripts/daemon.sh start            # 编译 release + 前台启动（日志打终端）
./scripts/daemon.sh bg               # 同上但后台运行（日志写 /tmp/everlasting-daemon.log）
./scripts/daemon.sh stop             # 停止（SIGTERM graceful 8s → SIGKILL 兜底）
./scripts/daemon.sh restart          # stop + bg（改前端后重新 serve dist 的最常用工作流）
./scripts/daemon.sh rebuild          # 只重新编译 release 二进制（不重启）
./scripts/daemon.sh status           # 进程状态 + GET /api/v1/health 检查
./scripts/daemon.sh logs             # tail -f 后台日志
```

通用选项：`--port N`（默认 7456）、`--no-build`（跳过编译，用现有二进制）。脚本用 PID 文件管理进程，会阻止同时跑两个 daemon（避免端口冲突 + 数据分裂）。

> 完整说明 `./scripts/daemon.sh help`；设计参考 [docs/HACKING-wsl.md](./docs/HACKING-wsl.md) §远程访问 daemon 部署。

## 能做什么

agent core 内置 24 个工具，按职能分组：

- **读写 & 文件** — 读 / 写 / 编辑（前置 ReadGuard 三道隔离 check）/ grep / glob / 列目录
- **Shell** — 前台 shell（落盘 + cat -n）/ 后台 shell（长任务，24h 保留，APPEND 到 user message 保 memory cache breakpoint）/ 状态查询 / kill
- **联网** — web 抓取（SSRF 拦截 + 5 MiB body cap + 来源标注）
- **技能 / 记忆 / UI** — Skill 调用（三层渐进披露）/ 生成式 UI 卡片（non-blocking）/ checklist 自跟踪 / 自主记忆写入
- **跨 turn 交互** — 向用户提问（支持自由输入）/ 申请切换 mode（用户 inline card 授权）
- **工作流编排** — task 状态机（Planning → Implement → Check → Done），Check→Done 触发 spec 沉淀
- **群聊编排** — nominate_speaker / end_discussion（群聊发言控制，仅 group_chat session 生效）
- **Subagent** — 派发 worker（独立 worktree 隔离）/ 合并 / 丢弃
- **Git 集成** — 每 session 一个 worktree，opt-in attach / detach / delete

### LLM Provider（自研 trait）

`AnthropicProvider` + `OpenAIProvider` 双实现，走自研 `Provider` trait + WireMessage 跨协议中间层 + retry 包装（Full Jitter / 首字节前重试 / retry-after）。早期 rig-core 计划已弃用。

### 运行形态

agent core 跑在独立 `everlasting-daemon` 进程（axum HTTP server），两种形态共享同一份 agent core：

- **Tauri GUI + sidecar daemon**（默认）— GUI 作为瘦客户端，自动 spawn daemon 子进程，前端经 `httpTransport`（同源 HTTP/SSE）通信。`?transport=tauri` + Full 模式是 daemon 故障逃生舱（回退 legacy in-process IPC）。
- **纯浏览器模式** — daemon 用 ServeDir 同源服务前端 SPA，任意浏览器开 `http://localhost:7456/` 即可用（WSL 内 daemon 经 localhost 转发可达 Windows 宿主浏览器）。管理脚本 `scripts/daemon.sh`。

详见 [docs/REMOTE-ACCESS-ROADMAP.md](./docs/REMOTE-ACCESS-ROADMAP.md) + [docs/ARCHITECTURE.md §1](./docs/ARCHITECTURE.md#1-系统架构)。

### 横切能力

- **权限系统** — 5-tier path-based 决策层 + 3 档 Mode（`edit` / `plan` / `yolo`）+ 审计日志 + `ToolKind::GitMutation`（避免 Shell 串扰）
- **Context 压缩** — token 阈值触发降级，memory 永远保护，MAX_TURNS=200 兜底
- **循环检测** — 精确签名硬触发 + Jaccard 软提示；连续触发走主动干预（用户决策）
- **网络健壮性** — `retry_open` wrapper + Full Jitter + retry-after + 双向熔断
- **自主记忆** — agent 自主产生 + 跨 session 召回 + 状态机（candidate → active → verified）+ 异步卫生 job

## 文档索引

设计文档在 [`docs/`](./docs/)，按"是什么 → 怎么搭 → 用什么 → 怎么做 → 未来"5 维拆分，**详细看 [docs/README.md](./docs/README.md)**。本文档不重复设计细节。

| 场景 | 文档 |
|---|---|
| 第一次接触 / 看项目边界 | [docs/DESIGN.md](./docs/DESIGN.md) §2-3 |
| 看当前在哪步 / 下一步选项 | [docs/ROADMAP.md](./docs/ROADMAP.md) |
| 写代码前看模块怎么分 / 调用怎么走 | [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) §2 16 关卡 |
| 选库 / 做依赖决策 | [docs/TECH.md](./docs/TECH.md) |
| 看"为什么这么做"的历史 ADR | [docs/IMPLEMENTATION/decisions.md](./docs/IMPLEMENTATION/decisions.md) |
| 评估新功能技术细节 | [docs/BACKLOG.md](./docs/BACKLOG.md) |
| daemon 化怎么分阶段落地 | [docs/REMOTE-ACCESS-ROADMAP.md](./docs/REMOTE-ACCESS-ROADMAP.md) |
| 撞 WSL / LLM API / markdown 渲染怪事 | [docs/HACKING-wsl.md](./docs/HACKING-wsl.md) / [docs/HACKING-llm.md](./docs/HACKING-llm.md) / [docs/HACKING-markdown.md](./docs/HACKING-markdown.md) |
| SQLite 直连调试 | [docs/DEBUG_DB.md](./docs/DEBUG_DB.md) |
| 术语（glossary） | [docs/CONTEXT.md](./docs/CONTEXT.md) |

## 约束（明确不做）

- 仅个人使用，非商业项目
- WSL Ubuntu 22.04 优先，Windows / macOS 不主动适配
- 不做移动端 / 云端部署 / 托管型 Web 版（注：本地浏览器模式 —— localhost 访问本机 daemon —— 是 daemon 化的副产物，不算"Web 版"；跨设备云端访问见 [BACKLOG §4](./docs/BACKLOG.md#4-跨设备)，未做）
- 不包装 Claude Code / Codex SDK（自研是学习目标）
- 不做通用 agent 框架（Cline / OpenHands 已在做）
- 不做 in-app 自动升级（走包管理器或手动）
- 不做云端触发器 / 云端触发回写本机（主动权必须在本地用户）

完整约束见 [docs/DESIGN.md §3.2](./docs/DESIGN.md#32-明确不做硬约束)。
