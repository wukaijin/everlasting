<!-- Token usage tracking scenario. Moved from llm-contract.md 2026-06-21 (A4, 2026-06-10) -->

# Token Usage Tracking (A4, 2026-06-10)

> **Source**: extracted from `.trellis/spec/backend/llm-contract.md` §"Scenario: Token Usage Tracking" (2026-06-21 doc-trim task).
>
> **Cross-references**:
> - Main LLM contract: [llm-contract.md](./llm-contract.md)
>
> **⚠ 2026-06-26 snapshot 重构**（task `06-26-fix-token-usage-snapshot`）：上下文占用从「每 turn **累加**」改为「**最后一次请求的快照**」。变更要点：
> - `TokenUsage` 加第 5 字段 `context_input_tokens`（跨 provider 归一化「本次请求总输入」，前端 `%` 的分子）
> - sessions 加 5 个 `last_*` 快照列（覆盖写）；`add_token_usage` 删除，换 `update_last_turn_usage`
> - worker（子代理）token **不再** fold 进父 session（reversal of RULE-A-015/PR2a），隔离到 `subagent_runs.token_usage_json`
> - 前端 `accumulateTokenUsage` → `setLastTurnUsage`（覆盖写，非 `+=`）；ChatInput 分子改 `context_input_tokens`
>
> 本文 §2 Signatures / §3 Contracts / §4 Validation Matrix / §5 Cases / §6 Tests / §7 Wrong-vs-Correct 已**全部同步**为 snapshot 语义（2026-06-26 doc-audit：修正了 §3 wire 说明、§4 矩阵、§5 用例、§6 测试清单、§7 示例中残留的累加 / `camelCase` / 已删函数名表述；仅 Design Decisions 段保留 A4 原始决策措辞，顶部注记标明 DB 语义已转 snapshot）。spec 初衷（§1 line "current context usage, **not cumulative session totals**"）至此落地。

## Scenario: Token Usage Tracking (A4, 2026-06-10)

### 1. Scope / Trigger

- Trigger: the agent loop needs per-session token totals to drive the
 ChatInput hint area (Anthropic-style statusline: "current context
 usage, not cumulative session totals", but scoped to a single
 session). The data must round-trip the LLM → Rust → SQLite →
 Pinia → ChatInput.vue without the agent loop ever touching
 protocol-specific field names.
- Why code-spec depth: mandatory — the new `TokenUsage` struct is
 the cross-layer contract that touches `ChatEvent::Done`, the
 Anthropic SSE parser, the OpenAI SSE parser, the agent loop's
 accumulation write, the DB schema, and the frontend's
 `chatStore.tokenUsageBySession`. A change here cascades through
 every layer.

### 2. Signatures

#### Backend types (`app/src-tauri/src/llm/types.rs`)

```rust
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenUsage {
 pub input_tokens: u32,
 pub output_tokens: u32,
 pub cache_creation_input_tokens: u32,
 pub cache_read_input_tokens: u32,
 /// 2026-06-26: 跨 provider 归一化的「本次请求总输入」= 进入
 /// context window 的全部 prompt token（含缓存命中）。前端 `%` 分子。
 /// Anthropic: input+cc+cr; OpenAI: prompt_tokens（勿再加 cache_read）。
 #[serde(default)]
 pub context_input_tokens: u32,
}

pub enum ChatEvent {
 // ... existing variants ...
 Done {
 stop_reason: Option<String>,
 usage: Option<TokenUsage>, // <-- A4 field
 },
 // ... existing variants ...
}
```

#### DB schema (`migrations.rs` A4 ALTER)

```sql
ALTER TABLE sessions ADD COLUMN input_tokens_total INTEGER;
ALTER TABLE sessions ADD COLUMN output_tokens_total INTEGER;
ALTER TABLE sessions ADD COLUMN cache_creation_total INTEGER;
ALTER TABLE sessions ADD COLUMN cache_read_total INTEGER;
```

All four columns are **nullable** (no `DEFAULT`) and are now
**frozen** (2026-06-26 snapshot 重构后代码不再写入；保留列 +
`SessionRow`/`SessionSummary` 字段以避免 migration/类型连锁，
后续 debt PR 可清理)。

