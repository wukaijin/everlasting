# Research: Subagent 通讯机制 — 行业调研

- **Query**: 主流 coding agent / agent framework 的 subagent 通讯模式(同步 vs 异步、并发模型、取消传播、失败可见性、上下文隔离、成本归集),为 Everlasting L3a(subagent 并发 + 不阻塞)的设计提供对照参考
- **Scope**: 外部调研(4 个项目,GitHub 源码 + 官方文档一手 + 二手交叉验证)
- **Date**: 2026-06-24
- **对比对象**:
  1. **Claude Code** `Agent` tool(原 `Task`,v2.1.63 改名)
  2. **OpenHands** `agent-sdk` 的 `TaskTool` + `DelegateTool`(双原语)
  3. **LangGraph** `Send` 原语 + `subgraph` 嵌套(subagent = Send + subgraph)
  4. **Hermes Agent** `delegate_task` 工具(NousResearch/hermes-agent)
- **调研方法**:
  - 派 4 个独立 `general-purpose` / `claude-code-guide` agent 并行抓 GitHub 源码 + 官方文档
  - 4 个原始调研笔记单独落地:`docs/research/subagent-communication-survey.md`(本文是综合);CC / OpenHands / Hermes 三个原始材料嵌在本文里,LangGraph 的材料已经在 `docs/research/langgraph-send-primitive-survey.md`(同目录)
  - 一手优先:GitHub raw 源码逐行 + 官方 docs;二手作交叉验证(DeepWiki、Medium、Substack)
- **关联**:Everlasting L3a 拆自原 L3 子项 1(见 `ROADMAP.md §2 第三档`,2026-06-24 改动);B6 当前 subagent 实现见 `app/src-tauri/src/agent/subagent/`

---

## 0. TL;DR — 4 个项目的核心取舍

1. **Claude Code `Agent`**:最贴近 Everlasting 当前模型(同步 + 单 tool_use / tool_result + final text),但额外提供 `background=true` 异步 + 嵌套子 agent(深度 5)+ fork(全上下文继承)。中间步骤对父 LLM 不可见,**final summary 才进父 LLM 上下文**。

2. **OpenHands `TaskTool` / `DelegateTool`**:**显式双原语**。`TaskTool` 同步阻塞(对应我们 `dispatch_subagent`);`DelegateTool` 两阶段 `spawn` → `delegate`,**真并行**(`ThreadPoolExecutor`,`max_children=5`)。结果 wire shape 是结构化 `TaskObservation{task_id, subagent, status, text}`,失败时 **partial output 保留**(`status="error"`,`text="<reason>\nPartial result:\n<partial>"`)。

3. **LangGraph `Send` + `subgraph`**:`Send` 是**低级 fan-out 消息包**(node+arg+timeout,3 字段),**不是 subagent**;真正 subagent = `Send + subgraph` 嵌套。**BSP 原子 superstep**:任一分支 raise → 整步回滚,无 partial state 写入(成功分支走 checkpoint 恢复)。`TimeoutPolicy` 三字段精细(`run_timeout` 硬墙钟 / `idle_timeout` 静默上限 / `refresh_on=auto|heartbeat`)。

4. **Hermes `delegate_task`**:默认同步,`background=true` 切异步(立即返回 delegation id,结果通过 async completion queue 在之后 turn 进对话)。并发上限 `max_concurrent_children=3`,**树形层级 `max_spawn_depth=2`**,`role=orchestrator` 才允许向下委派。**父打断递归取消所有子孙**(`_interrupt_requested` flag)。失败 wire shape 是 5 状态 enum(`complete`/`timeout`/`error`/`interrupted` + orchestrator 降级)+ `partial summary` 字段(可能 None)。

## 共性观察(跨 4 家)

