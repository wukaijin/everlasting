# B6 Subagent — main agent 派 worker agent 跑独立 context

> **Review 修订**:2026-06-19 接受 deepseek-v4-pro 评审(`docs/_review/b6-subagent-prd-review.md`),采纳全部 6 项修订(参数数 14→17、CancellationGuard 双删、dispatch_subagent 依赖传递、max_turns 加参、PermissionContext.is_worker、worker messages 顺序)。详见末尾 §Review 修订。

## Goal

main agent 在 agent loop 中派出一个 **worker agent** 跑独立 context(独立 messages / 独立 token 预算),完成后由 worker 把 **summary** 回填给 main agent。对标 Claude Code Task tool / OpenHands TaskToolSet / Cline use_subagents。

**为什么做**:ROADMAP §4.1 标注的"harness 学习价值最高"项 —— 消息流隔离、context 预算管理、summary 注入位置都是 harness 设计的核心命题。L3(并行 subagent + worktree 隔离)依赖 B6 落地。本项目核心目标是学习 harness engineering,B6 是旗舰级学习项。

## What I already know

### 现有 harness 结构(已读源码)

- **`run_chat_loop`**(`agent/chat_loop.rs:96-128`)是 **17 参**共享入口(源码 docstring `:83` 自己错写"14-parameter",顺手修),production + 9 个 `agent_loop_*` 集成测试都走它。参数:tool_defs / provider / context_window / rid / session_id / messages / sink / db / cancellations / session_active_request / read_guard / memory_cache / skill_cache / permission_asks / token / resend_seq / background_shells。
- **`CancellationGuard`**(`state.rs:395-420`)RAII 守护,Drop 时 `cancellations.remove(rid)` + `session_active_request.remove(session_id)`。worker 复用 parent_session_id 会触发误删(见 §Review 修订 #2)。
- **`execute_tool_inner`**(`tools/mod.rs:172`)签名仅 (name, input, ctx, guard, session_id, skill_cache, cancel) —— **拿不到** run_chat_loop 必需的 provider/db/cancellations 等。dispatch_subagent 不能走此路径(见 §Review 修订 #3)。
- **`ToolContext`**(`tools/mod.rs`):worktree_path / cwd / checklist / background_shells。每轮重建。
- **emit 走 `ChatEventSink` trait**(`state.rs`),production `AppHandleSink` / test `MockEmitter` —— 嵌套调 run_chat_loop 可传独立 sink。
- **B12 checklist**(`update_checklist.rs`):per-request `Arc<Mutex<Vec>>` handle,走 ToolContext;每轮 ephemeral **APPEND** 注入。直接先例。
- **MAX_TURNS = 50**(`agent/mod.rs:56`),`chat_loop.rs:492 for turn in 1..=MAX_TURNS` 硬编码常量;C3 context 压缩每轮 send 前触发。
- **`PermissionContext`**(`agent/permissions/mod.rs`,已 import 于 `chat_loop.rs:55`)—— 现成载体,加 `is_worker: bool` 字段(见 §Review 修订 #5)。
- **permission**:`permissions::check`(⑨ 关 5-tier)+ Mode(edit/plan/yolo)+ `permission_asks` map。
- **cancel**:`cancellations` + `session_active_request`(session→request 1:1)+ `inflight_exits`(RULE-E-005)。
- **B5 memory**:`build_instructions_blocks()`(`memory/loader.rs`)构造带 `cache_control: ephemeral` 的 synthetic user message(4 文件:User/Project × CLAUDE.md/AGENTS.md)。
- **tool 注册**:`builtin_tools()` 返回 13 个 ToolDef。

## Research References

- [`research/subagent-patterns-survey.md`](research/subagent-patterns-survey.md) —— 8 维度对比 Claude Code / OpenHands / Cline / Cursor / Aider(反例)+ §10 Mapping to Everlasting 10 节。

## Decisions(全部收敛,含 Review 修订)

| # | 决策点 | 结论 |
|---|---|---|
| 1 | MVP subagent 定义 + 触发 | **代码内置 2 个**(`researcher` 只读 + `general-purpose`)+ `dispatch_subagent` tool,LLM 驱动,**同步阻塞**。Markdown frontmatter 加载留 v2。 |
| 2 | worker 可见性 + 持久化 | **summary + worker 中间过程落独立 `subagent_runs` 表**,前端 ToolCallCard 可展开查看。需 DB migration + 前端展开 UI。 |
| 3 | worker permission_mode | **继承 main mode + 无 UI sink 时 ask→deny**(main=yolo→worker 全 allow;main=edit/plan→worker 写/shell 被 deny)。对标 Claude Code background subagent auto-deny。检测走 `PermissionContext.is_worker`(见 #5)。 |
| 4 | worker tool 子集 | **allowlist + 结构性禁项**:researcher=`[read_file,grep,glob,list_dir]`;general-purpose=全集减禁项。结构性禁项(所有 worker 不可用):`update_checklist` / `dispatch_subagent`(禁嵌套) / `run_background_shell`+`shell_status`+`shell_kill`(L1a session 级)。 |
| 5 | worker audit/token 归属 | **全落 `subagent_runs`,不污染父 session**:worker rid 只进 `cancellations`(**不进** `session_active_request`,且 `CancellationGuard` 传 `skip_session_active=true` 跳过双重 remove —— Review 修订 #2);token 在 subagent_runs 存 + 汇总进父 session 累积;C4 audit log 查父 session 不显示 worker tool_executed。 |
| 6 | worker memory | **加载 B5 memory**(对标 Claude Code)。worker messages = `[memory_blocks_user_message(带 cache_control), delegation_task_user_message]`,memory 在前享 cache,task 在后 APPEND(Review 修订 #6)。 |
| 7 | summary 结构化语义 | **MVP free-text**(不强制 file:line 收尾),v2 可加结构化引导。 |
| 8 | worker max_turns | **加 `run_chat_loop` 第 18 参 `max_turns: Option<usize>`**(None=默认 50;worker 传 `Some(20)`)。复用 C3 压缩。production + 9 测试传 None(Review 修订 #4)。 |
| 9 | 并发 dispatch | **MVP 串行**:多个 dispatch_subagent tool_use 顺序阻塞执行。真正并行 fan-out(`dispatch_subagents` plural)留 v2 / L3。 |
| 10 | dispatch_subagent 执行路径 | **agent loop 层拦截,不走 `execute_tool_inner`**(Review 修订 #3)。注册为 ToolDef 供 LLM 发现,但在 `chat_loop.rs` tool_use 处理处识别并直接调专门的 `run_subagent(...)`(拥有 provider/db/cancellations 等依赖)。类比 Claude Code Agent tool 是 SDK 层特殊处理。 |

## Technical Approach

### dispatch_subagent 是"agent 层控制流工具",非普通 I/O 工具(Review 修订 #3)

`execute_tool_inner` 拿不到 run_chat_loop 的依赖(provider/db/cancellations 等)。**方案 A**:dispatch_subagent 注册为 `ToolDef`(LLM 可发现 + 走权限 ⑨ 关),但在 `chat_loop.rs` 的 tool_use 处理循环里**拦截**——识别 `name == "dispatch_subagent"` 时,不走 `execute_tool`,直接调用同模块的 `run_subagent(deps..., input, ctx)`(它能拿到 run_chat_loop 的全部闭包依赖),其余 tool 走原 `execute_tool` 路径。

拦截点位于 L2 并行/串行分支之前(或并行集合排除 dispatch_subagent —— 它本就不在 `is_parallel_eligible` 集合,天然串行)。tool_use/tool_result 配对不变:拦截后构造 `ContentBlock::ToolResult` 回填,与普通 tool 一致。

### 嵌套调 `run_chat_loop`(17 参 + 第 18 参 max_turns)

```rust
// chat_loop.rs 内 run_subagent(deps 捕获自 run_chat_loop 闭包)
let worker_rid = format!("{}-sub-{}", parent_rid, nanoid_or_seq);
let worker_token = CancellationToken::new();
cancellations.lock().await.insert(worker_rid.clone(), worker_token.clone()); // 只进 cancellations
let worker_messages = build_worker_messages(&memory_cache, &delegation_task); // [memory, task]
let worker_sink = SubagentBufferSink::new(parent_sink, run_id); // 隔离 emit + 落 transcript
run_chat_loop(
    subagent_tool_defs, provider.clone(), context_window, worker_rid,
    parent_session_id, worker_messages, worker_sink, db,
    cancellations, session_active_request, read_guard, memory_cache,
    skill_cache, permission_asks, worker_token, None,
    background_shells,
    Some(20),   // ← 第 18 参 max_turns(Review 修订 #4)
).await;
// worker final assistant text → dispatch_subagent tool_result(status 前缀 + summary)
```

worker 用 parent_session_id 做 audit/DB 关联,但靠 `CancellationGuard.skip_session_active=true`(下条)避免误删父映射。

### CancellationGuard 加 `skip_session_active: bool`(Review 修订 #2)

`state.rs` CancellationGuard 加字段 `skip_session_active: bool`;Drop 时 `if !skip_session_active { session_active_request.remove(session_id) }`。worker 的 guard 传 `true`,drop 只清 `cancellations[worker_rid]`,不动 `session_active_request["parent_session"]`。production chat 的 guard 传 `false`(行为不变)。

### worker context 构造(messages 顺序,Review 修订 #6)

1. `build_instructions_blocks(memory_cache)` → memory synthetic user message(4 文件,带 `cache_control: ephemeral`)作为 messages[0] —— worker 自己的 cache breakpoint,与父正交。
2. **APPEND** delegation task 作为 messages[1](`assemble_subagent_prompt(name, description, task)`)。
3. worker system prompt = 完全替换(走 `assemble_subagent_prompt`,不混 main behavior_prompt)。

**prompt cache 不变量**:worker messages[0] 与父 messages[0] 正交,不污染父 cache key。B12 + L1a 两次踩过的 prepend 坑,B6 必须 APPEND。

### tool allowlist + 结构性禁项过滤

`filter_tools_for_subagent(builtin_tools(), &subagent_def.tools)`:allowlist 过滤 + 强制移除结构性禁项(`update_checklist` / `dispatch_subagent` / L1a 三件),无论 allowlist 怎么写。

### worker permission:无 sink 时 ask→deny(Review 修订 #5)

`PermissionContext` 加 `is_worker: bool` 字段。worker 嵌套调时构造 `PermissionContext { ..., is_worker: true }` 传入 `permissions::check`。Tier 4 `ask_path`/`ask_shell` 分支:`if ctx.is_worker { Decision::Deny } else { 原逻辑 }`(不向 permission_asks 注册,不 emit permission:ask)。main=yolo 时 worker 继承 yolo(Tier 4 全 allow)。main=edit/plan 时 worker Tier 4 被 deny。

### worker 中间过程持久化(subagent_runs 表)

- 新增 migration:`subagent_runs(id, parent_session_id, parent_request_id, subagent_name, status, started_at, finished_at, token_usage_json, summary, transcript_json)`。
- worker 的 `SubagentBufferSink` 把每轮 tool calls/thinking/tool_results 累积进 transcript_json。
- worker 完成/失败/取消时 update subagent_runs(status + finished_at + summary + token_usage)。
- token_usage 汇总:worker per-request token usage 记入 subagent_runs,同时累加进父 session 的 token usage(用户见总消耗)。

### summary 回填(主对话)

worker final assistant text → dispatch_subagent 的 `ContentBlock::ToolResult`,content 带 `status: completed|error|cancelled` 前缀(对标 OpenHands TaskObservation)。status=cancelled 复用 `CANCELLED_MARKER`。保持 tool_use/tool_result 配对不变量(同 RULE-A-007)。summary 落主对话 turn(复用 `db::persist_turn`)。

### 前端

- ToolCallCard 对 dispatch_subagent 特殊渲染:显示"子代理 `{name}` · {status}" + summary,可展开查询 `subagent_runs.transcript` 显示 worker 的 tool calls/thinking(新增 Tauri command `list_subagent_run` + store)。

## Requirements

- [R1] `dispatch_subagent` ToolDef 注册进 builtin_tools(供 LLM 发现);input:`{subagent: enum[researcher|general-purpose], task: string}`。但**执行在 agent loop 层拦截,不进 execute_tool_inner**。
- [R2] 代码内置 2 个 SubagentDef(researcher 只读 / general-purpose 全集减禁项),含 name/description/system_prompt/tools。
- [R3] `run_subagent` 嵌套调 `run_chat_loop`,worker context = `[memory_blocks, delegation_task]`(APPEND)。
- [R4] tool allowlist + 结构性禁项过滤(update_checklist / dispatch_subagent / L1a 三件)。
- [R5] `PermissionContext.is_worker` + worker Tier 4 ask→deny;继承 main mode。
- [R6] `run_chat_loop` 加第 18 参 `max_turns: Option<usize>`(None=50);worker 传 Some(20)。production + 9 测试传 None。
- [R7] `CancellationGuard` 加 `skip_session_active: bool`;worker rid 进 cancellations 但 guard 传 skip=true(不误删父 session_active_request)。
- [R8] `subagent_runs` 表 migration + worker 中间过程持久化(transcript)。
- [R9] summary 作为 tool_result 回填主对话,带 status 前缀;cancelled 用 CANCELLED_MARKER。
- [R10] token usage 汇总进父 session 累积。
- [R11] 前端 ToolCallCard dispatch_subagent 展开 UI + `list_subagent_run` command。

## Acceptance Criteria

- [ ] LLM 可通过 dispatch_subagent 派 researcher(只读)/ general-purpose worker,worker 跑独立 context,summary 回填主对话。
- [ ] worker context 与父正交:父 messages 不含 worker 中间过程;reload 主对话只看到 dispatch_subagent tool_call + summary tool_result。
- [ ] worker 中间过程可在 subagent_runs 查到,前端 ToolCallCard 可展开查看。
- [ ] main=yolo 时 worker 可写/shell;main=plan 时 worker 写/shell 被 deny(无 ask modal 弹出)。
- [ ] 用户 Stop 传播到 worker(worker cancel),dispatch_subagent tool_result 带 status=cancelled。
- [ ] **worker guard drop 不误删父 session_active_request**(父 chat 的 cancel_inflight_for_session / RULE-E-005 语义保持)—— 加 `skip_session_active` 回归测试。
- [ ] worker error(如 LLM stream error)→ tool_result 带 status=error,保持 tool_use/tool_result 配对。
- [ ] worker max_turns=20 兜底(Some(20) 传参,超限正确收尾)。
- [ ] prompt cache 不变量不被破坏(worker memory breakpoint 独立于父;summary APPEND 不 insert(0))。
- [ ] MockProvider 驱动的集成测试:worker 完整 turn + summary 回填 + cancel/error 路径 + guard 不误删。
- [ ] `cargo test --lib` 全 pass,0 warning;vitest 覆盖前端展开 store。

## Definition of Done

- `run_chat_loop` 加第 18 参 `max_turns: Option<usize>`(语义参数,非 hack);production + 9 测试传 None 保持 50(对齐 RULE-A-006 单一权威)。顺手修 `chat_loop.rs:83` docstring "14-parameter"→"17-parameter"(源码注释过时)。
- prompt cache 不变量 APPEND 规则贯穿(B12 + L1a 同款约束)。
- worker cancel/error 状态正确回填为 tool_result,保持配对。
- CancellationGuard skip_session_active 防误删,回归测试覆盖。
- Rust 单元 + 集成测试 + 前端 vitest 全绿,0 warning。
- spec 沉淀:`.trellis/spec/backend/tool-contract.md` "dispatch_subagent" scenario + `agent-loop-architecture.md` subagent 段(含 max_turns 第 18 参 + skip_session_active + PermissionContext.is_worker);DEBT.md 不新增债(或登记 follow-up)。

## Out of Scope (explicit)

- 异步 fan-out 并行 worker(`dispatch_subagents` plural)—— v2 / L3,复用 L1a drain_notifications。
- worker 嵌套(worker 派 worker)—— MVP 禁嵌套(对标 Cline)。
- Markdown frontmatter subagent 定义加载(对标 `.claude/agents/*.md`)—— v2,复用 resource_loader。
- Claude Code fork 模式(继承父完整历史 + 共享 cache)—— v2 高级档(需实测 Anthropic API 嵌套调用下 cache_control 复用)。
- OpenHands `LLMSummarizingCondenser` 主动摘要 —— 未来增强。
- worker 独立 model(每个 subagent 换模型)—— MVP 复用父 provider。
- summary 结构化强制(file:line 收尾)—— MVP free-text。
- worker transcript 实时流可见(进行中)—— MVP 完成后展开。

## Technical Notes

- 前置债务已清(RULE-A-008 / D-004 / D-005)2026-06-18 closed,无阻塞。
- 前置先例:B12 checklist(per-request handle + ephemeral APPEND)、L1a background shell(跨 turn 状态 + APPEND 通知)、L2 并行 task(FuturesUnordered + result_slots 回填)。
- prompt cache 陷阱是本项目反复踩的坑(B12 + L1a),B6 summary 注入必须 APPEND。
- worker 无 UI sink 是与 main 的关键差异,permission Tier 4 ask 不能透传(§Decisions 3/5)。
- worker rid 不进 session_active_request 的原因:该 map 是 session→request 1:1,worker 嵌套会覆盖父映射,破坏 cancel_inflight_for_session / RULE-E-005 语义;叠加 CancellationGuard 默认会 remove session_active_request[session_id] → 误删父(§Review #2)。
- DB migration 风险:subagent_runs 表加列,走现有 migrations.rs 模式(v7 递增)。
- dispatch_subagent 在 L2 并行集合外(不在 `is_parallel_eligible` 的 read-only 集合),天然走串行拦截路径,不与 L2 冲突。

## Implementation Plan (small PRs)

- **PR1(后端核心 + 基础设施)**:
  - `run_chat_loop` 加第 18 参 `max_turns: Option<usize>`(production + 9 测试传 None)+ 修 docstring 14→17。
  - `CancellationGuard` 加 `skip_session_active: bool`。
  - `PermissionContext` 加 `is_worker: bool`。
  - `dispatch_subagent` ToolDef + SubagentDef(researcher/general-purpose)+ `assemble_subagent_prompt`。
  - agent loop 层拦截 dispatch_subagent(不进 execute_tool_inner)→ `run_subagent` 嵌套 `run_chat_loop`(worker rid 进 cancellations + guard skip_session_active=true + PermissionContext.is_worker=true + max_turns=Some(20))。
  - tool allowlist + 结构性禁项过滤;worker context = [memory, task](APPEND)。
  - summary tool_result 回填(status 前缀)。worker 中间过程 SubagentBufferSink in-memory(**不含** DB 表)。
  - MockProvider 集成测试:worker 完成 / cancel / error / **guard 不误删父 session_active_request**。
- **PR2(持久化)**:`subagent_runs` migration + worker 中间过程落 transcript + status/summary/token 落表 + token 汇总进父 session + audit 不污染父(C4 audit log 验证)。
- **PR3(前端 + spec)**:`list_subagent_run` Tauri command + 前端 ToolCallCard dispatch_subagent 展开 UI + store + spec 沉淀(tool-contract.md / agent-loop-architecture.md,含 max_turns/skip_session_active/is_worker)。

---

## Review 修订(deepseek-v4-pro, 2026-06-19)

来源:`docs/_review/b6-subagent-prd-review.md`。评审核实源码后提出 6 项,全部采纳:

| # | 问题(核实结论) | 采纳方案 |
|---|---|---|
| 1 | "14 parameters"全文过时(**核实:实际 17 参**,`chat_loop.rs:96-128`;源码 docstring `:83` 自己也错写 14) | PRD 全文 14→17;PR1 顺手修 docstring。 |
| 2 | CancellationGuard 双重 remove(**核实属实**:`state.rs:416-417 drop` 会 `session_active_request.remove(session_id)`,worker 复用 parent_session_id 误删父映射,破坏 RULE-E-005) | `CancellationGuard` 加 `skip_session_active: bool`,worker 传 true(方案 A)。 |
| 3 | dispatch_subagent 依赖传递(**核实属实**:`execute_tool_inner`(`tools/mod.rs:172`)只 6 参,拿不到 provider/db/cancellations 等) | **agent loop 层拦截**,不进 execute_tool_inner;`run_subagent` 拿 run_chat_loop 闭包依赖(方案 A)。 |
| 4 | max_turns 决策悬而未决(**核实属实**:`chat_loop.rs:492 for turn in 1..=MAX_TURNS` 硬编码 const,不改签名无法传 20) | 加第 18 参 `max_turns: Option<usize>`(None=50)。 |
| 5 | worker permission 检测模糊(**核实:PermissionContext 已存在**,`chat_loop.rs:55` import) | 明确 `PermissionContext.is_worker: bool` 字段。 |
| 6 | worker context 构造细节缺失 | 明确 messages = `[memory_blocks(cache_control), delegation_task]`,memory 在前享 cache。 |

评审同时确认 7 项设计亮点正确(嵌套不改核心逻辑 / APPEND / worker rid 不进 session_active_request / 结构性禁项 / SubagentBufferSink / 3-PR / MockProvider 测试),不偏离。调研改进建议(fork cache 实测、LLMSummarizingCondenser 实现约束)记入 OOS 作为 v2 caveat。
