<!-- Latency tracking scenario. Moved from llm-contract.md 2026-06-21 (F5, 2026-06-11) -->

# Latency Tracking (F5, 2026-06-11)

> **Source**: extracted from `.trellis/spec/backend/llm-contract.md` §"Scenario: Latency Tracking" (2026-06-21 doc-trim task).
>
> **Cross-references**:
> - Main LLM contract: [llm-contract.md](./llm-contract.md)

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

> **分篇**(2026-09-19):本文保留 §1 Scope 与 §6 Tests Required;§2-§5、§7、Design Decisions/Future Work 与 Per-Turn follow-up 已按 tool-contract 模式拆至 `latency-tracking/` 子目录(一节一文件,原锚点以 stub 保留)。

### 2. Signatures

> **已拆出**(2026-09-19 doc-split):完整签名见 [`latency-tracking/signatures-contracts.md`](./latency-tracking/signatures-contracts.md)(含 §3 Contracts 与 §4 Validation & Error Matrix)。

### 3. Contracts

> **已拆出**(2026-09-19 doc-split):见 [`latency-tracking/signatures-contracts.md`](./latency-tracking/signatures-contracts.md)。

### 4. Validation & Error Matrix

> **已拆出**(2026-09-19 doc-split):见 [`latency-tracking/signatures-contracts.md`](./latency-tracking/signatures-contracts.md)。

### 5. Good / Base / Bad Cases

> **已拆出**(2026-09-19 doc-split):完整用例见 [`latency-tracking/good-base-bad-cases.md`](./latency-tracking/good-base-bad-cases.md)。
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
| `abbreviateDuration — formats sub-10s durations with one decimal` | `0.4s`, `1.5s`, `9.9s` |
| `abbreviateDuration — drops the decimal at 10s and above` | `10s`, `32s`, `54s`(2026-08-29 ladder 修订:原"sub-minute 一律一位小数"渲出 `54.0s` 尾巴) |
| `abbreviateDuration — switches to 'Xm Ys' format past 60 seconds` | `1m 23s`, `12m 4s` |
| `abbreviateDuration — compacts whole minutes to '5m' (not '5m 0s')` | `1m`, `5m`, `59m 59s` |
| `abbreviateDuration — switches to 'Xh Ym' format past 60 minutes` | `1h`, `1h 1m`, `2h 1m`(秒在小时档视为噪声) |
| `abbreviateDuration — clamps negative inputs to 0s` | Defensive against clock skew |
| `abbreviateDuration — clamps NaN / Infinity to 0s` | Defensive against buggy upstreams |

> **2026-08-29 ui-visual-polish 修订**:格式从"sub-minute 一律一位小数"
> 改为阶梯 `<10s 一位小数 → 整秒 → 分 → 时`。起因:子代理 drawer 长跑
> (statusDisplay / DrawerSection liveChip 原各自裸 `toFixed(1)`)渲出
> `14400.0s`;现在四处耗时显示(主聊天 footer chip、ToolCallCard header、
> drawer status pill、drawer section live chip)统一走
> `utils/duration.ts#abbreviateDuration` 单一出处。同批:ToolOutputBody
> summary 行的重复耗时 chip 删除(header 已渲同一 `result.durationMs`)。

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

> **已拆出**(2026-09-19 doc-split):完整对照见 [`latency-tracking/wrong-vs-correct.md`](./latency-tracking/wrong-vs-correct.md)。

### Design Decisions

> **已拆出**(2026-09-19 doc-split):完整决策记录(含 Future Work,Deferred from F5)见 [`latency-tracking/design-decisions.md`](./latency-tracking/design-decisions.md)。

### Future Work (Deferred from F5)

> **已拆出**(2026-09-19 doc-split):见 [`latency-tracking/design-decisions.md`](./latency-tracking/design-decisions.md)。

### Per-Turn Tracking (F5 follow-up, 2026-06-12)

> **已拆出**(2026-09-19 doc-split):完整契约见 [`latency-tracking/per-turn-tracking.md`](./latency-tracking/per-turn-tracking.md)。
