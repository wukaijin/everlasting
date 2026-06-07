# Handoff — 新 Session 引导

> **2026-06-07 更新**。当前阶段:**MVP 步骤 1 / 2 / 3a / 3b-1 已完成 + 路线图外完成 extended thinking + spike-005 follow-up 7 PR(UI/UX 修复 + 工具稳定性 + 打断机制 + markdown + git_branch + pwd `~/` 简化) + 字体栈调整 + 6 个 UI/状态 bug 修复**。步骤 3b-2(完整三栏 UI + rig-core 迁移)仍暂缓。
> spike-001/002/003/004/005 均已通过,工具链就位,环境坑沉淀(`HACKING-wsl.md` WSL 坑 + `HACKING-llm.md` LLM 兼容层差异 + `HACKING-markdown.md` 前端 markdown 渲染陷阱)。
> ⚠️ 本文档"当前进度"段会滞后于实际 commit,**权威以 `git log --oneline -20` + [IMPLEMENTATION §3 路线图](./IMPLEMENTATION.md#3-待办与下一步)为准**。

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

> 摘要可能滞后,权威看 `git log --oneline -20` + [IMPLEMENTATION §3](./IMPLEMENTATION.md#3-待办与下一步)。

**已完成**(2026-06-07 累计):
- ✅ 设计文档全套(`docs/` 下索引见 [README.md](./README.md))
- ✅ 2 份外部评审(REVIEW-glm-5.1 + REVIEW-deepseek-v4-pro)+ 3b-1 阶段 2 份专项评审(`docs/_archive/2026-06-3b-1/`)
- ✅ HACKING 系列(`HACKING-wsl.md` WSL 坑 / `HACKING-llm.md` LLM 兼容层差异 / `HACKING-markdown.md` 前端 markdown 渲染陷阱)
- ✅ **MVP 步骤 1 — 骨架 + LLM 直连**(commit `08dc818`,2026-06-04)
- ✅ **MVP 步骤 2 — Tool Calling + Agent Loop**(commit `fefc41f`,2026-06-04)
- ✅ **MVP 步骤 3a — SQLite + Session 持久化**(commit `0ce44b5`,2026-06-05;rehydrate 补丁 `a89a6fd`)
- ✅ **路线图外完成**:Anthropic extended thinking 块展示 + 持久化(commit `05671f5`,2026-06-05)
- ✅ **MVP 步骤 3b-1 — 项目基础结构 + 顶部 Tabs UI**(后端 PR1 `fefc41f` 之前的前置 + 前端 PR2 `93a0753` + post-fixes squash `18354a0` + docs follow-up `7e888c9`,2026-06-05/06)
- ✅ **spike-005 follow-up — 7 PR 合并**(commit `401396b`,2026-06-06):UI 紧凑 header (`801fb8a`) + git_branch 显示 (`8f25b7f`) + 启动 batch backfill (`7ce3209`) + pwd `~/` 简化数据通路 (`ef7cea8`) + write_file tracing (`ae1a711`) + LLM cancel 机制 (`11f01c6`) + markdown 渲染 (`cb41bcb`) + 首行空白修复 (`cfb7aac`)
- ✅ **字体栈调整**(commit `aabb9fa`,2026-06-06):HarmonyOS Sans SC 子集打包 + Dark theme 下中文渲染改善,沉淀到 `.trellis/spec/frontend/cjk-fonts.md`
- ✅ **6 UI/状态 bug 修复**(commits `bd5ea7b` + `abde429` + `bf9b35b`,2026-06-07):顶栏窗口控制 (bug 1+2 size / bug 3 minimize icon / bug 4 logo padding) + Markdown 表格 border (bug 5) + Tauri 2 权限补全 + streamController 状态架构重构 (bug 6)
- ✅ trellis 任务管理工作流引入(`.trellis/` 目录,commit `402afa5`)

**当前状态**:
- ⏸ 步骤 3b-2(完整三栏 UI + rig-core 迁移)仍暂缓
- ⏸ 步骤 4(Git 集成 — worktree + auto commit)未开始
- 🐛 已知 issue:**bug 1+2 position 在 RDP 双显示器下未完全修好**(窗口 grow rightward 而非贴 host 主屏左上角,候选 `setFullscreen(true)` 兜底会丢 maximize 语义 — 见 `.trellis/tasks/archive/2026-06/06-07-6-ui-bug-markdown-sse/prd.md` 'Progress so far')
- 下一步候选(详见 [IMPLEMENTATION §3](./IMPLEMENTATION.md#3-待办与下一步)):
  - 跳过 3b-2 继续主线 → 步骤 4 Git 集成(worktree + auto commit)
  - 或回头补完 3b-2(完整三栏 UI + Rig 迁移)
  - 或先收尾 bug 1+2 position(setFullscreen 兜底 vs 继续找正确 fix)

**最近 commit**:用 `git log -1 --oneline` 查,本文档不再硬编码(容易滞后)。

---

## 3. 5 分钟上手(必读顺序)

| 优先级 | 文档 | 什么时候读 |
|--------|------|------------|
| 1 | 本文件(`HANDOFF.md`) | **现在** |
| 2 | [IMPLEMENTATION.md §3 路线图全貌表](./IMPLEMENTATION.md#3-待办与下一步) | 看当前在哪一步 + 下一步选项 |
| 3 | [DESIGN.md §2.2 关键约束](./DESIGN.md#22-关键约束) | 知道"什么不做" |
| 4 | [ARCHITECTURE.md §1-2](./ARCHITECTURE.md) | 了解 16 关卡(写代码时反复查) |
| 5 | [HACKING-wsl.md](./HACKING-wsl.md) | 撞 WSL / 字体 / Rust 工具链问题时 |
| 6 | [HACKING-llm.md](./HACKING-llm.md) | 写 / 改 LLM 客户端时 |
| 7 | [HACKING-markdown.md](./HACKING-markdown.md) | 改 / 调试前端 markdown 渲染 (marked + DOMPurify) 时 |
| 8 | [spike-001](./spikes/001-wsl-tauri-window.md) | 想了解"WSL+Tauri 怎么验证"的全过程 |
| 9 | [spike-002](./spikes/002-reqwest-anthropic-sse.md) | 想了解"LLM 客户端 4 模式怎么测"的全过程 |
| 10 | [BACKLOG.md](./BACKLOG.md) | 评估新功能时 |
| 11 | [_reviews/REVIEW-glm-5.1.md](./_reviews/REVIEW-glm-5.1.md) + [_reviews/REVIEW-deepseek-v4-pro.md](./_reviews/REVIEW-deepseek-v4-pro.md) | 想看"外部怎么评"时(可选) |
| 12 | [.trellis/spec/frontend/state-management.md](../.trellis/spec/frontend/state-management.md) | 改前端 store / 流式逻辑前先读(单源 streamController + chat facade 模式) |

**目录**:
```
docs/
├── README.md                 # 索引
├── HANDOFF.md                # 本文件
├── DESIGN.md                 # 需求 + 边界
├── ARCHITECTURE.md           # 16 关卡 + Channel Adapter
├── TECH.md                   # 锁定的库
├── IMPLEMENTATION.md         # 7 步路线图 + 决策日志
├── BACKLOG.md                # 7 个候选功能
├── HACKING-wsl.md            # 10 个 WSL 环境坑 + fcitx5 输入法
├── HACKING-llm.md            # LLM 兼容层差异
├── HACKING-markdown.md       # 前端 markdown 渲染陷阱 (marked + DOMPurify)
├── HANDOFF.md                # 本文件
├── _reviews/
│   ├── REVIEW-glm-5.1.md         # 外部评审 #1
│   └── REVIEW-deepseek-v4-pro.md # 外部评审 #2
└── spikes/
    ├── 001-wsl-tauri-window.md
    └── 002-reqwest-anthropic-sse.md
```

---

## 4. 如何接续(自助式)

> 这个项目演进很快,具体"下一步"用 git log 校准比读本节安全。本节给"接续动作"的通用 checklist,避免每个步骤完成后都要重写本节。

### 4.1 看清现状(必做,顺序不能颠倒)

1. `git log --oneline -20` — 看最近 commit,有"步骤 N"字样的就是路线图节点
2. `git status` — 看工作区是否干净;不干净先弄清楚是什么(可能是其他机器没 commit / 没 push 的改动)
3. 读 [IMPLEMENTATION §3 路线图全貌表](./IMPLEMENTATION.md#3-待办与下一步) — 看路线图当前完成度
4. 读 [IMPLEMENTATION §4 决策日志](./IMPLEMENTATION.md#4-决策日志) 最近 1-2 条 — 看最近做了什么决策

### 4.2 选下一步

> **详细待办清单见 [IMPLEMENTATION.md §3 待办与下一步](../IMPLEMENTATION.md#3-待办与下一步)**——本节不重复维护。

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

> **完整决策日志见 [IMPLEMENTATION.md §4 决策日志](../IMPLEMENTATION.md#4-决策日志)**——本节不重复维护。最新关键决策会先出现在那里。

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
- **远端**:`git@github.com:wukaijin/everlasting.git`,**已同步**
- **最近 commit hash**:见 `git log -1 --oneline`(本文档不再硬编码,容易滞后)
- **当前日期**:2026-06-07

---

> 本文档随项目演进更新。任何重大架构变更后,先改这里,再改具体文档。
