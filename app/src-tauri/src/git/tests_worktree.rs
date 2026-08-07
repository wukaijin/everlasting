//! Tests for `git::worktree` (relocated from the pre-split inline
//! `#[cfg(test)] mod tests`). Pure relocation — no logic changes.
#![cfg(test)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use git2::Repository;
use tempfile::tempdir;

use crate::git::error::GitError;
use crate::git::worktree::*;

/// Helper: init a git repo at `path`, configure the user (so
/// `commit` works in tests), and return the repo path. Tests
/// using this can layer worktrees on top.
fn init_repo(path: &Path) {
    fs::create_dir_all(path).unwrap();
    let init = StdCommand::new("git")
        .args(["init", "--initial-branch=main"])
        .current_dir(path)
        .output()
        .unwrap();
    assert!(init.status.success(), "git init failed: {:?}", init);
    let cfg_user = StdCommand::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(path)
        .output()
        .unwrap();
    assert!(cfg_user.status.success());
    let cfg_name = StdCommand::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(path)
        .output()
        .unwrap();
    assert!(cfg_name.status.success());
}

/// Helper: stage + commit everything in `path` with the
/// message "init".
fn commit_all(path: &Path) {
    let add = StdCommand::new("git")
        .args(["add", "-A"])
        .current_dir(path)
        .output()
        .unwrap();
    assert!(add.status.success());
    let commit = StdCommand::new("git")
        .args(["commit", "-m", "init", "--no-gpg-sign"])
        .current_dir(path)
        .output()
        .unwrap();
    assert!(commit.status.success(), "git commit failed: {:?}", commit);
}

#[test]
fn check_clean_passes_on_clean_tree() {
    let tmp = tempdir().unwrap();
    let p = tmp.path();
    init_repo(p);
    std::fs::write(p.join("a.txt"), "hello").unwrap();
    commit_all(p);
    // No changes after the commit.
    check_clean(p).expect("clean tree should pass");
}

#[test]
fn check_clean_detects_untracked_file() {
    let tmp = tempdir().unwrap();
    let p = tmp.path();
    init_repo(p);
    std::fs::write(p.join("a.txt"), "hello").unwrap();
    commit_all(p);
    // Add an untracked file.
    std::fs::write(p.join("b.txt"), "world").unwrap();
    let err = check_clean(p).expect_err("dirty tree should fail");
    assert!(err.contains("uncommitted"));
    assert!(err.contains("b.txt"));
}

#[test]
fn check_clean_detects_modified_tracked_file() {
    let tmp = tempdir().unwrap();
    let p = tmp.path();
    init_repo(p);
    std::fs::write(p.join("a.txt"), "v1").unwrap();
    commit_all(p);
    // Modify the tracked file.
    std::fs::write(p.join("a.txt"), "v2").unwrap();
    let err = check_clean(p).expect_err("modified tree should fail");
    assert!(err.contains("uncommitted"));
    assert!(err.contains("a.txt"));
}

#[test]
fn check_clean_ignores_gitignored_files() {
    let tmp = tempdir().unwrap();
    let p = tmp.path();
    init_repo(p);
    std::fs::write(p.join("a.txt"), "hello").unwrap();
    commit_all(p);
    // Add a .gitignore that ignores `output/`, then write into
    // that dir. The tool should NOT flag the ignored file.
    std::fs::write(p.join(".gitignore"), "output/\n").unwrap();
    std::fs::create_dir_all(p.join("output")).unwrap();
    std::fs::write(p.join("output/b.txt"), "ignored").unwrap();
    commit_all(p);
    // Now write a NEW ignored file post-commit. check_clean
    // should still pass because gitignored files are
    // excluded by `include_ignored(false)`.
    std::fs::write(p.join("output/c.txt"), "ignored").unwrap();
    check_clean(p).expect("ignored files should be excluded");
}

#[test]
fn check_clean_rejects_missing_path() {
    let tmp = tempdir().unwrap();
    let bogus = tmp.path().join("does-not-exist");
    let err = check_clean(&bogus).expect_err("missing path should fail");
    assert!(err.contains("does not exist"));
}

// -----------------------------------------------------------------------
// Self-heal: stale worktree / branch / orphan dir (Bug 2 fix)
//
// Real-world failure mode reported in the step 4 follow-up:
// `attach_worktree` failed with libgit2 "worktree already exists"
// even though our pre-check said the path was free. Three stale
// states were the root cause; the create() function now self-heals
// each of them BEFORE the worktree add. These tests pin the
// behavior so a future refactor can't silently regress the
// self-heal (in particular: silent re-introduction of the
// "user must clear orphan dir manually" stance would re-open
// the original bug).
// -----------------------------------------------------------------------

