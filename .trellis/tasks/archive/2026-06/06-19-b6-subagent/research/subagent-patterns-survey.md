# Research: 业界 coding agent subagent / delegation 模式调研

- **Query**: 为 Everlasting B6 Subagent 调研业界 coding agent 的 subagent/delegation 模式,提供设计参考
- **Scope**: external(纯外部研究;本项目侧的映射基于已有 spec,不重新搜索代码)
- **Date**: 2026-06-19
- **调研对象**(按相关度,均已取到一手文档):
  1. **Claude Code subagents**(CLI + Agent SDK)—— 最直接类比
  2. **OpenHands SDK**(DelegateTool / TaskToolSet / File-based agents / Context Condenser)
  3. **Cline `use_subagents`** —— 只读并行探索子代理
  4. **Cursor subagents / `/in-cloud`** —— 后台 + 嵌套 + 独立 VM/branch
  5. **Aider architect/editor** —— 负面对照(单线程两段推理,非真 subagent)

---

## 0. 一句话约定对照表(供快速查阅)

| 维度 | Claude Code | OpenHands DelegateTool | OpenHands TaskToolSet | Cline `use_subagents` | Cursor `/in-cloud`/nested | Aider architect |
|---|---|---|---|---|---|---|
| Worker context 起点 | 全新空 + 任务消息(唯一通道是 prompt) | 全新空 + spawn/delegate 任务 | 全新空 + `prompt` 参数 | 全新空 + 每子代理独立 prompt | 全新空 + 独立 VM/branch | **同一对话**,architect 输出串到 editor |
| Summary 形态 | worker final message → 父 Agent tool_result | `delegate` 返回 consolidated message | `TaskObservation.text` → tool_result | "detailed report" → tool_result | 子代理 final message → 父 Task tool | architect 文本作为 editor 的 user 消息 |
| Token 预算 | 独立 context;无显式预算字段(有 `maxTurns`) | `max_iteration_per_run` | 无显式预算 | 独立 context + 独立 cost tracking | 独立 VM/context | 共享上下文 |
| Tool 子集 | `tools` allowlist / `disallowedTools` denylist;不可用 UI 工具固定排除 | frontmatter `tools` 列表 + 内置 `finish`/`think` | 同 file-based | **硬编码只读 6 件**(无 edit/browser/MCP) | 继承父 + 可嵌套 | architect 无 edit tool;editor 才有 edit tool |
| System prompt | **独立**(subagent 自己的 prompt,**不继承** Claude Code 系统提示) | 独立(frontmatter body = 系统提示,追加到父 suffix) | 同 file-based | 独立(系统内置) | 独立 prompt+model | 沿用 aider 主提示 |
| 并发模型 | foreground 阻塞 / background 并发(`background:true` 或 Ctrl+B);`CLAUDE_CODE_FORK_SUBAGENT=1` 全部后台 | **并行** fan-out(异步) | **同步阻塞**(parent 等到完成) | **并行**(多子代理同时) | background(父不停) / nested 任意深度 | **串行**两段 |
| 错误/cancel | 子代理独立 transcript,失败 final 回填;background auto-deny 危险工具 | `status: completed|error` 写进 observation | `status: error` → observation | 主代理照常取消,子代理结果丢失 | VM 独立,父不被污染 | 第二段失败=整体失败 |
| 可见性 | 中间过程隔离,只回 summary;`SubagentStart/Stop` hook 可观测;transcript 落 jsonl | 中间隔离;`DelegationVisualizer` 面板 | 中间隔离;task_id 可恢复 | 中间隔离;UI 显示 per-subagent stats | 中间隔离;Agents Window 面板 | 全程可见(同一对话) |

---

## 1. Context 隔离边界

### Claude Code(最严格的隔离)

