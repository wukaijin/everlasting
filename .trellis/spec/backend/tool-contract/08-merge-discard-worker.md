## Scenario: `merge_worker` / `discard_worker` tools (L3b PR3, 2026-06-27)

> **Source of truth**: implementation lives in
> `app/src-tauri/src/tools/{merge_worker,discard_worker}.rs`;
> the IPC surface is `commands::subagent_runs::{merge_worker_run,
> discard_worker_run}`; the helper that destroys the worker
> worktree + clears the DB column is in
> `tools::merge_worker::finalize_merge` + `tools::discard_worker::do_discard`
> (shared between tool layer and IPC layer).
> Sweep mechanism lives in `git::worktree::sweep_stale_worker_worktrees`
> + `git::worktree::resolve_cleanup_period_days`.

### 1. Scope / Trigger

After L3b PR1 (2026-06-27) lands, an isolated worker subagent that
exits with changes leaves behind a `worker/<run_id>` branch + worktree
+ a non-NULL `subagent_runs.worktree_path` column. The user / parent
LLM has no mechanism to dispose of these artifacts — they accumulate
forever (disk leak + branch-list pollution). PR3 closes the loop
with two new builtin tools (LLM-driven) + a startup sweep
(cron-style auto-cleanup) + a matching Tauri command pair (frontend
drawer-driven, see PR4).

### 2. Signatures

#### Tool declarations (appended to `builtin_tools()`)

```rust
// app/src-tauri/src/tools/merge_worker.rs
ToolDef {
    name: "merge_worker".to_string(),
    description: Some(
        "Merge a completed worker subagent's `worker/<run_id>` branch back into the parent \
         session's branch. ..."
            .to_string(),
    ),
    input_schema: json!({
        "type": "object",
        "properties": {
            "run_id": {
                "type": "string",
                "description": "The subagent run id (the `subagent_runs.id` UUID)."
            }
        },
        "required": ["run_id"]
    })
}

// app/src-tauri/src/tools/discard_worker.rs
ToolDef {
    name: "discard_worker".to_string(),
    description: Some(
        "Discard a completed worker subagent's preserved changes (the `worker/<run_id>` \
         branch + worktree) without merging. ..."
            .to_string(),
    ),
    input_schema: json!({
        "type": "object",
        "properties": {
            "run_id": {"type": "string"}
        },
        "required": ["run_id"]
    })
}
```

#### ToolContext.db (PR3 additive change)

`ToolContext` gains a `db: SqlitePool` field so `merge_worker` /
`discard_worker` can read the `subagent_runs` row + parent project
row for the worktree destroy. Production threads the chat loop's
`db: SqlitePool` parameter into the per-turn `ToolContext`. Test
helpers use `crate::tools::test_default_pool()` (an in-memory
`OnceLock<SqlitePool>` initialised via a fresh OS thread + a
fresh `current_thread` tokio runtime — sidesteps the "Cannot
start a runtime from within a runtime" pitfall). The field is
`Clone` (Arc-internal) so the per-turn `ToolContext::clone()`
pattern is unaffected.

> **06-30 follow-up**: `ToolContext` also gains a `data_dir:
> PathBuf` field (resolved from `AppState::app_data_dir`).
> `merge_worker::execute` reads it to thread into
> `ensure_parent_worktree_attached(... &data_dir)` for the
> lazy-attach path described below. Adding the field was a
> one-time ~17-site update across test fixtures
> (`tools/*.rs::test_ctx` + `agent/tests_subagent.rs`); see
> the lazy-attach pattern in
> `backend/worktree-contract.md` §"Pattern: Lazy Auto-Attach
> on Merge" for the full contract.

#### `merge_worker` execution pipeline (libgit2 + DB)

1. Parse `run_id` from input (missing → `is_error: true`,
   "Missing required parameter: run_id").
2. Look up `subagent_runs.worktree_path` for the row (load fail →
   `is_error: true`; row missing worktree_path → `is_error: true`,
   "worker has no worktree to merge (already merged or discarded)").
3. **Lazy auto-attach the parent** (06-30): call
   `ensure_parent_worktree_attached(db, data_dir,
   parent_session_id)`. Active → no-op; Detached → no-op
   (intentional skip per INV-M3); None → creates a fresh
   `session/<id>` worktree via `git::worktree::attach_session`
   and injects the `[worktree event] attached:` system event.
   Helper error surfaces as `(error_msg, true, ...)`. After
   the helper, reload the parent row from the DB to capture
   the authoritative `worktree_path` (ctx.worktree_path is
   stale when attach happened). The detached skip → if
   `worktree_path` is still None, return the actionable
   `"merge_worker: parent session ... is detached (...)"`
   error.
4. Open the parent worktree (`ctx.worktree_path` reloaded in step 3) with libgit2 +
   resolve the parent branch (`session/<parent_session_id>`) + the
   worker branch (`worker/<run_id>`).
5. **Fast-forward path**: if the parent tip is an ancestor of the
   worker tip (`repo.merge_base(parent_tip, worker_tip) == parent_tip`),
   move the parent ref to the worker tip via `repo.reference(name,
   oid, force=true, msg)` and call `repo.checkout_head(force)` to
   update the workdir. Result: "merged {worker} (fast-forward, 0
   merge commit)".
