#![cfg(test)]

// Unit tests for `ensure_parent_worktree_attached` (06-30
// follow-up). The helper tri-state contract must hold across
// both the IPC and tool entry points; the invariant is
// anchored here. End-to-end behavioral coverage for the IPC
// path lives in `commands/subagent_runs.rs` tests; for the
// tool path in `tests_subagent.rs`.

use crate::db::test_support::test_pool;
use crate::tools::merge_worker::*;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use tempfile::tempdir;

/// Init a git repo at `path`, add + commit a placeholder file
/// so the project root is clean (required by `attach_session`).
fn init_clean_git_repo(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    let status = StdCommand::new("git")
        .args(["init", "--initial-branch=main"])
        .current_dir(path)
        .status()
        .unwrap();
    assert!(status.success(), "git init failed");
    let _ = StdCommand::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(path)
        .output()
        .unwrap();
    let _ = StdCommand::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(path)
        .output()
        .unwrap();
    std::fs::write(path.join("a.txt"), "hello").unwrap();
    let add = StdCommand::new("git")
        .args(["add", "-A"])
        .current_dir(path)
        .status()
        .unwrap();
    assert!(add.success());
    let commit = StdCommand::new("git")
        .args(["commit", "-m", "init", "--no-gpg-sign"])
        .current_dir(path)
        .status()
        .unwrap();
    assert!(commit.success());
}

// --- merge_session_into_main (D, 2026-06-30) ---

#[test]
fn merge_session_into_main_fast_forwards_when_main_is_ancestor() {
    let project_dir = tempdir().unwrap();
    let project = project_dir.path();
    init_clean_git_repo(project);
    let session_id = "sess-ff";
    let session_branch = crate::git::worktree::branch_name(session_id);

    // Create session/<id> off main, advance it one commit.
    StdCommand::new("git")
        .args(["branch", &session_branch])
        .current_dir(project)
        .status()
        .unwrap();
    StdCommand::new("git")
        .args(["checkout", &session_branch])
        .current_dir(project)
        .status()
        .unwrap();
    std::fs::write(project.join("b.txt"), "session work").unwrap();
    StdCommand::new("git")
        .args(["add", "-A"])
        .current_dir(project)
        .status()
        .unwrap();
    StdCommand::new("git")
        .args(["commit", "-m", "session", "--no-gpg-sign"])
        .current_dir(project)
        .status()
        .unwrap();
    StdCommand::new("git")
        .args(["checkout", "main"])
        .current_dir(project)
        .status()
        .unwrap();

    let repo = git2::Repository::open(project).unwrap();
    let main_before = repo
        .find_branch("main", git2::BranchType::Local)
        .unwrap()
        .get()
        .peel_to_commit()
        .unwrap()
        .id();

    let result = merge_session_into_main(project, session_id).unwrap();
    assert!(result.contains("fast-forward"), "got: {}", result);

    let main_after = repo
        .find_branch("main", git2::BranchType::Local)
        .unwrap()
        .get()
        .peel_to_commit()
        .unwrap()
        .id();
    assert_ne!(main_before, main_after, "main must advance on fast-forward");
    // workdir updated to include the session's new file.
    assert!(project.join("b.txt").exists());
}

#[test]
fn merge_session_into_main_conflict_reports_error_and_keeps_main_clean() {
    let project_dir = tempdir().unwrap();
    let project = project_dir.path();
    init_clean_git_repo(project);
    let session_id = "sess-conf";
    let session_branch = crate::git::worktree::branch_name(session_id);

    // Both main and session modify a.txt → diverge → conflict.
    StdCommand::new("git")
        .args(["branch", &session_branch])
        .current_dir(project)
        .status()
        .unwrap();
    StdCommand::new("git")
        .args(["checkout", &session_branch])
        .current_dir(project)
        .status()
        .unwrap();
    std::fs::write(project.join("a.txt"), "session version").unwrap();
    StdCommand::new("git")
        .args(["add", "-A"])
        .current_dir(project)
        .status()
        .unwrap();
    StdCommand::new("git")
        .args(["commit", "-m", "session edit", "--no-gpg-sign"])
        .current_dir(project)
        .status()
        .unwrap();
    StdCommand::new("git")
        .args(["checkout", "main"])
        .current_dir(project)
        .status()
        .unwrap();
    std::fs::write(project.join("a.txt"), "main version").unwrap();
    StdCommand::new("git")
        .args(["add", "-A"])
        .current_dir(project)
        .status()
        .unwrap();
    StdCommand::new("git")
        .args(["commit", "-m", "main edit", "--no-gpg-sign"])
        .current_dir(project)
        .status()
        .unwrap();

    let main_before = git2::Repository::open(project)
        .unwrap()
        .find_branch("main", git2::BranchType::Local)
        .unwrap()
        .get()
        .peel_to_commit()
        .unwrap()
        .id();

    let result = merge_session_into_main(project, session_id);
    assert!(result.is_err(), "conflicting merge must error");
    let err = result.unwrap_err();
    assert!(err.contains("merge conflict"), "got: {}", err);

    // main unchanged (reset to clean HEAD — no half-merged state).
    let main_after = git2::Repository::open(project)
        .unwrap()
        .find_branch("main", git2::BranchType::Local)
        .unwrap()
        .get()
        .peel_to_commit()
        .unwrap()
        .id();
    assert_eq!(main_before, main_after, "main must not move on conflict");
    assert_eq!(
        std::fs::read_to_string(project.join("a.txt")).unwrap(),
        "main version",
        "workdir must be clean (no conflict markers)"
    );
}

