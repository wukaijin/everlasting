# LLM API Contract —核心类型与思考契约

> **基线**:2026-06-10 commit `0f9a167` (8-PR5拆分后) + 2026-06-21 (doc-trim 拆 3 个 scenario) + 2026-08-10 (Extended Thinking 拆出子文件)
> **来源**:从原 `llm-contract.md` (3149 行)拆出本文件 +4 个子文件 (`tool-contract.md` / `worktree-contract.md` / `multi-provider-contract.md` / `test-model-contract.md`)
> **同源文档**:
> - [llm-contract.md](./llm-contract.md) (本文) —核心类型 + Decision 汇总(Overview / 3 Decisions;Gotcha 与 5 个 Scenario 已拆至子目录 `llm-contract/`)
> - [llm-contract/extended-thinking.md](./llm-contract/extended-thinking.md) — Extended Thinking (Step6) 完整契约(2026-08-10 拆出)
> - [tool-contract.md](./tool-contract.md) —工具定义 + ReadGuard + shell spillover
> - [worktree-contract.md](./worktree-contract.md) — attach/detach/delete + cancel + system prompt
> - [multi-provider-contract.md](./multi-provider-contract.md) — Provider trait + catalog + Anthropic/OpenAI 分发
> - [test-model-contract.md](./test-model-contract.md) — `test_model` IPC
>
> **详细 scenario**(2026-06-21 doc-trim 拆出):
> - [latency-tracking.md](./latency-tracking.md) — F5(2026-06-11)+ Per-Turn Tracking follow-up
> - [token-usage-tracking.md](./token-usage-tracking.md) — A4 核心 + 11 Scenario 分篇(2026-06-10~09-08)
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

> **分篇**(2026-09-19):本文保留 Overview 与三条核心 Decision;Gotcha(tool_use↔tool_result 原子性)与 5 个 Scenario 已按 tool-contract 模式拆至 `llm-contract/` 子目录(一 Scenario 一文件,原锚点以 stub 保留)。

## Gotcha: tool_use ↔ tool_result Pair Atomicity (C3, 2026-06-12)

> **已拆出**(2026-09-19 doc-split):完整 Gotcha 见 [`llm-contract/gotcha-tool-result-pair-atomicity.md`](./llm-contract/gotcha-tool-result-pair-atomicity.md)。

## Scenario: DeepSeek-Via-Anthropic-Relay thinking block fix (RULE-D-003, 2026-06-20)

> **已拆出**(2026-09-19 doc-split):完整契约见 [`llm-contract/scenario-deepseek-relay-fix.md`](./llm-contract/scenario-deepseek-relay-fix.md)。

## Scenario: LLM Retry / Backoff (A5+, 2026-07-05)

> **已拆出**(2026-09-19 doc-split):完整契约见 [`llm-contract/scenario-retry-backoff.md`](./llm-contract/scenario-retry-backoff.md)。

## Scenario: E2 trace ChatEvent variants are server-emitted, not LLM-streamed (E2, 2026-07-14)

> **已拆出**(2026-09-19 doc-split):完整契约见 [`llm-contract/scenario-chatevent-server-emitted.md`](./llm-contract/scenario-chatevent-server-emitted.md)。

## Scenario: Image Blocks — dual-form lifecycle (B1, 2026-08-17)

> **已拆出**(2026-09-19 doc-split):见 [`llm-contract/scenario-image-blocks-and-sse-carry.md`](./llm-contract/scenario-image-blocks-and-sse-carry.md)(与 SSE chunk-boundary UTF-8 carry、ToolResultData dual-form serde 同文件)。

## Scenario: SSE chunk-boundary UTF-8 carry (RULE, 2026-08-18)

> **已拆出**(2026-09-19 doc-split):见 [`llm-contract/scenario-image-blocks-and-sse-carry.md`](./llm-contract/scenario-image-blocks-and-sse-carry.md)。

## Scenario: Tool-Result Image Blocks — ToolResultData dual-form serde (08-21-b1-image-followups, 2026-08-21)

> **已拆出**(2026-09-19 doc-split):见 [`llm-contract/scenario-image-blocks-and-sse-carry.md`](./llm-contract/scenario-image-blocks-and-sse-carry.md)。
