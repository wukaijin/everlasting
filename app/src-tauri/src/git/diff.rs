//! Diff between a session's worktree and the commit the session
//! branch was created from. Step 4's "what did the agent change
//! in this session?" answer.
//!
//! The base is the commit the `session/<id>` branch was created
//! from (which is the project's HEAD at session creation time,
//! not the current project HEAD). This shows only what THIS
//! session contributed, not cumulative project drift since the
//! session started.
//!
//! Uses libgit2's `diff_tree_to_workdir_with_index` so both
//! committed and staged-but-uncommitted changes are included.
//! Step 4 doesn't auto-commit (see prd.md Decision), so virtually
//! all changes live in the workdir — but the function works
//! identically when/if a future Skill adds commits.

use std::path::Path;

use git2::{Delta, Repository};
use serde::Serialize;

use crate::git::error::GitError;

/// One file in the diff. `path` is relative to the worktree root.
/// `status` is one of "added" / "deleted" / "modified" / "renamed"
/// / "copied" / "typechange" / "untracked" / "ignored" / "conflicted"
/// — but the common ones from a session's work are the first three.
/// `added` / `removed` are line counts (0 for binary or empty
/// files). `diff_text` is the unified diff body for this file —
/// ready to feed to a UI, no further processing required.
#[derive(Debug, Clone, Serialize)]
pub struct FileDiff {
    pub path: String,
    pub status: String,
    pub added: usize,
    pub removed: usize,
    pub diff_text: String,
}

/// The full diff for a session: the file list plus a structured
/// per-file payload. `files` is empty when the worktree matches
/// the base (e.g. immediately after worktree creation with no
/// edits).
#[derive(Debug, Clone, Serialize)]
pub struct DiffResult {
    pub files: Vec<FileDiff>,
}

/// Compute the diff between `worktree_path`'s current working dir
/// (including index) and the commit the `session/<session_id>`
/// branch points to.
///
/// Errors:
/// - `worktree_path` is not a git working tree
/// - the `session/<id>` branch doesn't exist
/// - libgit2 reports any other error during the diff
pub fn diff_worktree(worktree_path: &Path, session_id: &str) -> Result<DiffResult, GitError> {
    let repo = Repository::open(worktree_path)?;
    let branch_name = format!("session/{}", session_id);
    let branch = repo.find_branch(&branch_name, git2::BranchType::Local)?;
    let base_commit = branch.get().peel_to_commit()?;
    let base_tree = base_commit.tree()?;

    // `diff_tree_to_workdir_with_index` diffs the workdir PLUS
    // the index against the given tree. This includes any
    // `git add`-ed-but-uncommitted changes (relevant once a
    // future Skill adds staging). For step 4's no-commit model
    // the index is empty in practice, so the result is just the
    // workdir-vs-base diff.
    let diff = repo.diff_tree_to_workdir_with_index(Some(&base_tree), None)?;

    let mut files: Vec<FileDiff> = Vec::new();
    for (idx, delta) in diff.deltas().enumerate() {
        let status = match delta.status() {
            Delta::Added => "added",
            Delta::Deleted => "deleted",
            Delta::Modified => "modified",
            Delta::Renamed => "renamed",
            Delta::Copied => "copied",
            Delta::Typechange => "typechange",
            Delta::Untracked => "untracked",
            Delta::Ignored => "ignored",
            Delta::Conflicted => "conflicted",
            // Unmodified deltas don't show up in `diff.deltas()`
            // (they're filtered by the diff engine), but cover
            // them anyway for the non-exhaustive case. Unreadable
            // deltas indicate an I/O error; surface as "unreadable"
            // so the UI can flag it instead of crashing.
            Delta::Unmodified | Delta::Unreadable => "unreadable",
        };

        // Prefer the new file's path (handles renames where the
        // old path is "before" and the new path is "after"). For
        // pure deletions, fall back to the old path.
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        // `Patch::from_diff` is indexed: pass the delta's position
        // in the diff. It returns Option<Patch> — None if the
        // delta doesn't have a patchable diff (e.g. binary files
        // or submodules). For those, we still report the file but
        // with empty diff_text and 0/0 stats.
        let (added, removed, diff_text) = match git2::Patch::from_diff(&diff, idx) {
            Ok(Some(mut patch)) => {
                // `Patch::line_stats` returns (additions, deletions,
                // context_lines). We only need the first two for
                // the +/- count summary.
                let (a, d, _) = patch.line_stats().unwrap_or((0, 0, 0));
                let text = patch
                    .to_buf()
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default();
                (a, d, text)
            }
            Ok(None) => (0, 0, String::new()),
            Err(e) => {
                tracing::warn!(
                    path = %path,
                    error = %e,
                    "patch generation failed for diff delta; reporting empty diff"
                );
                (0, 0, String::new())
            }
        };

        files.push(FileDiff {
            path,
            status: status.to_string(),
            added,
            removed,
            diff_text,
        });
    }

    // Sort by path for stable UI rendering — the diff deltas are
    // not guaranteed to be in any particular order.
    files.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(DiffResult { files })
}
