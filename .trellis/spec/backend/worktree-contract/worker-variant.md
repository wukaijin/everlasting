# Pattern: Worker Worktree Variant (`create_worker` / `destroy_worker`)

> **Source**: extracted from `worktree-contract.md` §"Pattern: Worker Worktree Variant (`create_worker` / `destroy_worker`, L3b PR1, 2026-06-27)" (2026-08-10 doc-split task).

## Pattern: Worker Worktree Variant (`create_worker` / `destroy_worker`, L3b PR1, 2026-06-27)

L3b PR1 adds a **per-run** worktree variant to the existing per-session worktree machinery, letting a `dispatch_subagent` worker execute in an isolated checkout (independent `worker/<run_id>` branch) rather than reusing the parent session's checkout. The base mechanism (3-state self-heal, create-then-prune cleanup, libgit2 commit ownership workaround via OID round-trip) is shared with the session variant — no copy-paste of recovery logic.

### Signatures

```rust
pub fn create_worker(
    project_path: &Path,           // main repo (.git dir shared with parent worktree)
    worktree_path: &Path,           // new isolated checkout under <app_data_dir>/worktrees/<project_uuid>/worker/<run_id>
    base_worktree_path: &Path,      // parent session's worktree (its HEAD is the base commit)
    run_id: &str,                   // UUID; metadata name + branch suffix
) -> Result<(), GitError>

pub fn destroy_worker(
    project_path: &Path,
    worktree_path: &Path,
    run_id: &str,
) -> Result<(), GitError>
```

### Differences from session variant

- **Branch prefix**: `worker/<run_id>` (NOT `session/<id>`). Avoids collision with concurrent workers.
- **Base commit**: parent session worktree's HEAD (`base_worktree_path.head()`), NOT project main repo HEAD.
- **Lock**: `create_worker` calls `wt.lock(Some("L3b worker active"))` after a successful worktree add; `destroy_worker` calls `wt.unlock()` before `wt.prune(None)`. libgit2 refuses to prune a locked worktree without the `force` option, so this guard prevents `git worktree prune` (sweep, manual) from sweeping the worktree mid-run. `self_heal_for_create` also unlocks before pruning stale locks from crashed previous runs.
- **Self-heal reuse**: both `create_worker` and `create` go through the shared `self_heal_for_create` + `create_worktree_add` helpers (no copy-paste of the 3 stale-state roots).

### Behavior matrix

| Caller scenario | Worker worktree path | Branch | Lock |
|---|---|---|---|
| `isolation = Some(true)` (PR1 default for general-purpose) | `<app_data_dir>/worktrees/<project_uuid>/worker/<run_id>` | `worker/<run_id>` | yes |
| `isolation = Some(false)` (researcher / override false) | reuse parent worktree_path (no extra checkout) | n/a | n/a |
| Worker exits no changes | destroyed | branch deleted | n/a |
| Worker exits has changes | preserved (PR3 merge_worker picks it up) | `worker/<run_id>` retained | unlocked (user-initiated destroy will unlock first) |

### Tests Required (`git/worktree.rs` `#[cfg(test)]`)

| Test | Asserts |
|---|---|
| `create_worker_uses_worker_branch_prefix` | branch name is `worker/<run_id>`, NOT `session/<id>` |
| `create_worker_bases_off_parent_worktree_head` | base commit equals parent worktree HEAD (even if parent has unpushed commits) |
| `create_worker_self_heals_orphan_dir` | 3-state self-heal still triggers for worker variant |
| `destroy_worker_removes_branch_and_dir` | physical dir + libgit2 metadata + branch all gone |
| (added by PR1 inline review) `create_worker` + `destroy_worker` correctly lock/unlock |

See `tests_dispatch.rs::probe_worker_changes_*` (3 tests, also added in PR1) for the end-to-end create → worker edits → diff probe flow.

---

