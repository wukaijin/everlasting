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
- **Known test gap (unguarded contract)**: `streamController.handleChatEvent("done")` 对 `context_input_tokens` 的 wire-optional fallback（`?? input + cache_creation + cache_read`，`streamEvents.ts`(08-07 拆分)）目前**无前端单测覆盖**（`**/*.test.ts` 无相关断言）。旧后端 wire shape 或字段缺失时该 fallback 是 load-bearing 的，后续应补一个 fallback 正确性测试。
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

## Scenario: Group-Chat Per-Speaker Cache Rate (2026-08-10, task 08-10-group-chat-cache-rate)

### 1. Scope / Trigger

- Trigger: 群聊会话中每个参与者(含主持人)展示各自**最近一次 LLM 调用**的缓存率。
- 为什么不能读 `sessions.last_*`:群聊每轮(主持人/参与者)都覆盖写同一 session 的 `last_*` 快照,最后一位发言者胜出——**group chat 的 `last_*` 是"最后一位发言者"的值,不是聚合也分不出说话人**。per-speaker 数据只能走 `turn_trace`。

### 2. 数据模式:turn_trace × messages.speaker join(关键)

`turn_trace(session_id, seq, token_usage_json)` 行**没有 speaker 列**,但群聊里 `messages` 的 assistant 行带 `speaker`(主持人 = `"moderator"`,参与者 = `participant.name`),且 **assistant 行 seq == 该轮 turn 的 seq**(`chat_loop/drive.rs` push 时用当前 seq 后 `seq += 1`)。seq 群聊内全局连续(`chat_loop/init.rs` 每次调用从 DB max(seq)+1 起)。因此:

```sql
SELECT m.speaker,
       COALESCE(json_extract(t.token_usage_json, '$.cache_read_input_tokens'), 0) AS cache_read,
       COALESCE(json_extract(t.token_usage_json, '$.context_input_tokens'), 0)   AS context_input
FROM messages m
JOIN turn_trace t ON t.session_id = m.session_id AND t.seq = m.seq
WHERE m.session_id = ?1
  AND m.role = 'assistant'
  AND m.speaker IS NOT NULL          -- 普通聊天/worker 行 speaker 为 NULL,天然排除
  AND t.token_usage_json IS NOT NULL -- 该轮无 usage(取消/出错/只写了其他维度)
  AND m.seq = (                       -- 每 speaker 最近一次发言轮
      SELECT MAX(m2.seq) FROM messages m2
      WHERE m2.session_id = m.session_id AND m2.speaker = m.speaker
        AND m2.role = 'assistant'
  )
```

- rewrite 产物(user role 带 speaker)被 `role='assistant'` 过滤。
- 同一轮重试:`turn_trace` 按 (session, seq) 覆盖(`trace.rs` upsert),天然取最后一次 usage。
- **不回退语义**(locked):某 speaker 最新一轮无 usage(如取消)→ 该 speaker 整行不返回,前端显示 "—";**不**回退到更早的有 usage 轮次。理由:缓存率 = "最近一次调用"的单次语义,最近一次调用没有 usage 就没有可算的数。

### 3. 缓存率口径(单次)

- `cache_rate = cache_read_input_tokens / context_input_tokens`,单次调用语义,非多轮聚合。
- 分母**必须**用 `context_input_tokens`(跨 provider 归一化总输入),不能用 `input_tokens`:Anthropic 的 `input_tokens` 不含 cache read/creation,OpenAI 的 `prompt_tokens` 已含 cached——用 `input_tokens` 会让两 provider 的命中率口径不一致。
- `context_input <= 0`(legacy 4 字段行,`#[serde(default)]` 补 0)→ 前端显示 "—",不在 SQL 过滤(保留数据,前端决定展示)。

### 4. 契约

```rust
// db/trace.rs
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpeakerCacheUsage {
    pub speaker: String,     // "moderator" 或 participant.name
    pub cache_read: u32,     // 该 speaker 最近一次有 usage 轮次的 cache_read_input_tokens
    pub context_input: u32,  // 同上轮的 context_input_tokens
}
pub async fn list_speaker_cache_usage(pool: &SqlitePool, session_id: &str)
    -> Result<Vec<SpeakerCacheUsage>, sqlx::Error>
```