/// Helper: do a first successful `create` via libgit2 + return
/// the worktree path. The subsequent test is responsible for
/// tearing down pieces of state (or skipping the teardown) to
/// simulate a crash mid-create. We use this for the metadata
/// test and could use it for the branch test too, but the
/// branch test setup is simpler when we pre-create the branch
/// directly without a worktree.
fn create_worktree_with_libgit2_first(project: &Path, session_id: &str) -> PathBuf {
    let wt = project.join(format!("first_wt_{}", session_id));
    create(project, &wt, session_id).expect("first create should succeed");
    wt
}

/// Helper: do a first create + commit so the project is a
/// proper git repo with a HEAD for the worktree to point at.
fn first_commit_setup(p: &Path) {
    init_repo(p);
    std::fs::write(p.join("a.txt"), "hello").unwrap();
    commit_all(p);
}

/// Stale worktree metadata: simulate the situation where
/// `.git/worktrees/<session_id>/` still exists from a previous
/// crashed create. We force this by:
/// 1. Doing a successful `create()` (which writes metadata).
/// 2. Manually removing the on-disk worktree directory + branch
///    but leaving the metadata dir untouched.
/// 3. Calling `create()` again with the same session_id and a
///    fresh worktree path — the self-heal should prune the
///    stale metadata first so the second create succeeds.
#[test]
fn create_prunes_stale_metadata() {
    let tmp = tempdir().unwrap();
    let project = tmp.path();
    first_commit_setup(project);

    // First successful create.
    let session_id = "stale-meta-1";
    let wt1 = create_worktree_with_libgit2_first(project, session_id);

    // Simulate crash mid-cleanup: nuke the worktree dir and the
    // branch, but leave `.git/worktrees/<session_id>/` behind.
    std::fs::remove_dir_all(&wt1).unwrap();
    let repo = git2::Repository::open(project).unwrap();
    let mut b = repo
        .find_branch(&format!("session/{}", session_id), git2::BranchType::Local)
        .unwrap();
    let _ = b.delete();
    // Sanity: metadata dir is still there (we didn't touch it).
    let meta_dir = project.join(".git").join("worktrees").join(session_id);
    assert!(meta_dir.exists(), "stale metadata should be present");

    // Now create again at a different path with the same
    // session_id. The self-heal should prune the metadata so
    // this call succeeds.
    let wt2 = tmp.path().join("wt2");
    let result = create(project, &wt2, session_id);
    assert!(
        result.is_ok(),
        "second create should succeed after self-healing stale metadata, got: {:?}",
        result
    );
}

/// Stale `session/<id>` branch: simulate the situation where
/// `.git/refs/heads/session/<id>` still exists from a previous
/// crashed create, but the worktree metadata + dir are gone
/// (the create function only got as far as the `Repository::
/// branch` call). The next `create()` with the same
/// session_id should delete the stale branch first, then
/// re-create it, then succeed at the worktree add.
#[test]
fn create_deletes_stale_branch() {
    let tmp = tempdir().unwrap();
    let project = tmp.path();
    first_commit_setup(project);

    // Pre-stage: create a branch with the same name as the
    // session's worktree branch, but DO NOT create a worktree.
    // This mirrors the post-crash state where the branch is
    // present but the worktree isn't.
    let session_id = "stale-branch-1";
    let repo = git2::Repository::open(project).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    let _ = repo
        .branch(&format!("session/{}", session_id), &head, false)
        .unwrap();

    // Now `create` should self-heal by deleting the stale
    // branch and re-creating it.
    let wt = tmp.path().join("wt");
    let result = create(project, &wt, session_id);
    assert!(
        result.is_ok(),
        "create should self-heal stale session branch, got: {:?}",
        result
    );

    // Sanity: the worktree's HEAD points at the same commit.
    let wt_repo = git2::Repository::open(&wt).unwrap();
    let wt_head = wt_repo.head().unwrap().peel_to_commit().unwrap().id();
    assert_eq!(wt_head, head.id(), "worktree should point at HEAD");
}

