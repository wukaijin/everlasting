# B9-A use_ui 基础设施（后端 tool + 前端 registry）

> Child of `07-02-b9-generative-ui`。**B/C 的前提**（Child B code_block / Child C diff 都依赖本 child 的 use_ui tool + registry）。
> 父决策 D1-D6 见 parent `prd.md`。技术设计见 `design.md`，执行计划见 `implement.md`。

## Goal

打通 use_ui tool 管线：LLM 调 `use_ui` → `execute_tool` non-blocking dispatch → "已渲染" tool_result → 前端按 `primitive.type` 路由渲染。本 child 用 **mock primitive**（占位渲染）验证管线端到端通；真实 primitive（code_block/diff）实现归 Child B/C。

## Requirements

- **后端 `tools/use_ui.rs`**：
  - `definition()` → `ToolDef { name: "use_ui", description, input_schema }`，schema = `{ primitives: [{ type: "diff"|"code_block", ... }] }`（discriminated union by `type`）。
  - `execute(input, _ctx, _session_id)` → `(String, bool)` non-blocking，返回 `("已渲染 N 个 primitive", false)`。校验 `primitives` 非空 + `type` 合法（非法 → is_error + 中文错误）。
- **后端注册**：`builtin_tools()` 加 `use_ui::definition()`；`execute_tool_inner` match 加 `"use_ui" => use_ui::execute(...)` 分支（**普通 dispatch**，仿 `remember`，**不**走 ask_user_question 的 blocking 拦截）。
- **前端 component registry**：`type → Vue 组件` Map。MVP 注册 mock 占位组件（Child B/C 替换为真实组件）。
- **前端 `<UiCard>` 容器**：读 `call.input.primitives`，遍历按 `primitive.type` 从 registry 取组件渲染。
- **MessageItem dispatch 泛化**：`tool_name === "use_ui"` → 渲染 `<ToolCallCard>`（header，input 折叠）+ sibling `<UiCard>`（仿 ask_user_question 的 ToolCallCard + AskUserQuestionCard 对称结构）。
- **持久化**：复用 `persist_turn`（tool_result 天然落库），无新 DB 表 / migration。

## Acceptance Criteria

- [ ] `use_ui` 在 `builtin_tools()` 注册，LLM API 响应里能看到 tool 定义
- [ ] LLM 调 `use_ui` → `execute_tool_inner` match 命中 → 返回 non-blocking tool_result（"已渲染 N 个 primitive"）
- [ ] tool_result 经 `persist_turn` 持久化（刷新 session 后 tool 卡片仍在）
- [ ] 前端 MessageItem：`tool_name === "use_ui"` → ToolCallCard + UiCard（mock primitive 按 type 渲染）
- [ ] registry 可扩展：加新 type 只改 registry Map，不改 dispatch/UiCard 逻辑
- [ ] 非法 input（primitives 空 / type 未知）→ tool_result is_error + 中文提示，不崩
- [ ] `cargo test` 全绿（含 use_ui 单测）+ `vue-tsc --noEmit` 0 err + vitest 全绿

## Dependencies

- 无外部 child 依赖（本 child 是 B/C 的前提）
- B/C 在各自 prd 标注 `blockedBy: 07-02-b9-use-ui-infra`