IPC: `group_chat_cache_rates(sessionId)` → `Vec<SpeakerCacheUsage>`,三处注册(tauri invoke_handler + daemon route + `http.ts` CMD_TO_DOMAIN)。

### 5. 边界 / 测试要点

| 边界 | 行为 |
|------|------|
| `clear_session_trace` 清空 turn_trace | 查询返回空 → 全部 "—"(与回看功能共用数据,预期) |
| speaker 最新轮无 usage | 整行不返回(不回退,见 §2) |
| legacy 4 字段 JSON 缺 context_input | `COALESCE(json_extract(...), 0)` → 0 → 前端 "—" |
| 兼容代理 cache 字段全 0 | 缓存率 0%(真实数据,非错误) |
| 主持人 | speaker 固定 `"moderator"`(`group_chat_loop.rs` emit 值),model = `sessions.model_id`(前端 `SessionSummary` 已有) |

测试要点:每 speaker 只返回 max seq 轮的数字、无 usage 轮被跳过、最新轮无 usage 不回退(用"seq 9 有 usage + seq 10 无 usage"的 fixture 锁定,否则测试在两种 SQL 解释下都通过)、user/speaker-NULL 行排除。前端百分比计算是纯函数(放 `utils/tokenUsage.ts` 或同类),`context_input <= 0 → null` 可单测。

## Scenario: tools[] Token Measurement + Static Pruning (C7, 2026-08-14)

> 配套 task `08-14-c7-tools-token-governance`。把 `tools[]` 数组当作与
> messages 并列的**上下文治理对象**:R1 量(MVP)、R3 静态裁剪(MVP)、
> R2 Anthropic cache 断点(Phase 2)、D Stub 注册(Phase 2)。本 scenario
> 锁 MVP 两路径的可执行契约 + cache 率口径红线。

### 1. Scope / Trigger

- Trigger:多 turn 任务里 `tools[]` 每 turn 全量拼装下发(~7-8k tok/轮,
  单对话首回合 context 的大头 —— 实测 session 50b91178 一句"早上好"
  input=12838),挤占有效历史、提前触发 C3 压缩。C7 把它从"0 分的请求
  前缀段"变成可量化 + 可裁剪的治理对象。
- 为什么 code-spec depth:migration(新列)+ 跨层契约(tools_token 从
  Rust 估算 → SQLite → Pinia → TracePanel,且与 cache 率口径交织)→
  一处口径错(double-count)就污染所有占比展示。

### 2. DB Schema(`turn_trace.tools_token`)

```sql
-- CREATE TABLE 段(greenfield 已含)+ 幂等 ALTER(existing)。nullable:
-- NULL = pre-column 行 / worker(skip_persist)轮 / 估算被跳过。
ALTER TABLE turn_trace ADD COLUMN tools_token INTEGER;
```

migration helper:`add_turn_trace_column_if_missing(pool, "tools_token",
"INTEGER")`(`db/migrations/columns.rs`,镜像 `add_session_audit_events
_column_if_missing` 的 PRAGMA-table_info probe 模式)。

### 3. Signatures

```rust
// db/trace.rs —— tools_token 与 token_usage_json 同 upsert 写入
//   (都源自 Done-event 写点;UNIQUE(session_id,seq) 冲突时两列一起重写)
pub async fn upsert_turn_trace_token(
    pool: &SqlitePool,
    session_id: &str,
    seq: i64,
    usage: &TokenUsage,
    tools_token: Option<u32>,   // C7: cl100k of serialized tools[] JSON
) -> Result<(), sqlx::Error>

#[serde(rename_all = "camelCase")]  // → wire "toolsToken"
pub struct TurnTraceRow { /* ... */ pub tools_token: Option<i64> }

// tools/mod.rs —— R3 静态裁剪(provider 之前的 schema 层)
pub fn filter_tools_for_session_type(
    tools: Vec<ToolDef>,
    is_group_chat: bool,
) -> Vec<ToolDef>   // 非 group_chat 砍 nominate_speaker + end_discussion
```

