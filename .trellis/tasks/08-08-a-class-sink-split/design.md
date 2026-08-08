# A类单体重构:subagent sink 拆分 — Design

## 1. 目标形态

`SubagentBufferSink`(14 字段 struct + 3 impl 块 + 856 行内联测试)→ hub + 1 子模块 + 测试文件。

```
sink.rs(hub)              # thread_local! TEST_COLLECTOR + struct(14 字段,pub(crate))+ 3 构造器 + record + 11 个查询方法 + re-export
sink/events.rs            # impl ChatEventSink for SubagentBufferSink(6 方法:5 emit + record_worker_messages)
tests_sink.rs             # 原内联 mod tests(856 行,文件级 #![cfg(test)] 门控)
```

## 2. 拆分契约(方法整体平移,零行为变化)

| 归属 | 内容 | 可见性调整 |
|---|---|---|
| hub | `thread_local! TEST_COLLECTOR`、`SubagentBufferSink` struct、3 构造器、`record`、11 个查询方法、`record_permission_ask_resolved` | struct 字段 `pub(crate)`(events.rs 子模块访问);`record` 升 `pub(crate)`;其余保持 `pub` |
| events.rs | `impl ChatEventSink` 的 `emit_chat_event` / `emit_tool_call` / `emit_tool_result` / `emit_permission_ask` / `emit_permission_ask_resolved` / `record_worker_messages` | 方法整体平移(签名不变,`&self` 不变) |
| tests_sink.rs | 原 `mod tests` 全部 30 个测试 | `use super::*` → `use super::sink::*`(文件级)+ 文件内 `use super::*` 保持 |

**锁序不变论证**(评审关注点):所有 mutex 均为方法内单锁或顺序锁、无嵌套持有。方法整体平移后:
- `emit_chat_event`:text_parts → per_turn_usage 顺序获取(各自锁内操作后立即 drop,不嵌套)
- `emit_tool_call` / `emit_tool_result`:单锁 tool_call_received_at
- `record`:event_sink 无锁(Arc)+ transcript 单锁
- 查询方法:各自单锁
无跨方法嵌套锁 → 无死锁路径;拆分不引入新锁、不改变获取顺序。

**emit 顺序不变**:`emit_permission_ask` 内 `event_sink.emit_permission_ask` 先于 `record`(PR1.5 emit-first 约定);`record` 内 `event_sink.emit_subagent_event` 先于 transcript push。代码逐行平移,顺序不变。

## 3. 可见性矩阵(AC5:新增均为 pub(crate),pub 不变)

| 项 | 现状 | 拆分后 |
|---|---|---|
| `SubagentBufferSink` struct | `pub` | `pub`(不变;mod.rs re-export 依赖) |
| 14 个字段 | 私有 | `pub(crate)`(events.rs 方法访问;tests 兄弟模块可访问) |
| `new_without_app_handle` / `new_with_event_sink` | `pub` | `pub`(不变;dispatch.rs 调用) |
| `new_with_collector` | `#[cfg(test)] pub` | 不变 |
| `record` | 私有 | `pub(crate)`(events.rs 调用) |
| 11 个查询方法 | `pub` | `pub`(不变;dispatch.rs 调用) |
| `record_permission_ask_resolved` | `pub(crate)` | 不变 |
| 5 个 trait 方法 | trait 公开 | 不变(整体平移) |

## 4. 测试组织

- 内联 `mod tests`(L823–1679)整体迁出为 `tests_sink.rs`(`#![cfg(test)]` 文件级门控),声明加在 `subagent/mod.rs`(`mod tests_sink;`,文件位置决定声明位置——anthropic 专项 gotcha)。
- 文件级 `use super::*` → `use super::sink::*`(hub re-export 全量 pub(crate));`mod tests` 内 `use super::*` 保持(指向 tests_sink 文件级作用域)。
- `crate::agent::subagent::clear_test_collector` 等全路径引用不变(mod.rs re-export 保持)。
- 测试内 `impl crate::state::ChatEventSink for NoopSink` 自定义 impl 原样保留。

## 5. 风险与回滚

| 风险 | 缓解 |
|---|---|
| 字段可见性调整引入 pub(crate) 泄漏 | 仅字段/record 升 pub(crate)(AC5);`pub` API 面零变化 |
| 锁序/并发语义漂移 | 方法整体平移,逐行核对;现有并发/多线程测试(buffer_sink_* 系列 + ipc emit 测试)锁定 |
| 测试迁移引用断 | `use super::sink::*` + mod.rs 声明;`clear_test_collector` 全路径不变;迁出后 1662 全绿即验证 |
| hub re-export 与 mod.rs `pub use sink::SubagentBufferSink` 冲突 | hub 保留 `SubagentBufferSink` 定义(`pub struct`),mod.rs 路径不变 |
| thread_local 与 events.rs 解耦 | TEST_COLLECTOR 留 hub(record 生产路径引用);events.rs 不触碰 |

**回滚**:可见性准备与拆分各自独立 commit;拆分 commit 前 `cargo check` + 全量测试验证。

## 6. 明确不做

- 不合并字段子结构(SinkState 等)——方法整体平移是零行为变化的最低风险路径。
- 不重排锁获取顺序 / 不优化锁粒度。
- 不删 `app_handle` dead_code 字段(new_with_collector 测试构造器依赖)。
- 不新增测试(现有 30 个测试已覆盖;行为零变化由基线锁定)。