**2026-06-26 snapshot 列**（`migrations.rs` 加在 A4 4 列之后，覆盖写）：

```sql
ALTER TABLE sessions ADD COLUMN last_context_input_tokens INTEGER;
ALTER TABLE sessions ADD COLUMN last_input_tokens INTEGER;
ALTER TABLE sessions ADD COLUMN last_output_tokens INTEGER;
ALTER TABLE sessions ADD COLUMN last_cache_creation INTEGER;
ALTER TABLE sessions ADD COLUMN last_cache_read INTEGER;
```

5 列均 nullable，语义 = 「最后一次 LLM 请求」的快照（每次
`Done` 事件**覆盖写**，非累加）。`last_context_input_tokens`
是前端 `%` 的分子；4 个分量供 ChatInput 展开详情显示。pre-snapshot
session 全 NULL，前端 fallback 显示「—」（不是 0%）。

#### DB function (`app/src-tauri/src/db/sessions.rs`)

```rust
// 2026-06-26: 删除累加式 add_token_usage，换覆盖式快照。
pub async fn update_last_turn_usage(
 pool: &SqlitePool,
 session_id: &str,
 usage: &TokenUsage,
) -> Result<(), sqlx::Error> {
 // 单 UPDATE：5 个 last_* 列覆盖写（= 而非 +=），updated_at bumped。
}
```

#### Frontend payload (`app/src/stores/streamController.ts`)

```typescript
interface ChatEventPayload {
 request_id: string;
 kind: "start" | "delta" | "..." | "done" | "error";
 // ... existing fields ...
 usage?: { // <-- A4 field; only present on `done` events
 input_tokens: number;
 output_tokens: number;
 cache_creation_input_tokens: number;
 cache_read_input_tokens: number;
 };
}
```

### 3. Contracts

#### Wire format (snake_case, both layers)

```jsonc
// ChatEvent::Done { usage: Some(t) } on the chat-event channel:
{
 "kind": "done",
 "stop_reason": "end_turn",
 "usage": {
 "input_tokens": 1234,
 "output_tokens": 56,
 "cache_creation_input_tokens": 100,
 "cache_read_input_tokens": 200
 }
}

// ChatEvent::Done { usage: None } (cancel / error / network drop):
{
 "kind": "done",
 "stop_reason": "cancelled",
 "usage": null
}
```

The IPC field is **snake_case** end to end (the existing `kind`
discriminator and the existing `stop_reason` are both snake_case;
mixing styles here would break the `parse_*` symmetry on the TS
side). Field names mirror Rust's `TokenUsage` 1:1 — no
`camelCase` rewrite on the boundary. The outer `ChatEventPayload`
(`app/src-tauri/src/state.rs`) has **no** `rename_all` attribute —
it is just `{ request_id, #[serde(flatten)] event: ChatEvent }`,
and `ChatEvent` itself is `#[serde(tag = "kind", rename_all =
"snake_case")]`. So `kind`, `stop_reason`, and every field inside
`usage` are all snake_case on the wire — one consistent style,
not a camelCase-outer / snake_case-inner "polyglot" payload.
**See "Wrong vs Correct" §7 for the rationale.**

#### Anthropic protocol mapping

The Anthropic SSE `message_delta` event carries:

```jsonc
{
 "type": "message_delta",
 "delta": { "stop_reason": "end_turn" },
 "usage": {
 "input_tokens": 1234,
 "output_tokens": 56,
 "cache_creation_input_tokens": 100,
 "cache_read_input_tokens": 200
 }
}
```

The Anthropic adapter's `parse_anthropic_usage(usage_value)`
function (in `provider/anthropic.rs`) reads all four fields
verbatim. `usage` is **cumulative per turn** — the first
`message_delta` event for a turn typically reports `output_tokens:
1`; later ones carry the cumulative value. The adapter keeps the
**last seen** payload in a `let mut usage: Option<TokenUsage> = None`
local and yields it on the terminal `Done` event. A `usage: {}`
or all-zero payload is treated as `None` ("no usage") to skip the
agent loop's SQL write.

