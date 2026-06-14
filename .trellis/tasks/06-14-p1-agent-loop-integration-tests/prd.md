# P1 — Agent Loop 集成测试 (MockProvider)

## Goal

为 Agent Loop 提供 mock HTTP server 驱动的完整 turn 集成测试,覆盖 turn 边界关键路径(cancel / max_turns / C3 触发 / orphan 配对 / persist 失败 / audit 时序),填补 backend Agent Loop 无集成测试的缺口(469 个单测都是单点函数测)。

修复 RULE-A-006(原 P2,meta-review 升级 P1,前移到 PR5)— P0 修复(PR1-PR3 + 即将实施的 PR4 C3)无回归保护等于盲修。

## Decisions (ADR-lite)

**Context**:
- Provider trait 已 object-safe(`send` 返回 `Pin<Box<dyn Stream<...>>>`),实现新 Provider 只需 trait impl
- 469 个 backend 单测都是单点(LLM 协议解析、permission logic、C3 算法等),**无**端到端 turn 循环测试
- agent loop turn 边界风险最高(连续 50 轮 + SSE 流 + cancel 跨 turn + tool 调度 + DB persist + audit),只能手动验证
- 现有 105 个 LLM 子系统测试是 AnthropicProvider/OpenAIProvider 的协议细节测,非 Agent Loop 测

**Decisions**:

1. **MockProvider 位置**: `app/src-tauri/src/llm/provider/mock.rs`,新文件,在 `provider/mod.rs` 加 `pub mod mock;` + `#[cfg(test)]` 守卫
2. **不污染生产**: MockProvider 全部代码 `#[cfg(test)]` 包裹,**生产 binary 不包含 MockProvider 类型**
3. **chat 命令 provider 注入点**: `commands/sessions.rs::chat` 当前用 `lookup_provider_for_session` 拿 `Arc<dyn Provider>`,**集成测试需要绕过 DB lookup 直接构造 `Arc<MockProvider>`**。两条路径:
   - A. 抽 `chat_inner(provider, ...)` 函数,production 仍走 `chat` 命令,test 直接调 `chat_inner(MockProvider::new())`
   - B. 在 `chat` 函数里加 `#[cfg(test)]` 分支接受预构造 provider
   - 选 A:更干净,production 路径不变
4. **MockProvider 行为模型**: 不做 HTTP server,而是**scripted stream** — 测试预设一个 `Vec<ChatEvent>`(可能跨多 turn),MockProvider 在每次 `send` 调用时返回下一段 events。可精确模拟:
   - 单 turn text-only
   - 多 turn tool_use → tool_result → text
   - 触发 C3 的超长 messages
   - 中途 cancel
   - 触发 MAX_TURNS
   - 触发 persist 失败(需要 fault injection)
5. **cancel 测试**: 通过 `tokio::sync::oneshot` 或 `tokio::time::sleep` 让 turn 2 触发 cancel token,断言后续 turn 不再调 MockProvider.send
6. **DB 真实路径**: 集成测试用内存 SQLite(`sqlx::SqlitePool::connect("sqlite::memory:")`)或 tempfile,跑真实 `db::persist_turn`,验证 DB rows
7. **不重 mock SSE 解析**: 协议层已有 105 个测试,本 PR 专注 Agent Loop turn 编排,不重测 SSE
8. **不引入新测试依赖**: 仅用现有 `tokio` + `tempfile` + `tokio_util::CancellationToken`,不引 `wiremock` / `mockito` / `httpmock`(httpmock 已被 web_fetch 用,Agent Loop 测试不需要)

**Consequences**:
- Agent Loop 行为变更(turn 边界 / cancel / C3 / persist / audit)都有回归保护
- PR4 (C3 tail pair orphan + 超窗降级)可以依赖本 PR 写完整 turn 测试
- 测试运行时间可控(MockProvider 无 I/O)
- 469 → ~480 tests,+~10 个集成测试
- MockProvider 不在 production binary(零运行时成本)

---

## Requirements

### R1 — MockProvider 实现