估算点:`agent/chat_loop/drive.rs` 在 `turn_tool_defs` freeze 后(完整
过滤链 mode→workflow→session_type + dispatch_subagent append 之后)、move
进 `retry_open` 之前,`serde_json::to_string(&turn_tool_defs)` →
`memory::tokens::count_tokens`(cl100k,`tokio::sync::Mutex` 守护,µs–low-ms,
inline 安全)。best-effort:序列化失败 → 空串 → 0,不阻塞 turn。

### 4. Contracts — cache 率口径(关键,勿 double-count)

`context_input_tokens` **已含 tools**(provider 侧进入 context window 的
全部 prompt token;见 §2 Anthropic=input+cc+cr / OpenAI=prompt_tokens)。
`tools_token` 是对其中 tools[] 这一**切片**的单独估算 —— 它是
`context_input` 的**子集**,不是额外项。

- TracePanel tools 占比公式 = `tools_token / context_input_tokens`。
- ⚠️ **禁止** `tools_token / (context_input_tokens + tools_token)`
  —— context_input 已含 tools,加回是 double-count,系统性压低占比。
- ⚠️ **禁止**把 tools_token 加进 cache 率分母(`cache_read /
  context_input`)—— cache 率现状已被 tools 稀释偏低(tools 无 cache 断点;
  R2 Phase 2 后 cache_read 才含 tools);tools_token 只**单列**展示,
  不混入 cache 率分子或分母。
- wire:`tools_token` 经 `TurnTraceRow`(camelCase)→ 前端
  `TurnTraceRow.toolsToken` → `TurnTrace.toolsToken?`(undefined =
  pre-column / live 路径未带)。`list_turn_traces` IPC 自动透传,无需改
  IPC handler。live 路径(无 reload)tools_token 暂为 undefined(design
  决定:不为此加 ChatEvent 字段),reload 后(回看)落盘值出现。

### 5. R3 静态裁剪(跨层影响 + 先例)

过滤链 `drive.rs:504`:`filter_tools_for_session_type(filter_tools_for_
workflow(filter_tools_for_mode(...)))`。三环都是纯 `Vec<ToolDef>` 集合
减法,顺序无关。

- **非 group_chat 砍 `nominate_speaker` + `end_discussion`**(落实
  `tools/mod.rs:224` 的 "Phase 4 may filter" 注释)。省 ~465 tok/轮
  (<7%);大头(`use_ui`/`ask_user_question`/`remember`/`shell`)通用,
  静态裁不动 —— 省 window 的大动作是 D Stub(Phase 2)。R3 的 MVP 价值
  主要是"卫生"(非群聊不暴露无意义工具),不是省 token。
- **group_chat 是 no-op**:群聊走 `group_chat_tool_defs` 白名单(`group_
  chat_prompts.rs`,主持人含仲裁工具、参与者不含),本过滤器不二次干预。
- **cache 稳定性(prd R3.2)**:同一 session 的 `session_type` 固定 →
  裁剪结果跨连续 turn 稳定 → OpenAI 自动前缀缓存 / 未来 R2 断点在一个
  mode 段内命中。
- `session_type` 从 `loaded_session.session.session_type` 读(零成本,
  无 DB round-trip)。chat_loop 对 nominate/end 的运行时 no-op 拦截(按
  tool_name)不依赖 tools[] 注册,裁掉工具注册不影响该拦截。

### 6. Tests Required(断言点)

- `tools_token_defaults_null_for_legacy_or_worker_rows` —— raw SQL 写入
  (无 tools_token 列)读回 `None`;再 upsert 补 `Some(425)` 不丢
  token_usage_json。
- `upsert_overwrites_same_column_on_conflict` —— 两次 upsert 传
  `Some(111)` / `Some(222)`,断言最终 `Some(222)`(冲突时 tools_token 与
  token_usage_json 一起重写,锁定 C7 upsert 契约)。
- `upsert_accumulates_columns_across_writes` —— 传 `Some(7000)`,断言
  `rows[0].tools_token == Some(7000)`。
