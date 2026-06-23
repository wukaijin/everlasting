# 拆分 agent/tests.rs — 按域拆 5 个测试文件 + tests_common.rs

## Goal

`agent/tests.rs` 6547 行,扁平 `#![cfg(test)]` 结构(无 `mod tests` 包裹,所有测试函数在文件顶层),含 62 个测试 + 9 个 helper + 2 个 use 块。按域拆成 5 个测试文件 + 1 个共享 helper 文件,降低单文件阅读负担,测试可并行编译,git blame 噪音减少。零行为变更(纯物理搬运 + visibility/use 调整)。

## What I already know

* `tests.rs` 结构(`#![cfg(test)]` 文件级 gate,行 7):
  - **use 块 1**(9-25):cancellation/envelope/prompts 域用 — `std::{HashMap,Arc,Duration}` / `tokio::sync::{oneshot,Mutex}` / `tokio_util::sync::CancellationToken` / `crate::agent::helpers::{build_synthetic_tool_result_message, cancel_inflight_for_session, tool_result_envelope}` / `build_system_prompt` / `thinking::{flush_pending_thinking, PendingThinking}` / `db` / `llm::{ContentBlock, MessageContent, Role}` / `projects` / `state::CancellationGuard`
  - **use 块 2**(897-912):agent_loop 域用 — `std::sync::atomic::Ordering` / `std::sync::Mutex as StdMutex` / `futures_util::StreamExt` / `sqlx::SqlitePool` / `tokio::sync::Mutex as AsyncMutex` / `chat_loop::run_chat_loop` / `permissions::new_permission_store` / `llm::provider::mock::{MockProvider, MockResponse}` / `llm::types::{ChatEvent, ChatMessage, TokenUsage}` / `llm::Provider` / `memory::MemoryCache` / `skill::loader::SkillCache` / `state::{ChatEventPayload, ChatEventSink, ToolCallPayload, ToolResultPayload}` / `tools::read_guard::ReadGuard`
  - **MockProvider 来自 `crate::llm::provider::mock`**(生产 mock 模块,非 tests.rs 定义)
* **helper 分布**:
  - `make_session_row`(442) / `make_project_row`(470):仅 prompts 域用(499-693) → prompts.rs 自带
  - `MockEmitter`(925+impl 932/997) / `test_pool`(1012) / `TestHarness`(1044) / `make_harness`(1067) / `test_messages`(1128):连续在 925-1133,被 agent_loop + subagent 共享 → common.rs
  - `load_assistant_rows`(2640):仅 agent_loop error 系列 + checklist 用 → 留 agent_loop.rs
  - `messages_to_text`(3620):仅 agent_loop checklist + notification 用 → 留 agent_loop.rs
* **测试分类(62 个)**(按行号):

| 文件 | 测试数 | 行范围 | 内容 |
|---|---|---|---|
| tests_cancellation.rs | 9 | 31-372 | select_loop / cancellation_token / cancel_chat / two_concurrent / cancellation_guard / cancel_inflight×4 |
| tests_envelope.rs | 4 | 373-440 | tool_result_envelope×2 + flush_pending_thinking×2 |
| tests_prompts.rs | 11 | 498-924 | build_system_prompt×6 + behavior_prompt + assemble + synthetic_tool_result×4 |
| tests_agent_loop.rs | 26 | 1147-4718 | agent_loop_*(basic/tool_use/skill/cancel/max_turns/exhaustion/c3/error×7/checklist×3/parallel×4/notification×2) + mock_provider×2 + is_parallel×2 + load_assistant_rows + messages_to_text |
| tests_subagent.rs | 12 | 4720-6547 | dispatch_subagent×10 + system_prompt_override×2 |

## Requirements

### 必含(6 个新文件)

* **tests_common.rs** — 共享 helper(`#![cfg(test)]` + use 块 2 子集):`pub(crate) MockEmitter` + 2 impl / `pub(crate) test_pool` / `pub(crate) TestHarness` / `pub(crate) make_harness` / `pub(crate) test_messages`
* **tests_cancellation.rs** — 9 测试(`#![cfg(test)]` + use 块 1 子集)
* **tests_envelope.rs** — 4 测试(`#![cfg(test)]` + use 块 1 子集)
* **tests_prompts.rs** — 11 测试 + 私有 `make_session_row`/`make_project_row`(`#![cfg(test)]` + use 块 1 子集)
* **tests_agent_loop.rs** — 26 测试 + 私有 `load_assistant_rows`/`messages_to_text`(`#![cfg(test)]` + `use super::tests_common::*` + use 块 2 子集)
* **tests_subagent.rs** — 12 测试(`#![cfg(test)]` + `use super::tests_common::*` + use 块 2 子集)

### 不变

* 62 个测试的 body 零改动(纯搬运)
* helper 逻辑零改动(仅 visibility:common 的 helper 加 `pub(crate)`)
* 测试函数名 / `#[tokio::test]` / `#[test]` 属性不变
* MockProvider 仍来自 `crate::llm::provider::mock`(不搬)

