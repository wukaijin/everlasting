// workerBranch — branch-name formatting helpers for the L3b PR4
// SubagentDrawer merge / discard UI.
//
// The backend persists `subagent_runs.worktree_path` as the worker's
// isolated worktree path. From that path we derive a friendly
// branch name (`worker/<run_id>` → `Worker <short hash>`) so the
// drawer's badge reads naturally. The branch name itself is NOT
// stored — it's reconstructed from the run_id when needed.
//
// Convention:
//   - The libgit2 worktree name / branch ref is `worker/<run_id>`
//     (see `git/worktree.rs::worker_branch_name`). The run_id is
//     the `subagent_runs.id` UUID.
//   - The "short hash" display form is the first 8 chars of the
//     run_id (mirrors `git log --oneline`'s 7-char default, bumped
//     to 8 to better disambiguate the v4 UUID prefix). The user
//     sees this in the drawer badge + the ConfirmDialog body.

/** Format a `worker/<run_id>` branch ref (or the bare run_id) into
 *  a display-friendly label.
 *
 *  Input is permissive — the worker path on disk is
 *  `<app_data_dir>/worktrees/<project_uuid>/worker/<run_id>` (the
 *  last path segment is the run_id), but the same run_id is also
 *  the branch suffix. We extract the run_id from either form:
 *
 *  - `worker/<run_id>` → run_id
 *  - bare run_id → run_id (passthrough)
 *  - full worktree_path → last segment is run_id
 *  - empty / malformed → empty string (caller hides the badge)
 *
 *  Output: `Worker <first-8-chars>`. Empty string when the input
 *  is empty / the run_id is too short to shorten (< 8 chars). */
export function formatWorkerBranchLabel(input: string | null | undefined): string {
  if (!input) return "";
  // Extract the run_id: prefer `worker/<run_id>` suffix, fall
  // back to the last path segment, finally passthrough the input
  // as-is (it might already be a bare run_id).
  let runId = "";
  const slashIdx = input.lastIndexOf("/");
  if (slashIdx >= 0 && slashIdx < input.length - 1) {
    runId = input.slice(slashIdx + 1);
  } else {
    runId = input;
  }
  if (!runId) return "";
  // Short hash: first 8 chars. A v4 UUID is always 36 chars
  // (8-4-4-4-12 hex groups), so 8 chars is unambiguous in the
  // drawer context. For run_ids shorter than 8 chars, use the
  // whole thing verbatim rather than a confusingly-truncated
  // fragment.
  const short = runId.length >= 8 ? runId.slice(0, 8) : runId;
  return `Worker ${short}`;
}
