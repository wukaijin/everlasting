# Handoff — 新 Session 引导

> **2026-06-05 更新**。当前阶段:**MVP 步骤 3a 已完成，准备进入步骤 3b (多项目 + UI 三栏 + Rig 迁移)**。
> spike-001/002 已通过，前置硬依赖清零，工具链就位，环境坑已沉淀。
> **session 1** (2026-06-04):设计文档 + spike-001/002。
> **session 2** (2026-06-04):MVP 步骤 1 实施（骨架 + LLM 直连）。
> **session 2** (2026-06-04):搬骨架、写 LLM 客户端、IPC 桥、ChatWindow，11 个 Rust 单元测试 + pnpm build + cargo build 通过，`pnpm tauri dev` 已能启动(WebKit 进程在 WSL 内)。详见 §"已完成"。

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

**已完成**(2026-06-05 累计):
- ✅ 5 份设计文档(README + DESIGN + ARCHITECTURE + TECH + IMPLEMENTATION + BACKLOG)
- ✅ 2 份外部评审(REVIEW-glm-5.1 + REVIEW-deepseek-v4-pro)
- ✅ HANDOFF + 2 个 spike 模板
- ✅ 2 份 HACKING 文档(`HACKING-wsl.md` 10 个 WSL 坑 / `HACKING-llm.md` GLM 兼容层差异)
- ✅ **MVP 步骤 1 — 骨架 + LLM 直连**(session 2 完成)
- ✅ **MVP 步骤 2 — Tool Calling + Agent Loop**(session 3 完成)
- ✅ **MVP 步骤 3a — SQLite + Session 持久化**(session 4 完成)

**session 4 (步骤 3a 实施)**:
- ✅ Cargo.toml 加 sqlx / uuid / chrono 依赖
- ✅ 新增 `src-tauri/src/db.rs`（init_pool / run_migrations / 8 个 CRUD 函数 + 9 个测试）
- ✅ DB schema：sessions + messages 表，content 是 JSON (Vec<ContentBlock> round-trip 0 损失)
- ✅ lib.rs：AppState 加 db、chat 入参加 session_id、turn 边界 persist_turn、注册 4 个新 command (list/create/load/delete)
- ✅ 前端 session store：sessions / currentSessionId / loadSessions / createNewSession / switchSession / deleteSession
- ✅ 前端侧边栏 UI：+ 新对话 / 列表 / 切换 / 删除按钮
- ✅ 默认模型改为 MiniMax-M2.7
- ✅ 42 个 Rust 测试全过，pnpm build 通过
- ⚠️ **未 commit**（改动在工作区，待新 session 验证后 commit）