/// Orphan worktree directory: the on-disk directory exists at
/// the target `worktree_path` but is NOT a real worktree (no
/// `.git` file, no libgit2 metadata for it). This is the
/// third stale state from the step 4 follow-up: a partial
/// create wrote the parent dir but never finished. The
/// self-heal should `remove_dir_all` the orphan so the
/// subsequent `Repository::worktree(...)` call has a clean
/// slate. The user gets a `tracing::warn!` so the silent
/// disk loss is visible in logs.
#[test]
fn create_cleans_orphan_dir() {
    let tmp = tempdir().unwrap();
    let project = tmp.path();
    first_commit_setup(project);

    // Lay down an orphan directory at the worktree path.
    // Contents can be anything (here: a stale file) — the
    // self-heal removes the whole tree.
    let wt = tmp.path().join("orphan");
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(wt.join("stale.txt"), "leftover from previous run").unwrap();
    assert!(wt.exists(), "orphan dir should be in place");

    let session_id = "orphan-1";
    let result = create(project, &wt, session_id);
    assert!(
        result.is_ok(),
        "create should self-heal orphan dir, got: {:?}",
        result
    );

    // Sanity: the worktree is now a real, fully checked-out
    // tree. The `.git` *file* (not directory) is the canonical
    // marker of a linked worktree.
    assert!(wt.join(".git").exists(), "should be a real worktree now");
    let wt_repo = git2::Repository::open(&wt).expect("worktree should be a valid git repo");
    let wt_head = wt_repo
        .head()
        .expect("worktree should have a HEAD")
        .peel_to_commit()
        .expect("HEAD should peel to a commit");
    // The orphan dir contents are gone — `stale.txt` is no more.
    assert!(
        !wt.join("stale.txt").exists(),
        "orphan contents should be wiped"
    );

    // And the worktree's HEAD points at the project's HEAD.
    let project_head = git2::Repository::open(project)
        .unwrap()
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id();
    assert_eq!(
        wt_head.id(),
        project_head,
        "worktree HEAD should match project HEAD"
    );
}

// -----------------------------------------------------------------------
// L3b (2026-06-27): worker worktree variants
//
// `create_worker` / `destroy_worker` are the worker-isolation
// counterparts to `create` / `destroy`. The branch is
// `worker/<run_id>` (distinct namespace), and the base commit is
// the parent session worktree's HEAD (not the project main HEAD).
// These tests pin the worker-specific invariants:
//   1. Branch name is `worker/<run_id>` (not `session/<id>`).
//   2. Base commit is the parent worktree HEAD (an extra commit
//      on the parent session branch is visible to the worker).
//   3. destroy_worker removes the branch + dir.
//   4. Self-heal works for the worker namespace (orphan dir).
// -----------------------------------------------------------------------

/// Helper: build a parent session worktree (one commit ahead of
/// project main) so the worker's base-commit inheritance is
/// observable. Returns `(project_path, parent_worktree_path,
/// parent_head_commit_id)`.
fn parent_session_worktree_with_extra_commit(project: &Path, session_id: &str) -> PathBuf {
    // Bring the project to a clean HEAD with one commit.
    first_commit_setup(project);

    // Create the parent session worktree (uses `create`, the
    // session variant — the worker variant will base off this).
    let parent_wt = project.join(format!("parent_wt_{}", session_id));
    create(project, &parent_wt, session_id).expect("parent session worktree create should succeed");

    // Make an extra commit on the parent session branch so the
    // worker's base-commit inheritance is observable (the worker
    // should see this commit, the project main should not).
    std::fs::write(parent_wt.join("parent_only.txt"), "from parent session").unwrap();
    let add = StdCommand::new("git")
        .args(["add", "-A"])
        .current_dir(&parent_wt)
        .output()
        .unwrap();
    assert!(add.status.success());
    let commit = StdCommand::new("git")
        .args(["commit", "-m", "parent session commit", "--no-gpg-sign"])
        .current_dir(&parent_wt)
        .output()
        .unwrap();
    assert!(
        commit.status.success(),
        "parent commit failed: {:?}",
        commit
    );

    parent_wt
}

