<!-- Moved from memory/scenario-autonomous-memories.md 2026-09-13 (doc-split) -->

### 4. Validation & Error Matrix

| Condition | Result |
|---|---|
| `remember` with sensitive content (API key regex hit) | `insert_memory` returns `Err`; tool surfaces "rejected: sensitive content detected" |
| `remember` with content > 500 chars | `insert_memory` returns `Err`; tool surfaces "rejected: content exceeds 500 char cap" |
| `remember` with `scope=Project` but no `project_id` in context | `Err("project_id required for project scope")` |
| `remember` with `count_memories_for_session >= 50` | Tool returns "rejected: per-session cap of 50 memories reached" |
| FTS5 query empty | `build_recall_text` returns `None`; no block added; no error |
| FTS5 query non-empty, 0 matches | `build_recall_text` returns `None`; no block added; no error |
| FTS5 query N matches, sum > 500 tokens | Truncate at line boundary; newer first until budget exhausted |
| FTS5 query N matches, first entry alone > 500 tokens | Surface first entry anyway (defensive; should not happen with P2 content cap) |
| `bump_hit_count` fails (DB transient error) | `warn!`; recall text already in prompt; non-blocking |
| `delete_memory` with id not found | 0 rows affected; idempotent success |
| `delete_memory` for `status=Active`/`Verified` row (P5+) | P2 allows deletion of any status; P5+ may restrict to Candidate/Archived |
| Frontend `fetchMemories` IPC failure | `runtimeMemoriesError` set; UI shows error state |
| Frontend `deleteMemory` IPC failure | Toast / inline error; optimistic remove rolled back |
| **P3** `recall_pitfall_footnote` with `tool_name` no match in DB | Returns `Ok(None)`; no footnote; tool executes normally |
| **P3** `recall_pitfall_footnote` with active pitfall, `command_pattern` matches | Returns `Ok(Some("⚠️ Memory: ..."))`; prepended to `tool_result.content`; `bump_hit_count` fired |
| **P3** `recall_pitfall_footnote` with verified-status pitfall | Returns `Ok(None)` — `verified` is **P5 scope** (soft-intercept), P3 active-only filter strictly excludes |
| **P3** `recall_pitfall_footnote` with candidate-status pitfall | Returns `Ok(None)` — `candidate` is **P2 scope**; not yet promoted to recallable |
| **P3** `recall_pitfall_footnote` SQL `Err(sqlx::Error)` | `tracing::warn!` + `Ok(None)`; tool executes normally; **never blocks** (PRD hard rule) |
| **P3** `bump_hit_count` for pre-tool hit fails (fire-and-forget) | `warn!`; recall footnote already in tool_result; non-blocking; P5 state machine may read stale `hit_count` (acceptable) |
| **P4** `try_record_outcome` with single `is_error=true` (no prior failure for this `tool_name`) | Tracker increments to 1, no reflection triggered; subsequent success resets to 0 (PRD AC #3) |
| **P4** `try_record_outcome` with 2 consecutive `is_error=true` followed by `is_error=false` for same `tool_name` | Reflection triggered; `tokio::spawn` runs `reflect_to_pitfall`; tracker resets to 0; main loop continues immediately (PRD AC #1, #2) |
| **P4** `try_record_outcome` with success as the first event for a `tool_name` (no prior failures) | Tracker stays at 0; no reflection triggered (success is a no-op for trigger detection) |
| **P4** `try_record_outcome` with 1 failure followed by 1 success for same `tool_name` | Tracker increments to 1 on failure, resets to 0 on success (no trigger — below threshold) (PRD AC #3) |
| **P4** `try_record_outcome` per-tool isolation | `shell` failures do NOT increment `edit_file` counter; each `tool_name` has its own `TrackerEntry` |
| **P4** reflection LLM call returns non-JSON text (markdown wrapper, prose) | `strip_code_fence` + JSON parse; parse failure → `tracing::warn!` + silent drop; main loop unaffected (no panic, no `unwrap`) |
| **P4** `reflect_to_pitfall` calls `insert_memory` with sensitive content (LLM hallucinates an API key) | `insert_memory` safety net rejects (returns `Err`); `tracing::warn!` + silent drop; no row written |
| **P4** `reflect_to_pitfall` calls `insert_memory` with content > 500 chars (LLM verbose) | `insert_memory` safety net rejects; `tracing::warn!` + silent drop; no row written |
| **P4** `reflect_to_pitfall` calls `insert_memory` with `count_memories_for_session >= 50` | `insert_memory` frequency cap rejects; `tracing::warn!` + silent drop; no row written |
| **P4** reflection LLM call transient network/timeout error | `tracing::warn!` + silent drop; no retry; main loop unaffected (fire-and-forget hard rule) |
| **P4** `insert_memory` returns `Err(sqlx::Error)` (DB transient) | `tracing::warn!` + silent drop; no retry; main loop unaffected |
| **P4** P3 ↔ P4 close-the-loop | P4 writes pitfall via `insert_memory` → immediately recallable by P3's `find_pitfalls_by_trigger` (no extra index/migration; `idx_am_pitfall` already covers P4 writes) |
| **P5** verified pitfall + `is_full_match` + `memory_id ∉ already_blocked` | `recall_pitfall` 返 `SoftBlock`;chat_loop 短路 `execute_tool`、回灌 `is_error=false` 提示、记 `memory_id` 入 session set、`bump_hit_count` |
| **P5** 同坑二次命中(`already_blocked` 已含) | 降级 `Footnote` + 正常 `execute_tool`(D1 防循环,不卡 `MAX_TURNS`) |
| **P5** candidate `hit_count` 跨 2 | `promote_if_eligible` 升 active;active `hit_count` 跨 5 且 age≥3 天升 verified(嵌 `bump_hit_count` 同连接) |
| **P5** 宽泛 pitfall(command_pattern/path_globs 皆 None) | `is_full_match=false` → 永不 SoftBlock,降级 Footnote(比 design 字面更保守) |
| **P5** `PITFALL_SOFT_BLOCK_ENABLED=false` | `recall_pitfall` 永不返 SoftBlock,退回 P3 纯注脚(feature flag 回滚) |
| **P5** 卫生 job:同 `(scope,kind)` Jaccard>0.7 / trigger_key 全等 | 合并保留高 confidence/hit_count,`delete_memory` 删冗余;age>30天 且 hit<2 → `Demoted("aged_out")` |

### 5. Good / Base / Bad Cases

#### Good: full loop

1. Session 1 — user: "I prefer tabs over spaces". LLM calls `remember(title="pref-tabs", kind=Preference)`. Row inserted with `status=Candidate`, `source_session_id="s1"`.
2. Session 2 — user: "format this code". `build_recall_text("format this code", ...)` FTS5-hits the tabs preference. `build_recall_block` wraps in `<autonomous-memories>...</autonomous-memories>`, appended to instructions in the same `messages[0]` synthetic user message. LLM sees the preference, recommends tabs.

#### Base: fresh install

No memories. `build_recall_text` returns `None` for any query. No recall block. Prompt = base system prompt + instruction blocks only.

#### Bad: separate user message for recall

```rust
// BAD — recall as messages[1]
if let Some(text) = build_recall_text(...).await? {
    messages.push(synthetic_user_message(text));  // new message
}
provider.send(messages).await;
```

New user message shifts the Anthropic cache breakpoint → 5-10× cost on every turn. The instructions (4 files) are no longer the cache anchor. The recall block must **append** to `messages[0]`, not insert at index 1.

#### Bad: `cache_control` on recall block

```rust
// BAD — adding cache_control to the recall block
ContentBlock::Text { text: recall_text, cache_control: Some(Ephemeral) }
```

Anthropic's rule: "the last cache_control block is the breakpoint". Adding a second `cache_control: Ephemeral` shifts the breakpoint to the recall block, demoting the instruction files from cache anchor to plain text. The instruction block already carries the cache_control marker; recall blocks do not.

#### Bad: Tier 4 ask on `remember`

```rust
// BAD — treat remember like any other write tool
PermissionContext::new(...).with_ask(Tier4Ask::Write).check(REMEMBER_TOOL_NAME)?;
```

LLM either silently abstains (most common — predictive abstention) or interrupts the user 50 times per session. The whole point of autonomous memory is that the LLM writes it; the safety net is the actual guard.

#### Bad: 收紧 recall filter 到 ActiveVerifiedOnly(P5 推翻的预测)

1. P5 设计时曾预期"状态机落地后,session-start recall 收紧到 `RecallStatusFilter::ActiveVerifiedOnly`"(P2 注释 + 本节早期版本)。
2. 实现时发现**收紧会掐断 candidate 晋升路径**:candidate→active 的唯一 v1 触发是"被召回命中"(recall 命中 → `bump_hit_count` → 达 D2 阈值晋升);把 candidate 排除出 recall 则它永不命中、永不晋升(preference/fact 类无 `trigger_key`,只靠 FTS 召回,断路尤甚)。
3. **P5 实际决策**:session-start recall **保持 `IncludeCandidate`**;噪音靠"低阈值快速晋升"(candidate 命中 ≥2 即升 active,流出 candidate 池)+ 卫生 job age-out 控制,**不靠 filter 收紧**。

`RecallStatusFilter` 枚举仍是 load-bearing contract;P5 的教训是 —— **filter 收紧与"靠召回命中晋升"的状态机互斥**:设计晋升路径时,必须确认 recall filter 覆盖所有待晋升状态,否则状态机死锁。见 P5 contract "recall filter 方向"行。

#### Bad: pre-tool recall inside `permissions::check()` (P3 anti-pattern)

1. P3 lands with a Tier 1 hook that calls `recall_pitfall_footnote` from inside `check()`.
2. `check()` becomes a function that both *decides* (5-tier) and *recalls* (DB read) — mixed responsibilities.
3. Tooling that mocks `check()` (e.g. `permissions::tests_check.rs`) now has to mock the pool too, blowing up the test surface.
4. If recall fails, it now pollutes the `Decision` return — was previously a clean `Decision::Allow`, now it's `Result<Decision, ...>`.

The recall is **information injection**, not a *decision*. It lives at the chat_loop seam (check → execute), not inside `check()`. See [permission-layer/five-tier-decision-order.md §4.2](../permission-layer/five-tier-decision-order.md#42-tier-1-hooks-实际实现路径--p3-工具执行前召回2026-06-29-06-29-am-p3-tool-recall).

#### Bad: implementing verified soft-intercept in P3

1. P3 ships with verified-status pitfall hard blocking the tool (returning `Decision::Deny`).
2. P5 lands later wanting a "soft" intercept (return `Decision::Allow` + structured hint to LLM).
3. The P3 hard-block path is now dead code; the seam is in the wrong place.

P3 is **active-only footnote**, period. Verified soft-intercept is **P5 scope** (spike-007 §4, 命中分档表). The function `recall_pitfall_footnote` returns `Result<Option<String>, sqlx::Error>` because the recall result is a **hint, not a decision**. **P5 落地(2026-06-29)**:verified soft-intercept 用 `PitfallRecall` enum(`None` / `Footnote` / `SoftBlock`)+ `recall_pitfall` 分档函数 —— **不返 `Decision`**(SoftBlock 在 chat_loop seam 短路 `execute_tool`,不是权限 Deny);`recall_pitfall_footnote` 保留为 Footnote 档基础。早期"P5 加 sibling `verified_pitfall_decision` 返 `Decision`"的预测未采纳(enum 比 sibling 更统一)。见 P5 contract。

#### Bad: P4 reflection awaits the main loop (P4 anti-pattern)

1. P4 lands with `try_record_outcome` returning a `Future` that the main loop `await`s.
2. Main loop blocks while the LLM reflection runs (typically 1-5s).
3. The "fire-and-forget" guarantee is lost — the user's tool result is delayed by reflection latency.
4. If the LLM call hangs (network issue), the main loop hangs.

P4's reflection is **fire-and-forget**: `tokio::spawn` wraps the entire `reflect_to_pitfall` call, the spawned `JoinHandle` is dropped (not `.await`ed), and any failure is absorbed at `tracing::warn!`. The main loop sees the original `is_error` / `content` signal and continues immediately. See [agent-loop-architecture.md front-matter "Per-tool auto-reflect seam (P4)"](../agent-loop-architecture.md).

#### Bad: P4 bypasses P1's `insert_memory` and writes a raw `INSERT` (P4 anti-pattern)

1. P4 lands with a direct `sqlx::query("INSERT INTO autonomous_memories ...")` to avoid P1's `MemoryInput` struct.
2. P1's safety net (sensitive regex / length cap / 敏感路径 deny-list) is bypassed.
3. A hallucinated API key or local `/home/user/.ssh/...` path leaks into the autonomous memory table.
4. The 50/session frequency cap is bypassed — the agent self-poisons its own memory library.

P4's reflection **must** route through P1's `insert_memory` (`MemoryInput { kind: Pitfall, status: Active, scope: Project, ... }`) so the safety net, the state-machine fields (`hit_count` / `last_used_at` / `demoted_reason`), and the type enums (`MemoryKind` / `MemoryStatus` / `MemoryScope`) all flow through the single source of truth. There is no second write path.

#### Bad: P4 fires on every tool failure (P4 anti-pattern)

1. P4 ships with `REFLECTION_FAILURE_THRESHOLD = 1`.
2. Every single `is_error=true` triggers an LLM reflection.
3. The agent over-writes its memory library with shallow "this command failed" entries.
4. Cost + latency explodes; the precision-first P3 recall filter drowns in noise.

P4's threshold is **2 consecutive failures followed by a success** (per spike-007 §3 路径2 contract). Single failures are absorbed. The success-after-threshold pattern is the actual signal of "the agent tried X, failed twice, then found a working approach Y" — which is the only signal worth remembering. See [P4 contracts table — 触发阈值].

### 6. Tests Required

| Test | Asserts |
|---|---|
| `insert_memory_roundtrip` | Insert + read returns same fields; `status=Candidate`, `hit_count=0` |
| `insert_memory_rejects_sensitive_content` | API-key-shaped content → `Err`; no row inserted |
| `insert_memory_rejects_oversize_content` | >500 chars → `Err`; no row inserted |
| `search_memories_fts_finds_title` | Insert "tabs over spaces" + search "prefer tabs" → hit |
| `search_memories_fts_finds_content` | Same with content-only keyword |
| `search_memories_fts_trigram_supports_substring` | Insert "everlasting" + search "lastin" → hit (trigram, not just prefix) |
| `search_memories_fts_filters_by_status` | Insert Candidate + Active, filter Candidate only → 1 row |
| `list_memories_orders_by_created_at_desc` | Insert 3 rows with distinct timestamps → first is newest |
| `delete_memory_removes_row` | Insert + delete + list → 0 rows |
| `count_memories_for_session_returns_count` | Insert 2 in same session → 2 |
| `bump_hit_count_increments` | Insert + bump + read → `hit_count=1` |
| `build_recall_text_returns_none_for_empty_query` | `""` → `None` |
| `build_recall_text_returns_none_when_no_matches` | Non-matching query → `None` |
| `build_recall_text_surfaces_candidate_match` | 1 Candidate + search → text contains title+content |
| `build_recall_text_truncates_at_token_budget` | N rows summing >500 tokens → truncated; newer first |
| `build_recall_block_has_no_cache_control` | Returned `ContentBlock::Text.cache_control == None` |
| `inject_recall_appends_to_instruction_message_blocks` | Existing instruction message + recall block → `blocks.len()` grows by 1; cache_control on block 0 unchanged |
| `tools_remember_execute_writes_candidate_roundtrip` | Tool call → row with `status=Candidate`, `source_session_id=ctx.session_id` |
| `tools_remember_execute_rejects_sensitive_content` | Tool call with API-key content → `Err` |
| `tools_remember_execute_rejects_when_session_cap_reached` | Pre-seed 50 rows for session → 51st call → `Err` |
| `tools_remember_execute_no_turn_cap_p2` | 4 `remember` calls in same turn (P2) → all succeed (deferred to P5) |
| `commands_list_autonomous_memories_returns_runtime_list` | Insert 2 + invoke Tauri command → 2 rows in response |
| `commands_delete_autonomous_memory_removes_row` | Insert + invoke Tauri command → row gone |
| `commands_delete_autonomous_memory_project_isolation` | Insert in A, delete from B → row not deleted (404 / no-op) |
| `store_fetch_memories_happy_path` | Mock IPC → `runtimeMemories` populated |
| `store_fetch_memories_error_path` | Mock IPC rejects → `runtimeMemoriesError` set |
| `store_delete_memory_happy_path` | Mock IPC + 2 rows → 1 row left after delete |
| `store_delete_memory_error_path` | Mock IPC rejects → `runtimeMemoriesError` set; row not optimistically removed |
| `MemoryPreview_renders_runtime_memories_list` | 2 rows in store → component renders 2 list items |
| `MemoryPreview_delete_button_opens_confirm` | Click delete → `ConfirmDialog` opens with title |
| `MemoryPreview_confirm_delete_calls_store` | Confirm click → `store.deleteMemory(id)` invoked |
| `MemoryPreview_cancel_delete_keeps_row` | Cancel click → row remains; IPC not invoked |
| `recall_pitfall_footnote_active_hit_returns_text` (P3) | Insert active pitfall with `tool_name='shell'` + `command_pattern='cargo test'`; recall with matching `tool_name` + `command` → `Some("⚠️ Memory: ...")` |
| `recall_pitfall_footnote_unrelated_tool_returns_none` (P3) | Insert active pitfall for `shell`; recall with `tool_name='read_file'` → `None` |
| `recall_pitfall_footnote_verified_hit_returns_none_for_p3` (P3) | Insert verified pitfall (promote via direct DB write); recall → `None` (verified is P5 scope, P3 strictly excludes) |
| `recall_pitfall_footnote_candidate_hit_returns_none` (P3) | Insert candidate pitfall; recall → `None` (candidate is P2 scope, not yet promoted) |
| `recall_pitfall_footnote_command_pattern_mismatch_returns_none` (P3) | Insert pitfall with `command_pattern='cargo test'`; recall with `command='npm test'` → `None` |
| `recall_pitfall_footnote_empty_db_returns_none` (P3) | Empty DB; recall with any `tool_name` + `tool_input` → `None` (no panic, no error) |
| `single_failure_does_not_trigger` (P4) | `FailureTracker` with one `is_error=true` for `shell` → `try_record_outcome` returns `None`; no reflection spawned (PRD AC #3) |
| `two_failures_then_success_triggers` (P4) | `shell` × 2 `is_error=true` then `is_error=false` → `try_record_outcome` returns `Some(_)`; tracker resets to 0 (PRD AC #1) |
| `first_call_success_does_not_trigger` (P4) | `is_error=false` as first event for any `tool_name` → `try_record_outcome` returns `None` |
| `one_failure_then_success_does_not_trigger` (P4) | `shell` × 1 failure then 1 success → no trigger (counter resets on success; below threshold) (PRD AC #3) |
| `counter_resets_after_trigger` (P4) | Trigger fires, then 2 more failures → counter goes 0→1→2, no second trigger until a new success-then-failure cycle |
| `tools_have_independent_counters` (P4) | `shell` failure × 2 does NOT affect `edit_file` counter; each `tool_name` has its own `TrackerEntry` |
| `try_record_outcome_writes_active_pitfall_end_to_end` (P4) | Real DB + MockProvider: 2 failures + 1 success → `reflect_to_pitfall` runs → `insert_memory` writes a row with `kind=Pitfall`, `status=Active`, `scope=Project`, `source_ref=<request_id>:<tool_name>`, populated `trigger_key` (PRD AC #1) |
| `invalid_json_from_llm_does_not_panic_or_write` (P4) | MockProvider returns prose-without-JSON → `strip_code_fence` + `serde_json::from_str` fail → `tracing::warn!` + silent drop; no row in DB; no panic |
| `try_record_outcome_does_not_block_caller` (P4) | MockProvider with `tokio::time::sleep(10s)`; `try_record_outcome` returns in < 100ms (fire-and-forget hard rule) (PRD AC #2) |
| `strip_code_fence_handles_common_cases` (P4) | Input `"```json\n{...}\n```"` → `{...}`; input ` ```\n{...}\n``` ` → `{...}`; input `{...}` (no fence) → `{...}` |
| `truncate_for_reflect_under_cap_passes_through` (P4) | Input 1 KiB → output identical (under 2 KiB cap) |
| `truncate_for_reflect_over_cap_appends_marker` (P4) | Input 4 KiB → output truncated to 2 KiB with `…(truncated)` marker |
| `reflected_pitfall_is_recallable_by_p3_helper` (P4) | End-to-end: P4 reflection writes a pitfall with `tool_name='shell'`, `command_pattern='cargo test'`; subsequent `permissions::recall_pitfall_footnote(pool, 'shell', tool_input_with_cargo_test)` returns `Some("⚠️ Memory: ...")` (PRD AC #4) |
| `agent_loop_p5_soft_block_short_circuits_execute` (P5) | 端到端:verified pitfall + `is_full_match` → 首次 tool_use 触发 SoftBlock(`execute_tool` 未调、`is_error=false` 提示、`memory_id` 入 session set) |
| `agent_loop_p5_soft_block_second_hit_degrades_to_execute` (P5) | 同坑二次命中 → `Footnote` + 正常 `execute_tool`(D1 防循环) |
| `p5_recall_verified_full_match_returns_soft_block` (P5) | `recall_pitfall`:verified + 完全匹配 → `SoftBlock`;path/command-agnostic(皆 None)→ `Footnote` |
| `promote_if_eligible_*` (P5) | hit_count 跨 2 升 active;跨 5 + age≥3 天升 verified;demoted 不被 bump 晋升(矩阵拒绝) |
| `jaccard_*` / `char_trigrams_*` (P5) | 中文短句重叠 Jaccard>0.7;identical=1.0;disjoint=0.0;Unicode char 非 byte |
| `pick_keeper_*` / `trigger_key_equal_*` (P5) | 高 confidence/hit_count 胜出;trigger_key 三字段(tool+command_pattern+path_globs)全等 |
| `list_memories_project_isolation` (2026-09-02) | (None, Some("proj-b")) 不返回 proj-a 的 project 行(user 行照常);(None, None) admin 视图返回全表 |
| `list_memories_filters_by_scope_correctly` (2026-09-02 扩展) | (None, Some(id)) → user + 该项目行(H2 (c));(None, None) → 全部行;Some(Project)+None → Err |
| `store setRuntimeProjectFilter` (2026-09-02) | "all" → `{ projectId: null }`;pinned id 免 loadForProject 直查;同值 set 不发 IPC |

30+ tests across DB / agent / tool / IPC / store / component.

### 7. Wrong vs Correct

#### Wrong: per-tool Tier 4 ask on `remember` → Correct: silent-allow + safety net

```rust
// BAD
PermissionContext::new(...).with_ask(Tier4Ask::Write).check(REMEMBER_TOOL_NAME)?;

// GOOD — remember is a knowledge-write, not a file mutation.
// No Tier 4 ask. Safety net lives in insert_memory:
// - sensitive content regex
// - 500-char content cap
// - per-session count cap (50)
insert_memory(pool, InsertMemoryInput {
    scope, kind, title, content, tags, source_session_id: ctx.session_id, ...
}).await?;
```

The tool returns the new id. The user sees the new memory in `MemoryPreview` (runtime memories section) and can delete it. The write is "autonomous" — visible and revocable, not pre-approved.

#### Wrong: separate user message for recall → Correct: append to instruction message

```rust
// BAD — recall as messages[1]
if let Some(text) = build_recall_text(...).await? {
    messages.push(synthetic_user_message(text));
}
provider.send(messages).await;

// GOOD — recall is a new block in messages[0]
if let Some(text) = build_recall_text(...).await? {
    let block = memory_recall::build_recall_block(&text);
    messages[0].content.push(block);  // append, not insert
}
provider.send(messages).await;
```

Recall is ephemeral (not persisted to message history); it lives in the same `messages[0]` synthetic user message as `build_instructions_blocks`. The `cache_control: Ephemeral` breakpoint on the first instruction block stays put.

#### Wrong: `cache_control` on recall block → Correct: no `cache_control`

```rust
// BAD
ContentBlock::Text { text: recall_text, cache_control: Some(Ephemeral) }

// GOOD — no cache_control on recall blocks
ContentBlock::Text { text: recall_text, cache_control: None }
```

Anthropic's "last cache_control block is the breakpoint" rule: recall blocks must NOT carry a `cache_control` marker. The instruction block already carries the marker; adding another shifts the breakpoint to the recall block and demotes the instructions from cache anchor.

---