* 新建 `app/src-tauri/src/llm/provider/mock.rs`:
  ```rust
  #[cfg(test)]
  pub struct MockProvider {
      script: Vec<MockResponse>,  // 每个 MockResponse 对应一次 send 调用
      capabilities: ProviderCapabilities,
  }

  #[cfg(test)]
  pub enum MockResponse {
      /// 完整 ChatEvent stream(支持 text delta + tool_use + error)
      Events(Vec<ChatEvent>),
      /// 直接返回 LlmError
      Err(LlmError),
      /// 模拟 cancel 触发:stream 永远不结束,等 cancel
      HangingThenCancel,
  }

  #[cfg(test)]
  impl MockProvider {
      pub fn new(script: Vec<MockResponse>) -> Self { ... }
      pub fn with_capabilities(script, caps) -> Self { ... }
  }

  #[cfg(test)]
  impl Provider for MockProvider { ... }
  ```
* `send` 调用时根据调用次数取对应 `MockResponse`,返回对应 Stream
* `protocol()` 返回一个测试专用的 `ProviderProtocol::Mock` variant,**需在 `db::ProviderProtocol` enum 加 Mock variant**(`#[cfg(test)]` 守卫)

### R2 — chat 命令 provider 注入

* 抽 `chat_inner(provider: Arc<dyn Provider>, session: ..., messages: ..., request: ...)` 函数,放 `commands/sessions.rs::chat` 的现有逻辑
* production `chat` 命令仍调 `lookup_provider_for_session` → `chat_inner(Arc::new(provider))`
* 测试直接 `chat_inner(Arc::new(MockProvider::new(script)), ...)`

### R3 — 集成测试套件

新建 `app/src-tauri/src/agent/tests.rs`(或 `commands/sessions/tests.rs`),包含:

| 测试 | 覆盖 finding | 场景 |
|---|---|---|
| `agent_loop_basic_text_only_completes` | 基础路径 | 1 turn text-only,断言 emit Done(text) |
| `agent_loop_tool_use_triggers_tool_result_turn` | 基础路径 | 1 turn tool_use → tool_result → 2nd turn text |
| `agent_loop_cancel_in_turn_2_kills_loop` | RULE-A-001 + CancelGuard | 2 turn cancel,断言不再 send,emit Done("cancelled") |
| `agent_loop_max_turns_emits_done_marker` | RULE-A-001 + max_turns | 50 轮都 tool_use 不收敛,断言 emit Done("max_turns") |
| `agent_loop_c3_compaction_preserves_pairs` | RULE-A-001(C3 tail pair) | 大 messages 触发 C3,断言 tool_use/tool_result 仍配对 |
| `agent_loop_c3_still_over_emits_error` | RULE-A-002(超窗降级) | 单条 tool_result > target,断言 emit Error(非 send) |
| `agent_loop_persist_turn_failure_emits_error` | RULE-A-003 | 注入 DB 故障,断言 emit Error(不静默) |
| `agent_loop_cancel_audit_not_recorded` | RULE-A-004(audit 时序) | cancel 短路 tool,断言 audit 表无 tool_executed 行 |
| `agent_loop_uses_correct_provider_for_session` | catalog lookup | session → provider 解析,MockProvider.script[0] 被消费 |

### R4 — 测试基础设施

