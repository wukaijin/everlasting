# 拆分 chat_loop.rs — 抽出 run_subagent 到 subagent/dispatch.rs

## Goal

`agent/chat_loop.rs` 当前 2586 行,主循环 `run_chat_loop`(133–1904,~1772 行)与 worker 调度 helper `run_subagent`(2117–2569,~453 行)及其私有辅助 `resolve_project_id`(2575)、`SUBAGENT_MAX_TURNS` const(2114)混在同一文件。把 `run_subagent` + `resolve_project_id` + `SUBAGENT_MAX_TURNS` + 顶部 "B6 Subagent worker dispatch" 注释块(2065–2093)整体抽到新建文件 `agent/subagent/dispatch.rs`,`chat_loop.rs` 只留主循环及其直接辅助(`build_turn_latency` / `instant_delta_ms` / `emit_persist_failure` / `load_for_session` / `is_parallel_eligible`),实现"主循环编排"与"worker 调度"的物理隔离。零行为变更。

## What I already know

* `chat_loop.rs` 实测结构(`grep -nE "^(pub )?(async )?fn|^const|^// ---" chat_loop.rs`):
  - 133:`pub async fn run_chat_loop`(主循环 ~1772 行,到 1904)— **留**
  - 1905:`fn build_turn_latency` — **留**(主循环 latency 辅助)
  - 1920:`fn instant_delta_ms` — **留**
  - 1945:`fn emit_persist_failure` — **留**
  - 1959:`async fn load_for_session` — **留**
  - 2025:`pub(crate) fn is_parallel_eligible` — **留**(tool 并行资格判定)
  - 2065–2093:顶部"B6 Subagent worker dispatch"大段注释 — **搬**(随 run_subagent)
  - 2114:`const SUBAGENT_MAX_TURNS: usize = 200` — **搬**(仅 run_subagent 用)
  - 2117:`async fn run_subagent`(~453 行,到 2569)— **搬(主体)**
  - 2575:`async fn resolve_project_id`(到 2585)— **搬**(仅 run_subagent 调用)