- R3:`tools::tests_session_type_filter` —— classic_chat 裁两工具、
  group_chat no-op。
- 前端:`TurnCard` 渲染 `tools 7K` cell + title 含 `70%`
  (`toolsToken=7000, context_input=10000`);toolsToken 缺失时不渲染。
- 回归:`filter_tools_for_mode` * + `group_chat_tool_defs` * 不变。

### 7. Wrong vs Correct

#### Wrong:把 tools_token 加进分母(double-count)

```ts
// BAD —— context_input 已含 tools,加回是重复计算
const toolsPct = toolsToken / (contextInput + toolsToken);
// 结果系统性偏低(7000 / (10000+7000) ≈ 41% 而非真实 70%)
```

#### Correct:tools_token 是 context_input 的切片

```ts
// GOOD —— tools_token ÷ context_input(子集 / 全集)
const toolsPct = contextInput > 0 ? toolsToken / contextInput : null;
// 7000 / 10000 = 70%(与"tools[] 占本轮 context 七成"的直觉一致)
```

## 不做(Phase 2 / OOS,见 task prd.md)

- **R2 Anthropic tools cache 断点**:实测 session 50b91178(wukaijin
  relay)吃 `cache_control` 不 400 但 `cache_creation=0` → relay 静默忽略
  → 零收益;原生 Claude 未测(无 provider)。设计保留在 task design §R2,
  等配原生 Anthropic provider 后重启。
- **D Stub 注册**:触发 = R1 度量数据显示 tools[] 占 context 窗口 >15%。
  ✅ **2026-08-14 已落地**(task `08-14-c7d-tools-stub-registration`,
  见下方 Scenario:tools Stub 注册(D))。
- **memory 指令块治理**:记 `docs/BACKLOG.md`。

## Scenario:tools Stub 注册(D,2026-08-14)

> 配套 task `08-14-c7d-tools-stub-registration`(C7 Phase 2 之 D,触发线
> 已过:首轮 tools 占 context 38.5% > 15%)。完整契约(tool 形态 /
> `load_tool_schemas` 拦截 / 粘性 registry / 开关 / 红线)见
> [tool-contract/14-stub-registration.md](./tool-contract/14-stub-registration.md)。
> 本文只记与本 scenario(C7 度量)的交点 + 落地验证数据。

### 与 C7 度量的交点

- `turn_trace.tools_token` 度量链路**不改**:stubify 是 drive.rs 过滤链
  第 4 环(mode→workflow→session_type 之后、dispatch append 之后),
  `tools_token` 估算点在完整过滤链 + 两个 append 之后 — stub 后
  `tools[]` 体积自然缩小,tools_token 如实反映,AC1 用它验证。
- tools_token 占比如实变小是预期(前端 TracePanel 无改动)。

### 验证数据(2026-08-14)

- 基线(前):tools_token=6773 / context_input=17602 = 38.5%。
- 目标:开关开、经典 chat、首轮无 load 调用,tools_token ≤ 3700(2026-08-14 用户拍板;实测 3677,基线 6773 → 3677,省 3096,-45.7%)。
- 回滚通道:app_config `tools_stub_enabled = "false"` → 第 4 环直通 +
  不 append `load_tool_schemas`,tools_token 回 ~6773。

### 预算校准(静态度量单测,用户拍板)

静态线 ≤3700(= AC1,原设计 ≤3000 在 Edit 模式下数学不可达:核心 9
工具全量 2261 + dispatch_subagent 真实 def 984(生产 5 模型 enum,实测;
原预估 ~500 低估近半)= 3245,零 stub 已超 3000)。stub 描述走「极短
摘要 + load 指引」方案(10 个含 JSON 包装 330),Edit 合计 3675、live
实测 3677 — AC1 线随用户拍板定 3700。
## Scenario:memory 指令块度量 + digest(WP1/WP2,2026-08-15)

> 来源:`08-15-memory-block-governance`(BACKLOG §3.1)。与 tools_token
> 完全同构的「切片单列」口径;digest 是内容侧手段,不改度量链路。

