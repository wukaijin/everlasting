# 调研: coding agent 的异步与并行能力 — 行业实证与本项目评估

**日期**: 2026-06-19
**状态**: 调研完成,结论待决策(尚未立项)
**触发问题**: 抛开"对标 Claude Code"的目的,异步 tool / 让 agent 同时跑多个任务有没有价值? opencode / Hermes 之类的 agent 有没有这种机制? 为什么?

---

## TL;DR

1. 本轮之前给出的"主流 coding agent 都同步、对标 Claude Code 所以不做异步"结论**站不住**:① "用对标当论据证明不该做对标做的事"是循环论证;② 事实也不准 —— Claude Code 自己也有 Task/subagent 并行。
2. **行业实证**(基于 context7 文档检索): **opencode** 和 **Hermes**(Nous Research)都有成熟的并行/异步机制,且已在生产可用。
3. 异步/并行**有价值**,应按收益/成本分三层推进:**L2 单 turn 多 tool 并发** → **L1 后台 shell + 完成通知** → **L3 并行 subagent + worktree 隔离**。
4. 本项目 `app/src-tauri/src/git/` **已有 git2-rs worktree 模块**,这正是 L3 的硬前提基础设施 —— Hermes 花力气做的 worktree 隔离,本项目门槛更低。

---

## 1. 异步/并行的三层模型

讨论前必须区分"异步"的三个层次,它们解决的问题、前提、成本完全不同:

| 层级 | 能力 | 解决的问题 | 硬前提 |
|---|---|---|---|
| **L1** | 单个 tool 后台执行 + 完成通知 LLM | 长 shell(build/test/server)不阻塞 agent loop | 一个"会话/句柄 + 事件注入"机制 |
| **L2** | 一个 turn 内多个 tool_use 并发执行 | LLM 一次返回 N 个独立 tool(3 个 read_file),不串行排队 | tool 之间无写冲突 |
| **L3** | 多个独立 subagent 并行 + 任务编排 | "重构 A 的同时写 B 的测试"类并行任务 | **git worktree 隔离**(防写踩踏) + context 合并机制 |

---

## 2. 行业实证(context7 文档检索,2026-06-19)

> 注: 当日 `web_search_prime` / `web_reader` 限流(2026-06-30 重置),实证来自 context7 文档检索(不同后端,未限流)。下列机制均有对应文档出处,见 §6。

### 2.1 L1 — 后台 shell + 完成通知(本项目工作 2 原始问题的正解)

**opencode-pty**(opencode 的 PTY 插件,设计可直接借鉴):

```javascript
// 后台启动一个长任务,开启退出通知
pty_spawn({
  command: "npm",
  args: ["run", "build"],
  title: "Build",
  notifyOnExit: true,
  timeoutSeconds: 900,
})
```

进程退出时,**往 agent 的下一个 turn 注入一条结构化 XML 通知**(未改 LLM 协议):

```xml
<pty_exited>
ID: pty_a1b2c3d4
Title: Build
Exit Code: 0
Output Lines: 42
Last Line: Build completed successfully.
</pty_exited>
```

配套:`pty_list` / `pty_read`(分页 + 正则过滤) / `pty_write`(发输入,含 `\x03` 中断) / `pty_kill`。还支持 dev server、`--watch`、交互式 REPL。

**Hermes**: `terminal(background=true)` 立即返回 `session_id`,再用 `process(action=poll|wait|log|kill|write)` 管理生命周期。

**关键洞察**: L1 的标准实现**不改 LLM tool 协议**(tool 仍同步返回),而是"tool 返回一个会话句柄 + 进程退出时向 agent loop 注入一条系统消息"。opencode-pty 的 `<pty_exited>` 是最干净的范本。

### 2.2 L2 — 单 turn 多 tool 并行

**Hermes**: MCP server 需**显式声明** `supports_parallel_tool_calls: true`,Hermes 才会在一个 batch 内并发执行该 server 的多个 tool:

```yaml
mcp_servers:
  docs:
    command: "docs-server"
    supports_parallel_tool_calls: true
```

这层是"协议默认串行、需 opt-in 才并行"的保守设计 —— 因为并发执行要求 tool 之间确实无副作用冲突。

### 2.3 L3 — 并行 subagent + 任务编排(最有价值)

**opencode**: agent 有 `mode: subagent` 一档,主 agent 可通过 `@mention`(`@general help me search...`)或 `command.subtask: true` 调起 subagent;orchestrator agent 还有专门的 `task` 权限层(`"*": "deny", "orchestrator-*": "allow"`)。

**Hermes**(最完整):

```python
# 并行委派多个独立子任务,默认并发 3,可配到 30
delegate_task(tasks=[
    {"goal": "Research topic A", "toolsets": ["web"]},
    {"goal": "Research topic B", "toolsets": ["web"]},
    {"goal": "Fix the build", "toolsets": ["terminal", "file"]},
])
```

