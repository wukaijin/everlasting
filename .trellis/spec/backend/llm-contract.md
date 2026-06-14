# LLM API Contract —核心类型与思考契约

> **基线**:2026-06-10 commit `0f9a167` (8-PR5拆分后)
> **来源**:从原 `llm-contract.md` (3149 行)拆出本文件 +4 个子文件 (`tool-contract.md` / `worktree-contract.md` / `multi-provider-contract.md` / `test-model-contract.md`)
> **同源文档**:
> - [llm-contract.md](./llm-contract.md) (本文) —核心类型 + Extended Thinking + 反模式汇总
> - [tool-contract.md](./tool-contract.md) —工具定义 + ReadGuard + shell spillover
> - [worktree-contract.md](./worktree-contract.md) — attach/detach/delete + cancel + system prompt
> - [multi-provider-contract.md](./multi-provider-contract.md) — Provider trait + catalog + Anthropic/OpenAI 分发
> - [test-model-contract.md](./test-model-contract.md) — `test_model` IPC
>
> **何时读本文**:涉及 `ContentBlock` / `ChatMessage` / `ChatEvent` / Extended Thinking持久化 / 反模式排查时。

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

###1. Scope / Trigger

- Trigger: Added `ContentBlock::Thinking` and `ContentBlock::RedactedThinking`
 to satisfy the cross-layer request/response contract for Anthropic extended thinking.
- Why code-spec depth: mandatory — the request body must include the right `thinking`
 shape, the response must be parsed without losing the `signature` blob, the signature
 must round-trip on subsequent turns or Anthropic returns `400`, and the SSE parser
 emits three new event variants that the frontend must handle in order.

###2. Signatures

#### Backend types (`app/src-tauri/src/llm/types.rs`)

```rust
pub struct LlmConfig {
 pub base_url: String,
 pub model: String,
 pub api_key: String,
 pub max_tokens: u32,
 pub thinking_effort: String, // "low" | "medium" | "high" | "xhigh" | "max"
}

pub struct ChatRequest {
 pub model: String,
 pub max_tokens: u32,
 pub system: Option<String>,
 pub messages: Vec<ChatMessage>,
 pub tools: Vec<ToolDef>,
 pub thinking: Option<ThinkingConfig>, // always Some(Adaptive{..}) in practice
}

#[serde(tag = "type", rename_all = "snake_case")]
pub enum ThinkingConfig {
 Adaptive { display: String, effort: String }, // display always "summarized"
}

#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
 Text { text: String },
 ToolUse { id: String, name: String, input: serde_json::Value },
 ToolResult { tool_use_id: String, content: String, is_error: bool },
 Thinking { thinking: String, signature: String },
 RedactedThinking { data: String },
}

#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatEvent {
 Start { request_id: String },
 Delta { text: String },
 ThinkingDelta { text: String },
 SignatureDelta { signature: String },
 RedactedThinkingDelta { data: String },
 ToolCall { id: String, name: String, input: serde_json::Value },
 ToolResult { tool_use_id: String, content: String, is_error: bool },
 Done { stop_reason: String, usage: serde_json::Value },
 Error { message: String, kind: LlmErrorKind },
}
```

#### Frontend payload (`app/src/stores/chat.ts`)

```typescript
type ContentBlockPayload =
 | { type: "text"; text: string }
 | { type: "tool_use"; id: string; name: string; input: unknown }
 | { type: "tool_result"; tool_use_id: string; content: string; is_error: boolean }
 | { type: "thinking"; thinking: string; signature: string }
 | { type: "redacted_thinking"; data: string };

// ChatMessage.thinkingBlocks is in-memory only; persisted as
// ContentBlock::Thinking { thinking, signature } rows in the DB.
type ThinkingBlockInfo = { thinking: string; signature: string };
```

###3. Contracts

#### Request (always sent)

```json
{
 "model": "<from env LLM_MODEL>",
 "max_tokens":16384,
 "system": "<system prompt or omitted>",
 "messages": [ ... ],
 "tools": [ ... ],
 "thinking": { "type": "adaptive", "display": "summarized", "effort": "high" }
}
```

- `thinking` is **always present** in the request body. There is no kill switch.
 - If the upstream model does not support adaptive thinking, the call returns `400`.
 This is an accepted operational risk (see ADR D5 in the task PRD).
- `thinking.display` is **always `"summarized"`** — explicit, not omitted.
 - On Opus4.7+ the default `display` is `"omitted"`, which suppresses `thinking_delta`
 SSE events and breaks the UI.
- `thinking.effort` is sourced from `LLM_THINKING_EFFORT` (default `"high"`).
 - Valid values: `low` / `medium` / `high` / `xhigh` / `max` (Anthropic schema).
 - Invalid values pass through unchanged; the upstream API will reject them.
- `max_tokens` default is `16384` (was `1024` in step2; bumped in step6 because
 thinking tokens count against the same budget as the actual answer).

#### Response (SSE event sequence)

```
content_block_start { index:0, content_block: { type: "thinking", thinking: "" } }
content_block_delta { index:0, delta: { type: "thinking_delta", thinking: "..." } }
content_block_delta { index:0, delta: { type: "signature_delta", signature: "..." } }
content_block_stop { index:0 }
content_block_start { index:1, content_block: { type: "text", text: "" } }
content_block_delta { index:1, delta: { type: "text_delta", text: "..." } }
content_block_stop { index:1 }
message_delta { delta: { stop_reason: "end_turn" } }
message_stop
```

Block types observed in step6: `text`, `tool_use`, `thinking`, `redacted_thinking`.
Delta types observed: `text_delta`, `input_json_delta`, `thinking_delta`, `signature_delta`.

#### Environment keys

| Key | Required | Default | Notes |
|-----|----------|---------|-------|
| `ANTHROPIC_API_KEY` (or `ANTHROPIC_AUTH_TOKEN`) | yes | — | The dev setup uses `<your-anthropic-compat-host>` proxy tokens; `ANTHROPIC_AUTH_TOKEN` is the legacy alias. |
| `ANTHROPIC_BASE_URL` | no | `https://api.anthropic.com` | Trailing `/v1/messages` is appended by `LlmConfig::endpoint()`. |
| `LLM_MODEL` | no | `GLM-4.7` | |
| `LLM_MAX_TOKENS` | no | `16384` | Was `1024` before step6. |
| `LLM_THINKING_EFFORT` | no | `high` | Adaptive thinking effort. |

###4. Validation & Error Matrix

| Condition | Result |
|-----------|--------|
| `ANTHROPIC_API_KEY` missing at startup | `LlmConfig::unconfigured()` — `api_key: ""`, app still launches so UI shows a helpful error. |
| `LLM_MAX_TOKENS` is not a number | Falls back to default `16384`. |
| `LLM_THINKING_EFFORT` is unrecognized | Sent verbatim; upstream may400. |
| Upstream rejects `thinking: { type: "adaptive" }` | Anthropic returns400. Switch base_url or downgrade to manual mode (out of MVP scope). |
| `signature` is lost on round-trip (e.g. dropped during rehydrate) | Anthropic returns400 on the next turn. **Hard rule: `signature` must round-trip verbatim.** |
| `redacted_thinking.data` is mutated or truncated | Anthropic returns400 on the next turn. Opaque — store as-is. |
| `thinking` block appears after a `tool_use` block in history | Anthropic rejects the order. The rehydrate path emits thinking blocks FIRST. |
| `content_block_start` for `thinking` arrives with non-empty `thinking`/`signature` fields | Treated as the initial buffer content (defensive — Anthropic today sends empty). |

###5. Good / Base / Bad Cases

#### Good: streaming + persistence + round-trip

1. Model emits `thinking_delta` × N, then `signature_delta` ×1, then `content_block_stop`.
2. Backend buffers the signature; emits `ThinkingDelta` per `thinking_delta` event;
 emits a single `SignatureDelta` on `content_block_stop`.
3. Agent loop finalizes a `ContentBlock::Thinking { thinking, signature }` at the
 turn boundary; persists to DB; emits to frontend.
4. Frontend rehydrates on next session load; `toPayloadContent` puts the thinking
 block first in the assistant message.
5. Next request to Anthropic carries the full signature; no400.

#### Base: redacted_thinking

1. Safety filter triggers; Anthropic emits `content_block_start { type: "redacted_thinking", data: "..." }`
 followed immediately by `content_block_stop` (no streaming deltas).
2. Backend buffers `data`; emits a single `RedactedThinkingDelta` on stop.
3. Agent loop finalizes a `ContentBlock::RedactedThinking { data }`.
4. Frontend renders a "🔒 N redacted thinking block(s)" placeholder; the data is
 never displayed.

#### Bad: per-event signature emit

1. (The original step6 implementation emitted `SignatureDelta` per `signature_delta`
 event instead of buffering until `content_block_stop`.)
2. If Anthropic ever splits the signature across N `signature_delta` events (defensive),
 the frontend opens N empty-text thinking blocks; the DB stores N partial-signature
 blocks; the next turn's history is malformed → Anthropic returns400.
3. Fix: buffer in `BlockState::Thinking { signature_buf }`; emit once on stop.

#### Bad: thinking block re-emitted as second text block

1. UI's `MessageContent::to_text()` accidentally includes thinking text in the
 denormalized `text` column.
2. Rehydrate reads both the `text` block AND the thinking text into the bubble.
3. The user sees duplicated content; on the next turn the model is confused.

###6. Tests Required

The step6 PR added15 unit tests; the following are mandatory for any future
change to this area.

#### Backend (`cargo test`)