### memory_token 口径(与 tools_token 对称,逐条对齐)

- `turn_trace.memory_token`:cl100k 估算**实际注入的** memory 指令块
  (banner + wrappers + 层 body;digest 开启时即 digest 后体积 — WP1 的
  计算点在 init.rs 注入处,`LoopInit` 穿到 drive.rs Done 写点)。
- **per-request 常量**:memory 块每 request 组装一次,同一 request 的
  所有 turn 行同值(区别于 tools_token 每 turn 重估 — dispatch enum 等
  可能变)。
- 占比 = `memory_token / context_input_tokens`,**不 double-count**
  (context_input 已含 memory)。前端 `TurnCard.memoryPct` 同 toolsPct。
- `None` 语义:pre-column 行 + worker turn(worker 注入走
  `subagent/prompt.rs`,不在度量面 — design §3.5a)。
- upsert 契约:与 tools_token 同一 `upsert_turn_trace_token` 写点、同
  second-writer-wins(冲突双写)。

### 验证数据(2026-08-15,本仓库 live)

- 基线(digest off):memory_token=10124 / ctx=14079(72%)— 高于
  08-14 估算 ~7-8k,cl100k 对 CJK 实际计数 + wrapper 开销。
- digest on:memory_token=2080(28%),-79.5%;首轮 ctx -47%;tools
  3664→3805(Δ141 = load_memory_sections def,净收益仍 -6.6k)。
- 双轮 cache 率:on 99.8% vs off 99.7% — 不劣化(AC4)。
- turn-smoke `--turns N`:AC4 类双轮对比的标准入口(08-15 加)。

## Scenario: images_token — request-total image slice (B1, 2026-08-17)

`turn_trace.images_token`(PR4,B1 `08-16-b1-image-multimodal`)与 tools_token / memory_token 同语义的第三切片:**请求内全部图片块的 token 估算**,含历史重建(历史图每轮随请求重发、每请求计费——只算当轮新图会系统性低估,评审 P0-1)。

- **口径**:`estimate_images_token(&turn_messages)` 在 `drive.rs` 的 per-turn 请求 clone 上、resolve 之后计算——Σ 每图 `tokens_est`(attach 时 `(w×h)/750`,前端 FileReader 读粘贴图、后端 `imagesize` crate 读 @图文件头),缺失回退 1600/图垫板。写入点与 tools/memory 同一 Done upsert(`!skip_persist` gate,worker 轮 None)。
- **When this bites**:估算是"字段优先"——`attachments` 字段的精确值**替换**垫板贡献而非叠加;若某消息的 Image 块数与 attachments 数不一致(理论上 attach pass 保证 1:1),会以字段为准。live 实测(08-17):800×600 png → 640 tok 精确落值,无图轮 = 0。
- TurnCard `img` cell 门是 `> 0`(tools/mem 是 `!= null`)——无图轮不渲染噪声 cell。

## Scenario: 摘要压缩旁路 usage(C3,2026-08-18)

- **口径**:LLM 摘要调用是**旁路 completion**(无 tools、禁 thinking 采集、
  4k 输出兜底)—— 其 `TokenUsage` **不混入**主 turn 的
  `update_last_turn_usage`(`context_input`/per-turn 记账口径不变),只进
  `compaction_json.summary_usage`(trace.rs 手工 json!,与 method 同写点)。
- **When this bites**:主 turn 的 token 统计永远不包含压缩开销 —— 想算
  真实成本要看 compaction_json;TracePanel 的 TurnCard token 字段因此
  不因压缩而跳变(展示的是请求上下文,不是总消耗)。
- 摘要求输入 = 模板 + prior-summary + transcript(预算 0.7×window,溢出
  丢最旧 + `[older transcript omitted]` 记号),输出 `clamp_summary_output`
  4k token 兜底。

## Scenario: 统一估算 + at_files/system/window 三新列 + 实发口径(unified-context-budget,2026-08-19)