**当前任务**(下一步):
- → [IMPLEMENTATION §2.4 步骤 3b — 多项目 + UI 三栏 + Rig 迁移](./IMPLEMENTATION.md#24-步骤-3b--多项目--ui-三栏--rig-迁移-mvp)
- LLM client 从 reqwest 切到 rig-core
- 引入 Project 概念，左侧项目列表
- UI 重构：左侧项目列表 + 中间 session 列表 + 右侧 chat
- 记得先 commit session 4 的改动

**最近 commit**:
```
1bcc9e8 docs: 更新 HANDOFF + CLAUDE.md 反映步骤 2 完成
```

---

## 3. 5 分钟上手(必读顺序)

| 优先级 | 文档 | 什么时候读 |
|--------|------|------------|
| 1 | 本文件(`HANDOFF.md`) | **现在** |
| 2 | [IMPLEMENTATION.md §2.1](./IMPLEMENTATION.md#21-步骤-1--骨架与-llm-直连-mvp) | 了解 MVP 1 范围 |
| 3 | [DESIGN.md §2.2 关键约束](./DESIGN.md#22-关键约束) | 知道"什么不做" |
| 4 | [ARCHITECTURE.md §1-2](./ARCHITECTURE.md) | 了解 16 关卡(写代码时反复查) |
| 5 | [HACKING-wsl.md](./HACKING-wsl.md) | 撞 WSL / 字体 / Rust 工具链问题时 |
| 6 | [HACKING-llm.md](./HACKING-llm.md) | 写 / 改 LLM 客户端时 |
| 7 | [spike-001](./spikes/001-wsl-tauri-window.md) | 想了解"WSL+Tauri 怎么验证"的全过程 |
| 8 | [spike-002](./spikes/002-reqwest-anthropic-sse.md) | 想了解"LLM 客户端 4 模式怎么测"的全过程 |
| 9 | [BACKLOG.md](./BACKLOG.md) | 评估新功能时 |
| 10 | [REVIEW-glm-5.1.md](./REVIEW-glm-5.1.md) + [REVIEW-deepseek-v4-pro.md](./REVIEW-deepseek-v4-pro.md) | 想看"外部怎么评"时(可选) |

**目录**:
```
docs/
├── README.md                 # 索引
├── HANDOFF.md                # 本文件
├── DESIGN.md                 # 需求 + 边界
├── ARCHITECTURE.md           # 16 关卡 + Channel Adapter
├── TECH.md                   # 锁定的库
├── IMPLEMENTATION.md         # 8 步路线图 + 决策日志
├── BACKLOG.md                # 7 个候选功能
├── HACKING-wsl.md            # 5 个 WSL 环境坑
├── HACKING-llm.md            # LLM 兼容层差异
├── HANDOFF.md                # 本文件
├── REVIEW-glm-5.1.md         # 外部评审 #1
├── REVIEW-deepseek-v4-pro.md # 外部评审 #2
└── spikes/
    ├── 001-wsl-tauri-window.md
    └── 002-reqwest-anthropic-sse.md
```

---

## 4. MVP 步骤 1 是什么 + 起点 + 验收

### 4.1 目标(来自 [IMPLEMENTATION §2.1](./IMPLEMENTATION.md#21-步骤-1--骨架与-llm-直连-mvp))

**跑通"Tauri app + 跟 LLM 说一句话 + 流式显示"**。能聊天的最小 app,不做工具调用,不做 session 持久化。

### 4.2 实施内容

1. **搬 spike 项目到正经位置**
   - 源:`~/tauri-spike/spike-app/`
   - 目标:`/usr/local/code/github/everlasting/app/`
   - 不是 copy,是建新项目然后**选择性搬**:
     - ✅ 搬:`package.json` / `vite.config` / `tauri.conf.json` / Cargo.toml 依赖
     - ✅ 搬:`src-tauri/` 整个骨架(icons / build.rs / capabilities 模板)
     - ❌ 不搬:spike 改的 App.vue 中文测试 demo(重写)
     - ❌ 不搬:spike 的 node_modules / target/(重建)

2. **前端栈升级**([TECH §1](./TECH.md#1-决策vue-3-全家桶替代-react))
   - Vue 3 + Vite + **Pinia**(状态管理) + **reka-ui**(组件库)
   - 用 `pnpm create vite@latest` 创 Vue 模板,再加 `pinia` / `reka-ui` / `@tauri-apps/api` / `vue-router`(可选,步骤 1 可不上)

3. **Rust 端 LLM 客户端**(参照 sse-spike 验证过的模式)
   - 位置:`src-tauri/src/llm/`
   - 4 个模式切分(参考 HACKING-llm.md checklist):
     - `client.rs`(reqwest HTTP)
     - `sse.rs`(SSE 解析,事件顺序记录)
     - `error.rs`(错误归一化,4 类 + 网络)
     - `types.rs`(request/response 数据结构)
   - BASE_URL / model / key 全部从 env 读
   - 实施 checklist 11 项见 [HACKING-llm.md §"LLM 客户端实施 checklist"](./HACKING-llm.md#llm-客户端实施-checklist给步骤-1-2-写-rust-客户端时)

4. **Tauri IPC 桥**
   - `invoke("chat", { message })` 前端调用
   - Rust 端 spawn task 跑 stream,emit `chat-chunk` 事件到前端
   - 前端 `listen("chat-chunk", ...)` 接收,append 到消息列表

5. **最小 chat UI**
   - 一个输入框 + 一个发送按钮
   - 消息列表(用户右 / 助手左)
   - 流式 append,不要等完整响应

### 4.3 起点材料(本 session 留的)

| 资源 | 路径 | 用途 |
|------|------|------|
| spike Tauri 项目 | `~/tauri-spike/spike-app/` | 搬骨架的源 |
| sse-spike Rust 代码 | `~/sse-spike/src/main.rs` | LLM 客户端实现的参考 |
| sse-spike 二进制 | `~/sse-spike/target/release/sse-spike` | 快速验证 LLM API 还通(可改 env 重跑) |
| spike-001 文档 | `docs/spikes/001-wsl-tauri-window.md` | 已知坑 + 通过标准 |
| spike-002 文档 | `docs/spikes/002-reqwest-anthropic-sse.md` | SSE 4 模式 + GLM 差异 |
| HACKING-wsl | `docs/HACKING-wsl.md` | 5 个 WSL 坑 + 一次性脚本 |
| HACKING-llm | `docs/HACKING-llm.md` | GLM 差异 + 实施 checklist |

### 4.4 验收标准(本步骤完成判定)

- [x] `cd /usr/local/code/github/everlasting/app && pnpm tauri dev` 启动 < 30 秒
- [x] 窗口在 Windows 桌面正常显示(同 spike-001)
- [x] 中文输入 + 中文响应,中英文字号 baseline 对齐(同 spike-001)
- [x] 输入"你好" → 流式看到响应(token by token 出现)
- [x] 故意输错 API key → 友好错误提示(不是 panic,不是 500 页)
- [x] 5 次连续提问不崩 / 不卡
- [x] 至少 1 次热重载改 chat UI 不崩
- [x] WebView 进程在 WSL 内(同 spike-001)

### 4.5 本步骤不碰(留到后续步骤)

- ⏭ 工具调用(read_file / write_file / shell)—— 留到步骤 2
- ⏭ session 持久化(SQLite)—— 留到步骤 3a
- ⏭ 多项目 / 多 session 切换—— 留到步骤 3b
- ⏭ git worktree / 自动 commit—— 留到步骤 4
- ⏭ 权限系统 / xterm.js—— 留到步骤 5
- ⏭ MCP / 多 Provider—— 留到步骤 6
- ⏭ rig-core 迁移—— 留到步骤 3b

### 4.6 完成后

✅ 步骤 1 已完成(2026-06-04)。走 [IMPLEMENTATION §2.2 步骤 2 Tool Calling](./IMPLEMENTATION.md#22-步骤-2--tool-calling-mvp)。

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

## 6. 关键决策摘要(8 条)

1. **WSL 优先** — Tauri 跑在 WSL 内,WSLg 显示到 Windows 桌面,**无 wslapi 调用**
2. **自研 agent core** — 不用 SDK 包装(学习价值 + 控制粒度)
3. **每个 session 一个 git worktree** — `~/.local/share/everlasting/worktrees/<project_hash>/<session_id>`
4. **Agent Daemon 化**(v1 之后) — 拆出独立进程
5. **MCP 只外暴露,内部通信不绕** — agent 调自己的工具直接调 Rust 函数
6. **SQLite 是唯一存储** — sqlx + SQLite,FTS5
7. **前端栈 Vue 3 + Vite + Pinia + reka-ui**(本 session 才定的)
8. **方案 C:VPS 自托管 daemon(v2)** — 前期只留接口

完整决策日志:[IMPLEMENTATION §4](./IMPLEMENTATION.md#4-决策日志)。

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
- **最近 commit hash**:`1bcc9e8` (session 4 改动未 commit)
- **当前日期**:2026-06-04

---

> 本文档随项目演进更新。任何重大架构变更后,先改这里,再改具体文档。