- **Sync by default, async optional**:CC / Hermes / OpenHands `TaskTool` 都默认同步(LLM turn 阻塞),async 单独开关。
- **Final text only 进父 LLM 上下文**:4 家一致。中间步骤走 UI / 审计 / stream hook,父 LLM 不见。
- **结构化 status + partial output**:OpenHands / Hermes 都用结构化 status enum,失败时**保留 partial summary**——给父 LLM 补偿修复的素材。Everlasting 的 `summarize_worker_tool_actions` 部分做到了,但 wire 是 prefix-string 不是 typed enum。
- **成本按 worker 归集**:OpenHands `usage_to_metrics["task:{id}"]`、Hermes `_child_cost_usd` 拷进父 session counter、CC `/workflows` view 展示 per-agent。Everlasting `subagent_runs.token_usage_json` 做到了,但 streaming fold 进 `sessions.*_total` 延迟几秒(RULE-BackSubagent-002 option i)。
- **父打断递归取消**:Hermes 明文;CC UI 按钮触发;OpenHands `LocalConversation.pause()` 落在 conversation 级,tool 层未确认是否级联。Everlasting 用 `parent_token.child_token()` 已经有这能力,但目前只对 1 个 worker 必要。
- **没有银弹**:LangGraph atomic-superstep(强一致)vs Hermes partial-output(乐观并行)是根本设计取舍——一个保证"全成功或全失败",一个允许"成功的不被失败拖死"。

---

## 1. Communication Pattern

| 项目 | 父 LLM → subagent | subagent → 父 LLM | 中间进度 | 父可见中间事件? |
|---|---|---|---|---|
| **CC Agent** | tool_use `{Agent, prompt, subagent_type, ...}` | tool_result 文本块(final message),resumable 加 `agentId: <id>` trailer | 通过 Agent SDK 的 `parent_tool_use_id` 标记,UI 可见 | 否(父 LLM 看不见);UI 看 stream hook |
| **OpenHands TaskTool** | `TaskAction{prompt, subagent_type, resume, ...}` | `TaskObservation{task_id, subagent, status, text}` | 父 LLM 不可见,UI 看 `LocalConversation` stream | 否 |
| **OpenHands DelegateTool** | 两阶段:`spawn{ids, agent_types}` → `delegate{tasks: {id: task_str}}` | 单个 consolidated string `"Agent <id>: <result>\n..."` | 同上 | 否 |
| **LangGraph Send + subgraph** | `Send("subgraph_node", payload, timeout=...)` from conditional edge | 分支返回 `dict` 状态增量,经 channel + reducer 合并 | `stream_mode="updates"\|"events"` 订阅 | 否(默认);观察钩子可见 |
| **Hermes delegate_task** | tool `{goal, context, toolsets}` 或 `tasks: List[Dict]` 批量 | `List[{task_index, status, summary, error, duration_seconds, tokens}]` | child progress callback 推 TUI/spinner | 否(父 LLM 看不见);TUI 可见 |
| **Everlasting dispatch_subagent**(当前) | tool_use `{subagent, task}` | tool_result string:`[status: completed]\n<text>` 或带 partial actions | `subagent:event` IPC 流到 `<SubagentDrawer>` | 否(父 LLM 看不见);UI 可见 |

**Takeaway**:4 家在 "父 LLM 只看终态文本" 上**完全一致**;中间进度走 **UI / 审计** 分发,**不**进 LLM 上下文。Everlasting 已经做到。

---

## 2. Concurrency Model

| 项目 | 并行机制 | 上限 | 触发 |
|---|---|---|---|
| **CC Agent** | foreground blocking / background concurrent,parent LLM 一次发多个 `Agent` tool_use 自然并行 | 无硬上限(但 Workflow tool 上限 16 concurrent / 1000 total per run) | LLM 决策(隐式)or `/workflows` 脚本 |
| **OpenHands TaskTool** | 严格串行,1 个 subagent / 次 | N/A | 单 tool_use |
| **OpenHands DelegateTool** | `ThreadPoolExecutor(max_workers=5)`,`threading.Thread` 真并行 | `max_children: int = 5`(构造参数) | `spawn` + `delegate` 两阶段 |
| **LangGraph Send** | Pregel BSP superstep,asyncio gather N 个 Send | 框架无硬上限(用户/资源决定) | conditional edge return `list[Send]` |
| **Hermes delegate_task** | `ThreadPoolExecutor(max_workers=max_concurrent_children)`,`as_completed` + 按 `task_index` 排序 | 默认 3(`DELEGATION_MAX_CONCURRENT_CHILDREN` env 覆盖);批超限 `tool_error` 拒收(不静默截断) | LLM 调 tool + `tasks[]` 批量字段 |
| **Everlasting**(当前) | 父 LLM 一次批多个 `dispatch_subagent` 被 `is_parallel_eligible` 排除,强制 serial | 1 worker / 父 turn | `chat_loop.rs:1703` 拦截 + `await run_subagent` 阻塞 |

