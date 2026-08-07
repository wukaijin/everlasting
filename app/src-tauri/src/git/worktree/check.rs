//! check_clean: dirty-tree guard for attach/detach.
//!
//! Relocated verbatim from the pre-split `worktree.rs`.

use std::path::Path;

use git2::Repository;

/// Check that a git working directory (project root, worktree, or
/// any other tree) has **no uncommitted or untracked changes**.
/// Returns `Ok(())` for a clean tree, `Err(message)` for a dirty
/// one. The error message lists offending paths so the user knows
/// what to commit/stash.
///
/// Used by:
/// - `lib.rs::attach_worktree` — refuses to attach if the
///   project's main working directory has uncommitted changes
///   (the new worktree would diverge from a dirty base).
/// - `lib.rs::detach_worktree` — refuses to detach if the
///   worktree itself has uncommitted changes (detaching would
///   strand the user's WIP — the LLM's next tool call would
///   silently lose them).
///
/// Implementation: open the repo at `repo_path` and call
/// `repo.statuses(None)`. We classify any non-ignored entry with
/// a non-zero status bits (INDEX_NEW, WT_MODIFIED, etc.) as
/// "uncommitted". Ignored files (`include_ignored: false`) are
/// skipped — `.everlasting/outputs/` doesn't count.
pub fn check_clean(repo_path: &Path) -> Result<(), String> {
    if !repo_path.exists() {
        return Err(format!(
            "worktree path '{}' does not exist (it may have been deleted on disk)",
            repo_path.display()
        ));
    }
    let repo = Repository::open(repo_path).map_err(|e| {
        format!(
            "failed to open git repo at '{}': {}",
            repo_path.display(),
            e
        )
    })?;
    let mut opts = git2::StatusOptions::new();
    opts.include_ignored(false)
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_unmodified(false);
    let statuses = match repo.statuses(Some(&mut opts)) {
        Ok(s) => s,
        Err(e) => return Err(format!("failed to read git status: {}", e)),
    };
    if statuses.is_empty() {
        return Ok(());
    }
    // Collect up to 10 offending paths for a friendly error. The
    // libgit2 StatusEntry's `path()` is the worktree-relative
    // path (e.g. `src/main.rs`); good enough for the message.
    let mut paths: Vec<String> = Vec::new();
    for entry in statuses.iter() {
        if let Some(p) = entry.path() {
            paths.push(p.to_string());
            if paths.len() >= 10 {
                break;
            }
        }
    }
    Err(format!(
        "{} has uncommitted changes{}",
        repo_path.display(),
        if paths.is_empty() {
            String::new()
        } else {
            format!(": {}", paths.join(", "))
        }
    ))
}
