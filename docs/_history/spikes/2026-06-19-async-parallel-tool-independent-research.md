# 调研: coding agent 异步/并行 tool 执行 — 独立实证与设计模式

**日期**: 2026-06-19
**状态**: 调研完成
**范围**: 本次独立调研覆盖 Claude Code / Aider / Cline / Goose / Continue 的并行机制 + Anthropic/OpenAI 协议层能力 + LangGraph/AutoGen/CrewAI 框架模式 + 学术文献 + 已知失效模式。与同日 `2026-06-19-async-parallel-tool-research.md`(覆盖 opencode/Hermes)互补,两份报告合在一起形成完整行业全景。

---

## TL;DR

1. **协议层早已支持并行**: Anthropic 和 OpenAI 的 tool-use API **原生返回多个 tool_use/tool_calls**,但几乎所有 agent harness(除 Cline)都选择**串行执行**。这不是"做不到",是工程权衡。
2. **Claude Code 是功能最全的**: 它有 4 层并行: API 级并发 tool_use、Subagent(task 委派)、Agent View(独立 session 并行)、Agent Teams(多 agent 协作 + 消息通信)。不是单纯"同步 agent"。
3. **Aider 和 Goose 代表另一极**: Aider 用 edit format(非 tool-use API)天然批量;Goose 有人工确认门控所以无法并行。两者都是**合理的设计取舍**,不是"缺失"。
4. **大多数 agent framework 不默认并行**: 5 个被调研 agent 中仅 Cline 有 opt-in 并行开关。原因不是能力不够,是工程成本(共享状态踩踏、审计复杂、编排开销、调试困难)在大多数任务上超过收益。
5. **对本项目的建议**: 优先 L2(单 turn 多 tool 并发),这是纯收益零破坏的改动;L1(后台 shell)次之,直接解决当前 shell timeout 痛点;L3(并行 subagent)是旗舰特性,但需要完整的 worktree 隔离 + context 合并 + 审计重设计,成本最高。

---

## 1. 协议层: LLM API 原生支持并行 tool call

### 1.1 Anthropic (Claude)

Claude **默认启用并行 tool use**。一次响应中可以包含多个 `tool_use` block,`stop_reason` 为 `"tool_use"`。

**关键约束**(来自官方文档 [parallel-tool-use](https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/parallel-tool-use)):

| 约束 | 说明 |
|---|---|
| 结果必须单消息返回 | 所有 `tool_result` block 必须在**同一个 user message** 中返回;拆成多个消息会导致 Claude "学会"避免并行 |
| 执行顺序由调用方决定 | API 不规定执行顺序 —— 你可以 `Promise.all` 并发,也可以串行,也可以混合 |
| `disable_parallel_tool_use: true` | 可显式关闭并行。配合 `tool_choice: "auto"` 最多返回一个;配合 `"any"` 保证返回恰好一个 |
| Claude 4 模型尤其强 | 官方声明 Claude 4 "have excellent parallel tool use capabilities by default" |
| system prompt 可引导 | 官方推荐 system prompt 加入: *"For maximum efficiency, whenever you perform multiple independent operations, invoke all relevant tools simultaneously."* |

**服务器端 tool**(`web_search`, `code_execution`, `web_fetch`)有自己的内部循环,在 Anthropic 基础设施上迭代多次后才返回最终结果 —— 应用程序完全不处理中间的 `tool_result` block。

### 1.2 OpenAI

OpenAI Chat Completions API 同样支持多 `tool_calls` 在单个响应中:

- 由 `parallel_tool_calls` (boolean) 控制
- 模型自行决定何时批量化独立调用
- Cookbook 展示同时查询两个城市天气的模式
- 和 Anthropic 一样,**执行策略完全由调用方决定**

### 1.3 协议层 vs 应用层的 gap

**核心发现**: API 层早已具备并行能力,但几乎所有 agent harness 都没有暴露这个能力。原因见 §4。

---

## 2. 行业 agent 逐一剖析

### 2.1 Claude Code (Anthropic 官方) — 4 层并行模型

Claude Code **不是单纯的同步 agent**。它有完整的分层并行架构:

