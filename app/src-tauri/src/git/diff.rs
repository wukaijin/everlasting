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
//!
//! Step 4 follow-up (Bug 1): the libgit2 workdir diff does NOT
//! include untracked files. The agent routinely creates new files
//! (e.g. `write_file` on a never-before-seen path); without
//! untracked coverage, the diff popup shows nothing for a session
//! that only added new files. We layer a `repo.statuses(...)` scan
//! on top and synthesize a `Delta::Added` entry per untracked file.

use std::path::Path;
use std::process::Command as StdCommand;

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
    //
    // **Known limitation** (see module docs and the step 4
    // follow-up Bug 1 PRD): this libgit2 API does NOT include
    // untracked files. The agent's `write_file` on a new path
    // creates an untracked file; the libgit2 diff returns no
    // delta for it, so the UI shows nothing. We patch this
    // below by scanning `repo.statuses(...)` for untracked
    // entries and synthesizing a `Delta::Added` row per file.
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
                // Bug 2 (step 4 follow-up): libgit2's `Patch::line_stats()`
                // is known to under-count / mis-report additions for
                // `diff_tree_to_workdir_with_index` — the canonical
                // `"v1\n" → "v2\n"` case returns `(0, 1, 0)` (no
                // addition, one deletion) even though the diff text
                // clearly shows both `-v1` and `+v2`. For an
                // `edit_file` "insert N lines" edit this manifests
                // as a spurious `-N` on the UI's diff card. We
                // delegate the +/- counts to `git diff --numstat`
                // (authoritative) and only fall back to
                // `patch.line_stats()` when the subprocess fails
                // (git missing, weird worktree state, etc).
                let (a, d) = match git_numstat(worktree_path, &path) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(
                            path = %path,
                            error = %e,
                            "diff_worktree: git --numstat failed, falling back to libgit2 line_stats"
                        );
                        let (a, d, _) = patch.line_stats().unwrap_or((0, 0, 0));
                        (a, d)
                    }
                };
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

    // Step 4 follow-up (Bug 1): layer in untracked files. The
    // libgit2 workdir diff above does not report them; without
    // this pass, the UI shows an empty diff for a session that
    // only added new files (a common case for the agent's
    // `write_file` on a fresh path).
    //
    // Strategy: ask libgit2 for `WT_UNTRACKED` status entries
    // (ignoring `.gitignore`d files via `include_ignored(false)`),
    // then for each untracked file:
    //   1. read its full contents,
    //   2. synthesize a "unified diff" body that is the whole
    //      file as added lines (prefixed with `+ `),
    //   3. report `status: "added"`, `added = line_count`,
    //      `removed = 0`.
    //
    // This is intentionally similar to how `git diff --no-index`
    // shows new files: the entire file body is the "+ " block.
    // The UI's diff popup already handles large "added" sections
    // (line numbers + scrolling), so the user can see what was
    // written. For very large untracked files (megabytes), the
    // full payload could get heavy; we cap at 64 KiB and append
    // a truncation marker (matches the spirit of `read_file`'s
    // 50 KiB head+tail truncation).
    const UNTRACKED_DIFF_CAP: usize = 64 * 1024;
    let mut status_opts = git2::StatusOptions::new();
    // We only need workdir-only statuses here. The libgit2 workdir
    // diff above already covers index entries (modified / added /
    // deleted in the index would be returned by `diff_tree_to_
    // workdir_with_index`); we just want WT_NEW (untracked) on
    // top. The default `StatusShow` is `IndexAndWorkdir`, which
    // is why we explicitly set `Workdir` here.
    status_opts
        .show(git2::StatusShow::Workdir)
        .include_ignored(false)
        .include_untracked(true)
        .recurse_untracked_dirs(false) // top-level only; untracked dirs surface via their files
        .include_unmodified(false);
    let statuses = repo.statuses(Some(&mut status_opts))?;
    let mut untracked_count: usize = 0;
    for entry in statuses.iter() {
        let raw_path = match entry.path() {
            Some(p) => p.to_string(),
            None => continue,
        };
        // We only want untracked files (`WT_NEW` in libgit2's
        // status bitmask) — skip modified/staged/etc. (those are
        // already in the libgit2 workdir diff above).
        let st = entry.status();
        let is_untracked = st.is_wt_new()
            && !st.is_wt_modified()
            && !st.is_wt_deleted();
        if !is_untracked {
            continue;
        }
        let abs_path = worktree_path.join(&raw_path);
        let content = match std::fs::read(&abs_path) {
            Ok(b) => b,
            Err(e) => {
                // Permissions / race-with-rm / etc. Log and skip
                // — surfacing as an error would break the whole
                // diff for a single bad file.
                tracing::warn!(
                    path = %raw_path,
                    error = %e,
                    "diff_worktree: could not read untracked file; skipping"
                );
                continue;
            }
        };
        let (text, line_count, truncated) = build_untracked_diff(&content, UNTRACKED_DIFF_CAP);
        if truncated {
            tracing::info!(
                path = %raw_path,
                total_bytes = content.len(),
                cap_bytes = UNTRACKED_DIFF_CAP,
                "diff_worktree: untracked file exceeded cap, truncating"
            );
        }
        files.push(FileDiff {
            path: raw_path,
            status: "added".to_string(),
            added: line_count,
            removed: 0,
            diff_text: text,
        });
        untracked_count += 1;
    }

    // Sort by path for stable UI rendering — the diff deltas are
    // not guaranteed to be in any particular order, and the
    // untracked pass above appended in statuses() order.
    files.sort_by(|a, b| a.path.cmp(&b.path));

    tracing::info!(
        worktree = %worktree_path.display(),
        file_count = files.len(),
        untracked_count,
        "diff_worktree complete"
    );

    Ok(DiffResult { files })
}

