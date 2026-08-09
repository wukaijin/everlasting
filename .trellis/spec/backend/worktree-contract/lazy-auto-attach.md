# Pattern: Lazy Auto-Attach on Merge (06-30 follow-up)

> **Source**: extracted from `worktree-contract.md` §"Pattern: Lazy Auto-Attach on Merge (06-30 follow-up)" (2026-08-10 doc-split task).

## Pattern: Lazy Auto-Attach on Merge (06-30 follow-up)

The `merge_worker` tool and the `merge_worker_run` IPC command
both reach into a parent session's `session/<id>` worktree.
Both entry points historically hard-failed with
`"parent session has no worktree"` (IPC) or
`"parent branch '<id>' not found (parent session has no
worktree?)"` (tool path) when the parent was at
`WorktreeState::None`. This was a UX trap because session
creation is opt-in (`create_session` always starts at
`None`), so any user who dispatched an isolated worker
(`general-purpose` default `isolated=true`) without first
manually attaching a worktree would hit this wall at merge
time.

### Helper contract

`merge_worker::ensure_parent_worktree_attached(db, data_dir,
parent_session_id) -> Result<bool, String>` — called identically
by both entry points before `do_merge_blocking`:

| Parent state | Helper returns | Side effects |
|---|---|---|
| `Active` | `Ok(false)` | none |
| `Detached` | `Ok(false)` | none (do NOT re-attach — respect user intent) |
| `None` | `Ok(true)` | calls `git::worktree::attach_session` → DB row flips to Active, `[worktree event] attached:` row injected |
| none (helper error: non-git project, dirty root, etc.) | `Err(reason)` | none; caller surfaces verbatim |

The Detached → no-op decision is INV-M3 and is the only
non-obvious choice. The user explicitly tore down their
worktree (via `detach_worktree`), and silently re-attaching
on merge would override that intent. The merge then fails
downstream with a recognizable, actionable error so the
user can attach via the chat header manually.

### Helper layering

- `git::worktree::attach_session(db, project, session_id,
  data_dir) -> Result<PathBuf, GitError>` — extracted from
  `commands::worktree::attach_worktree` so the IPC and
  tool-layer call sites share the same disk + DB + system
  event injection path. The IPC's `attach_worktree`
  command body now delegates to this helper.
- `git::worktree::attach_session` does NOT include the
  state-machine guard — each caller enforces its own
  policy. The IPC command rejects `Active` (already
  attached) and accepts `None` / `Detached`; the
  merge_worker helper has the tri-state policy above.

### IPC return value

`merge_worker_run` returns `Result<MergeWorkerResult, String>`:
```rust
#[derive(Serialize, serde::rename_all = "camelCase")]
pub struct MergeWorkerResult {
    pub message: String,                  // libgit2 outcome (fast-forward / 3-way / conflict text)
    pub auto_attached_parent: bool,       // true iff this call did a lazy attach
}
```

Frontend `subagentRuns.mergeWorker(runId, parentSessionId?)`:
- On `Ok` with `autoAttachedParent = true`, calls
  `useChatStore().loadSessions(currentProjectId)` so the
  chat header's worktree chip flips `none → active`.
- Renders a specific toast (`"已合并到父 session 分支,并自动
  绑定了父工作区"`) only when the flag is set, so the user
  is told about the side effect.

The error arm is still a plain `String` (verbatim from
`attach_session` / `do_merge_blocking`), so `parseConflictFiles`
parsing on the frontend's catch path is unaffected.

### Validation & error matrix

| Condition | helper result | upstream message |
|---|---|---|
| Parent Active | `Ok(false)` | normal merge path |
| Parent Detached | `Ok(false)` | `merge_worker_run: parent session '<id>' is detached (no worktree bound); please attach via the chat header before merging` |
| Parent None + project non-git | `Err("project ... is not a git repository")` | `merge_worker_run: cannot auto-attach parent worktree: project ... is not a git repository` |
| Parent None + dirty project root | `Err("... uncommitted changes ...")` | `merge_worker_run: cannot auto-attach parent worktree: ...` |
| Parent None + libgit2 fail | `Err(GitError::Git2(e))` | propagated verbatim |
| Parent session row missing | `Err("parent session ... not found")` | propagated verbatim |

The tool-path tuple is `(String, true, ToolContextUpdate::default(), None)` on any error and `(...msg, false, ...)` on success. The LLM-facing return shape carries no auto-attach signal because the LLM doesn't need to render a chip — a `[worktree event] attached:` row in history is sufficient.