| 层级 | 机制 | 并行粒度 | 通信方式 | 文件隔离 |
|---|---|---|---|---|
| **API 级** | 多 `tool_use` 并发 | 单个 turn 内的多个 tool | 共享 context | 无(同一 session) |
| **Subagent** | `Task` tool 委派 | 子任务级 | 结果回报主 agent | 可选 worktree |
| **Agent View** | 多个独立 session | session 级 | 各自独立;supervisor 进程管理 | 各自 worktree |
| **Agent Teams** | 多 agent 协作(experimental) | agent 级 | 直接消息通信 + 共享任务列表 | 各自 worktree |

**Claude Code 不是"对标"对象 —— 它本身就是分层并行最完整的 agent 之一。**

额外机制:
- **Background bash**: 在后台跑 shell 命令,不阻塞对话
- **Forked subagent**: 继承完整对话 context 的 subagent
- **Dynamic Workflows**: 脚本编排,最多 16 并发 agent,单次运行最多 1000 个
- **Routines**: 定时循环 session(Anthropic 云端运行,非本机并行)

### 2.2 Cline / Roo Code — 唯一有 opt-in 并行的 agent

**源码位置**: `sdk/packages/agents/src/agent-runtime.ts`

```typescript
// AgentRuntimeConfig
toolExecution: "sequential" | "parallel"  // 默认 "sequential"

// executeToolCalls()
if (this.config.toolExecution === "parallel") {
    await Promise.all(prepared.map(exec => exec()));
} else {
    for (const exec of prepared) { await exec(); }
}
```

**设计思路**:
- 默认串行(安全优先)
- `"parallel"` 模式: 用户显式 opt-in,适合"我知道这些 tool 之间没有依赖"的场景
- 另有一个 **Kanban 产品**,从任务面板并行跑多个 agent,每个有独立 worktree

**评估**: 这是最务实的方案 —— 不替用户做决定,把选择权交给用户。

### 2.3 Aider — 根本不走 tool-use 路径

Aider 的"并行"概念完全不同:

- 不使用 tool-use API。LLM 输出的是 markdown fenced 的 structured text(edit format),不是 JSON tool call
- `editblock_func_coder.py` 把多个文件编辑打包进**单个函数调用**的数组参数中
- 在 `_update_files()` 中**串行应用**这些编辑

**源码**: `aider/coders/editblock_func_coder.py:85-112`

**评估**: Aider 的 edit format 本质是"单次 LLM 响应内批量编辑",不涉及并行 tool 执行。这是编辑格式(edit format)路线的固有特征 —— 一切都被塞进一个 text block,没有 tool_use 的概念。

### 2.4 Goose (Block → Agentic AI Foundation) — 人工确认门控阻止了并行

**源码位置**: `crates/goose/src/agents/tool_execution.rs`

```rust
// handle_approval_tool_requests()
for request in tool_requests.iter() {
    // inspect → confirm → dispatch → result
    // 每个 tool 等待人工确认后才能执行
}
```

**为什么 Goose 不并行**: Goose 的核心设计理念是**人工审批每一个 tool 调用**。tool 执行管线是: inspection → confirmation routing → dispatch → result collection。每个 tool 都要等人点"确认",所以 `for` 循环串行是唯一合理的选择。

Goose 内部的 `tokio::spawn` 和 `futures::stream` 用于 **intra-tool 异步**(单个 tool 内部的 I/O 并发,MCP 通信等),不是 inter-tool 并行。

**评估**: Goose 的选择是设计哲学驱动的,不是技术限制。在"人必须审批每个 tool"的前提下,并行没有意义。

### 2.5 Continue.dev — 已停止维护

Continue 的 `multiEdit` tool 和 Aider 类似,将多个编辑打包进单次调用。agent loop 是串行的。仓库已于 2025 年中声明只读。

---

## 3. Agent 框架的并行模式

### 3.1 LangGraph / LangChain

- **LangGraph** 原生支持图节点并行(fan-out)。新的 `create_agent`(LangChain)底层用 LangGraph,继承并行能力
- 并行 tool 执行是图拓扑的自然结果: 无依赖的节点可以同时跑
- LangChain tool-calling 文档将并行 function calling 描述为**模型级 feature**(模型返回多个 calls,agent loop 执行它们)

### 3.2 AutoGen (Microsoft)

- `AssistantAgent` **默认并行 tool calls**: "if the model client produces multiple tool calls, AssistantAgent will call the tools in parallel"
- **但有限制**: 使用 `AgentTool` 或 `TeamTool` 时**必须**关闭并行(`parallel_tool_calls=False`),因为这些持有内部状态的 agent/team 不能并发
- 支持 `max_tool_iterations` 控制多步 tool loop 深度

