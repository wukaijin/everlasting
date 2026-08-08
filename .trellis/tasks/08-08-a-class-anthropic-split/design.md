# A类单体重构:anthropic provider 拆分 — Design

## 1. 目标形态

`chat_stream_with_tools`(~430 行)→ 2 个辅助函数 + 5 个事件 handler 纯函数 + 宏体骨架(~190 行)。

```
chat_stream_with_tools(config, body) -> impl Stream
  ├─ request_log_fields(body) -> (String, usize, bool)              // 阶段 0(无 yield)
  ├─ send_request(config, url, body) -> Result<Response, LlmError>  // 阶段 A-D(无 yield,async)
  └─ stream! 宏体:
       ├─ client/HTTP 骨架(调用 send_request,yield Err 点)
       ├─ 初始化 + chunk 读取/utf8(含 yield,保留)
       ├─ 事件分发(每类事件 → handler 调用 + 统一 yield)
       └─ Done(含 yield,保留)
```

## 2. 提取契约(每个函数严格对应原代码连续段,无 yield)

| 函数 | 对应原段 | 返回类型 | 签名 | 说明 |
|---|---|---|---|---|
| `request_log_fields` | L149–165 | `(String, usize, bool)` | `fn request_log_fields(body: &serde_json::Value) -> (String, usize, bool)` | url 留在宏体(config 借用);log_model/log_tools_count/log_has_system 提取 |
| `send_request` | L181–227 | `Result<...>` | `async fn send_request(config: &LlmConfig, url: &str, body: &serde_json::Value) -> Result<reqwest::Response, LlmError>` | client 构建 + `→ LLM request` 日志 + post + 状态检查 + `classify_error_response`;yield Err 点 → `Err(...)` 返回,宏体 `match { Err(e) => { yield Err(e); return; } }` |
| `handle_content_block_start` | L268–339 | `()` | `fn handle_content_block_start(data: &str, block_state: &mut BlockState)` | 块类型分发(tool_use/thinking/redacted_thinking/默认 Text),纯状态转换,无即时事件(**无 yield**,返回 `()`);**JSON 解析失败时静默跳过(原代码行为,无 else 分支),不引入新日志** |
| `handle_content_block_delta` | L342–414 | `Option<ChatEvent>` | `fn handle_content_block_delta(data: &str, block_state: &mut BlockState) -> Option<ChatEvent>` | text_delta → `Some(Delta)`;input_json_delta → 累积 json_buf,`None`;thinking_delta → 累积 + `Some(ThinkingDelta)`;signature_delta → 累积,`None`;unknown → debug 日志,`None` |
| `handle_content_block_stop` | L417–490 | `Option<ChatEvent>` | `fn handle_content_block_stop(block_state: &mut BlockState) -> Option<ChatEvent>` | `mem::replace` 状态机终结:ToolUse → `Some(ToolCall)`(JSON 解析容错);Thinking → 签名非空则 `Some(SignatureDelta)`;RedactedThinking → 数据非空则 `Some(RedactedThinkingDelta)`;Text/Idle → `None` |
| `handle_message_delta` | L493–525 | `()` | `fn handle_message_delta(data: &str, stop_reason: &mut Option<String>, usage: &mut Option<TokenUsage>)` | stop_reason 提取;`parse_anthropic_usage` 结果若 `Some(u)` 则覆盖 `usage`(**None 时保留原值**);无即时事件(返回 `()`) |
| `handle_message_start` | L538–556 | `()` | `fn handle_message_start(data: &str, usage: &mut Option<TokenUsage>)` | usage 基线(message.usage / 顶层 usage,仅 `usage.is_none()` 时写入);无即时事件(返回 `()`) |

**宏体保留**(含 yield,不可提取):事件分发骨架(L265–267 循环 + 各事件分支的 `if let Some(ev) = handler(...) { yield Ok(ev); }`)、chunk 读取/utf8 错误路径(L250–263)、message_stop/ping/other 日志分支(L558–566)、Done(L571)、初始化(L232–247)。

**行为等价性论证**:
- 每个 handler 是原 match 分支的逐行平移,`yield` 点收敛为宏体的统一 `if let Some(ev)` 模式 —— yield 的事件内容与顺序与原代码逐一对应(Delta/ThinkingDelta 直接 yield、ToolCall/SignatureDelta/RedactedThinkingDelta 在 stop 时 yield、Done 收尾)。
- `send_request` 内日志与错误分类原样;宏体 `Err` 分支与原 `yield Err + return` 逐一对应。
- tracing 日志原样保留(位置:start 日志入 send_request、事件 debug 日志入对应 handler)。