/// Create a project + session row, return (project, session_id,
/// data_dir). Session is at `worktree_state=None` after this
/// call; tests that need a different state call
/// `db::set_worktree_state` to flip it explicitly.
async fn make_session(
    pool: &sqlx::SqlitePool,
    project_dir: &Path,
) -> (crate::projects::ProjectRow, String, PathBuf) {
    let data_dir = tempfile::tempdir().unwrap().keep();
    let project = crate::db::projects::create_project(
        pool,
        "merge-test",
        project_dir.to_str().unwrap(),
        true,
        None,
    )
    .await
    .unwrap();
    let session_id = uuid::Uuid::new_v4().to_string();
    crate::db::sessions::create_session(
        pool,
        &session_id,
        &project.id,
        project_dir.to_str().unwrap(),
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    (project, session_id, data_dir)
}

#[tokio::test]
async fn active_state_is_noop() {
    let pool = test_pool().await;
    let project_dir = tempdir().unwrap().keep();
    init_clean_git_repo(&project_dir);
    let (_project, session_id, data_dir) = make_session(&pool, &project_dir).await;

    // Flip to Active with a fake worktree_path (no disk effect).
    crate::db::sessions::set_worktree_state(
        &pool,
        &session_id,
        crate::db::WorktreeState::Active,
        Some("/data/fake_wt"),
        None,
    )
    .await
    .unwrap();

    let result = ensure_parent_worktree_attached(&pool, &data_dir, &session_id).await;
    assert_eq!(result, Ok(false), "Active parent must be no-op");

    // DB row's worktree_path must be UNCHANGED (we did nothing).
    let loaded = crate::db::load_session(&pool, &session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        loaded.session.worktree_path.as_deref(),
        Some("/data/fake_wt")
    );
}

#[tokio::test]
async fn detached_state_is_noop_skipped() {
    // Detached parent: we MUST NOT silently re-attach (prd
    // INV-M3). Returning `Ok(false)` lets the merge fail at
    // `do_merge_blocking` instead of overriding user intent.
    let pool = test_pool().await;
    let project_dir = tempdir().unwrap().keep();
    init_clean_git_repo(&project_dir);
    let (_project, session_id, data_dir) = make_session(&pool, &project_dir).await;

    crate::db::sessions::set_worktree_state(
        &pool,
        &session_id,
        crate::db::WorktreeState::Detached,
        None,
        Some("/data/old_wt"),
    )
    .await
    .unwrap();

    let result = ensure_parent_worktree_attached(&pool, &data_dir, &session_id).await;
    assert_eq!(result, Ok(false), "Detached parent must skip attach");

    // DB row stays Detached; no new worktree_path written.
    let loaded = crate::db::load_session(&pool, &session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        loaded.session.worktree_state,
        crate::db::WorktreeState::Detached
    );
    assert!(loaded.session.worktree_path.is_none());
}

#[tokio::test]
async fn lazy_attach_on_none_state() {
    let pool = test_pool().await;
    let project_dir = tempdir().unwrap().keep();
    init_clean_git_repo(&project_dir);
    let (project, session_id, data_dir) = make_session(&pool, &project_dir).await;

    // Initial state is None (create_session doesn't attach).
    let result = ensure_parent_worktree_attached(&pool, &data_dir, &session_id).await;
    assert_eq!(result, Ok(true), "None parent must trigger lazy attach");

    // Side effects: DB row flipped to Active with the new
    // worktree_path, and a [worktree event] row was injected.
    let loaded = crate::db::load_session(&pool, &session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        loaded.session.worktree_state,
        crate::db::WorktreeState::Active
    );
    let wt_path = loaded
        .session
        .worktree_path
        .as_deref()
        .expect("worktree_path should be set after lazy attach");
    let expected = crate::git::worktree::worktree_path(&data_dir, &project.id, &session_id);
    assert_eq!(wt_path, expected.to_str().unwrap());

    // The directory exists on disk and points at the new branch.
    assert!(std::path::Path::new(wt_path).exists());
    assert_eq!(
        loaded.messages.len(),
        1,
        "exactly one system event expected"
    );
    assert_eq!(loaded.messages[0].role, "user");
}
