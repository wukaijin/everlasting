# LLM API Contract —核心类型与思考契约

> **基线**:2026-06-10 commit `0f9a167` (8-PR5拆分后) + 2026-06-21 (doc-trim 拆 3 个 scenario) + 2026-08-10 (Extended Thinking 拆出子文件)
> **来源**:从原 `llm-contract.md` (3149 行)拆出本文件 +4 个子文件 (`tool-contract.md` / `worktree-contract.md` / `multi-provider-contract.md` / `test-model-contract.md`)
> **同源文档**:
> - [llm-contract.md](./llm-contract.md) (本文) —核心类型 + 反模式汇总(Overview / Decisions / Gotcha / 3 个中小 Scenario)
> - [llm-contract/extended-thinking.md](./llm-contract/extended-thinking.md) — Extended Thinking (Step6) 完整契约(2026-08-10 拆出)
> - [tool-contract.md](./tool-contract.md) —工具定义 + ReadGuard + shell spillover
> - [worktree-contract.md](./worktree-contract.md) — attach/detach/delete + cancel + system prompt
> - [multi-provider-contract.md](./multi-provider-contract.md) — Provider trait + catalog + Anthropic/OpenAI 分发
> - [test-model-contract.md](./test-model-contract.md) — `test_model` IPC
>
> **详细 scenario**(2026-06-21 doc-trim 拆出):
> - [latency-tracking.md](./latency-tracking.md) — F5(2026-06-11)+ Per-Turn Tracking follow-up
> - [token-usage-tracking.md](./token-usage-tracking.md) — A4(2026-06-10)
> - [permission-layer.md](./permission-layer.md) — A2 + B7(2026-06-13)⑨ 关 5-tier
>
> **何时读本文**:涉及 `ContentBlock` / `ChatMessage` / `ChatEvent` 核心类型 / tool_use 原子性反模式 / DeepSeek fix / Retry / E2 trace 时。Extended Thinking 持久化契约见 [extended-thinking.md](./llm-contract/extended-thinking.md)。

---

# LLM API Contract

> Anthropic Messages API contract enforced by the Rust agent core, with extended-thinking support.

---

## Overview

The LLM client (`app/src-tauri/src/llm/`) speaks the **Anthropic Messages API** schema
(`/v1/messages`, streaming via SSE) directly — not OpenAI, not a generic OpenAI-compat layer.
The `ChatRequest` / `ContentBlock` / `ChatEvent` types are aligned to the official schema
(serde tag `type` matches the wire string).

Two operator choices that look like "compat layer" are actually configuration knobs:

- **`ANTHROPIC_BASE_URL`** — a proxy or a self-hosted relay that follows the Anthropic
 schema. The current dev setup uses `<your-anthropic-compat-host>`'s Claude-compat endpoint; the
 payload is still Anthropic-shaped, not OpenAI-shaped.
- If at any point we switch to OpenAI-compat, the `reasoning_content` field replaces
 the `thinking` block entirely; that change would happen here, not in the UI.

For compatibility-layer caveats and what to test when the proxy changes, see
`docs/HACKING-llm.md`.

---

## Scenario: Extended Thinking Support (Step6)

> **已拆出**(2026-08-10 doc-split):完整 Extended Thinking 契约(7 段:Scope / Signatures /
> Contracts / Validation / Cases / Tests / Wrong-vs-Correct)见
> [llm-contract/extended-thinking.md](./llm-contract/extended-thinking.md)。

**何时读该子文件**:涉及 `ContentBlock::Thinking` / `ContentBlock::RedactedThinking` /
`ThinkingConfig::Adaptive` / `signature` round-trip / SSE `thinking_delta`·`signature_delta` /
`apply_deepseek_reasoning_fix` 时。

---

## Decision: Always send `thinking`, no per-session / per-request toggle

**Context**: MVP UX. Adding a toggle would expand the settings surface and the
DB schema.

**Decision**: `thinking` is always in the request body. The only knob is
`LLM_THINKING_EFFORT` env, applied globally.

**Consequences**: Simple. If the upstream model does not support adaptive
thinking the call400s — accepted as an operational risk.

## Decision: `display: "summarized"` is explicit, never omitted

**Context**: Opus4.7+ defaults to `display: "omitted"`, which suppresses
`thinking_delta` SSE events and breaks the UI's streaming label.

