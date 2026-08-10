## Pattern: Worker Subagent (B6 PR1, 2026-06-19)

**Problem**: The harness needs a way for the main agent to **delegate a
focused sub-task** to a worker agent running in an isolated context —
independent messages, independent token budget, independent turn cap —
without polluting the main conversation with verbose search / exploration
output. The worker's result must come back as a single summary
(`tool_result`), and the worker's intermediate state must stay isolated
from the parent's session DB / cancel maps.

**Solution**: Reuse `run_chat_loop` **recursively** as the worker's
executor. The worker IS just another `run_chat_loop` invocation, but
with 4 surgical guards (`max_turns` / `skip_session_active` / `skip_persist` /
`is_worker`) that keep its behavior isolated from the parent. The 2026-06-21
fix (B6 review defect A) adds a 5th element: `system_prompt_override: Some(p)`
threads the worker's `SubagentDef.system_prompt` so the worker actually sees
its role prompt instead of the parent's (the pre-fix `_worker_system_prompt`
was dead code — see the run_chat_loop doc comment + the
`assemble_subagent_prompt` doc comment for the full rationale).

```rust
// agent/chat_loop.rs::run_subagent (~:1802) — the interceptor helper
// captures the parent's run_chat_loop closure dependencies and spawns
// the worker with the 4 isolation flags + the system_prompt override.
Box::pin(run_chat_loop(
    worker_tool_defs,                        // filter_tools_for_subagent(builtin, def)
    provider.clone(),
    context_window,
    worker_rid,                              // "{parent_rid}-sub-{seq}"
    parent_session_id.to_string(),           // REUSE parent's session_id for DB linkage
    worker_messages,                         // [build_instructions_blocks, delegation_task]
    worker_sink_dyn,                        // SubagentBufferSink (does NOT forward to parent)
    db.clone(),
    cancellations.clone(),                   // worker rid registered
    _session_active_request.clone(),         // worker does NOT register (reuses parent's map)
    read_guard.clone(),
    memory_cache.clone(),
    skill_cache.clone(),
    permission_asks.clone(),
    worker_token,                            // CHILD of parent_token — parent cancel propagates
    None,                                    // 16: resend_seq
    background_shells.clone(),
    Some(SUBAGENT_MAX_TURNS),                // 18: 200 (worker turn cap; raised 20→200 by 06-21 task)
    true,                                    // 19: skip_session_active (worker Drop skips parent eviction)
    true,                                    // 20: skip_persist (worker turns stay in-memory)
    Some(true),                              // 21: is_worker (worker path → Tier 4 collapses to Deny)
    app_handle,                              // 22: forward parent AppHandle so worker's SubagentBufferSink can emit subagent:event (None in tests)
    Some(assemble_subagent_prompt(def, &task)),  // 23: worker overrides the parent's assemble_system_prompt with its SubagentDef.system_prompt
)).await;
```

### Why a recursive `run_chat_loop` (vs a separate worker loop function)?

The worker harness is the **same loop**: turn boundaries, C3 compaction,
tool execution, error/cancel paths, emit, persist. Duplicating it would
re-introduce the faithful-port drift hazard (see Pattern above). The 6
new params (4 isolation flags + the AppHandle IPC bridge + the system_prompt
override) are the minimal surface needed to isolate the worker from the
parent + wire its IPC emit path + thread the worker's role prompt; every
other param is reused as-is.

`Box::pin` breaks the async-fn recursion size-infinite Future chain
(workers have `dispatch_subagent` stripped, so depth is bounded at 1,
but the compiler can't prove this).

### The isolation flags + role-prompt override

| Flag | Value | What it prevents |
|---|---|---|
| `max_turns: Some(200)` | B6 PR1a (raised 20→200 by 06-21 task) | worker burning parent's token budget on a runaway loop |
| `skip_session_active: true` | B6 PR1b | worker's `CancellationGuard::drop` evicting `session_active_request[parent_session_id]` (would break parent's `cancel_inflight_for_session` / RULE-E-005) |
| `skip_persist: true` | B6 PR1b | worker writing to the shared `messages` table with the same `(session_id, seq)` UNIQUE constraint as the parent; **16** function-body gates cover all persist sites (PR1 spec said 18; PR2a RULE-A-015 corrected 2 over-broad gates) |
| `is_worker: Some(true)` | B6 PR2b | worker's Tier 4 `ask_path` / `ask_shell` emitting `permission:ask` → register into `permission_asks` → wait for oneshot (never comes — worker has no UI sink) → **hang until user Stop** (RULE-A-014). With this flag, the Tier 4 branch sees `ctx.is_worker = true` and collapses to `Decision::Deny` immediately |
| `app_handle: Some(parent's handle)` | B6 PR3 (PR2 hotfix) | worker's `SubagentBufferSink` would otherwise have no IPC emit path → frontend `<SubagentDrawer>` (PR3b) cannot stream the worker's transcript live (the worker would have to finish before the drawer sees anything). Forwarding the parent's `AppHandle` lets the sink emit `subagent:event` per worker emit; tests pass `None` so the emit path becomes a no-op (no Tauri runtime) |
| `system_prompt_override: Some(assemble_subagent_prompt(def, &task))` | B6 review defect A fix (2026-06-21) | worker previously inherited the parent's `assemble_system_prompt(mode_prefix, base_prompt)` output (`SubagentDef.system_prompt` was dead code — `_worker_system_prompt` discarded in `chat_loop.rs::run_chat_loop`), producing prompt / permission contradictions in Edit/Plan mode (worker told "you can write" but Tier 4 collapsed write tools to `Deny`). With this flag, the loop uses `def.system_prompt` verbatim for the worker; tests + production pass `None` so the parent's `assemble_system_prompt` path runs unchanged. 4 指令文件 prompt caching is unaffected — the 4 instructions live in a separate user-role synthetic message with its own `cache_control: Ephemeral` breakpoint, independent of the system role |

