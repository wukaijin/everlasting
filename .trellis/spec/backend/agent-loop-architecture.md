# Agent Loop Architecture

> **Purpose**: Codify the `run_chat_loop` shared entry point pattern that production
> and tests both route through. This is the canonical example of "single source
> of truth for the agent loop body" in the project. Any new agent-loop-shaped
> function should follow the same pattern unless the divergence is intentional
> and documented in DEBT.md.

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
    // Production + 9 base tests pass `None`; the worker path passes `Some(20)`.
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
    skip_persist: bool,                                                                // 20
) { ... }
```

### Why 20 parameters (and not a config struct)?

The 20 parameters look excessive, but they are the **exact set of state pieces
the agent loop body needs**, and grouping them into a config struct would:

1. Hide the dependency surface (a struct named `RunChatLoopArgs` would tempt
   callers to add fields that are *only* test-internal)
2. Add a layer of indirection without adding safety (Rust's borrow checker
   already enforces "use what you need")
3. Obscure the 1:1 correspondence between production and test call sites
   (the 31 `agent_loop_*` integration tests, including the 4 B6 worker tests,
   pass them in the same order, with the same types — a config struct would let
   them diverge silently)

`#[allow(clippy::too_many_arguments)]` is the deliberate cost of keeping the
dependency surface explicit. **Do not refactor this into a struct** without
re-running all 31 integration tests + cargo check.

#### Evolution log (parameter count grew with new features)

| Date | Count | PR / task | New param | Why |
|---|---|---|---|---|
| 2026-06-15 | 14 | `06-15-unify-chat-loop-dispatch` (RULE-A-006 closure) | — | baseline after production migrated through `run_chat_loop` |
| 2026-06-17 | 15 | D3 PR3 | `resend_seq: Option<i64>` | resend audit row at user-message persist site |
| 2026-06-19 | 17 | L1a | `background_shells: DefaultRegistry` | cross-request registry threaded into `ToolContext` + per-turn notification drain |
| 2026-06-19 | 18 | B6 PR1a | `max_turns: Option<usize>` | worker turn cap; production + tests pass `None` |
| 2026-06-19 | 19 | B6 PR1b | `skip_session_active: bool` | worker guard Drop skips `session_active_request.remove` |
| 2026-06-19 | 20 | B6 PR1b | `skip_persist: bool` | 18 persist-site gates inside the function body |

The B6 cluster (PR1a + PR1b, adding 3 params in a single task) is the
largest single jump. It is justified because the worker is a **structural**
extension of the agent loop (it re-uses `run_chat_loop` recursively via
`Box::pin`, see "Pattern: Worker Subagent" below), and the 3 params are the
minimal surface needed to keep production + worker behavior isolated
(session mapping cleanup + DB isolation + turn cap).

### Production + test call site parity

- **Production**: `app/src-tauri/src/agent/chat.rs::chat` Tauri command's
  `tauri::async_runtime::spawn` body, after pre-flight (provider lookup +
  cancel token registration + sink build). The call site passes `None` for
  `resend_seq` + `max_turns`, `false` for both `skip_session_active` and
  `skip_persist` — production is never a worker.
- **Tests**: `app/src-tauri/src/agent/tests.rs::agent_loop_basic_text_only_completes`
  and 30 sibling tests pass a `MockProvider` + `MockEmitter` for the
  `Arc<dyn Provider>` and `Arc<dyn ChatEventSink>` parameters. Other
  parameters are real (test DB, real `MemoryCache`, real `PermissionStore`,
  real `ReadGuard`).

The 20-parameter signature is **production-ready as written** — no test-only
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
        None,                          // 18: max_turns (production uses MAX_TURNS=50)
        false,                         // 19: skip_session_active (production chat owns the slot)
        false,                         // 20: skip_persist (production persists normally)
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
with 3 surgical guards (`max_turns` / `skip_session_active` / `skip_persist`)
that keep its behavior isolated from the parent.

