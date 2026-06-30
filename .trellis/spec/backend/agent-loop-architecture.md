# Agent Loop Architecture

> **Purpose**: Codify the `run_chat_loop` shared entry point pattern that production
> and tests both route through. This is the canonical example of "single source
> of truth for the agent loop body" in the project. Any new agent-loop-shaped
> function should follow the same pattern unless the divergence is intentional
> and documented in DEBT.md.
>
> **Per-turn context construction** (⑤a stage) includes TWO injected blocks in the
> same `messages[0]` synthetic user message:
> 1. **Instruction blocks** — `build_instructions_blocks(memory_cache)` returns
>    the 4 instruction files (User/Project × CLAUDE.md/AGENTS.md) with
>    `cache_control: Ephemeral` on the first block (the cache breakpoint).
>    See [memory.md §Scenario: Two-Layer Memory Injection](./memory.md).
> 2. **Recall block** — `memory_recall::build_recall_block(recall_text)` returns
>    a FTS5-recalled autonomous-memory block with **no** `cache_control` (must
>    NOT shift the breakpoint). Appended to the same `messages[0]` after
>    instruction blocks. See [memory.md §Scenario: Autonomous Memories](./memory.md#scenario-autonomous-memories-db-backed-runtime-memory-v2-2-期) for the full recall contract.
>
> **CRITICAL**: Recall must **append** to `messages[0]`; a new user message at
> index 1 shifts the Anthropic cache breakpoint and invalidates the cache on
> every turn (5-10× cost). Adding `cache_control` to the recall block shifts
> the breakpoint to the recall block and demotes the instructions from cache
> anchor. See [memory.md §7 Wrong vs Correct](./memory.md#7-wrong-vs-correct).
>
> **Per-tool pitfall recall seam (P3, 2026-06-29, 06-29-am-p3-tool-recall)**:
> in addition to the two `messages[0]` blocks above, the loop has a
> **post-check / pre-execute seam** in `chat_loop.rs` (parallel-batch L2 path
> ~line 1792 + serial path ~line 2361) where
> `permissions::recall_pitfall_footnote(pool, tool_name, tool_input)` is invoked.
> On `active`-status pitfall hit, the returned string is prepended to
> `tool_result.content` **before** the envelope wrap, so `tool_use_id` pairing
> and `is_error` semantics stay intact. Verified soft-intercept (returning a
> structured `Decision` from inside `check()`) is **P5 scope** — P3 is
> active-only footnote, mounted at the seam, not inside the 5-tier decision
> chain. See [permission-layer.md §4.2](./permission-layer.md#42-tier-1-hooks-实际实现路径--p3-工具执行前召回2026-06-29-06-29-am-p3-tool-recall) and
> [memory.md §Pre-tool pitfall recall contract](./memory.md#pre-tool-pitfall-recall-contract-p3-layer-2-of-2--2026-06-29-06-29-am-p3-tool-recall).
>
> **Per-tool auto-reflect seam (P4, 2026-06-29, 06-29-am-p4-event-reflect)**:
> the loop has a **post-execute seam** in `chat_loop.rs` (parallel-batch L2
> path + serial path, sibling to the P3 seams above) where
> `auto_reflect::try_record_outcome(failure_tracker, ...)` is invoked
> **after** `execute_tool` returns and **after** the audit-write check
> (`!token.is_cancelled()`), reading `ToolResultPayload.is_error` as the
> signal. A per-session `FailureTracker` (`Arc<Mutex<HashMap<tool_name,
> TrackerEntry>>>`) is created in `run_chat_loop` local scope and shared
> across turns + parallel/serial paths. On the pattern "consecutive
> `REFLECTION_FAILURE_THRESHOLD = 2` failures followed by a success for
> the same `tool_name`", the tracker fires a `tokio::spawn`'d
> `reflect_to_pitfall(provider, pool, ...)` that calls the **main provider
> instance** (not a separate one) with a dedicated `REFLECT_SYSTEM_PROMPT`
> + `REFLECT_USER_TEMPLATE` to elicit JSON
> `{title, content, trigger_key: {tool, command_pattern, path_globs}}`,
> then writes via P1's `insert_memory` (single source of truth for the
> write safety net) with `kind=Pitfall, status=Active, scope=Project,
> source_ref=<request_id>:<tool_name>`. The whole reflection pipeline
> is fire-and-forget — failures are absorbed at `tracing::warn!` and
> never bubble to the main loop. **P3 ↔ P4 close the loop**: a pitfall
> written by P4 is immediately recallable by P3's
> `find_pitfalls_by_trigger` (verified by P4 unit test
> `reflected_pitfall_is_recallable_by_p3_helper`). P4 does NOT touch
> the 5-tier decision chain, P3's pre-execute seam, or
> `ToolResultPayload` shape. See
> [memory.md §Event-driven bypass reflection contract (P4)](./memory.md#event-driven-bypass-reflection-contract-p4-write-side-of-the-loop--2026-06-29-06-29-am-p4-event-reflect).

---

## Signature: `run_chat_loop`

**Location**: `app/src-tauri/src/agent/chat_loop.rs`

```rust
#[allow(clippy::too_many_arguments)]
pub async fn run_chat_loop(
    tool_defs: Vec<ToolDef>,                                                          // 1
    provider: Arc<dyn Provider>,                                                       // 2
    context_window: u32,                                                               // 3
    rid: String,                                                                       // 4
    session_id: String,                                                                // 5
    messages: Vec<ChatMessage>,                                                        // 6
    sink: Arc<dyn ChatEventSink>,                                                      // 7
    db: SqlitePool,                                                                    // 8
    cancellations: Arc<Mutex<HashMap<String, CancellationToken>>>,                     // 9
    session_active_request: Arc<Mutex<HashMap<String, String>>>,                       // 10
    read_guard: ReadGuard,                                                             // 11
    memory_cache: Arc<MemoryCache>,                                                    // 12
    skill_cache: Arc<SkillCache>,                                                      // 13
    permission_asks: crate::agent::permissions::PermissionStore,                       // 14
    token: CancellationToken,                                                          // 15
    // D3 PR3 (2026-06-17): resend context. When `Some(seq)`, the
    // user-message persist site writes a `resend_message` audit row
    // pointing at the original user message's seq. `None` for normal
    // first-time sends. Best-effort.
    resend_seq: Option<i64>,                                                           // 16
    // L1a (2026-06-19): cross-request background-shell registry.
    // Threaded into `ToolContext` so the 3 L1a tools can call into it.
    // The agent loop itself reads it once per turn (after C3 compaction,
    // before `provider.send`) to drain completion notifications.
    background_shells: crate::background_shell::DefaultRegistry,                        // 17
    // B6 PR1a (2026-06-19): worker turn cap. `None` = default 50 (MAX_TURNS).
    // `let turn_limit = max_turns.unwrap_or(MAX_TURNS); for turn in 1..=turn_limit`.
    // Production + 9 base tests pass `None`; the worker path passes `Some(SUBAGENT_MAX_TURNS)` (= `Some(200)`).
    max_turns: Option<usize>,                                                          // 18
    // B6 PR1b (2026-06-19): when `true`, `CancellationGuard::drop` skips
    // `session_active_request.remove(session_id)`. The worker path uses this
    // so its Drop does NOT evict the parent's `session_active_request[parent_session_id]`
    // entry (REVIEW-SUBAGENT-PRD #2 / RULE-E-005). Production + tests pass `false`.
    skip_session_active: bool,                                                         // 19
    // B6 PR1b (2026-06-19): when `true`, all DB writes inside `run_chat_loop`
    // (persist_turn / update_message_metadata / touch_session / add_token_usage /
    // record_*_audit / persist_turn_cwd — 18 sites total) are skipped. The
    // worker path uses this so its intermediate turns stay in-memory only
    // (the `SubagentBufferSink` transcript captures them; PR2 persists into
    // `subagent_runs`). Skipping also avoids the UNIQUE-constraint collision
    // with the parent's own `persist_turn` calls — both loops would otherwise
    // write to the same `messages` table keyed by `(session_id, seq)`.
    //
    // **PR2a correction (2026-06-20, RULE-A-015)**: the 18 site count is
    // actually 16 in the implementation as merged. PR2a fixed 2 over-broad
    // gates that PR1b introduced: (a) `add_token_usage` — token-usage
    // metadata belongs to the `sessions` table, not the `messages` table, so
    // it should NOT be gated by `skip_persist` (worker must still stream its
    // per-turn token usage into the parent's `sessions` accumulator so the
    // parent's UI shows live total cost); (b) the terminal `Done` event
    // emit — the `SubagentBufferSink` was the BOTH the consumer of the
    // terminal `Done` and the data source for `transcript_snapshot()`, so
    // gating it killed the worker's `was_cancelled` tracking. Both are
    // now outside the gate. See "Pattern: PR2a corrected PR1 over-broad
    // skip_persist gate (RULE-A-015)" below.
    //
    // **2026-06-26 reversal (task 06-26-fix-token-usage-snapshot)**: item (a)
    // (`add_token_usage`) is REVERSED — worker token now stays OUT of the
    // parent's sessions totals. Rationale: worker reuses parent session_id,
    // so streaming its per-turn usage into the parent's accumulator polluted
    // the parent's "context occupancy %" (subagent turns summed in → 1.7M /
    // 100% blowup). Token usage switched to a per-turn **snapshot**
    // (`update_last_turn_usage`, overwrites not accumulates) and is gated
    // back inside `!skip_persist`; worker token lives in
    // `subagent_runs.token_usage_json` only. Item (b) (terminal Done emit)
    // stays outside the gate — that part of PR2a stands.
    skip_persist: bool,                                                                // 20
    // B6 PR2b (2026-06-20, RULE-A-014): when `Some(true)`, the
    // `PermissionContext` built inside the loop carries `is_worker: true`,
    // which makes the Tier 4 `ask_path` / `ask_shell` branches collapse
    // to `Decision::Deny` instead of emitting a `permission:ask` (workers
    // have no UI sink — a permission modal would hang forever on the
    // oneshot). `None` falls back to the session-row mode's natural
    // default (production-style = `false`, since no parent process is a
    // worker). The worker path passes `Some(true)`; production + 35
    // `agent_loop_*` integration tests pass `Some(false)` to make the
    // production default explicit at the call site.
    is_worker: Option<bool>,                                                           // 21
    // B6 PR3 (2026-06-20, PR2 hotfix): optional Tauri `AppHandle` used
    // ONLY by `run_subagent` to construct the worker's
    // `SubagentBufferSink` with a live IPC emit path (the
    // `subagent:event` channel). The agent loop body itself does NOT
    // read this parameter — only `run_subagent` does, when building
    // the worker sink. Production passes `Some(app.clone())` from the
    // `chat` Tauri command; tests pass `None` (no Tauri runtime, the
    // worker's IPC emit becomes a no-op). `AppHandle` is an `Arc`
    // internally so the clone is cheap.
    app_handle: Option<tauri::AppHandle>,                                              // 22
    // 2026-06-21 fix (B6 review defect A): worker system prompt
    // override. When `Some(p)`, the loop uses `p` directly as the
    // system prompt (skipping `assemble_system_prompt(mode_prefix,
    // base_prompt)`); when `None`, the loop builds the prompt from
    // the project + session row. `run_subagent` (worker nested call)
    // passes `Some(assemble_subagent_prompt(def, &task))`; the
    // production `chat` command + 36 `agent_loop_*` integration
    // tests pass `None` (parent path). 4 指令文件 prompt caching is
    // unaffected — the 4 instructions live in a separate user-role
    // synthetic message with its own `cache_control: Ephemeral`
    // breakpoint (see `build_instructions_blocks`), independent of
    // the system role.
    system_prompt_override: Option<String>,                                            // 23
) { ... }
```

### Why 23 parameters (and not a config struct)?

The 23 parameters look excessive, but they are the **exact set of state pieces
the agent loop body needs**, and grouping them into a config struct would:

1. Hide the dependency surface (a struct named `RunChatLoopArgs` would tempt
   callers to add fields that are *only* test-internal)
2. Add a layer of indirection without adding safety (Rust's borrow checker
   already enforces "use what you need")
3. Obscure the 1:1 correspondence between production and test call sites
   (the 36 `agent_loop_*` integration tests, including the 5 B6 worker tests
   and 1 B6 PR2b end-to-end test, pass them in the same order, with the same
   types — a config struct would let them diverge silently)

`#[allow(clippy::too_many_arguments)]` is the deliberate cost of keeping the
dependency surface explicit. **Do not refactor this into a struct** without
re-running all 36 integration tests + cargo check.

#### Evolution log (parameter count grew with new features)

| Date | Count | PR / task | New param | Why |
|---|---|---|---|---|
| 2026-06-15 | 14 | `06-15-unify-chat-loop-dispatch` (RULE-A-006 closure) | — | baseline after production migrated through `run_chat_loop` |
| 2026-06-17 | 15 | D3 PR3 | `resend_seq: Option<i64>` | resend audit row at user-message persist site |
| 2026-06-19 | 17 | L1a | `background_shells: DefaultRegistry` | cross-request registry threaded into `ToolContext` + per-turn notification drain |
| 2026-06-19 | 18 | B6 PR1a | `max_turns: Option<usize>` | worker turn cap; production + tests pass `None` |
| 2026-06-19 | 19 | B6 PR1b | `skip_session_active: bool` | worker guard Drop skips `session_active_request.remove` |
| 2026-06-19 | 20 | B6 PR1b | `skip_persist: bool` | persist-site gates inside the function body (PR1 spec: 18 sites; PR2a actual: 16 — see RULE-A-015) |
| 2026-06-20 | 21 | B6 PR2b (RULE-A-014) | `is_worker: Option<bool>` | thread `is_worker` to nested `run_chat_loop` so Tier 4 `ask_path` / `ask_shell` collapses to `Deny` on the worker path (workers have no UI sink) |
| 2026-06-20 | 22 | B6 PR3 (PR2 hotfix) | `app_handle: Option<tauri::AppHandle>` | thread the parent's `AppHandle` through so `run_subagent` can wire the worker's `SubagentBufferSink` with a live `subagent:event` IPC emit path (live transcript streaming for the PR3b `<SubagentDrawer>`); tests pass `None` |
| 2026-06-21 | 23 | `06-21-fix-worker-system-prompt-dead-code` (B6 review defect A) | `system_prompt_override: Option<String>` | thread the worker's `SubagentDef.system_prompt` through as the override (pre-fix `_worker_system_prompt` was dead code; the worker silently inherited the parent's `assemble_system_prompt` output, causing prompt/permission contradictions in Edit/Plan mode); production + 36 `agent_loop_*` tests pass `None`, the worker nested call passes `Some(assemble_subagent_prompt(def, &task))` |
| 2026-06-30 | 24 | `06-30-explicit-agent-dispatch` | `forced_dispatch: Option<ForcedDispatch>` | user `@@<agent> <task>` prefix → turn-1 short-circuit bypasses `provider.stream` (parent LLM zero calls) + reuses `run_subagent` directly; production passes the parsed `ForcedDispatch` (or `None`), worker nested + all tests pass `None` |

The B6 cluster (PR1a + PR1b + PR2b + PR3, adding 5 params across 4 sub-PRs in a
single 2-week window) is the largest single jump. It is justified because
the worker is a **structural** extension of the agent loop (it re-uses
`run_chat_loop` recursively via `Box::pin`, see "Pattern: Worker Subagent"
below), and the 5 params are the minimal surface needed to keep
production + worker behavior isolated (session mapping cleanup + DB
isolation + turn cap + Tier 4 collapse without hang + IPC emit path).
The follow-up `system_prompt_override` param (2026-06-21, B6 review defect A)
is a one-shot fix for a dead-code bug in the worker path — it restores the
worker to its `SubagentDef.system_prompt` after PR1b's nested call had
silently inherited the parent's `assemble_system_prompt` output (causing
prompt / permission contradictions in Edit/Plan mode). The 6 total B6 params
across 5 PRs remain the minimum surface needed.

### Production + test call site parity

- **Production**: `app/src-tauri/src/agent/chat.rs::chat` Tauri command's
  `tauri::async_runtime::spawn` body, after pre-flight (provider lookup +
  cancel token registration + sink build). The call site passes `None` for
  `resend_seq` + `max_turns`, `false` for both `skip_session_active` and
  `skip_persist`, `Some(false)` for `is_worker` (production is never a
  worker; the explicit `Some(false)` makes the production-style default
  obvious at the call site, matching PR2b's contract),
  `Some(app.clone())` for `app_handle` (PR2 hotfix: production threads a
  real `AppHandle` so the worker's `SubagentBufferSink` can emit
  `subagent:event` to the frontend), and `None` for
  `system_prompt_override` (production is never a worker, so the parent's
  `assemble_system_prompt(mode_prefix, base_prompt)` path runs unchanged).
- **Tests**: `app/src-tauri/src/agent/tests.rs::agent_loop_basic_text_only_completes`
  and 35 sibling tests pass a `MockProvider` + `MockEmitter` for the
  `Arc<dyn Provider>` and `Arc<dyn ChatEventSink>` parameters. Other
  parameters are real (test DB, real `MemoryCache`, real `PermissionStore`,
  real `ReadGuard`). Tests pass `Some(false)` for `is_worker` to make the
  non-worker test surface explicit, `None` for `app_handle` (no Tauri
  runtime — the worker's IPC emit path becomes a no-op; the worker's
  `SubagentBufferSink` is constructed via
  `SubagentBufferSink::new_without_app_handle` so transcript accumulation
  still works), and `None` for `system_prompt_override` (the production +
  test path runs through `assemble_system_prompt(mode_prefix,
  base_prompt)` unchanged).

The 23-parameter signature is **production-ready as written** — no test-only
gating (no `#[cfg(test)]`), no compile-time `dead_code` allowance, no
runtime branching on `cfg!(test)`.

---

## Pattern: Production + Test Shared Entry Point

**Problem**: Two structurally identical copies of the agent loop body exist
in `chat.rs::chat` (production) and `chat_loop.rs::run_chat_loop` (test).
Any change to one — bug fix, new tool, new emit point, new C3 degradation
rule — must be mirrored to the other. Drift is invisible until production
behaves differently from tests.

**Solution**: Route production directly through the function that tests
also call. One source of truth, two callers.

```rust
// chat.rs::chat (production) — PR1 (B6) call site
tauri::async_runtime::spawn(async move {
    // run_chat_loop owns its own CancellationGuard (cleans
    // cancellations + session_active_request maps on every exit
    // path). The chat command's pre-flight (provider lookup,
    // token registration, sink build) stays here.
    run_chat_loop(
        tool_defs, provider, context_window,
        rid.clone(), session_id.clone(), messages,
        sink_for_spawn, db, cancellations, session_active_request,
        read_guard, memory_cache, skill_cache, permission_asks, token,
        None,                          // 16: resend_seq
        background_shells.clone(),     // 17
        None,                          // 18: max_turns (production uses MAX_TURNS=200; 2026-06-22 bumped 50→200)
        false,                         // 19: skip_session_active (production chat owns the slot)
        false,                         // 20: skip_persist (production persists normally)
        Some(false),                   // 21: is_worker (production is never a worker)
        Some(app.clone()),             // 22: app_handle (production threads real AppHandle for worker subagent:event emit)
        None,                          // 23: system_prompt_override (production is never a worker; parent assemble_system_prompt path runs unchanged)
    ).await;
});
```

```rust
// tests.rs (test) — agent_loop_basic_text_only_completes
run_chat_loop(
    tool_defs.clone(), mock_provider.clone(), 8000,
    rid.clone(), session_id.clone(), messages,
    mock_emitter.clone(), test_db, test_cancellations,
    test_session_active, read_guard.clone(), memory_cache.clone(),
    skill_cache.clone(), permission_asks.clone(), token.clone(),
    None,                          // 16
    background_shells.clone(),     // 17
    None,                          // 18
    false,                         // 19
    false,                         // 20
    Some(false),                   // 21
    None,                          // 22: app_handle (tests have no Tauri runtime; worker IPC emit becomes a no-op)
    None,                          // 23: system_prompt_override (production-style caller — parent assemble_system_prompt path runs unchanged)
).await;
```

### When to apply this pattern

Apply when ALL of the following hold:

- A complex async function body is needed in production
- That function is also useful as a test fixture (high test value — covers
  the real flow, not a stripped-down mock)
- The function's state requirements are stable (not constantly evolving)

### When NOT to apply this pattern

- The function body is trivial (≤ 20 lines) — overhead of the call
  indirection > savings
- The test needs to inject behavior at the middle of the body
  (e.g., short-circuit before turn 2) — use a sub-trait or feature flag
  instead, so the test exercises a *real* sub-piece without forking the
  function
- Production has guarantees the test cannot (e.g., a real OS socket);
  in that case, the function needs a seam (e.g., a `Provider` trait
  parameter), not a duplicate

### Anti-pattern: faithful port as a drift hazard

The pre-2026-06-15 state had `run_chat_loop` as a **faithful port** of
`chat.rs::chat`'s spawn body. This was the right *interim* step (it let
integration tests be written against a stable surface) but it was
explicitly a **drift hazard**: any change to the production loop had to
be mirrored in the test loop. PR4 (C3 tail pair orphan) and RULE-A-001 /
RULE-A-002 demonstrated this hazard materializing in practice.

**Do not** maintain a "test-faithful port" longer than necessary. Once
integration tests prove the port, migrate production to call the port
directly (R1 in task 06-15-unify-chat-loop-dispatch). If you cannot
migrate immediately, **at minimum** the port must be guarded by a
DEBT.md entry with `Status: partial` and a `Re-evaluation Log` entry
showing when the divergence was last re-checked.

---

## Pattern: CancellationGuard Single-Source via Equivalence Proof

**Problem**: A Drop-based cleanup guard (`CancellationGuard`) is created in
two places — production `chat.rs::chat` spawn and `chat_loop.rs::run_chat_loop`.
Each creates the same struct with the same fields, intending the same
Drop behavior (clean `cancellations` and `session_active_request` maps
on every exit path). If the production code creates the guard, the
`run_chat_loop` also creates one, and the Drop runs **twice** — both
removes are no-ops on the second call, but the redundancy is fragile
(adding a non-idempotent cleanup in the future would silently double-fire).

**Solution**: Keep the guard in **exactly one place** (in this codebase,
`run_chat_loop`). Remove the other instance. Prove equivalence before
removing, by showing both instances are byte-equal at construction:

```rust
// Both sites construct the same struct with the same fields (pre-PR1b):
CancellationGuard {
    cancellations,                          // Arc<Mutex<HashMap<...>>>
    session_active_request,                 // Arc<Mutex<HashMap<...>>>
    request_id: rid,                        // String
    session_id,                             // String
}

// B6 PR1b (2026-06-19): the guard grew a 5th field — `skip_session_active: bool`.
// Production + tests pass `false`; the worker path (via `run_subagent`)
// passes `true`. See "Pattern: Worker Subagent" below for the worker context.
CancellationGuard {
    cancellations,
    session_active_request,
    request_id: rid,
    session_id,
    skip_session_active,                    // B6 PR1b
}
```

`impl Drop for CancellationGuard` does:

```rust
fn drop(&mut self) {
    // 1. cancellations.lock().remove(&self.request_id)  — ALWAYS
    // 2. if !self.skip_session_active { session_active_request.lock().remove(&self.session_id) }
    //                                                     ^^^ B6 PR1b gate
}
```

**Equivalence proof**: Same struct, same fields, same values, same Drop
implementation → same behavior. Removing one site leaves the other to
do the same work, with no double-fire.

**Verify** with the cancel test:
`agent_loop_cancel_in_turn_2_kills_loop` — asserts the loop is killed
mid-turn AND the maps are cleaned (no leaked entries in
`cancellations[rid]` or `session_active_request[session_id]`).
**B6 PR1b regression**: `agent_loop_dispatch_subagent_guard_does_not_evict_parent_session_active` —
asserts the worker's Drop (with `skip_session_active=true`) does NOT
evict the parent's `session_active_request[parent_session_id]` entry.

### When to apply this pattern

- Multiple construction sites for a Drop-implementing struct
- The struct's Drop is **idempotent** (multiple Drops are safe)
- You have a way to **prove** the Drop is idempotent (typically: it
  only does `HashMap::remove`, which is naturally idempotent)

### When NOT to apply

- The Drop has non-idempotent side effects (e.g., a global counter
  decrement that underflows at zero) — in that case, you **must**
  consolidate the side effect, not just prove equivalence
- The struct has runtime state that differs between construction sites
  (different `Arc`s, different closures) — there's no equivalence to prove

---

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
| `system_prompt_override: Some(assemble_subagent_prompt(def, &task))` | B6 review defect A fix (2026-06-21) | worker previously inherited the parent's `assemble_system_prompt(mode_prefix, base_prompt)` output (`SubagentDef.system_prompt` was dead code — `_worker_system_prompt` discarded at `chat_loop.rs:2052`), producing prompt / permission contradictions in Edit/Plan mode (worker told "you can write" but Tier 4 collapsed write tools to `Deny`). With this flag, the loop uses `def.system_prompt` verbatim for the worker; tests + production pass `None` so the parent's `assemble_system_prompt` path runs unchanged. 4 指令文件 prompt caching is unaffected — the 4 instructions live in a separate user-role synthetic message with its own `cache_control: Ephemeral` breakpoint, independent of the system role |

### Tool interception (NOT `execute_tool_inner`)

`dispatch_subagent` is **registered** in `builtin_tools()` so the LLM
can discover it + go through the ⑨ permission check, but its
**execution** is intercepted in `chat_loop.rs`'s tool_use handling loop
(at ~:1380). Why: `execute_tool_inner` signature is
`(name, input, ctx, guard, session_id, skill_cache, cancel)` — it has no
access to `provider` / `db` / `cancellations` / `session_active_request`
/ `read_guard` / `memory_cache` / `permission_asks` /
`background_shells`, all of which `run_subagent` needs (REVIEW-SUBAGENT-PRD
#3 verified this empirically). Pushing them into `ToolContext` would
blur the tool layer / agent layer boundary; the interception pattern
keeps them at the agent loop layer where they naturally live.

The interceptor builds a `ContentBlock::ToolResult` (with the
`[status: completed|cancelled|error|incomplete]` prefix from
`format_dispatch_result`) and pushes it into `result_blocks` — tool_use/
tool_result pairing is preserved (same invariant as RULE-A-007). For
non-completed terminal states, `format_dispatch_result` also appends a
`Worker partial actions:` summary of the worker's executed tool_calls so
the parent can do compensatory repair (RULE-BackSubagent-001, 2026-06-22;
wire shape + 2 KiB head+tail cap in `tool-contract.md` §dispatch_subagent).

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
  tool (asks the user a clarifying question mid-loop), a `spawn_parallel_workers`
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

## Pattern: Concurrent isolated dispatch (L3b PR2, 2026-06-27)

> **B update (2026-06-30)**: the force-isolate *trigger* moved from
> "general-purpose defaults to `Some(true)`" to "chat_loop's `DispatchBatch::Concurrent`
> passes `parallel=true` → decision `dispatch input > (parallel && worker_is_writable) > def default`".
> Race-dissolution proof below (per-worker `worker/<run_id>` branch) is unchanged —
> concurrent writes still land on separate branches. See `tool-contract.md`
> §"B update (2026-06-30)" for the full decision table + the auto-commit false-success fix.

**Problem**: The B6 `Worker Subagent` pattern above dispatches **one worker at a time** — the
parent turn blocks on a single `run_subagent().await`. When the parent LLM wants to research
multiple independent directions in parallel, serial fan-out costs `sum(worker_i)` wall-clock
instead of `max(worker_i)`. The fix is concurrent fan-out, but only **safely** — without worktree
isolation, N workers writing the same cwd would race.

**Evolution**:
- **L3a (2026-06-24)** solved this with `force_readonly=true` + shared cwd — the read-only
  scope dissolved 3 races (permission:ask, token usage, cancellations). The cost: `general-purpose`
  worker in concurrent batches was locked to read-only tools.
- **L3b PR1 (2026-06-27)** introduced per-worker worktree isolation (`worker/<run_id>`
  branch + per-run UUID + `worktree_override` threading).
- **L3b PR2 (2026-06-27, this section)** removes the `force_readonly` gate on the concurrent
  path. Each concurrent worker now runs in its own worktree (general-purpose builtin defaults
  to `isolation: Some(true)`); the race-dissolution proof is re-derived against the new
  isolated-write scope.

**Solution**: Concurrent fan-out scoped to **per-worker worktree isolation**. The branch is
still gated by a pure-function classifier (single dispatch + mixed batch = serial unchanged;
pure batch ≥ 2 = concurrent) and still reuses the L2 read-only-batch parallel path's structure
verbatim (`FuturesUnordered` + `result_slots[i]` + `Arc<AtomicBool>`). The only per-task
difference: `run_subagent(force_readonly=false)` (post-PR2; pre-PR2 it was `force_readonly=true`).

```rust
// chat_loop.rs serial-path entry — classify before the existing `for` loop
match classify_dispatch_batch(&tool_calls) {
    DispatchBatch::Concurrent { count }         => { /* FuturesUnordered: N × run_subagent(force_readonly=false) — per-worker worktree isolation via L3b PR1 */ }
    DispatchBatch::OverLimit { count, max_concurrent } => { /* hard-reject: all tool_error, 0 spawn */ }
    DispatchBatch::Serial                       => { /* existing serial `for` loop — UNCHANGED */ }
}
```

### Why mirror the L2 path (vs a new concurrency construct)

The L2 read-only-batch parallel path (`chat_loop.rs` ~:1439-1639) already solved exactly this shape:
order-preserving `result_slots`, shared `AtomicBool` cancel aggregation, per-task permission check +
RULE-A-004 audit-skip, streaming `emit_tool_result`. L3a swapped only the per-task body
(`run_subagent(force_readonly=true)` instead of `execute_tool`); L3b PR2 re-swaps to
`run_subagent(force_readonly=false)`. Writing a new construct would re-introduce the
**faithful-port drift hazard** (see the "Anti-pattern" Pattern above) — two parallel dispatch
loops that must stay in sync on ordering / cancel / audit semantics.

### `run_subagent` keeps `force_readonly: bool` (serial-only post-PR2)

`force_readonly` is **retained** as a parameter (not removed) for two reasons:
1. **L3a test compat** — the regression
   `l3a_single_dispatch_runs_serial_path_unchanged` + the
   `l3b_concurrent_general_purpose_workers_complete_shared` test (rebadged from
   `l3a_concurrent_general_purpose_workers_complete_readonly`) were written against the
   `force_readonly` API. Removing it would force those tests to re-thread their mock
   fixtures.
2. **Future "force read-only at the subagent level" feature** — an LLM opt-in or a future
   frontmatter flag can repurpose this param instead of adding a new one.

The concurrent branch always passes `false` now; only the serial single-dispatch path
retains the historical `false` (no behavior change). The `if force_readonly { false }`
short-circuit on `isolated` (in `dispatch.rs::run_subagent`) is therefore a no-op for
the concurrent branch — preserved for the future "force read-only" feature.

### Race dissolution by scope (the load-bearing argument, re-derived post-PR2)

Four race conditions that look scary for concurrent workers are **provably dissolved by
the per-worker-worktree scope** — no synchronization code is needed, and this is the
contract future edits must not silently violate:

| Race | Why it cannot occur in the isolated-worktree scope |
|---|---|
| **worktree write race** (NEW in PR2) | Each worker writes to its own `worker/<run_id>` worktree + branch. The parent's `HEAD` is untouched. `libgit2 worktree add` is serialized at the metadata level; per-worker worktrees coexist safely under the same `.git/` (this is the design point of git worktrees). |
| `permission:ask` contention (modified in PR2) | worker `is_worker=true` (B6 PR2b) no longer collapses Tier 4 `ask` to `Deny` (post-2026-06-22 RULE-FrontSubagent-003 the worker ask routes through the `WorkerAskBanner` round-trip — biased select over parent cancel / 120s timeout / oneshot). **N concurrent workers CAN now each fire a `WorkerAskBanner`** in the parent's UI; this is accepted per the L3a PRD's pre-emptive note. Workaround: user pre-AllowAlways the relevant tool in the parent turn. |
| `token usage` lost update | **Not folded into parent** (2026-06-26 reversal of RULE-A-015/PR2a). Worker token isolation: each worker's `TokenUsage` lives in `subagent_runs.token_usage_json` only. The parent's `sessions.last_*` snapshot is updated by the parent's own `Done` events (gated by `!skip_persist`; worker runs with `skip_persist=true`). **No shared column → no lost update by construction.** |
| `cancellations` fan-out | each worker registers a unique `worker_rid = "{parent_rid}-sub-{tool_use_id}"` (tool_use_id unique per batch); `worker_token = parent_token.child_token()` × N → one parent cancel fires all children. Unchanged from L3a. |

Also verified concurrency-safe (shared state, not races): each worker's `SubagentBufferSink` is
`new()`'d independently inside `run_subagent` (no shared sink); the parent sink
(`AppHandleSink` / test `MockEmitter`) is thread-safe; `PermissionContext` is pure data cloned
per task; each worker's `RunGrantCache` is `Arc::new()`'d per worker (2026-06-26
`06-26-subagent-per-run-grant` task).

### Concurrent worker ask banners — N `WorkerAskBanner`s is the accepted UX

Post-PR2, a concurrent batch where 1+ workers trigger Tier 4 `ask_path` / `ask_shell` /
`web_fetch` (in Edit/Plan mode, with a path outside `permission_ctx.cwd` for `ask_path`)
will see N `WorkerAskBanner`s in the parent's UI. The L3a PRD §"L3a AC4" preemptively
accepted this tradeoff ("user can pre-AllowAlways in parent turn before dispatching").
The "block" mode is also accepted — `WebFetch` and other asks block the worker on
`tokio::select!{cancel, timeout, oneshot}` for up to 120s; concurrent workers can each
block independently, with the parent still cancellable via the existing cancel fan-out
mechanism.

### When to apply this pattern

- The parent genuinely benefits from **parallel independent work** (multi-topic research,
  multi-file refactor, parallel writes to different parts of the repo).
- Workers need to **write** files concurrently — without per-worker worktree, the race
  dissolves only via the read-only scope (L3a). With per-worker worktree (L3b PR1+),
  writes are isolated per `worker/<run_id>` branch.
- You want concurrency **without** the daemon-ization machinery (background, non-blocking
  parent turn) — this pattern still blocks the parent turn until all workers join.

### When NOT to apply this pattern

- Workers need to **collaborate on the same file** (write + read in a tight loop) — worktree
  isolation dissolves concurrency at the file level (each worker sees its own copy). For
  this case, the serial single-worker path is the right primitive; the concurrent branch
  would just queue them serially via per-turn dispatch anyway.
- You need the parent to stay **responsive** during dispatch (background, non-blocking
  parent turn) → that's the daemon-ization track (L3b+); this pattern still blocks the
  parent turn until all workers join.

---

## System prompt assembly (3-layer, cache-stable)

**Location**: `agent/chat_loop.rs::run_chat_loop` entry calls
`system_prompt::assemble_system_prompt`; layers live in
`agent/behavior_prompt.rs` (`DEFAULT_BEHAVIOR_PROMPT`),
`agent/permissions/mod.rs::mode_system_prefix`, and
`agent/system_prompt.rs::build_system_prompt`.

The system prompt sent to the provider is assembled once per `run_chat_loop`
invocation (reused across turns) from three layers, ordered **stablest-first**
so the upstream prompt-cache prefix stays warm:

| Layer | Source | Mutability |
|---|---|---|
| `behavior_prompt` | `behavior_prompt::DEFAULT_BEHAVIOR_PROMPT` (const) | compile-time — tone, objectivity, tool-usage, code conventions, finishing, git safety, language |
| `mode_prefix` | `mode_system_prefix(Mode)` | per-session (Plan/Edit/Yolo permission boundary) |
| `base_prompt` | `build_system_prompt(...)` | per-invocation (cwd, worktree, HEAD sha) |

Assembled as `behavior_prompt + "\n\n" + mode_prefix + "\n\n" + base_prompt`.

### Layering is complementary, not overlapping

- `mode_prefix` is the **permission boundary** (what the system blocks — e.g.
  Plan can't write). `behavior_prompt`'s `Git safety` is the **model's own
  restraint** (never volunteer a commit). Orthogonal dimensions — keep both.
- Tool visibility is **never** described in any prompt layer. It lives
  exclusively in the `tools[]` array sent to the provider (RULE-E-013,
  2026-06-19: the old inline tool-name list in `build_system_prompt` drifted
  and missed 6 of 13 registered tools). The prompt only states tools are
  available + the path-relative convention.
- User-controlled project guidance (AGENTS.md / CLAUDE.md) is delivered via
  **user-role messages** + `cache_control` (`memory/loader.rs`), not the system
  field — a layer *above* this assembly, not part of it.
- **B6 PR1b (2026-06-19)**: the worker's system prompt is **fully replaced**
  via `subagent::assemble_subagent_prompt(def, task)`, NOT mixed with the
  parent's `behavior_prompt` + `mode_prefix` + `base_prompt` (Claude Code
  convention, see research §5). The worker's permission boundary is enforced
  at the ⑨ layer via `PermissionContext.is_worker` (and via `skip_persist`
  / `skip_session_active` for the surrounding isolation), not via prompt text.

### When modifying the system prompt

- New stable behavior rule → edit `DEFAULT_BEHAVIOR_PROMPT` (keep it a const;
  do not derive per-turn).
- New `Mode` → extend `mode_system_prefix`; assembly order is unchanged.
- Do **not** re-introduce a tool-name list in any prompt string — tool
  visibility stays in `tools[]`.

### Tests

`assemble_system_prompt_orders_layers_behavior_mode_base` pins the order;
`behavior_prompt_content_basics` pins the section set + the
`update_checklist`-not-`TodoWrite` invariant (system-prompt-research §7.2);
`build_system_prompt_no_hardcoded_tool_list` pins RULE-E-013.

---

> 历史 ADR 详见 [IMPLEMENTATION.md §4 2026-06-17 RULE-A-007 / 2026-06-20 RULE-A-015](../../docs/IMPLEMENTATION.md)

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
"Signature" block, plus the `tool-contract.md` §"dispatch_subagent"
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

> 历史 ADR 详见 [IMPLEMENTATION.md §4 2026-06-17 RULE-A-007 / 2026-06-20 RULE-A-015](../../docs/IMPLEMENTATION.md)

## Pattern: Turn-boundary persist symmetry — error arm matches cancel arm (RULE-A-007, 2026-06-17)

**Problem**: When the LLM stream emits `ChatEvent::Error` mid-turn,
the agent loop's per-event arm emits the Error to the frontend
(already rendered as a terminal signal) and sets `had_error = true`.
Before RULE-A-007, the post-stream-loop code did `if had_error { return; }`
— bailing out **without** persisting any of the turn's accumulated
`text_parts` / `finalized_thinking` / `tool_calls`. The cancel path,
in contrast, flushed pending thinking, appended `CANCELLED_MARKER` to
the text, and called `persist_turn` so the partial turn survived in
the DB. This asymmetry meant: a user who watched a partial response
render live, then reloaded the session, would find the assistant turn
missing entirely (cancel preserved, error discarded).

**Solution**: The error arm now mirrors the cancel arm. Both paths:

1. Flush pending thinking into `finalized_thinking`.
2. Log an info-level `tracing` line (`cancelled — persisting partial turn`
   vs `errored — persisting partial turn`) so the cause is distinguishable.
3. Build the assistant blocks (`thinking` + `text` + `tool_use` +
   `redacted_thinking`) and append a sentinel marker to the text:
   - Cancel → `CANCELLED_MARKER` (`"[已停止]"`)
   - Error → `ERROR_MARKER` (`"[生成出错中断]"`, RULE-A-007 new constant)
   - Empty-text edge case: marker alone (symmetric branch in each arm).
4. `persist_turn` the partial row.
5. Emit `ChatEvent::TurnComplete { seq, ...latency }` so the frontend
   has the seq + latency breakdown for the partial row.

The two arms differ in **two** places only:

| Concern | Cancel path | Error path |
|---|---|---|
| Persist failure handling | log-only (no emit; the loop is about to emit terminal `Done{cancelled}`) | **log-only** (no emit; the per-event arm already emitted terminal `Error`. A second Error would be a conflicting double-terminal — RULE-A-007 decision B) |
| Terminal signal after persist | `Done { stop_reason: "cancelled", usage: None }` | (none — the pre-emit `Error` is the terminal; no follow-up `Done`) |

### Why error persist failure is log-only (RULE-A-007 decision B)

RULE-A-003 (2026-06-15) made **normal-path** persist failures emit a
typed `ChatEvent::Error{Server}` + abort (otherwise disk-full / DB-lock
contention would silently lose the user message). The error path is
**different**: the per-event arm at `ChatEvent::Error { .. }` already
emits the Error to the frontend before the persist attempt. Emitting
`emit_persist_failure` on top would produce two terminal events
(Error + Error), and the frontend's terminal handling would fire twice.

The cancel path's synthetic tool_result persist already uses log-only
for the same reason (its terminal `Done{cancelled}` is about to fire).
RULE-A-007 makes the error path's assistant-turn persist follow the
same log-only pattern, keeping the "exactly one terminal event per
request" invariant intact.

### Why error path still emits TurnComplete (RULE-A-007 decision C)

`TurnComplete` carries the partial turn's `seq` + latency breakdown.
The frontend uses it to (a) know which row to attach the latency to
and (b) trigger any per-turn UI updates. Without it, the error path's
partial row would be in the DB but the live-streaming UI wouldn't know
its seq until a reload. The pre-emit `Error` event and the
`TurnComplete` event are **not** in conflict — they carry disjoint
information (Error = "something broke"; TurnComplete = "this seq's
partial turn landed + here's the latency"). The controller routes
each event independently.

### Constants

Both markers live in `app/src-tauri/src/agent/helpers.rs` next to
each other:

```rust
pub const CANCELLED_MARKER: &str = "[已停止]";
pub const ERROR_MARKER: &str = "[生成出错中断]";
```

The bracketed-text style survives DOMPurify unchanged, is
locale-friendly, and renders inline in the bubble's markdown. The UI
does not need a special "interrupted" render branch — existing
markdown rendering handles both markers uniformly.

### When to apply this pattern

- Any new terminal path through `run_chat_loop` that has already
  accumulated partial content (text / thinking / tool_use) MUST
  persist the partial turn. The pattern: flush → marker → persist →
  TurnComplete. Bailing out with raw `return` before persist is the
  anti-pattern that RULE-A-007 removed.
- The persist failure handling on a terminal path is log-only
  (NEVER `emit_persist_failure`) — the terminal event was already
  emitted; a second one would conflict.

### When NOT to apply

- A terminal path that has accumulated **zero** content (no
  `text_parts`, no `finalized_thinking`, no `tool_calls`,
  no `redacted_thinking_data`) skips the persist entirely — the
  `if !assistant_blocks.is_empty()` guard handles this. The error
  path's `ErrThenEnd` (no preceding delta) still hits the persist
  branch because the `ERROR_MARKER` alone populates `full_text`.
  This is intentional — the user sees a visible "[生成出错中断]"
  marker explaining what happened, rather than a blank turn.

---

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

All 28 must pass on every change to `run_chat_loop`. If any fails, the
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

## Pattern: Worker Worktree Override (`worktree_override` param, L3b PR1, 2026-06-27)

L3b PR1 introduces two new parameters to `run_chat_loop`:

- **25th `worktree_override: Option<PathBuf>`** — when `Some(path)`, the loop uses `path` as the worker's worktree root INSTEAD of `loaded_session.session.worktree_path` (which is the parent session's worktree — the root cause of worker reuse of the parent's checkout, see `git/diff.rs::diff_against_branch` for the diff-side contract).
- **26th `app_data_dir: PathBuf`** — pass-through to the dispatch_subagent interceptor (the agent loop body itself does NOT read it; only `run_subagent` does, when creating the worker worktree path).

Mirrors the existing `system_prompt_override` (23rd param) override pattern: per-call clarity, no config struct, thread the override at the `ToolContext` construction site (line ~452). When `None`, the loop builds `worktree_path` + `cwd` from the session row as before (production chat + test path, AND the non-isolated worker path).

### `worktree_override` interaction with `current_cwd`

When `worktree_override.is_some()`, `current_cwd` defaults to `worktree_path` (the override). The parent session's `current_cwd` history is meaningless for a worker (it points at a path inside the parent's checkout, not the worker's). When `None`, `current_cwd` falls through to `loaded_session.session.current_cwd` (legacy behavior, unchanged).

### Why 26 parameters (and not a config struct)?

Same argument as the existing 23-param `run_chat_loop` (see "Why 23 parameters" section above). The 2 added parameters follow the same precedent — per-call overrides are clearer than a config struct that grows every time. Tradeoff: marginal cost (each new override = 1 more param) vs one-time refactor cost.

### Interaction with `STRUCTURALLY_DISABLED` + worker nesting gate

The L3b override does NOT change the no-nesting invariant: `dispatch_subagent` is still stripped from worker's toolset via `STRUCTURALLY_DISABLED` + `effective_is_worker` gate (L3d PR3 lesson, see "Pattern: Worker Subagent" above). A worker's worker (depth 2+) cannot happen. PR1's worker worktree is depth 1 only.

### Tests Required (L3b PR1 additions to `agent/tests_agent_loop.rs` + `agent/tests_subagent.rs`)

The 6 `agent_loop_*` integration tests in `tests_agent_loop.rs` thread `None, h.app_data_dir.clone()` (production-style caller). The B6 worker tests in `tests_subagent.rs` gain:

| Test | Asserts |
|---|---|
| `l3b_worker_with_isolation_runs_in_worker_worktree` | dispatch_subagent with isolation=true → worker's tool calls observe a different `ToolContext.worktree_path` than the parent's session row |
| `l3b_worker_with_isolation_false_runs_in_parent_worktree` | dispatch_subagent with isolation=false → worker's tool calls run in parent session's worktree (legacy behavior preserved) |
| `resolve_isolation_truth_table` | 4-row merge semantics from `tool-contract.md §dispatch_subagent isolation` table |

## Pattern: Forced dispatch — `@@` explicit dispatch (2026-06-30)

**Problem**: `dispatch_subagent` is an LLM-owned tool — the model decides whether + which worker to run. The user had no way to force a specific agent (e.g. `@@spec-auditor 审一下 X.md`) without the LLM judging whether to comply.

**Mechanism** (`06-30-explicit-agent-dispatch`): a `@@<agent> <task>` input prefix is parsed by the frontend (`chat.ts send()`) into a `ForcedDispatch { subagent, task }`, threaded through the `chat` Tauri command → `run_chat_loop`'s 24th param `forced_dispatch: Option<ForcedDispatch>`.

**Turn-1 short-circuit** (sits AFTER the user-message persist site, BEFORE `for turn in 1..=turn_limit`): when `Some(fd)`, the loop:
1. emits `ChatEvent::Start` + a synthetic `dispatch_subagent` `tool:call` (`tool_use_id = "forced_{rid}-{seq}"`);
2. calls `run_subagent(...)` directly — **`provider.stream` is NOT called** (this is the load-bearing invariant: the parent LLM contributes zero calls);
3. emits the worker's summary as `tool:result` + `ChatEvent::Delta` (assistant text) + `ChatEvent::Done`;
4. persists the assistant turn (`Blocks = [ToolUse(dispatch), Text(summary)]`) + returns. **Forced dispatch runs exactly ONE turn** — no follow-up LLM loop.

**Reuses `run_subagent` verbatim** — the 19 params mirror the LLM-driven interceptor at `chat_loop.rs:2374` (force_readonly=false, parallel=false → single serial dispatch; isolation falls back to the subagent's frontmatter default via `resolve_isolation`). The permission chain (worker inherits parent Mode → `WorkerAskBanner`) is unchanged.

**Why this isn't a new dispatch path**: it's the same `run_subagent` the LLM-driven interceptor uses, just triggered by a user prefix instead of an LLM `tool_use`. The only thing the prefix skips is `provider.stream` on the parent's turn 1.

**Test**: `agent_loop_forced_dispatch_runs_worker_without_llm` asserts `mock.call_count() == 1` (only the worker's single turn — the parent contributed zero LLM calls).
| `builtin_*_defaults_to_isolated` | `general-purpose.isolation == Some(true)`, `researcher.isolation == None` |
| `probe_worker_changes_*` (3 tests in dispatch.rs) | empty worktree → no changes; tracked edit → changes; untracked file → changes |