* **`run_subagent` 依赖的兄弟函数(已在 `subagent/` 模块,搬过去后变 `super::`)**:`build_worker_messages`(mod.rs:287)、`assemble_subagent_prompt`(mod.rs:265)、`filter_tools_for_subagent`(mod.rs:363)、`lookup_subagent`(mod.rs:237)、`format_dispatch_result`(truncate_summary.rs:236)、`format_final_text` / `truncate_transcript_for_persistence` / `TRANSCRIPT_MAX_BYTES` / `build_subagent_finished_payload` / `summarize_worker_tool_actions`(subagent 模块内)、`SubagentBufferSink` / `SubagentStatus` / `SubagentDef`(subagent 类型)。
* **跨模块依赖**:`run_chat_loop`(chat_loop.rs:133,已 `pub async fn`)— `run_subagent` 递归调用 `Box::pin(run_chat_loop(...))`(2318)。搬到 dispatch.rs 后改 `crate::agent::chat_loop::run_chat_loop`。Rust 同 crate 内模块循环引用合法(`run_chat_loop`↔`run_subagent` 互相调用),编译期无障碍。
* **调用点**:`run_subagent` 唯一外部调用点是 chat_loop.rs:1627(dispatch_subagent serial-path)。搬走后改 `crate::agent::subagent::dispatch::run_subagent(...)`,`run_subagent` 提为 `pub(crate) async fn`。
* **测试覆盖**:`run_subagent` **无独立单测**。tests.rs(6547 行)里几十处提及 `run_subagent` 全是注释(描述 dispatch_subagent 路径预期,如 4693 行 "we can't — run_subagent clones the parent's Arc<dyn Provider>")。实际覆盖走 `run_chat_loop` + mock provider 间接驱动 dispatch_subagent tool 路径。→ **验收即 `cargo test --lib` 全绿,间接覆盖不变即可,无需新增测试**。
* **外部引用**:全 src/ 下 `run_subagent` 仅 chat_loop.rs:1627 实调用,其余(db/subagent_runs.rs、permissions/mod.rs、subagent/* 的注释)全是文档引用,搬运后需顺带更新这些注释里的路径(`agent::chat_loop::run_subagent` → `agent::subagent::dispatch::run_subagent`)。

## Requirements

### 必含(整体搬到 dispatch.rs)

* `run_subagent`(2117–2569)— 提为 `pub(crate) async fn`
* `resolve_project_id`(2575–2585)— dispatch.rs 内私有
* `const SUBAGENT_MAX_TURNS`(2114)— dispatch.rs 内私有
* 顶部"B6 Subagent worker dispatch"注释块(2065–2093)— 移到 dispatch.rs 文件头并改写"为什么在此模块"的结论(原注释解释"为什么在 chat_loop 而非 subagent",搬走后结论反转)

### 不变

* `run_chat_loop` 主循环 + `build_turn_latency` / `instant_delta_ms` / `emit_persist_failure` / `load_for_session` / `is_parallel_eligible` 留 chat_loop.rs
* `subagent/` 模块其余文件(mod.rs / sink.rs / transcript.rs / truncate_summary.rs)不动
* `run_subagent` 签名(22 参)、返回类型 `(String, bool, bool, Option<i32>)`、内部逻辑、DB wire shape、IPC emit 全部不变
* 测试不动(零新增零删除零修改)

### 顺手清理(用户确认)

* dispatch.rs 内对兄弟函数的 `crate::agent::subagent::xxx` 全路径调用 → `super::xxx`
* 对 `run_chat_loop` 的递归调用 → `crate::agent::chat_loop::run_chat_loop`

## Acceptance Criteria

* [ ] 新文件 `agent/subagent/dispatch.rs` 存在,含 `run_subagent` + `resolve_project_id` + `SUBAGENT_MAX_TURNS` + 文件头注释
* [ ] `agent/subagent/mod.rs` 新增 `pub(crate) mod dispatch;`
* [ ] `run_subagent` 提为 `pub(crate) async fn`,`#[allow(clippy::too_many_arguments)]` 保留
* [ ] `chat_loop.rs:1627` 调用点改 `crate::agent::subagent::dispatch::run_subagent`
* [ ] dispatch.rs 内兄弟函数调用清理为 `super::xxx`、`run_chat_loop` 调用改 `crate::agent::chat_loop::run_chat_loop`
* [ ] `chat_loop.rs` 原 2065–2585 整段删除(含注释、const、两个函数)
* [ ] 顺带更新 db/subagent_runs.rs / permissions/mod.rs / subagent/* 注释里 `agent::chat_loop::run_subagent` 路径引用
* [ ] `PKG_CONFIG_PATH=... cargo check`(app/src-tauri)全绿
* [ ] `PKG_CONFIG_PATH=... cargo test --lib`(app/src-tauri)全绿(dispatch_subagent 间接覆盖不变)
* [ ] `chat_loop.rs` 从 2586 → ~2064 行;`dispatch.rs` ~520 行

## Definition of Done

* worker 调度物理隔离完成,零运行时行为变化
* `cargo check` + `cargo test --lib` 全绿
* `run_subagent` 签名 / 返回 / 逻辑 / DB wire / IPC emit 零改动
* commit message: `refactor(agent): extract run_subagent from chat_loop.rs into subagent/dispatch.rs`

## Technical Approach

* **零行为变更**:纯文件物理搬运 + 可见性 / 调用路径调整 + 注释路径更新
* **跨模块循环依赖**:`agent::chat_loop::run_chat_loop` ↔ `agent::subagent::dispatch::run_subagent` 互相调用,Rust 同 crate 模块循环引用合法,编译期无障碍(已是 `pub async fn` / 提为 `pub(crate)`)
* **递归 Future**:`run_subagent` 内 `Box::pin(run_chat_loop(...))` 保持不变(打破 size-infinite Future chain)
* **可见性最小化**:`run_subagent` 用 `pub(crate)`(chat_loop 同 crate 可见);`resolve_project_id` / `SUBAGENT_MAX_TURNS` dispatch.rs 内私有

## Out of Scope

* 拆 `run_chat_loop` 主循环本身(1772 行,后续任务)
* 动 `subagent/` 现有 4 个文件
* 动任何测试
* 改 `run_subagent` 签名 / 逻辑 / 参数(纯搬运)
* 前端改动

## Decision (ADR-lite)

**Context**:`chat_loop.rs` 2586 行,`run_subagent`(+`resolve_project_id`+`SUBAGENT_MAX_TURNS`)~520 行 worker 调度逻辑混在主循环文件。需决定搬到哪个文件 + 路径引用风格。

**Decision**(2026-06-23 用户确认):
- ✅ **新建 `agent/subagent/dispatch.rs`** — 与 sink.rs / transcript.rs / truncate_summary.rs 并列;mod.rs 继续做纯函数层(lookup / assemble / filter / build_messages + 类型定义),dispatch.rs 专做 ~520 行有状态 worker 调度。职责分层最清晰,mod.rs 不膨胀。
- ✅ **顺手清理 `super::` 短路径** — dispatch.rs 内对兄弟函数 `crate::agent::subagent::xxx` → `super::xxx`;`run_chat_loop` → `crate::agent::chat_loop::run_chat_loop`。
- ✅ **`resolve_project_id` + `SUBAGENT_MAX_TURNS` 随 `run_subagent` 一起搬** — 它们仅服务 `run_subagent`,留在 chat_loop.rs 会成孤儿 + 跨模块可见性污染。

**Consequences**:
- ✅ `chat_loop.rs` 2586 → ~2064 行(主循环 + 主循环辅助),离"单文件 < 1000 行"目标仍有距离(主循环本身 1772 行,留给后续任务)
- ✅ `dispatch.rs` ~520 行,与 subagent/ 现有文件体量(sink 1450 / truncate_summary 910 / transcript 219)同量级
- ⚠️ 跨模块循环引用(`run_chat_loop`↔`run_subagent`)对新人略绕,但已是现状(只是从同文件变跨文件),注释已说明 RULE-A-006 + Box::pin 递归

## Open Questions

*(已全部解决)*

## Technical Notes

* 当前文件:`app/src-tauri/src/agent/chat_loop.rs`(2586 行)
* 目标文件:`app/src-tauri/src/agent/subagent/dispatch.rs`(新建)
* 编译检查:`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check`
* 测试:`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib`
* Trellis 优先级:P0

## Research References

* [`.trellis/spec/backend/agent-loop-architecture.md`](../../spec/backend/agent-loop-architecture.md) — run_subagent 在 agent loop serial-path dispatch 中的定位 + 契约不可变
* [`.trellis/spec/backend/subagent-runs-schema.md`](../../spec/backend/subagent-runs-schema.md) — run_subagent 落 subagent_runs 表 wire 契约
* [`.trellis/spec/backend/tool-contract.md`](../../spec/backend/tool-contract.md) — dispatch_subagent tool_use / tool_result 配对契约
* [`.trellis/spec/backend/permission-layer.md`](../../spec/backend/permission-layer.md) — worker is_worker 路径
* [`.trellis/spec/backend/test-model-contract.md`](../../spec/backend/test-model-contract.md) — 测试组织 + 覆盖不可丢
* [`.trellis/spec/backend/token-usage-tracking.md`](../../spec/backend/token-usage-tracking.md) — sink cumulative_usage 累积契约
* [`.trellis/spec/guides/cross-layer-thinking-guide.md`](../../spec/guides/cross-layer-thinking-guide.md) — 跨模块循环依赖验证清单