6. **3-way merge path**: build an `AnnotatedCommit` via
   `repo.reference_to_annotated_commit(worker_branch.get())`,
   call `repo.merge(&[annotated], &mut MergeOptions::new(),
   &mut CheckoutBuilder)` with `allow_conflicts(true)`.
7. **Conflict detection**: post-merge, `repo.index().has_conflicts()`
   is true if any path is in a conflict stage (we check
   `entry.flags & 0x3000 != 0` since `IndexEntry` exposes `flags`
   directly — git2-rs 0.20 has no `flags_stage()` method). On
   conflict: collect the conflict paths, hard-reset the index +
   workdir to the parent tip via `repo.reset(parent_obj,
   ResetType::Hard, CheckoutBuilder{force, remove_untracked})`
   (so the worktree isn't left in a half-merged state — the next
   `edit_file` / `read_file` would corrupt otherwise), then
   return `is_error: true` with `"merge conflict: [<file list>].
   The worker branch '{}' and parent branch '{}' both modified
   these files. Resolve manually, then call merge_worker again
   (or discard_worker to drop the changes)."`
7. **Clean merge**: write the merge commit via `repo.commit` with
   the resolved tree (2 parents: parent + worker). Cleanup state
   via `repo.cleanup_state()`.
8. **Post-merge cleanup (best-effort)**: call
   `git::worktree::destroy_worker(project_path, &worker_wt, run_id)`
   (removes the worktree + `worker/<run_id>` branch + libgit2
   metadata), then `db::subagent_runs::set_worktree_path(pool,
   run_id, None)` to clear the `worktree_path` column. Both
   steps log `warn!` and continue on failure (the merge has
   already committed — the artifact cleanup is best-effort).

The libgit2 calls run on `tokio::task::spawn_blocking` so the
async runtime isn't blocked (libgit2 is sync I/O).

#### `merge_worker` conflict error string contract (cross-layer, L3b PR4 2026-06-27)

> The conflict return shape is a **frontend-parsed regex** of the Rust `Err(String)` payload — there is no typed JSON. Locked string format so the frontend regex doesn't drift.

**Backend emits** (Rust side, `tools/merge_worker.rs::do_merge_blocking`):
```
"merge conflict: [<file1>, <file2>, ...]. The worker branch 'worker/<run_id>' and parent branch 'session/<id>' both modified these files. Resolve manually, then call merge_worker again (or discard_worker to drop the changes)."
```

**Frontend parses** (`app/src/stores/subagentRuns.types.ts::parseConflictFiles`):
```ts
const m = errStr.match(/^merge conflict: \[([^\]]*)\]/);
// m[1] is the inner list, split on ", " (matches Rust's `conflicts.join(", ")`)
```

| Edge case | Behavior |
|---|---|
| Empty conflict list `"merge conflict: []. ..."` | `parseConflictFiles` returns `[]` (NOT `null`) — caller treats as "conflict but no file list" |
| Non-conflict error (e.g. `"worker run not found"`, `"parent session has no worktree"`) | `parseConflictFiles` returns `null` — caller treats as generic error `{ kind: "error", message }` |
| File path containing literal `, ` | Splits incorrectly — accepted as edge case, file paths with `, ` are vanishingly rare |

**Why not typed JSON for the conflict return**: the LLM-facing `is_error: true` tool result is plain `String` for human readability; the typed JSON channel is reserved for success cases. Conflict file list embedded in the human message keeps the LLM-tool path symmetric with the frontend-drawer path (both consume the same string).