**Takeaway**:
- **CC / OpenHands DelegateTool / Hermes** 都是"用 ThreadPool / asyncio gather 真并行"
- **LangGraph** 是图引擎级 BSP 并行(对应用更透明)
- **OpenHands TaskTool / Everlasting 当前** 是同步串行
- **关键设计差异**:OpenHands `max_children=5` 构造参数 vs Hermes `max_children=3` + **批超限硬拒**(不静默截断)。后者更安全

---

## 3. Lifecycle / Cancel

| 项目 | 取消传播 | 中断粒度 | Resume / Checkpoint |
|---|---|---|---|
| **CC Agent** | UI 按钮触发 stop,`SubagentStop` hook | in-flight tool calls interrupted,**无 rollback 文档** | `SendMessage(agentId)` resume 保留完整历史 |
| **OpenHands TaskTool/Delegate** | `LocalConversation.pause()` / `interrupt()`,cooperative cancellation | pause 在 run-loop iteration 边界;**未确认是否级联到子 conversation**(源码 fetch 截断) | `resume=task_id` 通过 `TaskAction`,自动持久化 `<persistence_dir>/subagents/` |
| **LangGraph Send** | `timeout: TimeoutPolicy(run_timeout, idle_timeout, refresh_on)`,`Task.cancel()` | 依赖 asyncio 协作,同步阻塞代码不响应 | checkpoint 自动持久化已完成分支中间结果,resume 跳过 |
| **Hermes delegate_task** | **`_interrupt_requested` flag 在父 agent 上,父打断递归取消所有 active 子/孙**;无 wall-clock 默认超时(靠心跳 staleness 15/40 cycles) | `child_timeout_seconds` 可选(下限 30s,0=disabled) | **无 resume / checkpoint**;`background=true` detached 但 process-local,#48796 已知 `background=true` 600s 上限 bug 无 partial summary |
| **Everlasting**(当前) | `parent_token.child_token()` tokio child token,父 cancel 一发 worker 收到 | worker 的 `select!` 看到 cancel → cancelled 路径 | 无 resume,worker 退 `Incomplete` 状态 |

**Takeaway**:
- **Hermes 显式递归取消** 最干净,值得抄到 multi-worker
- **OpenHands resume 模型**(`task_id` 自动持久化)Everlasting 缺;`subagent_runs` 表已有 row,但无 resume API
- **LangGraph TimeoutPolicy 三字段** 比 Everlasting 当前 `SUBAGENT_MAX_TURNS=200`(turn 计数,非时间)精细

---

## 4. Context Isolation

| 项目 | 对话历史 | system prompt | tool set | permissions |
|---|---|---|---|---|
| **CC Agent** | **不继承**父历史(fresh context),fork 例外(全继承) | subagent 自己的 prompt + env details,不是父 prompt | 继承全部 internal + MCP,可用 `tools`/`disallowedTools` 裁剪 | 继承父 mode,`bypassPermissions`/`acceptEdits` 父强制不可覆盖;`auto` 父继承可被 frontmatter `permissionMode` 覆盖 |
| **OpenHands TaskTool** | **不继承**父消息历史(除非 `resume=task_id`) | `factory.definition` per-subagent-type | `factory.definition.tools` | **继承父 `state.confirmation_policy`**(可显式 override) |
| **LangGraph Send + subgraph** | Send arg **私有**(父 state 切片),分支不可见 | subgraph 独立 state schema | subgraph 内部自己定义 | subgraph 内部自己 |
| **Hermes delegate_task** | **强隔离**("zero knowledge of parent's history"),`goal`/`context` 显式塞 | `_build_child_system_prompt` 独立构造 | `toolsets` 显式声明 + 5 个硬阻断(`delegation`/`clarify`/`memory`写/`code_execution`/`send_message`) | 父 policy 继承,`subagent_auto_approve` / `_subagent_auto_approve` 加严 |
| **Everlasting**(当前) | **不继承**父对话历史(`build_worker_messages` 只放 `[memory_blocks, task]`) | 父 prompt **完全替换**为 `SubagentDef.system_prompt`(`system_prompt_override`) | `filter_tools_for_subagent` + 5 个 `STRUCTURALLY_DISABLED` 硬剥 | `PermissionContext.is_worker=true`,Tier 4 ask 走 `permission:ask` IPC(2026-06-22 RULE-FrontSubagent-003) |

