<!-- Token usage tracking scenario. Moved from llm-contract.md 2026-06-21 (A4, 2026-06-10) -->

# Token Usage Tracking (A4, 2026-06-10)

> **分篇**(2026-09-13):本文保留 A4 核心契约 + OOS 边界;后续追加的 11 个 Scenario 已按 tool-contract 模式拆至 `token-usage-tracking/` 子目录(一 Scenario 一文件,原锚点以 stub 保留在本文对应位置)。

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

> **二次分篇**(2026-09-19):A4 主 Scenario 内文再拆——§2-§4(签名/契约/校验矩阵)、§5+§7(用例与对照)、Design Decisions 分别拆至 `12-a4-signatures-contracts.md` / `13-a4-cases-and-wrong-vs-correct.md` / `14-a4-design-decisions.md`(编号顺延既有 01-11);本文保留 §1 Scope、§6 Tests Required、顶部 snapshot 重构注记与 OOS 边界,原锚点以 stub 保留。

### 2. Signatures

> **已拆出**(2026-09-19 doc-split):完整签名与契约(§2 Signatures / §3 Contracts / §4 Validation & Error Matrix)见 [`token-usage-tracking/12-a4-signatures-contracts.md`](./token-usage-tracking/12-a4-signatures-contracts.md)。

### 3. Contracts

> **已拆出**(2026-09-19 doc-split):见 [`token-usage-tracking/12-a4-signatures-contracts.md`](./token-usage-tracking/12-a4-signatures-contracts.md)。

### 4. Validation & Error Matrix

> **已拆出**(2026-09-19 doc-split):见 [`token-usage-tracking/12-a4-signatures-contracts.md`](./token-usage-tracking/12-a4-signatures-contracts.md)。

### 5. Good / Base / Bad Cases

> **已拆出**(2026-09-19 doc-split):完整用例见 [`token-usage-tracking/13-a4-cases-and-wrong-vs-correct.md`](./token-usage-tracking/13-a4-cases-and-wrong-vs-correct.md)。
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

> **已拆出**(2026-09-19 doc-split):完整对照见 [`token-usage-tracking/13-a4-cases-and-wrong-vs-correct.md`](./token-usage-tracking/13-a4-cases-and-wrong-vs-correct.md)。

### Design Decisions

> **已拆出**(2026-09-19 doc-split):完整决策记录见 [`token-usage-tracking/14-a4-design-decisions.md`](./token-usage-tracking/14-a4-design-decisions.md)。
## Scenario: Group-Chat Per-Speaker Cache Rate (2026-08-10, task 08-10-group-chat-cache-rate)

> **已拆出**(2026-09-13 doc-split):完整契约见 [`token-usage-tracking/01-group-chat-cache-rate.md`](./token-usage-tracking/01-group-chat-cache-rate.md)。

## Scenario: tools[] Token Measurement + Static Pruning (C7, 2026-08-14)

> **已拆出**(2026-09-13 doc-split):完整契约见 [`token-usage-tracking/02-tools-token-static-pruning.md`](./token-usage-tracking/02-tools-token-static-pruning.md)。

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

> **已拆出**(2026-09-13 doc-split):完整契约见 [`token-usage-tracking/03-tools-stub-registration.md`](./token-usage-tracking/03-tools-stub-registration.md)。

## Scenario:memory 指令块度量 + digest(WP1/WP2,2026-08-15)

> **已拆出**(2026-09-13 doc-split):完整契约见 [`token-usage-tracking/04-memory-digest-metering.md`](./token-usage-tracking/04-memory-digest-metering.md)。

## Scenario: images_token — request-total image slice (B1, 2026-08-17)

> **已拆出**(2026-09-13 doc-split):完整契约见 [`token-usage-tracking/05-images-token-b1.md`](./token-usage-tracking/05-images-token-b1.md)。

## Scenario: 摘要压缩旁路 usage(C3,2026-08-18)

> **已拆出**(2026-09-13 doc-split):完整契约见 [`token-usage-tracking/06-compaction-bypass-usage.md`](./token-usage-tracking/06-compaction-bypass-usage.md)。

## Scenario: 统一估算 + at_files/system/window 三新列 + 实发口径(unified-context-budget,2026-08-19)

> **已拆出**(2026-09-13 doc-split):完整契约见 [`token-usage-tracking/07-unified-context-budget.md`](./token-usage-tracking/07-unified-context-budget.md)。

## Scenario: worker per-turn 行 + run 维度唯一键(2026-08-20,task 08-20-worker-turn-trace-persist)

> **已拆出**(2026-09-13 doc-split):完整契约见 [`token-usage-tracking/08-worker-per-turn-trace.md`](./token-usage-tracking/08-worker-per-turn-trace.md)。

## Scenario: 工具图计费 — ToolResult.images 内联 tokens_est(08-21-b1-image-followups,2026-08-21)

> **已拆出**(2026-09-13 doc-split):完整契约见 [`token-usage-tracking/09-toolresult-images-billing.md`](./token-usage-tracking/09-toolresult-images-billing.md)。

## Scenario:tools=0 辅助请求归因判别器 + cache miss 归因次序(09-01-aux-call-cache-interference,2026-09-01)

> **已拆出**(2026-09-13 doc-split):完整契约见 [`token-usage-tracking/10-tools-zero-attribution.md`](./token-usage-tracking/10-tools-zero-attribution.md)。

## Scenario: 群聊 per-discussion / per-speaker 计费核算(GCE M4c,2026-09-08,task 09-08-gce-m4c-cost-governance-modal-redesign)

> **已拆出**(2026-09-13 doc-split):完整契约见 [`token-usage-tracking/11-group-chat-m4c-billing.md`](./token-usage-tracking/11-group-chat-m4c-billing.md)。