| Test | Asserts |
|------|---------|
| `thinking_block_serializes_to_anthropic_schema` | `ContentBlock::Thinking` → `{"type":"thinking","thinking":"...","signature":"..."}`. |
| `thinking_block_deserializes_from_anthropic_schema` | The reverse round-trip. |
| `redacted_thinking_block_serializes_to_anthropic_schema` | `type: "redacted_thinking"` with only the `data` field. |
| `redacted_thinking_block_deserializes_from_anthropic_schema` | Reverse. |
| `chat_message_round_trip_with_thinking_blocks` | Thinking blocks survive `to_json` / `from_json` losslessly. |
| `chat_message_round_trip_with_redacted_thinking` | Same for redacted. |
| `message_content_to_text_excludes_thinking` | `MessageContent::to_text()` does NOT include thinking text in the denormalized string. |
| `chat_event_thinking_delta_serializes_with_snake_case_kind` | The wire `kind` is `"thinking_delta"`. |
| `chat_event_signature_delta_serializes_with_snake_case_kind` | `"signature_delta"`. |
| `chat_event_redacted_thinking_delta_serializes_with_snake_case_kind` | `"redacted_thinking_delta"`. |
| `chat_request_thinking_omitted_when_none` | `Option<ThinkingConfig>` uses `skip_serializing_if` (or equivalent) when `None`. |
| `chat_request_thinking_adaptive_serializes_correctly` | Output: `{"type":"adaptive","display":"summarized","effort":"high"}`. |
| `default_max_tokens_is_16384_not_1024` | Env-less default is `16384`. |
| `thinking_config_is_adaptive_summarized_with_configured_effort` | `LlmConfig::from_env()` honors `LLM_THINKING_EFFORT` and always sets `display: "summarized"`. |
| `unconfigured_has_empty_thinking_effort` | `LlmConfig::unconfigured()` has `thinking_effort: ""` (does not panic). |

Total backend suite:57 tests pass as of step6.

#### Frontend

- `pnpm build` (vue-tsc strict) must pass. The thinking-related types live in
 `app/src/stores/chat.ts`; any field added there must be type-checked end-to-end.
- Manual smoke test (acceptance A9): `cd app && pnpm tauri dev`, observe
 thinking stream + `<details>` collapse + session switch round-trip.

###7. Wrong vs Correct

#### Wrong: emit `SignatureDelta` per `signature_delta` SSE event

```rust
// BAD — emit immediately on each delta
"signature_delta" => {
 let sig = delta.get("signature").and_then(|s| s.as_str()).unwrap_or("").to_string();
 yield Ok(ChatEvent::SignatureDelta { signature: sig });
 // signature_buf is dead code
}
```

If Anthropic ever splits the signature across events, the frontend opens N
thinking blocks, the DB stores N partial signatures, and the next turn400s.

#### Correct: buffer and emit on `content_block_stop`

```rust
// GOOD — buffer, then emit once on stop
"signature_delta" => {
 if let BlockState::Thinking { signature_buf, .. } = &mut block_state {
 signature_buf.push_str(delta.get("signature")...);
 }
 // no event emitted here
}
// ...
BlockState::Thinking { signature_buf, .. } if !signature_buf.is_empty() => {
 yield Ok(ChatEvent::SignatureDelta { signature: std::mem::take(signature_buf) });
}
```

The buffered signature is the full assembled blob, ready for the DB and the
next-turn payload.

#### Wrong: thinking block emitted after tool_use in history

```typescript
// BAD — toPayloadContent appends thinking at the tail
function toPayloadContent(m: ChatMessage): ContentBlockPayload[] {
 return [
 ...m.toolUses.map(...),
 ...m.thinkingBlocks.map(t => ({ type: "thinking", ...t })),
 ];
}
```

Anthropic requires thinking blocks at the head of an assistant message; the
next turn400s.

#### Correct: thinking blocks first

```typescript
// GOOD — thinking blocks first, then tool_use / text
function toPayloadContent(m: ChatMessage): ContentBlockPayload[] {
 return [
 ...m.thinkingBlocks.map(t => ({ type: "thinking", thinking: t.thinking, signature: t.signature })),
 ...m.text.map(t => ({ type: "text", text: t })),
 ...m.toolUses.map(...),
 ];
}
```

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
real workloads. Env override available.

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

**Test coverage** (in `agent/context.rs`):
- `case_3_tool_use_tool_result_pair_intact_or_dropped_together`
- `regression_pair_at_tail_split_under_pressure` (C3 PR1 regression)

**Related**:
- Thinking blocks have a similar atomicity requirement (see Validation & Error
 Matrix row "`thinking` block appears after a `tool_use` block in history") —
 the assistant turn is the atomic unit for thinking, while the pair is the
 atomic unit for tool_use.

---

## Scenario: Per-Session Mode + ⑨ 关 Permission Layer (A2 + B7, 2026-06-13)

### 1. Scope / Trigger

The ⑨ 关 permission layer is the unified decision point between
the agent loop's `provider.send()` stream and `tools::execute_tool`.
PR1 implemented the backend half (per the merged A2 + B7 task
`06-12-a2-b7-permission-and-mode`); PR3 wired the frontend
PermissionModal + `usePermissionsStore`. This section captures
the cross-layer contract that touches the LLM stream — the
per-turn system prompt prefix (⑧a first defense), the
per-turn tool list filter (⑧a second defense), the 5-tier
permission decision (⑨ 关), and the `permission:ask` IPC that
backs it. Changes here cascade through the agent loop,
Tauri commands, Pinia store, and the Vue modal.

### 2. Mode enum (per-session)

The active session carries a `Mode` enum value bound to
`sessions.mode` (TEXT column, nullable, backfilled to `chat`).
Five variants, but only four are user-facing — `Background` is
reserved in the enum for schema stability and is never exposed
in the UI (PR2 decision).

| Mode | UI | Tool execution | User confirm | Notes |
|---|---|---|---|---|
| `Edit` | ✓ | full | dangerous-tool ask | Default (2026-06-13 rename from `Chat`); matches Claude Code `default` |
| `Plan` | ✓ | read-only (write tools filtered) | — | ⑧a system-prompt + tool-list filter + runtime intercept |
| `Yolo` | ✓ | full (skip Tier 3 ask) | none | Hard kill list (Tier 2) still enforced |
| `Background` | ✗ (enum only) | n/a | n/a | Reserved; future |

`Mode` serializes lowercase (`"edit"` / `"plan"` / `"yolo"`
/ `"background"`) on the IPC wire. (2026-06-13: `"chat"`
renamed to `"edit"`, `"review"` removed; breaking wire
change — see ADR in `docs/IMPLEMENTATION.md §4`.)

### 3. ⑧a Triple Defense (Mode interception)

The mode check is enforced in three layers, mirroring the
Claude Code `--permission-mode plan` design (see
`research/mode-state-machine.md §Q3 "Design B"`):

