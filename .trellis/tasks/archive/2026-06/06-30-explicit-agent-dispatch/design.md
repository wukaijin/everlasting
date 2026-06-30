# Design — 显式 `@@` 强制 dispatch sub-agent

> 对应 `prd.md`。本文聚焦技术决策、IPC 契约、注入点与事件流、复用边界、风险。

## 1. 核心洞察

现有 LLM 自主 dispatch 的执行段在 `chat_loop.rs:2374-2421`：LLM 在 turn N 发出 `dispatch_subagent` tool_use → serial-path 拦截器拿 `id` + `input` → 调 `run_subagent(全量父闭包依赖, id, input, &token, &sink, app_handle, false/*force_readonly*/, &subagent_cache, &app_data_dir, false/*parallel*/)` → 拿回 `(content, is_error, cancel_parent, exit_code)` → 记 audit → 回填 tool_result。

**forced 路径 = 把这一段原样提前到 turn 1 开头执行，`id`/`input` 改由 `forced_dispatch` 合成，且不经过 `provider.stream`。** `run_subagent` 的 19 个参数照抄 2376-2419，**零逻辑复制**——这是"复用而非重写"的落点，也是 AC "复用 run_subagent" 的验证对象。

## 2. 数据流

```
前端 ChatInput: "@@spec-auditor 审一下 tool-contract.md"
  → chatInputCodeMirror 检测 @@ 触发器 → TriggerMenu 第三实例 ← list_subagents(projectPath)
  → 选中插入 @@token；用户继续输入 task
  ↓ send()
chat.ts send(): 拆分 → forcedDispatch={subagent:"spec-auditor", task:"审一下 tool-contract.md"}
  → invoke("chat", { requestId, sessionId, messages, resendSeq:null, forcedDispatch })
  ↓
chat 命令 (chat.rs:61): 新增 forced_dispatch 参数 → 透传 run_chat_loop
  ↓
run_chat_loop turn 1 前置短路:
  ① persist 用户消息(task 文本)
  ② 合成 tool_use(id=uuid, input={subagent,task}) → emit tool:call
  ③ run_subagent(全量依赖, ...) ——【不调 provider.stream】
  ④ emit tool:result + worker 的 subagent:event 流(SubagentDrawer 展开)
  ⑤ worker summary 作为 assistant 文本 → emit chat-event done
  ⑥ persist_turn(assistant=summary) → loop 退出(仅 1 turn)
```

## 3. IPC 契约（新增 / 变更）

### 3.1 `list_subagents`（新增，前端 `@@` 数据源）
```rust
#[tauri::command]
pub async fn list_subagents(project_path: String, state: State<'_, Arc<AppState>>) -> Result<Vec<SubagentInfo>, String>
// SubagentInfo { name, description, source: "builtin"|"user"|"project", tools: Vec<String> }
// 实现: state.subagent_cache.list(&project_path).await → 映射 LoadedSubagent
```
- 复用 `SubagentCache::list`（已 mtime-fenced + project>user>builtin 合并）。
- 注册进 `commands/mod.rs` invoke_handler。

### 3.2 `chat` 加参数（变更）
```rust
// chat.rs:61 签名新增末尾参数（蛇形，serde 自动转 JS forcedDispatch）
#[tauri::command]
pub async fn chat(..., resendSeq: Option<i64>, forcedDispatch: Option<ForcedDispatchArgs>) -> Result<(), String>
// ForcedDispatchArgs { subagent: String, task: String }  — 前端已校验非空 + agent 存在
```
- `run_chat_loop` 同步加 `forced_dispatch: Option<ForcedDispatch>` 末尾参数，透传。
- **resendSeq 与 forcedDispatch 互斥**：前端 send 时若 forced 则 resendSeq 恒为 None（resend 是重发历史消息，不带 @@ 强制语义）。

## 4. 后端注入点（`run_chat_loop` turn 1 前置短路）

位置：**紧随现有的"用户消息 persist 站点"（`last_user_snapshot` 捕获处）之后、`for turn in 1..=turn_limit` 循环体 LLM 请求之前**。伪代码：

```rust
// turn 循环开始前的一次性前置（不是每轮）
if let Some(fd) = &forced_dispatch {
    // ① 用户消息已在上方 persist 站点落库（task 文本）
    // ② 合成 tool_use
    let tool_use_id = format!("forced_{}", uuid::Uuid::new_v4());  // 对齐现有 id 生成风格
    let input = serde_json::json!({ "subagent": fd.subagent, "task": fd.task });
    emit_tool_call(&sink, &rid, &tool_use_id, /*name*/ DISPATCH_TOOL_NAME, &input).await;

    // ③ 复用 run_subagent —— 参数照抄 chat_loop.rs:2376-2419
    let (content, is_error, cancel_parent, _exit_code) = run_subagent(
        &provider, context_window, &rid, &session_id, &memory_cache, &read_guard,
        &skill_cache, &permission_asks, &cancellations, &session_active_request,
        &background_shells, &db, &current_ctx, &tool_use_id, &input, &token, &sink,
        app_handle.clone(), /*force_readonly*/ false, &subagent_cache, &app_data_dir,
        /*parallel*/ false,
    ).await;

    // ④ tool_result + audit（照抄 2374 段的 emit_tool_result / record_tool_executed_audit）
    emit_tool_result(&sink, &rid, &tool_use_id, &content, is_error).await;
    if !skip_persist { permissions::record_tool_executed_audit(...).await.ok(); }

    // ⑤ worker summary 作为 assistant 文本
    if cancel_parent { cancelled = true; }   // Stop 传播，对齐现有 cancel 语义
    sink.emit_chat_event(ChatEvent::Done { /* text = content */ }).await;

    // ⑥ persist_turn(assistant=summary) → 退出
    if !skip_persist { persist_turn(...).await; }
    return <loop 终态>;   // forced 仅 1 turn，不进 LLM 循环
}
// —— 正常 LLM turn 循环（不受影响）——
for turn in 1..=turn_limit { ... }
```

