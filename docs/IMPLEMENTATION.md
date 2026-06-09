# IMPLEMENTATION — 实现讲解

> Everlasting 的"按什么顺序做、决策记录、下一步是什么"。包括实施路线图、关键决策、待办、决策日志。
> 需求见 [DESIGN.md](./DESIGN.md),架构见 [ARCHITECTURE.md](./ARCHITECTURE.md),技术选型见 [TECH.md](./TECH.md),候选功能见 [BACKLOG.md](./BACKLOG.md)。

---

## 1. 决策:自己写 agent core,不用 SDK 包装

**背景**:Anthropic 2025-2026 年出了官方 Agent SDK(`claude-agent-sdk-python` / `-typescript`),用 `query()` 直接拿结构化消息流。OpenAI Codex CLI 是 Rust 写的(Apache 2.0)但没官方 SDK。

**为什么不用**:
1. **学习目标要求自研** — 用了 SDK 只学到"怎么调 SDK",学不到 harness 核心
2. **控制粒度** — SDK 帮你做了"消息流 → tool 调用 → 回填"的循环,你想插自定义逻辑(权限、审计、统计)就被抽象挡住了
3. **解耦厂商** — 一旦 SDK 协议变化,业务逻辑全挂

**什么时候用 SDK 合适**:赶时间、要快速出活、不在乎学习价值。本项目两个都不符合。

**自研的边界**:
- ✅ 自己写:Agent Loop、消息管理、tool 注册、流式解析、权限检查
- ✅ 自己写:Tauri IPC 事件协议、session 持久化、worktree 管理
- ❌ 不自己写:LLM HTTP 协议(用 rig)、SSE 解析(用 rig)、MCP 协议(用 rmcp)
- ❌ 不自己写:GUI 框架(Tauri 已有)、Diff 算法(用前端库)

---

## 2. 实施路线图

7 个步骤,每步完成才能进下一步。**不写时间承诺**,只写目标、可交付物、关键产出。

> **路线图变更记录**(2026-06-04):原 8 步合并为 7 步——删除原步骤 5(WSL 验证,spike-001 已通过),拆原步骤 3 为 3a/3b。

### 2.1 步骤 1 — 骨架与 LLM 直连 [MVP] ✅ 已完成(2026-06-04)

**目标**:跑通"Tauri app + 跟 LLM 说一句话 + 流式显示"