* `app/src-tauri/src/commands/sessions.rs::test_helpers` 模块(#[cfg(test)]):
  - `test_db_pool()` 返回内存 SQLite + migrations applied
  - `test_session_row()` 返回固定 session
  - `test_messages()` 返回基础 ChatMessage vec
* 不引入 `wiremock` / `mockito` 等 HTTP mock 库
* 测试运行时间 < 5s/个(无 I/O)

### R5 — 文档

* `app/src-tauri/src/llm/provider/mock.rs` module docstring 说明设计动机
* `.trellis/spec/backend/agent-loop/index.md`(若已建)加 "MockProvider + integration tests" 段
* `.trellis/reviews/DEBT.md` RULE-A-006 状态变更

---

## Acceptance Criteria

* [ ] `app/src-tauri/src/llm/provider/mock.rs` 存在,全 `#[cfg(test)]` 守卫
* [ ] MockProvider 实现 Provider trait(scripted stream 模式)
* [ ] `chat_inner(provider, ...)` 函数抽出,production 路径不变
* [ ] 至少 9 个集成测试覆盖 R3 表
* [ ] `cargo test --lib agent::tests` 全 pass
* [ ] `cargo test --lib` 全套 ~480 tests pass
* [ ] MockProvider 在 production binary 中不可见(`#[cfg(test)]` 守卫严格)
* [ ] 测试运行时间总和 < 30s
* [ ] `cargo check` 0 warning

---

## Definition of Done

* 上述 Acceptance Criteria 全 ✅
* PR merge 后更新 `docs/_reviews/DEBT.md` RULE-A-006:`Status: closed`
* ARCHITECTURE.md §2.5 加"Agent Loop 集成测试"段,引用本 PR

---

## Out of Scope

* MockProvider 跨 platform 行为(仅 Unix 跑测试即可,production 不含)
* MockProvider 的 mock SSE 协议(协议层 105 个测试已覆盖)
* WireMessage 中间层(已有 wire.rs 测试)
* Frontend 集成测试(后续)
* MockProvider 持久化 / 录制回放(MVP 不需要)

---

## Technical Approach

### Step 1: MockProvider 骨架

```rust
//! MockProvider — scripted stream Provider for Agent Loop tests.
//!
//! `#[cfg(test)]` only. Production code MUST NOT depend on this
//! module. The factory `build_provider` does not know about it.

use std::pin::Pin;
use std::sync::Mutex;

use async_stream::stream;
use futures_util::Stream;

use super::{Provider, ProviderCapabilities};
use crate::db::ProviderProtocol;
use crate::llm::error::LlmError;
use crate::llm::types::{ChatEvent, ChatMessage, ToolDef};

#[cfg(test)]
pub struct MockProvider {
    script: Mutex<Vec<MockResponse>>,
    capabilities: ProviderCapabilities,
}

#[cfg(test)]
pub enum MockResponse {
    Events(Vec<Result<ChatEvent, LlmError>>),
    HangingThenCancel,  // stream 永远不结束,直到 cancel
    Err(LlmError),
}

#[cfg(test)]
impl MockProvider {
    pub fn new(script: Vec<MockResponse>) -> Self {
        Self {
            script: Mutex::new(script),
            capabilities: ProviderCapabilities {
                supports_system_prompt: true,
                supports_tools: true,
                supports_streaming: true,
            },
        }
    }

    /// Inspect how many `send` calls have been made so far.
    pub fn calls_so_far(&self) -> usize {
        // ... track index separately
    }
}

#[cfg(test)]
impl Provider for MockProvider {
    fn send(
        &self,
        _system: Option<String>,
        _messages: Vec<ChatMessage>,
        _tools: Vec<ToolDef>,
    ) -> Pin<Box<dyn Stream<Item = Result<ChatEvent, LlmError>> + Send + 'static>> {
        let mut script = self.script.lock().unwrap();
        let response = if script.is_empty() {
            MockResponse::Err(LlmError::Server("MockProvider script exhausted".into()))
        } else {
            script.remove(0)
        };
        drop(script);
        match response {
            MockResponse::Events(events) => Box::pin(stream! {
                for e in events {
                    yield e;
                }
            }),
            MockResponse::HangingThenCancel => Box::pin(stream! {
                // 永远 pending — 测试通过 cancel token abort
                futures_util::future::pending::<()>().await;
                unreachable!()
            }),
            MockResponse::Err(e) => Box::pin(stream! {
                yield Err(e);
            }),
        }
    }

    fn capabilities(&self) -> ProviderCapabilities { self.capabilities }
    fn protocol(&self) -> ProviderProtocol { ProviderProtocol::Mock }
}
```

需要:
- `async-stream` crate 加到 dev-dependencies(若已有跳过)
- `ProviderProtocol` enum 加 `Mock` variant(`#[cfg(test)]`)