Some Anthropic-compatible proxies also attach `usage` to the
`message_start` event (an initial baseline). The adapter reads
this as the first non-null `usage` and lets subsequent
`message_delta.usage` overwrite it.

#### OpenAI protocol mapping

The OpenAI Chat Completions final chunk (when
`stream_options.include_usage: true` is set on the request body)
carries:

```jsonc
{
 "usage": {
 "prompt_tokens": 200,
 "completion_tokens": 30,
 "total_tokens": 230,
 "prompt_tokens_details": { "cached_tokens": 50 }
 }
}
```

The OpenAI adapter's `parse_openai_usage(usage_value)` function
(in `provider/openai.rs`) normalizes:

- `prompt_tokens` → `input_tokens`
- `completion_tokens` → `output_tokens`
- `prompt_tokens_details.cached_tokens` → `cache_read_input_tokens`
- `cache_creation_input_tokens` → 0 (no OpenAI equivalent today)

The adapter requires `stream_options: { include_usage: true }` on
the **request body** (set in `build_http_body`). Without this,
OpenAI omits the `usage` field on all chunks and the agent loop
has no per-turn token counts.

#### Agent loop snapshot write（2026-06-26 R3 — replaces R2 accumulation）

The agent loop's `ChatEvent::Done` handler in
`app/src-tauri/src/agent/chat_loop.rs` (Done-event arm, ~:1128-1189):

```rust
ChatEvent::Done { stop_reason: sr, usage } => {
 // ...
 if let Some(t) = usage {
     // 2026-06-26 reversal of RULE-A-015/PR2a: 重新关回 !skip_persist gate。
     // worker 复用父 session_id，若继续写会让父「上下文占用 %」混入
     // 子代理 turn（实测 1.7M/100% 爆表）。worker token 隔离到
     // subagent_runs.token_usage_json（dispatch.rs cumulative_usage 写出）。
     if !skip_persist {
         if let Err(e) = crate::db::update_last_turn_usage(&db, &session_id, t).await {
             tracing::warn!(error = %e, "failed to update last-turn usage");
         }
     }
 }
}
```

`update_last_turn_usage` **覆盖写** 5 个 `last_*` 列（`= ?`，不是
`+= ?`）。语义是「最后一次请求的占用快照」——多 turn session 的
`last_context_input_tokens` 反映**最近一次**请求的 context window
占用，不是历史和。worker（`skip_persist=true`）路径被 gate 挡住，
不写父 session；其 token 由 `SubagentBufferSink::cumulative_usage()`
在 worker 退出时写入 `subagent_runs.token_usage_json`。

#### Frontend snapshot (`chat.ts` + `streamController.ts`，2026-06-26 重构）

`streamController.handleChatEvent("done")` calls
`useChatStore().setLastTurnUsage(sid, event.usage)`（原
`accumulateTokenUsage`，改为**覆盖写** `tokenUsageBySession.set(sid,
{...usage})`，删 `+=` 分支）。`tokenUsageBySession: reactive(Map)` 持有
**最后一次请求**的快照（非 running total）。`currentSessionTokenUsage`
computed 供 ChatInput 读取。

`event.usage.context_input_tokens` 是前端 `%` 的分子（÷
`modelsStore.defaultModel.contextWindow`）。wire payload 上该字段
optional + fallback `input+cache_creation+cache_read`（兼容旧后端）。

Map 也从 `SessionSummary.last_*` **seed**（`loadSessions` 判定
`last_context_input_tokens !== null`），所以 reload 显示最后一次
快照（pre-snapshot session 显「—」）。

#### Color thresholds (UI)

| Percentage of `context_window` | Color | CSS class |
|--------------------------------|-------|-----------|
| 0-49% | green (`#4ade80`) | `chat-input__token-usage--ok` |
| 50-74% | amber (`#fbbf24`) | `chat-input__token-usage--warn` |
| 75%+ | red (`var(--color-tool-error)`) | `chat-input__token-usage--alert` |

