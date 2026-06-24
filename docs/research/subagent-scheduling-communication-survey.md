# 调研: Subagent 调度与实时通信机制 — 行业全景

**日期**: 2026-06-25  
**状态**: 调研完成,待决策  
**作者**: deepseek-v4-flash  
**触发问题**: L3a（subagent 并发 + 不阻塞）设计前,调研行业内 subagent 的调度方式和实时通信机制,为并发模型设计提供参考。

---

## TL;DR

1. **当前 Everlasting subagent 是同步阻塞、单 worker 串行**：父 LLM 通过 `dispatch_subagent` tool_use 委派 → `run_chat_loop` 递归 `.await` 阻塞 → subagent 跑完返回 `tool_result`。**无子→父中间流,无子↔子通信。**

2. **业界没有 "subagent 并发 + 实时通信" 的成品方案**——这两个目标本质矛盾：subagent 的核心价值是 context 隔离,如果父 LLM 能实时看到中间输出,隔离就被打破了。所有现有方案选择其一：
   - **并发（Hermes）**：做并行 fan-out,但子 agent 间/子→父无实时流,只拿最终结果
   - **实时通信（CC Agent Teams）**：做对等 agent 协作（Mailbox 消息系统）,但那不是 "subagent",是独立 session 的团队协作

3. **L3a 的核心难点排序**：文件隔离(worktree) > 结果合并 > 任务分解 > 编排 > 实时通信。Everlasting 的 `git/worktree.rs` 已就位,门槛比 Hermes 当初低。

4. **推荐的 L3a 起点**：并发 fan-out + worktree 隔离（仿 Hermes `delegate_task`）,不改 tool_result 回传模式,只改串行→并行。

---

## 1. 当前 Everlasting subagent 实现

### 1.1 调度流程

```
父 LLM turn N
  → LLM 输出 dispatch_subagent tool_use { subagent, task }
  → chat_loop.rs 拦截（绕过常规 execute_tool 路径）
  → run_subagent() @ dispatch.rs:85
    → 解析参数,lookup_subagent()
    → filter_tools_for_subagent() 构建 tool subset
    → build_worker_messages() 构造 [memory_blocks, delegation_task]
    → 子 CancellationToken（parent_token.child_token()）
    → subagent_runs 表 INSERT running 行
    → SubagentBufferSink（不转发到父 sink）
    → Box::pin(run_chat_loop(...)).await  ← 父阻塞在这里
      → subagent 跑自己的完整 agent loop（最大 200 turns,skip_persist=true）
      → 每轮 sink.record() 发射 subagent:event IPC（前端可见）
    → 跳出后 drain sink: final_text() + 状态判定
    → truncate_transcript_for_persistence（4 MiB cap）
    → update_run_finished 持久化 subagent_runs
    → emit subagent:finished IPC
    → format_dispatch_result() → (content, is_error, ...)
  → 包装为 ToolResultPayload,推入 result_blocks
  → 父 LLM turn N+1 收到 tool_result
```

### 1.2 关键特征

| 特征 | 状态 | 位置 |
|------|------|------|
| 同步/阻塞 | ✅ **同步阻塞** | `dispatch.rs:286` `.await`；`mod.rs:109` 自述 "synchronous" |
| 并发 | ❌ **单 worker 串行** | `chat_loop.rs:1697-1702` 排除在 `is_parallel_eligible` 外 |
| 子→父实时流 | ❌ 只拿最终 tool_result | SubagentBufferSink 只向前端 IPC 发射,不向父 LLM 回传 |
| 子↔子通信 | ❌ | 无此概念 |
| 父↔子双向 | ❌ | 单向委派 |
| 嵌套深度 | ❌ 限制深度 1 | `mod.rs:350` `STRUCTURALLY_DISABLED` |
| 前端实时展示 | ✅ `subagent:event` + `subagent:finished` IPC | `sink.rs:238-281` |
| 权限交互 | ✅ `permission:ask` 共享通道 + `is_worker=true` Deny | `sink.rs:608-659` |

---

## 2. Claude Code（Anthropic 官方）— 5 层并行模型

> 信息来源：docs.anthropic.com 官方文档（2026-06-25 获取）

### 2.1 Subagent（Task tool）— 与 Everlasting 同模式

Claude Code 最基本的 subagent 机制就是 **Agent tool**（曾用名 Task tool）,模式与 Everlasting 完全一致：

