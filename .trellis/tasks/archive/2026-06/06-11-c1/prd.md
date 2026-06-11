# C1: 取消机制完整化

## Goal

让用户在 agent loop 的任何阶段都能可靠地取消请求（LLM 流式、tool 执行中、多 turn 循环中），且取消后的状态一致、无泄漏、可恢复（用户能继续发送下一条消息）。

## What I already know

### 已有实现（相当完整）

**Rust 后端**：
- `CancellationToken` 注册在 `AppState.cancellations` map 中（`chat.rs:117-121`）
- `CancellationGuard` RAII 清理（`state.rs:326-351`）
- Agent loop 内 `tokio::select! { biased; _ = token.cancelled() => ... }` 在每个 SSE event 边界检测取消（`chat.rs:398-526`）
- 取消后 persist partial turn + synthetic tool_result 修复 2013 orphan bug（`chat.rs:536-653`）
- `cancel_chat` Tauri command（`commands/cancel.rs`）— 幂等，前端 Stop 按钮调用
- `cancel_inflight_for_session` helper — 破坏性操作（delete_session / detach / delete worktree）前自动取消

**Vue 前端**：
- Stop 按钮（ChatInput.vue）— `sending` 时发送按钮变形为 Stop
- `streamController.cancel(requestId)` — IPC 调 `cancel_chat`
- `handleChatEvent` 处理 `done(stop_reason:"cancelled")` — finalizeRequest 清理状态
- finalizeRequest → reloadAfterFinalize 从 DB 重载消息

### 关键 Gap：Tool 执行中途无法取消

当前 `execute_tool()` 签名不接受 CancellationToken。Shell tool 使用 `cmd.output()` 同步等待子进程完成，最多 300s timeout。用户点 Stop 后：
- Agent loop 的 `tokio::select!` 检测到取消
- 但如果正在 `execute_tool()` 内部执行 shell 命令，select! 无法中断它
- 子进程继续运行直到完成或 300s timeout
- 其他 tool（read_file / write_file / grep 等）是毫秒级，影响不大

### 取消覆盖矩阵（现状 vs 目标）

| 阶段 | 现状 | 目标 |
|------|------|------|
| LLM SSE 流式 | ✅ select! event 边界 | ✅ 不变 |
| Tool 执行中（shell） | ❌ 等 tool 完成 | ✅ kill 子进程 + 返回 |
| Tool 执行中（其他） | ❌ 等待（毫秒级，影响不大） | ✅ select! 包装统一中断 |
| Agent loop turn 间 | ✅ select! LLM 边界 | ✅ 不变 |
| 破坏性操作前自动取消 | ✅ cancel_inflight_for_session | ✅ 不变 |

## Requirements

### R1: execute_tool 统一 CancellationToken

- `execute_tool` 签名新增 `cancel: CancellationToken` 参数
- `execute_tool` 内部用 `tokio::select!` 包裹所有 tool 执行：
  - cancel arm → 返回 `("Tool execution was cancelled".into(), true, ToolContextUpdate::default())`
  - tool arm → 正常返回
- 所有 tool 自动获得 cancel 能力，无需逐个修改

### R2: Shell tool 子进程 kill

- `shell::execute` 接收 `CancellationToken`
- 改用 `Command::spawn()` → `Child` 替代 `cmd.output()`
- `tokio::select!` 包裹 `child.wait_with_output()` 和 `token.cancelled()`
- Cancel 时：`child.kill()` + 收集已有 stdout/stderr 部分输出
- 部分输出也走 disk-spill 逻辑（> 30K 写文件）

### R3: Agent loop 传递 token 到 tool 执行

- `chat.rs` tool 执行循环（`chat.rs:682-722`）传 token clone 给 `execute_tool`
- 如果 tool 被 cancel 中断，agent loop 进入已有的 cancel cleanup 路径

### R4: Esc 键盘快捷键（前端）

- ChatInput 监听 `keydown Escape`，当 `sending` 为 true 时触发 `emit("stop")`
- 与 Stop 按钮走同一条 IPC 路径

## Acceptance Criteria

* [ ] AC1: Agent loop 执行 shell tool 期间，用户点 Stop → 子进程在 1s 内被 kill
* [ ] AC2: 被取消的 shell 返回部分输出（已收集的 stdout/stderr）给 LLM
* [ ] AC3: 其他 tool（read_file / grep 等）被 cancel 时返回 "Tool execution was cancelled"
* [ ] AC4: Cancel 后 agent loop 正确 persist partial turn + synthetic tool_result（现有逻辑不变）
* [ ] AC5: Cancel 后 session 可继续发送下一条消息
* [ ] AC6: 按 Esc 键（sending 状态下）等效于点击 Stop 按钮
* [ ] AC7: cargo test --lib 通过（shell tool 现有测试 + 新 cancel 测试）
* [ ] AC8: vue-tsc --noEmit 通过

## Definition of Done

* Rust: `cargo test --lib` 通过
* 前端: `vue-tsc --noEmit` 通过
* 手动验证：shell 长 running 命令（`sleep 60`）中 Stop → 进程被 kill

## Technical Approach

### 两层 CancellationToken 传播

```
chat.rs agent loop
  ├─ tokio::select! (SSE stream vs token)     ← 已有
  ├─ tool 执行循环
  │   └─ execute_tool(name, input, ctx, guard, sid, token)  ← 新增 token 参数
  │       ├─ tokio::select! (tool_future vs token.cancelled())  ← R1: 通用包装
  │       └─ shell::execute(input, ctx, token)  ← R2: 内部 child.kill()
  └─ cancel cleanup path                        ← 已有
```

### Shell tool 重构要点

1. `Command::new("sh").spawn()` → 获取 `Child`
2. 用 `child.take_stdout()` + `child.take_stderr()` + `tokio::io::read_to_end` 收集输出
3. `tokio::select!` 包裹：cancel 时 `child.kill()` + `child.wait()` 获取已收集部分
4. 部分输出标记 `[cancelled, partial output]` 前缀

### 前端改动

ChatInput.vue 新增 Esc 键监听，3 行代码改动。

## Out of Scope

* subagent tool（B6，依赖 B5，第三档）
* Ctrl+C 全局取消（需要 Tauri 全局快捷键注册，复杂度高）
* Tool 执行超时缩短（300s 不变，取消机制解决等太久的问题）

## Technical Notes

* 关键文件：
  - `app/src-tauri/src/tools/mod.rs` — execute_tool 签名修改
  - `app/src-tauri/src/tools/shell.rs` — shell 重构为 spawn + select
  - `app/src-tauri/src/agent/chat.rs` — 传 token 到 execute_tool
  - `app/src/components/chat/ChatInput.vue` — Esc 键监听
* 现有测试不受影响（execute_tool 新参数在测试中传 `CancellationToken::new()` 即可）