## 3. 文件布局(Rust 2018 module 模式)

```
llm/provider/anthropic.rs          # hub:LlmConfig + DEFAULT_MAX_TOKENS + AnthropicProvider(new + chat_stream_with_tools 骨架) + impl Provider + 3 纯函数 + re-export
llm/provider/anthropic/events.rs   # BlockState + 5 个事件 handler(事件状态机)
llm/provider/anthropic/transport.rs# request_log_fields + send_request(HTTP 传输)
llm/provider/tests_anthropic.rs    # 原内联 mod tests 迁出(文件级 #![cfg(test)] 门控)
```

- hub 声明 `pub(crate) mod events; pub(crate) mod transport;` + `#[allow(unused_imports)] pub(crate) use events::*;` 等全量 re-export(对照 dispatch.rs batch1 惯例)。
- `provider/mod.rs` 的 `pub use anthropic::AnthropicProvider;` + `anthropic::LlmConfig{...}` 字段构造 + `anthropic::DEFAULT_MAX_TOKENS` 路径不变(hub 保留定义)。
- 子模块内引用 wire/llm 符号:`crate::llm::...` 全路径或 `super::super::...`。

## 4. 测试组织

- 原内联 `mod tests`(L945–1525)整体迁出为 `tests_anthropic.rs`,`use super::*` 改 `use super::anthropic::*`(测试文件在 `llm/provider/` 下,`super` = provider 模块,`super::anthropic` 是 hub)。
- **新增 handler 测试(AC7,纯逻辑零覆盖)**:
  1. `handle_content_block_stop` ToolUse 终结 → `ToolCall`(JSON 解析成功 + 空 buf 默认 `{}`)
  2. `handle_content_block_stop` Thinking 空签名 → `None`(非空 → `SignatureDelta`)
  3. `handle_content_block_start` 三类块 → 对应 BlockState 转换
  4. `handle_message_delta` usage 覆盖语义(后到覆盖先到)
  这些测试直接构造 `data` JSON 字符串与 `BlockState` 初值,不依赖网络。

## 5. 风险与回滚

| 风险 | 缓解 |
|---|---|
| `stream!` 宏内提取破坏编译(yield 泄漏)| handler 严格无 yield(纯函数签名强制);每提取 commit 后 `cargo check` 即时报 + 全量测试 |
| 事件顺序/内容漂移 | handler 逐行平移原 match 分支;宏体统一 `if let Some(ev) = handler(...) { yield Ok(ev); }` 模式与原 yield 点一一对应;1657 基线测试 + 新增 handler 测试锁定 |
| `LlmConfig` 字段构造路径断(provider/mod.rs)| hub 保留 LlmConfig 定义与 pub 字段,零改动 |
| 测试迁出后 `use super::*` 断 | tests_anthropic.rs 用 `use super::anthropic::*`(hub re-export 全量) |
| 内联测试与 `#[cfg(test)]` 门控 | 文件级 `#![cfg(test)]`(批1-3 惯例) |
| pattern-large-function-split gotcha 1(`#[allow(unused_imports)]` 仅作用于单个 use 语句) | events.rs / transport.rs 复制 use 集时用文件级 `#![allow(unused_imports)]`(inner attr) |
| pattern-large-function-split gotcha 2(`use super::x` 跨级) | 子模块内引用 wire/llm 符号用 `crate::llm::...` 全路径或 `super::super::...` |
| pattern-large-function-split gotcha 3/4(主函数前 doc 被吞 / 参数签名行数口径) | **N/A**:handler 提取不切主函数头部;chat_stream_with_tools 参数 ≤5,AC1 口径"宏体 ≤220 行"已明确 |

**回滚**:每个提取 commit 独立 `git revert`;拆分 commit 前先 `cargo check` 验证。

## 6. 明确不做

- 不改 `chat_stream_with_tools` 签名 / Stream 类型 / `yield` 顺序(PRD Out of Scope)。
- 不拆 `send`(130 行非单体)、3 个已有纯函数。
- 不引入 ctx struct / 流式状态打包(handler 用 `&mut` 参数,保持宏体可读)。