配置:
```yaml
delegation:
  max_concurrent_children: 30   # 并发批大小
  max_spawn_depth: 2            # 委派层级(最大 3)
```

之上还有 **Kanban 任务图编排**:`kanban_create` 拆任务 → `parents` 声明依赖 → dispatcher 自动派发 → worker agent 跑(`kanban_heartbeat` 上报进度)→ `kanban_complete` 汇总。典型 orchestrator 模式:

```python
# 拆成 2 个并行 research + 1 个 synthesis(依赖前两者完成)
kanban_create(title="research ICP funding, NA angle", assignee="researcher-a")  # t_r1
kanban_create(title="research ICP funding, EU angle", assignee="researcher-b")  # t_r2
kanban_create(
    title="synthesize into launch post draft",
    assignee="writer",
    parents=["t_r1", "t_r2"],   # 两者都完成后才 ready
)
```

**隔离**(L3 的硬前提): Hermes 明确用 **git worktree** 隔离每个并行 agent:

```bash
cd /path/to/your/repo
git worktree add ../repo-experiment-a feature/hermes-a
git worktree add ../repo-experiment-b feature/hermes-b
# 每个 worktree 里跑独立的 hermes
```

CLI 也有 `hermes -w`(在独立 worktree 里跑)专门支持并行 agent。

---

## 3. 本项目现状

| 项 | 现状 | 位置 |
|---|---|---|
| tool 执行模型 | **串行** `for (id,name,input) in &tool_calls { execute_tool().await }` | `agent/chat_loop.rs:995-1042` |
| shell | `tokio::process::Command`(异步 API)但 agent loop **同步 await 阻塞** | `tools/shell.rs:354-440` |
| 后台/异步 | **无** `tokio::spawn` 用于 tool、无 deferred result、无通知机制 | 全项目 |
| **worktree 基础设施** | **已有** `git2-rs worktree + diff` 模块 | `app/src-tauri/src/git/` |

即:本项目当前是纯同步串行模型,但 **L3 的硬前提(worktree 隔离)已经就位**,只是还没接到 agent loop。

---

## 4. 为什么不是所有 agent 默认并行(工程本质)

不是"做不到/没价值",是**有真实成本**,所以 opencode/Hermes 都做成可选/插件,让用户在值得的场景开:

1. **共享状态踩踏**: 并行 agent 改同一仓库会互相覆盖 → 必须用 **git worktree 隔离**。这是 L3 的硬前提,Hermes 文档专门讲了。
2. **编排/合并开销**: 拆任务、分配、收集、综合本身耗 token 和推理 —— 任务太小不值得。
3. **上下文隔离 vs 合并**: 每个并行 subagent 独立 context(防污染),结果要压缩后合并回主 context。
4. **可预测性/调试**: 串行好 replay,并行竞态难查。对本项目这种带 C4 审计日志(10 类 AuditKind)的系统,并行会显著增加审计复杂度。
5. **收益依赖任务可分性**: 调研多方案 / 独立重构 / 长后台任务 = 净收益;强顺序依赖的 coding = 没收益。

**结论**: 这是成本-收益权衡,不是"该不该做"。Hermes / opencode 判定值得,就做了。

---

## 5. 落地路径(按收益/成本排序)

| 步 | 能力 | 成本 | 价值 | 契合点 |
|---|---|---|---|---|
| **L2** | `for await` → 并发执行独立 tool(`futures::join_all`) | 低 | 高 | 纯收益,只读 tool 无冲突;改动集中在 `chat_loop.rs` |
| **L1** | 后台 shell + `notifyOnExit`:tool 返回 session_id,退出注入系统通知 | 中 | 中高 | 直接借鉴 opencode-pty;复用本项目 `sink` 事件 + 进程组 kill(RULE-E-002);解决 120s 超时盖不住的长任务 |
| **L3** | 并行 subagent + worktree 隔离(仿 Hermes `delegate_task`) | 高 | 高(旗舰) | 本项目学习 harness 工程的核心难点;**worktree 基础设施已就位** |

**推荐**: 把 **L3 当作本项目的旗舰特性**评估 —— 它是 harness 工程最有学习价值的部分(任务分解 / worktree 隔离 / context 合并 / 编排),且 Everlasting 的 `git/` worktree 模块让门槛比 Hermes 当初低。L1/L2 是顺手增益,L3 是真正拉开和"同步 agent"差距的能力。

### 5.1 本项目落地校准 — L1 的两个隐藏成本点(2026-06-19 评估补充)

> 以下两点在本项目代码现状 + DEBT 交叉核对后发现,**§2.1 / §5 的 opencode-pty 范本未点透**。L1 立项前必须先解。