**Decision**: `ThinkingConfig::Adaptive { display: "summarized", effort }` is
hard-coded in `LlmConfig::thinking_config()`.

**Consequences**: Streamed thinking is always visible. (Trade ~1-2 ms per
response for guaranteed streaming.)

## Decision: `max_tokens` default1024 →16384

**Context**: Thinking tokens count against the same budget as the actual answer.
1024 was too low — non-trivial turns would hit `stop_reason: "max_tokens"`.

**Decision**: `DEFAULT_MAX_TOKENS =16384`.

**Consequences**: Cheap requests waste ~8k of budget, but no truncation on
real workloads. Per-model override available via `models.max_tokens`.

## Gotcha: tool_use ↔ tool_result Pair Atomicity (C3, 2026-06-12)

**Rule**: Any code path that truncates / compacts / splits the `messages` array
(e.g. C3 `compact_messages`) MUST treat an `assistant(tool_use)` + the immediately
following `user(tool_result)` as **one atomic unit**. Either both stay in history
or both are dropped. Never split them.

**Why**: Anthropic returns `400 invalid_request_error` on the next turn if
history has an `assistant(tool_use)` block whose `tool_use_id` has no matching
`tool_result` (orphan request) or a `user(tool_result)` whose `tool_use_id`
has no matching `tool_use` (orphan result). The error does NOT name the
problem — the agent loop sees a generic 400 and retries, which 400s again.

**When this bites**:
- C3 context compression (the obvious case — dropping old turns)
- Any future "summarize old messages" feature
- Any future "sliding window context" feature
- Edge case in `compact_messages`: when a pair straddles the **protected tail**
  boundary (current user message is protected), the algorithm must recognize
  `messages[len-2] = assistant(tool_use)` + `messages[len-1] = user(tool_result)`
  and treat them as a single protected unit (not as separate droppable turns).
- The agent loop's own tool-execution gate (08-07-group-chat-role-history-
  isolation follow-up, 2026-08-07): `should_continue` keys on `tool_calls`
  alone — an OpenAI-compatible provider (Console Go) can end a tool_use
  stream with a NON-"tool_use" finish_reason ("stop" → "end_turn", or
  missing). The pre-fix predicate (`stop_reason == Some("tool_use")`)
  skipped tool execution entirely → the assistant(tool_use) row was already
  persisted but no tool_result followed → every later turn 400'd with
  "An assistant message with 'tool_calls' must be followed by tool messages
  responding to each 'tool_call_id'" and the group chat burned
  MAX_ORCHESTRATION_ROUNDS on `[生成出错中断]` retries (DB session
  `d7fe451c`: seq 5 emitted 3 read_file tool_uses, zero tool_results, 26
  consecutive error turns). The correct signal is the tool_calls themselves:
  if the model emitted ANY tool_use they MUST be executed and their results
  fed back; stop_reason only decides the terminal `Done` value.
  Test: `agent_loop_tool_use_with_non_tool_use_stop_reason_still_executes`
  (`agent/tests_agent_loop.rs`).

**Test coverage** (in `agent/context.rs`):
- `case_3_tool_use_tool_result_pair_intact_or_dropped_together`
- `regression_pair_at_tail_split_under_pressure` (C3 PR1 regression)

**Related**:
- Thinking blocks have a similar atomicity requirement (see Validation & Error
 Matrix row "`thinking` block appears after a `tool_use` block in history") —
 the assistant turn is the atomic unit for thinking, while the pair is the
 atomic unit for tool_use.

---

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

## Scenario: LLM Retry / Backoff (A5+, 2026-07-05)

> 堵长会话 5xx / 429 / 网络断连整轮重来的体验缺口。Provider 层补网络重试 + Full Jitter 退避 + retry-after 解析,SSE 断连走"首字节前重试"安全边界。调研:`docs/research/llm-network-resilience-survey.md`;完整 PRD `.trellis/tasks/07-04-a5plus-llm-network-resilience/`;决策见 [IMPLEMENTATION §4 2026-07-05](../../../docs/IMPLEMENTATION/decisions.md)。

###1. Scope / Trigger

- Trigger: `provider.send()` 在**首字节前**(stream 还没 emit 任何 `Ok(ChatEvent)`)返回 `Err(LlmError)`,且分类为可重试(`Network` / `Server` / `RateLimit`)。
- Why now: A5(07-02)错误契约已落地 5 类分类,但 provider 层无重试 — 单次 503 / 429 / 网络抖动让整轮 turn 失败,长会话体验脆。