```rust
// agent/chat_loop.rs::run_subagent (~:1802) — the interceptor helper
// captures the parent's run_chat_loop closure dependencies and spawns
// the worker with the 3 isolation flags.
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
    Some(SUBAGENT_MAX_TURNS),                // 18: 20 (worker turn cap)
    true,                                    // 19: skip_session_active (worker Drop skips parent eviction)
    true,                                    // 20: skip_persist (worker turns stay in-memory)
)).await;
```

### Why a recursive `run_chat_loop` (vs a separate worker loop function)?

The worker harness is the **same loop**: turn boundaries, C3 compaction,
tool execution, error/cancel paths, emit, persist. Duplicating it would
re-introduce the faithful-port drift hazard (see Pattern above). The 3
new params are the minimal surface needed to isolate the worker from the
parent; every other param is reused as-is.

`Box::pin` breaks the async-fn recursion size-infinite Future chain
(workers have `dispatch_subagent` stripped, so depth is bounded at 1,
but the compiler can't prove this).

### The 3 isolation flags

| Flag | Value | What it prevents |
|---|---|---|
| `max_turns: Some(20)` | B6 PR1a | worker burning parent's token budget on a runaway loop |
| `skip_session_active: true` | B6 PR1b | worker's `CancellationGuard::drop` evicting `session_active_request[parent_session_id]` (would break parent's `cancel_inflight_for_session` / RULE-E-005) |
| `skip_persist: true` | B6 PR1b | worker writing to the shared `messages` table with the same `(session_id, seq)` UNIQUE constraint as the parent; 18 function-body gates cover all persist sites |

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
`[status: completed|cancelled|error]` prefix from
`format_dispatch_result`) and pushes it into `result_blocks` — tool_use/
tool_result pairing is preserved (same invariant as RULE-A-007).

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
- `RULE-A-014` (B6 PR1b 2026-06-19, **open**): `PermissionContext.is_worker`
  is set on the worker side (`_worker_permission_ctx` constructed in
  `run_subagent`) but **not threaded into the nested `run_chat_loop` call**.
  `run_chat_loop` internally rebuilds its own `PermissionContext { is_worker: false }`
  from the session row. As a result, the `if ctx.is_worker { Decision::Deny }`
  branch at the top of `ask_path` is **unreachable on the worker path**:
  `general-purpose` + Edit/Plan mode + a write-tool that triggers Tier 4
  ask_path / ask_shell will emit `permission:ask` → register into
  `permission_asks` → wait for oneshot (which never comes because the
  worker has no UI sink) → **hang until user Stop**. The Tier 4 ask
  denial logic is unit-tested via `permissions::check` with
  `is_worker: true` directly (decision path verified), but the end-to-end
  worker path cannot trigger it without threading. **Fix** (~10 lines + 1
  end-to-end test): add `is_worker: Option<bool>` as the 21st parameter
  to `run_chat_loop`, pass `Some(true)` from `run_subagent`, have
  `run_chat_loop` override the default `false` when `Some`. **Status**:
  accepted as PR2+ follow-up; the trigger conditions are narrow
  (`general-purpose` + Edit/Plan + dangerous tool), Yolo mode is
  unaffected (Tier 4 bypass early-returns Allow), `researcher` is read-only
  and never triggers ask.
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
| `mock_provider_call_count_tracks_send_calls` | MockProvider instrumentation works (sanity) |
| `mock_provider_reports_mock_protocol` | MockProvider reports `Mock` protocol (sanity) |

All 21 must pass on every change to `run_chat_loop`. If any fails, the
production call site in `chat.rs` is **at risk** of the same defect
(failing the integration test means production would also fail).

The 4 B6 worker tests use the same `MockProvider` + `MockEmitter`
fixture as the existing 17 tests — no test-internal mock of the
worker; the worker runs against the same `run_chat_loop` recursion
that production would use, just with the 3 isolation flags set.