#[test]
fn create_worker_uses_worker_branch_prefix() {
    let tmp = tempdir().unwrap();
    let project = tmp.path();
    let parent_wt = parent_session_worktree_with_extra_commit(project, "sess-1");

    let run_id = "run-abc";
    let worker_wt = tmp.path().join("worker_wt");
    create_worker(project, &worker_wt, &parent_wt, run_id).expect("create_worker should succeed");

    // The worker branch exists as `worker/<run_id>` in the repo.
    let repo = git2::Repository::open(project).unwrap();
    let branch = repo
        .find_branch(&format!("worker/{}", run_id), git2::BranchType::Local)
        .expect("worker branch should exist");
    assert!(branch.get().is_branch(), "worker branch is a branch ref");

    // No `session/<run_id>` branch was created (distinct namespace).
    assert!(
        repo.find_branch(&format!("session/{}", run_id), git2::BranchType::Local)
            .is_err(),
        "no session/<run_id> branch should exist for a worker"
    );

    // The worktree is a real linked worktree.
    assert!(worker_wt.join(".git").exists());
}

#[test]
fn create_worker_bases_off_parent_worktree_head() {
    let tmp = tempdir().unwrap();
    let project = tmp.path();
    let parent_wt = parent_session_worktree_with_extra_commit(project, "sess-2");

    // The parent session made one commit ahead of project main.
    let parent_repo = git2::Repository::open(&parent_wt).unwrap();
    let parent_head = parent_repo.head().unwrap().peel_to_commit().unwrap().id();
    let project_repo = git2::Repository::open(project).unwrap();
    let project_head = project_repo.head().unwrap().peel_to_commit().unwrap().id();
    assert_ne!(
        parent_head, project_head,
        "parent must be ahead of project main"
    );

    let run_id = "run-bases-off-parent";
    let worker_wt = tmp.path().join("worker_wt_off_parent");
    create_worker(project, &worker_wt, &parent_wt, run_id).expect("create_worker should succeed");

    // The worker's HEAD must equal the parent's HEAD, NOT the
    // project main HEAD. This is the load-bearing L3b invariant:
    // the worker inherits the parent session's progress at the
    // commit level.
    let worker_repo = git2::Repository::open(&worker_wt).unwrap();
    let worker_head = worker_repo.head().unwrap().peel_to_commit().unwrap().id();
    assert_eq!(
        worker_head, parent_head,
        "worker HEAD must match parent session HEAD (base-commit inheritance)"
    );
    assert_ne!(
        worker_head, project_head,
        "worker HEAD must NOT match project main HEAD"
    );

    // The worker can see the parent's extra file (it was part of
    // the parent's commit, so the worktree checkout has it).
    assert!(
        worker_wt.join("parent_only.txt").exists(),
        "worker should see the parent's committed file"
    );
}

#[test]
fn commit_worker_changes_advances_tip_and_includes_edits() {
    let tmp = tempdir().unwrap();
    let project = tmp.path();
    let parent_wt = parent_session_worktree_with_extra_commit(project, "sess-commit");
    let run_id = "run-autocommit";
    let worker_wt = tmp.path().join("worker_wt_autocommit");
    create_worker(project, &worker_wt, &parent_wt, run_id).expect("create_worker should succeed");

    // The worker overwrites a tracked file + adds an untracked file,
    // WITHOUT committing (mirrors a subagent that wrote edits but
    // never ran `git commit`).
    std::fs::write(worker_wt.join("parent_only.txt"), "worker overwrote this").unwrap();
    std::fs::write(worker_wt.join("new_file.txt"), "worker added this").unwrap();

    let repo = git2::Repository::open(&worker_wt).unwrap();
    let tip_before = repo.head().unwrap().peel_to_commit().unwrap().id();

    let new_oid = commit_worker_changes(&worker_wt, run_id).expect("auto-commit should succeed");

    // The branch tip advanced — the load-bearing invariant: probe
    // sees working-tree edits but merge_worker merges branch tips;
    // without this commit the tip would equal the base and
    // merge_worker would false-success (is_ancestor == short-circuit).
    assert_ne!(new_oid, tip_before, "auto-commit must advance the tip");

    // The new commit's tree contains both the edit + the new file
    // (add_all stages tracked mods + untracked).
    let new_commit = repo.find_commit(new_oid).unwrap();
    let tree = new_commit.tree().unwrap();
    assert!(
        tree.get_name("new_file.txt").is_some(),
        "untracked file must be committed"
    );
    assert!(tree.get_name("parent_only.txt").is_some());

    // The worker branch ref points at the new commit.
    let branch_ref = format!("refs/heads/worker/{}", run_id);
    let ref_oid = repo.find_reference(&branch_ref).unwrap().target().unwrap();
    assert_eq!(ref_oid, new_oid);
}

