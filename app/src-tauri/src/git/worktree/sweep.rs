//! Worker worktree sweep: auto-commit, stale cleanup, period resolution.
//!
//! Relocated verbatim from the pre-split `worktree.rs`.

use std::path::Path;

use git2::Repository;

use super::lifecycle::destroy_worker;
use super::naming::worker_branch_name;
use crate::git::error::GitError;

/// Default cleanup period for worker worktrees (L3b PR3, 2026-06-27).
/// Matches Claude Code's `cleanupPeriodDays` default of 7 days.
/// Sweep keeps a worker worktree around for this many days
/// after its mtime; older ones are destroyed best-effort.
/// Commit all of a worker's changes (tracked modifications + untracked
/// files) onto its `worker/<run_id>` branch. Called by `run_subagent`
/// AFTER `probe_worker_changes` reports `has_changes=true` and BEFORE
/// the preserve-worktree decision, so the worker's branch tip truly
/// advances past the base — making a subsequent `merge_worker` (FF or
/// 3-way) actually carry the worker's edits.
///
/// Why this exists (the merge false-success gap): `probe_worker_changes`
/// diffs the **working tree** (detects uncommitted edits), but
/// `do_merge_blocking` merges **branch tips** (commits). Without this
/// auto-commit, a worker that never commits leaves `worker_tip ==
/// parent_tip`, and `merge_worker` hits the `is_ancestor` `==`
/// short-circuit (`tools/merge_worker.rs:651`) → returns "merged
/// fast-forward" with zero changes actually merged (silent
/// false-success). This helper closes that gap by always committing
/// the worker's working-tree changes before the branch is preserved.
///
/// Stages everything (`add_all(["*"])`, mirroring a human `git add -A`),
/// then commits on `refs/heads/worker/<run_id>` with the Everlasting
/// signature. Returns the new commit OID. Best-effort at the call site:
/// a failure is logged and the worktree preserved anyway; the merge
/// then degrades to the legacy behavior.
pub fn commit_worker_changes(worker_wt: &Path, run_id: &str) -> Result<git2::Oid, GitError> {
    let repo = Repository::open(worker_wt)?;
    let branch_ref = format!("refs/heads/{}", worker_branch_name(run_id));

    // Stage all changes — tracked modifications + untracked files.
    let mut index = repo.index()?;
    index.add_all(["*"], git2::IndexAddOption::DEFAULT, None)?;
    index.write()?;

    // Write the staged tree and commit on top of the current tip.
    let tree_oid = index.write_tree()?;
    let tree = repo.find_tree(tree_oid)?;
    let head_commit = repo.head()?.peel_to_commit()?;
    let sig = git2::Signature::now("Everlasting", "agent@everlasting")?;
    let oid = repo.commit(
        Some(&branch_ref),
        &sig,
        &sig,
        &format!("worker {}: auto-commit worker changes", run_id),
        &tree,
        &[&head_commit],
    )?;
    Ok(oid)
}

pub const DEFAULT_CLEANUP_PERIOD_DAYS: u32 = 7;

/// Environment override for [`DEFAULT_CLEANUP_PERIOD_DAYS`].
/// Read by [`sweep_stale_worker_worktrees`] when the caller
/// doesn't pass an explicit `cleanup_period_days` value.
pub const CLEANUP_PERIOD_DAYS_ENV: &str = "EVERLASTING_CLEANUP_PERIOD_DAYS";