```
父 LLM → Agent tool_use { subagent_type, prompt }
  → subagent 在独立 context 中运行
  → 完成后 final message 作为 tool_result 返回
  → 父 LLM 继续
```

| 属性 | 表现 |
|------|------|
| 是否阻塞 | ✅ 同步阻塞 |
| 子→父实时流 | ❌ 只拿最终结果 |
| 子↔子通信 | ❌ |
| Context 隔离 | ✅ 完全独立,不继承父对话历史 |
| 工具限制 | ✅ `tools`/`disallowedTools` allowlist |
| 嵌套 | ✅ 最多 5 层（v2.1.172+） |

**文档原文**: "Subagents are separate agent instances that your main agent can spawn to handle focused subtasks. ... Each subagent runs in its own fresh conversation. Intermediate tool calls and results stay inside the subagent; only its final message returns to the parent."

**Claude Code 有三种内置 subagent**：

| Subagent | Model | 工具 | 适用场景 |
|----------|-------|------|---------|
| Explore | Haiku（快速） | 只读（Read/Grep/Glob） | 代码搜索、文件发现 |
| Plan | 继承父模型 | 只读 | Plan 模式下的代码库调研 |
| General-purpose | 继承父模型 | 全部工具 | 复杂多步骤任务 |

Everlasting 的 `researcher` / `general-purpose` 二分法与此对标。

### 2.2 Forked Subagent — 不阻塞,继承父 context

```
/fork 或 Fork tool
  → 创建独立 session（非 tool_use 模式）
  → 继承父的完整对话历史
  → 父 session 继续运行,不阻塞
  → 子 session 独立,用户可切换
```

| 属性 | 表现 |
|------|------|
| 是否阻塞 | ❌ 不阻塞（独立 session） |
| 通信 | ❌ 无（独立 session,各自为政） |
| Context | ✅ 继承父完整对话（与普通 subagent 相反） |

**关键差异**：Forked subagent 是"分支对话"而非"工具委派",适合用户想并行探索不同方向但不想影响主对话的场景。

### 2.3 Agent View（claude agents）— 后台 session 管理

```
claude agents
  → 打开一个全屏 dashboard
  → 显示所有后台 session 的状态行（working / needs input / completed）
  → 每个 session 是独立 Claude Code 实例
  → supervisor 进程管理生命周期
  → 用户可 peek（Space 键看实时输出）、reply、attach（Enter 键进入全对话）
```

| 属性 | 表现 |
|------|------|
| 调度方式 | 用户显式 dispatch（或 `/bg` 把当前 session 转入后台） |
| 是否阻塞 | ❌ 不阻塞,后台持续运行 |
| 实时通信 | ✅ peek 面板实时查看状态行 + 最新输出 |
| 反向通信 | ✅ 可回复、可 attach 进入全对话 |
| 子↔子通信 | ❌ 每个 session 独立 |

**架构实现**：
- **supervisor 进程**：独立后台进程管理所有 session,session 在 agent view 关闭后持续运行
- **状态持久化**：session 状态存盘,机器休眠后恢复
- **Row summaries**：用 Haiku 模型每 15s 生成一行摘要（额外成本）
- **Worktree 隔离**：每个 dispatch session 自动移入独立 worktree

### 2.4 Agent Teams 🧪 — 对等多 agent 协作（最接近 "实时通信"）

> 🧪 实验性功能,默认关闭（`CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`）

```
Lead（当前 session）→ spawn teammates
  → 每个 teammate 是独立 Claude Code 实例
  → 共享任务列表（基于文件系统 `~/.claude/tasks/{team-name}/`）
  → Mailbox 消息系统实现 agent 间直接通信
  → Lead 通过 SendMessage tool 管理协作
  → 用户可通过 tmux split-pane 或 in-process 面板查看所有 teammate
```

| 属性 | 表现 |
|------|------|
| 调度方式 | LLM 自动或用户显式要求 spawn teammate |
| 是否阻塞 | ❌ 不阻塞（对等协作,非父→子委派） |
| 子↔子通信 | ✅ **Mailbox 消息系统** |
| 父↔子双向 | ✅ Lead 和 teammate 可互发消息 |
| 用户↔任意 agent | ✅ 可 attach 到任意 teammate |
| Context | 每个 teammate 独立 context window |
| 文件隔离 | ❌ 不自动隔离（需用户自行分区文件所有权） |
| 任务协调 | ✅ 共享任务列表 + 文件锁防 race |
| 质量门禁 | ✅ hooks: TeammateIdle / TaskCreated / TaskCompleted |

