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
