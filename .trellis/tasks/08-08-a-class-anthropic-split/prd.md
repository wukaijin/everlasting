# A类单体重构:anthropic provider 拆分

## Goal

把 `app/src-tauri/src/llm/provider/anthropic.rs`(1525 行)中 `chat_stream_with_tools`(~490 行 `stream!` 生成器)按"无 yield 纯函数提取"模式拆分:事件处理逻辑提取为纯 handler 函数,宏体保留骨架;再按 Rust 2018 module 模式子模块化(hub + `anthropic/` 子目录)。行为零变化,复用 `pattern-large-function-split`(dispatch 专项沉淀)方法论。

## Background / 已确认事实

- `anthropic.rs` 结构(1525 行):
  - `LlmConfig`(L55–82)+ `DEFAULT_MAX_TOKENS`(L49)— 配置,被 `provider/mod.rs` 以 `anthropic::LlmConfig` / `anthropic::DEFAULT_MAX_TOKENS` 路径引用(mod.rs L47/152/157,字段直接构造 → **pub 字段与 re-export 路径必须保持**)
  - `BlockState` enum(L91–113)— 事件状态机(Idle/Text/ToolUse/Thinking/RedactedThinking),仅被 chat_stream_with_tools 内部使用,无直接测试
  - `AnthropicProvider` + `impl`(L124–574):`new`(12 行)+ **`chat_stream_with_tools`(L145–573,~430 行)**
  - 3 个纯函数:`apply_deepseek_reasoning_fix`(L633–709,已有 **8** 个测试:7 个 reasoning_fix + 1 个 deepseek_relay_contract_v1_v2_v3)、`apply_speaker_prefix`(L710–758)、`parse_anthropic_usage`(L759–795,已有 4 个测试)
  - `impl Provider for AnthropicProvider`(L796–944):`send`(~130 行,请求组装,非单体——本任务范围外)、`capabilities`、`protocol`
  - **内联 `mod tests`(L947–1525,~580 行,`#[cfg(test)]`)**——与 dispatch 不同,不是独立 tests_ 文件;测试覆盖配置/纯函数,**无 chat_stream_with_tools / BlockState / 事件处理直接测试**
- `chat_stream_with_tools` 是 `async_stream::stream!` 宏生成器,返回 `impl Stream`;**`yield` 只能在宏体内**,含 yield 的代码无法提取为普通函数——拆分策略 = "事件处理逻辑提取为无 yield 纯函数(返回待 yield 的 `Option<ChatEvent>`),宏体保留骨架 + yield 点"。
- 函数内部阶段:
  - 阶段 0:url + observability 字段(L149–165)
  - 阶段 A:client 构建(L181–191,yield Err)
  - 阶段 B:请求日志(L193–199)
  - 阶段 C:HTTP 发送(L201–216,yield Err)
  - 阶段 D:错误状态处理(L218–227,classify_error_response,yield Err)
  - 阶段 E:Start 事件(L229–230)
  - 阶段 F:初始化(L232–247)
  - 阶段 G:事件循环(L249–569):chunk 读取 + utf8(L250–263)+ 事件分发(L265–567:content_block_start/delta/stop、message_delta/start/stop、ping)
  - 阶段 H:Done(L571)
- 前序:批1-3 B 类拆分 + batch1(dispatch)A 类拆分已沉淀 `pattern-large-function-split.md` spec(两阶段模式 + 4 个 gotcha)。
- 基线:`cargo test --lib` = 1657 全绿;`clippy --lib --tests` + `fmt --check` 零警告。

## Requirements

