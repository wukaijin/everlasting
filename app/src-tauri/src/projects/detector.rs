//! Probe whether a directory is a git repository (cached as
//! `projects.is_git_repo`) and read its current branch (cached as
//! `projects.git_branch`).
//!
//! We deliberately do **not** depend on the `git2` crate here — this is
//! the first time the project would touch git, and step 4 will own the
//! worktree logic. A short `git -C <path> rev-parse …` shell-out is
//! the lowest-cost read-only probe that works across the platforms we
//! target (Linux / WSL / macOS / Windows).
//!
//! `is_git_repo_sync` returns `true` iff `git rev-parse --show-toplevel`
//! exits 0 with non-empty stdout.
//!
//! `current_branch_sync` shells out to
//! `git -C <path> rev-parse --abbrev-ref HEAD`. The literal string
//! `"HEAD"` is returned verbatim for detached HEAD so the UI can
//! distinguish a real branch from a detached state.

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

/// Synchronous, blocking variant of the current-branch probe. Use this
/// from sync code paths (e.g. Tauri command bodies that don't want to
/// be async).
///
/// Returns `None` when:
/// - the path is not a git repo,
/// - the `git` binary is missing or fails,
/// - the output is empty.
///
/// For a detached HEAD the literal string `"HEAD"` is returned (not
/// `None`); the UI uses this to distinguish a real branch from the
/// detached state. v1 does not attach a short SHA.
pub fn current_branch_sync(path: &Path) -> Option<String> {
    if !is_git_repo_sync(path) {
        return None;
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("rev-parse")
        .arg("--abbrev-ref")
        .arg("HEAD")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8(output.stdout).ok()?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Async variant of `current_branch_sync` with the same 1s timeout
/// guard as `is_git_repo`. Returns `None` on any error / timeout.
#[allow(dead_code)]
pub async fn current_branch(path: &Path) -> Option<String> {
    let p = path.to_path_buf();
    match timeout(
        Duration::from_secs(1),
        tokio::task::spawn_blocking(move || current_branch_sync(&p)),
    )
    .await
    {
        Ok(Ok(branch)) => branch,
        // Either the timeout fired or the blocking task panicked.
        _ => None,
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

    /// Helper: configure an identity in the given repo so that
    /// `git checkout -b` and similar don't fail with "Please tell me
    /// who you are". Idempotent.
    fn try_config_identity(dir: &Path) {
        let _ = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["config", "user.email", "test@example.com"])
            .status();
        let _ = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["config", "user.name", "test"])
            .status();
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

    #[tokio::test]
    async fn current_branch_non_git_returns_none() {
        let dir = tempdir().unwrap();
        assert!(current_branch(dir.path()).await.is_none());
    }

    #[tokio::test]
    async fn current_branch_nonexistent_returns_none() {
        let dir = tempdir().unwrap();
        let ghost = dir.path().join("nope");
        assert!(current_branch(&ghost).await.is_none());
    }

    #[tokio::test]
    async fn current_branch_default_branch_name() {
        let dir = tempdir().unwrap();
        if !try_init_git(dir.path()) {
            eprintln!("git not available; skipping test");
            return;
        }
        try_config_identity(dir.path());
        // Make a commit so HEAD points to a real ref; on a fresh
        // `git init` some versions of git leave HEAD unborn which
        // makes `rev-parse --abbrev-ref HEAD` return `main` or `master`
        // depending on the init.defaultBranch config (which is what
        // we want to test — the underlying branch name).
        std::fs::write(dir.path().join("README.md"), "x").unwrap();
        let _ = Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["add", "."])
            .status();
        let _ = Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["commit", "--quiet", "-m", "init"])
            .status();

        let branch = current_branch(dir.path()).await;
        // The exact name depends on the user's init.defaultBranch
        // (main / master / etc.) — accept any non-empty, non-"HEAD"
        // branch name.
        let branch = branch.expect("expected a branch name");
        assert!(
            !branch.is_empty() && branch != "HEAD",
            "expected a real branch name, got {:?}",
            branch
        );
    }

    #[tokio::test]
    async fn current_branch_named_branch() {
        let dir = tempdir().unwrap();
        if !try_init_git(dir.path()) {
            eprintln!("git not available; skipping test");
            return;
        }
        try_config_identity(dir.path());
        // Make an initial commit so HEAD points to a real ref.
        // `git symbolic-ref HEAD refs/heads/feature/foo` refuses to
        // run if the target ref doesn't exist, so we need at least
        // one commit before we can point HEAD at a different branch.
        std::fs::write(dir.path().join("README.md"), "x").unwrap();
        let _ = Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["add", "."])
            .status();
        let _ = Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["commit", "--quiet", "-m", "init"])
            .status();
        // Create a new branch by switching to it.
        let status = Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["checkout", "-q", "-b", "feature/foo"])
            .status();
        if !matches!(status, Ok(s) if s.success()) {
            eprintln!("git checkout -b failed; skipping test");
            return;
        }

        let branch = current_branch(dir.path()).await;
        assert_eq!(branch.as_deref(), Some("feature/foo"));
    }

    #[tokio::test]
    async fn current_branch_detached_head_returns_head_literal() {
        let dir = tempdir().unwrap();
        if !try_init_git(dir.path()) {
            eprintln!("git not available; skipping test");
            return;
        }
        try_config_identity(dir.path());
        // Make an initial commit so HEAD points to a real SHA, then
        // detach by checking out the SHA. `git checkout --detach <sha>`
        // is the documented way to put HEAD in detached state.
        std::fs::write(dir.path().join("README.md"), "x").unwrap();
        let _ = Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["add", "."])
            .status();
        let commit_status = Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["commit", "--quiet", "-m", "init"])
            .status();
        if !matches!(commit_status, Ok(s) if s.success()) {
            eprintln!("git commit failed; skipping test");
            return;
        }
        let sha = String::from_utf8(
            Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(["rev-parse", "HEAD"])
                .output()
                .map(|o| o.stdout)
                .unwrap_or_default(),
        )
        .unwrap_or_default()
        .trim()
        .to_string();
        if sha.is_empty() {
            eprintln!("could not read HEAD SHA; skipping test");
            return;
        }
        let detach_status = Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["checkout", "--quiet", "--detach", &sha])
            .status();
        if !matches!(detach_status, Ok(s) if s.success()) {
            eprintln!("git checkout --detach failed; skipping test");
            return;
        }

        let branch = current_branch(dir.path()).await;
        assert_eq!(
            branch.as_deref(),
            Some("HEAD"),
            "detached HEAD should return the literal \"HEAD\""
        );
    }
}
