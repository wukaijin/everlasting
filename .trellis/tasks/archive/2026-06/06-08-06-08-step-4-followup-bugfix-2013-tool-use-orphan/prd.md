# 修复: 2013 tool_use 孤儿 (cancel 路径 + rehydrate 历史)

## 背景

attach worktree 之后让 LLM 改文件报 MiniMax 错误码 2013：

```
status=400 Bad Request
body={"error":{"type":"<nil>","message":"invalid params, tool call result does not follow tool call (2013)"},"type":"error"}
```

## 根因

`app/src-tauri/src/lib.rs` 的 cancel 路径（PR5 Stop 按钮 / `attach_worktree` 的 cancel hook / 网络中断）下：

1. LLM 流式输出 `tool_use`（SSE → `ChatEvent::ToolCall`），**已被持久化**到后端 `tool_calls: Vec<(id, name, input)>` 累积，并先后端构造 `assistant_blocks` → `persist_turn` 写入 `messages` 表
2. **在这个时点** cancel 触发（`token.cancelled()`），agent loop `break` → `return`
3. `lib.rs:1520` 的 `tool_result_msg` 构造 + `messages.push` + `persist_turn` **永远不执行**
4. DB 留下 `seq=N: assistant(tool_use)` 但 `seq=N+1: user(tool_result)` 永远不写
5. attach 之后 user 再次 send，前端 `controller.refresh` 从 DB 重新 load，rehydrate 出**孤儿 tool_use** 推到 history
6. 下一轮 LLM 看到 `assistant: [tool_use(id=X)]` 后面是 `user: "新消息"`，缺 `tool_result(id=X)` → API 2013

**为什么跟具体工具无关**：tool_use.id 是 LLM 生成的字符串（`toolu_xxx`），跟 read_file / edit_file / shell 哪个工具无关。哪个工具被 cancel 阻断都会触发。

**为什么"attach 之后"才报**：attach 触发的 in-flight cancel 是常见触发点；用户用 Stop 按钮也会触发，但 Stop 之后用户**通常不会立刻 send**（先看消息），attach 之后立刻发新请求更常见。

**为什么 `docs/HACKING-llm.md:189-211` "陷阱 2" 修过同类问题但没覆盖**：
- 陷阱 2：assistant role 含 `tool_result` 块 → 2013 "tool result's tool id not found"（**tool_result 错位**）
- 本 bug：assistant tool_use 后没 `tool_result` → 2013 "tool call result does not follow tool call"（**tool_result 缺失**）
- 修法独立：陷阱 2 用 `toPayloadContent` 按 role 分发；本 bug 用 cancel 路径补 result + rehydrate 治历史

## 修复方案：B + C 双层

### B: 后端 cancel 路径补 synthetic tool_result

**位置**：`app/src-tauri/src/lib.rs` `chat` 命令的 cancel 分支（line 1342-1444 区域）

**做法**：在 `cancelled = true` 触发后，构造 `tool_result_msg` 时为每个 `tool_call` 生成一个 synthetic `ContentBlock::ToolResult`，`is_error: true`，`content: "Tool execution was interrupted: the user stopped the request or the session was cancelled before the tool could run. The tool <name> did not run."`。把这条 user role message 跟原 assistant message 一起 persist，然后正常 return。

> **文案决策（user 已选）**：英文 + tool name。理由：LLM 兼容层是英文协议，synthetic tool_result 直接喂给 Anthropic API 跟英文提示符 prompt 风格一致；带 tool name 让 LLM 知道哪个工具没跑（避免 LLM 以为 read_file 跑了但只是 result 是 "interrupted"）。

**效果**：
- DB 序列: `seq=N: assistant(text + tool_use)` → `seq=N+1: user(tool_result: "interrupted", is_error=true)` 自洽
- 前端 rehydrate 时按正常路径 merge + 发送
- LLM 看到 tool_use 配对了 "interrupted" tool_result，知道工具没跑

**注意**：
- 顺序必须：先 `messages.push(assistant_msg); seq+=1`，再构造 `tool_result_msg`，再 `messages.push(tool_result_msg); seq+=1`，**这跟正常的 turn 切换顺序一致**
- `current_ctx.cwd` / `worktree_path` 用 cancel 时刻的值（已经被 turn_ctx 初始化过）
- 不要 emit `tool:result` Tauri event — 那是给前端"刚执行完"的 UX 提示用的，cancel 场景下没意义
- emit `done { stop_reason: "cancelled" }` 保持不变

### C: 前端 rehydrate 治历史孤儿

**位置**：`app/src/stores/streamController.ts` `rehydrateMessages` 函数（line 150-223）

**做法**：在 merge step 之后（line 187-200），扫一遍 `out` 数组，对每个 `toolCalls` 非空但下一条**不是** user role with matching `toolResults` 的 assistant message，**插入**一条 synthetic user role message with `toolResults`，内容跟后端 B 用的相同文案。

**插入规则**：
- 找到 assistant message `out[j]` 有 `toolCalls: [{id: "X"}]` 但 `out[j+1]` 要么不存在、要么 role !== user、要么是 user 但 `toolResults` 不含 id="X"
- 在 `out[j+1]` 位置 splice in 一条 synthetic message：`{ role: "user", toolResults: [{ toolUseId: "X", content: "Tool execution was interrupted: ...", isError: true }] }`
- 多个孤立 tool_use 在同一 assistant message 上的话，synthetic message 的 toolResults 数组按 toolCalls 顺序排列

**效果**：
- 旧 DB 里的孤儿 tool_use（cancel 留下的 / 网络断留下的）下次 send 时被自动补全
- 后端 B 修完上线后**新产生的孤儿是 0**，但用户本地 DB 里可能已有历史孤儿，C 治本