**架构组件**：

```
┌──────────────────────────────────────────────┐
│  Lead (主 session)                            │
│  - 负责任务拆分、分配、综合                    │
│  - 通过 SendMessage 与 teammates 通信         │
│  - 协调任务依赖关系                           │
├──────────────────────────────────────────────┤
│  Teammate A (独立 session)                    │
│  - 认领任务,独立工作                          │
│  - 向 lead 和其他 teammate 发送消息           │
├──────────────────────────────────────────────┤
│  Teammate B (独立 session)                    │
│  - ...                                       │
├──────────────────────────────────────────────┤
│  共享基础设施                                 │
│  - Task list (文件系统)                       │
│  - Mailbox (消息系统)                         │
│  - Team config (~/.claude/teams/{name}/)     │
└──────────────────────────────────────────────┘
```

**关键限制**：
- 不能对同一个文件并行编辑（需手动分区文件所有权）
- 不允许嵌套 team（teammate 不能再 spawn teammate）
- 每个 teammate 是独立 Claude Code 实例,token 消耗显著增加

### 2.5 Dynamic Workflows — 脚本编排

```
/deep-research <question>
  → Claude 写一个 JS 脚本
  → runtime 在后台执行该脚本
  → 脚本负责：fan-out agent → 收集结果 → 交叉验证 → 合成报告
  → 用户通过 /workflows 实时查看进度
```

| 属性 | 表现 |
|------|------|
| 编排持有者 | **JS 脚本**（而非 LLM turn-by-turn 决策） |
| 并发上限 | 最多 16 并发 agent / 1000 agent 总上限 |
| 是否阻塞 | ❌ 后台执行,session 保持响应 |
| 实时通信 | ✅ `/workflows` 查看阶段进度 + 各 agent token 用量 |
| 中间结果存在哪 | **脚本变量**（非 LLM context window） |
| 可重复性 | ✅ 脚本可保存为命令,可 rerun |
| 适用规模 | 数十到数百 agent |

**与 subagent 的关键区别**：编排逻辑从 LLM context 移到脚本代码,结果存在脚本变量而非 LLM 上下文,因此可以扩展到远超单个 context window 能容纳的规模。

### 2.6 CC 各层的选择树

```
你的任务需要并行工作吗？
├── 需要快速委派子任务,只关心结果
│   └── Subagent (Task tool) — 同步阻塞,context 隔离
├── 想探索不同方向,保留主对话
│   └── Forked Subagent — 不阻塞,继承 context
├── 有几个独立任务想放手让 Claude 跑
│   └── Agent View — 后台 session,可随时检查/介入
├── 子任务之间需要讨论和协调
│   └── Agent Teams 🧪 — 对等协作,Mailbox 通信
└── 规模太大,需要脚本化编排
    └── Dynamic Workflows — JS 脚本,后台 runtime
```

---

## 3. Hermes（Nous Research）— 最成熟的并行 fan-out

> **⚠️ ERRATA（2026-06-24 源码核实裁定）**: 本节有 2 处事实错误,已在抓取 `NousResearch/hermes-agent` `tools/delegate_tool.py` 源码后裁定,选型以本裁定为准:
> 1. **Hermes 默认同步阻塞**,不是"不阻塞父 agent"。`background` 参数默认 `False`（前台阻塞,父 agent 等所有 child 完成）;只有显式 `background=true` 才异步返回 `{"status":"dispatched"}` 让父 agent 继续。源码 `delegate_tool.py:2120` + `async_delegation.py:263` 注释 "results block"。
> 2. **并发上限默认 3,不是 30**。`_DEFAULT_MAX_CONCURRENT_CHILDREN = 3`（`delegate_tool.py:132`）。下方 yaml 的 `30` 是示例值,易误读为默认。批超限**硬拒**（返回 tool_error,`:2174-2180`,不截断不排队）。`max_spawn_depth` 默认也是 **1**（`MAX_DEPTH=1`,`:139`,默认禁嵌套;下方 `2` 同为示例值）。
> 3. **结论**:Hermes 的"并行"是 `ThreadPoolExecutor` 内部把 N 个 child 跑满、**父 agent 仍阻塞等齐**——这正是 Everlasting L3a 该抄的形态（父 turn 阻塞 + 内部 fan-out）,不是"父 agent 不阻塞"。
>
> **2026-06-24 L3a 范围决策**:L3a 限定**只读 worker 并发**（researcher/探索类）,worktree 隔离留 L3b（带写 worker 才需要）。本文 §6.1 方案 A 建议的"并发 + worktree"已被范围决策收窄为"并发只读、不含 worktree"。