**评估**: AutoGen 是唯一**默认并行**的框架。但它的"并行 agent tool"限制正好说明了并发 + 共享状态的根本矛盾。

### 3.3 CrewAI

- Tool 有 async 支持(`async def _run`),但这是**单 tool 内部异步**,不是多 tool 并行
- Process 模型是 **Sequential**(任务按序)或 **Hierarchical**(manager 委派),没有并行 agent 执行
- 并行仅限于单 agent 内的异步 I/O

---

## 4. 为什么大多数 agent 选择串行 — 工程成本分析

这不是"做不到",是**有真实成本**,且大多数场景下成本超过收益:

### 4.1 共享状态踩踏(最根本的原因)

并行 tool 写同一个文件/目录 = 互相覆盖。解决方案是 `git worktree` 隔离(Hermes 的做法),但这引入了:
- worktree 创建/销毁的开销
- 跨 worktree 的 diff/merge 复杂度
- 多 worktree 的磁盘占用

**这就是为什么 Hermes 花大量力气做 worktree 隔离 —— 它不是锦上添花,是并行的硬前提。**

### 4.2 编排/合并开销

拆任务、分配、收集、综合本身消耗 token 和推理。如果任务太小(比如并行 3 个 `read_file` 各 50 行),编排开销可能超过收益。

### 4.3 Audit 复杂度

并行执行引入竞态,审计日志的顺序不再等于因果顺序。对 Everlasting 这种有 10 类 `AuditKind` 的系统,并行会显著增加审计实现复杂度。

### 4.4 调试/Replay

串行 agent 的 session 可以在任何 turn 精确 replay。并行 agent 的竞态取决于 OS 调度,replay 不可复现。

### 4.5 收益依赖任务可分性

| 任务类型 | 并行收益 |
|---|---|
| 读多个独立文件 | ✅ 高 |
| 独立调研多个 topic | ✅ 高 |
| 同时跑 build + 写代码 | ✅ 中(需隔离) |
| 重构 A 模块同时写 B 模块测试 | ✅ 中(L3) |
| 修改一个文件的多个位置 | ❌ 低(强依赖) |
| 连续 bug 修复链条 | ❌ 低(依赖前一步结果) |

### 4.6 人工审批门控

Goose 的设计说明:如果每个 tool 都需要人点确认,并行没有意义。这是**交互模式决定执行模型**的经典案例。

**结论**: 这不是"该不该做"的问题,而是"在什么场景下做哪一层"的问题。Hermes / Claude Code / Cline 判定某些场景值得,就做了相应的层。

---

## 5. 学术视角: 多 agent 并行 ≠ 单 turn 并行

学术文献更关注**多 agent 协作**而非单 agent 的并行 tool 执行:

- **MetaGPT** (arXiv:2308.00352): SOP 编码的流水线并行 —— ProductManager/Architect/Engineer/QA 在结构化工作流中协作。是 pipeline-parallel,不是 concurrent-execution。
- **AutoGen** (arXiv:2308.08155): 多 agent 对话框架,底层支持并行 agent 执行。广泛用于代码生成但不是代码生成专用。
- **"More Agents Is All You Need"** (arXiv:2402.05120, TMLR 2024): 证明 LLM 性能随 agent 数量(sampling-and-voting)提升。正交于实现方法,可叠加到任何 agent 上。

**关键 gap**: 没有论文专门研究"单 agent turn 内的多 tool 并行执行" —— 这个领域完全由工程实践驱动,学术界还没跟上。

---

## 6. 已知失效模式(补充已有调研)

已有调研(同日 `async-parallel-tool-research.md`)覆盖了 5 类失效模式。以下补充**API 协议级**的已知陷阱:

### 6.1 错误的结果消息格式(Anthropic 特定)

**症状**: 把每个 `tool_result` 放在单独的 user message 中返回 → Claude 逐渐停止并行 tool 调用。

**原因**: Claude 的并行 tool use 训练数据中,多个 tool_result 总是在同一个 user message 中。拆分消息被视为"不想要并行"的信号。

**修复**: 始终将所有 tool_result block 打包进单个 user message(本项目 `chat_loop.rs:1133-1136` 已经做对了)。

### 6.2 依赖链断裂

**症状**: 并行 batch 中 tool B 依赖 tool A 的输出,但没有声明依赖 → B 执行失败或产生错误结果。

