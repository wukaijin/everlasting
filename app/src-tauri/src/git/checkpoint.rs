//! Turn-boundary file checkpoints for the N2 revert loop: dangling
//! snapshot commits + umbrella refs, with a strict zero-touch
//! contract on the user's repository.
//!
//! The snapshot is a **dangling git object chain**, not a branch
//! auto-commit (see the task PRD): each turn-end snapshot captures
//! the whole working state into a tree object, wraps it in a commit
//! that updates NO reference, and pins the chain tip with a single
//! umbrella ref under `refs/everlasting/<session_id>`. The commit
//! chain is invisible to `git log` / `git branch` / `git status`
//! (the namespace lives outside `refs/heads`); session deletion
//! removes the umbrella ref and the objects become garbage-collectable.
//!
//! Zero-touch invariants (AC1, pinned by the tests below):
//! - the user's branches / HEAD are never updated
//! - the on-disk index file is never written (no `index.write()` —
//!   every index mutation here happens on the in-memory handle only;
//!   byte-for-byte + mtime + inode of `.git/index` must not change)
//! - the working directory files are only ever touched by
//!   [`restore_paths`] (the explicit revert primitive)
//!
//! Semantics boundary (group-review approved, 2026-09-20): the
//! snapshot respects `.gitignore` — untracked-and-ignored files are
//! invisible to both the turn diff and the revert; files that are
//! tracked and *later* gitignored are still captured (and their
//! deletion still propagates). gitignored-path double invisibility
//! is disclosed in the revert confirm dialog (PR3).
//!
//! Layering rule: this module only knows about `git2::Repository`
//! / Oids / paths. No DB, no daemon types — the wiring layer (PR1)
//! resolves session→repo and owns the config gate. Unit tests run
//! against throwaway repos in tempdirs.
//!
//! Known failure mode: a user repository with unresolved merge
//! conflicts (EUNMERGED entries in the index) fails
//! [`build_state_tree`] with an error — the wiring layer treats any
//! error as fail-open (warn log, no snapshot for the turn, chain
//! resumes at the next successful snapshot).

// PR1 起接线层(agent/checkpoint.rs)消费快照原语,模块级 allow 已移除;
// PR2(09-20-n2-checkpoint-revert)读面命令消费 `diff_snapshots` /
// `count_snapshot_deltas`,对应 allow 已摘。仍归 PR3 的 revert 三件
// (`compute_restore_set` / `restore_paths` 及其 RestorePath/Action/
// Outcome 类型)保留按项 allow,随 revert 命令落地摘除 —— 同
// permissions/audit.rs AuditKind 的按项处理先例。
use std::path::Path;

use serde::Serialize;

use crate::git::diff::{diff_tree_to_tree, DiffResult};
use crate::git::error::GitError;

/// Fixed commit signature for snapshot commits. Deliberately NOT
/// read from the user's git config — depending on (or polluting)
/// the user's identity config would break the zero-touch principle.
const CHECKPOINT_SIG_NAME: &str = "everlasting-daemon";
const CHECKPOINT_SIG_EMAIL: &str = "everlasting-daemon@localhost";

/// One path in a revert restore set, as computed by
/// [`compute_restore_set`] and consumed by [`restore_paths`].
#[allow(dead_code)] // PR3 revert 确认弹窗消费(RestorePath 列表)
#[derive(Debug, Clone, Serialize)]
pub struct RestorePath {
    pub path: String,
    pub action: RestoreAction,
}

/// What to do with a path when reverting to the target snapshot.
#[allow(dead_code)] // 随 RestorePath / restore_paths 进 PR3
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreAction {
    /// The target tree has the file — write the snapshot blob's
    /// content to the working directory.
    Checkout,
    /// The target tree does not have the file — delete it from the
    /// working directory.
    Delete,
}

/// Counts returned by [`restore_paths`] (feeds the PR3 revert
/// result / toast).
#[allow(dead_code)] // PR3 revert 命令消费(RevertResult)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RestoreOutcome {
    pub restored: usize,
    pub deleted: usize,
}

/// Capture the current working state of `repo` into a tree object
/// and return its oid. Zero-touch: the on-disk index file is never
/// written — all index work happens on the in-memory handle.
///
/// The capture is "HEAD tree + workdir overlay": the in-memory
/// index is first reset to HEAD's tree (or left empty for an
/// unborn-HEAD repo), then `add_all` syncs the working directory
/// on top (adds new files, updates modified files, removes deleted
/// files — including inside nested directories). Content addressing
/// makes this idempotent: an unchanged state yields the same tree
/// oid, and the caller dedupes on that (no new objects).
///
/// `add_all` with `IndexAddOption::DEFAULT` respects `.gitignore`
/// for *untracked* files; tracked files keep flowing into the
/// snapshot even if a later `.gitignore` entry matches them.
///
/// Errors when the index holds unresolved merge conflicts
/// (EUNMERGED) — fail-open at the call site.
pub fn build_state_tree(repo: &git2::Repository) -> Result<git2::Oid, GitError> {
    let mut index = repo.index()?;

    if index.has_conflicts() {
        return Err(GitError::Git2(git2::Error::new(
            git2::ErrorCode::Unmerged,
            git2::ErrorClass::Merge,
            "cannot snapshot: unresolved merge conflicts in the index",
        )));
    }

    // Rebase the in-memory index onto HEAD so the snapshot is
    // deterministic and independent of the user's staging state.
    // An unborn HEAD (no commits yet) skips this — the empty index
    // then only collects what `add_all` finds in the workdir.
    if let Ok(head) = repo.head() {
        let head_tree = head.peel_to_tree()?;
        index.read_tree(&head_tree)?;
    }

    // Mirrors `git add -A`: add new + update modified + stage
    // deletions. Everything below stays in memory; nothing here
    // persists the index (the AC1 tests pin that).
    index.add_all(["*"], git2::IndexAddOption::DEFAULT, None)?;

    // Writes the tree object into the ODB only — never the index
    // file. Contrast with `git/worktree/sweep.rs` which calls
    // `index.write()`: that is the worker auto-commit's semantics,
    // deliberately NOT ours.
    Ok(index.write_tree_to(repo)?)
}