> 信息源：2026-06-19 调研 + hermes-agent.nousresearch.com 文档

### 3.1 delegate_task — 并行 fan-out

```python
delegate_task(tasks=[
    {"goal": "Research topic A", "toolsets": ["web"]},
    {"goal": "Research topic B", "toolsets": ["web"]},
    {"goal": "Fix the build", "toolsets": ["terminal", "file"]},
])
```

配置：
```yaml
delegation:
  max_concurrent_children: 30   # 示例值;源码默认 3(见 §3 ERRATA),可配高
  max_spawn_depth: 2            # 示例值;源码默认 1(禁嵌套),可配高
```

| 属性 | 表现 |
|------|------|
| 是否阻塞 | ⚠️ **默认同步阻塞**（父 agent 等所有 child 完成）;`background=true` 才不阻塞（见 §3 ERRATA,本文原文误为"不阻塞"） |
| 并发机制 | 配置并发上限,子 agent 并行独立运行 |
| 子→父实时流 | ❌ 只拿最终结果 |
| 子↔子通信 | ❌ 无 |
| Context 隔离 | ✅ 各自独立 context |
| 文件隔离 | ✅ **git worktree** 是硬前提 |

### 3.2 Kanban 任务图编排

```
kanban_create(title="research ICP funding, NA angle", assignee="researcher-a")  # t_r1
kanban_create(title="research ICP funding, EU angle", assignee="researcher-b")  # t_r2
kanban_create(
    title="synthesize into launch post draft",
    assignee="writer",
    parents=["t_r1", "t_r2"],   # 两者都完成后才 ready
)
```

- **依赖图管理**：`parents` 声明前置依赖,自动阻塞等待
- **进度上报**：`kanban_heartbeat` 让 worker agent 定时上报进度
- **结果汇总**：`kanban_complete` 触发 synthesis 阶段

**关键的实时通信局限**：`kanban_heartbeat` 是"上报到 kanban 系统"而不是"直接发给父 agent"。父 agent 在子运行时不做等待,最终通过 synthesis 步骤收集所有结果。本质上仍是**最终一致性**而非**实时流**。

### 3.3 与 Everlasting 的对比

| 维度 | Hermes | Everlasting 当前 |
|------|--------|-----------------|
| 并发数 | 源码默认 3（可配高,示例常写 30;见 §3 ERRATA） | 单 worker 串行 |
| Worktree 隔离 | 强制要求 | ✅ 已有模块(`git/worktree.rs`) |
| 任务依赖图 | Kanban + parents | ❌ 无 |
| 结果合并 | synthesis 步骤 | format_dispatch_result |
| 进度上报 | kanban_heartbeat | subagent:event（仅前端可见） |

---

## 4. 框架级别的并行模式

### 4.1 LangGraph `Send` — 图拓扑驱动的 fan-out

```python
def continue_to_jokes(state: State):
    return [Send("generate_joke", {"topic": t}) for t in state["topics"]]
```

| 属性 | 表现 |
|------|------|
| 并行粒度 | 图节点级（并非完整 agent loop） |
| 通信方式 | **共享 State 对象**（TypedDict） |
| 是否阻塞 | 框架管理,图节点并行执行后汇集 |
| 实时通信 | 通过 State 读写隐式完成 |
| 适用场景 | 有明确 DAG 结构的任务流程 |

**与本项目的关联**：LangGraph 的模式说明,"共享 State" 是 agent 间通信的最简形式——不需要消息总线,不需要 mailbox,只需要一个所有 agent 都能读写的结构化数据对象。

### 4.2 AutoGen（Microsoft）— AgentRuntime 消息总线

```python
# AssistantAgent 默认并行 tool calls
assistant = AssistantAgent(
    "assistant",
    llm_config={"config_list": [{"model": "gpt-4", ...}]},
    # 使用 AgentTool 时必须 parallel_tool_calls=False
)
```