- R1 事件处理逻辑提取为**无 yield 纯函数**(接收 `&mut BlockState` / `&mut Option<...>`,返回待 yield 的 `Option<ChatEvent>` 或 `()`),宏体保留骨架 + yield 点;每个提取独立 commit、独立回滚。
- R2 按 Rust 2018 module 模式拆分:`anthropic.rs` 保留为 hub(re-export 全量),子文件进 `anthropic/` 子目录;`provider/mod.rs` 的 `anthropic::LlmConfig` / `anthropic::DEFAULT_MAX_TOKENS` / `anthropic::AnthropicProvider` 路径解析不变。
- R3 行为零变化:yield 事件顺序、tracing 日志、错误路径(yield Err + return)、请求/响应处理顺序均不变。
- R4 测试:内联 `mod tests` 迁出为 `tests_anthropic.rs`(文件级 `#![cfg(test)]` 门控,`use super::*` 经 hub 解析);提取的事件 handler 为**纯逻辑且当前零覆盖**,补少量针对性单元测试(状态机转换表)。
- R5 被测私有项升 `pub(crate)`,禁止为可测性改公开 API。
- R6 文档引用 sweep:`anthropic.rs:LINE` 行号引用改符号引用;archive/历史快照不改。
- R7 收尾 `cargo fmt` + `clippy --lib --tests` + `fmt --check` 零警告;`cargo test --lib` 全绿(1657 基线 + 新增 handler 测试)。

## Acceptance Criteria

- [ ] AC1 `chat_stream_with_tools` 的 `stream!` 宏体 ≤ ~220 行:只剩 client/HTTP 骨架、chunk 读取、事件分发(每类事件一个 handler 调用 + 统一 yield 点)、Done。
- [ ] AC2 `anthropic.rs` 为 hub + `anthropic/{events,transport}.rs` 子模块;re-export 后 `provider/mod.rs` 既有路径(`anthropic::LlmConfig` 等)解析不变。
- [ ] AC3 每个提取/拆分 commit 单独可回滚(独立 commit)。
- [ ] AC4 `cargo test --lib` 全绿(≥1657:1657 基线 + 新增 handler 测试);`cargo fmt --check` + `clippy --lib --tests` 零警告。
- [ ] AC5 无公开 API 变更(新增项均 `pub(crate)`;`LlmConfig` 字段可见性不变)。
- [ ] AC6 非 archive 文档/注释无残留 `anthropic.rs:LINE` 行号引用。
- [ ] AC7 新增事件 handler 单元测试 ≥4 个:content_block_stop 状态机终结 ×2(含空 buf 默认 `{}`)+ content_block_start 块类型分发 ×1 + handle_message_delta usage 覆盖语义 ×1,全部通过。

## Out of Scope

- 不改 `chat_stream_with_tools` 签名 / `yield` 事件顺序 / 返回 Stream 类型。
- 不拆 `impl Provider::send`(~130 行非单体,判定:含 doc 注释 ~30 行、纯顺序请求组装无嵌套控制流——A 类判定阈值见 `pattern-large-function-split.md` §Problem">1000 行 + 多层控制流")与 `apply_deepseek_reasoning_fix` / `apply_speaker_prefix` / `parse_anthropic_usage`(已独立)。
- 不做 `sink.rs` / `chat_loop.rs`(后续专项)。
- 不新增 feature / 不修 bug / 不改行为。

## 已决决策(沿用 batch1,2026-08-08)

- D1 执行节奏:先提取(anthropic.rs 内部,每阶段独立 commit + 中间全量 cargo test)→ 再拆分(文件移动 + 测试迁出,单 commit)→ 文档 sweep。
- D2 事件 handler 形态(评审 P1-2 修订):`fn handle_*(data: &str, state: &mut ...) -> ()` 或 `-> Option<ChatEvent>`,**返回类型视原代码该分支是否 yield 决定**(start/message_delta/message_start 无即时事件 → `()`;delta/stop 有 yield 点 → `Option<ChatEvent>`);宏体统一 `if let Some(ev) = handler(...) { yield Ok(ev); }` 仅对返回 `Option<ChatEvent>` 的 handler 使用。
- D3 测试迁移:内联 `mod tests` → `tests_anthropic.rs`(文件级门控);新增 handler 测试(AC7)。