1. **Per-turn system prompt prefix** —
   `app/src-tauri/src/agent/permissions::mode_system_prefix(mode)`
   returns a per-mode instruction string that the agent loop
   prepends to the system prompt before every `provider.send()`
   call. The string tells the LLM in plain English what the
   mode allows (e.g. "you may read files but CANNOT execute
   write tools"). This is the cheapest layer; a well-instructed
   LLM never attempts a forbidden tool.
2. **Per-turn tool list filter** —
   `permissions::filter_tools_for_mode(tools, mode)` returns a
   filtered tool list. `Plan` drops `write_file`,
   `edit_file`, `shell`; `Edit` / `Yolo` keep the full set.
   The filtered list is what the LLM sees in the request body
   `tools` field — LLM doesn't even know the forbidden tools
   exist in those modes.
3. **⑧a runtime intercept** — even with layers 1 + 2, the
   LLM may still emit a forbidden `tool_use` (prompt-injection
   attack, model regression, etc.). The agent loop's Tier 4
   in `permissions::check` catches this: if
   `Mode::Plan` and the tool is in the
   write-block list, the loop returns
   `Decision::Deny { reason: "I cannot execute X in Y mode
   (read-only session)", critical: false }` and the agent loop
   appends an `is_error: true` tool_result for the LLM to
   self-correct.

### 4. ⑨ 关 5-Tier Decision Order

PR1's `agent::permissions::check` runs a 5-tier evaluation
in this exact order (SOT, matches Claude Code's
`deny > ask > mode > allow` rule from `permissions.md`):

```
Tier 1. Hooks           — pre-call interface (MVP: no-op)
       │ 命中 hook override? → 用 hook 决定(本期不实现)
       ↓
Tier 2. Deny rules      — hard kill list (dangerous::is_kill_listed)
       │ 命中 → 直接 Decision::Deny { critical: true, reason: ... }
       │ Yolo 模式也走 — 静默拒绝,不弹窗
       │ → Tier 6 写 audit (kind="tool_denied" 或 "tool_denied_yolo")
       ↓
Tier 3. Ask rules       — session_tool_permissions + emit + await
       │ 查 session_tool_permissions:
       │   有 "始终允许" 记录 → 跳过弹窗, 直接 Allow
       │   无 → emit("permission:ask", { rid, tool_name, tool_input, risk, reason? })
       │       等前端 permission_response (120s 超时 → 自动 Deny)
       │ 收到响应:
       │   allow_once  → 放行(不写表)
       │   allow_always → 放行 + INSERT INTO session_tool_permissions
       │   deny        → Deny { reason: "user denied" }
       │   timeout     → Deny { reason: "permission timed out after 120s, treat as denied" }
       │                + audit kind="permission_timeout"
       ↓
Tier 4. Mode check      — Plan 拦截 (3 档化 2026-06-13: Review 移除)
       │ Plan 模式 + tool ∈ {write_file, edit_file, shell}
       │ → Deny { reason: "I cannot execute X in Plan mode" }
       ↓
Tier 5. Allow rules     — 默认 allow-all (MVP 阶段)
       ↓
Tier 6. Audit hook      — 每个决策路径写 session_audit_events
       │ kind: tool_allowed / tool_denied / tool_permission_ask /
       │       permission_granted / permission_timeout / request_cancelled
       ↓
   → execute_tool(若 Allow) / 构造 is_error tool_result(若 Deny)
```

**关键行为**:
- **Deny 优先于 Ask**:`rm -rf /` 在 Yolo 模式下也是静默拒绝
  (Tier 2 在 Tier 3 之前)
- **Tier 3 拒绝 ≠ Cancel 整轮**:Deny 只跳该 tool_use,LLM 收到
  `is_error: true` 可自决;CancellationToken (C1) 才是整轮终止
- **超时 vs 主动 deny** 在 audit log 区分:`reason` 字段不同
  ("user denied" vs "permission timed out after 120s, treat as denied")

#### 4.1. Re-grill update 2026-06-13: 5-tier 重排 + path-based 决策

> Supersedes 4 节上述旧设计。Source of truth:
> `.trellis/tasks/06-13-a2-b7-regrill-path-based/prd.md` §1。

旧设计 ⑨ 关 Tier 3 "总是弹窗" 在 Edit 模式下读 README
都要弹,反直觉;Tier 4 Mode check 在 Ask 之后让 Plan
模式下"用户点始终允许,然后被 Mode 拒"成为坏交互。
re-grill 锁定 10 决策,把决策层重构:

**新 Tier 顺序**:

```
Tier 1. Hooks           (MVP no-op)
Tier 2. Deny rules      (硬 kill list,shell 9 个 regex,Yolo 走 — 静默拒)
Tier 3. Mode check      (Plan 拦截 write/edit/shell,text 错,不发 modal)
Tier 4. Path / Prefix / External policy
       ├─ Path 工具:is_within_root → 查 session_tool_permissions
       │   (match_kind='path') → hit Allow / miss silent(in) / miss ask(out)
       ├─ Shell:classify_prefix → Allow(whitelist) / Ask(asklist + 未知)
       └─ Web Fetch:查 match_kind='tool' for 'web_fetch' → hit Allow / miss ask
       (Yolo:整段 bypass,直接 Allow;Tier 2 仍 hard wall)
Tier 5. Allow rules     (default allow-all)
Tier 6. Audit           (写 session_audit_events)
```

**跟旧设计 diff**:

| 改动 | 旧 (PR1) | 新 (re-grill) |
|---|---|---|
| Tier 顺序 | Hooks → Deny → Ask → Mode → Allow → Audit | Hooks → Deny → **Mode → Path/Prefix** → Allow → Audit |
| 弹窗判定 | risk 等级 + 总是弹 | **path-based**:仓库内 silent,仓库外 ask |
| Mode check 时机 | Tier 4(在 Ask 之后) | **Tier 3(在 Ask 之前)** — 消除 Plan + 始终允许坏交互 |
| "始终允许"持久化 | 只 `tool` | **3 种 match_kind: tool + path-glob + prefix** |
| shell 策略 | 总是 Tier 3 | **白名单/asklist/未知 三档**(prefix 解析) |
| Yolo × 仓库外 | 走 Tier 3 modal | **silent**(Yolo bypass Tier 4) |
| Tier 2 kill list | 9 regex | **不变** |
| `PermissionAskPayload` | rid + tool + input + risk + reason | + **`path: Option<String>`** (新, `skip_serializing_if`) |
| `Risk` 字段 | 4 档 | 不变(4 档,UI 视觉) |

详细 ⑨ 关 contract 见 `tool-contract.md §"Scenario: Path-based
Permission Layer"`,包括 `shell_trust::classify_prefix` 的
whitelist / asklist 完整表。

### 5. ⑨ 关 ↔ `permission:ask` IPC 协议

**Server → Client**:后端 `agent::permissions::check` Tier 3 发:

```rust
app.emit("permission:ask", &PermissionAskPayload {
    rid: String,                // UUID
    tool_name: String,
    tool_input: serde_json::Value,
    risk: Risk,                  // Low | Medium | High | Critical (lowercase)
    reason: Option<String>,      // 人类可读原因
});
```

`PermissionAskPayload` uses `#[serde(rename_all = "camelCase")]`,
producing the wire shape:

```jsonc
{
  "rid": "uuid",
  "toolName": "shell",
  "toolInput": { "command": "ls -la" },
  "risk": "high",
  "reason": "The tool shell requires your confirmation (risk: 高)."
}
```

**Client → Server**:前端 `usePermissionsStore.respond(rid, decision)`:

```typescript
invoke("permission_response", { rid: "uuid", decision: "allow_once" | "allow_always" | "deny" })
```

后端 `commands::permissions::permission_response` (Tauri command)
查 `PermissionStore: Arc<Mutex<HashMap<rid, oneshot::Sender>>>`,
发响应到 `check()` 正在 await 的 oneshot,唤醒后续逻辑。

**Wire invariant**:frontend `respond(rid)` 必须用后端 emit
`permission:ask` 时附带的 `rid` — 后端 `HashMap<rid, Sender>`
的 key 是 emit 时刻的 UUID。客户端不能自己生成 rid;只能转发
后端给的 rid + decision。

### 6. Audit (`session_audit_events`) — 10 类 AuditKind

PR1 在 `agent::permissions::AuditKind` 实现了 10 类事件(见
`audit §3.4` 完整列表)。`payload_json` 字段统一结构:

```json
{
  "tool_name": "shell",
  "tool_input": { "command": "ls -la" },
  "reason": "matches denylist: rm -rf /",
  "mode": "edit",
  "critical": true
}
```

`critical: bool` 字段对前端 `PermissionModal` 的 3px 红左 border
+ shield-x icon 渲染至关重要(PR1 follow-up 加,PR3 使用)。

| AuditKind | 触发条件 |
|---|---|
| `tool_denied` | Tier 2 命中 + Tier 3 user deny + Tier 3 sender dropped |
| `tool_allowed` | Tier 3 AllowOnce / Tier 3 "始终允许" 命中 / Tier 5 默认 |
| `tool_permission_ask` | Tier 3 emit `permission:ask` |
| `permission_granted` | Tier 3 "始终允许" → 写 `session_tool_permissions` |
| `permission_timeout` | Tier 3 120s 超时 |
| `tool_denied_yolo` | Tier 2 命中 + mode = Yolo(audit 跟普通 tool_denied 区分) |
| `mode_changed` | `set_session_mode` 调用 |
| `yolo_entered` / `yolo_exited` | Mode 在 Yolo 之间切换 |
| `request_cancelled` | C1 cancel 触发(tier 3 await 被 cancel 打断) |

### 7. ⑨ 关 IPC 异常路径

| 异常场景 | 处理 |
|---|---|
| 用户从不响应 (>120s) | 后端 `tokio::time::sleep(ASK_TIMEOUT)` 触发 → 自动 deny + `is_error: true, content: "permission timed out after 120s, treat as denied"`(提醒 LLM 是超时不是 user 主动)。前端 store 也复制 120s timer 来关 modal + 弹 toast。 |
| 重复 `permission_response` | 后端 `HashMap<rid, Sender>`:`send().ok()` 失败(rid 不存在)=> 返回 `Ok(false)`,日志 warn。no-op。 |
| Session 在等待时被删除 | 当前 MVP `cancel_session_asks` 实现是"清空所有 pending"(rid 没绑 session_id);这会让所有 session 的 oneshot 失败。后续 PR 应改成 `HashMap<(session_id, rid), Sender>` 精细清理。 |
| `rid` 过期/无效 | 后端校验 rid 存在性,无效 → 日志 warn + no-op + 返回 `Ok(false)`。 |
| `mode` 字段在 wire 中是 `background` (enum 保留值) | 后端 lenient parse → 翻 `chat`;`set_session_mode` 不报错(用 Chat 写回)。 |

### 8. Tests Required

**Backend (`cargo test`)**(PR1 已有,PR3 不新增):

| Test | Asserts |
|---|---|
| `permissions::tests::risk_for_tool_categorization` | per-tool static map correct |
| `permissions::tests::risk_label_cn_is_full_text` | 中文 label 完整 |
| `permissions::tests::mode_as_str_round_trip` | 5 个 mode 都 round-trip |
| `permissions::tests::mode_from_str_unknown_defaults_to_chat` | lenient parse |
| `permissions::tests::filter_tools_for_mode_drops_writes_in_plan` | ⑧a tool filter |
| `permissions::tests::filter_tools_for_mode_keeps_full_for_chat_yolo` | ⑧a full tool list |
| `permissions::tests::mode_system_prefix_is_non_empty` | 5 个 mode 都有 prefix |
| `permissions::tests::audit_kind_round_trip` | 10 类 AuditKind 都 serializable |
| `permissions::dangerous::tests::kill_list_blocks_rm_rf_root` | Tier 2 命中 |
| `permissions::dangerous::tests::kill_list_blocks_fork_bomb` | Tier 2 命中 |
| `permissions::dangerous::tests::kill_list_normal_dev_commands_pass` | Tier 2 不误杀 |
| ...(总计 20 个 permission 测试,见 PR1 落地) |

**Frontend (`pnpm test`)**(PR3 新增):

| Test | Asserts |
|---|---|
| `usePermissionsStore — start() registers a permission:ask listener` | listener 注册 |
| `usePermissionsStore — setPending populates pendingPermission` | 写入 slot |
| `usePermissionsStore — a new ask replaces the prior` | 单 slot 语义 |
| `usePermissionsStore — respond fires permission_response IPC` | IPC wire |
| `usePermissionsStore — respond with allow_always / deny` | 3 decision 字符串 |
| `usePermissionsStore — respond does NOT clear pendingPermission if rid doesn't match` | race-guard |
| `usePermissionsStore — 120s timer fires deny + toast` | ASK_TIMEOUT_MS |
| `usePermissionsStore — stop() tears down the listener + clears state` | lifecycle |
| `PermissionModal — renders 3 buttons when ask pending` | UI |
| `PermissionModal — clicking 拒绝/仅一次/始终允许 calls store.respond` | 3 button wire |
| `PermissionModal — Esc / X / backdrop click → deny` | Q6 spec |
| `PermissionModal — Enter on non-critical → allow_once; critical → deny` | audit §6.2 |
| `PermissionModal — critical modifier class when risk==='critical'` | 3px 红 border |
| `PermissionModal — copy button writes JSON to clipboard` | UX |
| `PermissionModal — Chinese risk label per level` | audit §6.2 |

---

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

All four columns are **nullable** (no `DEFAULT`); a pre-A4 session
keeps NULL until its first LLM turn post-upgrade, when
`add_token_usage` initializes them from 0 (via `COALESCE(col, 0) + ?`).

#### DB function (`app/src-tauri/src/db/sessions.rs`)

```rust
pub async fn add_token_usage(
 pool: &SqlitePool,
 session_id: &str,
 usage: &TokenUsage,
) -> Result<(), sqlx::Error> {
 // Single UPDATE: 4 columns added in place, updated_at bumped.
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

The IPC field is **snake_case** (the existing `kind` discriminator
and the existing `stop_reason` are both snake_case; mixing styles
here would break the `parse_*` symmetry on the TS side). Field
names mirror Rust's `TokenUsage` 1:1 — no `camelCase` rewrite on
the boundary (the outer `ChatEventPayload` is camelCase via
`#[serde(rename_all = "camelCase")]` on the Rust side at the
struct level, but the inner `usage` JSON object keeps snake_case
to match the user's request). **See "Wrong vs Correct" §7 for the
rationale.**

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

#### Agent loop accumulation (R2)

The agent loop's `ChatEvent::Done` handler in
`app/src-tauri/src/agent/chat.rs`:

```rust
ChatEvent::Done { stop_reason: sr, usage } => {
 stop_reason = sr.clone();
 if let Some(t) = usage {
 if let Err(e) = crate::db::add_token_usage(&db, &session_id, t).await {
 tracing::warn!(error = %e, "failed to accumulate token usage");
 }
 } else {
 tracing::info!("skipping token accumulation (no usage in Done)");
 }
}
```

The single SQL `UPDATE col = col + ?` is column-additive, so
multi-turn sessions converge on the cumulative total. The
`COALESCE(col, 0) + ?` pattern handles pre-A4 rows (NULL treated
as 0).

#### Frontend accumulation (`chat.ts` + `streamController.ts`)

`streamController.handleChatEvent("done")` calls
`useChatStore().accumulateTokenUsage(sid, event.usage)`. The chat
store's `tokenUsageBySession: reactive(Map)` holds the running
totals. `currentSessionTokenUsage` is a `computed` that the
ChatInput hint reads.

The map is also **seeded** from `SessionSummary` /
`load_session` results, so a page reload shows the cumulative
value (not "—") immediately.

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
| `add_token_usage` on missing session id | `UPDATE` matches 0 rows. `sqlx::Error` is not raised. `tracing::info!` would log success, but the write is a no-op. |
| `add_token_usage` on a session where columns are NULL | `COALESCE(col, 0) + ?` evaluates `col` to 0, writes `?`. Subsequent reads show `Some(value)`. |
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
4. The stream yields `ChatEvent::Done { stop_reason: Some("end_turn"), usage: Some(TokenUsage { 1234, 56, 100, 200 }) }`.
5. The agent loop's `if let Some(t) = usage { add_token_usage(...) }` runs the SQL UPDATE.
6. The frontend's `streamController.handleChatEvent("done")` sees the `usage` field and calls `useChatStore().accumulateTokenUsage(sid, t)`. The `tokenUsageBySession` map updates; `currentSessionTokenUsage` re-evaluates.
7. The ChatInput hint area re-renders: `1.2K · 1% / 200K` (assuming 1234 tokens is ~0.6% of 200K context_window). The color is `ok` (green).

#### Good: OpenAI happy path

1. Same flow, but the `chat` command's `resolve_chat_provider` returns an `OpenAIProvider` (the user has switched the default to a gpt-4o model).
2. The OpenAI adapter's `build_http_body` includes `"stream_options": { "include_usage": true }`.
3. The SSE stream emits normal text deltas, then a final chunk with `usage: { prompt_tokens: 200, completion_tokens: 30, total_tokens: 230, prompt_tokens_details: { cached_tokens: 50 } }` and no `choices`.
4. The adapter's `parse_openai_usage` normalizes: `input_tokens: 200, output_tokens: 30, cache_read_input_tokens: 50, cache_creation_input_tokens: 0`.
5. The agent loop + frontend flow identically to the Anthropic path.

#### Base: cancel mid-stream

1. User sends a question; LLM starts streaming.
2. User hits Stop. The cancellation token fires; the agent loop's `tokio::select!` notices on the next event boundary.
3. The agent loop bails out, persists whatever's been collected so far, and yields `ChatEvent::Done { stop_reason: Some("cancelled"), usage: None }`.
4. The frontend's `done` handler sees `usage` is undefined; `accumulateTokenUsage` is not called.
5. The SQL write is skipped. The session's column totals reflect all PRE-cancel turns (i.e. the cancel did NOT roll anything back). This is the correct behavior — a user who cancelled 3 turns in still has those 3 turns' usage on the dashboard.

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

**`llm::types` (4 new tests)**

- `token_usage_serializes_with_snake_case_fields`
- `token_usage_default_is_all_zero`
- `token_usage_add_assign_saturates_at_u32_max`
- `chat_event_done_carries_usage_payload`
- `chat_event_done_with_none_usage_emits_null`

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

**`db::sessions` (4 new tests, in `db::tests`)**

- `add_token_usage_first_turn_initializes_columns`
- `add_token_usage_accumulates_across_turns`
- `add_token_usage_on_missing_session_is_noop`
- `list_sessions_includes_token_columns`

Total A4 tests: **18 new cargo tests** (rounds up to 285 from 281;
the implementation also has a few additional tests for chat_event
done serialization edge cases).

#### Frontend

- `pnpm build` (vue-tsc strict) must pass.
- Manual smoke test (acceptance A2 from the parent PRD):
 1. `cd app && pnpm tauri dev`
 2. Open a session, send a question, click Send.
 3. Observe the ChatInput hint area shows "X · Y% / 200K" (e.g. "1.2K · 1% / 200K"), green color (under 50%).
 4. After 4-5 turns, observe the percentage climbs. Watch the color shift to yellow at 50%, red at 75%.
 5. Hover the chip, observe the tooltip shows the four counters (input / cache_read / cache_creation / output).
 6. Open Settings, delete the model's `api_key` (or the model entirely). Send a message — observe the pre-flight error and "token usage not accumulating" (the agent loop never reached `add_token_usage`).
 7. Page reload. Observe the hint area still shows the cumulative value (seeded from `list_sessions`).

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
}

