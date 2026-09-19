<!-- Moved from llm-contract.md 2026-09-19 (doc-split) -->

## Scenario: DeepSeek-Via-Anthropic-Relay thinking block fix (RULE-D-003, 2026-06-20)

###1. Scope / Trigger

- Trigger: DeepSeek-v4 thinking mode (`deepseek-v4-flash` / `deepseek-v4-pro` / `deepseek-reasoner` alias) 经中转站（如 wukaijin.com — `https://api.wukaijin.com`）以 Anthropic Messages API 协议 (`POST /v1/messages`) 访问时，多轮对话第二轮起持续 400：
  ```
  {"error":{"type":"invalid_request_error","message":"Error from provider (DeepSeek): The `reasoning_content` in the thinking mode must be passed back to the API. (request id: ...)"}}
  ```
- Why code-spec depth: mandatory — 中转站 thin passthrough Anthropic schema 到 DeepSeek V4 后端，DeepSeek V4 thinking mode 契约要求 assistant message 带顶层 `reasoning_content` 字段；Anthropic 标准 `thinking` block + `signature` 单独不够（cumul. state 校验失败）。

###2. Root Cause

- 中转站对 assistant message 中的 thinking block 做累积状态校验（threshold 不稳定，与 thinking block 数量 / token / UUID signature 数量 / cache 状态综合相关），校验失败时报 "reasoning_content must be passed back"。
- Anthropic SSE 解析侧：中转站用 **UUID v4 字符串**（如 `c556ef17-b531-4366-9477-ebc7bdc29b9b`）作 thinking block `signature` 字段，不是 Anthropic 原生 base64 加密 blob。empty signature (`""`) 是 SSE `signature_delta` 事件未到达时的 fallback。
- `wukaijin.com` 对 empty-signature 块**不**做 reasoning_content 校验（DB 反推：`e9bf6c07` turn 0-1 empty sig 仍 work），对非空 UUID sig 块**做**校验（`0a8cc2f0` turn 1 empty + turn 2 UUID → turn 3 400）。

###3. Fix Contract

实现位于 `app/src-tauri/src/llm/provider/anthropic.rs::apply_deepseek_reasoning_fix`，pub(crate) 纯函数：

```rust
pub(crate) fn apply_deepseek_reasoning_fix(req: &ChatRequest) -> serde_json::Value
```

对每条 `role == "assistant"` 消息（`content` 必须是 block 数组形式；string content 直接跳过）：

1. **Filter (B)** — 从 `content[]` 移除所有 `{"type":"thinking","signature":""}` 块（含 signature 字段缺失 — `unwrap_or("")` 统一视为空）。保留 text / tool_use / thinking-with-non-empty-signature / redacted_thinking 块。块顺序保留（tool_use / text 交错不变）。
2. **Inject (A)** — 收集所有**保留下来**的 thinking block 的 `thinking` 文本，多块用 `\n` 拼接；若结果非空，在 message 顶层加 `reasoning_content: String` 字段（与 `content` 同级，Anthropic 协议非标扩展）。全空时**不**加 `reasoning_content: ""` 字段（避免 relay sentinel mismatch）。

`apply_deepseek_reasoning_fix` 在 `AnthropicProvider::send` 末尾调用，输出 `body: serde_json::Value` 传入 `chat_stream_with_tools(config, body)` 替代 `req: ChatRequest`。`chat_stream_with_tools` 签名从 `(config, req: ChatRequest)` 改为 `(config, body: serde_json::Value)`，HTTP POST 从 `.json(&req)` 改为 `.body(body.to_string())`。

###4. Anthropic 原生路径兼容性

- Anthropic 标准 `/v1/messages` API 接受未知字段（serde 默认行为），`reasoning_content` 字段被忽略，extended thinking 行为 1:1 等价。
- 顶层 `thinking: adaptive` 字段保留不动（Claude extended thinking 必需 — D 方案/FT-D-001 跟进）。
- `OpenAIProvider` 路径完全未触碰（R4）— 验证 `cargo test --lib openai::` 35 passed。
- 顶层 `tracing::info!` log 字段（`model` / `tools_count` / `has_system`）从 `body` JSON 提取，log 内容与 pre-fix 等价。

###5. Evidence (DB 反推)

4 个 DeepSeek-via-wukaijin session 对比（`session_audit_events` + `messages.created_at` + Anthropic SSE 解析 `signature` 字段值）：

| Session | mode | turn 0 | turn 1 | turn 2 | turn 3 | turn 4 | turn 5 | 结果 |
|---|---|---|---|---|---|---|---|---|
| 0a8cc2f0 | yolo | empty | UUID | **400** | - | - | - | ❌ |
| 053ae61e | yolo | UUID | UUID | UUID | UUID | **400** | - | ❌ |
| 11cefabc | edit | empty | empty | UUID | empty | UUID | **400** | ❌ |
| e9bf6c07 | yolo | empty | empty | UUID | UUID | UUID | UUID→empty | ✅ |

empty sig + UUID sig 混合 / 全 UUID 都可能 400；具体 threshold 不稳定。修复通过**降低触发因子**（减少 thinking block 数量 + 显式提供 `reasoning_content` 字段）规避 400。

`AuditKind` 14 variant 全部是 permission / mode / tool / edit 类，**没有 LlmError / NetworkError / ProviderError**（`app/src-tauri/src/agent/permissions/mod.rs:152`）—— LLM 错误**只**走 `tracing::warn!`（`anthropic.rs::send_request`），不进 `session_audit_events`。LLM 错误时间定位**只能靠** `messages.created_at` 的 `ERROR_MARKER`（`app/src-tauri/src/agent/helpers.rs:307` = `"[生成出错中断]"`）。

###6. Tests Required

`app/src-tauri/src/llm/provider/anthropic.rs` `#[cfg(test)] mod tests` 末尾 7 个新单测（`deepseek_reasoning_fix_*` 前缀）：

- `removes_empty_sig_thinking_blocks` — empty 块被移除，text/tool_use 保留
- `omits_reasoning_content_when_all_empty` — 全 empty 时**不**加 `reasoning_content` 字段
- `keeps_nonempty_sig_and_adds_reasoning_content` — 单 UUID 块加 `reasoning_content`
- `concatenates_multiple_nonempty_blocks` — 多块用 `\n` 拼接
- `skips_user_messages` — user 消息完全不动（R4）
- `no_thinking_blocks_no_reasoning_content` — 纯 text + tool_use 不加 `reasoning_content`
- `preserves_top_level_thinking_field` — 顶层 `thinking: adaptive` 字段保留

`cargo test --lib` → **739 passed; 0 failed**（anthropic 18 + openai 35 + wire 20 + 其他 666；OpenAI 路径完全未触碰，符合 R4）。

###7. Out of Scope (Follow-up Tasks)

- **FT-D-001**: 调查 Anthropic 顶层 `thinking` 字段对 DeepSeek V4 后端的影响（D 方案 — 移除顶层 `thinking: adaptive` 是否会改变 400 行为；需要更直接 evidence 才能动 Claude extended thinking 路径）
- **FT-D-002**: 调查 wukaijin.com 400 threshold 的精确机制（DB 4 session 对比表明 threshold 不稳定，需要按 relay 分类的实测数据）
- **FT-D-003**: 评估是否需要按 relay 自动分发 capability（heuristic 或新 ModelRow 字段 `disable_reasoning_content_inject`），让 strict Anthropic relay 不接收 `reasoning_content` 顶层字段

---

