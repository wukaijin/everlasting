<div align="center">

<img src="brand/png/logo-app-icon-512.png" width="120" alt="Everlasting logo">

# Everlasting

**个人 vibe coding 桌面工作台 —— 给在 WSL 里写代码的你**

*Everlasting — a personal vibe-coding workbench for WSL.*

[![CI](https://github.com/wukaijin/everlasting/actions/workflows/ci.yml/badge.svg)](https://github.com/wukaijin/everlasting/actions/workflows/ci.yml)

</div>

---

## 这是什么

一个跑在 WSL 里的桌面 AI 编程工具：跟 agent 聊天，它帮你读代码、改文件、跑命令。和市面上的同类工具能力相当，但底层完全自研——不用任何厂商 SDK 包装，Agent Loop、工具调用、流式输出全部自己实现。

和"一次性对话"的工具不同，它是一个**持久的工作环境**：

- **自研 agent core** — 自己实现 Agent Loop / Tool Calling / 流式 SSE / 18+ 关卡请求生命周期，学习 harness engineering，完全可控
- **深度 WSL 集成** — 项目放在 WSL 内部（不走 `/mnt/c`），GUI 通过 WSLg / Wayland 渲染到 Windows 桌面
- **多项目 / 多 session / 工作流** — 每个 session 一个独立 git worktree，可并行、可互不污染、可瞬时切换

## 特色功能

<table>
<tr>
<td width="50%">

**图片输入**

粘贴 / 拖拽图片就能聊，支持视觉模型读图分析。

</td>
<td width="50%">

**定时任务**

本地 cron 式调度，让 agent 按设定时间自动干活。

</td>
</tr>
<tr>
<td width="50%">

**跨会话搜索**

Cmd+K 一键检索全部历史会话，agent 也能自己搜。

</td>
<td width="50%">

**手机远程**

手机浏览器（PWA）经加密隧道远程操作家里的电脑。

</td>
</tr>
</table>

## 快速开始

**前提**：

- WSL 2 + Ubuntu 22.04（Windows 11）；macOS / 纯 Linux 可跑但非主目标
- Node 22 + pnpm 10
- Rust stable + 系统 webkit2gtk-4.1 依赖（WSL 装法见 [docs/HACKING-wsl.md](./docs/HACKING-wsl.md)）
- 一个 LLM API key（Anthropic / OpenAI / GLM 等任意 Anthropic 兼容协议）

> 没有 WSL？这个项目专为 WSL 设计，建议先装 WSL 2 再回来。

**5 步跑起来**：

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

> API key 在 UI Settings 里配置（落盘 DB catalog），不读环境变量。

## 运行形态

| 形态 | 说明 |
|------|------|
| **桌面 GUI**（默认） | `pnpm tauri dev`，agent core 跑在独立 daemon 进程，GUI 作为瘦客户端自动管理 |
| **纯浏览器** | daemon 同源服务前端 SPA，浏览器开 `http://localhost:7456/` 即用（日常管理脚本 `scripts/daemon.sh`） |

## 文档

- **项目边界 / 明确不做** — [docs/DESIGN.md](./docs/DESIGN.md)
- **技术路线图（单一 source of truth）** — [docs/ROADMAP.md](./docs/ROADMAP.md)
- **系统架构 / 请求生命周期** — [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md)
- **WSL 环境坑笔记** — [docs/HACKING-wsl.md](./docs/HACKING-wsl.md)

全量设计文档索引见 [docs/README.md](./docs/README.md)。
