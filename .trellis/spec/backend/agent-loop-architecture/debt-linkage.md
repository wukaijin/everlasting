## DEBT.md Linkage

- `RULE-A-006` (production chat.rs agent loop = test run_chat_loop): **closed (2026-06-15)**
  via task `06-15-unify-chat-loop-dispatch`. The migration eliminated the
  faithful-port drift hazard. If `run_chat_loop`'s signature changes, the
  change is **visible** in production (`cargo check` of `chat.rs` fails)
  rather than silently drifting in test-only code.
- `RULE-A-001` + `RULE-A-002` (C3 tail pair + over-budget degradation):
  **closed (2026-06-14)**. Both were originally closed by mirroring the
  fix into the faithful port; the 06-15 unification means the mirror is
  no longer needed.
- `RULE-A-014` (B6 PR2b 2026-06-20, **closed by PR2b**): the
  `PermissionContext.is_worker` override on the worker path is now
  threaded into the nested `run_chat_loop` call via the 21st parameter
  `is_worker: Option<bool>`. `run_subagent` passes `Some(true)`; the
  loop body reads `effective_is_worker = is_worker.unwrap_or(false)`
  and sets `PermissionContext { is_worker: effective_is_worker, ... }`.
  The Tier 4 `ask_path` / `ask_shell` branches now see `ctx.is_worker = true`
  and collapse to `Decision::Deny` instead of emitting `permission:ask`
  and waiting for a oneshot that the worker has no way to receive.
  Trigger conditions for the hang were narrow (`general-purpose` subagent
  + Edit/Plan mode + Tier 4 ask-triggering tool); Yolo was unaffected
  (Tier 4 bypass early-returns Allow); `researcher` was unaffected
  (read-only, never triggers ask). Regression test:
  `agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied`
  (PR2b, `app/src-tauri/src/agent/tests.rs`) — runs the worker with
  Edit mode + `general-purpose` + `write_file` to a path outside
  `permission_ctx.cwd`; verifies the loop exits in <15s (would hang
  forever on a stuck oneshot if PR2b's fix were reverted) and the
  tool_result is `is_error: true` with a deny reason.
- `RULE-A-015` (B6 PR2a 2026-06-20, **closed by PR2a**): PR1b's
  `skip_persist` gate was over-broad — it covered 2 sites that should
  NOT be gated: (a) `add_token_usage` (token-usage metadata lives on
  the `sessions` table, not the `messages` table — the worker must
  still stream per-turn usage into the parent's `sessions` accumulator
  so the parent's UI shows live total cost); (b) the terminal
  `emit_chat_event_via_sink(ChatEvent::Done)` (the `SubagentBufferSink`
  is BOTH the consumer of the terminal `Done` and the source of
  `transcript_snapshot()` — gating it killed the worker's
  `was_cancelled` tracking, so the persist step would always see
  `Completed` even on cancel). PR2a lifted both out of the gate. The
  accurate `skip_persist` site count is now **16** (not the 18 the
  PR1 spec said). See "Pattern: PR2a corrected PR1 over-broad
  skip_persist gate (RULE-A-015)" below for the design rationale.
- `RULE-A-016` (B6 PR3a 2026-06-20, **closed**): the Tier 4 `ask_path` /
  `ask_shell` worker branch previously called `record_audit_event(ToolDenied)`
  inside the `if ctx.is_worker { ... }` collapse path, landing a `tool_denied`
  row in the **parent's** `session_audit_events` (worker reuses
  `parent_session_id`). This polluted the C4 audit log: a user reviewing
  the parent session's audit would see a worker Tier 4 collapse that was
  never surfaced to the parent UI. PR3a fix: `ask_path` worker branch no
  longer calls `record_audit_event`; instead it emits a
  `PermissionAskPayload` via `sink.emit_permission_ask(...)` so
  `SubagentBufferSink::emit_permission_ask` records a
  `TranscriptKind::PermissionAsk` entry in the worker's transcript (PR3
  drawer renders the deny). The
  `agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied`
  test now asserts `tool_denied count == 0` in parent audit +
  transcript `PermissionAsk count == 1` + audit delta ≤ 2.
- `RULE-E-005` (worktree destroy await cancel): unaffected by B6 (worker
  Drop is properly bounded by `skip_session_active=true`; the parent
  `cancel_inflight_for_session` lookup continues to find the parent's
  rid because `session_active_request[parent_session_id]` is preserved).
  **Closed 2026-06-15**, remains closed under B6.

If a future change forks `run_chat_loop` into `run_chat_loop_v2` (e.g.,
for a new emission protocol), the original `run_chat_loop` **must** be
deleted in the same commit, and DEBT.md **must** be updated to record
the new `v2` as the canonical entry point. Do not leave a "v1" around
"for tests" — tests should track production.

---

> 历史 ADR 详见 [IMPLEMENTATION.md §4 2026-06-17 RULE-A-007 / 2026-06-20 RULE-A-015](../../../../docs/IMPLEMENTATION.md)