###2. Retryable 分类 (`LlmError::is_retryable`)

| 分类 | `is_retryable` | 理由 |
|---|---|---|
| `Network` | ✅ true | 连接重置 / DNS / 超时 — 瞬态 |
| `Server` (5xx) | ✅ true | 上游临时故障 |
| `RateLimit` (429) | ✅ true | 配额窗口 + retry-after advisory |
| `Auth` (401/403) | ❌ false | 重试同结果 |
| `InvalidRequest` (4xx 非 429) | ❌ false | 请求本身错,重试无意义 |

###3. Backoff — Full Jitter + retry-after + 双向熔断

`RetryPolicy::default()`: `max_retries=3, base=0.5s, cap=30s, budget=60s, retry_after_cap=60s`。

- **Full Jitter**(AWS Architecture Blog 共识):`sleep = uniform(0, min(cap, base·2^attempt))`。纯指数让并发客户端聚集(thundering herd);Full Jitter 是推荐方案。`attempt` 从 0 起(首次重试 wait ∈ [0, 0.5s])。
- **retry-after advisory 优先**:命中覆盖 jitter(尊重服务端意图),二次封顶 `retry_after_cap=60s`(SDK parity — Anthropic/OpenAI 都 60s),更长 fallthrough 到 jitter。
- **`parse_retry_after`** 解析 5 格式按优先级:`retry-after-ms`(ms)→ `retry-after`(秒整数 / HTTP-date)→ OpenAI `x-ratelimit-reset-requests` / `-tokens`(Go duration `6m0s`/`1s`/`500ms`,自写 parser 不引 humantime)。
- **双向独立熔断**:`max_retries`(次数)与 `budget`(总 sleep 累计)任一触达即停。budget 防 OpenCode 式"session 死几小时"。
- rng 抽象为参数(`fastrand::Rng`),测试注入确定性 seed 避免 flaky。

###4. 首字节边界 (R3) — 核心安全不变量

`retry_open`(`llm/retry.rs`):

```
loop {
  if token.is_cancelled() { return Cancelled }            // R7 早响应
  stream = provider.send(...)
  first = select! { biased; cancel → Cancelled, stream.next() → item }
  match first {
    Ok(ev) => return Stream(once(ev).chain(stream)),      // ★ 首字节 OK,之后永不重试
    Err(e) if retryable && attempt<max && budget_left => {
      emit Retrying; select! { cancel→Cancelled, sleep(wait) }; continue
    }
    Err(e) => return Stream(once_err(e)),                 // 不可重试 / 熔断 → chat_loop had_error
  }
}
```

**`OpenOutcome::Stream` 一旦返回,后续 stream Err 在 chat_loop per-event loop 处理**(had_error + ERROR_MARKER + partial tool),**不再回 retry_open**。

**为何首字节前重试 side-effect-free**:everlasting 的 tool 执行在 stream 完成后(per-event loop 消费完才进 tool 阶段),首字节前重发 = 无 tool 副作用,不需幂等 key / 去重表(research §5.4)。对齐 Claude Code "before visible output" 规则,但因 tool 流后执行故更彻底。

###5. `LlmError` headers 字段扩展(契约变更声明)

`RateLimit` / `Server` 变体加 `headers: HeaderMap` 字段(供 `parse_retry_after` 解析 advisory):

```rust
RateLimit { message: String, headers: HeaderMap },
Server { status: u16, message: String, headers: HeaderMap },
```

`Auth` / `InvalidRequest` / `Network` **不带 header**(网络错误无 response;4xx 非 429 不重试不需 header)。**5 类名称与分类逻辑不变**(prd constraint 3 边界细化,非破坏)。headers 不参与序列化(`LlmError` 不入 DB,无 migration)。

###6. Cancellation (R7)

两个 select 都把 `token.cancelled()` 放 `biased` 第一位:
- 首字节 await race cancel
- backoff sleep race cancel — **sleep 中取消立即响应**(返回 `OpenOutcome::Cancelled`)
- `Cancelled` → chat_loop 走 C1 路径(`cancelled = true`,**不** `had_error`)

retry_open 入口的 `if token.is_cancelled()` short-circuit 让 cancel 在 send 前触发时不调 `provider.send`(cancel 早响应、不浪费请求,是预期语义)。