The 50% / 75% thresholds are the same as Anthropic's statusline
recommendation. The CSS uses `var()` for the red (a project
token) and direct hex for green / amber (Tailwind 400-family
colors, not in the design token system per
`.trellis/spec/frontend/design-tokens.md` "Don't add a new
`--color-*` token for a one-off use" rule).

### 4. Validation & Error Matrix

| Condition | Result |
|-----------|--------|
| `ANTHROPIC_API_KEY` missing at startup | LLM stream never opens; the chat command returns pre-flight `ChatEvent::Error`. No usage data is ever written. |
| Anthropic `message_delta` event has no `usage` field | `parse_anthropic_usage` returns `None`; the `usage` local stays `None`; agent loop's `if let Some(t) = usage` skips the write. |
| Anthropic `usage` is all-zero | `parse_anthropic_usage` returns `None` (deliberate — see §5 Base case). Agent loop skips the write. |
| Anthropic `usage` is `{}` (empty object) | `parse_anthropic_usage` returns `None`. |
| OpenAI request body missing `stream_options` | The OpenAI server omits `usage` on all chunks. `parse_openai_usage` returns `None` for every chunk. Agent loop skips the write. (The `build_http_body_includes_stream_options_for_usage` test asserts the field is always present.) |
| OpenAI `usage` chunk has `prompt_tokens_details: {}` | Defensive path: `parse_openai_usage` reads `cached_tokens` as missing → 0. The other three fields parse normally. |
| OpenAI `usage` is all-zero | `parse_openai_usage` returns `None`. Same deliberate contract as Anthropic. |
| Cancel mid-stream (user hits Stop) | `ChatEvent::Done { usage: None, stop_reason: "cancelled" }`. Agent loop skips the write, `tracing::info!` records the skip. |
| Network error mid-stream | `ChatEvent::Error { category: Network }`. The agent loop's `if had_error { return }` short-circuits before any `Done` is processed — the `usage` write is naturally skipped. |
| `update_last_turn_usage` on missing session id | `UPDATE` matches 0 rows. `sqlx::Error` is not raised. The write is a silent no-op (0 rows changed). |
| `update_last_turn_usage` overwrites an existing snapshot | 5 个 `last_*` 列直接 `= ?`（非 `+= ?`、非 `COALESCE`）。每次 `Done` 覆盖写，读回即最近一次请求的值。pre-snapshot session 列为 NULL，首次写后变 `Some(value)`；后续每次写覆盖前值。 |
| Session switch mid-stream (user views a different session) | The stream keeps running on the controller's `request_id`; the `done` event routes by `request_id` to the originating session, updates `tokenUsageBySession` for that session (not the user's current view). When the user returns to the streamed session, the `currentSessionTokenUsage` computed re-evaluates and shows the updated total. |
| Page reload after N turns | `list_sessions` returns `SessionSummary` with `input_tokens_total` etc. (not NULL). `onProjectChange` seeds the in-memory Map. The hint area shows the cumulative value on first paint. |
| Pre-A4 session (columns NULL) | UI renders "—" with the "升级前未统计" tooltip. The first post-upgrade turn starts the counters from 0. |

### 5. Good / Base / Bad Cases

#### Good: Anthropic happy path

