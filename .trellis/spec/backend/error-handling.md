# Error Handling

> How errors are handled in this project.

---

## RULES 索引(error-handling 相关 RULE-A-*)

> 集中收纳与错误处理相关的 RULE-A-* 不变量。每条引 IMPLEMENTATION.md §4 对应日期 ADR。grep by `RULE-A-NNN`。

| RULE | 日期 | 含义 | 状态 | 主要落地 |
|---|---|---|---|---|
| RULE-A-001 | 2026-06-12 | C3 上下文压缩 tail pair orphan 不变量(compress 不留孤儿) | closed | `agent/context.rs`;ADR [IMPLEMENTATION §4 2026-06-12](../../../docs/IMPLEMENTATION.md#4-决策日志) |
| RULE-A-002 | 2026-06-14 | C3 over-budget degradation(超预算降级,不静默截断) | closed | `agent/context.rs`;ADR 2026-06-14 |
| RULE-A-003 | 2026-06-15 | **normal-path persist 失败 → emit `ChatEvent::Error{Server}` + abort,不静默吞用户消息** | closed `d8ee7d9` | `agent/chat_loop.rs`;本文 §"Agent Loop Error Paths" |
| RULE-A-004 | 2026-06-15 | cancel 掉的 tool 不记 audit / 不 commit tool_result(audit 在 cancel check 之后) | closed | `agent/chat_loop.rs`;[`backend/agent-loop-architecture.md`](./agent-loop-architecture.md) |
| RULE-A-006 | 2026-06-15 | chat_loop 单一权威(production = test 同一 `run_chat_loop`,23 参) | closed `759607c` | `agent/chat_loop.rs` + `agent/subagent/dispatch.rs` |
| RULE-A-007 | 2026-06-17 | **error arm 对称 cancel 路径 persist partial turn**(不丢弃已累积 text/thinking/tool_use) | closed | `agent/chat_loop.rs` + `agent/helpers.rs`(`ERROR_MARKER`);本文 §"Agent Loop Error Paths" |
| RULE-A-010 | 2026-06-17 | cancel「一次即终止」MVP 简化(未实现二次取消语义,spec 偏离声明) | closed(spec 偏离) | [`docs/ARCHITECTURE.md §2.5.1`](../../../docs/ARCHITECTURE.md) |
| RULE-A-012 | 2026-06-19 | **reqwest per-chunk `read_timeout` + stream-error `tracing::warn!`**(不静默 wrap) | closed | `llm/provider/anthropic.rs` + `openai.rs` + `agent/chat_loop.rs`;本文 §"RULE-A-012" |
| RULE-A-013 | 2026-06-19 | path-in-root 收口(async 并行只读工具集 boundary) | closed | `tools/`;[`spikes/2026-06-19-async-parallel-tool-research.md`](../../../docs/spikes/2026-06-19-async-parallel-tool-research.md) |
| RULE-A-014 | 2026-06-20 | worker 的 Tier 4 `ask_path`/`ask_shell` collapse to Deny(workers 无 UI sink,否则挂起) | closed PR2b | `agent/chat_loop.rs`(`is_worker` 第 21 参) |
| RULE-A-015 | 2026-06-20 | `skip_persist` gate 校正(PR1 过宽 18 → 实际 16);2026-06-26 partial reversal(worker token 隔离到 `subagent_runs`) | closed PR2a | `agent/chat_loop.rs`;[`backend/agent-loop-architecture.md`](./agent-loop-architecture.md) Pattern |
| RULE-A-016 | 2026-06-20 | worker `ask_path` 写 transcript `PermissionAsk`(historical mode 直吃 payload_json) | closed PR3a | `agent/subagent/transcript.rs` |
| RULE-A-017 | 2026-06-28 | C3 `agent_loop_c3_compaction_does_not_panic` 测试 setup 过时(指令注入撑大消息 vec 致 StillOver 误触发);只改测试,生产码零改动 | closed | `agent/tests_agent_loop.rs`;task [06-28-fix-rule-a017-c3-test-fail](../../tasks/archive/2026-06/06-28-fix-rule-a017-c3-test-fail/) |

---

## Overview

错误处理围绕 4 个目标(缺一不可):

1. **可观测** — 开发者能从 tracing log 定位根因(每个错误留 Rust 侧 breadcrumb,见 RULE-A-012)。
2. **可恢复** — 用户能判断是否值得重试(`retryable` 字段暴露给前端,Server/Network/RateLimit 默认可重试)。
3. **可读** — 用户看到中文友好消息(`user_message()`),而非 `unwrap` panic 或原始英文 stack。
4. **不静默** — 持久化失败、流中断、状态机违例都不允许吞掉(RULE-A-003 / A-007 / A-012)。

错误在 4 层流转,每层有明确职责:

| 层 | 职责 | 边界契约 |
|---|---|---|
| **Tool 内部** | tool 执行的领域错误(校验/IO/git/网络) | thiserror enum,`#[error]` 中文友好 |
| **Agent Loop** | 把领域错误包成 terminal `ChatEvent::Error` + persist partial turn | `LlmError` 5 类是 category 原型;RULE-A-007 对称 cancel |
| **IPC command** | Tauri command 返回结构化错误 | `AppCommandError` wire shape(本文 §API Error Responses) |
| **前端 errorBus** | `invoke().catch()` 收口,按 category 路由(toast/inline) | `useErrorBus` composable |

不变量:每个请求**恰好一个 terminal event**(RULE-A-007);persist 失败在 normal path emit、在 terminal path log-only(见 §Agent Loop Error Paths 表)。

---

## Error Types

### `LlmError`(`llm/error.rs`)— category 原型

5 个 variant,既是 LLM 调用错误,也是全仓 5 类 `ErrorCategory` 的原型:

| variant | category | retryable | 触发 |
|---|---|---|---|
| `Auth(String)` | Auth | false | API key 失效 / 401/403 |
| `RateLimit(String)` | RateLimit | true | 429 |
| `InvalidRequest(String)` | InvalidRequest | false | 4xx(非 401/403/429),含 Anthropic thinking 400 |
| `Server { status, message }` | Server | true | 5xx |
| `Network(String)` | Network | true | 连接/超时 |

> **spec drift 修正(2026-07-02)**:本节原称该 enum 为 `LlmErrorKind`、含 `Protocol` variant。代码实际命名为 `LlmError`(`llm/error.rs:17`),`Protocol` variant 实际叫 `InvalidRequest`。category enum `LlmErrorCategory` 在 `llm/types.rs`。

### 其余 9 个对外错误类型

均 `impl AppError`(trait 见 §Error Handling Patterns),variant → category 完整映射见任务 [`07-02-a5-error-handling-refine`](../../tasks/07-02-07-02-a5-error-handling-refine/design.md) §5。组成:

- **8 个 thiserror**:`GitError`(`git/error.rs`,NotARepo/Io/Git2/Dirty)、`MemoryInsertError` + `StatusTransitionError`(`db/memories.rs`)、`BackgroundShellError`(`background_shell/mod.rs`)、`ReflectError`(`agent/auto_reflect.rs`)、`WebFetchError`(`tools/web_fetch.rs`)、`ProviderBuildError`(`llm/provider/mod.rs`)。
- **2 个手写**:`QuestionStoreError`(`agent/question_store.rs`,手写 Display + 空 `impl Error`)、`PreFlightError`(`agent/provider.rs`,需补 Display + Error impl)。

排除:`ValidationError`(`tools/ask_user_question.rs`,`pub(crate)` tool 内部错误,不冒泡 IPC 边界,不纳入 `impl AppError`)。

---

## Error Handling Patterns

### 传播:`?` + `From<E>`

领域错误用 `?` 向上传播;IPC command 边界通过 `From<E> for AppCommandError` 收口:

```rust
// commands 内部:? 配合 From 转换(含 anyhow 边界兜底)
async fn create_session(req: CreateSessionReq) -> Result<SessionId, AppCommandError> {
    let id = db::create_session(&pool, req.project_id).await?;  // sqlx::Error → anyhow → AppCommandError(Server)
    Ok(id)
}
```

`From` 覆盖:10 个领域类型 + `From<anyhow::Error>`(commands 大量 `?anyhow`,未命中已知类型则归 Server/`kind="Anyhow"`)。

### `AppError` trait(`app/src-tauri/src/error.rs`)

每个对外错误类型 `impl AppError`,提供统一的对外接口:

```rust
pub trait AppError: std::error::Error {
    fn category(&self) -> ErrorCategory;
    fn user_message(&self) -> String;          // 中文友好,直接展示用户
    fn retryable(&self) -> bool {              // 默认按 category 派生
        matches!(self.category(), ErrorCategory::Server | ErrorCategory::Network | ErrorCategory::RateLimit)
    }
}
```

- `retryable` 默认派生,**本期零 override**(初版基于不存在的 `BackgroundShellError::Timeout` 的 override 设计已删除)。
- 两个 variant 在 `category()` 内部做条件分流:`WebFetchError::HttpStatus(u16)`(4xx→InvalidRequest / 5xx→Server)、`PreFlightError`(EmptyApiKey/DecryptFailed→Auth,其余→InvalidRequest)。

### 不静默(RULE-A-003 / A-007 / A-012)

- **persist 失败不静默**(RULE-A-003):normal-path `persist_turn` 失败 → emit `ChatEvent::Error{Server}` + abort;terminal-path(error/cancel)log-only(避免 double-terminal,见 §Agent Loop Error Paths 表)。
- **stream error 必留 trace**(RULE-A-012):`LlmError` 包成 `ChatEvent::Error` 时**必须**先 `tracing::warn!(category = ..., error = %err)`,否则错误只在前端 toast 可见、reload 后消失。
- **error arm 不丢内容**(RULE-A-007):流中断时已累积的 text/thinking/tool_use 必须 flush + persist(对称 cancel 路径)。

### 前端:useErrorBus 收口

`invoke().catch(e => useErrorBus().handle(e))` → `parseAppCommandError(e)`(容错 Tauri String rejection / JSON parse fail / 对象 + category 值域校验)→ 按 category 路由(Auth→Settings 引导 / RateLimit→toast / InvalidRequest→inline / Server→toast+重试 / Network→toast)。全局 `window.onerror` + `unhandledrejection` 兜底。

---

## API Error Responses

Tauri command 的错误返回统一为 `AppCommandError` wire shape(`app/src-tauri/src/error.rs`)。前端按 `category` 路由,`kind`/`requestId` 供诊断与日志关联。

### `AppCommandError` schema

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppCommandError {
    pub category: ErrorCategory,   // 前端路由键
    pub kind: String,              // 诊断短名,如 "LlmError::Auth" / "GitError::NotARepo"
    pub message: String,           // 中文友好,直接展示
    pub retryable: bool,           // 前端决定是否提供"重试"
    pub request_id: Option<String>,// chat command 透传前端 requestId;其余 None(见下 §request_id 语义)
}
```

IPC JSON payload:

```json
{
  "category": "RateLimit",
  "kind": "LlmError::RateLimit",
  "message": "请求过于频繁,请稍后再试",
  "retryable": true,
  "requestId": "mz8s3hqwx6rmqjswgte"
}
```

### `request_id` 语义

`request_id` 是 **chat 流的 cancel 配对 id**(前端为每个 `chat` invoke 生成 UUID,`cancel_chat` 凭它找到 `CancellationToken`)。R3.4 落地后的真实边界:

- **`chat` command**(`agent/chat.rs`):前置错误 emit `ChatEvent::Error` 走 stream;`AppCommandError` 只在 emit 本身失败的 IPC 双重故障路径返回,带 `Some(request_id)`(透传)。
- **其他所有 command**(sessions / projects / `merge_worker_run` / `discard_worker_run` / config / ...):UI-driven,不在 chat 流,**无 requestId 概念**,`request_id: None`。
- **后端不新生成 uuid**(会与前端 requestId 脱钩,tracing 对不上)。

> L3b PR4 曾给 `merge_worker_run`/`discard_worker_run` 预留 `_rid` sentinel(`"merge-pr4"`/`"discard-pr4"`),A5 R3.4 审查发现它是死占位(无消费方)后已清理。requestId 是 chat 独有语义,不为非 chat command 强加。

### 5 类 `ErrorCategory` 全集

全仓 10 个对外错误类型均归并到这 5 类(完整 variant→category 映射见任务 design.md §5):

| category | retryable 默认 | 覆盖 | 前端路由 |
|---|---|---|---|
| `Auth` | false | API key 缺失/失效 / 401/403 / 解密失败 / `PreFlightError::EmptyApiKey/DecryptFailed` | toast + 引导去 Settings |
| `RateLimit` | true | 429 | toast("请求过于频繁") |
| `InvalidRequest` | false | 4xx(非 401/403/429)/ 数据校验 / 状态机违例 / 配置错 / SSRF / NotFound | 内联错误(调用点) |
| `Server` | true | 5xx / DB(sqlx)错 / 内部错误 / libgit2 / io | toast + 重试按钮 |
| `Network` | true | 连接超时 / DNS / TLS / 中断 | toast(检查网络) |

### `From<E>` 转换

`AppCommandError` 集中实现 `From<E>`:10 个领域类型(各调 `AppError::category()/user_message()/retryable()`)+ `From<anyhow::Error>` 边界兜底(commands 大量 `?anyhow`,先 downcast 已知类型,未命中归 Server/`kind="Anyhow"`)。

### `LlmError` 5 类(category 原型)

`LlmError`(`llm/error.rs`)是 category 原型,5 个 variant 与 `ErrorCategory` 1:1:

- `Auth` — bad or missing API key, 401/403 from upstream.
- `RateLimit` — 429.
- `Server` — 5xx.
- `Network` — connection / timeout.
- `InvalidRequest` — 4xx other than 401/403/429, including 400 from a malformed request
  body. **This is the bucket that catches the thinking-related failures below.**

### Anthropic 400 from extended-thinking contract violations

The `InvalidRequest` kind covers failures caused by our own payload. Three patterns
all surface as a 400 with a message like `"messages.0.content.0.signature: Field required"`:

| Cause | Fix |
|-------|-----|
| Thinking block omitted from history on round-trip | The block is mandatory on the next turn after a thinking turn. Rehydrate must include all thinking blocks with their full `signature`. |
| `signature` lost, truncated, or mutated | Store verbatim; emit verbatim. The signature is opaque. |
| Thinking block positioned after `tool_use` (or anywhere other than the head of the assistant message) | `toPayloadContent` must put thinking blocks first. |

See `backend/llm-contract.md` §4 Validation & Error Matrix for the full list.

---

## Agent Loop Error Paths — terminal event + persist invariants

The agent loop's `run_chat_loop` (see `backend/agent-loop-architecture.md`)
has three terminal paths that exit the per-turn stream loop early:
**normal Done**, **cancel**, and **error**. Each has its own
terminal-event + persist contract.

### Path: `ChatEvent::Error` mid-turn (RULE-A-007, 2026-06-17)

When the LLM stream emits `ChatEvent::Error`, the per-event arm:

1. Emits the `Error` to the frontend immediately (this is the
   terminal signal — the controller treats it as end-of-stream).
2. Sets `had_error = true` and breaks out of the stream loop.

After the stream loop, the agent loop **persists the partial turn**
symmetric with the cancel path (RULE-A-007 fix; previously the error
arm did `if had_error { return; }` and dropped all accumulated
content):

1. Flushes pending thinking into `finalized_thinking`.
2. Builds assistant blocks (`thinking` + `text` + `tool_use` +
   `redacted_thinking`).
3. Appends `ERROR_MARKER` (`"[生成出错中断]"`) to the text —
   symmetric to the cancel path's `CANCELLED_MARKER`. Empty-text
   edge case: marker alone.
4. `persist_turn` the partial row.
5. Emits `ChatEvent::TurnComplete { seq, ...latency }` so the
   frontend has the partial row's seq + latency (RULE-A-007
   decision C). This **coexists** with the pre-emit `Error` event
   — they carry disjoint information and the controller routes
   each independently.
6. Persists cwd + touches the session, then returns. The error
   path does NOT emit a follow-up `Done` event (the pre-emit
   `Error` is the terminal; emitting `Done` would conflict).

### Persist failure on the error path is log-only (RULE-A-007 decision B)

RULE-A-003 (2026-06-15) made **normal-path** persist failures
emit a typed `ChatEvent::Error{Server}` + abort (so disk-full /
DB-lock contention doesn't silently swallow the user message).
The error path is **different**: the per-event arm already emitted
the terminal `Error`. Calling `emit_persist_failure` on top would
produce two terminal events (Error + Error) and the frontend's
terminal handling would fire twice.

The error path therefore follows the **same log-only pattern** the
cancel path uses for its synthetic tool_result persist (cancel's
terminal `Done{cancelled}` is about to fire, so an Error there
would also be a double-terminal). The "exactly one terminal event
per request" invariant stays intact.

| Persist site | Failure handling | Why |
|---|---|---|
| Initial user message (normal path) | `emit_persist_failure` + return | First persist; no terminal yet — Error becomes the terminal (RULE-A-003) |
| Assistant turn (normal Done path) | `emit_persist_failure` + return | Mid-request; no terminal yet — Error becomes the terminal (RULE-A-003) |
| Tool_result turn (normal path) | `emit_persist_failure` + return | Mid-request; no terminal yet — Error becomes the terminal (RULE-A-003) |
| Cancel's synthetic tool_result persist | `tracing::error!` log-only | Cancel's terminal `Done{cancelled}` is about to fire — double-terminal hazard |
| Cancel's cancelled tool_result persist | `tracing::error!` log-only | Same as above |
| **Error path's assistant partial persist** | **`tracing::error!` log-only** | **Per-event arm already emitted terminal `Error` — double-terminal hazard (RULE-A-007 decision B)** |

---

## Common Mistakes

<!-- Error handling mistakes your team has made -->

### Mistake: spec drift — 错误类型名 / variant 名与代码不一致

`error-handling.md` 曾两处与代码脱节:

- 称 LLM 错误 enum 为 `LlmErrorKind`,代码实际是 `LlmError`(`llm/error.rs:17`),category enum `LlmErrorCategory` 在 `llm/types.rs`。
- 称 5 类含 `Protocol` variant,代码实际命名为 `InvalidRequest`。

更早的规划文档(design.md §5 映射表)曾凭印象写出 `GitError::NotFound/Conflict`、`BackgroundShellError::Timeout`、`ReflectError::Parse`、`WebFetchError::Http4xx/Http5xx` 等**不存在的 variant** —— 把不存在的 variant 当契约来规划,会让实施直接返工。

**防护**:

- 写 spec / planning 前,先 `rg "pub enum .*Error" -g '*.rs'` + 逐 variant 核对真实名(thiserror 的 `#[error]` 行 + struct field 形态)。
- A5 落地 grep-style 单测(R5.2):`grep -rE "Result<.*, *String>" app/src-tauri/src/commands/` 必须无结果,挡住"签名回退"类 drift。
- 本文档 §Error Types 已用真实 variant 名;新增错误类型时同步更新本节 + 任务 design.md §5 映射表。

### Mistake: dropping the `signature` to "save space"

The `signature` on a `ContentBlock::Thinking` is a cryptographic anchor for
Anthropic. Drop it and the next turn 400s. The DB stores it in full; the
rehydrate path emits it in full. There is no compression, no truncation, no
"redact for privacy" — the field is opaque and the only safe behavior is
verbatim round-trip.

### Mistake: emitting `signature_delta` per SSE event

`signature_delta` is buffered in `BlockState::Thinking { signature_buf }` and
emitted as a single `ChatEvent::SignatureDelta` on `content_block_stop`.
Per-event emit was the step 6 v1 implementation; the check phase caught it
because Anthropic might split the signature across N events in a future
schema, and a per-event emit would scatter chunks across N thinking blocks.
See `backend/llm-contract.md` §7 Wrong vs Correct.

---

## RULE-A-012 (2026-06-19) — reqwest per-chunk `read_timeout` + stream-error tracing

> **Incident anchor**: 2026-06-18T17:56:52.654362Z, `request_id=mz8s3hqwx6rmqjswgte`,
> `messages.seq=37` (DB query confirms: `text="[生成出错中断]"`, partial thinking
> in content, seq=36→37 gap = 60.403s = exact reqwest total-deadline).

### Pattern A: streaming HTTP client config

When building a reqwest client for **SSE / chunked streaming responses**, use
`read_timeout` instead of `timeout`. Per the reqwest source docs
(`async_impl/client.rs:1448-1459`):

```rust
// ❌ WRONG — `.timeout()` is a TOTAL deadline from connect to body EOF.
//    For SSE, the body is unbounded and chunk rate varies (extended
//    thinking on a 3rd-party proxy can be 60s+ before the first text
//    delta). The 60s total will fire mid-stream.
let client = reqwest::Client::builder()
    .timeout(Duration::from_secs(60))
    .connect_timeout(Duration::from_secs(10))
    .build()?;

// ✅ CORRECT — `.read_timeout()` is per-read, resets on each chunk.
//    "More appropriate for detecting stalled connections when the size
//    isn't known beforehand." (reqwest source, verbatim). The 60s value
//    bounds silence between chunks; a truly dead proxy will surface
//    quickly while a slow-but-alive proxy streams freely.
let client = reqwest::Client::builder()
    .read_timeout(Duration::from_secs(60))
    .connect_timeout(Duration::from_secs(10))
    .build()?;
```

Applies at: `app/src-tauri/src/llm/provider/anthropic.rs:209-227` and
`app/src-tauri/src/llm/provider/openai.rs:424-442` (both Provider impls).

### Pattern B: stream-error observability (no silent wrap)

The agent loop's per-event arm wraps `LlmError` into `ChatEvent::Error` for
the frontend to toast. **The wrap MUST also emit a `tracing::warn!`** so the
Rust log has a breadcrumb. Otherwise the error is only visible in the UI
until reload — exactly the situation in the 2026-06-18 incident (zero
`WARN` / `ERROR` log lines).

```rust
// ❌ WRONG — silent wrap. User sees the toast; logs see nothing.
Err(err) => ChatEvent::Error {
    message: err.user_message(),
    category: err.category(),
},

// ✅ CORRECT — log first, then wrap. The `category` field gives an
//    immediate classifier (Auth / RateLimit / InvalidRequest / Server /
//    Network) without needing to parse `err.user_message()`.
Err(err) => {
    tracing::warn!(
        request_id = %rid,
        turn,
        category = err.category(),
        error = %err,
        "chat: LLM stream errored"
    );
    ChatEvent::Error {
        message: err.user_message(),
        category: err.category(),
    }
}
```

Applies at: `app/src-tauri/src/agent/chat_loop.rs:657-682` (per-event
arm inside the `event_result = stream.next()` select! branch).

### Out of scope (deliberate non-fix)

| Option | Why deferred |
|---|---|
| Raise total `timeout` to 600s (LiteLLM-style) | `read_timeout=60s` already covers slow-but-alive streams. A 60s silence truly means dead proxy — surfacing then is correct behavior. |
| Add `request_timeout_secs` column to `providers` / `models` tables | Premature DB schema churn. Revisit only if real users hit real per-provider timeouts. |

### Cross-references

- [docs/IMPLEMENTATION.md §4 2026-06-19](../../docs/IMPLEMENTATION.md#4-决策日志) — full ADR with alternatives rejected.
- [`.trellis/reviews/DEBT.md`](../../reviews/DEBT.md) — RULE-A-012 entry.
- [`.trellis/tasks/2026-06/06-19-fix-llm-streaming-timeout-and-tracing/`](../../tasks/2026-06/06-19-fix-llm-streaming-timeout-and-tracing/) — task directory.
- Related: RULE-A-007 (error arm partial-turn persistence) — same code path, complementary fix (one persists, one traces).