###7. 前端 Retrying 事件 (R8)

新 `ChatEvent::Retrying { attempt: u32, max_attempts: u32, wait_ms: u64, reason: String }`(wire `kind: "retrying"`,与 `delta`/`start`/`done`/`error` 并列)。`LlmRetrySink`(chat_loop)实现 `RetrySink` trait,通过 `emit_chat_event_via_sink` 真正 emit IPC。

前端 `streamController` `case 'retrying'` 挂到 in-flight assistant placeholder 的 `retrying` 瞬态字段(**不入 messages 数组,`rehydrateMessages` 不 copy,reload 后消失**)。`MessageItem` 在气泡上方渲染 chip:↩ 重试中 N/M,Ts 后重发…(reason)。`start`/`delta`/`done`/`error` 四个 arm 清 chip;malformed payload 防御性 drop。

###8. Tests Required

**`llm/retry.rs` 内联 `#[cfg(test)]`(35 tests)**:
- `full_jitter` 区间(注入确定性 rng)
- `parse_retry_after` 全格式(秒 / ms / HTTP-date / Go duration / 缺失 / >60s 截断)
- `retry_open`:5xx 序列成功 / 429+retry-after / 首字节前断连重试 / **首字节后断连不重试** / Auth 不重试 / max_retries 耗尽 / budget 熔断 / C1 sleep 中取消
- Step 8 边界:budget-先 / max_retries-先 / budget_remaining=0 / advisory clamp 不 overshoot

**`agent/tests_agent_loop.rs` 集成(3 tests)**:
- `a5plus_retry_does_not_double_count_token_usage`(R9 — 直查 SQL `sessions.last_input_tokens == <success_usage>` 非 N× 值;`update_last_turn_usage` 是 UPDATE OVERWRITE,只在 Done arm 调用)
- `a5plus_retry_emits_retrying_chat_events`(R8 — Retrying 字段 + rid 路由)
- `a5plus_retry_terminal_state_matches_no_retry_path`(R9 — retry 路径与无 retry 路径同位终态)

**前端 `streamController.test.ts`(6 tests)**:attach / clear-on-delta / clear-on-start / clear-on-done / clear-on-error / malformed-drop。

###9. Out of Scope

- **SSE 断点续传(message ID)**:research §5.4 证实 SSE 协议无 resumption,只能整请求重发。DESIGN §5.1 风险表"断点续传用 message ID"退路不可行,改走"首字节前重试"(本 Scenario)。
- **`Auth` / `InvalidRequest` 重试**:R2 明确不重试。
- **Retry 入审计**:transient UX,不入 AuditKind(prd grill §4 锁定,避免 AuditKind 膨胀)。

---

## Scenario: E2 trace ChatEvent variants are server-emitted, not LLM-streamed (E2, 2026-07-14)

> **Source**: E2 trace viewer task `07-14-e2-harness-trace-viewer`
> (child-1 `e2-backend-trace-pipeline`).

The 3 E2 trace variants — `ContextCompacted` / `LoopHint` /
`WorkflowBreadcrumb` — are **pushed by the server (`agent::trace::
record_*` helpers), not by the LLM stream**. The provider cannot
emit them, so the SSE consumer arm must drop them defensively if
they somehow appear in the stream (same pattern as `Recall` /
`Retrying` / `FileInjections`):

```rust
// chat_loop.rs SSE consumer fallback arm
ChatEvent::ContextCompacted { .. }
| ChatEvent::LoopHint { .. }
| ChatEvent::WorkflowBreadcrumb { .. } => {
    tracing::warn!(
        request_id = %rid,
        "chat: unexpected trace event in LLM stream (ignoring)"
    );
}
```

**Wire shape** (`#[serde(tag = "kind", rename_all = "snake_case")]`
on the enum):

| Variant | Fields |
|---|---|
| `ContextCompacted` | `seq`, `tokens_before`, `tokens_after`, `dropped_count`, `degradation` (`"none"` / `"no_candidates"` / `"still_over"`) |
| `LoopHint` | `seq`, `hit_count`, `verdict_kind` (`"hard"` / `"soft"`) |
| `WorkflowBreadcrumb` | `seq`, `task_slug` (Option), `status` (Option), `breadcrumb_text` |

