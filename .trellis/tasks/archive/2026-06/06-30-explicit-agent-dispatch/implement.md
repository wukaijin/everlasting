# Implement — 显式 `@@` 强制 dispatch sub-agent

> 对应 `prd.md` / `design.md`。有序 checklist，每段后跑验证命令。Review gate 在每段末尾。

## 段 0：准备

- [ ] `task.py start` 把任务状态置 in_progress（三件套 review 通过后）。
- [ ] 新建分支（若用分支工作流）：`feat/explicit-agent-dispatch`。

## 段 1：后端 IPC + 类型（先立契约，前后端可并行）

- [ ] 新增 `ForcedDispatch` 结构（`agent/subagent/mod.rs` 或 `agent/chat.rs`）：`{ subagent: String, task: String }`（+ serde 蛇形 alias，JS 传 `forcedDispatch`）。
- [ ] 新增 `commands/subagent_runs.rs`（或 `commands/panel.rs`）内 `list_subagents(project_path, state) -> Vec<SubagentInfo>`，调 `state.subagent_cache.list(...)` 映射 `{ name, description, source, tools }`。
- [ ] `commands/mod.rs` invoke_handler 注册 `list_subagents`。
- [ ] `chat` 命令（`chat.rs:61`）签名加 `forcedDispatch: Option<ForcedDispatchArgs>`，透传 spawn 闭包。
- [ ] `run_chat_loop`（`chat_loop.rs:132`）签名加 `forced_dispatch: Option<ForcedDispatch>` 末尾参数；所有现有调用点（生产 chat + worker 嵌套 + 9 个测试）补 `None`。

**验证**：
```bash
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check
```
**Review gate 1**：编译过、现有测试不因新参数报错（测试传 None 跑通）。

## 段 2：后端 forced 注入点（核心）

- [ ] 在 `run_chat_loop` 用户消息 persist 站点之后、`for turn` 循环之前，插入 forced 前置短路（见 `design.md §4` 伪代码）。
- [ ] 复用 `chat_loop.rs:2376-2419` 的 `run_subagent` 调用参数（**逐参对照，零改动**：`force_readonly=false`、`parallel=false`）。
- [ ] 合成 `tool_use_id = "forced_{uuid}"`；`emit_tool_call` / `emit_tool_result` 对齐现有 tool 事件发射辅助。
- [ ] audit：`!skip_persist` 时调 `record_tool_executed_audit`（照抄 2374 段）。
- [ ] `cancel_parent` → 父 loop `cancelled=true`（Stop 传播）。
- [ ] worker summary 作 assistant 文本 → `ChatEvent::Done`；`!skip_persist` 时 `persist_turn`；return 终态（不进 LLM 循环）。

**验证**：
```bash
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib forced_dispatch
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib agent_loop
```
**Review gate 2**：`forced_dispatch_runs_worker_without_llm`（stream 零调用 + summary 回填 + subagent_runs 有记录）通过；9 个 `agent_loop_*` 回归绿。

## 段 3：前端 `@@` 触发器 + 数据源

- [ ] `list_subagents` TS 绑定 + 一个轻量 store（或复用 `subagentRuns` / 新建）缓存当前 project 的 agent 列表，启动 + project 切换时拉取。
- [ ] `chatInputCodeMirror.ts`：加 `@@` 检测分支；`@` 检测分支前置 `@@` 排除（`@@x` 不触发文件补全）。
- [ ] `ChatInput.vue`：第三个 `<TriggerMenu trigger="@@" header-label="Agent">`，`items` 来自 store，`#row` slot 渲染 name + description + source chip（复用现有 chip 色：builtin/user/project）。
- [ ] 选中插入 `@@<name> `，光标定位空格后。

**验证**：
```bash
cd app && pnpm exec vitest run chatInputCodeMirror
```
**Review gate 3**：`@@` 检测 + 与 `@` 互斥单测绿。

## 段 4：前端 send 拆分 + chip

- [ ] `chat.ts` `send()`：正则拆 `@@<agent> <task>` → `forcedDispatch`；校验 agent ∈ 缓存列表（否则 toast、不发）、task 非空（否则阻止）；`forced` 时 `resendSeq=null`。
- [ ] `invoke("chat", { ..., forcedDispatch })` 传参。
- [ ] `chatInputTokens.ts`：`@@<name>` token 着色（复用 thinking 色或 `@file` family）；MessageItem 已发消息的 `@@<name>` 渲染只读 chip。
- [ ] 用户消息文本存 task（去 @@ 前缀）。

**验证**：
```bash
cd app && pnpm exec vitest run chat path    # chat.ts send 拆分 + token
cd app && pnpm build                          # vue-tsc 类型检查（forcedDispatch 类型链路）
```
**Review gate 4**：send 三分支（命中 / 未知 agent / 空 task）单测绿；`pnpm build` 类型过。

## 段 5：端到端 + 回归

- [ ] `pnpm tauri dev` 手测：`@@spec-auditor 审一下 .trellis/spec/backend/tool-contract.md` → SubagentDrawer 展开、spec-auditor 跑出结果、主对话无 LLM 自主决策回合。
- [ ] 手测未知 agent / 空 task 的前端拦截。
- [ ] 手测 `@文件` / `/命令` 仍正常（未受 `@@` 影响）。

**验证（全量）**：
```bash
cd app && pnpm exec vitest run                # 全前端单测
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test
cd app && pnpm build
```
**Review gate 5（最终）**：全绿 + 手测三场景通过 → 进入 Phase 3（spec 更新 / commit）。

## Rollback points

- 段 1 后：删 `forced_dispatch` 参数 + `list_subagents` 即净回滚（无 DB 变更）。
- 段 2 后：删 forced 前置短路块即回滚（参数留 None 不影响）。
- 段 3/4：前端三处（触发器 / 拆分 / chip）独立可删。

## Notes

- 复用 `run_subagent` 是硬约束——任何"在 chat_loop 外重写 worker 调度"的冲动都要拒绝（22+ 参数闭包依赖，复制即维护地狱）。
- 合成事件 shape 必须对齐现有 `dispatch_subagent` 的 `tool:call` / `tool:result` / `subagent:event`，否则 SubagentDrawer / ToolCallCard 渲染分叉。