/// Sweep stale worker worktrees for a project. Called once
/// at startup (see `AppState::load` integration) and
/// discoverable as a stand-alone helper for any future
/// on-demand sweep (e.g. a "clean up" tool).
///
/// L3b PR3 (2026-06-27): the function walks the project's
/// worker worktree directory
/// (`<app_data_dir>/worktrees/<project_uuid>/worker/`) and,
/// for each subdirectory, checks:
/// 1. **Lock presence** — the worktree is locked iff
///    `<project_path>/.git/worktrees/<name>/locked` exists
///    (libgit2's lock file is the canonical "this worktree is
///    in active use" marker). Locked worktrees are SKIPPED
///    (the `create_worker` lock mechanism protects running
///    workers; a sweep must not destroy a worker that's
///    still running).
/// 2. **Mtime** — the worktree directory's mtime (the
///    `Metadata::modified()` timestamp). If the mtime is
///    older than `cleanup_period_days` days (computed as
///    `now - cleanup_period_days * 86400` seconds), the
///    worktree is destroyed via
///    [`destroy_worker`], which also unlocks + deletes the
///    `worker/<run_id>` branch + removes the libgit2
///    worktree metadata.
///
/// Returns the number of worktrees destroyed (0 in the
/// common case). The caller logs the count for observability.
///
/// **Best-effort** semantics (per PRD §"Edge Cases"):
/// - A single failure (lock check error / mtime read error
///   / `destroy_worker` error) is logged at `warn!` and the
///   sweep continues with the next worktree. A failure on
///   one worktree does NOT abort the sweep — that would
///   leave other stale worktrees in place for a future
///   sweep that's not guaranteed to happen.
/// - A worktree that is `libgit2::find_worktree`-
///   unrecognizable (the on-disk dir is there but the
///   libgit2 metadata is gone — a crashed create + a manual
///   `git worktree prune`) is still best-effort destroyed
///   (we pass the path to `destroy_worker` which tolerates
///   "metadata already gone" via its own best-effort prune
///   + branch-delete).
/// - The sweep is a no-op when the worker dir doesn't exist
///   (fresh project, no workers have ever been spawned).
///
/// **Per-worker lock file detection** is the key correctness
/// invariant: a worker running RIGHT NOW (a 3-hour
/// `cargo build` etc.) has its worktree locked by
/// `create_worker` (see `create_worktree_add` →
/// `Worktree::lock`). The sweep MUST respect this — without
/// the lock check, a sweep during a long worker run would
/// silently destroy the in-flight worktree mid-execution
/// (the worker would write to a dir that no longer exists
/// and the parent would `prune` it on the next `git gc`).
pub fn sweep_stale_worker_worktrees(
    app_data_dir: &Path,
    project_uuid: &str,
    project_path: &Path,
    cleanup_period_days: u32,
) -> Result<usize, GitError> {
    let worker_root = app_data_dir
        .join("worktrees")
        .join(project_uuid)
        .join("worker");
    if !worker_root.exists() {
        // No worker dir → nothing to sweep. This is the
        // common case for fresh projects.
        return Ok(0);
    }

    // Open the project repo once. If it fails (the project
    // worktree dir itself was removed out from under us), we
    // log + return 0 — the sweep can't proceed without
    // libgit2 access.
    let repo = match Repository::open(project_path) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(
                project = %project_path.display(),
                error = %e,
                "sweep: could not open project repo; skipping sweep"
            );
            return Ok(0);
        }
    };

    let cutoff_secs = (cleanup_period_days as i64) * 86_400;
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let entries = match std::fs::read_dir(&worker_root) {
        Ok(rd) => rd,
        Err(e) => {
            tracing::warn!(
                worker_root = %worker_root.display(),
                error = %e,
                "sweep: could not read worker dir; skipping sweep"
            );
            return Ok(0);
        }
    };

    let mut destroyed_count = 0usize;
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, "sweep: read_dir entry error (non-fatal)");
                continue;
            }
        };
        let wt_path = entry.path();
        if !wt_path.is_dir() {
            continue;
        }
        // The worktree's libgit2 metadata name is the run_id
        // (no `worker/` prefix — see `create_worker`). The
        // on-disk dir layout is `<worker_root>/<run_id>`.
        let run_id = match wt_path.file_name().and_then(|n| n.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };

        // ----- Lock check (via libgit2 API) -----
        // `repo.find_worktree(metadata_name)` returns the
        // worktree handle; `.is_locked()` consults libgit2's
        // authoritative lock state (the canonical
        // `<project>/.git/worktrees/<name>/locked` file,
        // plus the in-memory lock marker). Using the API
        // rather than `Path::exists` keeps us robust to
        // future libgit2 changes (e.g. an in-process lock
        // mode that doesn't write the file).
        let locked = match repo.find_worktree(&run_id) {
            Ok(wt) => matches!(wt.is_locked(), Ok(git2::WorktreeLockStatus::Locked(_))),
            Err(_) => {
                // Worktree metadata not found (the on-disk
                // dir exists but libgit2 doesn't know about
                // it — a crashed create + a manual `git
                // worktree prune`). The destroy_worker call
                // is best-effort and tolerates this, so we
                // proceed (treating the worktree as
                // unlocked).
                false
            }
        };
        if locked {
            tracing::info!(
                run_id = %run_id,
                "sweep: skipping locked worker worktree (active worker)"
            );
            continue;
        }

        // ----- Mtime check -----
        let mtime = match std::fs::metadata(&wt_path).and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(
                    run_id = %run_id,
                    worktree = %wt_path.display(),
                    error = %e,
                    "sweep: could not stat worktree mtime (non-fatal; skipping)"
                );
                continue;
            }
        };
        let mtime_secs = mtime
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let age_secs = now_secs.saturating_sub(mtime_secs);
        if age_secs < cutoff_secs {
            // Not stale yet — skip.
            continue;
        }

        // ----- Destroy -----
        tracing::info!(
            run_id = %run_id,
            worktree = %wt_path.display(),
            age_days = age_secs / 86_400,
            "sweep: destroying stale worker worktree"
        );
        if let Err(e) = destroy_worker(project_path, &wt_path, &run_id) {
            tracing::warn!(
                run_id = %run_id,
                worktree = %wt_path.display(),
                error = %e,
                "sweep: destroy_worker failed (non-fatal; continuing)"
            );
            continue;
        }
        destroyed_count += 1;
        // Reference the repo to silence the "unused" warning
        // when the libgit2 worktree check below is skipped
        // (we use the project_path, not repo, to keep the
        // implementation simple — libgit2 worktrees are
        // discoverable via the on-disk layout, and
        // `destroy_worker` opens the repo itself).
        let _ = &repo;
    }

    Ok(destroyed_count)
}

/// Resolve the cleanup-period-days value: prefer the explicit
/// `cleanup_period_days` parameter, fall back to the
/// `EVERLASTING_CLEANUP_PERIOD_DAYS` env var, fall back to
/// [`DEFAULT_CLEANUP_PERIOD_DAYS`] (7). Returns the resolved
/// value. Used by [`sweep_stale_worker_worktrees`] callers
/// that want the env-aware default.
pub fn resolve_cleanup_period_days(explicit: Option<u32>) -> u32 {
    if let Some(d) = explicit {
        return d;
    }
    if let Ok(s) = std::env::var(CLEANUP_PERIOD_DAYS_ENV) {
        if let Ok(d) = s.parse::<u32>() {
            if d > 0 {
                return d;
            }
        }
    }
    DEFAULT_CLEANUP_PERIOD_DAYS
}
