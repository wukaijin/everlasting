<!-- Moved from llm-contract.md 2026-09-19 (doc-split) -->

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
- The agent loop's own tool-execution gate (08-07-group-chat-role-history-
  isolation follow-up, 2026-08-07): `should_continue` keys on `tool_calls`
  alone — an OpenAI-compatible provider (Console Go) can end a tool_use
  stream with a NON-"tool_use" finish_reason ("stop" → "end_turn", or
  missing). The pre-fix predicate (`stop_reason == Some("tool_use")`)
  skipped tool execution entirely → the assistant(tool_use) row was already
  persisted but no tool_result followed → every later turn 400'd with
  "An assistant message with 'tool_calls' must be followed by tool messages
  responding to each 'tool_call_id'" and the group chat burned
  MAX_ORCHESTRATION_ROUNDS on `[生成出错中断]` retries (DB session
  `d7fe451c`: seq 5 emitted 3 read_file tool_uses, zero tool_results, 26
  consecutive error turns). The correct signal is the tool_calls themselves:
  if the model emitted ANY tool_use they MUST be executed and their results
  fed back; stop_reason only decides the terminal `Done` value.
  Test: `agent_loop_tool_use_with_non_tool_use_stop_reason_still_executes`
  (`agent/tests_agent_loop.rs`).
- **Daemon crash during tool execution (RULE-PERSIST-001, 2026-08-24)**: the
  assistant(tool_use) row is persisted BEFORE tools run; a kill -9 mid-
  execution leaves the pair permanently orphaned in the DB — every later
  request in that session 400s. Startup guard: `db::recover_interrupted_
  messages` Step B scans each session's MAX(seq) tail; an assistant row with
  `has_tool_calls=1` gets a synthetic `is_error` tool_result user row at
  seq+1 (one block per tool_use_id, content notes the daemon interruption) —
  same repair shape as the error path's `build_synthetic_tool_result_message`.
  Test: `turn_checkpoint.rs` AC4 (second request's provider payload actually
  contains the paired tool_result).
- **Wire round-trip splitting a multi-result user row (09-11-deepseek-
  tool-result-split-400, 2026-09-11)**: the pair-atomicity rule extends to
  the **outbound wire shape** — ALL `tool_result` blocks answering one
  assistant message must ride in the **single user message immediately
  after it**. The PR2/PR3 wire round-trip lifted each `ToolResult` block
  into its own `WireMessage::Tool` and mapped each back as a separate
  `role:"user"` message; native Anthropic merges consecutive user messages
  so it tolerated the split, but **strict Anthropic-schema relays validate
  per-message**: the wukaijin deepseek channel 400s it with
  `messages.N: tool_use ids were found without tool_result blocks
  immediately after` (glm channels on the same relay are tolerant — the
  asymmetry cost a full RCA: group-chat session `caa5020a`, deepseek-flash
  participant three-struck `[生成出错中断]` while glm speakers with 2–4
  tool_use per message passed). Fix: `fuse_adjacent_tool_results`
  (`llm/provider/wire/from_wire.rs`) fuses runs of ≥2 pure-tool_result user
  messages back into ONE message at `wire_messages_to_chat_messages` exit;
  a trailing plain-text user message (loop-detection hint) is deliberately
  NOT folded in — the wire layer has lost row boundaries and the next user
  row may be another group-chat speaker's text. Note `orphan_tool_use_ids`
  does NOT catch this class (results EXIST in history, only split apart).
  Tests: `round_trip_fuses_adjacent_tool_results_into_one_user_message`,
  `round_trip_keeps_trailing_loop_hint_outside_fused_results` (wire),
  `outbound_body_carries_all_tool_results_in_one_user_message_after_
  multi_tool_use` (anthropic body-level, replays the incident shape).

**Test coverage** (in `agent/context.rs`):
- `case_3_tool_use_tool_result_pair_intact_or_dropped_together`
- `regression_pair_at_tail_split_under_pressure` (C3 PR1 regression)

**Related**:
- Thinking blocks have a similar atomicity requirement (see Validation & Error
 Matrix row "`thinking` block appears after a `tool_use` block in history") —
 the assistant turn is the atomic unit for thinking, while the pair is the
 atomic unit for tool_use.

---