`turn_trace` 新增 `at_files_token`(全部 user message 的 @-token 注入正文 est 之和;@图走 images_token 不重复计)/ `system_token`(system prompt 本体 + skill listing 归因)/ `context_window`(请求时窗口快照,TurnCard 预算行分母,旧行 NULL 前端回退 200_000)。写点与既有切片同一 Done upsert(`!skip_persist` gate,worker 轮 None;零注入 → at_files NULL)。

- **两类口径永不互相加计**(任务 prd D8,评审 F1 教训):总量 =
  `budget::estimate_request_tokens(system, tools_json, messages)` 三部件
  加法 —— memory 头对/skill listing/@文件/图片物理在 messages 里,公式
  上再单独加计任何一项即重复计数;归因切片只做占比条,之和 ≤ 总量。
- **压缩触发口径统一切换**:摘要触发(0.85)/ postcheck(0.95)/ 机械
  `compact_messages`(经 `extra_tokens` 参,无 gate 群聊/worker 同受益)
  从 messages-only 改统一总量 —— 修 tools+system 挤窗漏计洞(小窗口
  模型下请求可整体超窗而压缩不触发)。
- **实发口径**(prd D9):关卡⑤硬卡裁剪发生时,trace 各切片列记
  `预裁 − freed`(臂 3 触发时 memory_token 改记目录态值)—— 与
  provider `context_input` 可比;预裁值只进 audit payload。
- turn-smoke 报告列加 at_files/system/ctx_win + at_pct。
- 完整闸门语义见 [pattern-budget-gate](./agent-loop-architecture/pattern-budget-gate.md)。

## Scenario: worker per-turn 行 + run 维度唯一键(2026-08-20,task 08-20-worker-turn-trace-persist)

`turn_trace` 加 `run_id TEXT NOT NULL DEFAULT ''` 列,唯一键重建为
`UNIQUE(session_id, run_id, seq)`(`''` 哨兵 = 主 loop 行;worker 行 =
`subagent_runs.id`)。worker(`skip_persist=true`)的 Done upsert 开闸写
(父 sid, run UUID, seq)行;`update_last_turn_usage` 的 `!skip_persist` 门
**不动**(snapshot 隔离,RULE-A-015 reversal)。

- **seq 空间冲突是根因**(为什么必须动唯一键):worker loop 的 seq 从父
  DB messages max+1 起(`init.rs`),与父后续轮次**共享同一区间** —— 旧
  `UNIQUE(session_id, seq)` 下 worker 行会被父后续 Done upsert 撞行覆写
  (并发 fan-out 的多个 worker 亦互撞)。run 维度并入锚点后三方共存。
- **worker 行切片语义**(与列文档契约一致):usage_json + tools_token +
  system_token + context_window 落值;memory_token **按契约记 NULL** ——
  注意 worker 经共享 init 路径同样注入 memory banner(有层级时是真
  Some),在写点显式归 NULL(度量面排除 worker,非"估算为零");
  images/at_files worker 恒 NULL(无附件/无 @注入)。
- **读侧路由**:`list_turn_traces` 只回主行(`WHERE run_id = ''`,前端
  `Map<seq, TurnTrace>` 契约零变化);worker 行走
  `list_worker_turn_traces(run_id)`(SubagentDrawer「Token 明细」)。
  `list_speaker_cache_usage` 的 join 也要 `AND t.run_id = ''`(worker 行
  与父 messages 共享 seq 区间,不排除会误配)。
- **旁路写点归位**(本任务顺带修的既有跨归因 bug):机械压缩
  `record_compaction` 与 C2 软提示 `record_loop_hint` 原本**无 worker 门**,
  worker 撞线时以 (父 sid, worker seq) 写主行 —— 父后续同 seq 的 Done
  upsert 合并进该行,父卡片显示从未发生的压缩/loop 提示。现两写点按
  `run_key` 路由(worker 写 run 行,主路不变)。db 层 4 个 upsert 统一
  加 `run_id: &str` 参。
- **降级**:`insert_run` 失败 → `worker_run_id=None` → run_key='' →
  worker 写点自然不写(不造孤儿命名空间)。
- worker 行的 seq 是 loop 内游标,**勿当父 messages 全局 seq 消费**。