1. User opens a session, sends a question, hits Send.
2. `chat` command resolves the catalog (Anthropic + claude-sonnet-4-5), builds `AnthropicProvider`.
3. `AnthropicProvider::send` streams:
 - `message_start { ... usage: { input_tokens: 0, output_tokens: 0, ... } }` (initial baseline; the `usage` local in the adapter is set to `Some(TokenUsage { 0,0,0,0 })` but the all-zero case is mapped to `None` first... actually re-read: the baseline sets `usage` only if it's currently `None`, but the all-zero check is in the inner `parse_anthropic_usage`. Net: a baseline `usage: { 0,0,0,0 }` returns `None`, so the `usage` local stays `None` and the next `message_delta` overwrites it. See `parse_anthropic_usage_zero_returns_none` test.)
 - `content_block_start` / `delta`s (text + tool_use + thinking).
 - `message_delta { delta: { stop_reason: "end_turn" }, usage: { input_tokens: 1234, output_tokens: 56, cache_creation_input_tokens: 100, cache_read_input_tokens: 200 } }` — the `usage` local is overwritten with the non-zero value.
 - `message_stop`.
4. The stream yields `ChatEvent::Done { stop_reason: Some("end_turn"), usage: Some(TokenUsage { input_tokens: 1234, output_tokens: 56, cache_creation_input_tokens: 100, cache_read_input_tokens: 200, context_input_tokens: 1534 }) }` — `context_input_tokens` = input + cache_creation + cache_read = 1234 + 100 + 200 = 1534，是前端 `%` 的分子。
5. The agent loop's `if let Some(t) = usage { update_last_turn_usage(...) }`（在 `!skip_persist` gate 内，worker 路径被挡）runs the SQL UPDATE — 5 个 `last_*` 列覆盖写。
6. The frontend's `streamController.handleChatEvent("done")` sees the `usage` field and calls `useChatStore().setLastTurnUsage(sid, t)`（覆盖写 `tokenUsageBySession.set(sid, { ...usage })`，非 `+=`）。The `tokenUsageBySession` map updates; `currentSessionTokenUsage` re-evaluates.
7. The ChatInput hint area re-renders: `1.2K · 1% / 200K` (assuming 1234 tokens is ~0.6% of 200K context_window). The color is `ok` (green).

#### Good: OpenAI happy path

1. Same flow, but the `chat` command's `resolve_chat_provider` returns an `OpenAIProvider` (the user has switched the default to a gpt-4o model).
2. The OpenAI adapter's `build_http_body` includes `"stream_options": { "include_usage": true }`.
3. The SSE stream emits normal text deltas, then a final chunk with `usage: { prompt_tokens: 200, completion_tokens: 30, total_tokens: 230, prompt_tokens_details: { cached_tokens: 50 } }` and no `choices`.
4. The adapter's `parse_openai_usage` normalizes: `input_tokens: 200, output_tokens: 30, cache_read_input_tokens: 50, cache_creation_input_tokens: 0, context_input_tokens: 200`（= `prompt_tokens`，前端 `%` 分子；OpenAI 勿再加 `cache_read`）。
5. The agent loop + frontend flow identically to the Anthropic path.

#### Base: cancel mid-stream

1. User sends a question; LLM starts streaming.
2. User hits Stop. The cancellation token fires; the agent loop's `tokio::select!` notices on the next event boundary.
3. The agent loop bails out, persists whatever's been collected so far, and yields `ChatEvent::Done { stop_reason: Some("cancelled"), usage: None }`.
4. The frontend's `done` handler sees `usage` is undefined; `setLastTurnUsage` is not called.
5. The SQL write is skipped. Snapshot semantics: the `last_*` columns keep the **previous successful turn's** snapshot (cancel does not overwrite). The dashboard still shows the last completed request's context occupancy — cancel mid-stream does not zero it out.

#### Bad: OpenAI stream without `include_usage`

1. (Anti-pattern, NOT the implementation) The `build_http_body` does not set `stream_options.include_usage: true`.
2. OpenAI omits the `usage` field on every chunk.
3. `parse_openai_usage` returns `None` for every chunk. The adapter's `usage` local stays `None`.
4. The agent loop's `if let Some(t) = usage` is never true. No SQL write happens.
5. The user opens the app, sends a message, gets a response, and the ChatInput hint shows "—" (or the pre-A4 value). They report "token usage tracking doesn't work for OpenAI".

**Fix**: `build_http_body` must always set
`"stream_options": { "include_usage": true }`. The
`build_http_body_includes_stream_options_for_usage` test asserts
this is the case.

#### Bad: persistent strip of token usage

1. (Anti-pattern, NOT the implementation) The agent loop "saves tokens" by writing only `output_tokens` to the DB and discarding the other three.
2. The UI shows only output. The user has no visibility into the cache hit rate, the context pressure, or the cumulative input growth.
3. **Fix (A4 doesn't do this)**: the 4 columns are persisted verbatim. Future PRs (B6 subagent token quotas, A5 $ cost conversion) can read any of the four fields independently.

### 6. Tests Required

#### Backend (cargo test)

**`llm::types` (5 new tests)**

- `token_usage_serializes_with_snake_case_fields`
- `token_usage_default_is_all_zero`
- `token_usage_deserializes_legacy_4_field_json_with_default_context`（snapshot 重构新增：旧 4 字段 JSON 反序列化时 `context_input_tokens` 走 `#[serde(default)]` = 0）
- `chat_event_done_carries_usage_payload`
- `chat_event_done_with_none_usage_emits_null`

> 2026-06-26 snapshot 重构后，旧 `token_usage_add_assign_saturates_at_u32_max`（依赖 `impl Add for TokenUsage`）随累加语义一起删除；`Add` impl 已不存在。

**`llm::provider::anthropic` (4 new tests)**

- `parse_anthropic_usage_full_payload`
- `parse_anthropic_usage_minimal_payload`
- `parse_anthropic_usage_zero_returns_none`
- `parse_anthropic_usage_empty_object_returns_none`

**`llm::provider::openai` (6 new tests)**

- `build_http_body_includes_stream_options_for_usage`
- `parse_openai_usage_full_payload`
- `parse_openai_usage_minimal_payload`
- `parse_openai_usage_no_usage_key_returns_none`
- `parse_openai_usage_zero_returns_none`
- `parse_openai_usage_empty_prompt_tokens_details`

**`db::sessions` (2 snapshot tests, in `db::tests`)** — 2026-06-26 snapshot 重构后，旧 `add_token_usage_*` 累加测试已删，换为覆盖写不变量测试：

- `update_last_turn_usage_overwrites_not_accumulates` — 连续两次调用后，行保留**第二次**的值（非两者之和），锁定 snapshot 覆盖写语义。
- `list_sessions_includes_last_turn_columns` — `SessionSummary`（侧边栏列表）能读到 5 个 `last_*` 列，无需 per-session IPC。

Total token-usage cargo tests: **17**（types 5 + anthropic 4 + openai 6 + db::sessions 2 snapshot）。2026-06-26 snapshot 重构后 db::sessions 从 4 个累加测试降为 2 个覆盖写测试（净 -2）；types 段实际 5 个（旧文本写 "4" 但列了 5 个，本次订正）。

#### Frontend

- `pnpm build` (vue-tsc strict) must pass.
- **Known test gap (unguarded contract)**: `streamController.handleChatEvent("done")` 对 `context_input_tokens` 的 wire-optional fallback（`?? input + cache_creation + cache_read`，`streamController.ts:999-1003`）目前**无前端单测覆盖**（`**/*.test.ts` 无相关断言）。旧后端 wire shape 或字段缺失时该 fallback 是 load-bearing 的，后续应补一个 fallback 正确性测试。
- Manual smoke test (acceptance A2 from the parent PRD):
 1. `cd app && pnpm tauri dev`
 2. Open a session, send a question, click Send.
 3. Observe the ChatInput hint area shows "X · Y% / 200K" (e.g. "1.2K · 1% / 200K"), green color (under 50%).
 4. After 4-5 turns, observe the percentage climbs. Watch the color shift to yellow at 50%, red at 75%.
 5. Hover the chip, observe the tooltip shows the four counters (input / cache_read / cache_creation / output).
 6. Open Settings, delete the model's `api_key` (or the model entirely). Send a message — observe the pre-flight error and the hint stays at the previous snapshot (the agent loop never reached `update_last_turn_usage`).
 7. Page reload. Observe the hint area still shows the last-turn snapshot (seeded from `list_sessions` `last_*` columns; pre-snapshot session 显「—」)。

### 7. Wrong vs Correct

#### Wrong: snake_case in the inner `usage` object, but camelCase outer

```rust
// BAD — mixed naming convention
#[derive(Serialize)]
#[serde(rename_all = "camelCase")] // applied at the struct level
pub struct TokenUsagePayload {
 pub input_tokens: u32, // → "inputTokens" on the wire
 pub output_tokens: u32, // → "outputTokens"
 // ...
}

// Resulting IPC payload:
{
 "kind": "done",
 "stopReason": "end_turn",
 "usage": { "inputTokens": 1234, "outputTokens": 56, ... }
}
```

The frontend's TypeScript interface then has to mix `stopReason` (camelCase
outer) with `inputTokens` (camelCase inner usage) — a 2-style
"polyglot" payload that's hard to grep, hard to map to Anthropic
or OpenAI field names, and breaks the user's mental model of
"snake_case from the LLM, snake_case on the wire".

#### Correct: snake_case throughout the inner object

```rust
// GOOD — match the rest of the wire payload's snake_case
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenUsage {
 pub input_tokens: u32,
 pub output_tokens: u32,
 pub cache_creation_input_tokens: u32,
 pub cache_read_input_tokens: u32,
 #[serde(default)]
 pub context_input_tokens: u32, // 2026-06-26 第 5 字段（前端 % 分子）
}

// Resulting IPC payload (snake_case throughout — both the outer
// ChatEventPayload fields and the inner TokenUsage object;
// ChatEventPayload has NO `rename_all`, ChatEvent is
// `rename_all = "snake_case"`):
{
 "kind": "done",
 "stop_reason": "end_turn", // ChatEvent snake_case
 "usage": {
 "input_tokens": 1234, // TokenUsage snake_case
 "output_tokens": 56,
 "cache_creation_input_tokens": 100,
 "cache_read_input_tokens": 200,
 "context_input_tokens": 1534
 }
}
```

The frontend reads `event.usage.input_tokens` (matching the Rust
struct's field name verbatim). Anthropic / OpenAI API
documentation (which uses snake_case for the same fields) reads
1:1 with the IPC payload, making cross-referencing easy.

#### Wrong: agent loop branches on protocol

```rust
// BAD — agent loop checks Anthropic vs OpenAI to read the
// usage field
match event {
 ChatEvent::Done { stop_reason, usage } => {
 // ... accumulates ...
 // But what if some future protocol's `Done` carries a
 // different shape? The agent loop would need a new match arm.
 }
}
```

The Provider abstraction is leaky if the agent loop has to know
which protocol emitted the event.

#### Correct: provider-normalized payload

```rust
// GOOD — provider adapter normalizes; agent loop sees
// protocol-agnostic 4 fields
// In AnthropicProvider::parse_anthropic_usage:
let u = TokenUsage {
 input_tokens: v.get("input_tokens")...as u32,
 output_tokens: v.get("output_tokens")...as u32,
 cache_creation_input_tokens: v.get("cache_creation_input_tokens")...as u32,
 cache_read_input_tokens: v.get("cache_read_input_tokens")...as u32,
};
// In OpenAIProvider::parse_openai_usage:
let u = TokenUsage {
 input_tokens: prompt,
 output_tokens: completion,
 cache_creation_input_tokens: 0, // no OpenAI equivalent
 cache_read_input_tokens: cached,
};
// Both yield:
// ChatEvent::Done { stop_reason, usage: Some(TokenUsage { 5 fields }) }
// （第 5 字段 context_input_tokens：Anthropic = input+cc+cr，
//  OpenAI = prompt_tokens；agent loop is protocol-agnostic。）
```

Future protocols (Gemini, Ollama) plug in by writing their own
`parse_<protocol>_usage` function. The agent loop and the DB
schema don't change.

#### Wrong: persist the cumulative usage in the message row

```rust
// BAD — every assistant turn stores its own usage in the
// messages table; per-session total becomes a SUM() query on
// every read
INSERT INTO messages (..., input_tokens, output_tokens, ...)
```

This requires a `messages` schema change (out of scope per the
PRD's "排除" list), and the per-turn granularity is overkill
for the A4 hint (the PRD explicitly defers per-turn to "后续
C3 / B6 阶段").

#### Correct: per-session snapshot on the sessions row（2026-06-26 重构后）

```rust
// GOOD — single SQL UPDATE on the sessions row, OVERWRITES the
// 5 `last_*` snapshot columns（= ?，非 += ? / 非 COALESCE）。
// 语义 = 最近一次 LLM 请求的 context 占用快照。
UPDATE sessions
 SET last_context_input_tokens = ?,
 last_input_tokens = ?,
 last_output_tokens = ?,
 last_cache_creation = ?,
 last_cache_read = ?,
 updated_at = ?
 WHERE id = ?
```

The hint area reads the last-turn snapshot with no aggregation.
Future C3 / B6 work can ALTER `messages` to add per-turn columns
without changing the snapshot schema.（旧 A4 累加式 `*_total` 列仍
保留在 schema 中但代码不再写入 — 见 §2 DB schema "frozen" 注；
清理它们是后续 debt PR 的事。）

### Design Decisions

> **⚠ 2026-06-26 snapshot 注记**：以下 Decision 段保留 A4 原始（cumulative 模型）措辞，反映「token usage 走 provider 私有解析 + DB 为 source of truth + frontend Map 投影」的架构决策——这些**架构决策在 snapshot 重构后依然成立**。仅 DB 写入语义从「累加 `*_total`」变为「覆盖 `last_*` 快照」（见 §2 / §3），frontend Map 从「incremented」变为「overwritten」（见 §3 Frontend snapshot）。读时请把下文的 cumulative / accumulate / running total 心智替换为 snapshot 语义。

#### Decision: Anthropic also goes through the wire layer's usage normalization

**Context**: The A4 scope is "per-session token accumulation +
ChatInput hint". The cross-protocol question is: does
Anthropic's `message_delta.usage` parsing live in the
`AnthropicProvider` (and the agent loop) or in a shared
helper?

**Decision**: Provider-private parsing. `parse_anthropic_usage`
is a private free function in `provider/anthropic.rs`;
`parse_openai_usage` is private in `provider/openai.rs`. The
wire layer (`provider/wire.rs`) is unchanged — usage is not a
block-level concept, it's a stream-level one, and the wire
layer deals with `ContentBlock` round-trips not SSE-side
metadata.

**Consequences**:
- ✅ Anthropic and OpenAI each handle their own protocol's
 quirks (Anthropic's `message_delta` vs OpenAI's
 `data-only` SSE; Anthropic's `cache_*` fields vs OpenAI's
 `prompt_tokens_details.cached_tokens`).
- ✅ The wire layer is not contaminated with token-usage
 types — it stays focused on block-level cross-protocol
 conversion.
- ⚠️ A future "Gemini usage" function would be a 3rd private
 helper in `provider/gemini.rs`. The pattern is clear, the
 cost is one duplicated `Option`-handling decision per
 protocol.

#### Decision: in-memory accumulation on the frontend, not in the DB schema

**Context**: The frontend (Pinia store) and the backend (SQLite)
both need to know the cumulative per-session totals. Where does
the running total live?

**Decision**: The DB stores the per-session cumulative
(`sessions.*_total` columns, updated on every `Done` event). The
frontend also has a `tokenUsageBySession` Map that's seeded
from `list_sessions` and incremented on every `done` event with
a `usage` payload. The DB is the source of truth; the frontend
Map is a projection of the DB for live updates.

**Consequences**:
- ✅ A page reload shows the right number (seeded from DB).
- ✅ No need for a separate `token-usage` IPC command (the
 chat command's `done` event carries everything the
 frontend needs for live updates; the SessionSummary carries
 the historical total).
- ✅ The frontend can show a per-session usage chip in the
 sidebar (not in this PRD's scope, but the data is there
 in `SessionSummary` for a future PR to wire up).
- ⚠️ A reload mid-stream would show the pre-stream value
 (the in-flight `done` event hasn't fired yet). This is
 acceptable — the streaming session is ephemeral.

#### Decision: percentage denominator is the default model's `context_window`, not the session's `model_id`

**Context**: A session can override its model via
`sessions.model_id` (per-session model override). The model's
`context_window` varies (Sonnet 200K, Haiku 200K, GPT-4o
128K). The percentage denominator should match the model that's
actually being called.

**Decision**: The percentage uses
`modelsStore.defaultModel.contextWindow`. Reasoning:
- The chat command's `resolve_chat_provider` already
 resolves a session override to a specific model; the
 frontend doesn't easily track "which model this session
 last used" without another IPC.
- The visual stability is better (the denominator doesn't
 flicker when the user opens a session whose last-used
 model had a different window than the current default).
- The PRD explicitly scopes "current context usage, not
 cumulative" — the *current* default's window is the most
 useful visual baseline.

**Consequences**:
- ✅ The percentage always shows "X% / 200K" (or whatever
 the current default is) — stable across sessions.
- ⚠️ A session mid-flight on a per-session override with a
 smaller context_window (e.g. 128K) would show a
 *under*-estimated percentage. Future PR can thread the
 effective model's window through the IPC if this becomes
 a real problem.