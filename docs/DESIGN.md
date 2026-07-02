# DESIGN — 需求设计

> Everlasting 的"是什么、为什么、边界在哪"。这是文档的入口。
> 架构设计见 [ARCHITECTURE.md](./ARCHITECTURE.md),技术选型见 [TECH.md](./TECH.md),决策档案见 [IMPLEMENTATION.md](./IMPLEMENTATION.md),技术路线图见 [ROADMAP.md](./ROADMAP.md),候选功能见 [BACKLOG.md](./BACKLOG.md)。

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

## 3. 项目能力边界

> 本节讲"项目是什么 + 不是什么"。**做什么 + 什么时候做**见 [ROADMAP.md](./ROADMAP.md);**候选功能技术评估**见 [BACKLOG.md](./BACKLOG.md)。

### 3.1 项目能力(简略)

**已具备**(完整 commit 走 `git log`,粗粒度状态见 [ROADMAP.md §1](./ROADMAP.md#1-已实施mvp-主体--路线图外完成)):

- Tauri 2 + Vue 3 桌面应用,WSL 优先
- 自研 agent core:Agent Loop + Tool Calling + 流式 SSE + 16 关卡请求生命周期(详见 [ARCHITECTURE.md §2](./ARCHITECTURE.md#2-harness-设计从用户输入到文件变更的-16-道关卡))
- 多项目 / 多 session 管理(SQLite 持久化)
- 工具集(19 个 builtin,`app/src-tauri/src/tools/mod.rs::builtin_tools()` 注册):
  - 读 / 写:`read_file` / `write_file` / `edit_file`(ReadGuard 三道 check 前置)/ `grep` / `glob` / `list_dir`
  - Shell:`shell`(Bash 落盘 + cat -n)/ `run_background_shell` / `shell_status` / `shell_kill`(L1a 后台 shell,tokio Child 不带 PTY)
  - 联网:`web_fetch`(SSRF 拦截 + 5 MiB body cap,attribution prefix)
  - Skill / Memory / UI:`use_skill`(B4 三层渐进披露)/ `use_ui`(B9 生成式 UI,non-blocking)/ `update_checklist`(B12 loop-local)/ `remember`(V2 2 期自主记忆写入)
  - 交互:`ask_user_question`(跨 turn,B9 selector 复用)
  - Subagent:`dispatch_subagent`(B6)/ `merge_worker` / `discard_worker`(L3b worker worktree 收口)
- Git 集成:worktree 解耦 + opt-in attach / detach / delete;**L3b PR1-PR4 worker worktree 隔离**(branch 前缀 `worker/<run_id>` + `git worktree lock` + libgit2 fast-forward / 3-way merge + 启动 sweep 清理过期 worker)
- 多 LLM Provider(自研 `Provider` trait,Anthropic / OpenAI 双 Provider;rig-core 已弃用 2026-06-09)
- 顶层 GUI:三栏(Vue sub-components)+ SessionList + 顶部 Tabs + 流式指示器 + B9 `<UiCard>` + L3b PR4 `<WorkerBranchBadge>` + `<WorkerMergeControls>`
- A2+B7 权限系统:⑨ 关 5-tier path-based 决策层 + 3 档 Mode(`edit`/`plan`/`yolo`)+ ⑯ 审计日志 10 类 AuditKind + web_fetch 接入 ⑨ + **`ToolKind::GitMutation`**(L3b PR3+,WebFetch 式 tool-level grant,避免 Shell 串扰)(详见 [ARCHITECTURE §2.2 ⑨ / §2.5.8](./ARCHITECTURE.md))
- C3 Context 压缩 + token 硬卡:`context_window * 0.80` 触发,降到 `0.50`,B5 memory 永远保护,MAX_TURNS 20 → 50 → **200**(C2 06-24 调,详见 [ARCHITECTURE §2.5.5](./ARCHITECTURE.md#255-⑤-context-超限降级c3-mvp2026-06-12-落地已实施))
- C2 循环检测:分级触发 — L1 精确签名硬触发 N=3 + L2 Jaccard 软提示 N=5/0.85;软提示命中后注入 `ContentBlock::Text` hint,**不打断 loop**,MAX_TURNS=200 仍是硬兜底
- B5 Memory/指令文件系统:4 文件(User / Project × CLAUDE.md / AGENTS.md)+ `cache_control: ephemeral` 注入 + 100 KiB 硬卡 + tiktoken cl100k_base 估算 + notify 监听
- **V2 2 期** 自主记忆系统(2026-06-29 落地,5 child epic):agent 自主产生 + 跨 session 召回的经验库 — `autonomous_memories` 表(状态机 candidate→active→verified)+ 两层召回(per-turn FTS5 + 工具前 trigger_key 精确匹配)+ verified 软拦截重判 + 异步卫生 job
- A4 Token 用量统计:per-session 累积(4 列)+ ChatInput hint 区 0-49% 绿 / 50-74% 黄 / 75%+ 红
- D1 session 重命名 + 8 色标记
- C1 取消机制完整化:tool 执行中途可取消(CancellationToken)
- **B12** Checklist(agent 自跟踪进度清单):TodoWrite 式 `update_checklist` tool(全量替换 + 三态 pending/in_progress/done + 至多一 in_progress coerce),loop-local Vec,前端 `<ChecklistCard>` 浮层
- **L1a** 后台 shell + 完成通知:3 tool(`run_background_shell` / `shell_status` / `shell_kill`),session-scoped,默认 `max_runtime_ms` 24h,APPEND user message 保 memory cache breakpoint
- **L2** 单 turn 多 tool 并发(只读 batch):`is_parallel_eligible` 纯谓词 + `FuturesUnordered`,并发集合 `{read_file, grep, glob, list_dir, use_skill}`,多 tool_result 单消息打包
- **L3a-d** Subagent 全套:并发只读 dispatch / worker worktree 隔离 / worker 联网 / frontmatter loader(`~/.config/everlasting/agents/*.md` + `<project>/.everlasting/agents/*.md`)
- **B9** 生成式 UI(部分落地:selector / diff / code_block):`use_ui` tool + `<UiCard>` + component registry + `WorkerBranchBadge` / `WorkerMergeControls` for L3b PR4
- **RULE-D-001** provider api_key 加密存储:AES-256-GCM + HKDF(machine-id),`api_key_enc` 列 + `key_migrated_at` 哨兵,IPC 切断明文

**未做**(排期归 [ROADMAP.md §2](./ROADMAP.md#2-v2-路线图分类2026-06-10-重排),技术评估见 [BACKLOG.md](./BACKLOG.md)):

- 输入层扩展:图片粘贴 / @文件补全(B2)/ /command 命令面板(B3)
- 指令层:Skill 系统(B4);Runtime Memory 与 Session Memory(B5 留位未启用)
- 拓扑层:Subagent(B6,依赖 B5 Session Memory)/ DAG workflow(B8)
- 输出层:生成式 UI 4 primitives(B9)
- 触达层:飞书 IM(B10)/ 云端同步(B11)
- 循环检测(C2)/ 大输出截断统一(C6)

### 3.2 明确不做(硬约束)

> 硬约束 ≠ 排期相关。**这些是项目长期原则,不会因为 V2 / V3 路线图调整而松动**。路线图只动"做什么 + 什么时候做",不动"什么不做"。

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

**V2 重排后新增的"不做"**(2026-06-10 决策):
- ❌ **不做 xterm.js 嵌入式终端** — `shell` tool + 30K 落盘已覆盖"看 agent 在跑啥"的需求
- ❌ **不做 MCP 暴露** — 个人工具,工具集对外开放杠杆不足
- ❌ **不做 Provider 限流(令牌桶)** — 个人使用场景未撞到限流,后期按需再评估

> 完整"移除"决策矩阵见 [IMPLEMENTATION §4 决策日志 2026-06-10 条](./IMPLEMENTATION.md#4-决策日志) + [ROADMAP §3 移除项](./ROADMAP.md#3-移除项--已废弃v2-重排2026-06-10-决定)。

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
| Rig 0.x breaking change       | ✅ **已无** | rig-core 已弃用(2026-06-09),改自研 Provider trait;rig 升级不再适用 |
| Tauri 2 在 WSLg 下的 bug       | 低(✅ spike-001 已验证可用) | 准备 fallback 到 WSL 内部启动 + VNC/X11 转发  |
| Git2-rs worktree API 不全      | 中     | 必要时 spawn `git worktree` 命令              |
| Linux sandbox (bwrap/landlock) | 高     | WSL2 默认禁 user namespace,bwrap 实际不可用;退路:landlock(内核 5.13+,需 WSL2 内核版本对齐)/ firejail / 应用层黑名单(rm -rf /、curl \| sh 之类)。这是 [⑨ Tool 权限](./ARCHITECTURE.md#9-工具权限检查) 实施的前提 |
| LLM 流式 token 断连            | 低     | 实现重连,断点续传用 message ID                |
| 上下文爆炸                    | 高     | ✅ C3 context 压缩(0.80→0.50,B5 保护)+ 消息裁剪 + tool result 截断 |
| 循环检测(agent 死循环)        | 高     | ✅ C2 分级触发 — L1 精确签名硬触发 N=3 + L2 Jaccard 软提示 N=5/0.85;软提示注入 hint 不打断,MAX_TURNS=200 兜底 |

### 5.2 工程权衡

**复杂度 vs 学习价值**(历史决策,2026-06-04 起 + 2026-06-09 rig-core 弃用):
- 选 rig:省掉 50% 样板代码,但少学 50% harness 细节
- 选 reqwest:多学 50%,但每个字节都懂
- **决策**:前两步手写学(步骤 1-2);rig-core 评估后于 2026-06-09 弃用(0.38.1 阶段),改自研 `Provider` trait 走 Anthropic / OpenAI 双 Provider(详见 [TECH §2](./TECH.md#2-决策rig-core-弃用2026-06-09改自研-provider-trait) + [IMPLEMENTATION §4 决策日志 2026-06-09](./IMPLEMENTATION.md#4-决策日志))

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