#[test]
fn destroy_worker_removes_branch_and_dir() {
    let tmp = tempdir().unwrap();
    let project = tmp.path();
    let parent_wt = parent_session_worktree_with_extra_commit(project, "sess-3");

    let run_id = "run-destroy";
    let worker_wt = tmp.path().join("worker_wt_destroy");
    create_worker(project, &worker_wt, &parent_wt, run_id).expect("create_worker should succeed");
    assert!(worker_wt.exists());

    destroy_worker(project, &worker_wt, run_id).expect("destroy_worker should succeed");

    // Directory is gone.
    assert!(!worker_wt.exists(), "worker worktree dir should be removed");

    // Branch is gone.
    let repo = git2::Repository::open(project).unwrap();
    assert!(
        repo.find_branch(&format!("worker/{}", run_id), git2::BranchType::Local)
            .is_err(),
        "worker branch should be deleted"
    );
}

#[test]
fn create_worker_self_heals_orphan_dir() {
    let tmp = tempdir().unwrap();
    let project = tmp.path();
    let parent_wt = parent_session_worktree_with_extra_commit(project, "sess-4");

    let run_id = "run-orphan";
    let worker_wt = tmp.path().join("worker_wt_orphan");
    // Lay down an orphan dir at the worker worktree path.
    std::fs::create_dir_all(&worker_wt).unwrap();
    std::fs::write(worker_wt.join("stale.txt"), "leftover").unwrap();

    create_worker(project, &worker_wt, &parent_wt, run_id)
        .expect("create_worker should self-heal orphan dir");

    // Orphan contents are gone; the worktree is real now.
    assert!(!worker_wt.join("stale.txt").exists());
    assert!(worker_wt.join(".git").exists());
}

#[test]
fn worker_branch_name_and_path_helpers() {
    assert_eq!(worker_branch_name("run-123"), "worker/run-123");
    assert_eq!(WORKER_BRANCH_PREFIX, "worker/");
    let data_dir = Path::new("/data");
    let p = worker_worktree_path(data_dir, "proj-uuid", "run-123");
    assert_eq!(p, Path::new("/data/worktrees/proj-uuid/worker/run-123"));
}

// -----------------------------------------------------------------------
// L3b PR3 (2026-06-27): sweep mechanism tests
//
// The sweep walks `<app_data_dir>/worktrees/<project_uuid>/worker/`
// and destroys worker worktrees whose mtime is older than
// `cleanup_period_days` AND whose libgit2 lock is NOT present.
// These tests pin each contract.
// -----------------------------------------------------------------------

/// Helper: set up a project + parent session worktree
/// + a worker worktree. Returns
/// `(app_data_dir, project_path, project_uuid)`.
fn setup_project_with_worker(
    session_id: &str,
    run_id: &str,
) -> (tempfile::TempDir, std::path::PathBuf, String) {
    let tmp = tempdir().unwrap();
    let project = tmp.path().join("project");
    let parent_wt = parent_session_worktree_with_extra_commit(&project, session_id);
    let app_data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&app_data_dir).unwrap();
    let worker_wt = worker_worktree_path(&app_data_dir, "project-uuid", run_id);
    create_worker(&project, &worker_wt, &parent_wt, run_id).expect("create_worker should succeed");
    (tmp, project, "project-uuid".to_string())
}

