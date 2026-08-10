## Scenario: dispatch_subagent tool (B6 PR1, 2026-06-19)

### 1. Scope / Trigger

- Trigger: main agent 在 turn N 通过 LLM `tool_use` 派一个 worker subagent 跑**独立 context**(独立 messages + 独立 token 预算 + 独立 turn 上限),完成后 worker final summary 回填为 dispatch_subagent 的 `tool_result`。对标 Claude Code Task tool / OpenHands TaskToolSet。
- **不是普通 I/O tool** —— 是 agent 层控制流工具。注册为 `ToolDef` 供 LLM 发现 + 走 ⑨ 关权限 check,但**执行不走** `execute_tool_inner`(拿不到 `provider` / `db` / `cancellations` 等依赖),在 `chat_loop.rs` tool_use 处理循环**拦截** → 直接调 `run_subagent(deps..., input, ctx)`(REVIEW-SUBAGENT-PRD #3 核实的真实约束)。
- ROADMAP §4.1 "B6 = Subagent(harness 学习价值高)" + ROADMAP §1.2 计划项。

### 2. Signatures

#### Tool declaration(`app/src-tauri/src/agent/subagent/mod.rs::definition()`)

> **L3d PR3 (2026-06-26) 动态化**:dispatch_subagent **不再**注册进 `builtin_tools()` 启动快照(`state.tools`,由 `AppState::load()` 启动调一次固化)。production 用 `definition_with_cache(cache: &SubagentCache, project_path: &str) -> ToolDef`(async),每 turn 在 `chat_loop.rs` turn tool list 构造处(`filter_tools_for_mode` 之后)调用 —— `enum` 从 `cache.list(project_path).await` 取所有 subagent 名(builtin + user + project 按优先级合并),`description` 末尾追加 `Available subagents: <name> (source: <builtin|user|project>): <desc>; ...`。.md 改动经 mtime fence 下次 chat 自动生效(无 reload 命令)。下面的静态 `definition()` 保留供单元测试;`tools/mod.rs::builtin_tools()` 现只含其他 12 个工具。no-nesting 防护见 §3 callout。

```rust
ToolDef {
    name: "dispatch_subagent",
    description: "Dispatch a worker subagent to run a sub-task in its own isolated context \
                  (independent messages, independent turn budget). The worker runs to \
                  completion (synchronous — the parent chat blocks until the worker returns). \
                  When the worker finishes, its final summary is injected as the tool_result. \
                  Two built-in subagents: `researcher` (read-only: read_file / grep / glob / \
                  list_dir / web_fetch) and `general-purpose` (full toolset minus \
                  dispatch_subagent / update_checklist / background-shell tools). Worker \
                  inherits parent's permission Mode: Yolo → all-allow; Edit/Plan → a tool \
                  needing confirmation (writes, shells, web_fetch without a prior grant) \
                  surfaces a `WorkerAskBanner` in the parent's UI for allow/deny (120s \
                  timeout denies; since 2026-06-22 RULE-FrontSubagent-003).",
    input_schema: {
      "type": "object",
      "properties": {
        "subagent": {"type": "string", "enum": ["researcher", "general-purpose"]},
        "task":     {"type": "string"}
      },
      "required": ["subagent", "task"]
    },
}
```

#### `run_chat_loop` 嵌套调用(worker 路径,`chat_loop.rs::run_chat_loop`)

```rust
Box::pin(run_chat_loop(
    worker_tool_defs,                        // 1
    provider.clone(),                        // 2
    context_window,                          // 3
    worker_rid,                              // 4: "{parent_rid}-sub-{seq}"
    parent_session_id.to_string(),           // 5: 复用父 session_id
    worker_messages,                         // 6: [memory_blocks, delegation_task]
    worker_sink_dyn,                        // 7: SubagentBufferSink
    db.clone(),                              // 8
    cancellations.clone(),                   // 9: worker rid 注册(不进 session_active_request)
    _session_active_request.clone(),         // 10: 复用父 map(不修改)
    read_guard.clone(),                      // 11
    memory_cache.clone(),                    // 12
    skill_cache.clone(),                     // 13
    permission_asks.clone(),                 // 14
    worker_token,                            // 15
    None,                                    // 16: resend_seq
    background_shells.clone(),               // 17
    Some(SUBAGENT_MAX_TURNS),                // 18: 200
    true,                                    // 19: skip_session_active(REVIEW-SUBAGENT-PRD #2)
    true,                                    // 20: skip_persist(worker 中间过程不进 DB)
    Some(true),                              // 21: is_worker(RULE-A-014; worker asks route via WorkerAskBanner since 2026-06-22)
    app_handle,                              // 22: 转发父 AppHandle,worker SubagentBufferSink 可走 subagent:event IPC(测试 None)
    Some(assemble_subagent_prompt(def, task)), // 23: worker 覆写父 system_prompt(B6 review defect A 修复)
)).await;
```

#### `run_chat_loop` 5 个新参数(PR1a + PR1b + PR2b + PR3 + 06-21 fix)

| # | 参数 | PR | 用途 |
|---|---|---|---|
| 18 | `max_turns: Option<usize>` | PR1a | worker turn 上限(None = 50 默认)。`turn_limit = max_turns.unwrap_or(MAX_TURNS)` |
| 19 | `skip_session_active: bool` | PR1b | `CancellationGuard::drop` 时跳过 `session_active_request.remove(session_id)`。worker 传 `true` 避免误删父映射(REVIEW-SUBAGENT-PRD #2 / RULE-E-005 不破坏) |
| 20 | `skip_persist: bool` | PR1b + PR2a fix | run_chat_loop 函数体内 **16 处**(PR1 spec 写 18,PR2a RULE-A-015 拆出 2 处:`add_token_usage` 应 streaming 累加进父 sessions 表,不在 messages 表 UNIQUE 范围内;terminal `Done` emit 必经 sink 才能让 `was_cancelled` 正确 catch) `if !skip_persist { ... }` gate 守住所有 persist 站点(persist_turn / update_message_metadata / touch_session / record_*_audit / persist_turn_cwd)。worker 传 `true` 避免与父 `messages` 表 `(session_id, seq)` UNIQUE 约束冲突;worker 中间过程由 `SubagentBufferSink` transcript 捕获(PR2 落 `subagent_runs.transcript_json`) |
| 21 | `is_worker: Option<bool>` | PR2b (RULE-A-014) | worker 路径传 `Some(true)`,`run_chat_loop` 内部构造 `PermissionContext { is_worker: true }`。Pre-2026-06-22 让 Tier 4 `ask_path`/`ask_shell` 顶部 `if ctx.is_worker { Decision::Deny }` 立刻拒绝(worker 无 UI sink,弹 modal 会挂起到 user Stop);**2026-06-22 RULE-FrontSubagent-003 后** worker ask 走 `WorkerAskBanner` round-trip(见 permission-layer.md §5b:biased select over parent cancel / 120s timeout / oneshot),`is_worker` 现主要用于 (a) ask 内部 session key `"worker:{run_id}"` 隔离 + (b) 阻止 worker `AllowAlways` 持久化进父 `session_tool_permissions`(跨权限边界)。production + 35 个 `agent_loop_*` 集成测试传 `Some(false)` 显式声明非 worker;None 默认 `false`(向后兼容) |
| 22 | `app_handle: Option<AppHandle>` | PR3 (PR2 hotfix) | 转发父 AppHandle,worker SubagentBufferSink 才能 emit `subagent:event` IPC channel(否则 PR3 drawer 看不到 worker transcript live streaming)。production 传 `Some(app.clone())`,tests 传 `None`(无 Tauri runtime,emit 路径变 no-op) |
| 23 | `system_prompt_override: Option<String>` | 06-21 fix (B6 review defect A) | worker 路径传 `Some(assemble_subagent_prompt(def, task))`,让 worker 真正使用 `SubagentDef.system_prompt` —— pre-fix `_worker_system_prompt = assemble_subagent_prompt(def, task)` 是 dead code in `chat_loop.rs::run_chat_loop`,worker 实际拿到父的 `assemble_system_prompt(mode_prefix, base_prompt)` 输出,导致 prompt / permission 矛盾(pre-2026-06-22 Edit/Plan 模式下 worker prompt 写"可写"但 Tier 4 把写工具 collapse 到 `Deny`;2026-06-22 后写工具走 `WorkerAskBanner`)。fix 后 `run_chat_loop` 内部守卫:`Some(p)` → 直接用 `p`;`None` → 走原有 `assemble_system_prompt(mode_prefix, base_prompt)`(production + 36 tests 路径)。4 指令文件 prompt caching 不受影响(cache_control breakpoint 在 user role,跟 system 正交) |

### 3. Contracts

#### `SubagentDef` registry + 三层来源(`agent/subagent/mod.rs::builtin_subagents` builtin + `agent/subagent/cache.rs::SubagentCache` user/project(08-07 拆分))

> **L3d (2026-06-26)**:`SubagentDef` 字段已 owned 化(`name`/`description: String`,`tools: Vec<String>`,PR1)。builtin 之上新增 user 层(`~/.config/everlasting/agents/*.md`)+ project 层(`<project>/.everlasting/agents/*.md`),由 `SubagentCache`(read-through mtime fence,照搬 B3 `CommandCache`)合并,优先级 **project > user > builtin**(last-write-wins)。frontmatter schema(`name`/`description`/`tools`/`model`/body)+ 错误处理(per-file isolation,silent skip + warn,**无 fail-fast**)+ `tools` 继承语义(覆盖 builtin 同名且未声明 tools → 继承 builtin tools;全新 agent 未声明 → `vec![]` 全工具集)详见 [ROADMAP §1.2 L3d 已实施条目](../../../../docs/ROADMAP.md#12-路线图外完成)(**原设计 PRD `docs/subagent-loader.md` 已删除**,实施后归档,2026-06-26)。`model` 字段 v1 解析但 warn-ignored(`Provider` trait 单实例模型,不切换)。builtin 两个定义见下表。

| name | `tools` allowlist | system_prompt |
|---|---|---|
| `researcher` | `[read_file, grep, glob, list_dir, web_fetch]` | "你是只读研究子代理...Cannot edit/write/shell,不能嵌套 dispatch...(06-25 起含 web_fetch)" |
| `general-purpose` | `[]`(全集减结构性禁项) | "你是通用子代理...minus dispatch_subagent / update_checklist / background-shell..." |

#### `filter_tools_for_subagent(builtin_tools, def)`(`subagent.rs`)

1. `def.tools.is_empty()` → 起点全集(general-purpose 模式);否则 → 起点 allowlist
2. **`STRUCTURALLY_DISABLED` 永远 strip**(无论 allowlist 怎么写):
   - `update_checklist`(main 进度表,worker 写会污染)
   - `dispatch_subagent`(禁嵌套,对标 Cline)
   - `run_background_shell` + `shell_status` + `shell_kill`(L1a session 级通知注入,worker 无 sink)
3. 测试 `filter_strips_structurally_disabled_even_if_allowlist_lists_them` 锁定(防御未来 frontmatter 定义误开禁项)

> **⚠️ no-nesting 真实机制(L3d PR3, 2026-06-26 修正)**:`dispatch_subagent` 在 `STRUCTURALLY_DISABLED` 里是 **defense-in-depth,不是主机制**。PR3 起 dispatch_subagent 不再注册进 `builtin_tools()` 启动快照(见 §2),而是**每 turn 由 `definition_with_cache(&SubagentCache, project_path)` 动态 append** 到 turn tool list —— 此 append 在 parent/worker 共享的 `run_chat_loop` body 内,若不区分,worker nested 调用会同样 append → worker LLM 看得到 dispatch_subagent → **可嵌套**(PR3 check 发现的 BLOCKING 安全回归,单测全绿是因为没人断言 worker turn 的 tools 内容)。真正防嵌套的是 `chat_loop.rs` 的 `if !effective_is_worker { push definition_with_cache }` gate(worker 跳过 append);`filter_tools_for_subagent` 只作用于 seed list(`builtin_tools()`),而 dispatch_subagent 已不在 seed list,故 filter 对它是冗余兜底。
>
> **Forbidden Pattern**:在 parent/worker 共享的 `run_chat_loop` body 内 append 动态 tool(或任何结构性禁项 tool),**必须**用 `effective_is_worker` gate 区分;只靠下游 `filter_tools_for_subagent` 不够 —— filter 只过滤 seed list,不过滤 per-turn append。回归测试 `agent_loop_dispatch_subagent_completes_and_returns_summary` 用 `MockProvider::sent_tools()` 断言 worker turn(slot 1)收到的 tools 不含 `dispatch_subagent`。

#### worker context(`subagent.rs::build_worker_messages`,`chat_loop.rs::run_chat_loop` 传入)

- `messages[0]` = `build_instructions_blocks(memory_cache)` synthetic user message(4 文件:User/Project × CLAUDE.md/AGENTS.md,带 `cache_control: Ephemeral`,worker **自己** cache breakpoint,与父正交)
- (可选) `messages[1]` = synthetic assistant ack("Understood. I will follow these instructions...")—— 镜像 main loop 的 memory pair 保持 Anthropic wire user/assistant 交替
- 末尾 `messages.push` delegation task user message(**APPEND,不 prepend**)

**prompt cache 不变量**(B12 + L1a 两次踩过的坑锁死):worker `messages[0]` 与父 `messages[0]` 正交,不污染父 cache key。summary 注入主对话走 `ContentBlock::ToolResult`(天然末尾),绝不 `insert(0)`。

#### `SubagentBufferSink`(`subagent.rs`,实现 `ChatEventSink` trait)

- `transcript: Mutex<Vec<TranscriptEntry { kind, payload_json }>>` —— 累积 worker 的 chat-event/tool:call/tool:result,PR2 落 `subagent_runs.transcript_json`
- `text_parts: Mutex<Vec<String>>` —— Delta 事件累积,`final_text()` 拼成 summary
- `had_error: AtomicBool` —— `ChatEvent::Error` → true
- `was_cancelled: AtomicBool` —— `Done{stop_reason: cancelled}` → true(`max_turns` 不算 cancel,归 Completed)
- **不 forward 父 sink** —— 否则 main UI 被 worker 流刷屏(Claude Code 约定:中间过程对 main 隔离)

#### `format_dispatch_result(status, worker_text, partial_actions)`(`subagent.rs`)

| status | content | is_error |
|---|---|---|
| `Completed` | `[status: completed]\n<summary>`(空文本回退 `(worker produced no final text)`) | false |
| `Cancelled` | `[status: cancelled]\n<text>\n\n[CANCELLED_MARKER]`(空文本退化为 `marker alone`) | true |
| `Incomplete` | `[status: incomplete]\n<text>\n\n[INCOMPLETE_MARKER]`(`[未完成]`,空文本退化为 `marker alone`) | true |
| `Error` | `[status: error]\n<error text>` | true |

**task 07-03-subagent-frontmatter-model + 07-03-subagent-per-agent-model-ui (2026-07-03)**:生产路径改用 `format_dispatch_result_with_model(status, worker_text, partial_actions, model_display)`(4 参)。`model_display: Option<&str>` = `Some(name)` 时在 `[status: ...]` 后插 `\n[model: <name>]` 行(让 parent LLM 看到 worker 实际用的模型 —— 跨模型对抗 review 的关键可见性);`None`(`resolve_worker_provider` parent 继承 / catalog miss 降级)省略该行,wire 形态 = 旧 3 参。3 参 `format_dispatch_result` 保留为 `#[cfg(test)]` back-compat shim(11 个既有断言零改动)。worker provider / ctx / display 由 `run_subagent` 经两级纯函数解析:

1. **`resolve_final_model(db, name, frontmatter_model) -> Option<String>`** — 优先级 `DB override > frontmatter > None`(B6+ C 决策,见 `commands::subagents` IPC 与 `subagent_model_overrides` 表)。`None` 时走 `resolve_worker_provider` 的 parent 继承分支。catalog 命中失败(catalog miss)时 `resolve_worker_provider` 内 `warn!` + 降级 parent —— **不**回退到 frontmatter(frontmatter 是声明,不是 fallback;UI 表面无效 override 时标红"model 已删除")。
2. **`resolve_worker_provider(model, parent_provider, parent_ctx, catalog, db) -> (Arc<dyn Provider>, u32, Option<String>)`** — A 任务的 6 个既有单测零改动(B6+ C 改的是调用方优先级,不破 resolver 本身)。

catalog(`AppState.catalog`,`Arc<RwLock<HashMap<model_id, Arc<dyn Provider>>>>`)经 `app_subagent_catalog(&app_handle)`(`tauri::Manager::try_state::<AppState>()`)取 —— **`run_chat_loop` 23 参签名不动**,catalog 在 loop body 内现取后传 `run_subagent`(3 个调用点:forced dispatch / concurrent batch / serial interceptor)。frontmatter `model:` 字段(loader.rs)从 A 任务的 Q4 warn+discard 翻转为 store;值 = `models.id`(UUID)。B6+ C 加 `subagent_model_overrides` 表(全局 `agent_name → model_id`)让 builtin agent 也能在 Settings UI 里配,无需 frontmatter 文件。

**可观测性(AC13)**:worker dispatch 完成后,`run_subagent` 把 `resolve_worker_provider` 的 `Option<String>` display **直接**写入 `subagent_runs.model_display`(`insert_run_with_id` 末参,见 `subagent-runs-schema.md` "B6+ C additions")。语义与 `format_dispatch_result_with_model` 的 `[model:]` 行**完全一致**:`None` 同步省略行 + 写 NULL。**不改 `run_subagent` 签名** —— parent display 不 thread(留 follow-up)。前端 `card` + `drawer` 据此显示 chip / inline model(parent 继承 / catalog miss / pre-C 旧 row 一律 `v-if` 隐藏,不报错)。

**task 07-06-b6plus-b-dispatch-model-arg (2026-07-07, B6+ B)**:per-dispatch model override 把优先级链延伸到 **`dispatch > DB > frontmatter > parent`**。两条入口汇合到 `run_subagent` 的 `input.model` 字段:
- **LLM path**:`dispatch_subagent` tool schema 加 `model` 属性(optional string,**enum 值 = display_name**,动态构建 —— `definition_with_cache(cache, project_path, models: &[ModelBrief])` 的 enum 从 `models.iter().map(|m| m.display_name)` 取;`chat_loop.rs` 在 turn loop 外 `list_models` 快照成 `Vec<ModelBrief>` 一次喂 `definition_with_cache`)。LLM 传 display_name,后端反查。
- **user path**:`@@agent --model=<X> <task>` 前缀,前端 `resolveModelInput(raw, models)` 反查(id 精确 / display_name 取首 + `console.warn`),wire 只认 id。flag 位置必须紧跟 `@@agent` 之后、task 之前(git/cargo 语义;task 中间的 `--model=` 不误解析)。

`run_subagent` 解析:`dispatch_model = input.get("model")` → `resolve_model_by_name_or_id(db, input)`(新纯函数:① `get_model` 精确 id 直返;② `list_models` find `display_name == input` 取首;③ miss → `Ok(None)`)→ 叠加 `final_model = dispatch_model.clone().or(resolved_lower)`(resolved_lower = `resolve_final_model`)。`resolve_final_model` / `resolve_worker_provider` / `format_dispatch_result_with_model` 签名与既有测试**全不动**(B 叠加在上游)。`ForcedDispatch` 加 `model_id: Option<String>`(`#[serde(default)]`,wire snake_case —— 嵌套 IPC struct 字段经 serde verbatim,**不**像顶层 Tauri arg 那样 auto-camel;前端发 `model_id` 非 `modelId`)。失效兜底:dispatch model 不在 catalog(被删 / display_name 拼错反查无果)→ `warn!` + `dispatch_model=None` → 走 `resolve_final_model`(AC7)。会话内 CRUD model 的 enum 滞后(下 chat 会话刷新),catalog 实时(dispatch 时反查最新 DB),故 enum 滞后只影响"选项可见性",不影响解析正确性 + 失效兜底覆盖。

**RULE-BackSubagent-001 (2026-06-22)**:`partial_actions: Option<&str>` 对**非 completed 三态**(Cancelled / Incomplete / Error)在 body 之后 append `\n\nWorker partial actions:\n<summary>` 段,让 parent LLM 看到 worker 已执行的 tool_call 摘要(`- {name}({key_param}): ok|failed|?`),做补偿修复(跳过已落地 write/edit、重试 failed tool)。摘要由 `summarize_worker_tool_actions(transcript_snapshot)` 构建(tool_call/tool_result 按 `tool_use_id` 配对,orphan tool_call 标 `?`;`chat_event`/`permission_ask` 跳过;2 KiB head+tail cap,超限 `(N actions omitted)` 计数)。`Completed` 传 `None`;空摘要(worker 未执行任何 tool_call)也传 `None`,不产生空标题。摘要**只进 tool_result wire**(parent LLM 消费),**不进** `subagent_runs.final_text`(drawer 已有完整 Tools 段,避免冗余 —— `format_final_text` 不变)。

terminal `Done{cancelled}` 事件**不**守 `skip_persist` —— worker `SubagentBufferSink.was_cancelled` 仍能正确捕捉,只 DB writes(cwd / touch_session)守门。

### 4. Validation & Error Matrix

| 条件 | 行为 |
|---|---|
| `subagent` 不在 enum | LLM 错(input schema 校验拦在前面) |
| `subagent_cache.lookup(project_path, name)` 返 None(L3d PR3 起,替代 `lookup_subagent`) | 拦截点合成 `tool_result` `[status: error]\nunknown subagent: <name>. Available: <cache.list 全部名字>`,`is_error: true`,**tool_use/tool_result 配对保持**(同 RULE-A-007) |
| worker turn 超 `SUBAGENT_MAX_TURNS=200` | `Done{stop_reason: max_turns}` → status=**Incomplete**(soft,"ran out of budget"),summary 仍带 worker 产出 + `INCOMPLETE_MARKER` `[未完成]` 标记(R2: 06-21 task 把 max_turns 终止从 Completed 改为 Incomplete) |
| 用户 Stop 传播到 `worker_token`(child of `parent_token`) | `Done{stop_reason: cancelled}` → status=Cancelled + `CANCELLED_MARKER` |
| worker LLM stream error | `ChatEvent::Error` → SubagentBufferSink.had_error → status=Error |
| parent 复用 `session_id` + guard `skip_session_active=true` | worker Drop **不** evict 父 `session_active_request[parent_session_id]`(回归测试 `dispatch_subagent_guard_does_not_evict_parent_session_active`) |
| worker 内写 messages 表(`skip_persist=true`) | **16 处** gate 全部拦下(PR1 spec 18,PR2a RULE-A-015 拆出 2 处:terminal Done emit / add_token_usage streaming 累加父 sessions),worker 中间过程不进父 DB |

### 5. Good / Base / Bad Cases

**Good**:parent turn 1 LLM 派 `researcher`("找出所有引用 `dispatch_subagent` 的文件")→ researcher 跑 `read_file`/`grep`/`list_dir`(4 路径全 silent allow)→ final text "found 3 files: ..." → `format_dispatch_result(Completed, ...)` → parent 构造 `ContentBlock::ToolResult`,tool_use/tool_result 配对,parent turn 2 继续。

**Base**:parent turn 1 LLM 派 `general-purpose` 改文件 + main=yolo(继承 yolo,写/shell Tier 4 bypass 早返回 Allow)→ worker 跑 `write_file` + `shell`(无 ask modal 阻塞)→ final text "已修改 3 个文件: ..." → Completed。

**Bad**:parent turn 1 LLM 派 `general-purpose` + main=Edit + `write_file`(触发 Tier 4 ask)→ **RULE-A-014 修复前(B6 PR1b)**:worker 路径构造了 `_worker_permission_ctx { is_worker: true }` 但未 thread 进嵌套 `run_chat_loop`,run_chat_loop 内部从 session row 重建 `PermissionContext { is_worker: false }` → `ask_path` 顶部 `if ctx.is_worker { Deny }` 在嵌套路径不可达 → emit `permission:ask` 等 oneshot(永远等不到,worker 无 UI sink)→ **挂起直到 user Stop**。**RULE-A-014 修复后(B6 PR2b)**:worker 路径传 `is_worker=Some(true)` 给嵌套 `run_chat_loop`,loop 内部 `effective_is_worker = is_worker.unwrap_or(false) = true` → `PermissionContext { is_worker: true }` 构造成功 → Tier 4 `ask_path` 顶部立即 `Decision::Deny`,无 oneshot 等待,无挂起;tool_result `is_error=true` + deny 原因回 LLM 自我纠错。**RULE-A-016 修复后(B6 PR3a 2026-06-20)**:worker deny 不再写父 `session_audit_events`(改走 sink → transcript PermissionAsk entry,见 §3 "audit 不污染父的分工")。回归测试 `agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied`(`tokio::time::timeout(15s)` 包裹,若 PR2b 修复回退则卡 oneshot 触发 15s 超时 fail)。**B6 review defect A 修复后(2026-06-21)**:worker 路径额外传 `system_prompt_override=Some(assemble_subagent_prompt(def, task))`,让 worker 真正使用 `SubagentDef.system_prompt`(pre-fix `_worker_system_prompt` 是 dead code)。修后 worker prompt 写"可写"时即真正可写(yolo)、写"只读"时即 read-only(researcher),prompt / 权限行为一致。

### 6. Tests Required

**Unit**(`subagent.rs::tests`,~17 个):

| Test | 断言 |
|---|---|
| `definition_has_correct_name` | `ToolDef.name == DISPATCH_TOOL_NAME` |
| `definition_schema_requires_subagent_and_task` | `input.required` 含两字段 |
| `definition_schema_subagent_enum_covers_two` | enum == `["researcher", "general-purpose"]` |
| `builtin_subagents_has_two_entries` | registry 长度 2 |
| `builtin_subagents_researcher_tool_allowlist` | researcher.tools == 4 只读件 |
| `builtin_subagents_general_purpose_empty_allowlist` | general-purpose.tools.is_empty() |
| `lookup_subagent_unknown_returns_none` | unknown name → None |
| `filter_researcher_keeps_only_read_tools_and_strips_disabled` | researcher + 禁项全 strip |
| `filter_general_purpose_keeps_full_set_minus_disabled` | general-purpose 保留写/shell,strip 禁项 |
| `filter_strips_structurally_disabled_even_if_allowlist_lists_them` | 即便 allowlist 列了禁项也强制 strip |
| `buffer_sink_accumulates_text_deltas` | Delta 事件累积成 summary |
| `buffer_sink_tracks_cancelled_done` | `Done{cancelled}` → was_cancelled |
| `buffer_sink_tracks_error_event` | `Error` → had_error |
| `buffer_sink_records_transcript_entries` | 3 类 emit 进 transcript |
| `format_completed_with_summary` / `_empty_text_falls_back_to_note` | Completed 两种格式 |
| `format_cancelled_includes_marker` / `_empty_text_uses_marker_alone` | Cancelled 两种格式 |
| `format_error_includes_status_prefix` | Error 格式 |
| `format_dispatch_result_appends_partial_actions_when_some` / `_none_..._no_section` / `_empty_..._no_section` | RULE-BackSubagent-001:`Some`(非空) append `Worker partial actions:` 段;`None` / `Some("")` 不 append |
| `summarize_*`(pairs/key_param/unknown/skips/order/under_cap/head_tail/empty/no_tool_calls) + `key_param_truncates_long_values` | summarize 配对 ok/failed/? + per-tool key_param + head+tail cap + 跳过非 tool kind + 多字节截断 |

**Integration**(`agent/tests.rs::agent_loop_dispatch_subagent_*`,5 个):

| Test | 断言 |
|---|---|
| `dispatch_subagent_completes_and_returns_summary` | parent turn 1 dispatch_subagent tool_use → worker 跑 → summary tool_result `[status: completed]` + worker text;主对话 `phantom_worker_text == 0`(worker 中间过程**不**进父 messages) |
| `dispatch_subagent_cancel_propagates_to_worker` | parent_token cancel → worker_token child 触发 → status=cancelled + `CANCELLED_MARKER`;tool_use/tool_result 配对保持 |
| `dispatch_subagent_error_returns_status_error` | MockProvider stream error → status=error;tool_use/tool_result 配对保持 |
| `dispatch_subagent_error_includes_partial_transcript_summary` | RULE-BackSubagent-001:worker 先执行 `read_file` 再 stream error → status=error + tool_result content 含 `Worker partial actions:` 段 + `read_file(` 摘要行 |
| `dispatch_subagent_guard_does_not_evict_parent_session_active` | 用 `HangingThenCancel` worker(500ms 延迟 cancel)保住 worker 在飞,snapshot 验证父 `session_active_request[parent_session_id]` 仍正确(worker Drop 因 `skip_session_active=true` 不误删) |

### 7. Wrong vs Correct —— 拦截路径(execute_tool_inner vs chat_loop loop)

#### Wrong:`dispatch_subagent` 走 `execute_tool_inner`

```rust
// tools/mod.rs::execute_tool_inner
match name {
    "dispatch_subagent" => dispatch_subagent::execute(input, ctx, ...).await,
    // ...
}
```

**Why it's wrong**:`execute_tool_inner` 签名 `(name, input, ctx, guard, session_id, skill_cache, cancel)` 拿不到 `provider` / `db` / `cancellations` / `session_active_request` / `read_guard` / `memory_cache` / `permission_asks` / `background_shells`,而 `run_chat_loop` 嵌套调用需要全部(REVIEW-SUBAGENT-PRD #3 核实)。即使把它们塞进 `ToolContext`,会模糊工具层和 agent 层边界;`Box<dyn Any>` extension point hacky。

#### Correct:agent loop 层拦截

```rust
// chat_loop.rs tool_use 处理循环(约 :1380)
if tool_name == DISPATCH_TOOL_NAME {
    // 不走 execute_tool;直接调 run_subagent(拿到全部 run_chat_loop 闭包依赖)
    let (content, is_error, _cancel_parent, _exit_code) =
        run_subagent(/* 全部闭包依赖 */, tool_input, ctx).await;
    // 构造 ContentBlock::ToolResult 回填(配对)
    result_blocks.push(ContentBlock::ToolResult { tool_use_id, content, is_error });
    continue;
}
// 其他 tool 走原 execute_tool 路径
let (out, is_err, update, exit_code) = execute_tool(name, input, ...).await;
```

### 8. Design Decisions

#### Decision: 同步阻塞 MVP,异步 fan-out 留 v2 / L3

- **Context**: Claude Code `background: true` 字段区分前/后台;OpenHands `DelegateTool`(并行)vs `TaskToolSet`(同步)两个独立工具。
- **Decision**: MVP `dispatch_subagent` 同步阻塞(main 在 execute 里 await worker)。main UI 在 worker 跑期间不刷新,worker 完成后 summary 一次性回填。
- **Why**: 与本项目 L1a background shell 的"返回 handle + 下一轮 APPEND notification"模式正交;MVP 最小结构;异步 fan-out 是 v2 / L3 增强。
- **Future**: 加 `dispatch_subagents`(plural)并行 fan-out + 完成通知走 L1a `drain_notifications` 机制。

#### Decision: tool allowlist + 结构性禁项双层过滤

- **Context**: Claude Code `tools` allowlist + `disallowedTools` denylist;Cline 硬编码 6 只读件。
- **Decision**: allowlist + `STRUCTURALLY_DISABLED` 硬编码 5 项永远 strip(无论 allowlist 怎么写)。
- **Why**: allowlist 表达"worker 能用什么";`STRUCTURALLY_DISABLED` 表达"无论什么都不能用"(跨 subagent 类型的安全边界,防止未来 frontmatter 定义误开禁项)。`filter_strips_structurally_disabled_even_if_allowlist_lists_them` 测试锁定。

#### Decision: CancellationGuard 加 `skip_session_active` 字段(不复用等价证明)

- **Context**: 现有 `CancellationGuard::drop` 固定 `remove(rid) + remove(session_id)`。worker 复用 parent_session_id 时 Drop 会误删父 `session_active_request[parent_session_id]`,破坏 RULE-E-005 / `cancel_inflight_for_session`。
- **Decision**: 加 `pub skip_session_active: bool` 字段;Drop 包 `if !skip_session_active { ... }`。production chat 传 `false`(行为不变);worker 传 `true`。
- **Why(选 A 不选 B/C)**:A 干净,签名 `CancellationGuard { ..., skip_session_active: bool }`,B 让 worker 不创建 guard 手动管理清理脆弱(多个 cleanup site 易漏),C 用 dummy `session_active_request` map 浪费(且与真实 map 行为可能 drift)。

#### Decision: `skip_persist` 守 16 处 persist 站点(worker 中间过程不进 DB,PR1 18 → PR2a 16)

- **Context**: worker 复用 parent_session_id,直接调 `persist_turn` 会与父的 `(session_id, seq)` UNIQUE 约束冲突;worker 中间过程对父 messages 透明是核心约定。
- **Decision**: 加第 20 参 `skip_persist: bool`,run_chat_loop 函数体内 persist 调用全部包 `if !skip_persist { ... }`(initial user / resend audit / metadata / assistant turn / cancel-synthetic / parallel+serial tool_executed_audit / tool_result / max_turns / cwd / touch_session / etc.)。**PR1 写 18 处,PR2a 实测 16 处(RULE-A-015 拆出)**:(a) `add_token_usage` 不在 messages 表的 `(session_id, seq)` UNIQUE 范围,worker streaming 累加应进父 sessions 表,不归 skip_persist 守;(b) terminal `Done` emit 必经 `SubagentBufferSink` 才能让 `was_cancelled` 正确 catch,不是 DB write 不归 skip_persist 守。这 2 处修正同步进 `agent-loop-architecture/pattern-skip-persist-gate.md` §"Pattern: PR2a corrected PR1 over-broad skip_persist gate (RULE-A-015)" pattern 段,作为未来 skip_* flag 的设计参考(不重蹈 PR1 "all persist = same gate" 过度宽泛的反模式)。
- **Why 不在拦截点单独守门**:worker 调用 `run_chat_loop` 嵌套后,函数体本身不知道自己是 worker;把守门推到函数体内**单一权威**(对齐 RULE-A-006 单一权威),每个 persist site 一目了然。

---