// Resulting IPC payload (mixed at the outer/inner boundary;
// outer is the parent `ChatEventPayload` which has its own
// serde rename; inner is the raw Rust struct field names):
{
 "kind": "done",
 "stopReason": "end_turn", // outer ChatEventPayload camelCase
 "usage": {
 "input_tokens": 1234, // inner TokenUsage snake_case
 "output_tokens": 56,
 "cache_creation_input_tokens": 100,
 "cache_read_input_tokens": 200
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
// ChatEvent::Done { stop_reason, usage: Some(TokenUsage { 4 fields }) }
// Agent loop is protocol-agnostic.
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

#### Correct: per-session cumulative on the sessions row

```rust
// GOOD — single SQL UPDATE on the sessions row, additive
// on the existing column values
UPDATE sessions
 SET input_tokens_total = COALESCE(input_tokens_total, 0) + ?,
 output_tokens_total = COALESCE(output_tokens_total, 0) + ?,
 cache_creation_total = COALESCE(cache_creation_total, 0) + ?,
 cache_read_total = COALESCE(cache_read_total, 0) + ?,
 updated_at = ?
 WHERE id = ?
```

The hint area reads the cumulative value with no aggregation.
Future C3 / B6 work can ALTER `messages` to add per-turn columns
without changing the A4 schema.

### Design Decisions

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

---

## Common Mistakes

### Mistake: Treating `redacted_thinking` as a partial `thinking`

The `data` field of `redacted_thinking` is opaque — Anthropic redacts it for
safety reasons. Don't try to "decode" it, don't append it to the visible
thinking text, don't mutate it. Store and forward verbatim. The UI shows a
"🔒 redacted" placeholder.

### Mistake: Dropping the signature to "save space"

The `signature` is a cryptographic anchor — Anthropic uses it to verify the
thinking block was generated by the same session. Drop it, and the next turn
400s. The DB stores it in full; the rehydrate path emits it in full.

### Mistake: Ordering thinking blocks after tool_use in history

Anthropic requires thinking blocks at the head of an assistant message. If
`toPayloadContent` appends thinking at the tail (or interleaves it with
tool_use), the next turn400s. The fix lives in `toPayloadContent`; the
ordering is tested in the frontend type-check.

---

## Anti-Patterns

- **Don't** add a per-session or per-request thinking toggle. (MVP simplification;
 see PRD D1.)
- **Don't** add an `LLM_THINKING=off` env var. (PRD D5; if the upstream is broken,
 fix the upstream or change code.)
- **Don't** parse `signature` client-side. It is opaque; the proxy might transform
 it, but the wire format is "string of unknown base64-like data."
- **Don't** coalesce multiple thinking blocks into one on rehydrate. The
 interleaving is meaningful when it happens (rare), and Anthropic expects
 the same order on round-trip.

---

## Future Work (Deferred from Step6)

