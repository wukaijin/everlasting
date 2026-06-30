# 显式调用 sub-agent（输入框 `@@` 指定 agent 强制 dispatch）

## Goal

让用户在输入框用 `@@<agent>` 前缀**显式指定**一个 sub-agent（如 `@@spec-auditor 审一下 tool-contract.md`），发送时后端**绕过主 agent LLM 的自主决策**，直接对该 agent 强制 `dispatch_subagent`，前缀之后的文本作为 task。

> 当前 sub-agent 完全由 LLM 自主调度（`dispatch_subagent` 是注册给 LLM 的 tool，enum 含 builtin + user + project agents），用户无法显式指定跑哪个 agent。本任务把"选谁"的决定权从 LLM 收回到用户手里。

## Background（已核实）

- `dispatch_subagent` 是 LLM tool，定义在 `agent/subagent/mod.rs:116`；enum 由 `definition_with_cache`（`mod.rs:205`）从 `SubagentCache::list` 动态构造（builtin + `~/.config/everlasting/agents/` + `<project>/.everlasting/agents/`，mtime 热加载）。
- `run_subagent`（`agent/subagent/dispatch.rs:238`，`pub(crate) async fn`）是 `run_chat_loop` 闭包内 serial-path 调用的拦截器，复用父闭包的 `provider` / `db` / `cancellations` / `subagent_cache` / `permission_asks` / `app_handle` 等全部依赖。**不复制这段调度逻辑是本任务的核心约束。**
- 前端触发器现状（`ChatInput.vue:534` / `:550`）：`/` = 命令 + skill 面板；`@` = 文件补全（B2，fuzzysort）。无 agent 触发器。
- `chat` 命令（`agent/chat.rs:61`）：`invoke("chat", { requestId, sessionId, messages, resendSeq })` → spawn `run_chat_loop`（`chat_loop.rs:132`，`for turn in 1..=turn_limit`，turn_limit = `MAX_TURNS` = 50）。

## Requirements

### 前端

- **`@@` 触发器**：复用 `TriggerMenu` 第三实例（与 `/`、`@` 互斥——一行不以 `@@` 开头则不弹）。数据源来自新 IPC `list_subagents(projectPath)`（返回 `SubagentCache::list` 的 `name` + `description` + `source`）。
- **选中插入**：选中 agent 后插入 `@@<name> ` token；token 之后的输入文本作为 task。
- **send 拆分**：`chat.ts` `send()` 检测 `@@<agent>` 前缀，拆出 `forcedDispatch = { subagent, task }` 作为 `chat` 命令新参数；用户消息文本保留 task 内容（去掉 `@@` 前缀）。
- **UI 标记**：用户消息里的 `@@<agent>` 渲染为 agent chip，对齐 `@file` token 可视化（`chatInputTokens.ts`）。

### 后端

- **`chat` 命令加参数**：`forced_dispatch: Option<ForcedDispatch { subagent: String, task: String }>`，透传给 `run_chat_loop`。
- **forced 注入点**：`run_chat_loop` 首轮（turn 1）前置——若 `forced_dispatch.is_some()`：
  1. 合成一个 `dispatch_subagent` tool_use（`tool_use_id` 新生成，`input = { subagent, task }`），发 `tool:call` 事件；
  2. **不调 `provider.stream`**（这是"强制 / 绕过 LLM"的可测断言），直接调 `run_subagent`（复用闭包内全部依赖）；
  3. `run_subagent` 返回 `(content, is_error, cancel_parent, exit_code)`，发 `tool:result` + worker 的 `subagent:event` 流（SubagentDrawer 照常展开——run_subagent 内部已发）；
  4. worker `content`（summary）作为该 turn 的 assistant 文本输出（`chat-event` done）；
  5. `persist_turn` 照常；该 turn 即终态，loop 退出（forced dispatch 仅一轮）。
- **isolation**：走 `resolve_isolation` 默认（单次 dispatch = shared-cwd），**不**暴露前端开关。
- **permission**：worker 继承父 session Mode（`run_subagent` 现有逻辑，Edit/Plan 模式下写工具走 `WorkerAskBanner`，复用）。

### 持久化

- 用户消息：存 task 文本（`messages` 表）。
- assistant turn：worker summary（`persist_turn` 照常）。
- `subagent_runs` 记录：`run_subagent` 内部照常写。

## Acceptance Criteria

- [ ] 输入 `@@spec-auditor 审 X` 发送后，spec-auditor worker 真正运行（SubagentDrawer 展开、`subagent_runs` 有记录），且**主 agent LLM 首轮未被调用做 dispatch 决策**（断言：forced 路径下 `provider` mock 的 stream 零调用）。
- [ ] 前端 `@@` 面板列出 builtin + user + project agents，含 description + source chip（`builtin` / `user` / `project`）。
- [ ] `@@<未知agent>` ——前端拆分阶段校验，不在 `SubagentCache` 则报错 toast，**不**发 `chat`。
- [ ] task 为空（`@@spec-auditor ` 后无文本）——前端阻止发送并提示。
- [ ] forced dispatch 的 worker summary 作为 assistant turn 正常持久化并在消息流渲染。
- [ ] isolation 默认 shared（单次）；permission 继承父 Mode（Edit 模式下写工具走 `WorkerAskBanner`）。
- [ ] **复用 `run_subagent`**——不在 `chat_loop` 外重写 worker 调度（permission / isolation / transcript / drawer 事件一律复用）。
- [ ] 回归：现有 `/`、`@` 触发器 + LLM 自主 `dispatch_subagent` 路径不受影响（`tests_agent_loop.rs` + 前端 vitest 绿）。

## Out of Scope

- `@@` 语法里指定 `isolation` / 覆盖 tools（YAGNI，走默认）。
- 单条消息强制 dispatch 多个 agent（一次最多一个 `@@` 前缀）。
- 自定义 agent 的 CRUD UI（仍靠手写 `.md` + mtime 热加载）。
- forced dispatch 后让主 agent 继续接力该任务（forced turn 即终态，主 agent 下一轮可自然语言追问）。

## Notes

- 实现细节（合成事件 shape、`persist_turn` / `turn_limit` 交互、与现有 dispatch 拦截器的去重）在 `design.md` 梳理。
- `implement.md` 给有序 checklist + 验证命令 + review gate。
