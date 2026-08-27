# Research — workflow 角色门调用链与测试基建(main session 预研,2026-08-27)

> 用途:为 `role_gate_refresh` 集成测试提供寻路图。所有 file:line 均已核对;细节语义以代码为准。

## 1. 角色门链路(生产侧,全部 `app/src-tauri/src/`)

| 环节 | 位置 | 要点 |
|---|---|---|
| 门函数 | `agent/subagent/dispatch.rs:411` `check_workflow_role_gate(Option<&WorkflowCtx>, subagent_name, input) -> Option<String>` | 纯函数、无 I/O。返回 `Some(content)` = 拒绝(content 即 tool_error 正文);`None` = 放行 |
| 判定输入 | 同上 | `ctx.current_task.as_ref()?.status.as_str()`(无 task → 无门);角色集合 = `workflow::allowed_roles(ctx.task_workflow_def, state)`;`input.force==true` 旁路(warn 日志)。**C5 注释(同文件 ~425)**:`task_workflow_def` 取任务属主插件而非会话插件 |
| 调用点 | `agent/subagent/dispatch/parse.rs:170`(阶段 A `parse_dispatch` 内) | 拒绝走 `Err((content, true /*is_error*/, …))` → tool_result 错误;文本前缀 `"Role gate denied: '` |
| 工具载荷字段 | `parse.rs:74` | `input.get("subagent")`(LLM 参数名是 **subagent**,不是 name/role) |
| 对外工具名 | `chat_loop/suite.rs:315` 注释 | LLM tool_use 名为 **`dispatch_subagent`**(由 dispatch pipeline 提取后调 run_subagent hub) |
| **活引用契约** | `chat_loop/suite.rs:398-412`(`DispatchCtx::workflow_ctx` 字段 doc) | 必须是 run_chat_loop 函数域活引用;drive_turn 轮顶把盘上最新 task 刷进这份拷贝;**不能读 `request.workflow_ctx`(入口快照,loop 内永不更新)**——此即 08-27 漂移事故的现场记录,测试要抓的就是这两类回归 |
| **轮顶刷新** | `chat_loop/drive.rs:933-937`(R4,07-10 任务引入) | `ctx.current_task = workflow::inject::resolve_current_task(&current_ctx.worktree_path).await`;随后 :939 用刷新后的 ctx 注入面包屑(E2 trace 记录 status 快照) |
| 盘上 task 解析 | `agent/workflow/inject.rs:262` `resolve_current_task(project_path)` | 扫 `<project>/.everlasting/tasks/`(读目录条目挑选活跃 task 的规则见该函数体);测试造数据时按它反推 task.json 落点与字段 |
| WorkflowCtx 类型 | `crate::agent::workflow::WorkflowCtx` | 关键字段:`workflow_def`、`task_workflow_def`、`current_task`(构造/装配点用 `WorkflowCtx { … }` 全库 grep) |

## 2. 测试基建(消费侧)

- 入口:`app/src-tauri/src/agent/tests_agent_loop/mod.rs`(子模块清单 + `messages_to_text` 辅助);公共件在 `src/agent/tests_common.rs`(**TestHarness / MockEmitter / make_harness**,514 行,先读)。
- MockProvider:`crate::llm::provider::mock::{MockProvider, MockResponse}`;脚本化逐轮响应的用法见 `tests_agent_loop/mock_provider.rs`(基础)+ 已有**带工具调用**的多轮样例(checklist.rs / parallel_dispatch.rs 里找 ToolUse → tool_result 断言的写法)。`call_count()` 可断言轮数。
- workflow 会话既有样例:`tests_agent_loop/handoff.rs:518-575` —— `db::sessions::set_session_workflow_enabled(&h.db, id, true)` 是开 workflow 会话行的现成写法;但 handoff 未覆盖角色门。
- spec 契约:`.trellis/spec/backend/agent-loop-architecture/signature-run-chat-loop.md`(三套件 ChatLoopDeps/ChatLoopRequest/CallerRole 契约,08-27 重写)、`pattern-production-test-entry.md`、`pattern-worker-subagent.md`、`.trellis/spec/backend/test-model-contract.md`。

## 3. 测试设计草图(R1 场景映射)

1. **数据**:temp 项目目录建 `.everlasting/tasks/<slug>/task.json`(字段以 TaskJson 序列化格式为准),初始 `status=planning`;插件的 `roles_by_state` 选一对可区分状态:`planning→[spec]`、`implementation→[dev]`(插件 def 可用 builtin dev 或测试内自造最小 def,看哪个拼装成本低)。
2. **第 1 轮**:MockProvider 吐含 `dispatch_subagent{subagent:"dev", task:"…"}` ToolUse 的 Events → 断言 tool_result 为 error 且含 "Role gate denied" 与 state 名(planning)。此时 worker 不应被真正执行。
3. **两轮之间**:直接改写盘上 task.json → `status=implementation`。
4. **第 2 轮**:MockProvider 再吐同一 dispatch_subagent 调用 → 断言**无 denial** 且走了放行分支的可观测信号(轮数到 3 / worker 执行痕迹 / mock 后续 turn 被 drain——以 harness 实际形态择一)。
5. **回归红线**(变异验证,R3):若门改读入口快照或删掉 drive.rs 轮顶刷新,第 4 步必然仍 denial → 测试红。两个变异点各验一次并记录(转红 → 复原 → 绿)。

## 4. 风险 / 待实现时确认

- Worker 放行后的完整执行链可能重(subagent 解析、worktree);若太重,放行断言可弱化为"门不再拒绝"(例如第二轮回 `end_turn` 前插一轮只见 denial 缺席即可)——**底线是拒绝与放行两侧都不可观测地等价**。
- `resolve_current_task` 的多 task 目录选择规则、TaskJson 字段名从 inject.rs / task.rs 实读,勿猜。
- run_chat_loop 直驱路径(测试如何进入 loop 并带上 workflow_ctx)看 signature-run-chat-loop.md 三套件契约;handoff.rs 是最接近的现成参考。
