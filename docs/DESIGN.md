# DESIGN — 需求设计

> Everlasting 的"是什么、为什么、边界在哪"。这是文档的入口。
> 架构设计见 [ARCHITECTURE.md](./ARCHITECTURE.md),技术选型见 [TECH.md](./TECH.md),实现路径见 [IMPLEMENTATION.md](./IMPLEMENTATION.md),候选功能见 [BACKLOG.md](./BACKLOG.md)。

---

## 1. 文档目的

这不是产品规格说明,也不是产品愿景。这是**给自己看的工程决策备忘录**,用来:
- 在开始写代码前,把架构想清楚
- 记录"为什么这么选,而不是那么选"
- 之后再回来能快速回到上下文
- 讨论时有共同语言

讨论过程中产生的关键决策会沉淀到 [IMPLEMENTATION.md §4 决策日志](./IMPLEMENTATION.md#4-决策日志)。

---

## 2. 项目定位

### 2.1 一句话

**给个人用的 vibe coding 桌面工作台,WSL 是真正的家,agent core 自己写,顺带学 harness engineering。**

### 2.2 关键约束

| 维度     | 选择                                         | 反例                                  |
|----------|----------------------------------------------|---------------------------------------|
| 目标用户 | **仅本人**                                   | 不考虑他人付费、不考虑多租户          |
| 平台     | **WSL 优先**(Linux Ubuntu 22.04)             | Windows / macOS 都不主动支持          |
| 技术栈   | Tauri + Rust 后端 + Vue 3 前端             | 不考虑 Electron、PyQt 等              |
| Agent    | **自己写 runtime**                           | 不包装 Claude Code / Codex SDK        |
| 商业化   | 无                                           | 不会发出去、不考虑隐私多用户          |

### 2.3 核心差异点

普通 coding agent(Claude Code / Cursor / Copilot)都是通用工具。Everlasting 的存在理由是:

1. **WSL 体验做到极致** — 不是"凑合能用",是"Windows 用户在 WSL 里写代码的最佳伴侣"
2. **自研 agent core** — 学习价值 + 完全可控 + 不被厂商牵着走
3. **多项目 / 多 session / 工作流** — 不是一次性对话,是一个持久的工作环境

如果哪天这三点不再重要,这个项目就失去了存在理由。

---

## 3. Scope(明确什么做、什么不做)

### 3.1 MVP(核心必做)

- [ ] 单项目 = 一个本地目录 + git 仓库
- [ ] 一个项目下多个 session,每个 session 独立对话历史
- [ ] 跟单个 agent 对话,流式输出 token
- [x] Agent 工具基础集:`read_file` / `write_file` / `shell` 已实现(步骤 2)
- [x] Agent 工具扩展:`edit_file` / `grep` / `glob` 已实现(2026-06-07,1 个 `feat(tools):` commit)
- [ ] 文件变更实时显示(diff 视图)
- [ ] Git 集成:每个 session 一个 worktree,自动 commit
- [ ] WSL 项目管理:原生支持,文件操作走 WSL 内部路径
- [ ] SQLite 持久化 session、message、tool call 记录

### 3.2 v1(MVP 之后)

- [ ] **多 LLM provider 切换**(Anthropic / OpenAI / 本地 Ollama)
- [ ] Token 用量统计 + 预算警告
- [ ] 嵌入式 xterm.js,实时显示 agent 跑的 shell 命令
- [ ] 完整的 tool 权限系统(per-tool, per-session, per-project)
- [ ] Session 恢复(关掉应用明天接着聊)
- [ ] MCP server 支持(把工具暴露给 Claude Desktop / Cursor)

### 3.3 v2 及后续(候选功能)

> ⚠️ **版本号语义统一**:本文档用两套版本号,**不重叠**——
>
> - **产品版**:MVP / v1 / v2 / v3+(§3.2 的 v1 指"整体产品的第一个发布版",多 LLM / xterm / 权限 / MCP / session 恢复)
> - **功能版**:Phase 1 / Phase 2(下文各功能内的"v1/v2"是该**功能自身**的阶段,例:UI primitives Phase 1 必做 4 种、角色 Phase 1 不做编排)
>
> 任何文档读到 v1/v2 时,先判定属于"产品版"还是"功能版",再读。

具体技术评估见 [BACKLOG.md](./BACKLOG.md),本节列出全部候选功能(以 9 项为基础,补 3 项):

- [ ] **输入层扩展**:图片粘贴/拖拽 / @文件补全 / /command 命令面板
- [ ] **Agent Skill**:打包"做某事的方法",用户和 LLM 都可调
- [ ] **多层 Memory**:user / project / session / runtime 四层指令
- [ ] **多角色**:架构师 / 开发者 / reviewer / tester / 文档作者
- [ ] **多模式**:chat / plan / review / background / yolo
- [ ] **可编排**:多 agent DAG 串行/并行(留 Phase 2)
- [ ] **生成式 UI**:LLM 输出渲染为可交互组件(约束式,默认关)
- [ ] **IM 通道**:飞书接入(触发 agent daemon 化,重大架构变更)
- [ ] **云端同步**:Cloudflare Workers + D1,只 push 摘要
- [ ] **工作树可视化**:git graph(看 session 分支关系)
- [ ] **项目级自定义指令**:CLAUDE.md 之类的 per-project 系统 prompt
- [ ] **Context 压缩**:长 session 自动摘要(早期不实现,失忆时再做)

**注意事项**:
- 这 12 项不全是独立功能,有一些强依赖(见 [BACKLOG.md §0 全局视角](./BACKLOG.md#0-全局视角这-7-个功能落在-5-个不同的层))
- 飞书会触发 **agent daemon 化**,是最大的架构变更,要早评估
- Skill / Memory / Role 共享 frontmatter loader,可以一次设计统一实现
- 云端同步**不是**多端协作(明确不做),只是个人远程遥控

### 3.4 远期(v3+)

- [ ] 团队协作(暂时不做)
- [ ] 云端同步(个人远程遥控,详见 [BACKLOG.md §7](./BACKLOG.md#7-云端状态同步))
- [ ] 移动端(不做)
- [ ] 公开市场(不做)

> ⚠️ **关于"云端"**:本文档区分两个概念——
> - **云端部署**:把 agent 跑在云服务器上 ❌ 不做
> - **云端同步**:用云服务做"状态镜像 / 远程遥控通道" ✅ 远期考虑
>
> 这两项**不矛盾**,前者是"agent 跑哪",后者是"数据镜像到哪"。

### 3.5 明确不做

**核心不做**(项目根基):
- ❌ **不包装 Claude Code SDK / Codex SDK** — 违背学习目标(详见 [IMPLEMENTATION.md §1](./IMPLEMENTATION.md#1-决策自己写-agent-runtime不用-sdk-包装))
- ❌ **不做通用 agent 框架** — Cline / OpenHands 已经在做
- ❌ **不做 Windows 端优化** — WSL 跑得好就行(详见下文 §4 WSL 优先)
- ❌ **不做云端部署** — 本地优先,agent 进程不出本机
- ❌ **不做移动端 / Web 版** — 桌面应用

**范围守护**(避免后期蔓延):
- ❌ **不做 Yolo 模式默认开** — Yolo(无任何确认)必须显式开启,默认拒绝(详见 [BACKLOG §4.2](./BACKLOG.md#42-多模式mode))
- ❌ **不做云端触发器** — 定时/事件触发源必须在本地(系统时间、fs 事件、本地 webhook);Cloudflare Cron Trigger 之类不接
- ❌ **不做 in-app 自动升级** — 新版本走包管理器或手动下二进制,降低供应链攻击面和复杂度
- ❌ **不做云端触发回写本机** — agent 不接受"从云端推下来"的任务,主动权必须在本地用户

**关于"云端"语义**:
> - **云端部署**:把 agent 跑在云服务器上 ❌ 不做
> - **云端同步**:用云服务做"状态镜像 / 远程遥控通道" ✅ 远期考虑
>
> 这两项**不矛盾**,前者是"agent 跑哪",后者是"数据镜像到哪"。详见 [BACKLOG §7](./BACKLOG.md#7-云端状态同步) 和 [BACKLOG §9 跨设备](./BACKLOG.md#9-跨设备v2-候选)。

---

## 4. 决策:WSL 优先,Windows 次要

**WSL 优先的具体含义**:
- Tauri 编译运行在 WSL 内部,通过 WSLg / Wayland 显示到 Windows 桌面
- 项目文件存放在 WSL 内部(`~/projects`),不走 `/mnt/c`
- 所有 git / shell / fs 操作都在 WSL 内完成
- 不考虑纯 Windows 路径作为项目主路径

**为什么要这样**:
- WSL 内部的文件系统性能远高于 `/mnt/c`
- 几乎所有 dev 工具在 Linux 上行为最标准
- Git 在 WSL 里跑比 Windows 快很多
- 沙箱在 Linux 内核支持更全(landlock / namespace),虽然在 WSL2 内有用户态限制(详见 [§5.1 风险表](#51-技术风险))

**用户视角**:
- 在 Windows 桌面看到一个 Tauri 窗口
- 背后是 WSL 里的 Linux 进程
- 像用 WSL Terminal 一样,只是多了个 GUI

---

## 5. 风险与权衡

### 5.1 技术风险

| 风险                          | 严重度 | 缓解                                          |
|-------------------------------|--------|-----------------------------------------------|
| Rig 0.x breaking change       | 中     | 锁版本,major 升级专门花时间                  |
| Tauri 2 在 WSLg 下的 bug       | 低(✅ spike-001 已验证可用) | 准备 fallback 到 WSL 内部启动 + VNC/X11 转发  |
| Git2-rs worktree API 不全      | 中     | 必要时 spawn `git worktree` 命令              |
| Linux sandbox (bwrap/landlock) | 高     | WSL2 默认禁 user namespace,bwrap 实际不可用;退路:landlock(内核 5.13+,需 WSL2 内核版本对齐)/ firejail / 应用层黑名单(rm -rf /、curl \| sh 之类)。这是 [⑨ Tool 权限](./ARCHITECTURE.md#9-工具权限检查) 实施的前提 |
| LLM 流式 token 断连            | 低     | 实现重连,断点续传用 message ID                |
| 上下文爆炸                    | 高     | 实现 context 压缩、消息裁剪、tool result 截断 |
| 循环检测(agent 死循环)        | 高     | 跟踪相同 tool call 出现 N 次,自动打断        |

### 5.2 工程权衡

**复杂度 vs 学习价值**:
- 选 rig:省掉 50% 样板代码,但少学 50% harness 细节
- 选 reqwest:多学 50%,但每个字节都懂
- **决策**:前两步手写学,第三步切 rig 干活

**功能范围 vs 完成度**:
- MVP 8 项都做,每项做到 70 分,胜过做 15 项每项 40 分
- 任何一项发现比预期难,砍掉,不要延期

**本地 vs 云端**:
- Agent 进程 100% 本地,数据不出本机
- 不考虑自建 LLM 网关(除非后期多设备用)
- 远期"云端同步"只 push 摘要(详见 BACKLOG.md §7)

### 5.3 不可控因素

- **Anthropic / OpenAI 协议变化** — 只能靠 adapter 层隔离
- **Tauri 2 生态成熟度** — 还在快速演进,有些库还在追
- **WSL 跨版本差异** — WSL2 行为稳定,但偶尔有 kernel upgrade 引起的边角问题

**Step 8 (代码重构与文档清理) 进度 (2026-06-10)**:
- 8-PR1 ✅ lib.rs (3195L) 拆为 state/commands/agent 子目录
- 8-PR2 ✅ db.rs (2862L) 拆为 db/ 子模块
- 8-PR3 ✅ ChatWindow/ModelsTab 拆为 sub-components
- 8-PR4 🔄 文档更新 + 9 个空 spec 文件清理(本次)
- 8-PR5 ⏳ STRUCTURE.md 创建

详见 [IMPLEMENTATION §2.9](./IMPLEMENTATION.md#29-步骤-8--代码重构与文档清理)。

---

## 附录 A:常用命令速查

```bash
# 开发
cd app && pnpm tauri dev         # 启动 Tauri 开发模式
cd app && pnpm tauri build       # 打包生产版本

# 数据库
sqlite3 ~/.local/share/everlasting/db.sqlite ".tables"

# 调试
RUST_LOG=debug pnpm tauri dev    # 在 app/ 目录下

# WSL 内查看 Tauri 进程
ps aux | grep everlasting
```

---

> 本文档随项目演进更新。任何重大架构变更,先改这里,再改代码。
