## Pattern: PR2a corrected PR1 over-broad `skip_persist` gate (RULE-A-015, 2026-06-20)

**Problem**: PR1b (2026-06-19) introduced the `skip_persist: bool` flag
(20th `run_chat_loop` parameter) with the spec claim "18 persist-site gates
inside the function body". PR2a (2026-06-20) found 2 of those 18 sites
were over-broad — they should NOT have been gated by `skip_persist`,
because gating them broke core worker / parent invariants:

1. **`add_token_usage`** — the per-turn `TokenUsage` update belongs to the
   `sessions` table (the parent's `input_tokens_total` /
   `output_tokens_total` counters), not the `messages` table. With
   `skip_persist=true` gating the call, the worker could not stream its
   per-turn usage into the parent's running total. The parent UI's
   per-request token counter would freeze at the value the parent
   accumulated *before* dispatching the worker — a noticeable UX
   regression. The "skip persist" intent was "skip writes that share the
   `(session_id, seq)` UNIQUE key with the parent"; `add_token_usage`
   doesn't share that key, so it was the wrong gate.

2. **`emit_chat_event_via_sink(ChatEvent::Done { ... })`** — the terminal
   `Done` event drives the worker's `SubagentBufferSink.was_cancelled`
   flip. With `skip_persist=true` gating the emit, the worker's
   `SubagentBufferSink` would never see the terminal `Done{cancelled}` →
   `was_cancelled` stayed `false` → `format_dispatch_result` always
   reported `SubagentStatus::Completed` (even on a real cancel) → the
   `subagent_runs.status` column always read `'completed'` (PR2's
   persistence path couldn't tell cancel from completion). The "skip
   persist" intent was "skip DB writes"; an `emit_chat_event_via_sink`
   is a sink write, not a DB write, so it was the wrong gate.

**Solution**: Both sites are now OUTSIDE the `if !skip_persist { ... }`
gate. PR2a's actual gate count is **16**, not the 18 the PR1 spec said.
The spec the implementation lives in (`agent-loop-architecture.md`
"Signature" block, plus the `tool-contract/04-dispatch-subagent.md` §"dispatch_subagent"
entry) updates the gate count from 18 to 16 in the same commit; this
"Pattern" section is the design rationale.

### Why the original "all persist = gated" framing was wrong

The PR1 spec framed `skip_persist` as a "persist site gate" — every
`persist_*` call inside `run_chat_loop` was wrapped. The framing was
correct as a *default* (worker should not write to the `messages` table
that the parent owns), but the implementation was too literal: it
captured the call shape (`persist_turn` / `update_message_metadata` /
`add_token_usage` / `record_*_audit` / `persist_turn_cwd`) without
distinguishing which writes shared the `(session_id, seq)` UNIQUE key
with the parent. The two sites that didn't share the key (token
accumulation + sink emit) were collateral damage.

The right framing: **`skip_persist` is a "do not write to the
`(session_id, seq)`-keyed `messages` table" gate, not a "do not write
anything" gate**. PR2a re-frames the rule accordingly and the
implementation matches.

### Detection: how PR2a caught the bug

PR2a's `agent_loop_dispatch_subagent_cancelled_persists_status_cancelled`
test (regression for RULE-A-015) ran the worker with
`parent_token.cancel()` mid-flight, then asserted
`subagent_runs.status == 'cancelled'`. The first run saw
`status == 'completed'` despite the parent cancel — the bug. Tracing
showed `SubagentBufferSink.was_cancelled` was still `false` after
`run_chat_loop` returned, which traced back to the worker never
receiving the terminal `Done{cancelled}` event. The terminal emit was
inside `if !skip_persist { ... }`; the gate was too broad. The fix
(lift the terminal emit out of the gate) is the entire PR2a RULE-A-015
patch.

### When to apply this pattern

When a new "worker-isolated" flag is added to `run_chat_loop` (or any
shared entry point with multiple call-site modes), **enumerate each
gated site and verify it actually shares the contended key** (or
whatever the gate is trying to protect). Defaulting to "all writes are
the same kind" is the PR1 anti-pattern; the gate should be defined by
the *contention invariant*, not the *call site shape*.

If a future change adds another `skip_*` flag (e.g. `skip_audit`),
audit each newly-gated site for the same "is the gate really what
protects this site?" question. A test that exercises the worker's
terminal path (e.g. cancel mid-flight, error mid-stream) is the
regression guard for the "Did the gate hide a needed write?" question.

### When NOT to apply

- The gate is by *site shape* and the shape is *exactly* the contended
  key (e.g. `if message_persist_key == parent_key { ... }` — then
  shape == intent and no re-framing is needed).
- The PR2a fix is the canonical exception, not a precedent. The PR1
  design intent ("worker doesn't write to parent's `messages` table")
  was right; the implementation just over-reached. Future flags should
  default to "narrow gate, verify each site".
