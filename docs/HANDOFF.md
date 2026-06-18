# Handoff — 新 Session 引导

> 路线图与状态详见 [`docs/ROADMAP.md`](./ROADMAP.md)(V2 4 档分类 + 已实施粗粒度归类 + 维护承诺)。本文档只做 session 引导。
> **最近 commit hash**:用 `git log -1 --oneline` 查,本文档不再硬编码(容易滞后)。
> ⚠️ 本文档"当前进度"段会滞后于实际 commit,**权威以 `git log --oneline -20` + [ROADMAP.md](./ROADMAP.md) 为准**。

---

## 1. 项目是什么(30 秒版)

**Everlasting**:个人使用的 vibe coding 桌面工作台。Tauri 2 + Vue 3 + 自研 agent core,WSL 优先。

**核心定位**:
- 给"在 WSL 里写代码的 Windows 用户"用
- 自研 agent core(学习 harness engineering)
- 多项目 / 多 session(后续扩展)

**硬约束**([DESIGN §2.2](./DESIGN.md#22-关键约束)):
- 仅本人使用
- WSL 优先,Windows / macOS 不主动适配
- 数据本地(SQLite 单文件)
- 不包装 Claude Code / Codex SDK

---

## 2. 当前进度

> 摘要可能滞后,权威看 `git log --oneline -20` + [ROADMAP.md §1 已实施](./ROADMAP.md#1-已实施mvp-主体--路线图外完成) + [ROADMAP.md §2 V2 路线图分类](./ROADMAP.md#2-v2-路线图分类2026-06-10-重排)。

**已完成项以 `git log --oneline` 为准;本节仅列结构性 milestone**(完整 commit 列表见 `.trellis/tasks/archive/2026-06/`):
- ✅ 设计文档全套(`docs/` 下索引见 [README.md](./README.md))
- ✅ 2 份外部评审(REVIEW-glm-5.1 + REVIEW-deepseek-v4-pro)+ 3b-1 阶段 2 份专项评审(`docs/_archive/2026-06-3b-1/`)
- ✅ HACKING 系列(`HACKING-wsl.md` WSL 坑 / `HACKING-llm.md` LLM 兼容层差异 / `HACKING-markdown.md` 前端 markdown 渲染陷阱)
- ✅ MVP 主体步骤全部完成,详见 [ROADMAP.md §1.1](./ROADMAP.md#11-mvp-主体原-7-步路线图)
- ✅ 路线图外完成项,详见 [ROADMAP.md §1.2](./ROADMAP.md#12-路线图外完成)
- ✅ 完整 V2 路线图重排(2026-06-10)+ 9 文档对齐 + 顶层入口导航
- ✅ V2 第二档 2/7 落地(2026-06-12/13):**C3** context 压缩 + token 硬卡 + **A2+B7** 权限系统 + 多模式(含 3 档 Mode `edit`/`plan`/`yolo` + ⑨ 关 5-tier path-based 决策层 + ⑯ 审计日志 10 类 AuditKind + web_fetch 接入 ⑨)。详见 [ROADMAP.md §1.2](./ROADMAP.md#12-路线图外完成) + [IMPLEMENTATION §4 决策日志 2026-06-13](./IMPLEMENTATION.md#4-决策日志)。

**position bug(2026-06-14 ✅ 已解决)**:根因是 Wayland 协议禁止客户端设置窗口位置(WSLg/Weston 下 `setPosition()` 被合成器忽略,Tauri issue #14913,**非 Tauri bug,无法绕过**),故 `TitleBar.vue` 放弃手动 setSize+setPosition 铺满整屏,全平台改原生 `toggleMaximize()`。RDP 双屏环境验证通过。决策档案见 [IMPLEMENTATION §4 2026-06-14](./IMPLEMENTATION.md#4-决策日志);历史见 `.trellis/tasks/archive/2026-06/06-07-6-ui-bug-markdown-sse/prd.md` 'Progress so far'

**下一步候选**(详见 [ROADMAP.md §2](./ROADMAP.md#2-v2-路线图分类2026-06-10-重排),以 `git log` + ROADMAP 为准):
- 🟢 第一档(4 项)✅ 全部完成:A4 Token 用量 / B5 Memory(user+project)/ C1 取消机制 / D1 session 重命名
- 🟡 第二档:B3 `/command` palette ✅ / C4 审计日志查询 UI ✅ / B2 @文件补全 ✅ / D3 消息编辑重发 ✅ — **仅剩 D2 FTS5 全文搜索**
- 🟠 第三档:B4 Skill ✅(2026-06-18 落地,`skill/` 模块 + `/skill` + `use_skill` tool);余 B6 Subagent / B9 生成式 UI / C2 循环检测 / C6 大输出截断 / B1 图片 / A5-A6 打磨
- 🔴 第四档(远期,3 项):B8 DAG workflow / B10 飞书 IM / B11 云端同步

**最近 commit**:用 `git log -1 --oneline` 查,本文档不再硬编码(容易滞后)。

---

## 3. 5 分钟上手(必读顺序)

| 优先级 | 文档 | 什么时候读 |
|--------|------|------------|
| 1 | 本文件(`HANDOFF.md`) | **现在** |
| 2 | [ROADMAP.md](./ROADMAP.md) §1-2 | 看当前在哪一步 + 下一步选项 |
| 3 | [DESIGN.md §3 项目能力边界](./DESIGN.md#3-项目能力边界) | 知道"什么不做" |
| 4 | [ARCHITECTURE.md §1-2](./ARCHITECTURE.md) | 了解 16 关卡(写代码时反复查) |
| 5 | [HACKING-wsl.md](./HACKING-wsl.md) | 撞 WSL / 字体 / Rust 工具链问题时 |
| 6 | [HACKING-llm.md](./HACKING-llm.md) | 写 / 改 LLM 客户端时 |
| 7 | [HACKING-markdown.md](./HACKING-markdown.md) | 改 / 调试前端 markdown 渲染 (marked + DOMPurify) 时 |
| 8 | [spike-001](./spikes/001-wsl-tauri-window.md) | 想了解"WSL+Tauri 怎么验证"的全过程 |
| 9 | [spike-002](./spikes/002-reqwest-anthropic-sse.md) | 想了解"LLM 客户端 4 模式怎么测"的全过程 |
| 10 | [BACKLOG.md](./BACKLOG.md) | 评估新功能时 |
| 11 | [_reviews/REVIEW-glm-5.1.md](./_reviews/REVIEW-glm-5.1.md) + [_reviews/REVIEW-deepseek-v4-pro.md](./_reviews/REVIEW-deepseek-v4-pro.md) | 想看"外部怎么评"时(可选) |
| 12 | [.trellis/spec/frontend/state-management.md](../.trellis/spec/frontend/state-management.md) | 改前端 store / 流式逻辑前先读(单源 streamController + chat facade 模式) |
| 13 | [IMPLEMENTATION.md §4 决策日志](./IMPLEMENTATION.md#4-决策日志) | 想看"为什么这么做"的历史 ADR 决策 |

**目录**(完整索引见 [docs/README.md](./README.md)):
```
docs/
├── README.md                 # 索引(全部条目)
├── HANDOFF.md                # 本文件
├── ROADMAP.md                # 技术路线图(单一 source of truth)
├── DESIGN.md                 # 需求 + 边界
├── ARCHITECTURE.md           # 16 关卡请求生命周期
├── CONTEXT.md                # A4 Token 术语表(glossary)
├── TECH.md                   # 锁定的库
├── IMPLEMENTATION.md         # 决策档案(§1 自研 core + §4 ADR 日志)
├── BACKLOG.md                # 候选功能技术评估
├── HACKING-wsl.md / HACKING-llm.md / HACKING-markdown.md
├── _reviews/                 # 设计评审快照(8 份,见 README)
├── _archive/                 # 历史任务归档
└── spikes/                   # 技术验证(001-004 + bug/feature 笔记)
```

---

## 4. 如何接续(自助式)

> 这个项目演进很快,具体"下一步"用 git log 校准比读本节安全。本节给"接续动作"的通用 checklist,避免每个步骤完成后都要重写本节。

### 4.1 看清现状(必做,顺序不能颠倒)

1. `git log --oneline -20` — 看最近 commit,有"步骤 N"字样的就是路线图节点
2. `git status` — 看工作区是否干净;不干净先弄清楚是什么(可能是其他机器没 commit / 没 push 的改动)
3. 读 [ROADMAP.md §1-2](./ROADMAP.md) — 看路线图当前完成度 + 下一步候选
4. 读 [IMPLEMENTATION §4 决策日志](./IMPLEMENTATION.md#4-决策日志) 最近 1-2 条 — 看最近做了什么决策

### 4.2 选下一步

> **详细待办清单见 [ROADMAP.md §2 V2 路线图分类](./ROADMAP.md#2-v2-路线图分类2026-06-10-重排)**——本节不重复维护。

### 4.3 起手前确认环境

| 检查项 | 命令 | 期望 |
|---|---|---|
| Rust 版本 | `cargo --version` | 1.85+ |
| Node 版本 | `node --version` | 18+ |
| webkit2gtk | `pkg-config --modversion webkit2gtk-4.1` | 2.50.x |
| 字体对齐 | `fc-match "sans-serif:lang=zh"` | Noto Sans CJK SC |
| 中文输入 | `fcitx5-remote` | 不 crash,返回 0/1/2 |
| Anthropic key | `echo $ANTHROPIC_API_KEY` | 非空 |

不过关的项 → 查 [HACKING-wsl.md](./HACKING-wsl.md)(10 个坑覆盖全部环境配置)。

### 4.4 上手 build / run

```bash
cd app && pnpm tauri dev          # 启动 Vite + Tauri 窗口
cd app/src-tauri && cargo test    # 跑 Rust 单元测试
```

完整命令见 [CLAUDE.md "Common Commands" 段](../CLAUDE.md#common-commands)。

### 4.5 新步骤起点指引应该在哪写?

不在本文件。新步骤的"起点 + 验收 + 不碰范围"应写在对应 trellis 任务的 `.trellis/tasks/<task-dir>/prd.md`,本文件只留通用 checklist 避免反复过时。

---

## 5. 工具链状态(已就位,不用重装)

| 工具 | 版本 | 来源 | 备注 |
|------|------|------|------|
| Rust | 1.96.0 | linuxbrew(`/home/linuxbrew/.linuxbrew/bin/cargo`) | 1.83 太老,已升级;**用 brew 装不要用 rustup**(本机如此) |
| Node | 22.21.0 | nvm | 满足 >= 18 |
| pnpm | 9.4.0 | `/root/.local/share/pnpm` | 死代理已清 |
| webkit2gtk-4.1 | 2.50.4 | apt | 装时需 sudo,见 HACKING-wsl |
| Tauri CLI | 2.11.2(项目级) | `@tauri-apps/cli` 在 devDependencies | **不要全局装**(会跟项目级锁 cache) |
| Noto Sans CJK SC | 已装 | apt | `/etc/fonts/local.conf` 已配 |
| 系统字体默认 | `sans-serif:lang=zh` → Noto Sans CJK SC | fontconfig 修过 | fc-cache 已刷 |

`pkg-config --modversion webkit2gtk-4.1` → `2.50.4`(`PKG_CONFIG_PATH` 已持久化到 bashrc/zshrc)

---

## 6. 关键决策摘要

> **完整决策日志见 [IMPLEMENTATION.md §4 决策日志](./IMPLEMENTATION.md#4-决策日志)**——本节不重复维护。最新关键决策会先出现在那里。

---

## 7. 撞过的坑(沉淀在 HACKING 文档)

- **WSL 环境**(5 个,见 [HACKING-wsl.md](./HACKING-wsl.md)):
  - linuxbrew pkg-config 不搜系统路径
  - pnpm 死代理
  - linuxbrew Rust 1.83 太老
  - cargo cache 锁冲突
  - WSLg CJK 字体对齐(装 Noto CJK + 写 local.conf)

- **LLM 兼容层**(3 处差异,见 [HACKING-llm.md](./HACKING-llm.md)):
  - 401 `error.type` 是 `new_api_error` 不是 `authentication_error`
  - 400 类错误可能返 5xx
  - 不严格验证 max_tokens 上限

---

## 8. 关联上下文

- **项目根**:`/usr/local/code/github/everlasting/`
- **当前 branch**:`main`
- **远端**:`git@github.com:<your-github-username>/everlasting.git`,**已同步**
- **最近 commit hash**:见 `git log -1 --oneline`(本文档不再硬编码,容易滞后)
- **当前日期**:2026-06-18

---

> 本文档随项目演进更新。任何重大架构变更后,先改这里,再改具体文档。
