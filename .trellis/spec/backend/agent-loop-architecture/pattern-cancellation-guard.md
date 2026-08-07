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