**出现条件**: LLM 不清楚 tool 之间的隐式依赖(比如 "read A 文件内容,然后 edit A 文件" —— LLM 可能同时发出 read 和 edit 的 tool_use,但 edit 不知道 A 文件内容)。

**修复策略**:
1. **保守策略**(Cline 默认): 始终串行,避免依赖问题
2. **启发式策略**: 分析 tool name + input,检测到同一文件路径的 read+write 同批 → 串行化
3. **显式依赖**(理想但复杂): tool schema 中增加 `depends_on: [tool_use_id]` 字段

### 6.3 并发 tool 与 stateful agent 冲突(AutoGen 特定)

**症状**: `AgentTool` 或 `TeamTool` 并发执行导致 agent 内部状态损坏。

**修复**: AutoGen 文档明确要求这种情况下 `parallel_tool_calls=False`。这是并发 + 共享可变状态的经典问题,不仅限于 agent —— 任何有状态的 tool 都不能并发。

### 6.4 超时放大

**症状**: 串行时 3 个 tool 各 10s = 30s 总延迟。并行时 3 个 tool 各 10s = 10s 总延迟 ✅。但如果取消发生在 5s 时,串行只有一个 tool 被取消,并行三个 tool **都**要被取消(3x 清理开销)。

**本项目相关**: `chat_loop.rs:1055` 的 `token.is_cancelled()` 检查在串行模式下只影响"下一个 tool 是否执行"。如果改成并行,需要同时取消**所有正在运行的 tool**(本项目 shell tool 的 RULE-E-002 process-group kill 已经支持取消,但其他 tool 没有 cancel 感知)。

---

## 7. 对本项目的独立评估

### 7.1 当前状态确认

与已有调研(`async-parallel-tool-research.md §3`)一致,补充源码细节:

| 位置 | 内容 |
|---|---|
| `agent/chat_loop.rs:994-1089` | `for (id, name, input) in &tool_calls { execute_tool().await }` — 纯串行,无并发 |
| `tools/mod.rs:121-129` | `execute_tool()` 返回 `(content, is_error, ctx_update, exit_code)` — 同步语义 |
| `tools/mod.rs:134-143` | cancel wrapper: 每个 tool 调用前有 `tokio::select!` cancel check |
| `agent/mod.rs:51` | `MAX_TURNS = 50` — 硬上限 |
| `git/worktree.rs` | 完整 worktree 生命周期(create/destroy/self-heal/diff) — **已就位** |
| `agent/` 全目录 | **无任何 subagent/task/delegation 概念** — grep 零匹配 |

### 7.2 独立判断: 三层推进的优先级

与已有调研的"L2 → L1 → L3"推荐一致,但理由有补充:

#### L2: 单 turn 多 tool 并发 — **建议立即做**

理由(补充已有调研):
- **改动极小**: 仅需修改 `chat_loop.rs` 中的 tool 执行 loop,从 `for` 串行改为 `futures::join_all` 或 `FuturesUnordered`
- **纯收益**: 读 3 个文件的耗时从 `t1+t2+t3` 变为 `max(t1,t2,t3)`
- **LLM 已经准备好了**: Anthropic API 默认返回多 tool_use;本项目用的是 Anthropic API,不需要额外引导
- **风险可控**: 10 个 tool 中,7 个是只读(read_file/grep/glob/list_dir/web_fetch/use_skill/update_checklist),天然无冲突
- **write_file/edit_file/shell 需要特殊处理**: 如果某 batch 混合了读写 tool,需要启发式判断(同一文件路径的 read+write → 串行)或保守策略(批量中只要有写 tool 就全部串行)

**建议实现策略**: 
- 第一步: 只并行纯只读 tool batch
- 第二步: 引入启发式 write-conflict 检测
- 第三步: 用户可配 `parallel_tools: "auto" | "never" | "always"`

#### L1: 后台 shell + 完成通知 — **建议近期做**

理由(补充已有调研):
- 直接解决当前 shell `timeout` 痛点(120s 硬上限不够用)
- 借鉴 opencode-pty 的 `<pty_exited>` 注入模式,不改 LLM tool 协议
- 本项目 shell tool 已有 process-group kill(RULE-E-002)、env isolation(RULE-E-001)、disk spill——后台化只需要在 tool 层加一个"立即返回 session_id + tokio::spawn + 完成注入"的 wrapping
- 与 daemon 化路线天然契合