### Tool interception (NOT `execute_tool_inner`)

The following tools are **registered** in `builtin_tools()` so the LLM
can discover them + go through the ⑨ permission check, but their
**execution** is intercepted in `chat_loop.rs`'s tool_use handling loop
(at ~:1380 for `dispatch_subagent`, ~:3110 for the blocking
user-interaction pair). Why: `execute_tool_inner` signature is
`(name, input, ctx, guard, session_id, skill_cache, cancel)` — it has no
access to the agent-loop dependencies these tools need. Pushing them
into `ToolContext` would blur the tool layer / agent layer boundary; the
interception pattern keeps them at the agent loop layer where they
naturally live.

| Intercepted tool | Need that `execute_tool_inner` can't satisfy | What chat_loop does |
|---|---|---|
| `dispatch_subagent` | `provider` + `db` + `cancellations` + `session_active_request` + `read_guard` + `memory_cache` + `permission_asks` + `background_shells`(for nested `run_chat_loop`) | Calls `run_subagent(...)` recursively with 4 isolation flags + `system_prompt_override`; constructs `ContentBlock::ToolResult` with `[status: ...]` prefix from `format_dispatch_result` (see [tool-contract/04-dispatch-subagent.md §7](../tool-contract/04-dispatch-subagent.md)) |
| `ask_user_question` | `QuestionStore` oneshot + `current_session_id` (for `get_pending_question` IPC key) | Calls `ask_user_question::execute_blocking(...)` → `store.register` → `sink.emit_tool_question` → `tokio::select!{cancel, oneshot}` → constructs `ContentBlock::ToolResult` (see [tool-contract/11-request-mode-change.md §8 Design Decisions](../tool-contract/11-request-mode-change.md) + [chat/generative-ui.md §B9/AskUserQuestionCard](../../frontend/chat/generative-ui.md)) |
| `request_mode_change` (2026-07-07) | `QuestionStore` extended to `PendingInteraction` (kind = `question` \| `mode_change`) + `current_mode` snapshot (for noop check) + `ChatEventSink::emit_mode_change_request` + `CancellationToken` (for `tokio::select!` oneshot wait) | Calls `request_mode_change::execute_blocking(input, session_id, tool_use_id, current_mode, question_store, sink, cancel)` → noop check → `store.register(PendingInteraction::ModeChange(...))` → `sink.emit_mode_change_request` → `tokio::select!{cancel, oneshot}` → constructs `ContentBlock::ToolResult` (see [tool-contract/11-request-mode-change.md §7](../tool-contract/11-request-mode-change.md)) |

The interceptor builds a `ContentBlock::ToolResult` (with the
`[status: completed|cancelled|error|incomplete]` prefix from
`format_dispatch_result` for `dispatch_subagent`; with the JSON
`{"allowed": true, ...}` / `{"cancelled_by_user": true}` / `{"noop": true, ...}`
shape for the user-interaction pair) and pushes it into `result_blocks` —
tool_use/tool_result pairing is preserved (same invariant as RULE-A-007).
For non-completed terminal states, `format_dispatch_result` also appends a
`Worker partial actions:` summary of the worker's executed tool_calls so
the parent can do compensatory repair (RULE-BackSubagent-001, 2026-06-22;
wire shape + 2 KiB head+tail cap in `tool-contract/04-dispatch-subagent.md` §dispatch_subagent).

**Common rationale** (`ask_user_question` / `request_mode_change`):
both are **blocking** tools — the agent loop must wait for user
interaction before continuing. `execute_tool`'s per-tool batch
execution path is designed for **non-blocking I/O** (`read_file` /
`shell` / `grep` etc.) where the tool returns immediately; pushing
oneshot waits through it would blur "execute and return" vs "wait for
user" semantics and complicate the per-batch ordering / cancellation
story. Routing them through chat_loop's interception point keeps
the blocking contract at the agent loop layer (where `tokio::select!`
already exists for cancel/timeout coordination).

### worker context (APPEND, never insert at 0)

