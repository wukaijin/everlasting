//! Probe whether a directory is a git repository (cached as
//! `projects.is_git_repo`).
//!
//! We deliberately do **not** depend on the `git2` crate here — this is
//! the first time the project would touch git, and step 4 will own the
//! worktree logic. A short `git -C <path> rev-parse --show-toplevel`
//! shell-out is the lowest-cost "is this a git repo" check that works
//! across the platforms we target (Linux / WSL / macOS / Windows).
//!
//! Returns `true` iff the command exits 0 and stdout is a non-empty
//! absolute path.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use tokio::time::timeout;

/// Synchronous, blocking variant. Use this from sync code paths (e.g.
/// Tauri command bodies that don't want to be async).
pub fn is_git_repo_sync(path: &Path) -> bool {
    match Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--show-toplevel"])
        .output()
    {
        Ok(out) => {
            if !out.status.success() {
                return false;
            }
            let s = String::from_utf8_lossy(&out.stdout);
            let trimmed = s.trim();
            // `git rev-parse --show-toplevel` exits 0 with output "."
            // when the cwd is a git repo and prints the toplevel
            // absolute path otherwise. An empty stdout means something
            // is off — treat as "not a git repo" defensively.
            !trimmed.is_empty()
        }
        Err(_) => false,
    }
}

/// Async variant with a short timeout (1s) so a slow `git` invocation
/// cannot stall the Tauri command. Returns `false` on any error.
#[allow(dead_code)]
pub async fn is_git_repo(path: &Path) -> bool {
    let p = path.to_path_buf();
    match timeout(
        Duration::from_secs(1),
        tokio::task::spawn_blocking(move || is_git_repo_sync(&p)),
    )
    .await
    {
        Ok(Ok(b)) => b,
        // Either the timeout fired or the blocking task panicked.
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Helper: a directory that *is* a git repo (initialized with
    /// `git init`). Skips the test if `git` is not on PATH.
    fn try_init_git(dir: &Path) -> bool {
        let status = Command::new("git")
            .arg("init")
            .arg("--quiet")
            .arg(dir)
            .status();
        matches!(status, Ok(s) if s.success())
    }

    #[tokio::test]
    async fn non_git_directory_returns_false() {
        let dir = tempdir().unwrap();
        assert!(!is_git_repo(dir.path()).await);
    }

    #[tokio::test]
    async fn git_directory_returns_true() {
        let dir = tempdir().unwrap();
        if !try_init_git(dir.path()) {
            // `git` not on PATH — skip rather than fail in this
            // minimal environment.
            eprintln!("git not available; skipping test");
            return;
        }
        assert!(is_git_repo(dir.path()).await);
    }

    #[tokio::test]
    async fn nonexistent_directory_returns_false() {
        let dir = tempdir().unwrap();
        let ghost = dir.path().join("nope");
        assert!(!is_git_repo(&ghost).await);
    }
}