| Item | Why deferred |
|------|-------------|
| Parse `usage.output_tokens_details.thinking_tokens` from `message_delta` | UI uses `length /4` estimate; real count would require a new `ChatEvent` variant + plumbing. |
| Preserve interleaved `thinking → redacted_thinking` order | Redacted is rare and interleaved is even rarer; current code appends redacted to the tail. |
| Coalesce `text → text` into one block | Multiple text blocks in one turn are coalesced; interleaved `thinking → text → thinking → text` loses the second text's position. |
| Fix pre-existing `tool_result` in `assistant` role | From step3a; out of step6 scope. Follow-up task. |
| OpenAI-compat `reasoning_content` fallback | Different wire format entirely; would require its own `ContentBlock` variant + parser. (Cross-protocol handled by Wire layer — see multi-provider-contract.md.) |

---

## Scenario: Latency Tracking (F5, 2026-06-11)

> Per-message wall-clock timing for every LLM turn. Three
> measurements per turn (TTFB / gen / total) and per-tool
> duration for every tool invocation, all persisted to the
> DB so the user can see where their LLM time is going —
> both for the current turn (assistant bubble footer + Tool
> Call Card status row) and cumulatively per session
> (ChatPanel footer). Switching models, switching sessions,
> or restarting the app preserves the timing data.

### 1. Scope / Trigger

- Trigger: add a per-message latency breakdown (TTFB /
  generation / end-to-end) and a per-tool-call duration,
  display both in the chat UI, persist both to the DB.
- Why code-spec depth: mandatory — the new columns on
  `messages`, the new IPC commands, the timing
  measurement boundary (frontend `Date.now()` deltas
  around the SSE event stream), and the embed-in-JSON
  pattern for tool duration all cross multiple layers
  (Rust DB schema → Tauri IPC → Pinia store → Vue
  components) and are non-trivial to recover from
  without a code-spec. A change here cascades to
  rehydrate / latency rehydration / ToolCallCard
  rendering / ChatPanel footer.

### 2. Signatures

#### DB types (`app/src-tauri/src/db/types.rs`)

```rust
/// A message as stored in the DB. `content` is JSON (`Vec<ContentBlock>`).
pub struct MessageRow {
    pub id: i64,
    pub session_id: String,
    pub role: String,
    pub content: serde_json::Value,
    pub text: String,
    pub has_tool_calls: bool,
    pub has_tool_results: bool,
    pub created_at: String,
    pub seq: i64,
    pub metadata: Option<serde_json::Value>,
    /// F5: per-message latency breakdown. All three are
    /// `null` for pre-F5 rows; the `update_message_latency`
    /// IPC fires at `done` to populate them.
    pub ttfb_ms: Option<i64>,
    pub gen_ms: Option<i64>,
    pub total_ms: Option<i64>,
}

/// Three-field latency breakdown measured by the frontend
/// around the SSE event boundaries of one chat invocation.
/// All three fields are optional because the cancel / error
/// paths may only know the total (no `delta` was ever
/// received → no `ttfb_ms`).
#[derive(Debug, Clone, Copy, Default)]
pub struct MessageLatency {
    pub ttfb_ms: Option<i64>,
    pub gen_ms: Option<i64>,
    pub total_ms: Option<i64>,
}
```

#### DB schema (`migrations.rs` F5 ALTER)

```sql
ALTER TABLE messages ADD COLUMN ttfb_ms INTEGER;
ALTER TABLE messages ADD COLUMN gen_ms INTEGER;
ALTER TABLE messages ADD COLUMN total_ms INTEGER;
```

All three columns are **nullable** (no `DEFAULT`); a
pre-F5 session keeps NULL until its first LLM turn
post-upgrade, when `update_message_latency` initializes
them via the IPC. Tool duration follows R2 (embedded
in `messages.content` JSON; **0 schema change**).

#### DB functions (`app/src-tauri/src/db/sessions.rs`)

```rust
/// Update the three latency columns on an already-persisted
/// message row. The IPC looks up the row id by
/// `(session_id, seq)` first (via `find_message_id_by_seq`)
/// and updates by id. Each value is optional — a
/// `Some/None` mix is allowed (cancel / error paths).
pub async fn update_message_latency(
    pool: &SqlitePool,
    message_id: i64,
    latency: &MessageLatency,
) -> Result<(), sqlx::Error>;

/// Resolve `(session_id, seq)` to the auto-incrementing
/// row id. The frontend tracks the seq (the agent loop's
/// handle), not the id, so this is the IPC's lookup
/// bridge. Returns `None` if the pair is unknown (defensive
/// — the controller could in principle race the agent
/// loop's `persist_turn` if the user cancels mid-stream
/// and the cancel cleanup path persists at a later time).
pub async fn find_message_id_by_seq(
    pool: &SqlitePool,
    session_id: &str,
    seq: i64,
) -> Result<Option<i64>, sqlx::Error>;

/// Patch the `duration_ms` field onto a `tool_result`
/// content block embedded in `messages.content` JSON,
/// keyed by `tool_use_id`. Per R2 (ADR-lite decision 1),
/// the per-tool duration is embedded in the `tool_result`
/// block rather than a column — zero schema change for the
/// tool side. Returns `true` if a block was patched, `false`
/// if no matching block was found (defensive — see §4).
pub async fn record_tool_duration(
    pool: &SqlitePool,
    session_id: &str,
    tool_use_id: &str,
    duration_ms: i64,
) -> Result<bool, sqlx::Error>;
```

#### IPC commands (`app/src-tauri/src/commands/sessions.rs`)

| Command | Args (Rust) | Returns | Notes |
|---|---|---|---|
| `update_message_latency` | `session_id: String, seq: i64, ttfb_ms: Option<i64>, gen_ms: Option<i64>, total_ms: Option<i64>` | `Result<bool, String>` | Resolves `(session_id, seq)` to row id internally; returns `Ok(false)` if the seq isn't found. Fire-and-forget from the controller. |
| `record_tool_duration` | `session_id: String, tool_use_id: String, duration_ms: i64` | `Result<bool, String>` | Patches the `tool_result` block in `messages.content` JSON. `Ok(false)` = no matching block (defensive no-op). |

Both are fire-and-forget IPCs from the frontend
`streamController`; the agent loop itself does not call
them. A failure logs in the backend but doesn't surface
to the user — the in-memory value is what the UI shows.

#### Frontend payload (`app/src/stores/chat.ts`)

```typescript
/** F5: per-message latency breakdown measured by the
 *  frontend around the SSE event boundaries of one chat
 *  invocation. Mirrors the `MessageRow.ttfb_ms` / `gen_ms`
 *  / `total_ms` columns in the DB and the Rust
 *  `MessageLatency` struct. */
export interface LatencyInfo {
  ttfbMs?: number;
  genMs?: number;
  totalMs?: number;
}

export interface ToolResultInfo {
  toolUseId: string;
  content: string;
  isError: boolean;
  /** F5: per-tool wall-clock duration in ms. Embedded in
   *  the persisted `tool_result` block as `duration_ms`
   *  (per R2 / ADR-lite decision 1). The ToolCallCard
   *  displays "0.3s" next to the status text when set. */
  durationMs?: number;
}

export interface ChatMessage {
  // ... existing fields ...
  /** F5: per-message latency breakdown. Rehydrated from
   *  the `messages.ttfb_ms` / `gen_ms` / `total_ms` columns
   *  on session load; the controller populates it during
   *  streaming (via `Date.now()` deltas) and fires
   *  `update_message_latency` IPC at `done` to persist. */
  latency?: LatencyInfo;
  /** F5: the seq the agent loop assigned to this row. Used
   *  by the `update_message_latency` IPC to look up the
   *  SQLite id via `find_message_id_by_seq`. Set during
   *  rehydrate (from `messages.seq`). */
  seq?: number;
}
```

### 3. Contracts

#### Measurement boundary (frontend `Date.now()`)

Three timestamps are captured on the `RequestState`:

| Timestamp | When set | Source event |
|---|---|---|
| `sendAt` | `startRequest` | Send click |
| `firstDeltaAt` | First `delta` event of the chat | `ChatEvent::Delta` |
| `doneAt` | `done` / `error` event | `ChatEvent::Done` / `Error` |

The three millisecond values are derived at `done` /
`error` time:

```typescript
const ttfbMs = firstDeltaAt !== null ? firstDeltaAt - sendAt : null;
const genMs = firstDeltaAt !== null ? doneAt - firstDeltaAt : null;
const totalMs = doneAt - sendAt;
```

`ttfbMs` and `genMs` are `null` when no `delta` event ever
arrived (e.g. the LLM returned `end_turn` immediately on
a no-op prompt — pathological but defensive). `totalMs`
is always set. The cancel path records `totalMs` only
(no `delta` arrived between cancel and `done`).

#### Tool duration measurement

Two timestamps are captured per tool on the
`RequestState.toolStartedAt: Map<tool_use_id, number>`:

| Timestamp | When set | Source event |
|---|---|---|
| `toolStartedAt.get(id)` | `tool:call` event | `ChatEvent::ToolCall` (or the `tool:call` channel event from the agent loop) |
| `now` | `tool:result` event | `tool:result` channel event |

The duration is `Date.now() - toolStartedAt.get(id)` at
`tool:result` time. The result is:
1. Patched onto the in-memory `toolResult.durationMs` (UI sees it immediately).
2. Sent to the `record_tool_duration` IPC (DB sees it
   on reload).
3. Embedded in the `tool_result` block as `duration_ms`
   (no schema change — see R2).

#### Persistence order (round-trip invariant)

1. Frontend controller's `done` handler computes the three
   latency values, writes them to `last.latency`
   (in-memory), and updates the per-session cumulative
   total via `accumulateLatency` (so the ChatPanel footer
   updates in the same tick).
2. The controller stashes the latency on
   `req.latencyPending` (the request state, in a
   separate `completedRequests` Map so the synchronous
   `finalizeRequest` cleanup doesn't drop it before the
   async IPC fires).