**关键不变量**：
- forced 路径 `provider.stream` / `provider.send` **零调用**（AC 断言点）。
- `skip_persist=false`（父 session），assistant turn 正常持久化；worker 内部 `run_subagent` 自带 `skip_persist=true` 的嵌套 loop，行为与 LLM-dispatch 完全一致。
- `cancel_parent` 传播 Stop（用户在 worker 跑时点 Stop → worker 取消 → 父 loop `cancelled=true`）。

## 5. isolation / permission（不新增逻辑）

- **isolation**：`run_subagent` 内部已按 `resolve_isolation(frontmatter_default, dispatch_input)` 决策。forced 路径 `dispatch_input=None`（不暴露 isolation 开关，见 OOS）→ 单次 dispatch 落 shared-cwd（spec-auditor frontmatter 无 isolation 字段 → shared）。
- **permission**：worker 继承父 session Mode，`run_subagent` 现有逻辑不变。Edit/Plan 模式下 worker 的写工具走 `WorkerAskBanner`，Yolo 全放行——forced 路径透明继承。

## 6. 前端改动

### 6.1 `@@` 触发器（`chatInputCodeMirror.ts` + `ChatInput.vue`）
- 在 composable 加 `@@` 检测分支：当前行以 `@@` 开头（且非 `@` 单字符——文件补全）→ 开 `agentPaletteOpen`，filter = `@@` 之后的文本。
- `ChatInput.vue` 加第三个 `<TriggerMenu trigger="@@" header-label="Agent">`，`items` 来自 `list_subagents` invoke（启动时 + project 切换时拉取，缓存到 store）。
- 互斥规则：`/` / `@` / `@@` 三者一行只能触发一个（`@` 检测需排除 `@@` 前缀——现 `@` 检测点要加 `@@` 排除）。
- 选中：插入 `@@<name> `；光标留在空格后等用户输入 task。

### 6.2 send 拆分（`chat.ts` send()）
- 正则 `/^@@([a-zA-Z0-9_-]+)\s+(.+)$/s` 匹配输入：
  - 命中 → `forcedDispatch = { subagent: $1, task: $2.trim() }`；用户消息文本 = task（去掉 @@ 前缀）。
  - 校验：`subagent` ∈ 已加载 agent 列表（来自 list_subagents 缓存）→ 否则 toast 报错、不发；`task` 非空 → 否则阻止。
- `invoke("chat", { ..., forcedDispatch })`。

### 6.3 agent chip 渲染（`chatInputTokens.ts` + MessageItem）
- `@@<name>` token 着色，对齐 `@file` token 的 thinking 色 family（或新增 agent 色——倾向复用 thinking 色避免 token 膨胀）。
- 已发送消息里 `@@<name>` 同样渲染为 chip（只读）。

## 7. 风险 / 边界

| 风险 | 处理 |
|---|---|
| `@@` 与 `@` 检测冲突（`@@x` 被当文件补全） | `@` 检测分支前置 `@@` 排除；单测覆盖 `@@x` / `@x` / `@@` 单独 |
| forced 时 worker 取消传播 | 复用 `cancel_parent`，与 LLM-dispatch 同语义；测试 cancel 路径 |
| 未知 agent 名绕过前端校验进后端 | 后端 `run_subagent` 内 `cache.lookup` 失败已返回 error content（现有行为），forced 路径透传 is_error；但前端应首道拦截（AC） |
| forced + resendSeq 同传 | 前端互斥（§3.2）；后端 defensive：两者同传时以 forced 为准、记 warn |
| 合成 tool_use_id 与 LLM 真 id 冲突 | `forced_` 前缀命名空间隔离 |
| `persist_turn` 的 usage 统计 | forced turn 无 LLM token 消耗（worker 的消耗记在 worker 自己的 run 记录里）；父 turn usage=0 合理 |

## 8. 测试策略

**后端**（`agent/tests_agent_loop.rs` 或新建 `tests_forced_dispatch.rs`）：
- `forced_dispatch_runs_worker_without_llm`：forced 路径下 `MockProvider::stream` 调用计数 == 0；`run_subagent` 被调（通过 worker summary 回填断言）；`subagent_runs` 表有记录；assistant turn 持久化。
- `forced_dispatch_cancel_propagates`：worker 跑时触发 token cancel → 父 loop `cancelled=true`。
- 回归：`agent_loop_*` 既有 9 个集成测试 + LLM 自主 dispatch 测试全绿（forced 参数传 None 时行为不变）。

**前端**（vitest）：
- `chatInputCodeMirror` `@@` 检测 + 与 `@` 互斥。
- `chat.ts` send 拆分：命中/未知 agent/空 task 三分支。
- `@@` TriggerMenu 渲染 list_subagents 结果 + source chip。

## 9. 回滚形态

- 后端：`forced_dispatch` 参数默认 `None` → 行为与现状完全一致；删除注入点 + 参数 + `list_subagents` command 即回滚。
- 前端：`@@` 触发器分支 + send 拆分 + chip 渲染，三处独立删除；`@`/`/` 路径不受影响。
- 无 DB migration（`subagent_runs` / `messages` schema 不变），无需数据回滚。