/// Append a snapshot commit to the checkpoint chain: `repo.commit`
/// with `update_ref = None`, so the commit object exists in the ODB
/// but no reference is moved (the dangling-commit core of the
/// design). `parent = None` starts a fresh chain (first snapshot);
/// otherwise the parent is the previous snapshot's commit oid.
///
/// The commit message is diagnostics-only; the durable addressing
/// lives in the DB rows (PR1).
pub fn append_snapshot(
    repo: &git2::Repository,
    parent: Option<git2::Oid>,
    tree: git2::Oid,
    session_id: &str,
    seq: u64,
) -> Result<git2::Oid, GitError> {
    let sig = git2::Signature::now(CHECKPOINT_SIG_NAME, CHECKPOINT_SIG_EMAIL)?;
    let tree_obj = repo.find_tree(tree)?;
    let parent_commits: Vec<git2::Commit<'_>> = parent
        .map(|oid| repo.find_commit(oid))
        .transpose()?
        .into_iter()
        .collect();
    let parents: Vec<&git2::Commit<'_>> = parent_commits.iter().collect();
    let message = format!("everlasting checkpoint {session_id} seq={seq}");
    Ok(repo.commit(None, &sig, &sig, &message, &tree_obj, &parents)?)
}

/// Canonical umbrella-ref name for a session's checkpoint chain.
/// `session_id` is a UUID — a valid single ref-name component.
pub fn umbrella_ref_name(session_id: &str) -> String {
    format!("refs/everlasting/{session_id}")
}

/// Point the session's umbrella ref at `tip` (the chain-head commit),
/// creating or overwriting it. Keeps the chain reachable (git GC
/// walks from refs) and gives session deletion a single cleanup
/// handle. Via a worktree handle the ref lands in the shared
/// common-refs store — i.e. the main repository's `.git/refs/` —
/// same place for `none` / `active` / `detached` modes.
pub fn set_umbrella_ref(
    repo: &git2::Repository,
    session_id: &str,
    tip: git2::Oid,
) -> Result<(), GitError> {
    let name = umbrella_ref_name(session_id);
    // force=true: a chain append overwrites the previous tip.
    repo.reference(&name, tip, true, "everlasting checkpoint chain tip")?;
    Ok(())
}