3. `reloadAfterFinalize` runs as part of `finalizeRequest`.
   It loads the messages from DB (which gives us the
   assistant row's `seq`), fires the
   `update_message_latency` IPC with the seq, and
   drops the `completedRequests` entry.

The agent loop emits `done` AFTER `persist_turn` returns
(`agent::chat::chat` lines around the `Done` event
emission), so the seq is stable by the time the
controller's reload sees it. A cancel path persists a
synthetic assistant turn (the same seq-based pipeline),
so the IPC also works for cancelled turns.

#### Wire format

```typescript
// invoke("update_message_latency", { ... })
{
  sessionId: "...",
  seq: 3,
  ttfbMs: 420,    // number | null
  genMs: 2100,    // number | null
  totalMs: 3200,  // number (always)
}

// invoke("record_tool_duration", { ... })
{
  sessionId: "...",
  toolUseId: "toolu_abc",
  durationMs: 250,
}

// IPC return value
true   // patched / found
false  // seq unknown / no matching tool_result block
```

The `false` return is a defensive no-op, NOT an error.
The frontend treats it as a benign outcome (logged but
not surfaced).

#### Tool duration embed-in-JSON (R2)

The `record_tool_duration` IPC patches the `tool_result`
block in `messages.content` JSON:

```jsonc
// Before patch
{
  "type": "tool_result",
  "tool_use_id": "toolu_abc",
  "content": "...",
  "is_error": false
}

// After patch
{
  "type": "tool_result",
  "tool_use_id": "toolu_abc",
  "content": "...",
  "is_error": false,
  "duration_ms": 250
}
```

The patch is a single object mutation; the rest of the
`content` array (other blocks in the same message) and
other message columns are untouched. The
`rehydrateMessages` path reads `duration_ms` off the
block on session load — the value is available in the
ToolCallCard immediately on reload, without any extra
IPC.

### 4. Validation & Error Matrix

| Condition | Result |
|---|---|
| `update_message_latency` called for a `(session_id, seq)` pair with no matching row | IPC returns `Ok(false)` (no error); frontend logs |
| `record_tool_duration` called for a `tool_use_id` not in any persisted `tool_result` block | IPC returns `Ok(false)` (no error); frontend logs |
| `record_tool_duration` called for a `tool_use_id` only present in a tool_result block that's already been patched | Idempotent: the patch overwrites the same value |
| `record_tool_duration` for a tool result block on an assistant row (orphan-repair or synthetic-on-cancel) | The patch lands on the matching block; the block's `tool_use_id` is the discriminator |
| Cancel mid-stream (no `delta` arrived) | `totalMs` recorded, `ttfbMs` / `genMs` are `null`, UI shows "—" for them in the hover tooltip |
| Error mid-stream (network / API error) | Same as cancel: `totalMs` recorded, others `null` |
| User clock change causes `Date.now() - start` to go negative | Rehydrate clamps `duration_ms` to 0 (defensive — see `rehydrateMessages` clamp logic) |
| Pre-F5 session loaded | All three columns are `NULL` on the message rows; UI shows "—" with no tooltip; `latency` is `undefined` on the in-memory `ChatMessage` |
| Brand-new session (no LLM turn yet) | `sessionTotalLatencyMs.get(sid)` is `undefined`; UI shows "—" with the "升级前未统计" tooltip |
| Page reload after N turns | `load_session` returns rows with `ttfb_ms` / `gen_ms` / `total_ms` set; `rehydrateMessages` rebuilds `latency` per message; `ensureLoaded` sums `totalMs` over assistant rows and calls `accumulateLatency` (single seed call); subsequent turns add on top |
| Session switch mid-stream | The in-flight request keeps running on the backend; the controller's listener routes events to the matching `request_id` regardless of the user's current view. When the user returns, the cumulative is up-to-date |

### 5. Good / Base / Bad Cases

#### Good: Anthropic happy path

1. User sends "explain closure"; controller records
   `sendAt = T0`.
2. Backend streams:
   - `start` event
   - `delta` × N: controller records `firstDeltaAt = T0+0.4s`
     on the first one.
   - `done` with `stop_reason: "end_turn"`, no usage.
3. Controller's `done` handler:
   - `doneAt = T0+3.2s`
   - `ttfbMs = 400`, `genMs = 2800`, `totalMs = 3200`
   - In-memory: `last.latency = { ttfbMs: 400, genMs: 2800, totalMs: 3200 }`
   - Cumulative: `sessionTotalLatencyMs.get(sid) += 3200`
   - Stash: `req.latencyPending = { ttfbMs: 400, genMs: 2800, totalMs: 3200 }`
4. `reloadAfterFinalize` fires (synchronous cleanup
   happened already):
   - `load_session` returns the assistant row with `seq = 3`.
   - IPC: `update_message_latency({ sessionId, seq: 3, ttfbMs: 400, genMs: 2800, totalMs: 3200 })`.
5. UI: assistant bubble shows "3.2s" at the bottom-right;
   hover tooltip shows three lines.

#### Good: OpenAI same flow

The timing is protocol-agnostic — Anthropic and OpenAI
both emit a `delta` event on the first content byte, and
both emit a `done` (or equivalent) at the end. The TTFB
/ gen / total math is identical. The cross-protocol
strip doesn't affect the chat-event channel.

#### Good: per-tool duration

1. User asks the LLM to read a file; the LLM emits
   `tool_use` with `id: "toolu_abc"`.
2. Controller's `tool:call` handler:
   `req.toolStartedAt.set("toolu_abc", T0)`.
3. Backend executes the tool and emits `tool:result` at
   `T0+0.35s`.
4. Controller's `tool:result` handler:
   - `durationMs = 350`
   - In-memory: `last.toolResults.push({ ..., durationMs: 350 })`
   - IPC: `record_tool_duration({ sessionId, toolUseId: "toolu_abc", durationMs: 350 })`.
5. The backend patches the `tool_result` block in
   `messages.content` JSON.
6. UI: ToolCallCard shows "0.4s" next to the status text.

#### Base: cancel during TTFB

1. User sends "explain closure"; controller records
   `sendAt = T0`.
2. The backend hasn't started streaming yet (slow proxy).
3. User hits Stop; the agent loop bails out, persists
   the partial turn (with `usage: None`), emits `done`
   with `stop_reason: "cancelled"`.
4. Controller's `done` handler:
   - `firstDeltaAt = null` (no `delta` arrived)
   - `ttfbMs = null`, `genMs = null`, `totalMs = doneAt - sendAt` (e.g. 8000)
   - In-memory: `last.latency = { totalMs: 8000 }`
   - Cumulative: adds 8000.
   - IPC: `update_message_latency({ ..., ttfbMs: null, genMs: null, totalMs: 8000 })`.
5. UI: assistant bubble shows "8.0s" at the bottom-right;
   hover tooltip shows only the "端到端: 8.0s" line (the
   TTFB / 生成 rows are hidden because the values are
   `null`).

#### Base: pre-F5 session on first load

1. User has a session from before the F5 migration; the
   three columns are all `NULL` on the assistant rows.
2. `load_session` returns the messages with all three
   latency fields `null`.
3. `rehydrateMessages`: the `hasLatency` check fails for
   every row → no `latency` is attached to any
   `ChatMessage`.
4. `ensureLoaded`: no assistant row has
   `latency.totalMs` set → no `accumulateLatency` seed.
5. UI: assistant bubbles show "—" (no chip); the
   ChatPanel footer shows "—" with the
   "升级前未统计" tooltip.

#### Bad: stripping timing on cross-protocol switch

1. User has an active session on `claude-sonnet-4-5`;
   assistant turns have `ttfb_ms` / `gen_ms` / `total_ms`
   in the DB.
2. User switches the default model to `gpt-4o` and
   sends a new message.
3. (Anti-pattern) The new LLM call's history is filtered
   to drop the timing columns; the in-memory
   `ChatMessage` loses its `latency`.
4. UI: the assistant bubble no longer shows the
   "3.2s" chip, even though the data is in the DB.
5. **Fix (F5 doesn't do this)**: F5 timing is purely a
   display concern; the timing columns live on the
   message row, not in the wire payload. They survive
   model switches naturally. The wire layer's cross-
   protocol strip is for `tool_use` / `thinking` blocks
   — `ttfb_ms` / `gen_ms` / `total_ms` are per-message
   metadata, not blocks, so they're untouched.

#### Bad: per-tool duration lost on assistant row only

1. The agent loop emits `tool_use`, the tool runs, the
   `tool:result` event fires, the controller patches the
   `tool_result` block in memory and fires the
   `record_tool_duration` IPC.
2. (Pre-fix anti-pattern) The IPC was hard-coded to look
   only at user-role `tool_result` blocks (because the
   rehydrate merge step moves them onto the assistant
   message AFTER the IPC fires).
3. Result: the timing patch lands on a user-role row's
   `content` JSON, but the UI reads from the assistant
   message's `toolResults` (the merged view), which has
   no `durationMs` because the patch went to the wrong
   row.
4. **Fix (F5's `record_tool_duration` walks ALL rows)**:
   the function searches every `tool_result` block in
   the session (user-role and assistant-role rows) for
   the matching `tool_use_id`. Whichever row holds the
   block gets the patch. The 2013 orphan-repair row is
   also covered (its `tool_result` blocks are valid
   candidates for the patch).

### 6. Tests Required

#### Backend (`cargo test`)

| Test | Asserts |
|---|---|
| `persist_turn_with_latency_writes_three_columns` | `persist_turn` with `Some(&MessageLatency)` writes all three INTEGER columns |
| `persist_turn_with_no_latency_leaves_columns_null` | `persist_turn` with `None` keeps all three columns NULL (tool_result rows, pre-F5 callers) |
| `update_message_latency_patches_columns_by_id` | The IPC's backend function writes the three columns when given an id |
| `update_message_latency_accepts_partial_payload` | Cancel / error paths with `ttfb_ms = None`, `gen_ms = None` work; NULLs are written, not 0 |
| `find_message_id_by_seq_returns_none_for_unknown_pair` | Defensive: a race between controller IPC and agent loop persist returns `None` |
| `record_tool_duration_patches_matching_tool_result_block` | The function finds the matching `tool_use_id` in `messages.content` JSON, writes `duration_ms` on the block, leaves the rest of the array untouched |
| `record_tool_duration_returns_false_when_no_block_matches` | A `tool_use_id` not in any persisted block returns `Ok(false)`, no error |
| `record_tool_duration_handles_text_only_message_without_error` | A text-only message has no `tool_result` blocks; the function returns `Ok(false)` cleanly |

#### Frontend (`pnpm test`)

| Test | Asserts |
|---|---|
| `rehydrateMessages — F5 latency rehydration > populates the latency triple on an assistant message that has all three values` | `rehydrateMessages` builds `latency: { ttfbMs, genMs, totalMs }` for a fully-set row |
| `rehydrateMessages — F5 latency rehydration > omits latency when all three columns are NULL (pre-F5 rows)` | The `hasLatency` check correctly omits the field |
| `rehydrateMessages — F5 latency rehydration > includes only the non-NULL fields in a partial-latency row` | Cancel-path latency with only `totalMs` set is rendered with just the total |
| `rehydrateMessages — F5 per-tool duration rehydration > reads duration_ms off a persisted tool_result block` | The merge step + the per-block read both surface `durationMs` |
| `rehydrateMessages — F5 per-tool duration rehydration > leaves durationMs undefined when the field is missing (pre-F5 rows)` | Pre-F5 blocks render no time |
| `rehydrateMessages — F5 per-tool duration rehydration > rounds fractional durationMs to an integer` | Defensive round |
| `rehydrateMessages — F5 per-tool duration rehydration > clamps negative durationMs to 0 (defensive against clock skew)` | Pathological user clock change doesn't break the UI |
| `abbreviateDuration — formats sub-second durations with one decimal` | `0.4s`, `1.0s`, `999ms → 1.0s` |
| `abbreviateDuration — formats sub-minute durations with one decimal` | `1.0s`, `3.2s`, `59.9s` |
| `abbreviateDuration — switches to 'Xm Ys' format past 60 seconds` | `1m 0s`, `1m 23s`, `12m 4s` |
| `abbreviateDuration — formats the seconds portion with one decimal only when fractional` | `1m 30s` not `1m 30.0s`; `1m 30.5s` for fractional |
| `abbreviateDuration — clamps negative inputs to 0.0s` | Defensive against clock skew |
| `abbreviateDuration — clamps NaN / Infinity to 0.0s` | Defensive against buggy upstreams |

#### Existing 2013 / A4 invariants (must continue to pass)

| Test | Asserts |
|---|---|
| `finalizeRequest (06-08-06-08 step-4 follow-up — 2013 wire invariant) > evicts the in-memory message buffer and unloads from DB cache` | `pinnedSessions` is cleared on `finalizeRequest` (the synchronous part of the F5 contract) |
| `finalizeRequest (06-08-06-08 step-4 follow-up — 2013 wire invariant) > invalidates the chat store's diff cache for the same session` | `invalidateDiff` still runs (paired invariant) |
| `finalizeRequest (06-08-06-08 step-4 follow-up — 2013 wire invariant) > both actions fire on the same finalizeRequest call (paired invariant)` | `pinnedSessions` clear + `diffCache` clear happen in the same synchronous tick |

The 2013 tests' buffer-clear assertions were updated
in F5 (they no longer assert `messagesBySession` is
cleared synchronously — that's now `reloadAfterFinalize`'s
async job). The synchronous contract that
`finalizeRequest` owns is the `pinnedSessions` /
`activeRequests` cleanup.

### 7. Wrong vs Correct

#### Wrong: timing on the backend (Instant::now / SystemTime)

```rust
// BAD — Rust side timing. Requires plumbing the
// SystemTime through every layer (agent loop → DB →
// IPC). Duplicates what the frontend already measures.
let start = SystemTime::now();
// ... do work ...
let elapsed = start.elapsed().unwrap();
db::update_message_latency(pool, message_id, &MessageLatency {
    ttfb_ms: Some(elapsed.as_millis() as i64 - 400),
    // ...
});
```

The backend already has timing (it logs the
`request_id` and `done` event latency in
`tracing::info!`). But the *measurement boundary* the
user cares about is "send click → first delta on
screen", which the frontend can measure more
accurately (the network round-trip from the
`tauri::async_runtime::spawn` IPC is the user's
perceived latency). The backend would over-count the
spawn overhead and miss the client-side render.

#### Correct: timing on the frontend (`Date.now()`)

```typescript
// GOOD — frontend deltas, persisted via IPC.
const sendAt = Date.now();
// First `delta`:
const firstDeltaAt = Date.now();
// `done`:
const doneAt = Date.now();
const ttfbMs = firstDeltaAt - sendAt;
const totalMs = doneAt - sendAt;
```

Single source of truth, same as A4 token usage. The
backend's role is persistence (the IPC + DB write), not
measurement.

#### Wrong: tool duration as a new table

```sql
-- BAD — adds a `tool_durations` table with a foreign key
-- to messages, a composite key on (session_id, tool_use_id).
CREATE TABLE tool_durations (
  session_id TEXT NOT NULL,
  tool_use_id TEXT NOT NULL,
  duration_ms INTEGER,
  PRIMARY KEY (session_id, tool_use_id)
);
```

Adds migration churn (a new table + indexes + an
on-delete-cascade constraint), rehydrate path complexity
(a third join in `load_session`), and complicates the
2013 orphan-repair logic (a duration for an orphan
tool_use needs to be cleaned up too). For one number
per tool, the cost outweighs the benefit.

#### Correct: tool duration embedded in `messages.content` JSON

```rust
// GOOD — single object mutation on the existing
// `content` JSON. The block is already there (it carries
// `tool_use_id` and `content`); adding one field is
// one INSERT-or-UPDATE.
obj.insert("duration_ms".to_string(), serde_json::Value::Number(duration_ms.into()));
```

Zero schema change. Rehydrate reads `duration_ms` off
the same block it's already walking. The 2013
orphan-repair flow is unaffected.

#### Wrong: latency columns on a separate `latencies` table

```sql
-- BAD — separate table for the three columns, joined by
-- message id. Rehydrate now has two round trips + a
-- join; the in-memory representation needs a second
-- Map.
CREATE TABLE message_latencies (
  message_id INTEGER PRIMARY KEY,
  ttfb_ms INTEGER,
  gen_ms INTEGER,
  total_ms INTEGER
);
```

Same as the tool-duration anti-pattern, but worse:
the three columns are **per-message** (not per-tool),
and every `load_session` rehydrate would need the
join. The columns are part of the message metadata;
they belong on the same row.

#### Correct: nullable INTEGER columns on `messages`

```sql
-- GOOD — three nullable columns on the existing
-- `messages` row. `load_session`'s SELECT picks them up
-- for free. NULL semantics align with the in-memory
-- `latency?: LatencyInfo` (absent = no timing).
ALTER TABLE messages ADD COLUMN ttfb_ms INTEGER;
ALTER TABLE messages ADD COLUMN gen_ms INTEGER;
ALTER TABLE messages ADD COLUMN total_ms INTEGER;
```

Pre-F5 rows keep NULL → the rehydrate path's
`hasLatency` check correctly omits the in-memory
`latency` field, and the UI shows "—". Post-F5 rows
have all three values set by the IPC.

#### Wrong: include timing in the wire payload

```typescript
// BAD — wire payload grows by 3 numbers per message.
// Doubles the LLM-side per-turn token usage payload
// (Anthropic charges input tokens, so we pay for this
// twice on the cache hit + the inbound round).
const payload = {
  role: "assistant",
  content: [...],
  // NO! timing is per-message UI metadata, not LLM state
  latency: { ttfbMs: 400, genMs: 2800, totalMs: 3200 },
};
```

Anthropic charges for input tokens including cached
content. Embedding latency in the wire payload would
add 30-50 tokens per turn that Anthropic re-parses on
every rehydrate (cache_control: ephemeral caches the
instructions, not the assistant turns).

#### Correct: timing stays in the DB / in-memory

```typescript
// GOOD — the in-memory `ChatMessage` carries the
// `latency` field for the UI, but the outbound wire
// payload (built by `toPayloadContent` in chat.ts) does
// NOT emit it. The DB column is for rehydrate on next
// session load, not for LLM round-trip.
```

The wire payload stays the same (4 fields: text /
thinking / tool_use / tool_result). The DB column is
for rehydrate. The in-memory field is for the UI. Three
disjoint concerns, three disjoint storage paths.

### Design Decisions

#### Decision: Tool duration embedded in `tool_result` JSON (R2, locked 2026-06-11)

**Context**: The original F5 spec (in the
`06-11-session-loading` archive task) assumed a
separate `tool_results` table. The actual DB schema
embeds `tool_result` blocks in `messages.content`
JSON.

**Decision**: Tool-use-id-scoped `durationMs` is
written onto the `tool_result` block in
`messages.content` JSON. The backend uses
`serde_json::Value::pointer_mut` (via the
`record_tool_duration` function) to patch the
matching block. New IPC `record_tool_duration(session_id,
tool_use_id, duration_ms)`.

**Consequences**:
- Zero schema change for the tool side; only the 3
  column ALTERs for R3 (TTFB / gen / total).
- Rehydrate path is zero-modification (the function
  already walks the `content` array, picking up
  `duration_ms` along the way).
- Trade-off: `content` JSON gains one field per
  tool_result block. ~25 bytes per tool call —
  negligible.
- The 2013 orphan-repair flow is unaffected
  (orphan-repaired `tool_result` blocks are
  identical to live ones; the IPC patches them by
  `tool_use_id` lookup).

#### Decision: Frontend `Date.now()` timing (ADR-lite, locked 2026-06-11)

**Context**: A4 token usage is also frontend-computed;
`test_provider` has `latencyMs` but that's a single
HTTP probe (not the per-message timing the user
wants to see).

**Decision**: F5 measures everything on the
frontend. R1's three values (TTFB / gen / total) and
R2's per-tool duration are all `Date.now()` deltas.
The backend's only role is persistence (the
`update_message_latency` and `record_tool_duration`
IPCs) — no `Instant::now()` / `SystemTime` calls in
the agent loop or the SSE parser.

**Consequences**:
- Consistency with A4 (single source of truth = the
  frontend).
- No new system-clock coupling between Rust and
  TypeScript; the agent loop stays timing-agnostic.
- Known limitation: a user who changes their system
  clock mid-stream will see weird numbers (negative
  TTFB, etc.). The rehydrate path clamps to 0
  (defensive). Same trade-off as A4 — acceptable.
- The `request_id`-based event routing in the
  controller means a single request's timing stays
  coherent even when the user switches sessions
  mid-stream.

#### Decision: Per-message IPC + cumulative in-memory (matches A4)

**Context**: A4's token usage has per-session
cumulative (`sessions.*_total` columns +
`tokenUsageBySession` map). F5 follows the same
shape for the latency cumulative (in-memory
`sessionTotalLatencyMs` map, no schema column for
the cumulative).

**Decision**: The cumulative is a frontend-only
projection. The DB stores the per-message
`ttfb_ms` / `gen_ms` / `total_ms` columns; the
`sessionTotalLatencyMs` map is `Σ totalMs WHERE
role = 'assistant' AND totalMs IS NOT NULL`,
rehydrated on session load.

**Consequences**:
- Reload after N turns shows the cumulative
  immediately (seeded from the messages in
  `ensureLoaded`).
- No `sessions.total_latency_ms` column needed
  (the SUM is trivial; the DB doesn't have to keep
  the running total).
- The cumulative updates synchronously on `done`
  (the chat store's `accumulateLatency` runs in the
  same tick as the in-memory message update), so
  the ChatPanel footer reflects the new value
  without an extra IPC.

#### Decision: 1 PR all-in (locked 2026-06-11)

**Context**: R1-R8 are tightly coupled (timing
measurement → in-memory mutation → IPC fire →
DB column write → rehydrate path → UI rendering).
A 2-PR split would have an un-runnable middle state.

**Decision**: 1 PR for the whole F5. Same pattern as
A4 (LLM usage parsing + DB schema + agent loop + UI
+ spec + decision log).

**Consequences**: 8-12 file diff (Rust 4-5 + Vue
3-4 + spec 1 + docs 1). Review difficulty rises;
commit message must list all touched concerns.

### Future Work (Deferred from F5)

| Item | Why deferred |
|---|---|
| P50 / P95 latency stats per session | Out of scope (PRD OOS #1). The user wants to *see* their LLM calls, not analyze them statistically. |
| Historical trend chart across sessions | Out of scope (PRD OOS #2). |
| CSV / JSON export of latency data | Out of scope (PRD OOS #3). |
| Backend-precise timing (Rust `Instant::now()`) | PRD ADR-lite decision 2. |
| Token rate (tokens/second) | Out of scope (PRD OOS #5). |
| Per-model / per-provider latency breakdown | Out of scope (PRD OOS #6). |
| Cross-session global cumulative | Out of scope (PRD OOS #7). |
| Per-session latency in SessionList sidebar row | Out of scope (PRD OOS #8). |
| Persist the cumulative total in a `sessions.total_latency_ms` column | Not needed — the SUM is trivial; in-memory is fine. |
| `update_message_latency` for tool_result rows | Tool-result rows have no per-message latency triple (per-tool duration lives in the JSON, not the columns). The IPC is only called for assistant turns. |
| LLM-claimed TTFB (parse from `usage.creation_time` if exposed) | Anthropic doesn't expose this on `message_delta`; the frontend's `Date.now()` is the only source. |

### Per-Turn Tracking (F5 follow-up, 2026-06-12)

F5 ships per-`RequestState` (one request = one LLM stream invocation), which works for the single-turn case but silently corrupts multi-turn agent responses: only the LAST assistant row's latency columns get written. This follow-up scopes the per-turn shape explicitly. The "Known Limitations: Per-turn latency only captured for the LAST assistant message" section that documented the bug has been **removed** — the bug is fixed by the changes below.

#### Wire format — new `ChatEvent::TurnComplete` variant

A new `ChatEvent` variant carries per-turn latency, emitted by the agent loop AFTER each `persist_turn` for an assistant row. Payload (mirrors `db::MessageLatency` plus the row's `seq`):

```rust
TurnComplete {
    seq: i64,                          // assistant row seq, written by persist_turn
    ttfb_ms: Option<i64>,              // first_delta_at - turn_send_at
    gen_ms: Option<i64>,               // done_at - first_delta_at
    total_ms: Option<i64>,             // done_at - turn_send_at
    thinking_ms: Option<i64>,          // turn_thinking_done - turn_thinking_start
}
```

`seq` is the per-turn row handle (assigned by the agent loop in `app/src-tauri/src/agent/chat.rs` from the per-session `next_seq` counter). The 4 ms fields are `Option` so a turn that never reached the relevant boundary (e.g. thinking-only turn cut by a `tool:call`) still serializes cleanly. Wire transport: same `chat-event` channel as every other variant (the Tauri `emit("chat-event", payload)` in `app/src-tauri/src/agent/helpers.rs` `emit_chat_event`), discriminator is `"kind": "turn_complete"`.

#### Backend — `persist_turn` writes the 4 columns in one INSERT

`app/src-tauri/src/agent/chat.rs` outer loop tracks 4 per-turn `Option<Instant>` locals per iteration:

- `turn_send_at` — set right before `provider.send(...)`
- `turn_first_delta_at` — set on the first `ChatEvent::Delta` for this turn
- `turn_thinking_start` — set on the first `ChatEvent::ThinkingDelta` for this turn
- `turn_thinking_done` — set on the first non-thinking boundary (text `Delta`, `ToolCall`, `Done`, or `Error`)

`ChatEvent::Start` no longer has the `if turn == 1` guard (`app/src-tauri/src/agent/chat.rs:422-425`) — every turn emits Start so the frontend can key its `latencyByTurn` per turn reliably.

At the `persist_turn` call site (line 600-607) the existing `latency: Option<&MessageLatency>` parameter is filled with `Some(&MessageLatency { ttfb_ms, gen_ms, total_ms, thinking_ms })` derived from the 4 Instants. The INSERT statement (`app/src-tauri/src/db/sessions.rs:565-595`) already binds all 4 columns — F5 added `thinking_ms` on 2026-06-12. Per-turn rows therefore get all 4 columns populated atomically, no follow-up `UPDATE` needed for the common case.

Right after each successful `persist_turn` (assistant row), the loop emits `ChatEvent::TurnComplete { seq, ttfb_ms, gen_ms, total_ms, thinking_ms }`. Cancel-mid-turn and cancel-during-tool-exec paths also fire TurnComplete for whatever assistant row they persisted. The `MAX_TURNS = 20` safety net does NOT fire TurnComplete (it never persists).

The final `ChatEvent::Done` emit (line 660-679, gated on `!should_continue`) is unchanged — it still terminates the stream and carries `stop_reason` + `usage`. Per-turn and stream-terminating events are conceptually distinct; collapsing them would muddy the wire contract.

#### Frontend — `RequestState` keys per turn, not per request

`app/src/stores/streamController.ts` `RequestState` (lines 56-120) drops the per-request single-value fields:

- **Removed**: `latencyPending: { ttfbMs, genMs, totalMs } | null` (line 111)
- **Removed**: per-request single-value `thinkingDurationMs: number | null` (line 96)

…in favor of two new fields:

- `currentTurnIndex: number` — bumped in the `case "start"` arm of `handleChatEvent`'s switch (line 637-640)
- `latencyByTurn: Map<number, TurnLatency>` — keyed by `currentTurnIndex`, where `TurnLatency = { seq, ttfbMs, genMs, totalMs, thinkingMs }` mirrors the Rust `TurnComplete` payload

The 4 close-boundary sites that snapshot `thinkingDurationMs` (text `delta` line 661, `tool:call` line 856, `done` line 715, `error` line 805) keep their single-value `thinkingStartedAt` / `thinkingDurationMs` locals as PER-TURN timer state — they're reset on every `Start` event, not on `startRequest`. The new `case "turn_complete"` arm writes to `latencyByTurn.set(currentTurnIndex, ...)` AND in-place mutates the reactive placeholder's `latency` / `thinkingDurationMs` so `currentSessionLatencyTurns` (in `chat.ts`) updates in real-time per turn, with no reload.

`accumulateLatency(req.sessionId, totalMs)` moves from the `done` handler (line 743) to the `turn_complete` handler — same A4 `accumulateTokenUsage` per-done pattern, just one event earlier and fired N times per request instead of once.

#### Re-attach — fire N `update_message_latency` IPCs per request

`reloadAfterFinalize` (line 974-1113) iterates `req.latencyByTurn` and fires one `update_message_latency` IPC per entry, keyed by `lat.seq` (not by "max seq" of all assistant rows as in the F5 path). The in-place mutate loop is `m.seq === lat.seq` (per-turn) instead of "max seq" (per-request). `cancel` / `error` paths go through the same `reloadAfterFinalize` and naturally fire N IPCs for whatever turns had a `TurnComplete` arrive before the cancel/error.

`update_message_latency` IPC signature is unchanged (F5 + 2026-06-12 already takes `(sessionId, seq, ttfbMs, genMs, totalMs, thinkingMs)`). The 4-column `UPDATE` in `app/src-tauri/src/db/sessions.rs:662-677` is also unchanged — it's just called N times instead of once.

