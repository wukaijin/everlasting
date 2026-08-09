# Pattern: Worker Worktree Sweep (L3b PR3)

> **Source**: extracted from `worktree-contract.md` §"Pattern: Worker Worktree Sweep (L3b PR3, 2026-06-27)" (2026-08-10 doc-split task).

## Pattern: Worker Worktree Sweep (L3b PR3, 2026-06-27)

PR3 adds a one-shot startup sweep that destroys
worker worktrees whose mtime is older than a configurable
retention period AND whose libgit2 lock is NOT present
(a locked worktree is an active worker — sweeping it
would destroy the in-flight worker's checkout
mid-execution). Closes the disk-leak + branch-list
pollution loop that PR1 introduced (worker branches +
worktrees accumulate forever otherwise).

### Signatures

```rust
pub const DEFAULT_CLEANUP_PERIOD_DAYS: u32 = 7;
pub const CLEANUP_PERIOD_DAYS_ENV: &str = "EVERLASTING_CLEANUP_PERIOD_DAYS";

pub fn sweep_stale_worker_worktrees(
    app_data_dir: &Path,
    project_uuid: &str,
    project_path: &Path,         // explicit, not derived
    cleanup_period_days: u32,
) -> Result<usize, GitError>;     // count destroyed

pub fn resolve_cleanup_period_days(explicit: Option<u32>) -> u32;
```

### Behavior

| Step | Source of truth |
|------|-----------------|
| Walk `<app_data_dir>/worktrees/<project_uuid>/worker/` for subdirectories | one entry per worker run_id |
| **Lock check**: `repo.find_worktree(run_id).is_locked() == Locked(_)` → SKIP | libgit2 API; `WorktreeLockStatus::Locked(_)` is the canonical "active worker" signal |
| **Mtime check**: `now - mtime > cleanup_period_days * 86400` → destroy | `std::fs::metadata(&wt_path).modified()` |
| Destroy via `git::worktree::destroy_worker(project_path, &wt_path, run_id)` | same best-effort semantics as the explicit discard path |
| Increment destroyed counter; continue to next entry | best-effort: a single failure doesn't abort the sweep |

### Tests Required (`git::tests_worktree`)

| Test | Asserts |
|------|---------|
| `sweep_removes_stale_worker_worktrees` | 30-day-old worker, unlocked → sweep returns 1; worktree + branch destroyed |
| `sweep_skips_locked_worker_worktrees` | 30-day-old worker, locked → sweep returns 0; worktree + branch preserved |
| `sweep_keeps_recent_worker_worktrees` | 0-day-old worker → sweep returns 0; worktree preserved |
| `sweep_with_no_worker_dir_is_noop` | no worker dir → sweep returns 0 (no error) |
| `resolve_cleanup_period_days_prefers_explicit` | `Some(14)` → 14 |
| `resolve_cleanup_period_days_uses_default_when_no_env` | `None` + env unset → 7 (or env value if set) |

### Startup wiring

`lib.rs::run` spawns a one-shot background task at startup
that walks every `projects` DB row and calls
`sweep_stale_worker_worktrees(...)` for each. The task runs
`tauri::async_runtime::spawn` (not awaited) so the Tauri
window's first paint isn't blocked by the sweep. Per-project
failures log `warn!` and the sweep continues to the next
project. The total destroyed count is emitted as a
`tracing::info!` event at the end (only when `> 0` — the
common no-op case stays quiet).

### Env override

`EVERLASTING_CLEANUP_PERIOD_DAYS=<N>` (parsed as `u32`).
A value of `0` is treated as unset (the sweep's "0 days"
interpretation would destroy every unlocked worker worktree
on every startup, which is clearly not what the user
intended if they accidentally set the env var to `0`).

---