/// Delete the session's umbrella ref. Idempotent: a missing ref is
/// a successful no-op (session deletion must not fail on double
/// cleanup). The dangling commit chain becomes garbage-collectable
/// after this.
pub fn delete_umbrella_ref(repo: &git2::Repository, session_id: &str) -> Result<(), GitError> {
    let name = umbrella_ref_name(session_id);
    match repo.find_reference(&name) {
        Ok(mut r) => {
            r.delete()?;
            Ok(())
        }
        Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Diff two snapshot trees and return the per-file result, reusing
/// the session diff view's `FileDiff` / `DiffResult` shapes (the
/// frontend consumes the same structure for turn-to-turn diffs).
pub fn diff_snapshots(
    repo: &git2::Repository,
    a_tree: git2::Oid,
    b_tree: git2::Oid,
) -> Result<DiffResult, GitError> {
    let a = repo.find_tree(a_tree)?;
    let b = repo.find_tree(b_tree)?;
    diff_tree_to_tree(repo, &a, &b)
}

/// Count the changed paths between two snapshot trees — the
/// `list_turn_checkpoints` badge's `files_changed`. Count-only on
/// purpose: no patch text, no `git --numstat` subprocess (the badge
/// needs the number of changed paths, not their bodies or line
/// counts; the full body flows through [`diff_snapshots`] when the
/// user actually opens the turn diff). Zero for identical trees
/// (baseline rows and net-zero write turns).
pub fn count_snapshot_deltas(
    repo: &git2::Repository,
    a_tree: git2::Oid,
    b_tree: git2::Oid,
) -> Result<usize, GitError> {
    let a = repo.find_tree(a_tree)?;
    let b = repo.find_tree(b_tree)?;
    let diff = repo.diff_tree_to_tree(Some(&a), Some(&b), None)?;
    Ok(diff.deltas().count())
}

/// Compute the revert restore set for going back to `target_tree`:
/// the full path set where the current working state differs from
/// the target. Files the target tree has (added / modified relative
/// to now) get [`RestoreAction::Checkout`]; files it lacks get
/// [`RestoreAction::Delete`]. Unchanged paths are absent from the
/// set and will not be touched by [`restore_paths`].
///
/// The current state is captured fresh (via [`build_state_tree`])
/// so the set is consistent with the gate-tree recomputation the
/// PR3 preview/execute flow performs.
#[allow(dead_code)] // PR3 revert preview 命令消费
pub fn compute_restore_set(
    repo: &git2::Repository,
    target_tree: git2::Oid,
) -> Result<Vec<RestorePath>, GitError> {
    let current = build_state_tree(repo)?;
    if current == target_tree {
        return Ok(Vec::new());
    }
    let current_tree = repo.find_tree(current)?;
    let target = repo.find_tree(target_tree)?;
    let diff = diff_tree_to_tree(repo, &current_tree, &target)?;

    let mut set: Vec<RestorePath> = diff
        .files
        .into_iter()
        .map(|f| RestorePath {
            action: if f.status == "deleted" {
                RestoreAction::Delete
            } else {
                RestoreAction::Checkout
            },
            path: f.path,
        })
        .collect();
    // Tree diffs are path-sorted already; sort defensively so the
    // confirm dialog renders deterministically regardless of any
    // future diff-option change.
    set.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(set)
}

/// Apply the restore set: write the target snapshot's blob content
/// into the working directory for [`RestoreAction::Checkout`] paths,
/// delete files for [`RestoreAction::Delete`] paths. Never touches
/// branches, HEAD, the index, or any path outside the set.
///
/// Checkout restores the executable bit on unix when the snapshot
/// blob was mode 100755. Delete prunes now-empty parent directories
/// (best-effort, bounded by the workdir root) so the layout matches
/// a plain checkout.
#[allow(dead_code)] // PR3 revert execute 命令消费
pub fn restore_paths(
    repo: &git2::Repository,
    target_tree: git2::Oid,
    paths: &[RestorePath],
) -> Result<RestoreOutcome, GitError> {
    let workdir = repo.workdir().ok_or_else(|| GitError::NoWorktree {
        path: repo.path().display().to_string(),
    })?;
    let tree = repo.find_tree(target_tree)?;
    let mut restored = 0usize;
    let mut deleted = 0usize;

    for p in paths {
        let abs = workdir.join(&p.path);
        match p.action {
            RestoreAction::Checkout => {
                let entry = tree.get_path(Path::new(&p.path))?;
                let blob = repo.find_blob(entry.id())?;
                if let Some(parent) = abs.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| GitError::Io {
                        path: parent.display().to_string(),
                        source: e,
                    })?;
                }
                std::fs::write(&abs, blob.content()).map_err(|e| GitError::Io {
                    path: abs.display().to_string(),
                    source: e,
                })?;
                #[cfg(unix)]
                if entry.filemode() == i32::from(git2::FileMode::BlobExecutable) {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&abs, std::fs::Permissions::from_mode(0o755));
                }
                restored += 1;
            }
            RestoreAction::Delete => {
                match std::fs::remove_file(&abs) {
                    Ok(()) => {}
                    // Already gone — revert is idempotent per path.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        return Err(GitError::Io {
                            path: abs.display().to_string(),
                            source: e,
                        })
                    }
                }
                // Prune now-empty parents (checkout semantics), up
                // to the workdir root. `remove_dir` fails on
                // non-empty dirs — exactly the guard we want.
                let mut dir = abs.parent();
                while let Some(d) = dir {
                    if d == workdir || !d.starts_with(workdir) {
                        break;
                    }
                    if std::fs::remove_dir(d).is_ok() {
                        dir = d.parent();
                    } else {
                        break;
                    }
                }
                deleted += 1;
            }
        }
    }

    Ok(RestoreOutcome { restored, deleted })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::process::Command as StdCommand;

    use tempfile::tempdir;

    // -------------------------------------------------------------------
    // Test helpers (same style as git/diff.rs and tests_worktree.rs:
    // CLI-driven repo setup, libgit2 for the unit under test).
    // -------------------------------------------------------------------

    /// Init a git repo at `path` and configure user identity so CLI
    /// commits work (tests never read the real user config).
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

    /// Stage + commit everything in `path` (CLI, message "init").
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

    /// Run `git <args>` in `dir`, assert success, return stdout.
    fn git_out(dir: &Path, args: &[&str]) -> String {
        let out = StdCommand::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {:?}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Open a libgit2 handle for `path`.
    fn open(path: &Path) -> git2::Repository {
        git2::Repository::open(path).unwrap()
    }

    /// All blob paths inside the tree at `tree_oid`, sorted. Walks
    /// nested trees so assertions read like workdir paths.
    fn tree_paths(repo: &git2::Repository, tree_oid: git2::Oid) -> Vec<String> {
        let tree = repo.find_tree(tree_oid).unwrap();
        let mut out: Vec<String> = Vec::new();
        tree.walk(git2::TreeWalkMode::PreOrder, |root, entry| {
            if entry.kind() == Some(git2::ObjectType::Blob) {
                let root = root.trim_end_matches('/');
                let name = entry.name().unwrap().to_string();
                if root.is_empty() {
                    out.push(name);
                } else {
                    out.push(format!("{root}/{name}"));
                }
            }
            0
        })
        .unwrap();
        out.sort();
        out
    }

    /// Read a path's blob content from a tree (test assertion aid).
    fn tree_blob(repo: &git2::Repository, tree_oid: git2::Oid, path: &str) -> Vec<u8> {
        let tree = repo.find_tree(tree_oid).unwrap();
        let entry = tree.get_path(Path::new(path)).unwrap();
        repo.find_blob(entry.id()).unwrap().content().to_vec()
    }

    /// Build a baseline snapshot chain of one snapshot: returns
    /// `(tree_oid, commit_oid)` for the current workdir state.
    fn snapshot(repo: &git2::Repository, sid: &str, seq: u64) -> (git2::Oid, git2::Oid) {
        let tree = build_state_tree(repo).expect("build_state_tree");
        let commit = append_snapshot(repo, None, tree, sid, seq).expect("append_snapshot");
        (tree, commit)
    }

    fn snapshot_on(
        repo: &git2::Repository,
        parent: git2::Oid,
        tree: git2::Oid,
        sid: &str,
        seq: u64,
    ) -> git2::Oid {
        append_snapshot(repo, Some(parent), tree, sid, seq).expect("append_snapshot")
    }

    /// On-disk identity of `.git/index`: byte content + mtime
    /// (whole seconds + subsec nanos) + inode. AC1's three-part
    /// invariant. (inode is `MetadataExt` — unix-only, which covers
    /// both build targets, Linux + macOS.)
    #[cfg(unix)]
    #[derive(PartialEq, Debug)]
    struct IndexIdentity {
        bytes: Vec<u8>,
        mtime_secs: u64,
        mtime_nanos: u32,
        inode: u64,
    }

    #[cfg(unix)]
    fn index_identity(repo_dir: &Path) -> IndexIdentity {
        let index_path = repo_dir.join(".git").join("index");
        let meta = fs::metadata(&index_path).expect("index file should exist");
        let mtime = meta
            .modified()
            .expect("mtime")
            .duration_since(std::time::UNIX_EPOCH)
            .expect("mtime after epoch");
        use std::os::unix::fs::MetadataExt;
        IndexIdentity {
            bytes: fs::read(&index_path).expect("read index"),
            mtime_secs: mtime.as_secs(),
            mtime_nanos: mtime.subsec_nanos(),
            inode: meta.ino(),
        }
    }

    // -------------------------------------------------------------------
    // AC1: zero-touch invariants
    // -------------------------------------------------------------------

    /// Snapshotting must not touch the on-disk index file in any
    /// observable way: byte content, mtime (incl. subsec nanos) and
    /// inode must all survive unchanged. Also pins the behavioral
    /// half of AC1: `git status --porcelain`, `git log --oneline`
    /// and HEAD are identical before/after, and no visible ref is
    /// created (`git for-each-ref` unchanged).
    #[cfg(unix)]
    #[test]
    fn build_state_tree_zero_touch_index_and_refs() {
        let tmp = tempdir().unwrap();
        let project = tmp.path().join("project");
        init_repo(&project);
        fs::write(project.join("a.txt"), "v1\n").unwrap();
        fs::create_dir_all(project.join("src")).unwrap();
        fs::write(project.join("src/lib.rs"), "fn main() {}\n").unwrap();
        commit_all(&project);

        // `git status` may opportunistically refresh the index's
        // stat cache and rewrite the file — run it BEFORE reading
        // the identity we compare against, so the delta we measure
        // is strictly the libgit2 snapshot call's effect.
        let status_before = git_out(&project, &["status", "--porcelain"]);
        let log_before = git_out(&project, &["log", "--oneline"]);
        let head_before = git_out(&project, &["rev-parse", "HEAD"]);
        let refs_before = git_out(&project, &["for-each-ref"]);
        let identity_before = index_identity(&project);

        let repo = open(&project);
        let _ = build_state_tree(&repo).expect("snapshot should succeed");
        drop(repo);

        let identity_after = index_identity(&project);
        assert_eq!(
            identity_before, identity_after,
            "on-disk index must be byte/mtime/inode-identical across a snapshot"
        );

        // Behavioral visibility: unchanged too.
        assert_eq!(status_before, git_out(&project, &["status", "--porcelain"]));
        assert_eq!(log_before, git_out(&project, &["log", "--oneline"]));
        assert_eq!(head_before, git_out(&project, &["rev-parse", "HEAD"]));
        assert_eq!(refs_before, git_out(&project, &["for-each-ref"]));
    }

    /// A repo with no commits yet (unborn HEAD): the HEAD-tree
    /// rebase is skipped and the snapshot still captures the
    /// workdir (this is the fresh-project baseline case).
    #[test]
    fn build_state_tree_succeeds_without_head() {
        let tmp = tempdir().unwrap();
        let project = tmp.path().join("project");
        init_repo(&project);
        fs::write(project.join("early.txt"), "before first commit\n").unwrap();

        let repo = open(&project);
        let tree = build_state_tree(&repo).expect("snapshot on unborn HEAD");
        assert_eq!(tree_paths(&repo, tree), vec!["early.txt".to_string()]);
    }

    // -------------------------------------------------------------------
    // Content addressing / capture semantics
    // -------------------------------------------------------------------

    /// Content addressing dedupe: an unchanged workdir yields the
    /// same tree oid on repeated snapshots, and reverting the
    /// content returns to the original oid. Only a real content
    /// change produces a new tree.
    #[test]
    fn build_state_tree_dedupes_unchanged_state() {
        let tmp = tempdir().unwrap();
        let project = tmp.path().join("project");
        init_repo(&project);
        fs::write(project.join("a.txt"), "v1\n").unwrap();
        commit_all(&project);

        let repo = open(&project);
        let t1 = build_state_tree(&repo).unwrap();
        let t2 = build_state_tree(&repo).unwrap();
        assert_eq!(t1, t2, "unchanged state must dedupe to the same tree");

        fs::write(project.join("a.txt"), "v2\n").unwrap();
        let t3 = build_state_tree(&repo).unwrap();
        assert_ne!(t1, t3, "a content change must produce a new tree");

        fs::write(project.join("a.txt"), "v1\n").unwrap();
        let t4 = build_state_tree(&repo).unwrap();
        assert_eq!(t1, t4, "reverting the content returns to the same tree oid");
    }

    /// Untracked files — including inside never-seen nested
    /// directories — are captured into the snapshot (the AC2
    /// "bare git diff can't see new files, we can" property).
    #[test]
    fn build_state_tree_includes_untracked_files() {
        let tmp = tempdir().unwrap();
        let project = tmp.path().join("project");
        init_repo(&project);
        fs::write(project.join("tracked.txt"), "base\n").unwrap();
        commit_all(&project);

        fs::write(project.join("new.txt"), "brand new\n").unwrap();
        fs::create_dir_all(project.join("deep/nested")).unwrap();
        fs::write(project.join("deep/nested/file.txt"), "nested new\n").unwrap();

        let repo = open(&project);
        let tree = build_state_tree(&repo).unwrap();
        assert_eq!(
            tree_paths(&repo, tree),
            vec![
                "deep/nested/file.txt".to_string(),
                "new.txt".to_string(),
                "tracked.txt".to_string(),
            ]
        );
        assert_eq!(tree_blob(&repo, tree, "new.txt"), b"brand new\n");
    }

    /// `add_all` must stage deletions like `git add -A`: files
    /// removed from the workdir — including whole nested directory
    /// trees — disappear from the next snapshot tree (review
    /// addition: nested-dir deletion pinned explicitly).
    #[test]
    fn build_state_tree_propagates_deletions_including_nested_dirs() {
        let tmp = tempdir().unwrap();
        let project = tmp.path().join("project");
        init_repo(&project);
        fs::write(project.join("top.txt"), "top\n").unwrap();
        fs::create_dir_all(project.join("a/b")).unwrap();
        fs::write(project.join("a/b/c.txt"), "c\n").unwrap();
        fs::write(project.join("a/b/d.txt"), "d\n").unwrap();
        fs::write(project.join("stay.txt"), "stay\n").unwrap();
        commit_all(&project);

        let repo = open(&project);
        let t1 = build_state_tree(&repo).unwrap();
        assert_eq!(
            tree_paths(&repo, t1),
            vec![
                "a/b/c.txt".to_string(),
                "a/b/d.txt".to_string(),
                "stay.txt".to_string(),
                "top.txt".to_string(),
            ]
        );

        // Delete a top-level file AND the whole nested `a/` tree.
        fs::remove_file(project.join("top.txt")).unwrap();
        fs::remove_dir_all(project.join("a")).unwrap();

        let t2 = build_state_tree(&repo).unwrap();
        assert_eq!(
            tree_paths(&repo, t2),
            vec!["stay.txt".to_string()],
            "deletions (incl. nested dirs) must propagate into the snapshot"
        );
    }

    /// The gitignore boundary (review-approved wording): a file
    /// that is already tracked keeps flowing into the snapshot
    /// after a `.gitignore` entry matches it — modifications are
    /// captured and deletions still propagate. The complement
    /// (untracked-and-ignored is invisible) is pinned in the same
    /// test.
    #[test]
    fn build_state_tree_keeps_tracked_file_that_became_gitignored() {
        let tmp = tempdir().unwrap();
        let project = tmp.path().join("project");
        init_repo(&project);
        fs::write(project.join("secret.txt"), "v1\n").unwrap();
        commit_all(&project);

        // After the file is tracked, ignore it (plus a *.log rule
        // for the untracked complement) and mutate/delete it.
        fs::write(project.join(".gitignore"), "secret.txt\n*.log\n").unwrap();
        fs::write(project.join("secret.txt"), "v2\n").unwrap();
        fs::write(project.join("data.log"), "never captured\n").unwrap();

        let repo = open(&project);
        let tree = build_state_tree(&repo).unwrap();
        assert_eq!(
            tree_blob(&repo, tree, "secret.txt"),
            b"v2\n",
            "tracked file must stay in the snapshot despite gitignore"
        );
        assert!(
            !tree_paths(&repo, tree).contains(&"data.log".to_string()),
            "untracked-and-ignored file must not be captured"
        );

        // Deletion propagates too ("删除照传").
        fs::remove_file(project.join("secret.txt")).unwrap();
        let tree2 = build_state_tree(&repo).unwrap();
        assert!(
            !tree_paths(&repo, tree2).contains(&"secret.txt".to_string()),
            "deletion of a tracked-but-ignored file must propagate"
        );
    }

    /// An unresolved merge conflict (EUNMERGED entries in the
    /// index) must surface as `Err` — never a panic — so the PR1
    /// wiring can fail-open deterministically (warn log, no
    /// snapshot row for the conflicted turn).
    #[test]
    fn build_state_tree_errors_on_conflicted_index() {
        let tmp = tempdir().unwrap();
        let project = tmp.path().join("project");
        init_repo(&project);
        fs::write(project.join("a.txt"), "base\n").unwrap();
        commit_all(&project);

        // Two divergent edits of the same line → conflicting merge.
        git_out(&project, &["checkout", "-b", "side"]);
        fs::write(project.join("a.txt"), "side\n").unwrap();
        commit_all(&project);
        git_out(&project, &["checkout", "main"]);
        fs::write(project.join("a.txt"), "main\n").unwrap();
        commit_all(&project);
        let merge = StdCommand::new("git")
            .args(["merge", "side", "--no-edit"])
            .current_dir(&project)
            .output()
            .unwrap();
        assert!(
            !merge.status.success(),
            "setup: merge is expected to conflict"
        );

        let repo = open(&project);
        assert!(
            repo.index().unwrap().has_conflicts(),
            "setup: index should hold conflict entries"
        );
        let result = build_state_tree(&repo);
        match result {
            Err(GitError::Git2(e)) => {
                assert!(
                    e.message().contains("conflict"),
                    "error should describe the conflict state, got: {e}"
                );
            }
            other => panic!("expected Err(GitError::Git2), got: {other:?}"),
        }
    }

    // -------------------------------------------------------------------
    // Dangling commit chain + umbrella refs
    // -------------------------------------------------------------------

    /// The snapshot chain is built from commit objects that move no
    /// reference: HEAD and the visible ref listing are unchanged
    /// after appends, parents link the chain, and the diagnostic
    /// message carries session id + seq.
    #[test]
    fn append_snapshot_chain_is_dangling_and_linked() {
        let tmp = tempdir().unwrap();
        let project = tmp.path().join("project");
        init_repo(&project);
        fs::write(project.join("a.txt"), "v1\n").unwrap();
        commit_all(&project);

        let head_before = git_out(&project, &["rev-parse", "HEAD"]);
        let refs_before = git_out(&project, &["for-each-ref"]);

        let sid = "chain-test";
        let repo = open(&project);
        let (t0, c0) = snapshot(&repo, sid, 0);
        fs::write(project.join("a.txt"), "v2\n").unwrap();
        let t1 = build_state_tree(&repo).unwrap();
        let c1 = snapshot_on(&repo, c0, t1, sid, 1);

        // Chain shape: c0 is a root commit of the chain (no
        // parents), c1's parent is c0.
        let commit0 = repo.find_commit(c0).expect("chain root in odb");
        assert_eq!(commit0.parent_count(), 0);
        let commit1 = repo.find_commit(c1).expect("chain child in odb");
        assert_eq!(commit1.parent_count(), 1);
        assert_eq!(commit1.parent_id(0).unwrap(), c0);
        assert_eq!(commit1.tree_id(), t1);
        assert!(
            commit1.summary().unwrap().contains(sid)
                && commit1.summary().unwrap().contains("seq=1"),
            "diagnostic message should carry session/seq, got: {:?}",
            commit1.summary()
        );

        // Nothing visible moved. (Drop the commit handles first —
        // they borrow the repo.)
        drop(commit1);
        drop(commit0);
        drop(repo);
        assert_eq!(head_before, git_out(&project, &["rev-parse", "HEAD"]));
        assert_eq!(refs_before, git_out(&project, &["for-each-ref"]));
        let _ = (t0, t1);
    }

    /// Umbrella ref lifecycle: create → overwrite (chain advance)
    /// → delete → idempotent re-delete. The ref lives outside
    /// `refs/heads`, so branches / `git log <branch>` never see it.
    #[test]
    fn umbrella_ref_lifecycle_and_head_invisibility() {
        let tmp = tempdir().unwrap();
        let project = tmp.path().join("project");
        init_repo(&project);
        fs::write(project.join("a.txt"), "v1\n").unwrap();
        commit_all(&project);

        let sid = "018f3c2a-0000-7000-8000-000000000001";
        let repo = open(&project);
        let (_, c0) = snapshot(&repo, sid, 0);

        set_umbrella_ref(&repo, sid, c0).expect("set umbrella ref");
        let name = umbrella_ref_name(sid);
        let r = repo.find_reference(&name).expect("umbrella ref exists");
        assert_eq!(r.target().unwrap(), c0);

        // Chain advance overwrites the previous tip.
        fs::write(project.join("a.txt"), "v2\n").unwrap();
        let t1 = build_state_tree(&repo).unwrap();
        let c1 = snapshot_on(&repo, c0, t1, sid, 1);
        set_umbrella_ref(&repo, sid, c1).expect("update umbrella ref");
        let r = repo.find_reference(&name).unwrap();
        assert_eq!(r.target().unwrap(), c1, "tip must move with the chain");

        // Not a branch: invisible to the heads listing.
        let branches: Vec<String> = repo
            .branches(Some(git2::BranchType::Local))
            .unwrap()
            .filter_map(|b| b.ok())
            .filter_map(|(b, _)| b.name().ok().flatten().map(str::to_string))
            .collect();
        assert!(
            !branches.iter().any(|b| b.contains(sid)),
            "umbrella ref must not appear as a branch: {branches:?}"
        );

        // Delete → gone; deleting again is a no-op Ok.
        delete_umbrella_ref(&repo, sid).expect("delete umbrella ref");
        assert!(repo.find_reference(&name).is_err(), "ref must be gone");
        delete_umbrella_ref(&repo, sid).expect("second delete is idempotent");
    }

    /// Snapshots and umbrella refs taken through a *worktree*
    /// handle must land in the shared common-refs store: the main
    /// repository sees the ref (and the physical loose-ref file
    /// sits in the main `.git/refs/everlasting/`), while the
    /// snapshot reflects the worktree's own working directory.
    #[test]
    fn worktree_handle_snapshots_and_refs_land_in_common_store() {
        let tmp = tempdir().unwrap();
        let project = tmp.path().join("project");
        init_repo(&project);
        fs::write(project.join("base.txt"), "base\n").unwrap();
        commit_all(&project);

        // Session worktree off the project HEAD.
        let sid = "wt-shared-1";
        let wt_path = tmp.path().join("wt");
        crate::git::worktree::create(&project, &wt_path, sid).expect("create worktree");

        // Mutate the worktree (tracked edit + untracked new file).
        fs::write(wt_path.join("base.txt"), "worktree edit\n").unwrap();
        fs::write(wt_path.join("wt-only.txt"), "from worktree\n").unwrap();

        let wt_repo = open(&wt_path);
        let tree = build_state_tree(&wt_repo).expect("snapshot via worktree handle");
        assert_eq!(
            tree_paths(&wt_repo, tree),
            vec!["base.txt".to_string(), "wt-only.txt".to_string()],
            "snapshot must reflect the worktree's working directory"
        );
        let commit = append_snapshot(&wt_repo, None, tree, sid, 0).expect("append via worktree");
        set_umbrella_ref(&wt_repo, sid, commit).expect("umbrella ref via worktree handle");

        // The main repository sees the ref — the load-bearing
        // "shared common refs" invariant (PR1's delete fallback
        // relies on this physical location).
        let main = open(&project);
        let r = main
            .find_reference(&umbrella_ref_name(sid))
            .expect("umbrella ref visible from main repo");
        assert_eq!(r.target().unwrap(), commit);
        let loose = project
            .join(".git")
            .join("refs")
            .join("everlasting")
            .join(sid);
        assert!(
            loose.is_file(),
            "loose ref should physically live in the main repo: {}",
            loose.display()
        );
    }

    // -------------------------------------------------------------------
    // Turn diff + revert restore set
    // -------------------------------------------------------------------

    /// Snapshot-to-snapshot diff reuses the session diff shapes, in
    /// PRD AC2's literal scenario: 3 files modified + 1 file created
    /// across 2 turns → `diff ckpt(N) ckpt(N+2)` contains exactly 4
    /// files, with the new file visible (bare `git diff` can't see
    /// it). Per-file pins: a modified file reports 1+/1- (guards the
    /// tree-to-tree line_stats path against the workdir-diff
    /// undercount bug class), a created file reports added=N /
    /// removed=0 (the pure-insertion pattern), and the reversed
    /// direction inverts statuses.
    #[test]
    fn diff_snapshots_reports_changes_across_turns() {
        let tmp = tempdir().unwrap();
        let project = tmp.path().join("project");
        init_repo(&project);
        fs::write(project.join("a.txt"), "v1\n").unwrap();
        fs::write(project.join("b.txt"), "b\n").unwrap();
        fs::write(project.join("c.txt"), "c\n").unwrap();
        commit_all(&project);

        let sid = "diff-test";
        let repo = open(&project);
        let (t0, c0) = snapshot(&repo, sid, 0);

        // Turn 1: modify a.txt (canonical 1-for-1 replacement).
        fs::write(project.join("a.txt"), "v2\n").unwrap();
        let t1 = build_state_tree(&repo).unwrap();
        let c1 = snapshot_on(&repo, c0, t1, sid, 1);

        // Turn 2: create a new file + modify two tracked ones.
        fs::write(project.join("new.txt"), "alpha\nbeta\n").unwrap();
        fs::write(project.join("b.txt"), "b2\n").unwrap();
        fs::write(project.join("c.txt"), "c2\n").unwrap();
        let t2 = build_state_tree(&repo).unwrap();
        let _c2 = snapshot_on(&repo, c1, t2, sid, 2);

        // AC2 cross-turn diff: 3 modified + 1 new across two turns,
        // diffed from the baseline, contains exactly those 4 files.
        let across = diff_snapshots(&repo, t0, t2).expect("diff_snapshots");
        let mut paths: Vec<&str> = across.files.iter().map(|f| f.path.as_str()).collect();
        paths.sort();
        assert_eq!(paths, vec!["a.txt", "b.txt", "c.txt", "new.txt"]);

        let a = across
            .files
            .iter()
            .find(|f| f.path == "a.txt")
            .expect("a.txt in diff");
        assert_eq!(a.status, "modified");
        assert_eq!(a.added, 1, "canonical v1->v2 replacement: 1 added");
        assert_eq!(a.removed, 1, "canonical v1->v2 replacement: 1 removed");

        let new = across
            .files
            .iter()
            .find(|f| f.path == "new.txt")
            .expect("new file visible in snapshot diff");
        assert_eq!(new.status, "added");
        assert_eq!(new.added, 2);
        assert_eq!(new.removed, 0, "pure creation must not report removals");

        // Reversed diff flips the statuses.
        let rev = diff_snapshots(&repo, t2, t0).expect("reverse diff");
        let rev_new = rev
            .files
            .iter()
            .find(|f| f.path == "new.txt")
            .expect("new.txt in reverse diff");
        assert_eq!(rev_new.status, "deleted");
    }

    /// Revert semantics (AC3 core): the restore set is exactly the
    /// path set where now differs from the target; executing it
    /// restores modified files, resurrects deleted ones, removes
    /// files the target predates — and leaves everything else
    /// (unchanged files, untracked-and-ignored files, HEAD, index)
    /// untouched.
    #[test]
    fn compute_restore_set_and_restore_paths_revert_semantics() {
        let tmp = tempdir().unwrap();
        let project = tmp.path().join("project");
        init_repo(&project);
        // The *.log rule is part of the baseline world, so the
        // ignored artifact below is invisible to every snapshot.
        fs::write(project.join(".gitignore"), "*.log\n").unwrap();
        fs::write(project.join("modified.txt"), "v1\n").unwrap();
        fs::write(project.join("resurrect.txt"), "was here\n").unwrap();
        fs::write(project.join("untouched.txt"), "same\n").unwrap();
        fs::create_dir_all(project.join("nested")).unwrap();
        fs::write(project.join("nested/added.txt"), "late\n").unwrap();
        commit_all(&project);

        let sid = "revert-test";
        let repo = open(&project);
        #[cfg(unix)]
        let index_before = index_identity(&project);
        let head_before = git_out(&project, &["rev-parse", "HEAD"]);

        // Baseline snapshot (round 0).
        let (t0, _c0) = snapshot(&repo, sid, 0);

        // Round 1 work: modify two files, delete one, and create a
        // brand-new nested one. An untracked-and-ignored file also
        // appears (must survive the revert untouched — the
        // documented gitignore double-invisibility boundary).
        fs::write(project.join("modified.txt"), "v2\n").unwrap();
        fs::remove_file(project.join("resurrect.txt")).unwrap();
        fs::write(project.join("nested/added.txt"), "late\nmore\n").unwrap();
        fs::create_dir_all(project.join("newdir")).unwrap();
        fs::write(project.join("newdir/extra.txt"), "born this round\n").unwrap();
        fs::write(project.join("spill.log"), "ignored artifact\n").unwrap();
        let (t1, _c1) = snapshot(&repo, sid, 1);
        assert_ne!(t0, t1);

        // Restore set for going back to round 0.
        let set = compute_restore_set(&repo, t0).expect("compute_restore_set");
        let fmt: Vec<String> = set
            .iter()
            .map(|p| format!("{}:{:?}", p.path, p.action))
            .collect();
        assert_eq!(fmt.len(), 4, "exactly the divergent paths: {fmt:?}");
        let find = |path: &str, action: RestoreAction| {
            assert!(
                set.iter().any(|p| p.path == path && p.action == action),
                "expected {path} with {action:?} in [{fmt:?}]"
            );
        };
        find("modified.txt", RestoreAction::Checkout);
        find("resurrect.txt", RestoreAction::Checkout);
        find("nested/added.txt", RestoreAction::Checkout);
        find("newdir/extra.txt", RestoreAction::Delete);
        assert!(!set.iter().any(|p| p.path == "untouched.txt"));
        assert!(!set.iter().any(|p| p.path == "spill.log"));

        // Execute the revert.
        let outcome = restore_paths(&repo, t0, &set).expect("restore_paths");
        assert_eq!(
            RestoreOutcome {
                restored: 3,
                deleted: 1
            },
            outcome
        );

        // Content is back to the round-0 world.
        assert_eq!(
            fs::read_to_string(project.join("modified.txt")).unwrap(),
            "v1\n"
        );
        assert_eq!(
            fs::read_to_string(project.join("resurrect.txt")).unwrap(),
            "was here\n"
        );
        assert_eq!(
            fs::read_to_string(project.join("nested/added.txt")).unwrap(),
            "late\n"
        );
        assert!(!project.join("newdir/extra.txt").exists());
        assert!(!project.join("newdir").exists(), "empty parents are pruned");
        assert_eq!(
            fs::read_to_string(project.join("untouched.txt")).unwrap(),
            "same\n"
        );

        // The ignored artifact survives: not in any snapshot, so
        // never in a restore set, so never deleted by a revert.
        assert!(project.join("spill.log").exists());

        // Zero-touch for revert too: HEAD and the index file are
        // exactly where they were.
        drop(repo);
        assert_eq!(head_before, git_out(&project, &["rev-parse", "HEAD"]));
        #[cfg(unix)]
        assert_eq!(index_before, index_identity(&project));

        // Post-revert state snapshot dedupes back to the target
        // tree (the workdir now matches round 0).
        let repo = open(&project);
        let after = build_state_tree(&repo).unwrap();
        assert_eq!(after, t0, "post-revert state must equal the target tree");
    }

    /// Reverting to an already-matching state is an empty set and
    /// a no-op restore (idempotence for double-click protection).
    #[test]
    fn restore_set_is_empty_when_state_matches_target() {
        let tmp = tempdir().unwrap();
        let project = tmp.path().join("project");
        init_repo(&project);
        fs::write(project.join("a.txt"), "v1\n").unwrap();
        commit_all(&project);

        let repo = open(&project);
        let (t0, _c0) = snapshot(&repo, "noop-test", 0);
        let set = compute_restore_set(&repo, t0).expect("compute_restore_set");
        assert!(set.is_empty(), "matching state must yield an empty set");
        let outcome = restore_paths(&repo, t0, &set).expect("no-op restore");
        assert_eq!(outcome.restored, 0);
        assert_eq!(outcome.deleted, 0);
    }

    /// Checkout restores the executable bit when the snapshot blob
    /// was mode 100755 (unix-only; the bit is part of "content is
    /// back to round N").
    #[cfg(unix)]
    #[test]
    fn restore_paths_preserves_executable_bit() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempdir().unwrap();
        let project = tmp.path().join("project");
        init_repo(&project);
        fs::write(project.join("run.sh"), "#!/bin/sh\necho hi\n").unwrap();
        let perms = fs::Permissions::from_mode(0o755);
        fs::set_permissions(project.join("run.sh"), perms).unwrap();
        commit_all(&project);

        let repo = open(&project);
        let (t0, _c0) = snapshot(&repo, "exec-test", 0);

        // Strip the exec bit, then revert it back via the restore
        // set.
        fs::set_permissions(project.join("run.sh"), fs::Permissions::from_mode(0o644)).unwrap();
        let set = compute_restore_set(&repo, t0).expect("restore set");
        assert_eq!(set.len(), 1, "mode-only change is a diff delta");
        restore_paths(&repo, t0, &set).expect("restore");
        let mode = fs::metadata(project.join("run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "exec bit must be restored");
    }
}