- 搭 Tauri 2 + Vue 3 + Vite + Pinia + reka-ui 项目(栈细节见 [TECH §1](./TECH.md#1-决策vue-3-全家桶替代-react))
- Rust 端 LLM 客户端:`reqwest` + `futures-util` + `serde_json`,**手写 SSE 解析**(用 `futures-util::StreamExt`,**不**用 `eventsource-stream`——spike-002 验证手写解析器够用,见 [HACKING-llm.md](./HACKING-llm.md) 和 [spike-002 §"代码关键改动"](./spikes/002-reqwest-anthropic-sse.md#实际执行2026-06-04))
- 4 个文件分模块(client / sse / error / types),实施 11 项 checklist 见 [HACKING-llm.md §"LLM 客户端实施 checklist"](./HACKING-llm.md#llm-客户端实施-checklist给步骤-1-2-写-rust-客户端时)
- 前端:简单 chat UI(输入框 + 消息列表)
- Tauri event 把 SSE chunk 推到前端(`emit("chat-event", ...)`)
- **可交付物**:能聊天的最小 app

**前置硬依赖**:
- ✅ spike-001(WSL + Tauri 窗口 + 中文/Emoji)已于 2026-06-04 通过,见 [spikes/001](./spikes/001-wsl-tauri-window.md)
- ✅ spike-002(reqwest + SSE + 错误分类)已于 2026-06-04 通过,见 [spikes/002](./spikes/002-reqwest-anthropic-sse.md)
- 起点:搬 `~/tauri-spike/spike-app/` 到 `/usr/local/code/github/everlasting/app/`,扩成正式骨架。详细起点 + 验收标准见 [HANDOFF §4](./HANDOFF.md#4-mvp-步骤-1-是什么--起点--验收)

**撞过的环境坑**:见 [HACKING-wsl.md](./HACKING-wsl.md)(linuxbrew pkg-config / pnpm 代理 / Rust 1.83 / cargo cache 锁 / WSLg CJK 字体)。**新机器或怀疑环境有问题时,先读 HACKING-wsl**。

### 2.2 步骤 2 — Tool Calling [MVP] ✅ 已完成(2026-06-04)

**目标**:agent 能读写文件、跑 shell

- 定义 3 个 tool:`read_file` / `write_file` / `shell`
- 解析 `tool_use` 块,执行,构造 `tool_result` 回填
- agent loop 实现:消息 → LLM → tool_use? → 执行 → tool_result → LLM → ... → 完成
- 前端显示 tool 调用过程(简化版)
- **可交付物**:能帮我改代码的 agent

### 2.3 步骤 3a — SQLite + Session 持久化 [MVP] ✅ 已完成(2026-06-05)

**目标**:消息存 DB,重启能恢复,session 切换看历史

- 引入 SQLite (`sqlx`),存 session / message
- session 列表 + session 切换(单项目,无左侧项目栏)
- 消息从 SQLite 加载,不再全内存
- LLM 客户端不动(继续用 reqwest + 手写 SSE)
- **可交付物**:关掉 app 再打开,历史消息还在

### 2.4 步骤 3b — 多项目 + UI 三栏 + Rig 迁移 [MVP] ⚠️ 3b-1 ✅ 已完成(2026-06-05/06),3b-2 ⛔ 废弃(2026-06-09)

**目标**:引入 Project 概念,三栏 UI,切 rig-core

**拆解**:
- **3b-1 ✅ 已完成** — 项目基础结构 + 顶部 Tabs UI(后端 PR1 + 前端 PR2 + post-fixes squash + docs follow-up)。`projects` 表 + `project_uuid`,`ToolContext.cwd` 默认 `~/`,`pick_project_dir` 走 Tauri native dialog,agent 工具不越出 project root。
- **3b-2 ⛔ 废弃 (2026-06-09 决策)** — rig-core 0.38.1 弃用;完整三栏 UI 暂搁置(功能上不阻塞主线,后续若有需求再做)。多 Provider 抽象已通过自研 `Provider` trait 在 06-08 独立任务中完成,详见 §2.7 步骤 6a。

**PR1(后端,3b-1 之一,2026-06-05 落地) ✅**:
- `db.rs` migration + `projects` 表 + Auto-default `__default__` 兜底 + `PRAGMA foreign_keys = ON`
- `projects/` 新模块(types / store / detector / boundary)
- `tools/` 全部改造 — `ToolContext` 注入 + `shell` 加 `working_directory` 校验 + `read_file`/`write_file` 相对/绝对路径过 boundary
- `lib.rs` `chat` 命令构造 `ToolContext`,turn 结束一次性写 `sessions.current_cwd`
- 4 个现有 commands 改造(`create_session` / `list_sessions` / `load_session` / `delete_session`) + 7 个新 commands(`list_projects` / `create_project` / 等)
- `ARCHITECTURE §3` worktree 路径术语 `project_hash` → `project_uuid`
- 设计稿:`docs/_archive/2026-06-3b-1/PROPOSAL-project-binding-and-top-tabs.md` + spec `.trellis/spec/backend/project-cwd-boundary.md`

**PR2(前端,3b-1 之二,commit `93a0753`,2026-06-05 落地) ✅**:
- `stores/projects.ts` 新增
- `stores/chat.ts` 改造
- `ChatWindow.vue` Tab 栏 + 空状态 + "最近隐藏项目"列表
- `pick_project_dir` + 手动输入 fallback
- 端到端测试

**Post-fixes(commit `18354a0` squash,2026-06-06 落地) ✅**:
- 3 个 hotfix 修 PR1/PR2 wire format 偏差(Tauri 2 IPC arg camelCase + `Option<T>` null 行为 + tool_result 块只能出现在 user role)
- FU-1 cwd `~/` 数据通路准备(backend `get_home_dir` + frontend `simplifyPath` 工具)
- 完整 follow-up 列表 6 条沉淀到 `docs/_archive/2026-06-3b-1/FOLLOW-UP.md`(commit `7e888c9`)

**可交付物**:能管多个项目、多个对话,agent 工具调用不越出 project root,为步骤 4 准备好 `<project_uuid>` 字段 ✅

### 2.5 步骤 4 — Git 集成 [MVP] ✅ 已完成(2026-06-07/08,auto-commit 延后)

**目标**:session 隔离 + 自动 commit

- `git2-rs` 集成
- session 创建时建 worktree(见 [ARCHITECTURE.md §3 worktree 决策](./ARCHITECTURE.md#3-决策每个-session-一个-git-worktree))
- session 结束或定时自动 commit
- 前端 diff 视图(用 `diff` (jsdiff) + 自渲染,见 `app/src/components/chat/DiffView.vue`)
- **可交付物**:每个 session 是独立分支,能看 diff

**落地状态**:
- ✅ worktree 解耦(opt-in attach / detach / delete + 三态状态机 + LLM 透明度 + 安全网,2026-06-08)
- ⏸ auto commit 仍 OOS(2026-06-09 决策延后,后续 v1 阶段再评估)
- ✅ `git-diff-contract.md` 沉淀(Workdir-vs-branch-tip FileDiff source of truth)

### 2.6 步骤 5 — 嵌入式终端 + 权限系统 [v1] 🔽 降为可选(v1 之后)

**目标**:能看 agent 在跑啥,能控制 agent 能干啥

- `xterm.js` + `portable-pty` 跑 shell
- agent 跑的 `shell` tool 输出实时到 xterm
- 权限系统雏形:每个 tool 可以 ask / allow / deny
- **可交付物**:能看见、能拦住 agent

**状态 (2026-06-09 决策)**:v1 阶段工具集已通过 `shell` tool + 30K 落盘部分覆盖"看 agent 在跑啥"的需求,嵌入式 xterm 完整 UI 降为可选(留作 v1 之后);权限系统基础架构在 `provider` 抽象里预留 hook,完整 per-tool/ per-session/ per-project 权限留 v1。

### 2.7 步骤 6 — MCP 暴露 + 多 Provider [v1]

**目标**:你的工具 Claude Code 也能用;切模型无痛

#### 2.7.1 步骤 6a — 多 Provider [v1] ✅ 已完成(2026-06-08/09,4 PR + 1 follow-up)

- `rmcp` 起一个 stdio MCP server(待 v1.5 实施,当前未开始)
- 加 OpenAI provider 切换
- 加 Ollama provider 切换(纯本地,省钱,可选)
- **可交付物**:模型随便切 ✅

**实际落地**(2026-06-08/09 独立任务 `06-08-multi-model-llm-provider-planning`):
- **PR1** — data layer:3 表 `providers` / `models` / `app_config` + 8 CRUD + 10 IPC + seed(commit `f9c5648`)
- **PR2** — Anthropic adapter:`Provider` trait + `AnthropicProvider` impl + catalog dispatch(commit `0a787ef`)
- **PR3** — OpenAI adapter + 跨协议:`OpenAIProvider` + `provider::wire` WireMessage 中间层 + `strip_unsupported` 静默降级
- **PR4 follow-up** — `fix(llm): OpenAI adapter endpoint() double-prefixes /v1/` (commit `96e1f98`)
- 完整设计:`.trellis/tasks/archive/2026-06/06-08-multi-model-llm-provider-planning/prd.md`
- 契约沉淀:`.trellis/spec/backend/llm-contract.md` "Scenario: Multi-Provider Abstraction (PR1)" section

#### 2.7.2 步骤 6b — MCP 暴露 [v1] ⏸ 未开始

- `rmcp` 0.16.0 起一个 stdio MCP server
- 验证:Claude Desktop 能调用你的 read_file / shell / edit_file 等
- **可交付物**:工具集对外开放;模型随便切(已完成)
- **状态**:6a 已完成(多 Provider),6b MCP 未开始。

### 2.8 步骤 7 — 打磨与文档 [跨阶段]

- Token 用量统计
- 错误处理完善
- README 写完
- 录个 demo 视频
- **可交付物**:能给别人看的最小可用版本

> ⚠️ **Agent Daemon 化的占位**:16 关卡(见 [ARCHITECTURE §2](./ARCHITECTURE.md#2-harness-设计从用户输入到文件变更的-16-道关卡))中有 ⑮ Channel 输出(daemon → client)这一关,需要 agent core 拆出独立进程才能落地。**触发条件**:
> - **若** BACKLOG 远期（v3+）段飞书 channel 决定实施 → 在步骤 5 之后插入"步骤 5.5 — Agent Daemon 化",再做步骤 6
> - **若** 飞书不做 → 推迟到 v2 之后,daemon 化不阻塞当前 7 步
> - **判断窗口**:在步骤 5 完成后、步骤 6 开始前问自己"长跑任务被打断是不是真痛?",痛就拆,不痛就跳
> - 详见 [ARCHITECTURE.md §4 决策:Agent Daemon 化](./ARCHITECTURE.md#4-决策agent-daemon-化为多-channel-接入铺路)

### 2.9 步骤 8 — 代码重构与文档清理 [跨阶段] 🔄 当前进行中

**目标**:对积累下来的大型文件做物理拆分,让后续维护成本下降;同步清理过时的文档/spec

**拆分动机**(详见 [`docs/_reviews/REVIEW-claude-opus-2026-06-09.md` §2](./_reviews/REVIEW-claude-opus-2026-06-09.md)):
- `app/src-tauri/src/lib.rs` 3195L → 单文件过大,改一个命令要 scroll 几百行
- `app/src-tauri/src/db.rs` 2862L → SQLite CRUD 全部塞一个文件,8 个函数相互纠缠
- `app/src/components/ChatWindow.vue` 单文件 + `app/src/stores/chat.ts` 700+ L 流式逻辑混杂
- 文档/HANDOFF 多处滞后,需要统一校准到 2026-06-09 git log 真实状态

**5 PR 序列**:
- **8-PR1 ✅ 已落地 (commit `5171ecf`)** — lib.rs 拆分为 `state/` + `commands/` + `agent/` 子目录
- **8-PR2 ✅ 已落地 (commit `c151c77`)** — db.rs 拆分为 `db/` 子模块(mod/migrations/types/models/config/providers/projects/sessions/tests)
- **8-PR3 ✅ 已落地 (commit `2f8a677`)** — ChatWindow / ModelsTab 拆为 `chat/` `settings/` `layout/` sub-components;`chat.ts` 拆出 `streamController.ts` 独立 Pinia store
- **8-PR4 🔄 本 commit** — 文档统一更新到 2026-06-09 真实状态 + 9 个空 spec 文件清理(directory-structure / quality-guidelines / logging-guidelines / component-guidelines / hook-guidelines / type-safety)
- **8-PR5 ⏳ 待办** — 创建 `STRUCTURE.md` 详细记录最终目录结构 + 模块职责

**完成定义**:
- ✅ `pnpm tauri build` 干净通过
- ✅ `cargo test --lib` 全部 pass
- ✅ 7 个文档(CLAUDE.md / README.md / TECH.md / DESIGN.md / HANDOFF.md / IMPLEMENTATION.md / BACKLOG.md)同步校准到 2026-06-09 状态
- ⏳ `STRUCTURE.md` 创建(8-PR5)

---

## 3. 待办与下一步

**最后更新**:2026-06-10

### 3.1 已收尾(milestone 表,权威以 `git log --oneline -20` 为准)

| 步骤 | 内容 | 阶段 | 状态 |
|------|------|------|------|
| 1 | 骨架 + LLM 直连 | MVP | ✅ 已完成(2026-06-04) |
| 2 | Tool Calling(agent loop + 3 个 tool) | MVP | ✅ 已完成(2026-06-04) |
| 3a | SQLite + Session 持久化 | MVP | ✅ 已完成(2026-06-05) |
| 3b-1 | 项目基础结构 + 顶部 Tabs UI | MVP | ✅ 已完成(2026-06-05/06) |
| 4 | Git 集成(worktree + opt-in attach / detach / delete) | MVP | ✅ 已完成(2026-06-07/08);auto commit 延后 |
| 5 | WSL 体验 | MVP | ✅ spike-001 通过(2026-06-04) |
| 6a | 多 Provider(Anthropic / OpenAI) | v1 | ✅ 已完成(2026-06-08/09) |
| 8-PR1/2/3 | 代码重构(8-PR1 lib.rs / 8-PR2 db.rs / 8-PR3 前端) | 跨 | ✅ 已完成(2026-06-09) |
| — | 路线图外:extended thinking / spike-005 follow-up 7 PR / 字体栈 / 6 UI bug 修复 / 工具集扩展 / step 4 follow-up / 多 Provider 4 PR | 额外 | ✅ 已完成 |
| 3b-2 | ~~完整三栏 UI + rig-core 迁移~~ | — | ⛔ 废弃 (2026-06-09) |
| 6b | MCP 暴露 | v1 | ⏸ 未开始 |
| 5(原) | 嵌入式终端 + 权限系统 | v1 | 🔽 降为可选 |

> ⚠️ **编号语义注意**:commit `05671f5` 标题写"步骤 6 — Anthropic extended thinking",跟路线图 §2.7 "步骤 6 = MCP + 多 Provider" 不一致。extended thinking 实际是路线图外的额外功能(在表里单列"—"),并非提前实现 MCP 步骤 6。详见 [§4 决策日志 2026-06-05 条](#2026-06-05--路线图状态校对步骤-3a-完成步骤-3b-暂缓extended-thinking-路线图外完成)。

### 3.2 当前进行

- 🔄 **Step 8 (代码重构与文档清理)** — 8-PR4 本 commit(文档更新 + 9 个空 spec 文件清理);接下来 8-PR5 STRUCTURE.md

### 3.3 下一步候选(三选一,详见 [HANDOFF §2 当前状态](./HANDOFF.md#2-当前进度))

- 完成 Step 8 → **8-PR5 STRUCTURE.md** (收尾,优先级最高)
- 主线推进 → **步骤 6b (MCP 暴露)** 或 **步骤 5 (嵌入式终端 + 权限系统)**
- 收尾已知 issue → **bug 1+2 position 修复**(RDP 双显示器场景,候选 `setFullscreen(true)` 兜底 — 详见 `.trellis/tasks/archive/2026-06/06-07-6-ui-bug-markdown-sse/prd.md`)

**已沉淀(spike 期间完成的)—— 不必再做,出问题查这里**:
- ✅ Tauri 在 WSL 跑得通 + 中文对齐 → [spikes/001](./spikes/001-wsl-tauri-window.md)
- ✅ Rust 端 LLM 客户端手写 reqwest + SSE 可走(GLM 3 处差异已知)→ [spikes/002](./spikes/002-reqwest-anthropic-sse.md)
- ✅ WSL 环境坑 → [HACKING-wsl.md](./HACKING-wsl.md)
- ✅ LLM 客户端实施 checklist + 切真 Claude 重测清单 → [HACKING-llm.md](./HACKING-llm.md)

**已决定(不再讨论)**:
- [x] 前端框架:**Vue 3 + Vite + Pinia**(见 [TECH §1.1](./TECH.md#11-锁定项经过调研验证))
- [x] 前端 UI 库:**reka-ui** / shadcn-vue primitives(见 [TECH §1.4](./TECH.md#14-扩展功能新增依赖随候选功能引入))
- [x] 包管理器:**pnpm**
- [x] LLM 客户端:**手写 reqwest + SSE + 自研 Provider trait**(步骤 2 继续手写,2026-06-09 决策放弃 rig-core 改自研 `Provider` trait,见 [TECH §2](./TECH.md#2-决策rig-core-弃用2026-06-09改自研-provider-trait) + [spike-002 §"结论"](./spikes/002-reqwest-anthropic-sse.md#结论))
- [x] LLM BASE_URL / model / key:**全部从 env 读**(便于切 wukaijin / 真 Claude / 其他)
- [x] 工作目录:**WSL 内部**(`~/...` 或 `/usr/local/code/...`),不走 `/mnt/c`
- [x] Agent Daemon 化:**v1 之后再说**,本项目 7 步不阻塞(见 §2.8 占位)

**等做完步骤 1-2 再决定**:
- [ ] SQLite schema 最终长什么样
- [ ] Tool 注册的最佳实践
- [ ] Frontend 状态管理边界(Pinia store 怎么分)

**候选功能(来自 [BACKLOG.md](./BACKLOG.md))的待评估**:
- [ ] 是否要做 Skill / Memory / Role(技术选型已就绪,详见 BACKLOG §2 + 远期（v3+）段)
- [ ] 是否做生成式 UI(详见 BACKLOG 远期（v3+）段)
- [ ] 是否做飞书 channel(会触发架构变更,详见 BACKLOG 远期（v3+）段)

---

## 4. 决策日志

> 按时间倒序记录。每次重大决策都加一条,包含"为什么"。

### 2026-06-07 — 工具集扩展批次(edit_file / grep / glob / list_dir + ReadGuard + Bash 落盘 + cat -n)

- **决策**:`edit_file` 用 claude-code 风格 str_replace_editor + 3 道强制 check(read-before-edit / on-disk freshness / match + uniqueness),失败文案是 plain English(LLM 能自纠)
  - **原因**:`write_file` 整文件覆盖 token 浪费大 + 改错位置不报;claude-code Edit 是 token 经济 + 防护成熟的方案
  - **关键设计**:`ReadGuard` Tauri State,`Mutex<HashMap<SessionId, HashMap<PathBuf, Fingerprint>>>`,session 隔离(切回不重读),edit 写成功后自动 invalidate(逼 LLM 重读)
  - **0 匹配处理**:claude-code 风格直接报错 + 0-3 个最相似行 hint(Jaccard 相似度排序)——**不**自动 strip 空白重试(OpenHands 风格)
- **决策**:`grep` / `glob` / `list_dir` 三个浏览工具跟 edit_file 一起合
  - **grep**:`tokio::process::Command::new("rg")` spawn,3 种 output_mode(files_with_matches / content / count),line cap 500 字符(抄 pi_agent_rust),默认遵守 .gitignore
  - **glob**:`globset` crate,cap 100,按 mtime 倒序,**不**强制 .gitignore(跟 claude-code 一致)
  - **list_dir**:`tokio::fs::read_dir` 字母排序 + 目录加 `/` 后缀,hidden 默认 false(避免 `.git/` 灌爆),非递归(递归归 glob)
- **决策**:`offset/limit` 包含 `old_string` 出现位置就算 read 过(不要求覆盖全文)
  - **原因**:LLM 智能只读相关区段是合法操作,不必要求 LLM 重调 read_file 读全文浪费 token
- **决策**:顺手 2 件在同批次合(read_file 加 `cat -n` 行号 prefix + shell 30K 落盘)
  - **cat -n**:`read_file` 返回每行加 `\t<line_num>\t` 前缀(1-based),截断保留行号;跟 edit_file 报错带行号协同,LLM 拿到内容就能定位"第 42 行"
  - **Bash 落盘**:> 30K 字符写到 `<session_cwd>/.everlasting/outputs/<uuid>.txt`,tool_result 返回 path + 1KB head+tail preview;`delete_session` 调 `cleanup_outputs_dir` best-effort 清理(失败不 cascade)
- **决策**:1 个 `feat(tools):` commit 一次性合(用户拍板)
  - **原因**:4 tool + ReadGuard + Bash 落盘 + cat -n 互相依赖(ReadGuard 跨 edit_file/read_file),分开 commit 反而中间状态编译过不了
- **测试**:77 新 tool test + 3 cleanup_outputs_dir test = 80 新;cargo test 163→166 全过;pnpm build 干净
- **沉淀**:`.trellis/spec/backend/llm-contract.md` 新增 §"Scenario: Tool Set Extension" 段(7 sections code-spec depth,含错误矩阵 + Good/Base/Bad + 24 个必测项 + Wrong/Correct 对照)
- **Out of Scope 守住** (13 条):`hashline_edit` / `MultiEdit` / `LSP` / `WebFetch` / `WebSearch` / damage-control 路径规则 / Bash `cat|head|sed` 等价 read / `replace_all` preview / 前端 tool card 改造 / `read_file` PDF / binary 检测 / `read_many_files` / grep `output_mode=json` —— 全部 0 命中

### 2026-06-07 — 6 UI/状态 bug 修复 + streamController 状态架构重构

- **决策**:抽 `useStreamControllerStore()` 独立 Pinia store 作为 in-flight SSE 流的**单一来源**,`useChatStore()` 改 thin facade
  - **原因**:旧设计把 messages / `streamingSessionId` / `currentRequestId` / SSE listener 全放 `useChatStore()`,session 切换时会丢 streaming message + 漏 `done` event 处理(red dot + stop button + `sending` 卡死)
  - **新边界**:`streamController` 拥有 per-session message buffer (LRU 20) + activeRequests + 单全局 SSE listener(按 `request_id` 路由,不再按 `currentSessionId` 过滤);`chatStore` 拥有 sessions 列表 + currentSessionId + currentCwd + session CRUD 委托
  - **流指示器分层**:`streamingProjectIds` → AppHeader 红点;`streamingSessionIds` → SessionList 蓝点 1.5s pulse
  - **沉淀**:`.trellis/spec/frontend/state-management.md` 新增 §"Stream Controller Pattern"
  - **测试**:12 个 LRU 单测 + 36 vitest + 103 cargo 全过
  - **commit**:`abde429` + spec `bf9b35b`
- **决策**:顶栏窗口控制 bug 1+2(尺寸 + 位置)的 size 部分通过 Tauri 2 capabilities 补全权限修好
  - **原因**:`setSize` 之前静默失败是 Tauri 2 默认 deny(没在 `capabilities/default.json` 声明);补 `set-size` / `set-position` / `outer-size` / `outer-position` / `current-monitor` 等 11 个权限
  - **已知 issue**:position 部分在 RDP 双显示器场景下未完全修好(窗口 grow rightward 而非贴 host 主屏左上角),TODO 跟踪,候选 `setFullscreen(true)` 兜底
  - **commit**:`bd5ea7b`
- **决策**:Markdown 表格 td/th border 改用 `--color-bg-border-strong: #3B475A`
  - **原因**:dark mode 下原 `--color-bg-border: #1E2530` 跟气泡底色 `#1A2030` 只差 4 亮度单位,看不清
  - **commit**:`bd5ea7b`
- **决策**:顶栏 minimize 按钮改用 `MinusIcon`(替换原 ✕ 图标)
  - **原因**:icon 跟功能不对应(bug 3);补 `Icon.vue` heroicons 注册
  - **commit**:`bd5ea7b`
- **决策**:顶栏 logo 加 `padding-right: 12px`,跟 tab 区拉开间距(bug 4)
  - **commit**:`bd5ea7b`

### 2026-06-06 — 字体栈调整 + spike-005 follow-up 7 PR 合并

- **决策**:Dark theme 下中文字体栈首位改 HarmonyOS Sans SC,子集打包嵌入(3500 常用字 + ASCII + 标点,woff2 + brotli → 472 KB)
  - **原因**:Noto Sans CJK SC 在 dark theme 下笔画粗细不均,影响阅读
  - **沉淀**:`.trellis/spec/frontend/cjk-fonts.md`(系统字体兜底局限、3500 字覆盖率、Vite+Tauri 资源链路、license 合规三处声明 pattern)
  - **commit**:`aabb9fa` + docs follow-up `d1d51cf` / `adf4ed6`
- **决策**:spike-005 后续 7 PR 合并为单个 commit,代表"MVP 基础体验可上桌"的状态点
  - **覆盖范围**:UI 紧凑 header (`801fb8a`) + git_branch 显示 + 启动 batch backfill(`7ce3209` 推翻 PR2 懒探测决策) + pwd `~/` 简化数据通路 (`ef7cea8`) + write_file tracing + LLM cancel 机制 + markdown 渲染 (marked v18 + DOMPurify + vitest 基础架构) + 首行空白修复
  - **commit**:`401396b`
- **决策**:`projects.git_branch` 用启动时 batch backfill,不再用 PR2 的"打开 project tab 时懒探测"
  - **原因**:老项目(无 git_branch 字段)开了 tab 才能看到分支,首屏体验差;启动 batch 一次扫所有项目,DB 落库
  - **commit**:`7ce3209`

### 2026-06-05 — 路线图状态校对(步骤 3a 完成、步骤 3b 暂缓、extended thinking 路线图外完成)

- **决策**:步骤 3b(多项目 + UI 三栏 + Rig 迁移)**暂缓**,优先做 Anthropic extended thinking 支持
  - **原因**:thinking 功能跟正在落地的 LLM 工作流强相关,延后做返工成本高;3b 是 UI / 多项目重构,可以后做不影响其他步骤
- **决策**:extended thinking 单列为"路线图外完成",不挤占现有步骤编号
  - **原因**:保护 7 步路线图原结构,避免重新编号引发的级联引用更新
- **已知 issue(不修)**:commit `05671f5` 标题误用"步骤 6 — ..."字样,实际不对应 §2.7 步骤 6(MCP + 多 Provider)。**不改 commit message**(git 历史不动);路线图 §3 表格、ARCHITECTURE §2.4 实施映射表里都加注释说明语义偏差
- **决策**:HANDOFF / CLAUDE.md / IMPLEMENTATION / DESIGN / ARCHITECTURE / README 所有"当前进度"段落统一校准到本日 git log 真实状态
  - **原因**:HANDOFF 停留在 2026-06-04 步骤 3a 前夕,跟实际偏差过大,新 session 读完会做错假设

### 2026-06-04 — 路线图重构(步骤 1 完成后审视)

- **决策**:删除原步骤 5(WSL 验证),8 步合并为 7 步
  - **原因**:spike-001 已通过,步骤 1 也在 WSL 内完成,原步骤 5 是空壳
- **决策**:拆原步骤 3 为 3a(SQLite + session 持久化)和 3b(多项目 + UI + rig-core)
  - **原因**:原步骤 3 包含 4 件独立大事,任何一件卡住整个步骤都交付不了
- **决策**:事件协议用混合模式(高频 payload 判别 + 低频独立事件名)
  - **原因**:兼顾流式 token 性能和低频事件可 filter 性
- **决策**:SQLite 不提前到步骤 2,保持步骤 3a
  - **原因**:步骤 2 专注 agent loop 核心学习,加 SQLite 会膨胀范围
- **决策**:步骤 2 继续手写 reqwest,步骤 3b 才切 rig-core
  - **原因**:手写 agent loop 是核心学习价值,"先学再依赖"

### 2026-06-04 — 项目正式启动

- **决策**:自己写 agent core,不用 Anthropic Agent SDK 包装
  - **原因**:学习目标 + 控制粒度 + 厂商解耦
- **决策**:WSL 优先,Windows 不主动适配
  - **原因**:个人使用场景就是 WSL;Tauri 在 WSL 跑得通
- **决策**:每个 session 一个 git worktree
  - **原因**:多 session 并行 / 互不污染 / 切换瞬时
- **决策**:agent core 用 rig-core(后期切),前期手写
  - **原因**:先学再依赖,避免黑盒
- **决策**:MCP 只外暴露,内部通信不绕
  - **原因**:内部直接调 Rust 函数最快;外部用 MCP 生态杠杆
- **决策**:SQLite 是唯一存储
  - **原因**:单文件、零运维、FTS5 搜索、sqlx 异步
- **决策**:不做 workflow 编排(到 v2 再说)
  - **原因**:单 agent 体验没做透之前,workflow 是空中楼阁
- **决策**:不做团队协作、不做云端部署、不做移动端
  - **原因**:个人工具,这些是另一个产品的事
- **决策**:Skill / Memory / Role 共用 frontmatter loader
  - **原因**:三者载体都是 Markdown + YAML,加载时机不同;统一 loader 省重复代码
- **决策**:Agent Daemon 化(agent core 从 Tauri 进程拆出为独立 daemon)
  - **原因**:为多 channel 接入铺路(飞书不能依赖 GUI);GUI 重启不打断长跑任务;长跑任务稳定性。详见 [ARCHITECTURE.md §4](./ARCHITECTURE.md#4-决策agent-daemon-化为多-channel-接入铺路)
- **决策**:生成式 UI 走约束式(LLM 输出 JSON,前端按 type 渲染),默认关闭
  - **原因**:自由式(LLM 生成 HTML)沙箱难做、调试难;约束式够覆盖 80% 用例。详见 [BACKLOG.md 远期（v3+）段](./BACKLOG.md#远期v3暂不评估)
- **决策**:Channel Adapter 抽象(`Channel` trait,3 个实现:TauriGui / Feishu / Cli)
  - **原因**:多入口统一接口;新增 channel 不用改 agent core;测试友好。详见 [ARCHITECTURE.md §5](./ARCHITECTURE.md#5-决策channel-adapter-抽象为多入口铺路)
- **决策**:候选功能方向锁定(7 个),但暂不排优先级
  - **原因**:先把方向沉淀下来,实施前再按"价值/成本/依赖"评估。详见 [BACKLOG.md](./BACKLOG.md) 全章
  - 方向:输入层(图/@ /command) / Skill / 多层 Memory / 多角色+多模式+编排 / 生成式 UI / 飞书 / 云端同步

### 2026-06-04 — 方案 C:VPS 自托管 daemon(远期留接口)

- **决策**:v2 之后考虑在 VPS 上跑自托管 daemon,做多设备同步;**前期不展开,只留接口**
  - **原因**:个人用 + 学习目的,前期过度设计分布式拓扑是负担;但 Channel Adapter 协议要设计成 network-ready,worktree 路径要跨机器一致,否则后期改不动
- **决策**:Channel Adapter 协议必须走明文 JSON,载体无关(Unix socket / HTTPS / WSS 都能承载)
  - **原因**:不锁传输层,后期接 VPS 客户端不用改 agent core
- **决策**:worktree 路径统一用 XDG 标准 `~/.local/share/everlasting/worktrees/<project_hash>/<session_id>`
  - **原因**:跨机器接续时,worktree 路径必须可预测;XDG 标准是 Linux 共识
- **前期动作**(本决策已落地):
  - ARCHITECTURE §3 改 worktree 路径
  - ARCHITECTURE §5 Channel trait 注明 network-ready 约束
  - BACKLOG §4 列 v2 跨设备候选
- **后期展开**(v2 再说):
  - 多设备接续、配置/状态/session 列表同步、显式"工作树迁移"流程
  - 接续前置条件:必须 push 过 + 目标机器不能在跑 LLM
  - 详见 [BACKLOG §4 跨设备（v2 候选）](./BACKLOG.md#4-跨设备v2-候选)

### 2026-06-05 — 步骤 3b-1 follow-up 沉淀 (FU-1/2/3 项目决策)

- **FU-1 · cwd 简化为 `~/`**：3b-1 起 `ToolContext.cwd` 默认值从 `std::env::current_dir()` 改为 `~/`（`dirs::home_dir()`）。理由：LLM 工具调用产生的相对路径在跨 session 时能稳定解析。详见 [`docs/_archive/2026-06-3b-1/FOLLOW-UP.md FU-1`](../_archive/2026-06-3b-1/FOLLOW-UP.md)。
- **FU-2 · TS interface 字段 snake_case → camelCase**：Tauri 2 IPC 默认 `rename_all = "camelCase"`，前端 TypeScript interface 字段必须用 camelCase，**不要**在 TS 侧再写 snake_case 类型（如 `initialCwd` 不要写成 `initial_cwd`）。详见 [`docs/_archive/2026-06-3b-1/FOLLOW-UP.md FU-2`](../_archive/2026-06-3b-1/FOLLOW-UP.md)。
- **FU-3 · `pick_project_dir` 用 reka-ui 渲染 dialog**：Tauri command 不再负责弹原生 dialog，统一改为前端用 reka-ui 的 `Dialog` 组件（后端只暴露 path 校验）。详见 [`docs/_archive/2026-06-3b-1/FOLLOW-UP.md FU-3`](../_archive/2026-06-3b-1/FOLLOW-UP.md)。