来源:[docs.claude.com/en/docs/claude-code/sub-agents §"Manage subagent context" / "What loads at startup"](https://docs.claude.com/en/docs/claude-code/sub-agents) + [code.claude.com/docs/en/agent-sdk/subagents §"What subagents inherit"](https://code.claude.com/docs/en/agent-sdk/subagents)。

**非 fork 子代理的初始 context 包含:**

- 子代理**自己的**系统提示(markdown body 或 `prompt` 字段)+ Claude Code 追加的环境细节(cwd 等)。**注意:不是完整的 Claude Code 系统提示**。
- 任务消息:Claude(main)写的 delegation prompt。SDK 文档原话:**"The only channel from parent to subagent is the Agent tool's prompt string"** —— 父→子只有这一根管道,父对话历史 / 父工具结果 / 父系统提示都不传。
- CLAUDE.md / memory 层级(每个被 main 加载的层,包括 `~/.claude/CLAUDE.md`、project CLAUDE.md、`CLAUDE.local.md`、managed policy)。**内置 Explore / Plan 跳过这一项**。
- git status 快照(父 session 启动时的)—— 同样 Explore/Plan 跳过。
- 预加载 skills(`skills` 字段,全量内容注入)。

**fork 子代理**(变体):继承父对话**完整**历史 + 同 system prompt + 同 tools + 同 model + 共享 prompt cache。用于"任务需要太多背景才能 useful"的场景。

**理由**:隔离 = context savings + 防止父对话被冗长搜索结果/日志污染。Explore/Plan 进一步跳过 CLAUDE.md/git status 是为了"快和便宜"。

### OpenHands DelegateTool / TaskToolSet

来源:[Sub-Agent Delegation](https://docs.all-hands.dev/sdk/guides/agent-delegation) + [Task Tool Set](https://docs.all-hands.dev/sdk/guides/task-tool-set) + [File-Based Agents](https://docs.all-hands.dev/sdk/guides/agent-file-based)。

- 子代理 = 独立 `Agent` 实例 + 独立 `Conversation`。
- 父→子的唯一通道是 `delegate`/`task` 工具的 prompt 字符串。
- File-based agent 的 markdown body 通过 `AgentContext(system_message_suffix=...)` **追加到父 system message**,而不是替换。即 OpenHands 子代理会看到一份"父系统提示 + 子代理 frontmatter body"的合成系统提示(与 Claude Code"完全替换"不同)。
- TaskToolSet:每个 task 是全新 conversation,完成后持久化到磁盘,可 `resume=<task_id>` 恢复完整历史。
- `LLMSummarizingCondenser`(`max_size` + `keep_first`):worker 长了会用第二个 LLM 摘要老消息 —— 这是 worker 防 context 爆掉的官方机制(见 §3)。

### Cline `use_subagents`

来源:[docs.cline.bot/features/subagents](https://docs.cline.bot/features/subagents)。

- 每个 subagent **独立 context window + 独立 token budget**。
- 每个 subagent 接到一段"focused research question" prompt —— 没有 main 对话历史。
- "Cannot spawn nested subagents" —— 单层。
- 定位就是"keep main context clean while gathering broad information fast",与 Claude Code Explore 几乎同构。

### Cursor

来源:[/in-cloud changelog](https://www.cursor.com/changelog/cloud-in-agents-window) + [nested subagents changelog](https://www.cursor.com/changelog)。

- 子代理独立 context,跑在**自己的 VM 和 branch** 上 —— 物理隔离(workspace 都不共享)。
- nested subagents:子代理可再 spawn 子代理,任意深度。每层独立 prompt 和 model。
- "background without interrupting the parent agent" —— 父可继续在本地或云端跑。

### Aider architect/editor(**反例对照**)

来源:[aider.chat/2024/09/26/architect.html](https://aider.chat/2024/09/26/architect.html) + [docs/usage/modes.html](https://aider.chat/docs/usage/modes.html)。

- **不是真正的 context 隔离**:architect 和 editor 在同一对话流里。
- 流程:用户请求 → architect 输出"解决方案描述" → 这个描述作为 editor 的 user 消息 → editor 用 edit format 输出 file edits。
- 只有"职责分工"(reasoning vs formatting),没有"独立 context window"。**这是对照点:Everlasting B6 若要 context 隔离,不能走 aider 这条路**。

---

## 2. Summary 注入位置

### Claude Code

- 子代理完成时,**只有它的 final message 回到父**(中间 tool calls / 中间 thinking 都不回)。
- 父这边:作为 **Agent tool 的 tool_result**(SDK:"Messages from within a subagent's context include a `parent_tool_use_id` field" —— 用来追踪某条消息属于哪个子代理执行)。
- Summary 是 **LLM 自生成**(子代理最后的 assistant turn),系统不做裁剪。
- fork 模式:同样只有 final 回填,但 fork 的中间 tool calls 也隔离("fork's own tool calls still stay out of your conversation")。

### OpenHands

- **DelegateTool**:`{"command":"delegate", ...}` 返回一个 consolidated message(多个子代理结果合并)→ 作为 `delegate` 工具的 observation。
- **TaskToolSet**:返回 `TaskObservation { task_id, subagent, status: completed|error, text }` → 作为 `task` 工具的 observation。`text` = 子代理 final response(或错误消息)。
- Summary 也是 LLM 自生成(子代理最终的回复),系统不二次裁剪。
- resume 机制:第二次 `task(resume=task_00000001)` 复用子代理完整历史,不重跑。

### Cline

- 子代理返回 "detailed report focused on the most relevant file paths for the main agent to read next" —— 注意 Cline 的 summary **有明确语义指导**("relevance" 排序的 file path 列表),不只是 free-text。
- 作为 `use_subagents` 的 tool_result。

### Cursor

- 子代理 final message → 父的 Task 工具 tool_result。
- `/babysit` 场景:子代理(在云端)持续 iterate 直到 PR ready。

### Aider

- 不是 summary 回填,而是 **architect 的全文输出喂给 editor 作为编辑依据**。没有"摘要"概念,因为不走隔离 context。

### 约定提炼

- **所有真 subagent 实现都把 summary 作为 tool_result 回填**(Agent/Delegate/Task/use_subagents 工具的 observation)。这是 Everlasting 应该沿用的:worker 完成结果作为 `dispatch_subagent` 工具的 tool_result,天然复用现有 `ContentBlock::ToolResult` 协议。
- **summary 都是 LLM 自生成的 final turn**,没有任何工具做系统级二次裁剪。Everlasting 不需要单独写"summarizer 模型"。
- Cline 给 summary 一个 **结构化语义**(relevant file paths),值得借鉴:可以在 worker system prompt 里强制"summary 必须以 file:line 引用列表收尾"。

---

## 3. Token 预算 / 防 worker 爆 context

### 各工具的硬约束

| 工具 | 独立预算? | 防爆机制 |
|---|---|---|
| Claude Code | 独立 context,但**无显式 token 预算字段** | `maxTurns`(frontmatter,最大轮数);subagent 自动 auto-compaction(与主对话同逻辑);`CLAUDE_AUTOCOMPACT_PCT_OVERRIDE` 对 subagent 也生效 |
| OpenHands | 独立 context | `max_iteration_per_run`(frontmatter);`LLMSummarizingCondenser(max_size, keep_first)` 主动摘要老消息;blog 声称"2x API 成本下降" |
| Cline | 独立 context + **独立 token/cost 跟踪**(UI 显示 per-subagent) | 靠 context window 自然上限 + 独立 cost 防失控 |
| Cursor | 独立 VM/context | 独立 VM 自然隔离;嵌套有深度上限?changelog 说"任意深度"但每层独立 |
| Aider | 无 | 共享上下文,不适用 |

### 关键约定

1. **轮数限制(`maxTurns` / `max_iteration_per_run`)是普遍兜底**,比 token 预算更易实现且语义清晰。Everlasting 已有 `MAX_TURNS = 50`(全局),worker 可用一个更小的 `max_turns`(例如 20)参数,与 `run_chat_loop` 现有 `MAX_TURNS` 机制天然兼容。
2. **Claude Code 让 subagent 复用主对话的 auto-compaction 逻辑** —— 不为 subagent 单独写压缩代码。Everlasting 已有 C3 context 压缩(token 硬卡),worker 直接复用 `run_chat_loop` 的 C3 路径即可,无需新写压缩。
3. OpenHands 的 `LLMSummarizingCondenser` 是更"主动"的策略(超过 `max_size` 条消息就摘要),可作为未来增强。

---

## 4. Worker 可用 Tool 子集 + 嵌套

### Claude Code

- 默认**继承所有 internal + MCP 工具**。
- `tools` 字段 = allowlist;`disallowedTools` = denylist(先 deny 后 resolve allow)。两者都支持 MCP server 通配 `mcp__<server>` / `mcp__*`。
- **结构性禁用**(即使在 `tools` 里列了也不可用):
  - `AskUserQuestion`、`EnterPlanMode`、`ExitPlanMode`(除非 subagent `permissionMode=plan`)、`ScheduleWakeup`、`WaitForMcpServers`。
  - **理由**:这些依赖主对话 UI / session 状态,subagent 隔离环境下没有宿主。
- 嵌套:v2.1.172+ subagent 可 spawn 自己的 subagent。**深度上限固定为 5**(从 main 算起,不区分 foreground/background),不可配置。深度 5 的 subagent 不再获得 Agent 工具。
- `Agent(agent_type)` allowlist 语法:只允许 spawn 指定类型的 subagent(coordinator 模式)。

### OpenHands

- File-based frontmatter `tools` 列表。所有 agent 自动获得 `finish` + `think` 工具(即使 frontmatter 不写)。
- 内置 agent 的工具分配(参考点):
  - `general-purpose`: terminal + file_editor + task_tracker
  - `code-explorer`: **只 terminal(只读探索)**
  - `bash-runner`: terminal
  - `web-researcher`: browser + MCP(fetch/tavily)
- 没看到明确的"结构性禁用 UI 工具"清单(因为 OpenHands 工具集本身不含 AskUser 这类 UI 工具)。
- 嵌套:文档没明确说,但子代理的 tool 列表里若包含 `DelegateTool` 自己,理论上可嵌套(未验证有深度限制)。

### Cline(**最保守**)

- **硬编码 6 件只读工具**,不可配置:`read_file` / `list_files` / `search_files` / `list_code_definition_names` / `execute_command`(只读) / `use_skill`。
- 明确禁用:edit / browser / MCP / web search / **嵌套 subagent**。
- 设计意图:子代理是"研究"角色,不产生副作用,所以连配置项都不给。

### Cursor

- 嵌套:**任意深度**(changelog 原话:"nest subagents to any depth ... There's nothing to turn on")。
- 自定义工具自动对所有子代理可见。

### Aider

- architect **没有 edit 工具**(只输出方案文本);editor 才有 edit 工具。这是"职责切片"而非"权限收窄"。

### 约定提炼(Everlasting 适用)

- **allowlist + denylist 双字段** 比 Cline 的"硬编码"更灵活,推荐沿用 Claude Code 模式。
- **结构性禁用 UI/会话相关工具**这一约定值得直接抄:Everlasting 的 worker 不应看到 `update_checklist`(那是 main 的进度表)、`run_background_shell` 的通知注入机制(那是 session 级)、权限 ask modal(父有 `permission_asks`,worker 没有 UI sink)。可以在 builtin tool 注册时打 `subagent_capable: bool` 标记,worker 只接受 `true` 的工具。
- **嵌套深度上限固定** > 可配置:Claude Code 选 5,OpenHands 不限。Everlasting MVP 建议先**单层禁嵌套**(对标 Cline),待有需求再放深度。

---

## 5. System Prompt

### Claude Code(完全替换)

- 子代理**只**用自己的 system prompt(自定义 markdown body 或 `prompt` 字段)+ 环境细节。**不继承 Claude Code 主系统提示**。
- "Built-in agents have predefined prompts" —— Explore/Plan/general-purpose 各有自己的。
- memory 层(CLAUDE.md)以 **user-role 消息** 注入,不在 system field。

### OpenHands(追加到父 prompt)

- File-based body 通过 `system_message_suffix` 追加到父系统消息(见 §1)。
- 与 Claude Code 的关键差异:OpenHands worker 看得到父的 base 系统提示。

### Cline

- 独立系统提示(系统内置,固定)。

### 约定提炼

- **完全替换 vs 追加** 两种流派。Claude Code 的"完全替换"更干净(worker 不知道自己是 worker 之外的任何事),Everlasting 建议**完全替换**:worker 只看到"你是 X 子代理,任务:..."。这与本项目 system prompt 三层组装(`behavior_prompt` + `mode_prefix` + `base_prompt`)正交 —— worker 走另一条 `assemble_subagent_prompt(name, description, task)`,不混入 main 的 `behavior_prompt`。

---

## 6. 并发模型

### Claude Code

- **Foreground(默认阻塞)**:父等到完成,permission prompt 透传给用户。
- **Background**(`background: true` frontmatter 或 Ctrl+B 或 `CLAUDE_CODE_FORK_SUBAGENT=1`):并发跑,auto-deny 任何需要 prompt 的工具调用;若 background 子代理需要 ask clarifying,该工具调用失败但子代理继续。
- 失败 fallback:"如果 background 子代理因权限失败,可启动新 foreground 子代理用同样任务重试(带交互 prompt)"。
- `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1` 全关。

### OpenHands(**两个工具分别对应两种并发**)

- **DelegateTool** = 并行 fan-out(`spawn` 多个 id 后 `delegate` 一次性发任务,等所有完成返回合并结果)。
- **TaskToolSet** = 同步阻塞(一次一个,parent 等)。
- 文档明确对比表:

|  | TaskToolSet | DelegateTool |
|---|---|---|
| Execution | Sequential (blocking) | Parallel (concurrent) |
| Concurrency | One task at a time | Multiple sub-agents simultaneously |
| Resumption | Built-in via `resume` | Persistent sub-agents by ID |
| API | Single `task` tool call | `spawn` + `delegate` commands |
| Best for | Expert delegation, multi-turn | Fan-out / fan-in parallelism |

### Cline

- **并行**(多 subagent 同时跑),定位就是"broad context from multiple areas at once"。

### Cursor

- background(父不停),`/in-cloud` 跑独立 VM。父和子可同时各自工作。

### 约定提炼(Everlasting 适用)

- **同步阻塞是 MVP 最低成本路径** —— main 在 `dispatch_subagent` tool 的 `execute` 里 `.await` 一个嵌套 `run_chat_loop` 调用。这与本项目 L1a background shell 的"返回 handle + 下一轮注入 notification"异步模式正交;B6 MVP 先做同步版,异步版可后续叠加(对标 Claude Code 的 background 字段)。
- OpenHands 用**两个独立工具**(DelegateTool vs TaskToolSet)区分并发模式 —— 这是个干净的设计,Everlasting 也可考虑 `dispatch_subagent`(同步) vs `dispatch_subagents`(并行 fan-out)双工具。
- Everlasting 已有 **L1a 异步通知注入先例**(background shell 完成后下一轮 append user 消息),未来异步 subagent 完全可复用这套机制。

---

## 7. 错误与 cancel 传播

### Claude Code

- 子代理失败 → final message 仍是回填(可能是错误描述)。
- background 子代理 auto-deny 危险工具 = 不向用户弹 prompt,直接当 deny。
- 子代理 transcript 独立 jsonl 文件;main 对话 compaction 不影响子代理 transcript。
- "自动清理基于 `cleanupPeriodDays`(默认 30 天)"。

### OpenHands

- `TaskObservation.status = "error"` → observation.text = 错误消息。父看到 status,自行决定后续。
- TaskToolSet 持久化:即使错误,task 仍可 resume。

### Cline

- 没有显式错误传播文档;子代理结果丢失 = tool_result 为空或部分。
- auto-approve:子代理 launch 受 "Read project files" auto-approve 控制。

### 约定提炼

- **status 字段 + error as text** 是通用模式。Everlasting 的 `dispatch_subagent` tool_result 应包含显式 status(`completed` / `error` / `cancelled`),参考 OpenHands `TaskObservation`。
- worker 的 CancellationToken:本项目已有 per-request `CancellationToken`(`cancellations: Arc<Mutex<HashMap<String, CancellationToken>>>`),worker 应该有自己的 rid 注册进这个 map,这样用户 Stop 能传播到 worker。worker 失败要落 audit(已有 `AuditKind`),失败 tool_result 要回填保持 tool_use/tool_result 配对不变量。

---

## 8. 可见性

### Claude Code

- 中间过程对 main **隔离**:只有 final message 回父。
- 但对**用户**部分可见:
  - `/agents` Running tab 列所有正在/最近完成的子代理,可点开看 transcript。
  - nested 时面板显示树(`(+N)` 后代计数,展开看直接子节点)。
  - fork 模式有面板(`↑↓` 切行,`Enter` 看详情,`x` 停止,`Esc` 回 prompt)。
- `SubagentStart` / `SubagentStop` 项目级 hook(在 settings.json 配,matcher 按 agent_type)。
- transcript 路径:`~/.claude/projects/{project}/{sessionId}/subagents/agent-{agentId}.jsonl`。

### OpenHands

- 中间隔离;`DelegationVisualizer` 提供终端面板可视化层级和结果。

### Cline

- 中间隔离;UI 显示 **per-subagent stats**(tool calls、tokens、cost)。

### Cursor

- 中间隔离;Agents Window 面板。

### 约定提炼

- **"对 main 隔离,对 user 可观测"** 是普遍约定。Everlasting 可分两层:
  1. **MVP**:worker 跑完,main 只看到 summary tool_result;前端 `ToolCallCard` 显示"子代理 X 完成"+ 折叠的 summary。worker 中间过程可先不入主对话 SQLite。
  2. **v2**:worker transcript 独立 SQLite 表(对标 Claude Code 的 `subagents/agent-{id}.jsonl`),前端可点开查看 —— 这是 B6 增强而非 MVP 必须。
- **per-subagent token/cost 跟踪**(Cline 模式)直接复用本项目 per-session token usage 机制,worker 用自己的 session-like id 记账。

---

## 9. Fork 模式(Claude Code 独有,值得单独说)

Claude Code 的 fork 是个"隔离例外":继承父对话**完整历史** + 同 system prompt + 同 tools + 同 model + **共享 prompt cache**(因为 system prompt 和 tool 定义字节相同,第一个请求能复用父的 cache)。

**何时用 fork**:named subagent 需要太多背景才能 useful,或想从同一起点并行试多个方案。

**限制**:fork 不能再 spawn fork(只能 spawn 别的 subagent 类型,计入深度)。

**对 Everlasting 的启示**:fork 本质是"共享 prompt cache 的廉价 subagent"。本项目已有 prompt caching(memory/loader.rs 的 `cache_control: Ephemeral`),fork 模式可作为 B6 的"高级档"——但 MVP 不必做,fresh-context subagent 已覆盖 90% 场景。

---

## 10. Mapping to Everlasting

把上述约定 map 到本项目已知约束(全部来自现有 spec,不重新搜索):

### 10.1 架构入口:复用 `run_chat_loop`

来源:`.trellis/spec/backend/agent-loop-architecture.md` —— `run_chat_loop` 是 14 参数的共享入口,production 和 9 个 `agent_loop_*` 集成测试都走它。

**映射**:worker 就是嵌套调一次 `run_chat_loop`:

```rust
// dispatch_subagent::execute 内
let worker_messages = vec![ChatMessage { /* delegation task as user msg */ }];
run_chat_loop(
    subagent_tool_defs,          // §10.3 过滤后的子集
    provider.clone(),            // 可换 model: subagent_def.model
    context_window,
    worker_rid,                  // 独立 request id,注册进 cancellations
    parent_session_id,           // 复用父 session_id 做 audit 关联
    worker_messages,
    worker_sink,                 // §10.6 隔离的 sink
    db,
    cancellations,
    session_active_request,
    read_guard,
    memory_cache,
    permission_asks,
    worker_cancel_token,
).await;
```

`run_chat_loop` 的 14 参数签名**完全够用**,不需要改签名(对齐 spec §"Why 14 parameters").worker 的 max_turns 可以通过 messages 或新的 wrap 控制,不必改 `run_chat_loop` 主体。

### 10.2 Context 隔离:走 Claude Code 流派(fresh + prompt-only)

- worker context = `[delegation_task_user_message]` 单条,无父历史。
- 唯一父→子通道 = `dispatch_subagent` 工具的 `task` 字符串(对标 Claude Code SDK 原话)。
- worker system prompt = 完全替换(不混 main 的 `behavior_prompt`),走 `assemble_subagent_prompt(name, description)` 新函数。

### 10.3 Tool 子集:allowlist + 结构性禁用

- subagent 定义支持 `tools: Option<Vec<String>>`(None = 继承 builtin 全集;Some = allowlist)。
- **结构性禁用**(对标 Claude Code 的 UI 工具排除):本项目应禁 worker 使用:
  - `update_checklist`(B12,那是 main 的进度表 —— spec `tool-contract.md §"update_checklist tool"`)
  - `run_background_shell` 的通知注入机制(那是 session 级 ephemeral 注入,worker 没有 sink)
  - 权限 ask modal 透传(worker 没 UI sink,直接 deny —— 对标 Claude Code background subagent auto-deny)
- 内置"只读研究"档(对标 Cline `use_subagents` + Claude Code Explore):tools = `[read_file, grep, glob, list_dir]`。
- MVP **禁嵌套**(对标 Cline):worker 的 tool 列表不含 `dispatch_subagent`。

### 10.4 Summary 回填:tool_result + status

- worker 完成后,final assistant text 作为 `dispatch_subagent` 的 `ToolResult` 回填(沿用 `ContentBlock::ToolResult`)。
- 结构:参考 OpenHands `TaskObservation` —— 在 tool_result content 里加显式 `status: completed|error|cancelled` 前缀,便于 main LLM 判断后续。
- summary 是 worker LLM 自生成,不二次裁剪(所有工具的约定)。

### 10.5 Prompt cache 不变量(关键陷阱)

来源:`tool-contract.md §"update_checklist tool" §7` + `§"L1a Background Shell" §"Completion notification injection"` —— **APPEND,never prepend**;`messages[0]` 的 memory breakpoint 必须保持字节稳定。

**映射**:worker 的 delegation task 是 worker 自己 context 的 `[0]`,与父的 `messages[0]` 正交,**不会污染父的 cache key**。但如果未来要让 worker summary 注入父对话(非 tool_result 路径),必须走 APPEND,不能 insert(0)—— 这条规则本项目已被 B12 和 L1a 两次踩坑后锁死,B6 必须遵守。

### 10.6 并发模型:MVP 同步阻塞,异步留扩展

- **MVP**:`dispatch_subagent` 的 `execute` 内 `.await run_chat_loop`,main 阻塞等(对标 OpenHands TaskToolSet + Claude Code foreground)。
- worker sink:不向 main 的 frontend 直接 emit(否则 main UI 被worker 流刷屏)。worker 自己 emit 到一个 buffer,完成后只把 summary 作为 tool_result 回填 main 的 sink。对标 Claude Code"中间过程隔离"。
- **异步扩展**(留接口,对标 Claude Code background + 本项目 L1a):未来加 `dispatch_subagents`(plural,并行 fan-out)+ 完成通知走 L1a 的 drain_notifications 机制(下一轮 append user 消息)。

### 10.7 Token 预算:轮数兜底 + 复用 C3

- worker `max_turns` 用更小值(例如 20,main 是 50)。直接复用 `run_chat_loop` 现有 MAX_TURNS 路径。
- worker context 爆掉:复用现有 C3 压缩(`agent_loop_c3_compaction_does_not_panic` 已覆盖)—— 不为 worker 单写压缩代码(对标 Claude Code"subagent 复用主对话 auto-compaction")。
- 不引入显式 token 预算字段(没有工具这么做,都是用 `maxTurns`)。

### 10.8 持久化 + 可见性

来源:本项目 SQLite 持久化 + per-session token usage。

- **MVP**:worker 中间过程不落 DB,只把 summary 作为 `dispatch_subagent` tool_result 落主对话 turn(沿用 `db::persist_turn`)。
- **v2**:独立 `subagent_runs` 表(`id, parent_session_id, parent_request_id, subagent_name, status, started_at, finished_at, token_usage_json, summary`),前端 `ToolCallCard` 加"展开查看子代理"按钮(对标 Claude Code transcript + Cline per-subagent stats)。
- per-subagent token/cost:复用 per-session token usage 机制,worker 用 `worker_rid` 记账,父的 request usage 把所有 worker usage 汇总。

### 10.9 错误 / cancel 传播

- worker rid 注册进 `cancellations: Arc<Mutex<HashMap<String, CancellationToken>>>`,用户 Stop 传播到 worker。
- worker 失败 → tool_result `status: error` + 错误文本回填(保持 tool_use/tool_result 配对不变量 —— RULE-A-007 的同款约束)。
- worker 取消 → tool_result `status: cancelled` + `CANCELLED_MARKER`(复用 `helpers::CANCELLED_MARKER`)。
- audit:worker 的 tool 调用走 ⑨ 关 permission check(`permissions::check`),失败落 `session_audit_events`(已有 AuditKind)。worker 的 permission_mode 可与父不同(对标 Claude Code `permissionMode` 字段)。

### 10.10 Subagent 定义来源

- **MVP**:代码内置 1-2 个 subagent(只读 "researcher" + 通用 "general-purpose"),对标 Claude Code built-in。
- **v2**:Markdown frontmatter 文件加载(对标 Claude Code `.claude/agents/*.md` + OpenHands `.agents/agents/*.md`)—— 复用本项目 `resource_loader.rs`(Markdown + frontmatter 通用加载,Skill/Role/B3 已用)。

---

## Caveats / Not Found

- **Continue / Hermes 的 delegation 机制未取到一手文档**(`docs.continue.dev/customize/delegates` 和 `/features/agents` 都 404)。Continue 的 agent 系统在最近的版本里有 "Agent selector + gear icon" 配置(从搜索片段看到),但具体 delegation 语义没拿到。鉴于 Claude Code / OpenHands / Cline / Cursor 已覆盖主流约定,Continue 缺失影响不大。
- **SWE-agent 的多 agent 模式未取到**(老 URL `docs/usage/cl_tips.md` 404)。SWE-agent 主要是单 agent + ACI,本就不以 delegation 见长,可忽略。
- **OpenHands `DelegateTool` / `TaskToolSet` 源码具体实现**(`openhands/tools/delegate/delegate.py` 404)没读到 —— 因为 OpenHands 最近重构为 `openhands-sdk` 包,源码路径变了。本文的 OpenHands 约定全部来自官方文档(权威性足够),未读源码。
- **嵌套深度上限**:Claude Code 固定 5(明确);OpenHands 未明确(可能无限);Cursor 说"任意深度"(可能实际有别的限制)。Everlasting MVP 选"禁嵌套"是更保守的选择,与 Cline 一致。
- **worker 可见性的"实时流"**:Claude Code 的 `/agents` Running tab 能实时看 worker 进行中状态,但底层是 transcript 文件流还是单独 IPC 没确认。本项目 MVP 不做实时,完成后再展开即可。
- **fork 模式的 prompt cache 共享**:Claude Code 文档明确说 fork 因 system prompt + tools 字节相同而共享父 cache。本项目若做 fork 需验证 `cache_control` breakpoint 在嵌套调用下是否被 provider 正确复用 —— 这块未在调研中验证。