**Why server-emitted, not LLM-streamed.** The trace signals describe
harness decisions (C3 compaction result, C2 loop verdict, workflow
breadcrumb text) that the LLM has no knowledge of and no reason to
echo back. Server emit + persist via the `agent::trace::record_*`
helpers gives a single, typed source of truth and lets the
`turn_trace` table be populated in lockstep with the live emit
(double-write best-effort, see `agent/trace.rs`).

**Why the defensive drop arm.** If a future provider is wrapped in
a way that re-emits a `ChatEvent` it observed (e.g. during a
provider rewrite), the SSE consumer would otherwise panic on a
`match` fallthrough. The `warn!` + drop pattern keeps the agent
loop resilient without affecting the trace pipeline.


## Scenario: Image Blocks — dual-form lifecycle (B1, 2026-08-17)

`ContentBlock` 两个图片变体(`08-16-b1-image-multimodal` PR2):

- **`ImageRef { file, media_type }`** — 稳定引用形态(serde tag `image_ref`,内部专用,永不进 provider wire)。存在于历史/role_history clone/C3 估算;`drive.rs` 在 retry_open 前调 `attachments::resolve_image_refs`(每轮每图一次读盘)换成 resolved 形态。**resolve 带不上 session 上下文就会降级占位**——签名显式收 `app_data_dir + session_id`,勿让块脱离请求上下文独立 resolve。
- **`Image { source: ImageSource }`** — resolved 预发形态(serde 即 Anthropic 原生 `{"type":"image","source":{"type":"base64",…}}`,Anthropic adapter serde 直发零转换;OpenAI adapter 映射 `image_url` data URL)。

**Pair Atomicity 不变**:图片只进 user 消息;assistant 消息里的图(防御路径)在 to_wire 降级为文本占位。

**caps 降级(R3)**:`WireCapabilities.supports_images=false` 时 `strip_unsupported` 对 UserBlocks 内 Image **替换**(非丢弃)为 `[image: {label} — 当前模型不支持图片,未发送]` 文本——模型必须知道有图未发,防幻觉。live 实证(08-17,MiniMax-M3):模型读到占位后明确拒答"图片没有送达"。

**When this bites**:① user 消息含图强制 UserBlocks 路径(图不能进 `User{content: String}`);② OpenAI 侧含图 content 必须是数组(text + image_url 混排),无图时保持历史字符串形状(回归锁测试);③ C3 估算对图块用固定 ~1600 tok 垫板(base64 字符串会百倍高估);④ Anthropic cache 断点在首块 text,Image 追加在 user 消息尾部不耦合。


## Scenario: SSE chunk-boundary UTF-8 carry (RULE, 2026-08-18)

**Bug (incident `3qnzktvosvxmsycoz46` turn=25)**:流式 LLM 生成中断,日志
`WARN ... chat: LLM stream errored ... error=network error: non-utf8 chunk:
incomplete utf-8 byte sequence from index 4082`。根因:两个 provider 对每个
网络 chunk 单独 `std::str::from_utf8`,而 TCP/HTTP chunking 会把一个多字节
UTF-8 字符(如 CJK 的 3 字节字)切开分到两个 chunk——chunk 尾部截断属于
"incomplete"(`error_len() == None`)而非损坏,应跨 chunk 拼接,但代码直接
`yield Err(Network)` 杀掉了整轮健康的生成。中文会话高频触发(3 字节/字,
单轮几千 token,25 轮里迟早抽中边界)。

**Rule**:流式 SSE 解码必须跨 chunk carry 残留字节;`error_len().is_none()`
(尾部截断)时缓冲等下一 chunk,只有 `error_len().is_some()`(流内真无效字节)
才报 Network 错。共享 helper 在 `llm/sse.rs` 的 `utf8_chunk_text(&mut carry,
&bytes)`:`Ok(Some(text))` / `Ok(None)`(等待)/ `Err(Utf8Error)`(硬错误)。
两个 provider(`anthropic.rs` / `openai.rs` 的 stream 循环)都必须走它,禁止
对 `bytes_stream()` 的裸 chunk 直接 `from_utf8`。

**When this bites**:① 任何 UTF-8 多字节内容(中文/emoji/数学符号)长输出;
② 代理/网关分包偏小或乱切时概率上升;③ 换 provider 重写流式读取时照抄了
"每 chunk 独立解码"的旧模式。回归测试在 `llm/sse.rs`(CJK 切 3 段、逐字节
喂、ASCII 前缀 + 截断尾、无效字节仍报错)。
