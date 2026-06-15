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
    tool_defs: Vec<ToolDef>,
    provider: Arc<dyn Provider>,
    context_window: u32,
    rid: String,
    session_id: String,
    messages: Vec<ChatMessage>,
    sink: Arc<dyn ChatEventSink>,
    db: SqlitePool,
    cancellations: Arc<Mutex<HashMap<String, CancellationToken>>>,
    session_active_request: Arc<Mutex<HashMap<String, String>>>,
    read_guard: ReadGuard,
    memory_cache: Arc<MemoryCache>,
    permission_asks: crate::agent::permissions::PermissionStore,
    token: CancellationToken,
) { ... }
```

### Why 14 parameters (and not a config struct)?

The 14 parameters look excessive, but they are the **exact set of state pieces
the agent loop body needs**, and grouping them into a config struct would:

1. Hide the dependency surface (a struct named `RunChatLoopArgs` would tempt
   callers to add fields that are *only* test-internal)
2. Add a layer of indirection without adding safety (Rust's borrow checker
   already enforces "use what you need")
3. Obscure the 1:1 correspondence between production and test call sites
   (the 9 `agent_loop_*` integration tests pass them in the same order, with
   the same types — a config struct would let them diverge silently)

`#[allow(clippy::too_many_arguments)]` is the deliberate cost of keeping the
dependency surface explicit. **Do not refactor this into a struct** without
re-running the 9 integration tests + cargo check.

### Production + test call site parity

- **Production**: `app/src-tauri/src/agent/chat.rs::chat` Tauri command's
  `tauri::async_runtime::spawn` body, after pre-flight (provider lookup +
  cancel token registration + sink build). The call site is ~20 lines:
  pure argument marshalling + one `.await`.
- **Tests**: `app/src-tauri/src/agent/tests.rs::agent_loop_basic_text_only_completes`
  and 8 sibling tests pass a `MockProvider` + `MockEmitter` for the
  `Arc<dyn Provider>` and `Arc<dyn ChatEventSink>` parameters. Other
  parameters are real (test DB, real `MemoryCache`, real `PermissionStore`,
  real `ReadGuard`).

The 14-parameter signature is **production-ready as written** — no test-only
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
// chat.rs::chat (production)
tauri::async_runtime::spawn(async move {
    // run_chat_loop owns its own CancellationGuard (cleans
    // cancellations + session_active_request maps on every exit
    // path). The chat command's pre-flight (provider lookup,
    // token registration, sink build) stays here.
    run_chat_loop(
        tool_defs, provider, context_window,
        rid.clone(), session_id.clone(), messages,
        sink_for_spawn, db, cancellations, session_active_request,
        read_guard, memory_cache, permission_asks, token,
    ).await;
});
```

```rust
// tests.rs (test)
run_chat_loop(
    tool_defs.clone(), mock_provider.clone(), 8000,
    rid.clone(), session_id.clone(), messages,
    mock_emitter.clone(), test_db, test_cancellations,
    test_session_active, read_guard.clone(), memory_cache.clone(),
    permission_asks.clone(), token.clone(),
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
// Both sites construct the same struct with the same fields:
CancellationGuard {
    cancellations,                          // Arc<Mutex<HashMap<...>>>
    session_active_request,                 // Arc<Mutex<HashMap<...>>>
    request_id: rid,                        // String
    session_id,                             // String
}
```

`impl Drop for CancellationGuard` does:

```rust
fn drop(&mut self) {
    // 1. cancellations.lock().remove(&self.request_id)
    // 2. session_active_request.lock().remove(&self.session_id)
}
```

**Equivalence proof**: Same struct, same fields, same values, same Drop
implementation → same behavior. Removing one site leaves the other to
do the same work, with no double-fire.

**Verify** with the cancel test:
`agent_loop_cancel_in_turn_2_kills_loop` — asserts the loop is killed
mid-turn AND the maps are cleaned (no leaked entries in
`cancellations[rid]` or `session_active_request[session_id]`).

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

If a future change forks `run_chat_loop` into `run_chat_loop_v2` (e.g.,
for a new emission protocol), the original `run_chat_loop` **must** be
deleted in the same commit, and DEBT.md **must** be updated to record
the new `v2` as the canonical entry point. Do not leave a "v1" around
"for tests" — tests should track production.

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
| `mock_provider_call_count_tracks_send_calls` | MockProvider instrumentation works (sanity) |
| `mock_provider_reports_mock_protocol` | MockProvider reports `Mock` protocol (sanity) |

All 12 must pass on every change to `run_chat_loop`. If any fails, the
production call site in `chat.rs` is **at risk** of the same defect
(failing the integration test means production would also fail).
