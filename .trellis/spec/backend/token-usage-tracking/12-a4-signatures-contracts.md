<!-- Moved from token-usage-tracking.md 2026-09-19 (doc-split) -->

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
-- ⚠ 2026-08-21 勘误(08-20-turn-usage-event-quota-view 实现期发现):
-- input_tokens_total / output_tokens_total 自 2026-06-26 snapshot 重构后
-- **无写点**(update_last_turn_usage 只写 last_* 快照列),是孤儿列;
-- session 全周期累计的可信口径 = turn_trace 全量聚合
-- (db::usage::usage_window 的 lifetime 口径),勿再读这两列。
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

