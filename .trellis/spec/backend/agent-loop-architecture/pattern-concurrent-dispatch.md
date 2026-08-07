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