**① L1 的 request-scoped 断裂点 + daemon 化强耦合**

- **现状**:本项目 chat loop 是 **request-scoped** —— 前端一次 `invoke("chat")` → Rust `chat` 命令跑完整个 agent loop(`agent/chat_loop.rs`,`MAX_TURNS=50`)→ 返回,一个用户回合顶到顶。
- **矛盾**:后台 shell 的生命周期天然**跨多个回合**(agent 在后台 `npm run build` 跑着时继续对话,或 build 在两次 `invoke` 之间完成),它塞不进单个 request 的 await 链。
- **必需形态**:`AppState`/`SessionState` 持有**跨请求常驻态** —— `pty_sessions: HashMap<SessionId, Vec<PtySession>>` + `pending_notifications: VecDeque<SystemNotice>`,agent loop 每轮开头消费 `pending_notifications` 注入 context(对齐 §2.1 `<pty_exited>`)。这是 **mini-daemon 形态**。
- **强耦合点**:这套会话生命周期管理(close session 时 kill 所有 pty、app 退出清理)与 [CLAUDE.md](../../CLAUDE.md) 的 **daemon 化路线**(GUI 进程与 Agent Daemon 分离,Unix socket/WebSocket IPC)**是同一命题** —— daemon 化天然承载 L1 的常驻会话。
- **决策含义**:L1 立项前先定 daemon 化边界 —— 先做"GUI 进程内常驻"中间态(daemon 化时要重构),还是直接奔 daemon。否则写出要推翻两次的会话管理代码。**建议:L1 与 daemon 化一并规划,不单独 ship 会被推翻的中间态。**

**② PTY vs 后台 Command 分叉(成本差一倍)**

- §2.1 opencode-pty 是**真 PTY**(`pty_write` 可发交互输入,支持 dev server / `--watch` / 交互式 REPL),但范本未点透真 PTY 的实现重量。L1 有两条成本差一倍路径:

| 路径 | 实现 | 解的痛点 | 成本 |
|---|---|---|---|
| **后台 `tokio::process::Command`** | spawn 子进程 + `tokio::spawn` 监听退出 + 完成注入 `<pty_exited>` | shell 120s timeout 盖不住的长任务(build/test) | **低** |
| **真 PTY** | `portable-pty`/`rustix` pty 分配 + 主从端 + 输入写入 + 尺寸调整 | 上述 + 交互式 dev server / REPL(可 `pty_write`) | **高(翻倍)** |

- 本项目 `tools/shell.rs` 当前是 `tokio::process::Command`(管道模式,**非** PTY),RULE-E-002 进程组 kill 已就位(`pty_kill` 可复用)。
- **分阶段建议**:**L1a(后台 Command,解 timeout,低)→ L1b(真 PTY,交互式,按需)**。120s timeout 痛点 L1a 即解,交互式 dev server 是 L1b 的增量价值;不要一上来背 PTY 复杂度。

> **共同含义**:L1 不是"复用 shell.rs 加个 spawn"的轻活 —— 真实门槛在会话生命周期架构(daemon 化)和 PTY 选型,不在 tool 执行本身。与 L2(改一个循环、低门槛)对比,进一步佐证推进顺序 **`L2 → L1`**。

---

## 6. 出处(context7 文档源)

- opencode 核心(subagent/agent/task 权限): `github.com/anomalyco/opencode` `packages/web/src/content/docs/agents.mdx`、`specs/v2/tools.md`
- opencode-pty(L1 后台 shell + `<pty_exited>` 通知): `github.com/shekohex/opencode-pty` `README.md`、`src/plugin/pty/tools/spawn.txt`
- opencode-sessions(parallel exploration): `github.com/malhashemi/opencode-sessions`
- Hermes(L1/L2/L3 全套): `hermes-agent.nousresearch.com/docs/user-guide/features/{delegation,tools,mcp,kanban}`、`docs/guides/delegation-patterns`、`docs/user-guide/git-worktrees`、`docs/user-guide/cli`

---

## 7. 关联

- **与 shell `timeout` 调研的关系**: L1(后台 shell)正是"长任务"问题的正解 —— 长任务不该靠"把 timeout 调到 600s 阻塞等",而该后台跑 + 完成通知。详见 timeout 引导已落地,见 commit `72329ff`(`docs(shell): timeout description`) + `shell.rs` description。
- **与 daemon 化路线的关系**: CLAUDE.md 提到后期"Tauri GUI 进程与 Agent Daemon 进程分离,通过 Unix socket / WebSocket IPC"。daemon 化天然适合承载 L1/L3 的后台会话与多 agent 编排。
- **登记状态**: L1/L2/L3 已登记 `docs/ROADMAP.md` §2 第三档(L3 依赖 B6 subagent,缓做)。