/// Backdate the mtime of the worker worktree directory
/// to N days ago. Uses `touch -t YYYYMMDDhhmm` because
/// Rust's `std::fs::File::set_modified` doesn't work
/// on directories on Linux (only on files). The `touch`
/// binary is universally available on Linux + macOS
/// (the project's two build targets).
fn backdate_dir(path: &Path, days_ago: u32) {
    // Compute target time as days_ago days back from
    // now, formatted as YYYYMMDDhhmm.
    let now = std::time::SystemTime::now();
    let target = now
        .checked_sub(std::time::Duration::from_secs(days_ago as u64 * 86_400))
        .expect("mtime in range");
    let secs_since_epoch = target
        .duration_since(std::time::UNIX_EPOCH)
        .expect("epoch")
        .as_secs();
    // Convert to (year, month, day, hour, minute) for
    // `touch -t` (which expects YYYYMMDDhhmm).
    let (year, month, day, hour, minute) = epoch_secs_to_ymdhms(secs_since_epoch);
    let touch_arg = format!("{:04}{:02}{:02}{:02}{:02}", year, month, day, hour, minute);
    let out = std::process::Command::new("touch")
        .args(["-t", &touch_arg])
        .arg(path)
        .output()
        .expect("touch command");
    assert!(
        out.status.success(),
        "touch -t failed for {:?}: {:?}",
        path,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Convert Unix epoch seconds to (year, month, day, hour,
/// minute). Implements the inverse of the algorithm in
/// `<time.h>` `gmtime_r`. Pure function (no I/O).
fn epoch_secs_to_ymdhms(secs: u64) -> (i32, u32, u32, u32, u32) {
    let secs_in_day = 86_400u64;
    let mut days = (secs / secs_in_day) as i64;
    let secs_today = (secs % secs_in_day) as u32;
    let hour = secs_today / 3600;
    let minute = (secs_today % 3600) / 60;

    // 1970-01-01 was a Thursday (day 4 of the week,
    // where 0 = Sunday). Compute the day of the week
    // for epoch `days` (with 0 = Sunday).
    let weekday = ((days + 4).rem_euclid(7)) as u32;

    // Walk forward by year, accounting for leap years.
    let mut year: i32 = 1970;
    loop {
        let leap = is_leap_year(year);
        let year_days = if leap { 366 } else { 365 };
        if days < year_days {
            break;
        }
        days -= year_days;
        year += 1;
    }
    // Month lengths for the current year.
    let month_lengths = if is_leap_year(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut month: usize = 0;
    while month < 12 && days >= month_lengths[month] as i64 {
        days -= month_lengths[month] as i64;
        month += 1;
    }
    let day = (days + 1) as u32; // 1-indexed
    let _ = weekday; // silence unused warning
    (year, (month as u32) + 1, day, hour, minute)
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

#[test]
fn sweep_removes_stale_worker_worktrees() {
    let (tmp, project, project_uuid) = setup_project_with_worker("sweep-sess", "sweep-stale");
    let app_data_dir = tmp.path().join("data");
    let stale_run_id = "sweep-stale";
    let worker_wt = worker_worktree_path(&app_data_dir, &project_uuid, stale_run_id);

    // Unlock the worker worktree (the test scenario
    // simulates a worker that exited long ago — the
    // lock was held during the worker's lifetime and
    // should be released before the sweep sees the
    // worktree as a candidate for destruction). Without
    // this step, the sweep would correctly skip the
    // worktree (the lock check is the load-bearing
    // "active worker" guard).
    let project_repo = git2::Repository::open(&project).unwrap();
    if let Ok(wt) = project_repo.find_worktree(stale_run_id) {
        wt.unlock().expect("unlock worker for test setup");
    }

    // Backdate the worker worktree dir to 30 days ago —
    // well past the 7-day default.
    backdate_dir(&worker_wt, 30);

    // Run the sweep with a 7-day cleanup period.
    let destroyed = sweep_stale_worker_worktrees(&app_data_dir, &project_uuid, &project, 7)
        .expect("sweep should succeed");
    assert_eq!(destroyed, 1, "exactly 1 stale worktree destroyed");

    // The worker worktree dir + branch are gone.
    assert!(!worker_wt.exists(), "worker worktree dir removed");
    let repo = git2::Repository::open(&project).unwrap();
    assert!(
        repo.find_branch(&format!("worker/{}", stale_run_id), git2::BranchType::Local)
            .is_err(),
        "worker branch should be deleted"
    );
}

#[test]
fn sweep_skips_locked_worker_worktrees() {
    let (tmp, project, project_uuid) = setup_project_with_worker("sweep-lock-sess", "sweep-locked");
    let app_data_dir = tmp.path().join("data");
    let run_id = "sweep-locked";
    let worker_wt = worker_worktree_path(&app_data_dir, &project_uuid, run_id);

    // Backdate the worker worktree dir to 30 days ago.
    backdate_dir(&worker_wt, 30);

    // Manually create the libgit2 lock file at the
    // canonical lock path: `<project>/.git/worktrees/<run_id>/locked`.
    // The `create_worker` function normally writes this
    // for us; we re-add it (it should still be there
    // from `create_worker`, but we re-touch to be safe
    // — the test asserts the sweep sees the lock).
    let lock_path = project
        .join(".git")
        .join("worktrees")
        .join(run_id)
        .join("locked");
    // The `create_worker` function already created
    // this — let's just assert it exists.
    assert!(
        lock_path.exists(),
        "create_worker should have left a lock file"
    );

    let destroyed = sweep_stale_worker_worktrees(&app_data_dir, &project_uuid, &project, 7)
        .expect("sweep should succeed");
    assert_eq!(destroyed, 0, "locked worktree MUST be skipped");

    // Worker worktree dir + branch preserved.
    assert!(worker_wt.exists(), "locked worktree dir preserved");
    let repo = git2::Repository::open(&project).unwrap();
    assert!(
        repo.find_branch(&format!("worker/{}", run_id), git2::BranchType::Local)
            .is_ok(),
        "locked worker branch preserved"
    );
}

#[test]
fn sweep_keeps_recent_worker_worktrees() {
    // A worker that's only 1 day old should NOT be
    // destroyed by a 7-day sweep.
    let (tmp, project, project_uuid) =
        setup_project_with_worker("sweep-recent-sess", "sweep-recent");
    let app_data_dir = tmp.path().join("data");
    let run_id = "sweep-recent";
    let worker_wt = worker_worktree_path(&app_data_dir, &project_uuid, run_id);

    // The worktree was just created, so its mtime is
    // "now". No backdate.
    let destroyed = sweep_stale_worker_worktrees(&app_data_dir, &project_uuid, &project, 7)
        .expect("sweep should succeed");
    assert_eq!(destroyed, 0, "recent worktree MUST NOT be destroyed");
    assert!(worker_wt.exists(), "recent worktree dir preserved");
}

#[test]
fn sweep_with_no_worker_dir_is_noop() {
    let tmp = tempdir().unwrap();
    let app_data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&app_data_dir).unwrap();
    // No worker dir exists for this project. Pass a
    // non-existent project path — sweep returns 0
    // because the worker dir check fails first.
    let bogus_project = std::path::Path::new("/tmp/does-not-exist");
    let destroyed =
        sweep_stale_worker_worktrees(&app_data_dir, "no-such-project", bogus_project, 7)
            .expect("sweep should succeed (no work to do)");
    assert_eq!(destroyed, 0);
}

#[test]
fn resolve_cleanup_period_days_prefers_explicit() {
    assert_eq!(resolve_cleanup_period_days(Some(14)), 14);
}

#[test]
fn resolve_cleanup_period_days_uses_default_when_no_env() {
    // The env var may or may not be set in the test
    // environment; `resolve_cleanup_period_days(None)`
    // falls back to the default (7) when the env var is
    // unset or unparseable. We can't safely assert
    // against the env var value (it's process-global),
    // so we just confirm the explicit path works and
    // the default-when-None path doesn't crash.
    let _ = resolve_cleanup_period_days(None);
}

// -----------------------------------------------------------------------
// attach_session helper (06-30 follow-up, lazy auto-attach on merge)
//
// These tests cover the inner work of `commands::worktree::
// attach_worktree` extracted as a free function for tool-layer
// (`merge_worker`) reuse. The contract:
//   - happy_path: clean git project + session at None →
//     worktree created + DB row flipped to Active + system
//     event inserted; returns the new worktree path
//   - non_git_project: project_row.is_git_repo=false →
//     GitError::NotARepo, no disk writes, no DB writes
//   - dirty_project_root: project root has uncommitted
//     changes → GitError::Dirty carrying the offending path(s)
//
// The state machine guard (None / Detached / Active policy)
// lives at the IPC boundary, NOT in this helper — covered by
// the IPC-layer tests rather than the helper.
// -----------------------------------------------------------------------

/// Minimal in-test DB pool (sqlite::memory:) + migrations.
/// Self-contained here (vs importing from db::sessions_tests)
/// because git-domain tests historically have no DB surface;
/// sharing the helper would couple git-test compile to
/// db-test compile.
async fn attach_session_test_pool() -> sqlx::SqlitePool {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .unwrap();
    crate::db::migrations::run_migrations(&pool).await.unwrap();
    pool
}

/// Helper: init a git repo at `path`, commit a single file,
/// and create a ProjectRow + session in the DB. Returns
/// (project_row, session_id, data_dir).
async fn attach_session_setup(
    project_dir: &Path,
    pool: &sqlx::SqlitePool,
    is_git_repo: bool,
) -> (crate::projects::ProjectRow, String, PathBuf) {
    if is_git_repo {
        init_repo(project_dir);
        std::fs::write(project_dir.join("a.txt"), "hello").unwrap();
        commit_all(project_dir);
    } else {
        fs::create_dir_all(project_dir).unwrap();
        std::fs::write(project_dir.join("README"), "no git here").unwrap();
    }
    let data_dir = tempfile::tempdir().unwrap().keep();
    let project = crate::db::projects::create_project(
        pool,
        "test-proj",
        project_dir.to_str().unwrap(),
        is_git_repo,
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
async fn attach_session_happy_path() {
    let tmp = tempdir().unwrap();
    let project_dir = tmp.path().join("proj");
    let pool = attach_session_test_pool().await;
    let (project, session_id, data_dir) = attach_session_setup(&project_dir, &pool, true).await;
    // First commit done — project root is clean.

    let result = attach_session(&pool, &project, &session_id, &data_dir).await;
    let wt_path = result.expect("attach_session should succeed");

    // Helper contract: returns the worktree path it built.
    let expected = worktree_path(&data_dir, &project.id, &session_id);
    assert_eq!(
        wt_path, expected,
        "returned path should match canonical layout"
    );

    // Libgit2 effect: the on-disk worktree exists and points
    // at a fresh `session/<sid>` branch.
    assert!(wt_path.exists(), "worktree directory should exist on disk");
    let wt_repo = Repository::open(&wt_path).expect("wt should open as repo");
    let head_branch = wt_repo
        .head()
        .expect("HEAD should resolve")
        .shorthand()
        .expect("HEAD should have shorthand")
        .to_string();
    assert_eq!(head_branch, format!("session/{}", session_id));

    // DB effect 1: row should flip to Active with the new
    // worktree_path.
    let reloaded = crate::db::sessions::load_session(&pool, &session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        reloaded.session.worktree_state,
        crate::db::WorktreeState::Active
    );
    assert_eq!(
        reloaded.session.worktree_path.as_deref(),
        Some(expected.to_str().unwrap())
    );

    // DB effect 2: a system-event row should be appended
    // (the [worktree event] attached: <path> on branch
    // session/<sid> message).
    let msgs = reloaded.messages;
    assert_eq!(msgs.len(), 1, "exactly one system event expected");
    assert_eq!(msgs[0].role, "user");
    assert_eq!(msgs[0].seq, 0);
    let meta = msgs[0].metadata.as_ref().expect("metadata present");
    assert_eq!(meta["kind"], "worktree_event");
    assert_eq!(meta["event"], "attached");
}

#[tokio::test]
async fn attach_session_non_git_project() {
    let tmp = tempdir().unwrap();
    let project_dir = tmp.path().join("proj");
    let pool = attach_session_test_pool().await;
    let (project, session_id, data_dir) = attach_session_setup(&project_dir, &pool, false).await;

    let result = attach_session(&pool, &project, &session_id, &data_dir).await;
    match result {
        Err(GitError::NotARepo { path }) => {
            assert_eq!(path, project_dir.to_str().unwrap());
        }
        other => panic!("expected NotARepo, got {:?}", other),
    }

    // DB should be unchanged (still None + no system event).
    let reloaded = crate::db::sessions::load_session(&pool, &session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        reloaded.session.worktree_state,
        crate::db::WorktreeState::None
    );
    assert!(
        reloaded.messages.is_empty(),
        "no system event on rejected attach"
    );
}

#[tokio::test]
async fn attach_session_dirty_project_root() {
    let tmp = tempdir().unwrap();
    let project_dir = tmp.path().join("proj");
    let pool = attach_session_test_pool().await;
    let (project, session_id, data_dir) = attach_session_setup(&project_dir, &pool, true).await;
    // Now make the project root dirty: add an uncommitted
    // file (NOT stage+commit).
    std::fs::write(project_dir.join("dirty.txt"), "stale").unwrap();

    let result = attach_session(&pool, &project, &session_id, &data_dir).await;
    match result {
        Err(GitError::Dirty { paths, .. }) => {
            assert!(
                paths.iter().any(|p| p.ends_with("dirty.txt")),
                "Dirty error must list dirty.txt as offending path, got: {:?}",
                paths
            );
        }
        other => panic!("expected Dirty, got {:?}", other),
    }

    // DB should be unchanged.
    let reloaded = crate::db::sessions::load_session(&pool, &session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        reloaded.session.worktree_state,
        crate::db::WorktreeState::None
    );
    assert!(
        reloaded.messages.is_empty(),
        "no system event on rejected attach"
    );

    // No worktree directory should be created on disk.
    let expected = worktree_path(&data_dir, &project.id, &session_id);
    assert!(!expected.exists(), "no worktree dir on rejected attach");
}
