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


---

## Part Index (08-07-large-file-splitting)

- [signature-run-chat-loop](./agent-loop-architecture/signature-run-chat-loop.md)
- [pattern-production-test-entry](./agent-loop-architecture/pattern-production-test-entry.md)
- [pattern-cancellation-guard](./agent-loop-architecture/pattern-cancellation-guard.md)
- [pattern-worker-subagent](./agent-loop-architecture/pattern-worker-subagent.md)
- [pattern-concurrent-dispatch](./agent-loop-architecture/pattern-concurrent-dispatch.md)
- [system-prompt-assembly](./agent-loop-architecture/system-prompt-assembly.md)
- [pattern-skip-persist-gate](./agent-loop-architecture/pattern-skip-persist-gate.md)
- [debt-linkage](./agent-loop-architecture/debt-linkage.md)
- [pattern-turn-boundary-persist](./agent-loop-architecture/pattern-turn-boundary-persist.md)
- [tests-required](./agent-loop-architecture/tests-required.md)
- [pattern-worker-worktree-override](./agent-loop-architecture/pattern-worker-worktree-override.md)
- [pattern-llm-compaction](./agent-loop-architecture/pattern-llm-compaction.md)
- [pattern-turn-limit-softcap](./agent-loop-architecture/pattern-turn-limit-softcap.md)
- [pattern-budget-gate](./agent-loop-architecture/pattern-budget-gate.md)
- [pattern-turn-checkpoint](./agent-loop-architecture/pattern-turn-checkpoint.md)
- [pattern-message-queue-driver](./agent-loop-architecture/pattern-message-queue-driver.md)
- [pattern-doc-extraction](./agent-loop-architecture/pattern-doc-extraction.md)