### 关键改动

* **visibility**:common.rs 的 5 个 helper 从私有 → `pub(crate)`(供 agent_loop/subagent `use super::tests_common::*`)
* **mod.rs**:删 `pub mod tests;`(行 47)→ 加 6 个 `pub mod tests_*;`(跟随原惯例,不加 `#[cfg(test)]`,依赖文件级 `#![cfg(test)]`)
* **use 拆分**:use 块 1 按域分到 cancellation/envelope/prompts;use 块 2 按 helper(catalog)vs 测试(agent_loop/subagent)分

## Acceptance Criteria

* [ ] 6 个新文件存在,各自 `#![cfg(test)]` 文件头
* [ ] tests.rs 删除
* [ ] mod.rs 声明 6 个 `pub mod tests_*;`(无 `pub mod tests;`)
* [ ] common.rs 的 5 个 helper 为 `pub(crate)`
* [ ] agent_loop.rs / subagent.rs 顶部 `use super::tests_common::*`(或显式列举)
* [ ] 62 个测试全部保留(cargo test --lib 测试数不变)
* [ ] `PKG_CONFIG_PATH=... cargo test --lib` 全绿(813 passed,0 failed — 与拆分前一致)
* [ ] 0 warning(无 unused import)
* [ ] tests_agent_loop.rs ~3500 行 / tests_subagent.rs ~1800 行 / 其余各 < 1000 行

## Definition of Done

* 测试按域物理隔离,零行为变化
* `cargo test --lib` 813 passed 0 failed 0 warning
* commit message: `refactor(agent): split tests.rs into 5 domain files + tests_common.rs`

## Technical Approach

* **零行为变更**:纯文件物理搬运 + visibility(`pub(crate)`) + use 路径调整
* **helper 共享**:common.rs 的 helper `pub(crate)`,agent_loop/subagent `use super::tests_common::*` glob 引入(Rust 同 crate cfg(test) 模块间合法)
* **use 精确分配**:每个文件只 use 自己用到的(依赖 cargo test 的 unused-import / cannot-find 报错精确收敛)
* **提取策略**:用 `sed -n 'start,end p'` 按 5 个域的连续行范围提取 body,各文件 prepend `#![cfg(test)]` + use 块

## Out of Scope

* 改任何测试 body / 断言 / 逻辑
* 拆 chat_loop.rs 主循环(那是另一任务)
* 动 MockProvider(`crate::llm::provider::mock` 生产模块)
* 改 helper 逻辑(仅 visibility)
* 重新设计 TestHarness / MockEmitter

## Decision (ADR-lite)

**Context**:`tests.rs` 6547 行,task 原描述估"4 文件 34 测试",实测 62 测试且 agent_loop 域 36 个(含 dispatch_subagent 10)占 ~5600 行。需决定拆分粒度 + helper 共享。

**Decision**(2026-06-23 用户确认):
- ✅ **5 文件 + tests_common.rs** — dispatch_subagent(10)+ system_prompt_override(2)独立成 tests_subagent.rs;agent_loop.rs 降到 ~3500 行(26 测试)。体量最平衡。
- ✅ **tests_common.rs 平级共享** — MockEmitter/test_pool/TestHarness/make_harness/test_messages 提为 `pub(crate)`,agent_loop+subagent `use super::tests_common::*`。
- ✅ **load_assistant_rows / messages_to_text 留 agent_loop.rs** — 仅 agent_loop 域用,不进 common。
- ✅ **make_session_row / make_project_row 留 prompts.rs** — 仅 prompts 域用,自带。

**Consequences**:
- ✅ tests.rs 6547 → 6 文件,最大 tests_agent_loop.rs ~3500 行(仍偏大但 agent_loop 是连贯集成测试域,进一步拆需更多文件)
- ✅ helper 零重复(common 共享)
- ⚠️ common.rs 的 helper 加 `pub(crate)` 略放宽封装,但 cfg(test) 限定 + 同 crate 可接受

## Open Questions

*(已全部解决)*

## Technical Notes

* 当前文件:`app/src-tauri/src/agent/tests.rs`(6547 行,`#![cfg(test)]` 扁平)
* MockProvider 来源:`crate::llm::provider::mock`(非 tests.rs)
* 编译/测试:`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib`
* 提取命令:`sed -n '31,372p' tests.rs` 等(5 个连续域范围)
* Trellis 优先级:P1

## Research References

* [`.trellis/spec/backend/test-model-contract.md`](../../spec/backend/test-model-contract.md) — `#[cfg(test)] mod tests` 组织 + 命名约定(拆分后各文件合规)
* [`.trellis/spec/backend/agent-loop-architecture.md`](../../spec/backend/agent-loop-architecture.md) — agent_loop_* 集成测试覆盖契约(62 测试不可丢)
* [`.trellis/spec/guides/cross-layer-thinking-guide.md`](../../spec/guides/cross-layer-thinking-guide.md) — 跨文件 helper 共享验证清单
