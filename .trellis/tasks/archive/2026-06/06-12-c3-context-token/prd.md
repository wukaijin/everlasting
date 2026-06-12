# C3 Context 压缩 + Token 预算管理

## Goal

在 agent loop 中引入 token 预算感知机制，当对话历史接近模型 context window 上限时，自动裁剪老消息，替代当前的 `MAX_TURNS=20` 硬编码安全阀（改大至 50 作为兜底）。这是 ARCHITECTURE.md §2.5.5 ⑤ Context 超限降级的 MVP 实现。

## Decisions (ADR-lite)

**Context**: Agent loop 当前硬编码 `MAX_TURNS=20`，没有 token 预算管理，长对话会撞 API context window 上限报错。

**Decisions**:
1. **Context window 来源**：直接读 `models.context_window`（DB catalog 字段已存在，`ResolvedChatProviderWrapper` 已能拿到）
2. **压缩策略**：MVP 做简单裁剪（丢老消息），不做 LLM summarization（留给 C3-v2）
3. **MAX_TURNS**：保留为兜底，从 20 改大到 50
4. **触发阈值**：`context_window * 0.80` 触发；裁剪目标 `context_window * 0.50`
5. **保护优先级**：system_prompt > B5 synthetic memory (messages[0..1]) > 当前 user message > 老 runtime tool_result > 老 user/assistant turn
6. **配对保护**：assistant(tool_use) + user(tool_result) 必须成对丢，避免 API 400
7. **不丢**：Thinking / RedactedThinking blocks（signature 对不上会 400）
8. **UX**：MVP 只做 `tracing::info!` 日志；前端 UI 标记（"context compressed at turn N"）分到第二个 PR
9. **被裁剪消息去向**：完全丢弃（不持久化 compressed_out 标记；审计回看留给 C4）

**Consequences**:
- 长对话早期上下文会丢失，但 agent coding 场景中早期消息通常价值低
- token 估算用 tiktoken cl100k_base 有 1-2% 漂移，0.80/0.50 阈值留足余量
- 50 轮兜底覆盖极端 case，正常 token 预算会先触发

## Requirements

* `ResolvedChatProviderWrapper` 暴露 `context_window: u32` 给 agent loop
* Agent loop 每次 `provider.send()` 前估算 messages 总 token 数
* 当估算值 ≥ `context_window * 0.80` 时触发裁剪，目标降到 `context_window * 0.50`
* 裁剪按保护优先级 + 配对保护执行
* MAX_TURNS 从 20 改大到 50
* 压缩发生时 `tracing::info!` 记录：turn 号、压缩前后 token 数、丢弃的消息数

## Acceptance Criteria

* [ ] Rust 单元测试覆盖裁剪逻辑（3 个 case：未触发 / 触发后降到目标 / 配对保护）
* [ ] 长对话场景不撞 API context window 上限（手动验证）
* [ ] B5 memory synthetic message 永远不被裁剪
* [ ] Thinking / RedactedThinking blocks 永远不被裁剪
* [ ] assistant(tool_use) + user(tool_result) 配对完整
* [ ] `cargo check` + `cargo test --lib` green
* [ ] MAX_TURNS=50 兜底仍能工作（单元测试）

## Definition of Done

* Rust 单元测试覆盖裁剪核心逻辑
* `cargo check` + `cargo test --lib` green
* ARCHITECTURE.md §2.5.5 标注"已实施 C3 MVP"
* ROADMAP.md §1.2 加 C3 行 + commit hash

## Out of Scope

* LLM summarization（C3-v2）
* 前端 "context compressed" UI 标记（分到 PR2）
* compressed_out DB 列 + 完整历史回看（C4 审计日志覆盖）
* 流式 token 实时预算扣减
* OpenAI o1/o3 thinking token 特殊处理
* token 估算缓存（每轮重算即可，性能不是瓶颈）

## Technical Approach

### 实施分 PR

**PR1: 核心裁剪逻辑（纯 Rust，无 IPC 改动）**
- 新增 `app/src-tauri/src/agent/context.rs`：
  - `estimate_messages_tokens(messages: &[ChatMessage]) -> u32` — 调 `memory::tokens::count_tokens`
  - `compact_messages(messages, context_window, target_ratio) -> CompactResult` — 裁剪函数
  - 配对保护 + 优先级算法
  - 完整单元测试
- `agent/mod.rs`：`MAX_TURNS` 20 → 50；导出 `context` 模块
- `agent/chat.rs`：
  - `ResolvedChatProviderWrapper` 加 `context_window: u32` 字段
  - `lookup_provider_for_session` 填充该字段
  - agent loop 每次 `provider.send()` 前调 `compact_messages`，传 `context_window`
  - 压缩发生时 `tracing::info!`

**PR2（可选，本任务不做）：前端 UI 标记**
- ChatInput hint 区加 "context compressed at turn N" 小标记
- 复用 A4 token hint 渲染路径

### 关键算法（配对保护）

```text
messages = [memory_synthetic, assistant_ack, ...runtime_turns, current_user]

# 1. 锁定不动段：[memory_synthetic, assistant_ack] + 最后一条 current_user
# 2. 中间段按 turn 分组：(assistant_with_tool_use, user_with_tool_result) 成对
# 3. 从最老的 turn 开始丢，直到 token ≤ target
# 4. 单独的 user/assistant turn（无 tool 配对）单独丢
# 5. Thinking blocks 在 assistant turn 内，整 turn 丢时一起丢（不会出现"丢一半"）
```

## Technical Notes

* 关键文件：
  - `app/src-tauri/src/agent/chat.rs`（agent loop 主循环，~1200 行）
  - `app/src-tauri/src/agent/mod.rs`（MAX_TURNS 常量）
  - `app/src-tauri/src/agent/provider.rs`（ResolvedChatProviderWrapper 定义）
  - `app/src-tauri/src/memory/tokens.rs`（`count_tokens` 已存在）
  - `app/src-tauri/src/db/types.rs:90`（ModelRow.context_window 字段）
* ARCHITECTURE.md §2.5.5 定义保护优先级
* Provider trait 的 `send()` 接口不变
* Anthropic API 报 `invalid_request_error` 当 input_tokens 超限
* tiktoken cl100k_base 与 Anthropic 实际 tokenizer 有 1-2% 漂移，0.80 阈值已留足余量

## Research References

* 无外部 research — 决策基于现有代码 + ARCHITECTURE.md §2.5.5 已有设计
