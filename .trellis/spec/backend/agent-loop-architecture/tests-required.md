## Tests Required

| Test | Asserts |
|------|---------|
| `agent_loop_basic_text_only_completes` | Production call path: text-only response → `done` event with no tool calls |
| `agent_loop_tool_use_triggers_tool_result_turn` | tool_use → execute → tool_result → next turn |
| `agent_loop_cancel_in_turn_2_kills_loop` | CancellationGuard cleanup: maps empty after cancel |
| `agent_loop_max_turns_emits_done_marker` | MAX_TURNS hit → `done` event with `cancelled: true` |
| `agent_loop_mock_provider_exhaustion_surfaces_error` | Provider error → `ChatEvent::Error` emitted |
| `agent_loop_c3_compaction_does_not_panic` | C3 compaction in turn N → turn N+1 still runs |
| `agent_loop_c3_still_over_emits_error_and_skips_provider` | C3 still-over → emit error, skip `provider.send` (PR4 invariant) |
| `agent_loop_error_path_emits_chat_event_error` | Error mid-loop → `ChatEvent::Error` → loop exits |
| `agent_loop_persist_failure_emits_error` | RULE-A-003 (2026-06-15): `persist_turn` failure on a normal persist site → `ChatEvent::Error{Server}` + loop aborts (matches the StillOver pattern) |
| `agent_loop_cancel_skips_audit_for_cancelled_tool` | RULE-A-004 (2026-06-15): a tool cancelled mid-execution is NOT recorded as `tool_executed` (audit moved after the cancel check) |
| `agent_loop_error_persists_partial_text` | RULE-A-007 (2026-06-17): error mid-turn → partial text + ERROR_MARKER persisted (symmetric to cancel) |
| `agent_loop_error_empty_text_uses_error_marker` | RULE-A-007 edge: empty-text error → text is exactly `ERROR_MARKER` (symmetric to cancel's empty → CANCELLED_MARKER) |
| `agent_loop_error_persists_thinking_and_tool_calls` | RULE-A-007: thinking + tool_use blocks accumulated before the error survive in the persisted `content` JSON |
| `agent_loop_error_persist_failure_is_log_only` | RULE-A-007 decision B: persist failure on error path is log-only (no double-terminal Error event) |
| `agent_loop_error_emits_turn_complete` | RULE-A-007 decision C: error path emits `TurnComplete` (seq + latency) for the partial turn, coexisting with the pre-emit Error |
| `agent_loop_dispatch_subagent_completes_and_returns_summary` | B6 PR1b: parent turn 1 dispatch_subagent tool_use → worker runs → summary tool_result `[status: completed]`; parent's persisted messages do NOT contain the worker's intermediate text (`phantom_worker_text == 0`) |
| `agent_loop_dispatch_subagent_cancel_propagates_to_worker` | B6 PR1b: parent_token cancel → worker_token child fires → status=cancelled + CANCELLED_MARKER; tool_use/tool_result pairing preserved |
| `agent_loop_dispatch_subagent_error_returns_status_error` | B6 PR1b: MockProvider stream error → status=error; tool_use/tool_result pairing preserved |
| `agent_loop_dispatch_subagent_guard_does_not_evict_parent_session_active` | B6 PR1b: `HangingThenCancel` worker + 500ms delayed cancel keeps worker in flight; snapshot verifies parent `session_active_request[parent_session_id]` is preserved (worker Drop with `skip_session_active=true` does NOT evict it) |
| `agent_loop_dispatch_subagent_persists_subagent_run` | B6 PR2a: parent dispatches `general-purpose`; `subagent_runs` row exists with `status='completed'`, `summary` carries `final_text`, `transcript_json` non-empty, `transcript_truncated=0`; `transcript_snapshot` is not empty |
| `agent_loop_dispatch_subagent_cancelled_persists_status_cancelled` | B6 PR2a + RULE-A-015: parent_token cancel mid-worker → `subagent_runs.status='cancelled'` + `finished_at` NOT NULL; regression: terminal `Done` emit is OUTSIDE the `skip_persist` gate (PR2a fix) so `SubagentBufferSink.was_cancelled` was reachable |
| `agent_loop_dispatch_subagent_audit_not_polluted_by_worker` | B6 PR2a: parent + `researcher` worker (silent allow, Tier 5) → parent's `session_audit_events` only carries parent's own ⑨ decisions; worker Tier 5 decisions stay in `transcript` (NOT in `session_audit_events`) |
| `agent_loop_dispatch_subagent_token_usage_does_not_fold_into_parent` | 2026-06-26 reversal of PR2a RULE-A-015: worker emits usage → parent's `last_context_input_tokens` reflects ONLY the parent's own turn (NOT the worker sum); worker usage lives in `subagent_runs.token_usage_json`. The old fold-into-parent design caused the 1.7M / 100% context-occupancy blowup. Companion: `l3a_concurrent_token_usage_does_not_fold_into_parent` |
| `agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied` | B6 PR2b + RULE-A-014 + RULE-A-016: parent Edit mode + `general-purpose` worker + `write_file` to path outside `permission_ctx.cwd` → `tokio::time::timeout(15s)` wraps the worker; Tier 4 `ask_path` sees `ctx.is_worker=true` → `Decision::Deny` IMMEDIATELY (no oneshot wait, no hang); tool_result is `is_error: true` with deny reason. RULE-A-016 (closed B6 PR3a 2026-06-20): the worker's deny does NOT write a `tool_denied` row to the parent's `session_audit_events`; instead `ask_path` emits a `PermissionAskPayload` via the sink → `SubagentBufferSink::emit_permission_ask` records a `TranscriptKind::PermissionAsk` entry in the worker's transcript. The test asserts `tool_denied count == 0` in parent audit + `permission_ask count == 1` in worker transcript + audit delta ≤ 2 (only parent's `tool_allowed` + `tool_executed` for `dispatch_subagent`). |
| `mock_provider_call_count_tracks_send_calls` | MockProvider instrumentation works (sanity) |
| `mock_provider_reports_mock_protocol` | MockProvider reports `Mock` protocol (sanity) |
| `system_prompt_override_worker_path_sends_override` | B6 review defect A fix (2026-06-21): worker path passes `Some(assemble_subagent_prompt(def, &task))` as the 23rd `system_prompt_override` parameter; `MockProvider::sent_systems()` captures the system prompt the LLM actually receives, and the test asserts it equals `SubagentDef.system_prompt` (NOT the parent's `assemble_system_prompt(mode_prefix, base_prompt)` output — which was the pre-fix bug). The negative guard `!received.contains("Yolo mode"|"Edit mode"|"Plan mode")` locks that the parent's `mode_prefix` does not leak into the worker's prompt. |
| `system_prompt_override_none_path_uses_parent_assembly` | B6 review defect A fix (2026-06-21): regression guard that the parent path (`None` override) still goes through `assemble_system_prompt(mode_prefix, base_prompt)` unchanged — recomputes the expected prompt for the harness's project + session row and asserts the LLM received that exact string |
| `role_gate_denies_then_allows_after_mid_loop_task_json_status_change` | RULE-TEST-002 (2026-08-27): workflow role-gate refresh across turns. Round-1 `dispatch_subagent{subagent:"checker"}` denied at state `planning` (mock LLM's same-turn `write_file` flips task.json to `in_progress` first); round-2 same-role dispatch allowed because `drive_turn`'s turn-top `resolve_current_task` refresh feeds the live-ref `DispatchCtx::workflow_ctx`. Mutation-verified against both drift classes (gate rebound to entry snapshot / refresh block removed). Known boundary: the gate opens silently if `resolve_current_task` were to return None mid-loop — third regression class, intentionally not covered. |

All 29 must pass on every change to `run_chat_loop`. If any fails, the
production call site in `chat.rs` is **at risk** of the same defect
(failing the integration test means production would also fail).

The 5 B6 worker tests + the 2 new `system_prompt_override_*` tests use the same `MockProvider` + `MockEmitter`
fixture as the existing 17 base tests — no test-internal mock of the
worker; the worker runs against the same `run_chat_loop` recursion
that production would use, just with the 5 isolation flags set.

The 4 B6 PR2a + 1 PR2b tests cover the persistence + audit + RULE-A-014
invariants on top of the PR1 worker surface. The 7 `subagent_runs::tests_*`
integration tests in `db/tests.rs` cover the DB CRUD + CASCADE + 4 MiB
cap + token-usage streaming layer separately (the persistence layer's
own regression suite, distinct from the agent-loop layer).