**Why preserve the worker branch + worktree on conflict**: the backend hard-resets the index + workdir to parent tip (so the next `edit_file` / `read_file` doesn't see half-merged state — invariant), but KEEPS the `worker/<run_id>` branch + worktree on disk. The user resolves conflicts in git CLI, then can call `merge_worker` again (or `discard_worker` to drop). The frontend `WorkerMergeControls` keeps the Merge/Discard buttons visible after a conflict (the cached `subagent_runs.worktree_path` is unchanged on conflict).

#### `discard_worker` execution pipeline

1. Parse `run_id` (missing → `is_error: true`).
2. Load the `subagent_runs` row + the parent project row.
3. **Fail-fast on `worktree_path IS NULL`**: return
   `"worker already destroyed"` (MVP 不做幂等, per PRD
   §"Edge Cases"). Future work: track destroy state for
   idempotent `discard_worker` retries.
4. Call `git::worktree::destroy_worker(project_path, &worker_wt,
   run_id)` (best-effort — failures log + continue).
5. `db::subagent_runs::set_worktree_path(pool, run_id, None)` to
   clear the column (best-effort).

### 3. ⑨ 关 Permission Routing

Both tools are `Risk::High` (per
`permissions::types::risk_for_tool`) — they modify the user's
project history (a merge changes the parent session branch;
a discard deletes the worker branch). The Tier 4 path branch
classifies them as `ToolKind::GitMutation` (tool-level grant +
ask, mirroring `WebFetch` — the input is a `run_id` UUID, not a
filesystem path, so the permission modal renders no path-scope
row). `check_tool_grant` is called with the ACTUAL tool name, so a
grant on `discard_worker` is not confused with `merge_worker`. Plan
mode filters them out (`filter_tools_for_mode` lists `merge_worker`
/ `discard_worker` alongside the write tools). Worker subagents
cannot reach them (`STRUCTURALLY_DISABLED` — only the parent LLM /
user via the PR4 drawer may merge or discard).

The merge / destroy actions are reversible at the git level
(`git reset --hard <parent_tip>` unmerges; `git branch -D
worker/<run_id>` recreates a discarded branch + its
commits are still in the reflog if not GC'd). This is
WHY `Risk::High` and not `Risk::Critical`: a user could
recover from a bad merge / accidental discard by going to
the terminal and resetting. The same property doesn't hold
for the `shell` tool's high-risk variants (e.g. `rm -rf`).

### 4. Sweep Mechanism

#### `git::worktree::sweep_stale_worker_worktrees`

```rust
pub fn sweep_stale_worker_worktrees(
    app_data_dir: &Path,
    project_uuid: &str,
    project_path: &Path,         // NEW in PR3: explicit project path
    cleanup_period_days: u32,
) -> Result<usize, GitError>     // returns count destroyed
```

Walks `<app_data_dir>/worktrees/<project_uuid>/worker/` and,
for each subdirectory, checks:

1. **Lock check** (via libgit2 `repo.find_worktree(run_id).is_locked()`).
   Locked worktrees are SKIPPED (a locked worktree is an
   active worker — sweeping it would destroy the in-flight
   worker's checkout mid-execution).
2. **Mtime check**: `now - mtime > cleanup_period_days * 86400`
   → destroy via `git::worktree::destroy_worker(...)`. mtime
   read via `std::fs::metadata(&wt_path).and_then(|m| m.modified())`.

Best-effort: a single failure (lock check / mtime / destroy)
logs at `warn!` and the sweep continues with the next worktree.
A worktree with no libgit2 metadata (the on-disk dir exists
but `find_worktree` returns Err) is still best-effort destroyed
(destroy_worker tolerates missing metadata via its own
best-effort prune + branch-delete).

**NEW PR3 signature change**: the function now takes
`project_path: &Path` explicitly (not derived from
`app_data_dir.join("worktrees").join(project_uuid)`). Reason:
the project's main repo isn't always at that path (the user
can have a project whose checkout is in a user-chosen
location; the worktree machinery is in `app_data_dir` but the
project's primary checkout isn't). The function uses
`project_path` to open the repo + look up the libgit2
worktree list. The startup wiring in `lib.rs::sweep_stale_workers`
passes `Path::new(&project.path)` (from the `projects` DB row).

#### `git::worktree::resolve_cleanup_period_days`

```rust
pub fn resolve_cleanup_period_days(explicit: Option<u32>) -> u32;
```

Returns the cleanup period: prefer `explicit` if `Some`; fall
back to the `EVERLASTING_CLEANUP_PERIOD_DAYS` env var
(parsed as `u32`; 0 is treated as unset); fall back to
`DEFAULT_CLEANUP_PERIOD_DAYS = 7`. Used by
`sweep_stale_worker_worktrees` callers that want the env-aware
default.

#### Startup wiring

`lib.rs::run` spawns a one-shot background task at startup that
walks every `projects` DB row and calls
`sweep_stale_worker_worktrees(...)` for each. The task runs
`spawn` (not awaited) so the Tauri window's first paint isn't
blocked by the sweep. Per-project failures log `warn!` and
the sweep continues to the next project.

### 5. Validation & Error Matrix

| Condition | `merge_worker` result | `discard_worker` result |
|-----------|----------------------|------------------------|
| `run_id` missing | "Missing required parameter: run_id" (`is_error: true`) | same |
| `run_id` unknown | "worker run not found: <id>" (`is_error: true`) | same |
| Parent session has no worktree | "parent branch 'session/<id>' not found (parent session has no worktree?): ..." (`is_error: true`) | "discard_worker: ..." (only destroys, no parent read) |
| Worker has no `worktree_path` set | "worker has no worktree to merge (already merged or discarded)" (`is_error: true`) | "worker already destroyed" (`is_error: true`, fail-fast per PRD) |
| Fast-forward merge | success: "merged <worker> (fast-forward, 0 merge commit)" | n/a |
| 3-way clean merge | success: "merged <worker> into <parent> (3-way, merge commit <oid>)" | n/a |
| Conflict | `is_error: true`, "merge conflict: [<files>] ..."; both branches preserved; worktree reset to parent tip | n/a |
| `libgit2::Error` on `Repository::open` | "merge_worker: could not open parent worktree at '...': <err>" | n/a |
| `set_worktree_path` DB write fails | log-only warn; merge result still returned to LLM | log-only warn; discard result still returned |
| `destroy_worker` filesystem failure | log-only warn; merge result still returned | log-only warn; discard result still returned |
| `sweep_stale_worker_worktrees` with worker dir absent | n/a | n/a | `Ok(0)` (no-op) |
| `sweep_stale_worker_worktrees` with locked worktree | n/a | n/a | skipped, `Ok(0)` for this entry |
| `sweep_stale_worker_worktrees` with stale worktree (mtime > period) | n/a | n/a | destroyed, count incremented |
| `sweep_stale_worker_worktrees` with recent worktree (mtime ≤ period) | n/a | n/a | skipped, `Ok(0)` for this entry |

### 6. Concurrency

`do_merge_blocking` serialises per `parent_session_id` via
`merge_lock_for` (a `std::sync::Mutex` keyed by session id, held
across the whole libgit2 merge). Both call sites — the
`merge_worker` tool's `execute` and the `merge_worker_run` IPC
command — flow through it, so concurrent merges into the SAME
parent branch are serialised; independent sessions still merge in
parallel. This closes the race that two frontend drawer Merge
clicks (or a drawer click racing an LLM-driven merge) could
otherwise corrupt — libgit2 is not thread-safe across `Repository`
handles backing the same `.git` dir.

`discard_worker` is intentionally NOT serialised: `do_discard` only
calls `destroy_worker` (worker branch + worktree) and never touches
the parent session's git index, so concurrent discards of the same
run_id are safe (the 2nd sees `worktree_path` already NULL → "worker
already destroyed").

### 7. Tests Required

#### Backend (`cargo test --lib`)

| Test | Asserts |
|------|---------|
| `l3b_merge_worker_happy_path_fast_forward` | parent branch fast-forwards to worker tip; worktree_path column cleared; worker worktree + branch destroyed |
| `l3b_merge_worker_conflict_returns_error` | both branches modify the same file; `is_error: true` with conflict file list; both branches preserved; worktree reset to clean state |
| `l3b_merge_worker_no_parent_worktree_errors` | parent session has no worktree attached; tool returns `is_error: true` with "not found" / "worktree" error message |
| `l3b_discard_worker_happy_path` | worker has a worktree; `discard_worker` returns `is_error: false`; worktree + branch destroyed; worktree_path column cleared |
| `l3b_discard_worker_already_destroyed_errors` | row's `worktree_path` is NULL; `discard_worker` returns `is_error: true` with "worker already destroyed" |
| `sweep_removes_stale_worker_worktrees` | 30-day-old worker, unlocked → sweep returns 1; worktree + branch destroyed |
| `sweep_skips_locked_worker_worktrees` | 30-day-old worker, locked → sweep returns 0; worktree + branch preserved |
| `sweep_keeps_recent_worker_worktrees` | 0-day-old worker → sweep returns 0; worktree preserved |
| `sweep_with_no_worker_dir_is_noop` | no worker dir → sweep returns 0 |
| `resolve_cleanup_period_days_prefers_explicit` | `Some(14)` → 14 |
| `resolve_cleanup_period_days_uses_default_when_no_env` | `None` + env unset → 7 (or env value if set) |

#### Frontend (`pnpm build`)

- `vue-tsc --noEmit` must pass. The frontend changes for PR3
  are zero (the tools are LLM-driven; the drawer buttons
  land in PR4). The IPC commands (`merge_worker_run` /
  `discard_worker_run`) are registered but not yet called
  from the frontend; the tauri `invoke_handler!` accepts
  them but the TS layer doesn't need any new bindings
  for the type-check.

### 8. Design Decisions

#### Decision: `merge_worker` / `discard_worker` are tools (not just IPC commands)

**Context**: Claude Code exposes `merge_worker` / `discard_worker`
as LLM-driven tools (the LLM decides when to merge). The
frontend drawer's merge / discard buttons (PR4) reuse the
same backend via the Tauri command surface.

**Decision**: implement once as a tool; the IPC command is a
thin adapter that opens the same helper. **Why not
control-flow in `chat_loop.rs`** (the `dispatch_subagent`
pattern): these tools do NOT need the chat loop's `provider`
/ `db` / `cancellations` (no nested `run_chat_loop`); they
just need libgit2 + a DB pool. Keeping them in the tool
layer avoids the `chat_loop.rs` control-flow bloat that
`dispatch_subagent` already added.

#### Decision: failure-best-effort post-merge cleanup

**Context**: after a clean 3-way merge, the merge commit is
on disk. The worker worktree + branch destroy is
mechanically separate. If the destroy fails (e.g. branch
already gone from a manual `git branch -D`), what should
the LLM see?

**Decision**: log `warn!` and continue. The merge commit
IS the user-visible artifact; the destroy is just
artifact cleanup. A failed destroy is a state inconsistency
(missing branch on disk + worktree_path still set) that
the user can fix manually, but it doesn't invalidate the
merge itself. Alternative (fail the whole call) would
hide the successful merge from the LLM.

#### Decision: conflict fail-fast (not auto-resolve)

**Context**: a 3-way merge with conflicting file edits has
many possible resolution strategies (union / ours / theirs
/ manual). Auto-resolving an arbitrary merge conflict is
NP-hard in the general case (it's the same problem as
textual merge resolution in version control).

**Decision**: return the conflict file list and let the
user decide. The worker branch + worktree are preserved
so the user can `cd` into the worker worktree + run
`git mergetool` + commit the resolution + call
`merge_worker` again. The alternative (auto-resolve
with a simple strategy like "ours" or "theirs") would
silently produce an incorrect result in non-trivial
cases.

#### Decision: sweep default 7 days

**Context**: Claude Code uses 7 days as the default for
`cleanupPeriodDays` (the most common convention for
artifact retention). L3b PR1's worktree creation adds
~5-20 MB of disk per worker (typical code checkout);
a 7-day retention balances "user has time to notice +
recover" against "disk leak under normal usage".

**Decision**: 7-day default + `EVERLASTING_CLEANUP_PERIOD_DAYS`
env override. Tests can also pass an explicit
`cleanup_period_days` arg to bypass both.

#### Decision: libgit2 `is_locked()` (not `path.exists`)

**Context**: the original implementation checked for the
lock via `<project>/.git/worktrees/<name>/locked` file
existence. This works for the standard libgit2 lock
state, but is fragile to future libgit2 changes (e.g.
an in-process lock mode that doesn't write the file).

**Decision**: use `repo.find_worktree(run_id).is_locked()`
(the libgit2 API). If the worktree metadata is missing
(a crashed create + a manual `git worktree prune`),
treat it as unlocked (the `destroy_worker` call is
best-effort and tolerates the missing metadata).

---