/// Build a synthetic "unified diff" for a newly-added (untracked)
/// file: the entire body as `+ `-prefixed lines, with a trailing
/// truncation marker if the payload was capped.
///
/// The format is intentionally non-standard — it's not a real
/// unified diff (there's no hunk header, no `@@ -0,0 +1,N @@`)
/// because the consumer is the UI's diff popup, not a
/// patch-application tool. The prefix `+ ` is enough for the UI
/// to render the file in green / added style.
fn build_untracked_diff(
    content: &[u8],
    cap: usize,
) -> (String, usize, bool) {
    if content.is_empty() {
        return (String::new(), 0, false);
    }
    let (slice, truncated) = if content.len() > cap {
        // Truncate at a UTF-8 char boundary to avoid
        // splitting a multi-byte sequence. We do this by
        // walking back from `cap` while the leading byte
        // is a UTF-8 continuation byte (b & 0xC0 == 0x80).
        let mut end = cap;
        while end > 0 && (content[end] & 0xC0) == 0x80 {
            end -= 1;
        }
        (&content[..end], true)
    } else {
        (content, false)
    };
    // Count newlines in the original (un-truncated) content so
    // the `added` line count reflects the file the agent
    // actually wrote, not the truncated preview. This matches
    // `git diff --numstat`'s behaviour for plain-text files
    // and is good enough for the +/- stat display in the UI.
    // Logic: count `\n` bytes; if the file doesn't end in `\n`,
    // the trailing partial line still counts as a line.
    let nl_count = content.iter().filter(|b| **b == b'\n').count();
    let line_count = if content.is_empty() {
        0
    } else if content.last() == Some(&b'\n') {
        nl_count
    } else {
        nl_count + 1
    };
    let mut text = String::with_capacity(slice.len() + 16);
    // Render as `+ ` per line. We tolerate non-UTF-8 by lossy
    // conversion — the original content might be binary, in
    // which case the UI should show the lossy render with the
    // replacement char rather than panic.
    let s = String::from_utf8_lossy(slice);
    for line in s.split_inclusive('\n') {
        text.push_str("+ ");
        text.push_str(line);
        if !line.ends_with('\n') {
            text.push('\n');
        }
    }
    if truncated {
        text.push_str(&format!(
            "\n... [truncated, total {} bytes > cap {} bytes]\n",
            content.len(),
            cap
        ));
    }
    (text, line_count, truncated)
}