```rust
// subagent.rs::build_worker_messages
messages.push(/* synthetic user msg with build_instructions_blocks(memory_cache) */
                /* banner carries cache_control: Ephemeral — worker's OWN breakpoint */);
messages.push(/* optional synthetic assistant ack — keeps Anthropic wire alternation */);
messages.push(/* delegation task user msg — APPEND, NOT prepend */);
```

**Prompt-cache invariant** (B12 + L1a both hit this trap): worker
`messages[0]` is the worker's own cache breakpoint — independent of the
parent's `messages[0]`. APPEND keeps the breakpoint stable. The summary
returns to the parent as a `ContentBlock::ToolResult` (naturally at the
end of the parent's accumulated `result_blocks`), so the parent's cache
breakpoint is never disturbed.

### When to apply this pattern

- A new "control-flow tool" needs to be added to the agent loop (a tool
  whose execution path is *not* a pure I/O function but does manipulate
  agent-loop state). Examples that would qualify: a `delegate_to_user`
  tool (asks the user a clarifying question mid-loop — sibling of
  `ask_user_question` / `request_mode_change`), a `spawn_parallel_workers`
  tool (PR2+ dispatch_subagents plural).
- A new sub-mode of `run_chat_loop` is needed (e.g., a "headless"
  loop that doesn't go through the chat-event sink). Adding a new flag
  + a new function-body gate is the right move; duplicating the function
  is the anti-pattern.

### When NOT to apply this pattern

- The new tool's execution is a **pure I/O function** — register it in
  `builtin_tools()` and add a `match` arm in `execute_tool_inner` like
  every other tool. Only control-flow tools (those that need `provider` /
  `db` / `cancellations` etc.) belong in `run_chat_loop`'s interception
  loop.
- The new sub-mode has **fundamentally different invariants** from
  `run_chat_loop` (e.g., it doesn't emit `TurnComplete`, doesn't go
  through C3). Write a separate function instead of a flag — flags
  accumulate and obscure the code.

### Worker ask resolve outcome + turn counting (RULE-WorkerAsk-001 + RULE-FrontSubagent-004, 2026-06-22)

Two transcript-related contracts that the worker `SubagentBufferSink` is now responsible for. Both are transcript-only (no IPC, no audit) and survive into the persisted `subagent_runs.transcript_json` + `turn_count` columns.

**1. Ask resolve outcome → `TranscriptKind::PermissionAskResolved` entry.** The `ask_path` worker branch in `permissions/mod.rs` runs `tokio::select!{cancel, timeout, oneshot}`; after the select returns, the sink records a `PermissionAskResolved` entry with `payload_json = { rid, outcome }`. Outcome is one of `"allow" | "deny" | "timeout" | "cancel"` (worker `AllowAlways` collapses to `"allow"` per Session 62; `OneshotDropped` → `"cancel"`). Surface via `SubagentBufferSink::emit_permission_ask_resolved(&self, rid, outcome)`, the only override of `ChatEventSink`'s trait default no-op — keeps `AppHandleSink` and all test sinks compiling unchanged (no `Arc<dyn>` downcast needed). Why transcript-only: live interaction card flip is already driven by `usePermissionsStore` rid removal (Session 62 `89e5ba1`), so a second `permission:ask` IPC on resolve would be redundant + risk re-arming the live card. The `PermissionAsk` + `PermissionAskResolved` pair in the transcript gives historical replay the full decision + outcome in one place.

**2. Real per-turn `Done` count → `subagent_runs.turn_count`.** `SubagentBufferSink::turns_completed() -> u64` is a `fetch_add(1)` in the `Done` event arm **only when `stop_reason` is NOT `Some("cancelled")` and NOT `Some("max_turns")`**. This is the "synthetic terminal" exclusion: real per-turn `Done` events (LLM finished a turn normally) increment; the synthetic `Done { stop_reason: Some("cancelled") }` and `Done { stop_reason: Some("max_turns") }` terminals emitted by `chat_loop.rs` (~:1820, ~:1866) do NOT. Net effect: `turn_count` at terminal write time is always the **real** count of completed LLM iterations, never inflated by the synthetic end-of-run signal. `run_subagent` threads `Some(worker_sink.turns_completed() as i64)` into `update_run_finished(..., turn_count)`; the column is nullable (no DEFAULT) so pre-PR2 rows keep NULL and the drawer's `statusDisplay` falls back to `terminalDurMs` (wall-clock) for legacy rows. The same `stop_reason` guard also protects the existing `per_turn_usage` push, so `turn_count` and `token_usage_json` stay in 1:1 lockstep (regression-protected by `subagent_runs_update_finished_round_trips_turn_count`).

**Why both are sink-level, not chat_loop-level.** The sink already owns the per-event record pathway (chat_event / tool_call / tool_result / permission_ask); adding `emit_permission_ask_resolved` + the `Done` counter to the same struct keeps the transcript the single source of truth and avoids threading new state through `run_chat_loop`'s 23-param signature. The trait-default no-op for `emit_permission_ask_resolved` is the template for any future sink-side contract that doesn't apply to the main chat (`AppHandleSink` — no transcript — inherits the no-op for free).