**依赖**: 需要一个"将 system message 注入到 agent loop 的下一轮"的机制。当前 `chat_loop.rs` 的 turn 结构是纯函数式的,不支持外部注入。需要:
1. 在 `AppState` 或 `SessionState` 中维护一个 `pending_notifications: Vec<String>`
2. Agent loop 每轮开头消费这些通知并注入 context

#### L3: 并行 subagent + worktree 隔离 — **旗舰特性,建议规划但不急于实现**

理由(补充已有调研):
- 最有学习价值的 harness 工程难题
- 本项目的 `git/worktree.rs` 已经是 production-grade,门槛比 Hermes 当初低
- 但需要的前置工作量大: context 合并/压缩、多 session 管理、审计重设计、subagent 生命周期
- Claude Code 的 Agent Teams 还在 experimental,说明即使是 Anthropic 也认为这层还没稳定

---

## 8. 两个调研的差异与互补

| 维度 | 已有调研(`async-parallel-tool-research.md`) | 本调研 |
|---|---|---|
| **覆盖 agent** | opencode, Hermes | Claude Code, Aider, Cline, Goose, Continue |
| **协议层深度** | 未涉及 | Anthropic/OpenAI 原生并行 tool use 协议详解 |
| **框架覆盖** | 无 | LangGraph, AutoGen, CrewAI |
| **学术视角** | 无 | MetaGPT, AutoGen 论文, "More Agents Is All You Need" |
| **失效模式** | 5 类(工程层面) | 补充 4 类(协议/依赖/stateful/超时放大) |
| **本项目评估** | 现状 + 推荐路径 | 现状(源码细节) + 实现策略建议 |
| **信息源** | context7 文档检索 | web 搜索 + 源码阅读(Live GitHub) |

**建议**: 两份报告合在一起覆盖了当前 coding agent 生态的完整并行/异步全景。后续决策时两份应并读。

---

## 9. 出处

### API 协议文档
- Anthropic Parallel Tool Use: `https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/parallel-tool-use`
- Anthropic How Tool Use Works: `https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/how-tool-use-works`
- OpenAI Parallel Function Calling: `https://cookbook.openai.com/examples/how_to_call_functions_with_chat_models`

### Claude Code
- Sub-agents: `https://docs.anthropic.com/en/docs/claude-code/sub-agents`
- Agent View: `https://docs.anthropic.com/en/docs/claude-code/agent-view`
- Agent Teams: `https://docs.anthropic.com/en/docs/claude-code/agent-teams`
- Workflows: `https://docs.anthropic.com/en/docs/claude-code/workflows`
- Agents overview: `https://docs.anthropic.com/en/docs/claude-code/agents`

### 源码出处
- Aider: `github.com/paul-gauthier/aider` — `aider/coders/editblock_func_coder.py:85-112`
- Cline: `github.com/cline/cline` — `sdk/packages/agents/src/agent-runtime.ts`
- Goose: `github.com/block/goose` — `crates/goose/src/agents/tool_execution.rs`
- Continue: `github.com/continuedev/continue` (read-only since mid-2025)
- Everlasting: 本项目 `app/src-tauri/src/agent/chat_loop.rs`, `tools/mod.rs`, `tools/shell.rs`, `git/worktree.rs`

### 框架文档
- LangChain Tool Calling: `https://python.langchain.com/docs/concepts/tool_calling/`
- AutoGen Parallel Tool Calls: `https://microsoft.github.io/autogen/stable/user-guide/agentchat-user-guide/tutorial/agents.html`
- CrewAI Tools: `https://docs.crewai.com/concepts/tools`

### 学术
- MetaGPT: `arXiv:2308.00352`
- AutoGen: `arXiv:2308.08155`
- "More Agents Is All You Need": `arXiv:2402.05120` (TMLR 2024)

---

## 10. 关联

- **与同日 `async-parallel-tool-research.md` 的关系**: 互补。本报告覆盖 Claude Code / Aider / Cline / Goose / Continue + 协议层 + 框架 + 学术;已有报告覆盖 opencode / Hermes + 三层模型 + context7 出处。两份合在一起形成完整全景。
- **与 shell timeout 调研**: L1(后台 shell) 正是解决"长任务不该靠调 timeout"的正解。
- **与 daemon 化路线**: L1/L3 天然适合 daemon 化后的多 session 管理。
- **登记建议**: 若决策推进,将 L1/L2/L3 作为候选功能登记到 `docs/BACKLOG.md`(排期归 `docs/ROADMAP.md`)。
