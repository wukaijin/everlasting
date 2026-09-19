<!-- Moved from token-usage-tracking.md 2026-09-19 (doc-split) -->

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