/// Run `git diff --no-color --numstat HEAD -- <path>` in `worktree`
/// and parse the (added, removed) line counts for that path. This
/// is the authoritative source for the +/- stat on each diff card
/// in the UI; libgit2's `Patch::line_stats()` is known to
/// under-count additions for `diff_tree_to_workdir_with_index` (see
/// the call site for the canonical failure case).
///
/// `HEAD` is used explicitly so the result is the workdir + index
/// against the session branch tip (= the base commit the
/// `session/<id>` branch was created from), matching the libgit2
/// call we run side-by-side. With an empty index (the step 4
/// no-staging model) this is equivalent to plain `git diff -- <path>`,
/// but the explicit `HEAD` is robust to a future Skill that adds
/// staging.
///
/// Returns an error on subprocess failure (git missing, non-zero
/// exit, etc.) so the caller can fall back to `patch.line_stats()`.
/// A non-existent path (no diff) yields `(0, 0)` with `Ok`.
fn git_numstat(worktree: &Path, path: &str) -> Result<(usize, usize), std::io::Error> {
    let output = StdCommand::new("git")
        .args(["diff", "--no-color", "--numstat", "HEAD", "--", path])
        .current_dir(worktree)
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("git --numstat exited with {:?}", output.status),
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Each output line: "<added>\t<removed>\t<path>". For binary
    // files both counts are "-". We only ever query one path so
    // the first non-empty line (if any) is the answer; an empty
    // stdout means the file has no workdir-vs-HEAD diff (which
    // shouldn't happen for files in the libgit2 delta list, but
    // is a safe no-op if it does).
    for line in stdout.lines() {
        let mut cols = line.split('\t');
        let a_raw = cols.next().unwrap_or("0");
        let r_raw = cols.next().unwrap_or("0");
        let added = if a_raw == "-" { 0 } else { a_raw.parse::<usize>().unwrap_or(0) };
        let removed = if r_raw == "-" { 0 } else { r_raw.parse::<usize>().unwrap_or(0) };
        return Ok((added, removed));
    }
    Ok((0, 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Helper: init a git repo at `path`, configure the user, make
    /// an initial commit so `session/<id>` branches have a parent.
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

    /// Helper: create a session worktree at `wt_path` for
    /// `session_id` using libgit2 directly (so the test doesn't
    /// depend on `git::worktree::create` and can pre-populate the
    /// branch off of an initial commit). Returns the worktree
    /// path on success.
    fn make_worktree(project: &Path, session_id: &str, wt_path: &Path) {
        let repo = git2::Repository::open(project).unwrap();
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        let branch_full = format!("session/{}", session_id);
        let branch = repo.branch(&branch_full, &head, false).unwrap();
        let branch_ref = branch.into_reference();
        let mut opts = git2::WorktreeAddOptions::new();
        opts.reference(Some(&branch_ref));
        repo.worktree(session_id, wt_path, Some(&opts)).unwrap();
    }

    // -----------------------------------------------------------------------
    // Bug 1: untracked files must appear in the diff
    // -----------------------------------------------------------------------

    /// Step 4 follow-up (Bug 1): when the agent creates a new
    /// file with `write_file`, that file is untracked. libgit2's
    /// `diff_tree_to_workdir_with_index` does NOT include
    /// untracked files; the diff used to be empty in this case.
    /// This test pins the fix: untracked files appear in the
    /// diff with `status: "added"` and a `+ `-prefixed body.
    #[test]
    fn diff_worktree_includes_untracked_files() {
        let tmp = tempdir().unwrap();
        let project = tmp.path().join("project");
        init_repo(&project);
        fs::write(project.join("a.txt"), "hello\n").unwrap();
        commit_all(&project);

        let session_id = "untracked-1";
        let wt = tmp.path().join("wt");
        make_worktree(&project, session_id, &wt);

        // Write a new untracked file inside the worktree.
        fs::write(wt.join("new.txt"), "alpha\nbeta\ngamma\n").unwrap();

        let result = diff_worktree(&wt, session_id).expect("diff should succeed");
        // The untracked file MUST show up.
        let new_file = result
            .files
            .iter()
            .find(|f| f.path == "new.txt")
            .expect("new.txt should appear in diff");
        assert_eq!(new_file.status, "added");
        assert_eq!(new_file.added, 3, "three added lines");
        assert_eq!(new_file.removed, 0);
        // The body is the whole file with `+ ` prefix.
        assert!(new_file.diff_text.contains("+ alpha"));
        assert!(new_file.diff_text.contains("+ beta"));
        assert!(new_file.diff_text.contains("+ gamma"));
    }

    /// Step 4 follow-up (Bug 1): modified tracked files are
    /// still included (this is the pre-existing behavior; the
    /// untracked patch above must NOT regress it).
    ///
    /// Bug 2 (step 4 follow-up): after switching the +/- stat
    /// source from libgit2's `Patch::line_stats()` (known to
    /// under-count — returned `(0, 1, 0)` for the canonical
    /// "v1\n" → "v2\n" case) to `git diff --numstat`, the
    /// per-file counts must agree with git for a simple
    /// single-line replacement: 1 added, 1 removed.
    #[test]
    fn diff_worktree_modified_tracked_file_unchanged() {
        let tmp = tempdir().unwrap();
        let project = tmp.path().join("project");
        init_repo(&project);
        fs::write(project.join("a.txt"), "v1\n").unwrap();
        commit_all(&project);

        let session_id = "modified-1";
        let wt = tmp.path().join("wt");
        make_worktree(&project, session_id, &wt);

        // Modify the tracked file in the worktree.
        fs::write(wt.join("a.txt"), "v2\n").unwrap();

        let result = diff_worktree(&wt, session_id).expect("diff should succeed");
        let file = result
            .files
            .iter()
            .find(|f| f.path == "a.txt")
            .expect("a.txt should appear in diff");
        assert_eq!(file.status, "modified");
        // Bug 2 fix: +/- counts now come from `git diff --numstat`
        // and must match git's view of the diff (1 line in, 1
        // line out for a one-for-one replacement).
        assert_eq!(
            file.added, 1,
            "single-line replacement should report 1 added (got: added={}, removed={}, diff_text={:?})",
            file.added, file.removed, file.diff_text
        );
        assert_eq!(
            file.removed, 1,
            "single-line replacement should report 1 removed (got: added={}, removed={}, diff_text={:?})",
            file.added, file.removed, file.diff_text
        );
        // The unified diff text must show both the old and new
        // content. This is what the UI's diff popup renders.
        assert!(
            file.diff_text.contains("-v1"),
            "diff text should contain the old line: {:?}",
            file.diff_text
        );
        assert!(
            file.diff_text.contains("+v2"),
            "diff text should contain the new line: {:?}",
            file.diff_text
        );
    }

    /// Bug 2 regression test: a pure insertion (no replacement)
    /// of N lines via `edit_file` style in-place write must
    /// report `added=N, removed=0` on the diff card. Before the
    /// `git --numstat` switch, libgit2's `line_stats()` would
    /// report a spurious non-zero `removed` for some insertions,
    /// causing the UI to show a confusing red "-N" alongside
    /// the correct "+N".
    #[test]
    fn diff_worktree_insert_lines_purely_added() {
        let tmp = tempdir().unwrap();
        let project = tmp.path().join("project");
        init_repo(&project);
        fs::write(
            project.join("a.txt"),
            "line1\nline2\nline3\n",
        )
        .unwrap();
        commit_all(&project);

        let session_id = "insert-1";
        let wt = tmp.path().join("wt");
        make_worktree(&project, session_id, &wt);

        // In-place append at end of file (the canonical
        // "edit_file insert N lines" case): original 3 lines
        // + 2 new lines, nothing deleted.
        fs::write(
            wt.join("a.txt"),
            "line1\nline2\nline3\ninserted1\ninserted2\n",
        )
        .unwrap();

        let result = diff_worktree(&wt, session_id).expect("diff should succeed");
        let file = result
            .files
            .iter()
            .find(|f| f.path == "a.txt")
            .expect("a.txt should appear in diff");
        assert_eq!(file.status, "modified");
        assert_eq!(
            file.added, 2,
            "appending 2 new lines should report added=2 (got: added={}, removed={}, diff_text={:?})",
            file.added, file.removed, file.diff_text
        );
        assert_eq!(
            file.removed, 0,
            "pure insertion should report removed=0 (got: added={}, removed={}, diff_text={:?})",
            file.added, file.removed, file.diff_text
        );
        // The diff text body should still render both the
        // pre-existing context and the new lines (this part was
        // always correct via libgit2's Patch::to_buf, but pin
        // it to catch any future regression on the numstat
        // path).
        assert!(file.diff_text.contains("+inserted1"));
        assert!(file.diff_text.contains("+inserted2"));
    }

    /// Step 4 follow-up (Bug 1): untracked dirs and untracked
    /// `.gitignore`d files are NOT reported. We don't want to
    /// flood the diff with noise from the agent's spillover
    /// outputs or `.everlasting/` state.
    #[test]
    fn diff_worktree_ignores_gitignored_untracked() {
        let tmp = tempdir().unwrap();
        let project = tmp.path().join("project");
        init_repo(&project);
        fs::write(project.join(".gitignore"), "output/\n").unwrap();
        fs::write(project.join("a.txt"), "hello\n").unwrap();
        commit_all(&project);

        let session_id = "ignored-1";
        let wt = tmp.path().join("wt");
        make_worktree(&project, session_id, &wt);

        // Create an untracked-but-ignored file.
        fs::create_dir_all(wt.join("output")).unwrap();
        fs::write(wt.join("output/spill.txt"), "should be ignored").unwrap();

        // And a real untracked file that should appear.
        fs::write(wt.join("real.txt"), "yes\n").unwrap();

        let result = diff_worktree(&wt, session_id).expect("diff should succeed");
        let paths: Vec<&str> = result.files.iter().map(|f| f.path.as_str()).collect();
        assert!(
            paths.iter().any(|p| *p == "real.txt"),
            "real.txt should appear: {:?}",
            paths
        );
        assert!(
            !paths.iter().any(|p| p.contains("output/") || p.contains("spill.txt")),
            "gitignored output/ should not appear: {:?}",
            paths
        );
    }
}