**不**影响：
- 正常 `assistant(tool_use) → user(tool_result)` 配对（merge step 已处理，synthetic step 检测不到）
- `[worktree event]` 系统消息（不含 tool_use / tool_result 块）

## 验收标准

- [ ] **AC-1: B 单元测试** — `cargo test` 加 `chat::cancel_persists_synthetic_tool_result`：
  - mock LLM stream 返回 `tool_use(id="X", name="read_file", input={path: "/foo"})`
  - 在 tool_use 到达后立即 cancel
  - 验证 DB 里有两条连续消息：`assistant` 含 ToolUse 块 + `user` 含 ToolResult(tool_use_id="X", is_error=true)
- [ ] **AC-2: C 单元测试** — vitest（如果项目有 vitest）或新加 vue-tsc-checked TS test：
  - 构造 `rehydrateMessages` 输入：assistant(toolCalls=[X])，后面无 user(toolResult=X)
  - 验证输出数组里：assistant 之后是 synthetic user(toolResults=[{toolUseId:"X", isError:true}])
- [ ] **AC-3: 现有 `rehydrateMessages` 配对测试不退化** — `streamController.test.ts`（如存在）加 case：assistant(tool_use) + user(tool_result) 不被 C 重复处理
- [ ] **AC-4: 端到端验证** — 手工操作：
  - 启动 app → 创建 session → 让 LLM 调 `read_file` → 在 tool_use 到达但 tool_result 还没回时点 Stop
  - attach worktree → 再让 LLM 调 `read_file`
  - 不再 2013，LLM 正常回复
- [ ] **AC-5: 文档更新** — `docs/HACKING-llm.md` 加 "陷阱 3" 节：tool_use 孤儿（cancel 路径）→ 2013，记录 B + C 修法和 trade-off
- [ ] **AC-6: spec 更新** — `.trellis/spec/backend/llm-contract.md` Scenario 7 加一段：cancel 路径的 tool_result 契约（如果存在）
- [ ] **AC-7: 编译 + 现有测试全绿** — `cargo test` + `pnpm build` + 现有 vitest 全过，无新增 warning

## Definition of Done

- AC-1 ~ AC-7 全部 ✓
- `cargo test` 全过
- `pnpm build`（含 vue-tsc --noEmit）全过
- 单 commit（squash）落地，commit message 风格跟 `6f3d557` 一致（"fix: 2013 tool_use orphan from cancel path" + bullet 列表）
- archive 当前 task 到 `.trellis/tasks/archive/2026-06/06-08-step-4-followup-bugfix-2013-tool-use-orphan/`
- `.trellis/workspace/Carlos/journal-1.md` 记录本次修复

## 风险

- **synthetic tool_result 文案选择（已确定）**：英文 + tool name。带 name 帮 LLM 知道是哪个工具没跑；走英文是因为 synthetic content 直接进入 Anthropic 协议流，跟 LLM-compatible 提示符风格一致
- **C 插入位置**：splice 后会改变 out 数组的索引，需要重做 `for (let i = 0; ...)` 的扫描方向（建议从后往前扫，避免 splice 错位）
- **C 影响 markRaw**：synthetic message 也需要 markRaw toolResults 数组（跟正常 rehydrate 一致）

## 范围外

- 修复历史的孤儿 DB 行（脚本化 patch）— C 在 rehydrate 阶段治本即可，不需要后端迁移
- 改 LLM streaming 协议用 stop_reason="cancelled" 时跳过 tool_use 块（更激进，但会丢信息）
- 跨 session 的孤儿检测（tool_use id 跨 session 重用概率 0，不考虑）
- 端到端 vitest 自动化（项目无 vitest 配置，本任务用 cargo test + 手工 e2e 覆盖）

## Decision (ADR-lite)

**Context**: 2013 "tool call result does not follow tool call" 错误有两个修复点：(1) 后端 cancel 路径不再产生孤儿 tool_use，(2) 前端 rehydrate 治历史孤儿。需要决定修哪几层、文案风格、commit 粒度。

**Decision**:
1. **B + C 双层修**：B 后端 lib.rs:1342 cancel 分支补 synthetic tool_result；C 前端 streamController.ts:150 rehydrate 治历史。
2. **文案**：英文 + tool name (`"Tool execution was interrupted: ... The tool <name> did not run."`)
3. **commit**：单 commit squash（跟 `6f3d557` 风格一致）

**Consequences**:
- DB 序列从"assistant(tool_use) → user(新消息)"不连续 变成 连续 → 不再 2013
- 旧 session 的孤儿 tool_use 也会被前端 rehydrate 自动补全 → 用户不需要手动清理 DB
- LLM 看到 `is_error: true` 的 tool_result 知道工具没跑 → 不会重发同一个 tool_use

## 技术备注

- `HACKING-llm.md:189-211` 陷阱 2 是"tool_result 错位"修法（已修），本任务是"tool_result 缺失"修法
- `lib.rs:1234` `cancelled` 标志在 streaming 内层循环置位，`lib.rs:1352` 检测
- `lib.rs:1520-1531` 正常 tool_result 构造路径参考
- `streamController.ts:187-200` merge step 是 C 的插入点参考
- `db.rs:1118` `persist_turn` 已支持任意 `MessageContent` 序列化
- `ContentBlock::ToolResult` 序列化时 `is_error: false` 字段被 `skip_serializing_if` 过滤（`types.rs:71`），所以 `is_error: true` 会出现在 JSON 里 — LLM 会感知