| 属性 | 表现 |
|------|------|
| Agent 间通信 | **AgentRuntime message queue**（publish/subscribe） |
| 默认并行 | ✅ `AssistantAgent` 默认并行 tool calls |
| 限制 | 使用 `AgentTool` 或 `TeamTool` 时**必须**关闭并行 |
| 核心洞见 | 并发 + 共享可变状态是根本矛盾 |

**AutoGen 的核心贡献**：它明确指出了 "stateful agent 不能并行" 的工程约束——这对 L3a 的 worktree 隔离策略是直接佐证（worktree 解决了"共享状态"问题,让并行编辑不同文件成为可能）。

### 4.3 CrewAI — 串行/层次化 Process

- Tool 支持 async,但那是**单 tool 内部异步 I/O**,不是多 tool 并行
- Process 模型只有 **Sequential**（任务按序）或 **Hierarchical**（manager 委派）
- 不支持 agent 级并行执行

---

## 5. 实时通信 — 全景分类

### 5.1 四种通信模式

| 模式 | 代表 | 机制 | 适用场景 |
|------|------|------|---------|
| **① 无通信**（最终结果） | CC Subagent, Hermes, Everlasting | tool_use/tool_result,同步阻塞或异步等完成 | 子任务只产出"答案",不产出"过程" |
| **② 单向进度通知** | CC Agent View, Hermes Kanban | peek 面板 / heartbeat 上报 / IPC 事件 | 用户想看进度,但 agent 不需要 |
| **③ 双向消息传递** | CC Agent Teams | Mailbox + SendMessage tool | 对等 agent 需要讨论、挑战、协调 |
| **④ 共享 State** | LangGraph, CC Workflows | 脚本变量 / TypedDict State | 编排层持有数据,agent 只读写 |

### 5.2 关键发现：父 LLM 不需要实时通信

所有调研的方案中,**没有任何一个让父 LLM 在 subagent 运行时实时看到中间结果**。原因是本质性的：

- **如果父 LLM 能看到中间结果** → 这些结果必须注入父的 context window → subagent 的 context 隔离被打破 → subagent 失去存在意义
- **subagent 的 ROI 公式**：减少的 context 污染量 - 编排开销 > 0

所以方案是：
- **用户想看进度** → 做 IPC 事件 + UI（✅ Everlasting 已有 `subagent:event`）
- **父 LLM 只需要最终结果** → tool_result 回传（✅ 当前模式）
- **如果父 agent 需要协作** → 那已经超出 "subagent" 范畴,进入 "多 agent 团队协作"领域

---

## 6. L3a 设计建议

### 6.1 三个候选方案

#### 方案 A：并发 fan-out + worktree（仿 Hermes）⭐ 推荐

```
父 LLM 一次派发 N 个 dispatch_subagent tool_use
  → run_chat_loop 检测到 N>1,并行 spawn
  → 每个 subagent 在独立 worktree 中运行
  → 使用 FuturesUnordered / JoinSet 管理并发
  → 全部完成后,结果合并注入
```

| 维度 | 评估 |
|------|------|
| 改动范围 | 中（改 chat_loop.rs 串行→并行 + worktree 集成） |
| 通信需求 | ❌ 无（保持 tool_result 回传） |
| 文件隔离 | ✅ worktree 已就位 |
| 兼容性 | 与现有权限/审计/取消机制兼容 |
| 风险 | 低（并行执行独立 subagent 无共享状态） |

#### 方案 B：后台 subagent session（仿 CC Agent View / Hermes）

```
dispatch_subagent 改为立即返回 { session_id, status: "running" }
  → subagent 在跨 request 常驻态中运行（AppState 持有）
  → 父 agent 通过 check_subagent(session_id) tool 查进度
  → 完成时通过系统通知注入告知父 agent
```

| 维度 | 评估 |
|------|------|
| 改动范围 | 大（需常驻态 session 管理 + 跨 request 生命周期 + 通知注入机制） |
| 通信需求 | 需父 agent 主动轮询或被动接收通知 |
| 依赖 | daemon 化（或至少 GUI 进程内常驻态） |
| 风险 | 高（与 daemon 化强耦合,写出的中间态可能被推翻） |

#### 方案 C：并发 fan-out + 共享 State（仿 LangGraph）

