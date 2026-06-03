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

**目标**:跑通"Tauri app + 跟 Claude 说一句话 + 流式显示"

- 搭 Tauri 2 + React + Vite 项目
- Rust 端:`reqwest` + `serde` + `eventsource-stream`,打 Anthropic Messages API
- 前端:简单 chat UI
- Tauri event 把 SSE chunk 推到前端
- **可交付物**:能聊天的最小 app

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
- 前端 diff 视图(用 `react-diff-viewer`)
- **可交付物**:每个 session 是独立分支,能看 diff

### 2.5 步骤 5 — WSL 体验 [MVP]

**目标**:在 WSL 内原生跑起来

- 验证 Tauri 在 WSLg / Wayland 跑得通(此步为整个 app 的**前置硬依赖**,失败则步骤 1-4 全部回滚重新评估)
- 项目文件存放在 WSL 内部(默认 `~/projects/...`),不走 `/mnt/c`
- 所有 git / shell / fs 操作都在 WSL 内完成,**无 wslapi 调用、无路径转换**
- **可交付物**:Tauri GUI 在 Windows 桌面显示,背后跑在 WSL 内,无 Windows ↔ WSL 跨边界

> 💡 步骤 1-4 都在"假设 Tauri 跑得通"前提下推进,本步是首次验证。**强烈建议先把这一小步抽出来作为"步骤 0"跑一次 hello world**,确认底层 OK 再启动步骤 1。

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

**现在就可以做**:
- [ ] 读 Anthropic Agent SDK 的 `query.py` 和 message parser(理解原理,不用)
- [ ] 读 OpenHands Local GUI 的前后端通信协议
- [ ] clone rig 仓库,看 Agent 抽象源码
- [ ] 搭 Tauri 2 项目骨架,跑通 hello world
- [ ] 在 WSL 里验证 Tauri 能编译并显示窗口

**已经倾向但需要最终定**:
- [ ] 前端框架:React(参见 [TECH.md §1.1](./TECH.md#11-锁定项经过调研验证))
- [ ] 前端 UI 库:shadcn/ui primitives(参见 [TECH.md §1.4](./TECH.md#14-扩展功能新增依赖随候选功能引入))
- [ ] 包管理器:pnpm
- [ ] Rust 编辑器:个人偏好

**等做完前两步再决定**:
- [ ] rig vs 手写 LLM client 的边界在哪
- [ ] SQLite schema 最终长什么样
- [ ] Tool 注册的最佳实践

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
