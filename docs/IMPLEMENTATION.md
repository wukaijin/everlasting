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

8 个步骤,每步完成才能进下一步。**不写时间承诺**,只写目标、可交付物、关键产出。

### 2.1 步骤 1 — 骨架与 LLM 直连 [MVP]

**目标**:跑通"Tauri app + 跟 LLM 说一句话 + 流式显示"

- 搭 Tauri 2 + Vue 3 + Vite + Pinia + reka-ui 项目(栈细节见 [TECH §1](./TECH.md#1-决策vue-3-全家桶替代-react))
- Rust 端 LLM 客户端:`reqwest` + `futures-util` + `serde_json`,**手写 SSE 解析**(用 `futures-util::StreamExt`,**不**用 `eventsource-stream`——spike-002 验证手写解析器够用,见 [HACKING-llm.md](./HACKING-llm.md) 和 [spike-002 §"代码关键改动"](./spikes/002-reqwest-anthropic-sse.md#实际执行2026-06-04))
- 4 个文件分模块(client / sse / error / types),实施 11 项 checklist 见 [HACKING-llm.md §"LLM 客户端实施 checklist"](./HACKING-llm.md#llm-客户端实施-checklist给步骤-1-2-写-rust-客户端时)
- 前端:简单 chat UI(输入框 + 消息列表)
- Tauri event 把 SSE chunk 推到前端(`emit("chat-chunk", ...)`)
- **可交付物**:能聊天的最小 app

**前置硬依赖**:
- ✅ spike-001(WSL + Tauri 窗口 + 中文/Emoji)已于 2026-06-04 通过,见 [spikes/001](./spikes/001-wsl-tauri-window.md)
- ✅ spike-002(reqwest + SSE + 错误分类)已于 2026-06-04 通过,见 [spikes/002](./spikes/002-reqwest-anthropic-sse.md)
- 起点:搬 `~/tauri-spike/spike-app/` 到 `/usr/local/code/github/everlasting/app/`,扩成正式骨架。详细起点 + 验收标准见 [HANDOFF §4](./HANDOFF.md#4-mvp-步骤-1-是什么--起点--验收)

**撞过的环境坑**:见 [HACKING-wsl.md](./HACKING-wsl.md)(linuxbrew pkg-config / pnpm 代理 / Rust 1.83 / cargo cache 锁 / WSLg CJK 字体)。**新机器或怀疑环境有问题时,先读 HACKING-wsl**。

### 2.2 步骤 2 — Tool Calling [MVP]

**目标**:agent 能读写文件、跑 shell

- 定义 3 个 tool:`read_file` / `write_file` / `shell`
- 解析 `tool_use` 块,执行,构造 `tool_result` 回填
- agent loop 实现:消息 → LLM → tool_use? → 执行 → tool_result → LLM → ... → 完成
- 前端显示 tool 调用过程(简化版)
- **可交付物**:能帮我改代码的 agent

### 2.3 步骤 3 — 切到 Rig + 项目模型 [MVP]

**目标**:重构到 rig,引入 project / session 概念

- LLM client 从 `reqwest` 切到 `rig-core`
- 引入 SQLite (`sqlx`),存 project / session / message
- UI:左侧项目列表、中间 session 列表、右侧 chat
- session 切换能看到历史消息
- **可交付物**:能管多个对话

### 2.4 步骤 4 — Git 集成 [MVP]

**目标**:session 隔离 + 自动 commit

- `git2-rs` 集成
- session 创建时建 worktree(见 [ARCHITECTURE.md §3 worktree 决策](./ARCHITECTURE.md#3-决策每个-session-一个-git-worktree))
- session 结束或定时自动 commit
- 前端 diff 视图(用 `diff` (jsdiff) + 自渲染,或 `vue-diff-view`)
- **可交付物**:每个 session 是独立分支,能看 diff

### 2.5 步骤 5 — WSL 体验 [MVP]

**目标**:在 WSL 内原生跑起来

- 验证 Tauri 在 WSLg / Wayland 跑得通(此步为整个 app 的**前置硬依赖**,失败则步骤 1-4 全部回滚重新评估)
- 项目文件存放在 WSL 内部(默认 `~/projects/...`),不走 `/mnt/c`
- 所有 git / shell / fs 操作都在 WSL 内完成,**无 wslapi 调用、无路径转换**
- **可交付物**:Tauri GUI 在 Windows 桌面显示,背后跑在 WSL 内,无 Windows ↔ WSL 跨边界

> 💡 步骤 1-4 都在"假设 Tauri 跑得通"前提下推进,本步是首次验证。**强烈建议先把这一小步抽出来作为"步骤 0"跑一次 hello world**,确认底层 OK 再启动步骤 1。

**状态更新(2026-06-04)**:**spike-001 已通过**,本步骤的"WSL + Tauri 跑得通"目标已实现,见 [spikes/001-wsl-tauri-window.md](./spikes/001-wsl-tauri-window.md)。**新 session 不必再单独跑这一步**,直接做步骤 1-4,有问题参考 spike-001 文档的 5 个环境坑即可。

### 2.6 步骤 6 — 嵌入式终端 + 权限系统 [v1]

**目标**:能看 agent 在跑啥,能控制 agent 能干啥

- `xterm.js` + `portable-pty` 跑 shell
- agent 跑的 `shell` tool 输出实时到 xterm
- 权限系统雏形:每个 tool 可以 ask / allow / deny
- **可交付物**:能看见、能拦住 agent

### 2.7 步骤 7 — MCP 暴露 + 多 Provider [v1]

**目标**:你的工具 Claude Code 也能用;切模型无痛

- `rmcp` 起一个 stdio MCP server
- 验证:Claude Desktop 能调用你的 read_file / shell
- 加 OpenAI provider 切换
- 加 Ollama provider 切换(纯本地,省钱)
- **可交付物**:工具集对外开放;模型随便切

### 2.8 步骤 8 — 打磨与文档 [跨阶段]

- Token 用量统计
- 错误处理完善
- README 写完
- 录个 demo 视频
- **可交付物**:能给别人看的最小可用版本

> ⚠️ **Agent Daemon 化的占位**:16 关卡(见 [ARCHITECTURE §2](./ARCHITECTURE.md#2-harness-设计从用户输入到文件变更的-16-道关卡))中有 ⑮ Channel 输出(daemon → client)这一关,需要 agent core 拆出独立进程才能落地。**触发条件**:
> - **若** BACKLOG §6 飞书 channel 决定实施 → 在步骤 5 之后插入"步骤 5.5 — Agent Daemon 化",再做步骤 6
> - **若** 飞书不做 → 推迟到 v2 之后,daemon 化不阻塞当前 8 步
> - **判断窗口**:在步骤 5 完成后、步骤 6 开始前问自己"长跑任务被打断是不是真痛?",痛就拆,不痛就跳
> - 详见 [ARCHITECTURE.md §4 决策:Agent Daemon 化](./ARCHITECTURE.md#4-决策agent-daemon-化为多-channel-接入铺路)

---

## 3. 待办与下一步

**最后更新**:2026-06-04(spike-001/002 + HACKING 文档沉淀完成,环境就位)

**下一步(本 session 之后,新 session 第一件事)**:
- → **[MVP 步骤 1 — 骨架与 LLM 直连](./HANDOFF.md#4-mvp-步骤-1-是什么--起点--验收)**:搬 `~/tauri-spike/spike-app/` 到 `/usr/local/code/github/everlasting/app/`,扩成正式骨架(Vue 3 + Vite + Pinia + reka-ui),Rust 端 LLM 客户端,最小 chat UI。详细起点 + 验收标准见 [HANDOFF §4](./HANDOFF.md#4-mvp-步骤-1-是什么--起点--验收)。
- 步骤 1 完成后 → 步骤 2 [Tool Calling](./IMPLEMENTATION.md#22-步骤-2--tool-calling-mvp)

**已沉淀(spike 期间完成的)—— 不必再做,出问题查这里**:
- ✅ Tauri 在 WSL 跑得通 + 中文对齐 → [spikes/001](./spikes/001-wsl-tauri-window.md)
- ✅ Rust 端 LLM 客户端手写 reqwest + SSE 可走(GLM 3 处差异已知)→ [spikes/002](./spikes/002-reqwest-anthropic-sse.md)
- ✅ WSL 环境坑 → [HACKING-wsl.md](./HACKING-wsl.md)
- ✅ LLM 客户端实施 checklist + 切真 Claude 重测清单 → [HACKING-llm.md](./HACKING-llm.md)

**已决定(不再讨论)**:
- [x] 前端框架:**Vue 3 + Vite + Pinia**(见 [TECH §1.1](./TECH.md#11-锁定项经过调研验证))
- [x] 前端 UI 库:**reka-ui** / shadcn-vue primitives(见 [TECH §1.4](./TECH.md#14-扩展功能新增依赖随候选功能引入))
- [x] 包管理器:**pnpm**
- [x] LLM 客户端:**手写 reqwest + SSE**(不切 rig-core 到步骤 3,见 [spike-002 §"结论"](./spikes/002-reqwest-anthropic-sse.md#结论))
- [x] LLM BASE_URL / model / key:**全部从 env 读**(便于切 wukaijin / 真 Claude / 其他)
- [x] 工作目录:**WSL 内部**(`~/...` 或 `/usr/local/code/...`),不走 `/mnt/c`
- [x] Agent Daemon 化:**v1 之后再说**,本项目 8 步不阻塞(见 §2.8 占位)

**等做完步骤 1-2 再决定**:
- [ ] SQLite schema 最终长什么样
- [ ] Tool 注册的最佳实践
- [ ] Frontend 状态管理边界(Pinia store 怎么分)

**候选功能(来自 [BACKLOG.md](./BACKLOG.md))的待评估**:
- [ ] 是否要做 Skill / Memory / Role(技术选型已就绪,详见 BACKLOG §2-4)
- [ ] 是否做生成式 UI(详见 BACKLOG §5)
- [ ] 是否做飞书 channel(会触发架构变更,详见 BACKLOG §6)

---

## 4. 决策日志

> 按时间倒序记录。每次重大决策都加一条,包含"为什么"。

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
  - **原因**:自由式(LLM 生成 HTML)沙箱难做、调试难;约束式够覆盖 80% 用例。详见 [BACKLOG.md §5](./BACKLOG.md#5-生成式-ui-开关)
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
  - BACKLOG §9 列 v2 跨设备候选
- **后期展开**(v2 再说):
  - 多设备接续、配置/状态/session 列表同步、显式"工作树迁移"流程
  - 接续前置条件:必须 push 过 + 目标机器不能在跑 LLM
  - 详见 [BACKLOG §9 跨设备(候选)](./BACKLOG.md#9-跨设备v2-候选)