```
父 LLM 派发 N 个 subagent,传递一个共享 State 对象
  → 每个 subagent 可以读写 State
  → 父 agent 在每轮 loop 中检查 State 更新
  → 所有 subagent 完成后,synthesis 读取最终 State
```

| 维度 | 评估 |
|------|------|
| 改动范围 | 大（需 State 定义 + 序列化 + 并发安全访问） |
| 通信需求 | 共享 State 读写在 subagent 内完成 |
| 复杂度 | 高（State schema 管理 + 版本兼容 + 合并策略） |
| 收益 | 在 L3a 阶段不明确（大多数场景最终结果即可） |

### 6.2 推荐路径

```
第一优先: 方案 A（并发 fan-out + worktree）
  → 复用现有 run_chat_loop 递归结构
  → 只改串行→并行 + worktree 挂载/卸载
  → 保持 tool_result 回传,不做实时通信
  → 最小化改动,最大化学习价值

第二优先: 方案 B 的子集（不阻塞 dispatch）
  → 需要 daemon 化或常驻态基础设施
  → 建议与 daemon 化一并规划
  → 解决 "父 agent 在等待 subagent 时不能做其他事" 的问题

暂缓: 方案 C（共享 State）
  → 复杂度高,收益不明确
  → 如果后续需要 agent 间细粒度协作可重新评估
```

### 6.3 不建议做的

- ❌ **父 LLM 实时流中间结果**——破坏 subagent 的 context 隔离核心价值
- ❌ **Subagent 间直接通信**——在 L3a 阶段过度设计,协调开销 > 收益
- ❌ **依赖 daemon 化的方案**——daemon 化尚未立项,先做方案 A 不走 daemon 路径

---

## 7. 出处

### 官方文档
- Claude Code Sub-agents: `https://docs.anthropic.com/en/docs/claude-code/sub-agents`
- Claude Code Agent View: `https://docs.anthropic.com/en/docs/claude-code/agent-view`
- Claude Code Agent Teams: `https://docs.anthropic.com/en/docs/claude-code/agent-teams`
- Claude Code Dynamic Workflows: `https://docs.anthropic.com/en/docs/claude-code/workflows`
- Claude Code Run agents in parallel (overview): `https://docs.anthropic.com/en/docs/claude-code/agents`
- Claude Code Agent SDK Subagents: `https://docs.anthropic.com/en/docs/agent-sdk/subagents`

### 框架文档
- LangGraph: `https://langchain-ai.github.io/langgraph/concepts/high_level/`
- AutoGen: `https://microsoft.github.io/autogen/`

### 过往调研（同一仓库,本调研的参照基线）
- `docs/spikes/2026-06-19-async-parallel-tool-research.md` — opencode/Hermes + L1/L2/L3 三层模型
- `docs/spikes/2026-06-19-async-parallel-tool-independent-research.md` — CC/Aider/Cline/Goose/Continue + frameworks + academic

### 本调研源码参考
- Everlasting: `app/src-tauri/src/agent/subagent/`（mod.rs / dispatch.rs / sink.rs / transcript.rs）
- Everlasting: `app/src-tauri/src/agent/chat_loop.rs`（L1685-1783 拦截器）
- Everlasting: `app/src-tauri/src/git/worktree.rs`（worktree 隔离基础设施,已就位）

---

## 8. 关联

- **与 [ROADMAP.md §2 L3a](../ROADMAP.md#2-v2-路线图分类2026-06-10-重排2026-06-13-收尾更新) 的关系**：本调研是 L3a 设计的前置步骤（ROADMAP 自述 "Plan 阶段先做行业 subagent 通讯机制调研"）,为并发模型设计提供行业参照
- **与 L2（并行 tool 执行）的关系**：L3a 是 L2 在 agent 级别的扩展——L2 做单 turn 多 tool 并行（只读 batch）,L3a 做多 subagent 并行（独立 context + worktree）
- **与 daemon 化路线的关系**：方案 B 依赖 daemon 化,方案 A 不需要。建议 L3a 先走方案 A,daemon 化作为独立 PM 项推进
- **与 B6（subagent 基础实现）的关系**：L3a 依赖 B6 落地（当前已有 `dispatch_subagent` + `subagent_runs` 持久化 + SubagentDrawer）——B6 已完成,可以开始 L3a
- **与 C4（审计日志）的关系**：并行 subagent 的审计日志需要处理竞态——建议 L3a 沿用现有 AuditKind,不新增并行专用审计类型