**Takeaway**:Everlasting 已经在"**强隔离** + 5 个工具硬阻断"上对齐 Hermes(都禁了 `dispatch_subagent` no nesting + `update_checklist` 进度跟踪污染 + L1a 后台 shell)。CC `fork` 是个有意思的"全继承"模式(罕见但有场景),不一定要抄。

---

## 5. Failure Visibility

| 项目 | wire shape | partial output? | 结构化 status? |
|---|---|---|---|
| **CC Agent** | text block,resumable 加 `agentId` trailer | 未明文,推测 error text 进入 final message | 否(纯文本) |
| **OpenHands TaskTool** | `TaskObservation{is_error, task_id, subagent, status, text}`,**`status` enum `"completed"\|"error"`** | **是**——`status="error"` 时 `text="<reason>\nPartial result:\n<partial>"` | 是,`TaskStatus` 枚举 |
| **OpenHands DelegateTool** | per-agent inline:`"Agent <id> ERROR: <error_msg>"` | 是(同 TaskTool 路径) | 是 |
| **LangGraph Send** | raise → 父 conditional edge 拿原始异常;正常路径 → reducer 合并 state | **否**——atomic superstep,任一 raise 整步回滚;partial 通过 checkpoint 恢复 | 否(走 channel state) |
| **Hermes delegate_task** | `List[{status, summary, error, ...}]`,5 状态 enum(`complete`/`timeout`/`error`/`interrupted` + orchestrator `exit_reason`) | **是**——`summary` 字段即使 error/timeout 也可能非 None(0 progress 时为 None,#48796) | 是 |
| **Everlasting**(当前) | string `[status: X]\n<text>` + 可选 `Worker partial actions:` 段 | 部分——`summarize_worker_tool_actions` 把已执行的 tool_call/result 表格化(RULE-BackSubagent-001) | 否(prefix string) |

**Takeaway**:
- **OpenHands / Hermes** 的结构化 status enum + partial output 是 4 家里最强的失败可见性
- **LangGraph** atomic superstep 是反面——一致性强但不容忍乐观并行
- **Everlasting** 已有 `summarize_worker_tool_actions` 做部分补偿,但 wire 是 prefix string,不是 typed 字段;父 LLM 要做字符串解析

**建议 L3a**:把 wire 从 `[status: X]\n<text>` 升级为**结构化 JSON**(`status: enum + summary: str + partial_actions: [{name, input, is_error}]`),对齐 OpenHands / Hermes。

---

## 6. Token Usage / Cost Attribution

| 项目 | 归集方式 | UI 展示 |
|---|---|---|
| **CC Agent** | 各自 context 独立计数;fork 优化:复用父 prompt cache(便宜) | `/workflows` view per-agent;`/costs` 文档 |
| **OpenHands** | `parent.conversation_stats.usage_to_metrics["task:{id}"]` / `["delegate:{id}"]`;子 `reset_metrics()` 隔离 | Agent Canvas TUI 展示 |
| **LangGraph** | per-node 各自计,父 graph 聚合 | LangSmith 集成 |
| **Hermes** | **三层**:per-call token 聚合到子 `session_estimated_cost_usd` → `_child_cost_usd` 拷到结果 dict 关键注释:"Port of Kilo-Org/kilocode#9448 — previously the footer only reflected the parent's direct API calls and under-counted subagent-heavy runs" → 父累加(`_child_cost_usd` 字段在序列化前 strip,进 parent counter 但不进 LLM context) | TUI `/agents` overlay tree 形展示 |
| **Everlasting**(当前) | worker 复用父 session_id → per-turn 用量通过 `db::add_token_usage` 折进 `sessions.*_total`(`chat_loop.rs:1031` 旁路 `skip_persist`);子 `subagent_runs.token_usage_json` 单独存 | `<SubagentDrawer>` 读 DB;父 UI 计数器延迟几秒(RULE-BackSubagent-002 option i) |

**Takeaway**:
- 4 家一致:**子独立计 → 父聚合**(OpenHands 走 key 索引,Hermes 走 child-close 前 fold)
- Hermes 注释点名 "**Port of Kilo-Org/kilocode#9448**" 是 subagent-heavy 任务成本归集 bug 的修复,值得 Everlasting 借鉴——**显式 fold 子 cost 到父 counter**,别只 fallback 到 "复用 session_id 自然累加"
- LangGraph / OpenHands 都没有做"子 cost 字段在序列化前 strip" 这一步(因为它们的 wire 不是 string-only),Hermes 专门做了因为它的 wire 是 dict

---

## 7. 一手参考清单(per project)

### Claude Code
- 官方文档:https://code.claude.com/docs/en/sub-agents
- SDK subagents:https://code.claude.com/docs/en/agent-sdk/subagents
- Agent teams:https://code.claude.com/docs/en/agent-teams
- Dynamic workflows:https://code.claude.com/docs/en/workflows
- 版本注:v2.1.63 `Task` → `Agent`;v2.1.117 fork;v2.1.154 dynamic workflows;v2.1.161 `/fork`;v2.1.172 nested subagents(depth 5);v2.1.186 background subagent permission prompts

### OpenHands `agent-sdk`
- 仓库:https://github.com/All-Hands-AI/agent-sdk(subpaths `openhands-sdk/` 和 `openhands-tools/`)
- 文档:https://docs.openhands.dev/sdk/guides/agent-delegation.md
- 文档:https://docs.openhands.dev/sdk/guides/task-tool-set.md
- 文档:https://docs.openhands.dev/sdk/guides/file-based-agents.md
- 关键源码(精确行号):
  - `openhands-tools/.../task/definition.py:50` `TaskAction` schema
  - `openhands-tools/.../task/definition.py:79` `TaskObservation` schema
  - `openhands-tools/.../task/impl.py:30` "translates a TaskAction into a blocking sub-agent execution"
  - `openhands-tools/.../task/manager.py:151-156` `_evict_task`(pause+close)
  - `openhands-tools/.../task/manager.py:312-321` `_run_stop_detail` — partial result on non-finish
  - `openhands-tools/.../delegate/impl.py:48` `max_children: int = 5`
  - `openhands-tools/.../delegate/impl.py:251-263` `threading.Thread` 并行派发
  - `openhands-tools/.../delegate/impl.py:295-301` metrics roll-up under `delegate:{id}`

### LangGraph `Send`
- 仓库:https://github.com/langchain-ai/langgraph
- 文档:https://docs.langchain.com/oss/python/langgraph/use-graph-api
- 文档:https://docs.langchain.com/oss/python/langgraph/pregel
- 文档:https://docs.langchain.com/oss/python/langgraph/use-subgraphs
- API ref:https://reference.langchain.com/python/langgraph/types/Send
- 关键源码:
  - `libs/langgraph/langgraph/types.py:437-496` `Send` 类
  - `libs/langgraph/langgraph/types.py:305-361` `TimeoutPolicy` 类
- 单独调研:本目录 `langgraph-send-primitive-survey.md`(2000 字,§8 列出对 Everlasting 5 条借鉴)

### Hermes `delegate_task`
- 仓库:https://github.com/NousResearch/hermes-agent
- 文档:https://hermes-agent.nousresearch.com/docs/user-guide/features/delegation/
- 文档:https://hermes-agent.nousresearch.com/docs/guides/delegation-patterns
- 关键源码:`NousResearch/hermes-agent/tools/delegate_tool.py`(行 ~760 = `_build_child_system_prompt`、行 ~800-850 = `_run_single_child` 成本归集、行 900-950 = `tasks[]` 校验、行 950-1000 = `ThreadPoolExecutor` 批调度、行 1313+ = `delegate_task` 入口)
- 已知 issue:
  - #48796 `background=true` 600s 上限 bug 无 partial summary
  - #5012 / #10995 per-call model override
  - #9402 estimated vs actual cost 对账
  - #9459 agent profiles / custom orchestration harness
- 二手参考:Blake Crosley · Hermes Agent: the practitioner's reference (2026);fast.io · subagent-delegation-patterns;DeepWiki 5.7 subagent-delegation

---

## 8. 调研方法(可复现性)

1. **CC 部分**:`claude-code-guide` agent 用 WebFetch 抓 `code.claude.com/docs/en/sub-agents` 等 5 篇官方文档,版本号交叉对 GitHub changelog。**只读一手**,没有二手补漏。
2. **OpenHands 部分**:`general-purpose` agent 用 WebFetch 抓 GitHub raw `agent-sdk` 仓库 5 个核心文件(精确到行号)+ `docs.openhands.dev` SDK 文档。1 处源码 fetch 截断(`local_conversation.py` cancel 级联路径),明确标"未知"。
3. **LangGraph 部分**:`general-purpose` agent 抓 GitHub `types.py` Send/TimeoutPolicy 源码 + LangChain 官方文档 4 篇 + DeepWiki/Substack/Medium 二手交叉。已单独落 `docs/research/langgraph-send-primitive-survey.md`(~2000 字)。
4. **Hermes 部分**:`general-purpose` agent 抓 GitHub 仓库 `tools/delegate_tool.py` + 官方 docs + AGENTS.md + 5 个已知 issue。**消歧了** "Hermes" 指向(`nousresearch/hermes` 是模型仓,不是 agent 框架;`NousResearch/hermes-agent` 才是 `delegate_task` 工具的实际位置),排除 3 个混淆项。

---

## 9. 对 Everlasting L3a 的设计建议(分阶段)

### Phase 1 — 并发(最低风险,先做)

把 `dispatch_subagent` 加进 `is_parallel_eligible` 的 `NAME_ELIGIBLE` 名单(`chat_loop.rs` 的 `is_parallel_eligible` 谓词),父 LLM 一次发 N 个 `dispatch_subagent` 走 `FuturesUnordered` 并发(对齐 L2 单 turn 多 tool 并发执行的现有模式,`chat_loop.rs` 已有这条管线)。worker 各自跑独立 `run_chat_loop`,共享 `cancellations` map(已有 key 设计,新 rid 各自注册)。

- **风险点**:worker 各自 `tokio::spawn` 后,父循环的 `await` 语义要改——`await` 的不是单 worker,而是 N 个 worker 的 `JoinSet`。
- **上限**:参考 Hermes `max_concurrent_children=3` 起步,加一个 `DELEGATION_MAX_CONCURRENT_CHILDREN` 配置(对齐 Hermes 命名)。**批超限硬拒**(对齐 Hermes,不静默截断)。
- **取消传播**:`parent_token.child_token()` 已有,递归到所有 worker 是一次性 fan-out。

### Phase 2 — 结构化 wire(中风险,中期)

把 `[status: X]\n<text>` 升级为结构化 JSON:

```json
{
  "status": "completed" | "cancelled" | "error" | "incomplete",
  "summary": "<worker final text>",
  "partial_actions": [
    { "name": "write_file", "input": {...}, "is_error": false }
  ]
}
```

`partial_actions` 已经在 `summarize_worker_tool_actions`(`truncate_summary.rs:335-462`)实现,只是 wire 是 string。提到 typed 字段,父 LLM 不再做字符串解析。

- **风险点**:跨层 wire 变更——`ToolResultPayload` schema 改、前端 `<SubagentDrawer>` 渲染路径改、IPC payload 改。
- **收益**:对齐 OpenHands / Hermes,失败可见性 + 父 LLM 补偿修复效率提升。

### Phase 3 — Per-worker timeout(低风险,可独立做)

参考 LangGraph `TimeoutPolicy` 三字段:

```rust
struct WorkerTimeoutPolicy {
    run_timeout_ms: Option<u64>,     // 硬墙钟(对齐 SUBAGENT_MAX_TURNS 200 turn)
    idle_timeout_ms: Option<u64>,    // 静默上限(检测 stalled worker)
    refresh_on: "auto" | "heartbeat", // heartbeat 需 worker 周期性 emit
}
```

替代当前 `SUBAGENT_MAX_TURNS` 单 turn 计数。`idle_timeout` 配合 worker 的 `process_alive` 心跳是更精细的"卡死检测"(对齐 Hermes 的心跳 staleness 监控)。

### Phase 4 — Resume / Checkpoint(高风险,长期)

参考 OpenHands `resume=task_id` + 自动持久化。当前 Everlasting `subagent_runs` 表有 row 但无 resume API。要做的话:

- 加 `subagent_runs.resume_from_id` 字段
- worker 退出时把完整 messages(不只 final text)存到 `transcript_json`
- 父 LLM 调 `dispatch_subagent({subagent, task, resume_from_id})` → worker 从 transcript 重建 messages 继续跑

这个跟 C3 压缩正交,需要单独排期。

### Phase 5 — 多层 delegation(对齐 CC depth 5)(最远)

`STRUCTURALLY_DISABLED` 当前硬剥 `dispatch_subagent`(no nesting)。要支持多层:

- 改 `max_spawn_depth` 配置(对齐 Hermes `max_spawn_depth=2`)
- worker 的 `PermissionContext` 加 depth 字段,超过阈值 Tier 4 ask 自动 deny
- transcript / IPC 都加 depth 字段,UI 渲染树形

**建议先不做**——L3b 阶段(writeup 隔离 + Hermes `delegate_task` 范式)再考虑。

---

## 10. 取舍总结

| 关键决策 | 行业默认 | Everlasting 当前 | L3a 建议 |
|---|---|---|---|
| 默认 sync vs async | Sync(CC / Hermes) | Sync(强制,无 async 开关) | **保持 sync**(MVP LLM-turn 阻塞约定),async 是 v3+ |
| 并发上限 | 3-5(Hermes 3 / OpenHands 5 / CC Workflow 16) | 1 | **3-5**,可配 |
| 批超限行为 | 硬拒(Hermes) | N/A | **硬拒**,对齐 Hermes |
| Failure wire | Typed enum(OpenHands / Hermes) | Prefix string | **升级为 typed JSON** |
| Partial output on fail | 是(OpenHands / Hermes) | 部分(`summarize_worker_tool_actions` 已实现,但 wire 是 string) | **wire 升级** |
| Cancel 传播 | 父 → 子(CC UI / Hermes 显式 flag / LangGraph 整 superstep) | `parent_token.child_token()` 单 worker | **递归到 N worker**,加 `_interrupt_requested` 模式 |
| Timeout 维度 | run + idle + heartbeat(LangGraph) | turn 计数(200) | **run + idle + heartbeat** |
| Cost 归集 | 子独立计 → 父聚合(OpenHands key / Hermes fold) | session_id 复用自然累加 + 延迟几秒 | **显式 fold**,对齐 Hermes `_child_cost_usd` 模式 |

**最关键的 1 件事**:Phase 2(结构化 wire)单独拎出来做,**不要把并发 + 结构化 + 多层 + resume 一次做完**。Phase 1 并发是最低风险,能立刻解"父 LLM 想并行调研"的实际痛点;Phase 2 改 wire 是中风险但高收益;Phase 3-5 是远期。

---

## 附:调研过程中发现的小问题(顺手记)

- `docs/research/langgraph-send-primitive-survey.md` §8 出现"everlasing"拼写错误 2 处,需修正。
- OpenHands 调研一处"未知"：`LocalConversation.pause()` 是否级联到子 conversation,源码 fetch 截断,建议补一次 raw 抓取。
- Hermes `NousResearch/hermes-agent` 在 ROADMAP 里被引用为 "Hermes",但项目名是 `hermes-agent`,引用时应明确消歧,避免后人混淆 `nousresearch/hermes`(模型仓)。
