## Scenario: C2+ loop intervention (2026-07-06)

**Context**: C2 (06-24) `loop_detection::detect` hit injects only a soft
hint into `tool_result` (`chat_loop/tools.rs::finalize_turn`); it does NOT terminate the
loop — `MAX_TURNS=200` is the sole hard backstop. C2+ adds a middle
active layer: when soft hints fail N consecutive turns, the harness asks
the user whether to terminate.

**Counter & trigger** (`chat_loop.rs`):
- `loop_hit_count: u32` declared per-`run_chat_loop`-local next to
  `loop_window` (cross-turn accumulating; worker-inherited via nested
  `run_chat_loop`).
- After `detect()`: `verdict != None → loop_hit_count += 1`;
  `None → reset 0` (continuity reset).
- Trigger when `loop_hit_count >= 3` (Hard/Soft share ONE counter; N=3 —
  Hard is zero-false-positive so ~5 consecutive byte-identical calls
  before asking).

**Main-loop ask path** (chat_loop top-level caller, NOT the
`ask_user_question` tool path — harness-driven):
1. Build fixed `ToolQuestionPayload` (harness-generated copy, NOT LLM):
   header `循环检测干预`, options `[终止 loop | 继续]`.
2. `QuestionStore::register(session_id, "loop_intervention_<turn>", payload)`
   + `sink.emit_tool_question(&payload)` — reuses the entire QuestionStore
   + IPC chain + frontend `<AskUserQuestionCard>`. Pseudo `tool_use_id`
   (frontend card does not match on it).
3. `record_loop_intervention_audit(action="asked")` (best-effort).
4. `tokio::select!{ biased; token.cancelled() => ..., rx => ... }`:
   - cancel arm → `QuestionStore::remove` + `Done{stop_reason="cancelled"}`.
   - `Answered(["终止 loop"])` | `Cancelled` → audit `terminated` +
     `Done{stop_reason="loop_terminated"}` + return.
   - `Answered(["继续"])` → audit `continued` + `loop_hit_count = 0` +
     inject enhanced hint + continue loop (build `result_blocks` normally).

**AlreadyPending degradation**: if the LLM concurrently holds an
`ask_user_question` slot, `register` returns `AlreadyPending` → C2+ logs
warn + falls back to the original hint path (does not block the loop;
retries next turn). QuestionStore's single-pending gate dissolves the
race.

**Worker path** (`effective_is_worker` gate, R5): worker triggers C2+ →
directly `Done{stop_reason="loop_terminated"}` with NO `QuestionStore`
round-trip and NO audit row. `dispatch.rs::run_subagent` reads
`worker_sink.was_loop_terminated()` → routes to existing
`SubagentStatus::Incomplete` (no new variant, no DB migration) +
caller-appends `[loop terminated: worker 因循环重复操作被自动终止，未完成全部步骤]`
after `format_dispatch_result_with_model` (NOT a 5th format-fn param —
matches the existing `worker_changes_summary` tail-append pattern). The
parent agent then decides retry / switch path / accept.

**AuditKind::LoopIntervention** (no DB migration — `kind` column is TEXT,
reuses the generic `record_audit_event` path): payload
`{hit_count: u32, verdict_kind: "hard"|"soft", action: "asked"|"terminated"|"continued", run_id: Option<str>}`.
`run_id` is a future-proofing placeholder (main loop passes `None`; the
worker path writes no audit row per R5, so `Some(run_id)` is reserved for
a future worker audit surface). Recorded via `record_loop_intervention_audit`
(best-effort, mirrors `record_message_resend_audit`). `as_str()` returns
`"loop_intervention"`; the match is exhaustive with no `_` wildcard
(compiler-enforced on future variants).

**stop_reason three states**: `loop_terminated` (C2+ break) / `cancelled`
(user Stop) / `end_turn` (normal or MAX_TURNS). Frontend treats
`stopReason` as an opaque string (`WorkerTextTimeline.vue`) — no new
frontend case needed.

**`loop_detection.rs` unchanged**: C2+ is pure caller-side logic; the
pure-function detector (31 unit tests) is not modified.

**Tests required**: `cargo test --lib c2plus` (6 tests: AC1-AC6 main-loop
+ AC8 worker break) + `cargo test --lib audit` (AuditKind round-trip +
record fn payload) + `AuditLogItem.test.ts` (10 frontend kind-dispatch
tests). Full PRD: `.trellis/tasks/07-05-c2-loop-active-intervention/`.
