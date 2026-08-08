# A类单体重构:subagent sink 拆分

## Goal

把 `app/src-tauri/src/agent/subagent/sink.rs`(1679 行)按职责拆分:`SubagentBufferSink`(14 字段 struct)+ `impl ChatEventSink`(5 个 emit 方法 ~330 行)与查询/构造侧分离,内联 856 行测试迁出。行为零变化;为 `chat_loop.rs`(5132 行)拆分前的最后一个 A 类专项,复用 `pattern-large-function-split` 方法论。

## Background / 已确认事实

- `sink.rs` 结构(1679 行,非单函数单体——是大 struct + 大 trait impl + 大测试块):
  - `thread_local! TEST_COLLECTOR`(L38–41)— 测试收集器,声明在模块级(非 cfg(test),record 生产路径会引用)
  - `pub struct SubagentBufferSink`(L64–195)— **14 字段**:`transcript` / `text_parts` / `per_turn_usage` / `tool_call_received_at` / `worker_messages`(5 × StdMutex)+ `had_error` / `was_cancelled` / `was_incomplete` / `was_loop_terminated` / `turns_completed`(5 × atomic)+ `event_sink`(Arc<dyn SubagentEventSink>)+ `app_handle`(Option,dead_code 测试构造器用)+ `run_id` / `session_id`(String)
  - `impl SubagentBufferSink`(L197–455):3 构造器(`new_without_app_handle` / `new_with_event_sink` / `new_with_collector`(cfg(test)))+ `record`(L289,内部)+ 查询方法(`final_text` / `worker_messages` / `had_error` / `was_cancelled` / `was_incomplete` / `was_loop_terminated` / `turns_completed` / `emit_subagent_finished` / `transcript_snapshot` / `drain_per_turn_usage` / `cumulative_usage`)
  - `impl ChatEventSink for SubagentBufferSink`(L456–785):`emit_chat_event`(109 行)/ `emit_tool_call`(63)/ `emit_tool_result`(63)/ `emit_permission_ask`(50)/ `emit_permission_ask_resolved`(4)/ `record_worker_messages`(6)
  - `impl SubagentBufferSink` 第二块(L787–821):`record_permission_ask_resolved`(pub(crate))
  - **内联 `mod tests`(L823–1679,856 行)**
- **锁序推理(已核实,拆分不改变)**:所有 mutex 均为**方法内单锁或顺序锁,无嵌套持有**——`emit_chat_event` 顺序获取 text_parts → per_turn_usage(各自锁内操作后立即 drop);`emit_tool_call`/`emit_tool_result` 单锁 tool_call_received_at;`record` 单锁 transcript;查询方法各自单锁。无跨方法嵌套 → 无死锁路径。拆分 = 方法整体平移,锁序与并发语义不变。
- 外部引用:`SubagentBufferSink` 经 `mod.rs:122 pub use sink::SubagentBufferSink;` re-export,state.rs/db/chat_loop 等均为符号引用,拆分后路径不变;`arm_test_collector` / `clear_test_collector` 经 `mod.rs:105 pub(crate) use event_sink::{...}`(cfg(test)),测试以 `crate::agent::subagent::clear_test_collector` 全路径引用。
- 测试引用:`use super::*` + `use crate::state::ChatEventSink` + `crate::agent::permissions::*` 全路径 + `impl crate::state::ChatEventSink for NoopSink`(测试内自定义 impl)。
- 基线:`cargo test --lib` = 1662 全绿;`clippy --lib --tests` + `fmt --check` 零警告。

## Requirements

- R1 按职责拆分:`sink.rs` 保留为 hub(struct + 构造器 + record + 查询方法),`impl ChatEventSink` 移入 `sink/events.rs` 子模块;`sink.rs` hub + `sink/` 子目录共存,全量 re-export(对照 dispatch/anthropic 惯例)。
- R2 被测私有项升可见性:struct 字段升 `pub(crate)`(子模块方法访问),`record` 等内部方法升 `pub(crate)`;禁止为可测性改公开 API(`pub` 项保持)。
- R3 行为零变化:锁序、emit 顺序(event_sink 先于 record)、tracing 日志、transcript 写入均不变。
- R4 测试:内联 `mod tests` 迁出为 `tests_sink.rs`(文件级 `#![cfg(test)]`,`use super::sink::*` 经 hub 解析);测试数量不变(1662 基线)。
- R5 文档引用 sweep:`sink.rs:LINE` 行号引用改符号引用;archive/历史快照不改。
- R6 收尾 `cargo fmt` + `clippy --lib --tests` + `fmt --check` 零警告;`cargo test --lib` 全绿(1662 基线)。

## Acceptance Criteria

- [ ] AC1 `sink.rs` hub ≤ ~500 行:thread_local + struct + 3 构造器 + record + 查询方法 + re-export;`sink/events.rs` 含 `impl ChatEventSink` 全部 6 方法。
- [ ] AC2 `crate::agent::subagent::SubagentBufferSink` 等既有路径解析不变;`pub` 方法/构造器可见性不变。
- [ ] AC3 每个 commit 单独可回滚(可见性准备、拆分、测试迁出、文档 sweep 各独立 commit)。
- [ ] AC4 `cargo test --lib` 全绿(1662 基线无减少);`cargo fmt --check` + `clippy --lib --tests` 零警告。
- [ ] AC5 无公开 API 变更;新增可见性均为 `pub(crate)`。
- [ ] AC6 非 archive 文档/注释无残留 `sink.rs:LINE` 行号引用。
- [ ] AC7 锁序验证:`emit_chat_event` 的 text_parts → per_turn_usage 顺序获取、`event_sink` 先于 `record` 的 emit 顺序,拆分前后一致(代码平移核对 + 现有并发测试锁定)。

## Out of Scope

- 不改 struct 字段语义 / 构造器签名 / trait 方法签名。
- 不合并字段子结构(不引入 SinkState 等新 struct——方法整体平移风险最低)。
- 不做 `chat_loop.rs`(最后专项)。
- 不新增 feature / 不修 bug / 不改行为。

## 已决决策(沿用 batch1/2,2026-08-08)

- D1 执行节奏:可见性准备(独立 commit)→ 拆分(events.rs + hub re-export + 测试迁出,单 commit)→ 文档 sweep → 终验。
- D2 拆分形态:`impl ChatEventSink` 整体移入 `sink/events.rs`(方法整体平移,锁序/emit 顺序零变化);查询方法留 hub。
- D3 测试迁出:`mod tests` → `tests_sink.rs`(subagent/ 下,`use super::sink::*`;声明加在 `subagent/mod.rs`)。
