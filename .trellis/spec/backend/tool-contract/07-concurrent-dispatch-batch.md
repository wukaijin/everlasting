## Scenario: Concurrent dispatch_subagent batch (L3a → L3b PR2, 2026-06-24 → 2026-06-27)

### 1. Scope / Trigger

- Trigger: L3a — the parent LLM emits **≥2 `dispatch_subagent` tool_use in one turn** (a "pure
  dispatch batch") and expects them to run concurrently, the parent turn blocking until all complete.
  Aligns with Hermes default-foreground `delegate_task` (ThreadPoolExecutor fan-out, parent blocks)
  and Claude Code `Agent` (multiple `Agent` tool_use run concurrently per turn). Replaces the B6 MVP
  serial "one worker at a time" path (chat_loop.rs ~:1697 self-ack "parallel fan-out is v2 / L3").
- Why code-spec depth: mandatory — first **multi-worker concurrent** control-flow path. The batch
  classifier, the read-only enforcement, the hard-reject limit, and the order-preservation contract
  are all executable. Critically, the **3 race conditions** (permission:ask / token usage /
  cancellations) are provably dissolved by the existing architecture in the read-only scope — that
  proof is itself a load-bearing contract future edits must not violate (full derivation in
  `agent-loop-architecture/pattern-concurrent-dispatch.md §"Pattern: Concurrent isolated dispatch"`).

### 2. Signatures

#### Batch classifier (chat_loop.rs)

```rust
enum DispatchBatch { Serial, OverLimit { count: usize, max_concurrent: usize }, Concurrent { count: usize } }
fn classify_dispatch_batch(tool_calls: &[(String, String, Value)]) -> DispatchBatch;
//   d = count(name == DISPATCH_TOOL_NAME); o = count(everything else)
//   d == 0                       → Serial   (not a dispatch batch; L2 read-only path handles it upstream)
//   d >= 2 && o == 0 && d <= MAX → Concurrent { d }
//   d >  MAX                     → OverLimit { d, MAX }
//   else (d == 1 || o > 0)       → Serial   (single dispatch, or mixed batch → existing serial loop)
```

#### Read-only enforcement (subagent/mod.rs)

```rust
// web_fetch is read-only (network fetch, no file mutation) so it is kept
// in the concurrent-branch allowlist; the 4 file-reading tools cover the
// on-disk read surface. See `READONLY_TOOL_ALLOWLIST` doc in
// `subagent::tools_filter::READONLY_TOOL_ALLOWLIST` for rationale.
const READONLY_TOOL_ALLOWLIST: &[&str] = &["read_file", "grep", "glob", "list_dir", "web_fetch", "search_history"];  // search_history joined 2026-08-17 (D2②, read-only DB query)
pub fn filter_tools_readonly(tools: Vec<ToolDef>) -> Vec<ToolDef>;   // mirrors STRUCTURALLY_DISABLED pattern

// run_subagent gained a trailing param:
pub(crate) async fn run_subagent(/* …existing params… */, force_readonly: bool)
    -> (String, /* is_error */ bool, /* cancel_parent */ bool, Option<i32>);
//   force_readonly == true  → apply filter_tools_readonly AFTER filter_tools_for_subagent
//                              + force isolated = false (L3a concurrent safety floor;
//                              retained for L3a regression compat + future opt-in
//                              read-only callers post-L3b PR2).
//   force_readonly == false → unchanged (serial path keeps B6 behavior; L3b PR2
//                              concurrent path also passes false and relies on
//                              per-worker worktree isolation for the write safety).
```

#### Concurrency primitive

`FuturesUnordered` + `result_slots: Vec<Option<ContentBlock>>` (pre-allocated to N, each task writes
its own index) + `Arc<AtomicBool>` shared `cancelled_flag` — **structurally mirrored from the L2
read-only-batch parallel path** (chat_loop.rs ~:1439-1639); the only per-task difference is the body
calls `run_subagent(force_readonly=false)` (L3b PR2; pre-PR2 was `force_readonly=true`) instead
of `execute_tool`. L3b PR2 dropped the L3a `force_readonly=true` read-only scope in favor of
per-worker worktree isolation (see "L3b PR2 update" below).

### 3. Contracts

- **Pure-batch gate**: only `d >= 2 && o == 0` enters the concurrent branch. A mixed batch
  (`dispatch_subagent` + `read_file` in the same turn) falls through to the serial path unchanged.
- **Order preservation**: `tool_result` blocks re-collapse into `result_blocks` in **tool_use
  order** via `result_slots[i]`, NOT completion order. Streaming `emit_tool_result` fires in
  completion order (frontend UX); the LLM context sees tool_results in tool_use order (matches L2).
- **Write safety — 1 layer (L3b PR2)**: each concurrent worker runs in its own
  `worker/<run_id>` worktree (PR1 isolation; builtin `general-purpose.isolation: Some(true)`,
  `researcher.isolation: None`). Writes land on the worker's branch, not the parent's —
  no cwd write race. The 3-layer read-only guarantee from L3a is **removed** for the
  concurrent path (no longer needed; isolation is the safety argument). Layer 1
  (`SubagentDef` allowlist) and layer 3 (`is_worker: true` ⑨ collapse-to-Deny) remain
  in effect for the serial path. The `force_readonly` parameter on `run_subagent` is
  retained for serial-only opt-in + L3a regression compat.
- **Hard reject on over-limit** (Hermes alignment): `d > DELEGATION_MAX_CONCURRENT_CHILDREN` → every
  `dispatch_subagent` returns an `is_error: true` tool_result ("exceeded concurrent delegation
  limit"); **0 workers spawn** (no truncation, no queueing, no partial execution).
- **Cancel aggregation**: `cancelled_flag` set by any task whose worker returned `cancel_parent=true`
  (parent Stop reached it) OR detected `token.is_cancelled()`; after the join, the main loop flips
  its local `cancelled`.
- **`run_subagent` single source of truth**: L3a adds the `force_readonly` param (4-line filter)
  rather than duplicating the ~450-line function — duplication is the faithful-port drift hazard
  (`agent-loop-architecture/pattern-production-test-entry.md §"Anti-pattern: faithful port as a drift hazard"`). Serial call site
  passes `false`; B6 single-dispatch behavior byte-for-byte unchanged. **L3b PR2 (2026-06-27)**:
  the concurrent call site **also** passes `false` now (was `true` under L3a) — per-worker
  worktree isolation (L3b PR1) is the new safety argument, not the read-only scope. The
  `force_readonly` param is retained for L3a regression compat + future opt-in read-only callers.

#### Environment keys

| Key | Default | Purpose |
|---|---|---|
| `DELEGATION_MAX_CONCURRENT_CHILDREN` | `3` | max concurrent workers per dispatch batch; mirrors Hermes `_DEFAULT_MAX_CONCURRENT_CHILDREN`. Read per-call (no cache) so tests can override in-process. Non-integer / missing → falls back to 3. |

### 4. Validation & Error Matrix

| Condition | Result |
|---|---|
| Pure batch, d=3 (≤ limit) | 3 workers spawn concurrently; 3 tool_results in tool_use order |
| Pure batch, d=4 (> limit 3) | 4 tool_results all `is_error: true`; 0 workers spawn |
| Mixed batch (dispatch + read_file) | Falls to serial path; serial loop runs dispatch (B6) + read_file in order |
| Single dispatch (d=1) | Serial path (B6 behavior, `force_readonly=false`) — unchanged |
| `general-purpose` in concurrent branch | worker toolset stripped to the read-only allowlist (`read_file, grep, glob, list_dir, web_fetch, search_history` — 6 since D2② 2026-08-17); writes impossible |
| Parent Stop mid-batch | `parent_token` fires → all N `child_token()` fire → all workers cancelled; `cancel_parent` aggregated |
| One worker errors, others succeed | each returns its own `(content, is_error, …)`; tool_results carry per-worker `[status: …]` prefix independently |

### 5. Good / Base / Bad Cases

- **Good**: parent emits 3 `dispatch_subagent{researcher, …}` for 3 topics → 3 workers run
  concurrently (wall-clock ≈ max(single), not sum) → 3 tool_results in tool_use order.
- **Base**: parent emits 3 dispatch but one is `general-purpose` → all 3 still concurrent; the
  `general-purpose` worker is silently downgraded to read-only (its writes would be Deny'd anyway).
- **Bad (anti-pattern the gate prevents)**: parent emits 5 dispatch (> limit 3) hoping to fan out →
  hard-rejected, 0 spawn, parent told to reduce count or raise the env limit. No silent truncation
  (which would make the parent think 3 ran when it sent 5).

### 6. Tests Required

Backend (`cargo test --lib`, `agent/tests_subagent.rs`):

| Test | Asserts |
|---|---|
| `l3a_filter_tools_readonly_keeps_only_read_tools` | unit: allowlist keeps exactly the read-only tools (6 since D2② 2026-08-17: `read_file, grep, glob, list_dir, web_fetch, search_history`), strips writes incl. `dispatch_subagent` (anti-nesting pin). Renamed from `..._keeps_only_five_read_tools` when `search_history` joined. L3b PR2: no longer the concurrent-branch safety argument, but the function + test stay (L3a opt-in / future explicit read-only callers). |
| `l3a_classify_dispatch_batch_branches_correctly` | unit: all 3 branches (Serial/OverLimit/Concurrent) classified by (d, o) |
| `l3a_pure_batch_of_three_dispatches_runs_concurrently` | AC1/6: 3 workers, tool_use order preserved (asserted via persisted DB messages). L3b PR2: workers now in per-worker worktrees; the order-preservation invariant is unchanged. |
| `l3a_pure_batch_over_limit_hard_rejects_all` | AC3: 4 dispatch → all tool_error, 0 workers (call_count, runs empty) |
| `l3b_concurrent_general_purpose_workers_complete_shared` | L3b PR2: rebadged from `l3a_concurrent_general_purpose_workers_complete_readonly`. 2 general-purpose workers with `isolation: false` dispatch override (truth table: `frontmatter Some(true)` + `dispatch Some(false)` → NOT isolated) → both complete with `[status: completed]`. Pins that the L3a "concurrent batch completes" contract is reachable via the new isolation-truth-table path (no git fixture required). |
| `l3a_concurrent_cancel_propagates_to_all_workers` | AC4: parent cancel → 3 cancelled tool_results + 3 cancelled runs + parent Done{cancelled}. L3b PR2: cancel fan-out is unchanged. |
| `l3a_concurrent_token_usage_does_not_fold_into_parent` | 2026-06-26 reversal: worker emits usage → parent's `last_context_input_tokens` reflects ONLY the parent's own turn (NOT the worker sum); worker usage lives in `subagent_runs.token_usage_json`. The old fold-into-parent design caused the 1.7M / 100% context-occupancy blowup. Companion: `l3a_concurrent_token_usage_folds_into_parent` (in earlier PRs) is obsolete. |
| `l3a_mixed_batch_falls_through_to_serial_path` | AC7: dispatch + read_file → serial path |
| `l3a_single_dispatch_runs_serial_path_unchanged` | regression: d=1 → B6 serial behavior, `force_readonly=false`. L3b PR2: continues to pin the serial path; the concurrent branch no longer passes `force_readonly=true` but the single-dispatch serial path still passes `false` (unchanged). |
| `l3b_concurrent_general_purpose_workers_complete_with_writes` | L3b PR2 AC1: 2 general-purpose workers in pure batch, each in own `worker/<run_id>` worktree, both complete with `[status: completed]`. 2 `subagent_runs` rows persist (one per worker). Requires `make_harness_with_git_repo` (PR1 init-repo helper promoted to `tests_common.rs`). |
| `l3b_concurrent_workers_have_isolated_worktrees` | L3b PR2 AC2: concurrent workers' `worker_run_id` UUIDs are distinct (per-run isolation primitive) + `parent_request_id` (worker_rid) is distinct + each worker_rid carries its `tool_use_id` suffix. `subagent_runs.worktree_path` post-exit is `None` (destroy path clears) — see "post-destroy 列清空" caveat in IMPLEMENTATION §4 2026-06-27 L3b PR2 ADR. |
| `l3b_concurrent_force_readonly_param_no_longer_set` | L3b PR2 AC3: regression — concurrent branch no longer threads `force_readonly=true` to `run_subagent`. Worker turn's tools contain `write_file` / `edit_file` / `shell` (L3a 5-tool strip is removed); `dispatch_subagent` / `update_checklist` still excluded by `STRUCTURALLY_DISABLED` (defense-in-depth). |

### 7. Wrong vs Correct — concurrency race handling

#### Wrong: add explicit locks / channels for the 3 race points

```rust
// BAD — re-inventing concurrency control the existing architecture already provides.
let permit = Arc::new(Semaphore::new(MAX));   // ← the batch size IS the gate (hard-reject handles over-limit)
let ask_mutex = Arc::new(Mutex::new(()));     // ← worker is_worker=true collapses ask to Deny; no concurrent ask exists
let usage_mutex = Arc::new(Mutex::new(()));   // ← add_token_usage is col = COALESCE(col,0)+? atomic SQL; no read-modify-write
```

The 3 race points are **dissolved by scope**, not by new synchronization (full derivation in
`agent-loop-architecture/pattern-concurrent-dispatch.md §"Race dissolution by scope"`):
1. `permission:ask` — worker `is_worker=true` → Tier 4 `ask` → `Decision::Deny` (no oneshot wait);
   read tools are low-Tier silent-allow. **No concurrent interactive ask can occur.**
2. `token usage` — `add_token_usage` / `add_token_usage_streaming` are `col = COALESCE(col,0) + ?`
   atomic increment; SQLite's single-writer lock serializes. **No lost updates.**
3. `cancellations` — each worker registers a unique `worker_rid = "{parent_rid}-sub-{tool_use_id}"`;
   `worker_token = parent_token.child_token()` × N → parent cancel fires all children. **Free fan-out.**

#### Correct: reuse the existing architecture + the L2 parallel template

```rust
// GOOD — the concurrent branch is the L2 read-only-batch path with
// run_subagent(force_readonly=false) in place of execute_tool (L3b PR2;
// pre-PR2 was force_readonly=true with the read-only scope as the
// safety argument). No new locks; the per-worker worktree scope IS the
// safety argument. L3a legacy code path still passes force_readonly=true
// for opt-in read-only callers (L3a regression-pinned tests).
let result_slots: Vec<Option<ContentBlock>> = (0..n).map(|_| None).collect();
let cancelled_flag = Arc::new(AtomicBool::new(false));
let mut fu: FuturesUnordered<_> = dispatches.enumerate().map(|(i, (id, input))| async move {
    // L3b PR2 (2026-06-27): force_readonly=false (was true under L3a).
    // Per-worker worktree isolation (L3b PR1) is the new safety
    // argument. Pre-PR2 the read-only scope was the only thing
    // preventing cwd write races between concurrent workers.
    let (content, is_error, cancel_parent, _) =
        run_subagent(/* …shared-ref deps… */, /*force_readonly=*/ false).await;
    if cancel_parent { cancelled_flag.store(true, Ordering::SeqCst); }
    Some((i, ContentBlock::ToolResult { tool_use_id: id, content, is_error }))
}).collect();
while let Some(Some((i, block))) = fu.next().await { result_slots[i] = Some(block); }
let result_blocks = result_slots.into_iter().flatten().collect();
```

> **Invariant to preserve on any future edit (L3b PR2 update)**: post-PR2, the concurrent
> branch **IS widened** to allow write-capable workers — the safety argument is per-worker
> worktree isolation, not the read-only scope. The race-dissolution proof in
> `agent-loop-architecture/pattern-concurrent-dispatch.md §"Pattern: Concurrent isolated dispatch (L3b PR2)"` is the new
> contract. If the worktree isolation is ever weakened (e.g. concurrent workers land on the
> same `worker/<run_id>` branch, or `worktree_override` is bypassed), the proof breaks —
> re-derive it before lifting the safety. **Do NOT add another "concurrent write" mechanism
> that bypasses per-worker worktree isolation.**

### `dispatch_subagent` worktree-isolation input (L3b PR1, 2026-06-27)

L3b PR1 extends `dispatch_subagent` with a `isolation: Option<bool>` input parameter and a matching `SubagentDef.isolation: Option<bool>` frontmatter field. The merge semantics (`resolve_isolation(frontmatter_default, dispatch_input) -> bool` in `agent/subagent/resolve.rs`):

| frontmatter `isolation` | dispatch `isolation` | result |
|---|---|---|
| `Some(true)` | not specified | isolated |
| `Some(true)` | `Some(false)` | shared (LLM opted out) |
| `Some(false)` or `None` | `Some(true)` | isolated (LLM opted in) |
| `Some(false)` or `None` | not specified | shared (legacy behavior) |
| `Some(false)` or `None` | `Some(false)` | shared |
| `Some(true)` | `Some(true)` | isolated |

Precedence: **dispatch input > frontmatter default > not isolated**.

#### Builtin defaults

- `general-purpose`: `isolation: None` (shared) — **B (2026-06-30)**: changed from `Some(true)`. Single serial dispatch reuses the parent cwd (zero merge, matches Claude Code). Concurrent-write isolation moved to `chat_loop.rs`'s `DispatchBatch::Concurrent` branch (force-isolate-writable), so this default no longer carries it.
- `researcher`: `isolation: None` — read-only workers don't need a separate checkout; saves the per-dispatch checkout cost.

#### Tool schema addition

```json
{
  "isolation": {
    "type": "boolean",
    "description": "Override the subagent's worktree-isolation decision for THIS dispatch only. When `true`, the worker runs in its own git worktree on branch `worker/<run_id>`; when `false`, the worker reuses the parent session's checkout (legacy B6 behavior). Precedence: this input overrides the subagent's frontmatter default. See `agent-loop-architecture.md` §worktree_override + `worktree-contract/worker-variant.md` for the runtime behavior."
  }
}
```

### L3b PR2 update on the concurrent dispatch warning above (2026-06-27)

L3b PR2 **fully closes** the "concurrent branch write-capable" warning above.
The `chat_loop.rs` concurrent dispatch branch no longer passes `force_readonly=true`;
concurrent workers run with `force_readonly=false` and rely on per-worker worktree
isolation (L3b PR1) for the race-dissolution. `general-purpose` builtin defaults to
`isolation: Some(true)`, so concurrent workers land their writes on independent
`worker/<run_id>` branches — no cwd write race.

The race-dissolution proof in the preceding "Concurrent dispatch" scenario block has
been **re-derived** against the new isolated-write scope and lives in
`.trellis/spec/backend/agent-loop-architecture/pattern-concurrent-dispatch.md` §"Pattern: Concurrent isolated
dispatch (L3b PR2, 2026-06-27)". Net changes from the L3a proof:

- **New row in the race table**: **worktree write race** — provably dissolved by
  per-worker `worker/<run_id>` branch + `libgit2 worktree add` (concurrent worktrees
  under the same `.git/` are safe by git's own design).
- **`permission:ask` row modified**: worker `is_worker=true` no longer collapses
  Tier 4 `ask` to `Deny` (post-2026-06-22 RULE-FrontSubagent-003 routes through
  `WorkerAskBanner`). N concurrent workers CAN now each fire a banner; this is
  accepted per the L3a PRD's pre-emptive note (workaround: pre-AllowAlways in
  parent turn).
- **`token usage` row modified**: NOT folded into parent (2026-06-26 reversal
  of RULE-A-015/PR2a). Worker token lives in `subagent_runs.token_usage_json` only —
  no shared `sessions.last_*` column → no lost update by construction.
- **`cancellations` row unchanged** from L3a.

`force_readonly` parameter is **retained** on `run_subagent` (serial-only post-PR2)
for L3a regression compat + a future "explicit read-only opt-in" feature.

### B update: serial-default-shared + parallel-force-isolate + auto-commit (2026-06-30)

Task `06-30-ab-autocommit-shared-default` (parent `06-30-subagent-worktree-smooth`).
Two changes to the isolation model above + one bug fix:

1. **`general-purpose` default `Some(true)` → `None`** (Builtin defaults above).
   Single serial dispatch is now shared (zero merge).
2. **Concurrent-write safety re-anchored**: the `DispatchBatch::Concurrent` branch
   in `chat_loop.rs` passes a new `run_subagent` param `parallel=true`; the decision
   becomes `dispatch input > (parallel && worker_is_writable) > def default`. A
   concurrent batch defaults writable workers to isolated (replacing the old
   "general-purpose defaults to isolated" argument) **but** an explicit
   `isolation: false` still opts out (precedence preserved). Read-only concurrent
   workers (`researcher`) stay shared — no write race, saves the checkout.
3. **Auto-commit fix (`commit_worker_changes`)**: `run_subagent` now commits the
   worker's working-tree changes onto `worker/<run_id>` right after
   `probe_worker_changes` reports `has_changes`. This closes the merge
   false-success gap: `probe` diffs the working tree but `do_merge_blocking`
   merges branch tips — without the auto-commit, an uncommitted worker left
   `worker_tip == parent_tip` and `merge_worker` returned "fast-forward" with
   zero changes merged (`merge_worker.rs:651` `==` short-circuit).

`resolve_isolation` signature is unchanged; the `resolve_isolation_*` truth-table
tests still pass. `cargo test --lib`: 1081 passed.