### Step 2: chat_inner 抽函数

`commands/sessions.rs::chat` 当前签名:
```rust
#[tauri::command]
pub async fn chat(
    state: State<'_, AppState>,
    request: ChatRequest,
) -> Result<...>
```

抽 `chat_inner`:
```rust
#[tauri::command]
pub async fn chat(
    state: State<'_, AppState>,
    request: ChatRequest,
) -> Result<...> {
    let provider = lookup_provider_for_session(...).await?;
    chat_inner(state, request, provider).await
}

pub async fn chat_inner(
    state: State<'_, AppState>,
    request: ChatRequest,
    provider: Arc<dyn Provider>,
) -> Result<...> {
    // 现有 chat 命令逻辑
}
```

### Step 3: 集成测试

每条测试模式:
```rust
#[tokio::test]
async fn agent_loop_basic_text_only_completes() {
    let state = test_app_state();
    let mock = Arc::new(MockProvider::new(vec![
        MockResponse::Events(vec![
            Ok(ChatEvent::MessageStart),
            Ok(ChatEvent::ContentBlockStart { ... }),
            Ok(ChatEvent::TextDelta { delta: "hi".into() }),
            Ok(ChatEvent::ContentBlockStop),
            Ok(ChatEvent::MessageStop { stop_reason: Some("end_turn".into()) }),
            Ok(ChatEvent::Done { reason: "natural".into() }),
        ]),
    ]));
    let result = chat_inner(state, request, mock).await;
    assert!(result.is_ok());
}
```

---

## Technical Notes

### 关键文件

- `app/src-tauri/src/llm/provider/mod.rs` — 加 `#[cfg(test)] pub mod mock;`
- `app/src-tauri/src/llm/provider/mock.rs` — 新建
- `app/src-tauri/src/db/types.rs` — `ProviderProtocol` 加 `Mock` variant(`#[cfg(test)]`)
- `app/src-tauri/src/commands/sessions.rs` — 抽 `chat_inner`
- `app/src-tauri/src/agent/tests.rs` 或 `commands/sessions/tests.rs` — 新建集成测试
- `app/src-tauri/Cargo.toml` — dev-deps `async-stream` 若缺

### ProviderProtocol 加 Mock variant 影响

- `ProviderProtocol` 是 serde Serialize/Deserialize enum,加 variant 影响 wire format
- 用 `#[cfg(test)]` 守卫:production binary 不包含
- 测试用 `ProviderProtocol::Mock` 不会泄漏到 production

### 与现有 469 tests 的关系

- 现有 LLM 测试是 `cargo test --lib llm` 共 105 个,集中在协议层
- 本 PR 加 ~10 个 `cargo test --lib agent::tests`,集中在 turn 编排层
- 现有 boundary/read_guard/edit tests 6-16 个,本 PR 不动

### 与 PR4 (C3 tail pair) 的协作

PR4 修复 context.rs 后,本 PR 的 `agent_loop_c3_compaction_preserves_pairs` 测试可以作为 PR4 的回归保护。但 PR4 应**先**实施,本 PR 提供**后续**回归保护。

按 meta-review 顺序:本 PR5 应在 PR4 之前或紧接 PR4 之后实施。本 task 按"先 PR5"决策推进。

### 测试运行时间

每个 ~50ms(无 I/O),10 个 ~500ms。CI 总开销可忽略。

---

## Research References

* `.trellis/reviews/DEBT.md` §RULE-A-006
* `docs/_reviews/REVIEW-agent-loop-full-audit-2026-06-14.md` §3.5 + §1.1(全局架构图 agent loop 主体)
* `app/src-tauri/src/llm/provider/mod.rs` — Provider trait
* `app/src-tauri/src/commands/sessions.rs` — chat 命令入口
* REVIEW-sse-agent-loop §8 P5(mock HTTP server 集成测试,2026-06-12 提出,0 落地)— 本 PR 兑现