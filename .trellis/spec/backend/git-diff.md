# Git Diff — Workdir-vs-Branch-Tip FileDiff Contract

> Source-of-truth contract for the multi-file diff the agent UI renders
> on each session's worktree card. Captures the libgit2 footgun hit in
> step 4 follow-up Bug 2 (2026-06-08, task
> `06-08-fix-diff-replace-libgit2-line-stats-with-git-numstat-for-accurate-counts-in-edit-file-follow-up`).
>
> **Audience**: backend code touching `app/src-tauri/src/git/diff.rs`,
> or any future code that needs +/- line counts for a workdir-vs-base
> git diff. **Frontend** does not need to read this; it consumes the
> already-corrected `FileDiff.added` / `removed` fields.

---

## Status

Filled (2026-06-08). Authoritative source for +/- line counts is
`git diff --no-color --numstat`, NOT libgit2's `Patch::line_stats()`.

---

## Overview

The workdir-vs-branch-tip diff is computed by
`pub fn diff_worktree(worktree_path: &Path, session_id: &str) -> Result<DiffResult, GitError>`
in `app/src-tauri/src/git/diff.rs`. It returns a `DiffResult` with a
`Vec<FileDiff>`; each `FileDiff` has `added` / `removed` line counts
plus a unified `diff_text` body. The UI renders these directly on
each session's diff card.

Two upstream sources of line counts exist:
1. **libgit2** — `git2::Patch::line_stats()` returns `(usize, usize, usize)`
   for `(added, removed, _context)`.
2. **git CLI** — `git diff --no-color --numstat HEAD -- <path>` returns
   `<added>\t<removed>\t<path>` per line; binary files print `-` for
   both columns.

**libgit2's `line_stats()` is known to under-report additions** for
`diff_tree_to_workdir_with_index` outputs. The canonical failure case
is a one-line content change: `"v1\n"` → `"v2\n"` returns
`(added=0, removed=1)`, which makes the UI show a confusing red "-1"
next to no "+1" for what the user perceives as a pure addition. This
is a libgit2 upstream behaviour, not a bug we can patch locally.

---

## Scenario: Workdir-vs-Branch-Tip FileDiff

### 1. Scope / Trigger

Trigger any of:
- Adding / changing per-file `added` / `removed` line counts in
  `FileDiff` returned from `diff_worktree` or any sibling function.
- Switching the diff source library (libgit2 → git CLI, or any other).
- Re-deriving the diff from a different libgit2 API (e.g.
  `diff_tree_to_tree` instead of `diff_tree_to_workdir_with_index`).

### 2. Signatures

```rust
// app/src-tauri/src/git/diff.rs (existing — frozen public surface)
pub struct FileDiff {
    pub path: String,
    pub status: String,
    pub added: usize,    // line count, MUST come from `git --numstat`
    pub removed: usize,  // line count, MUST come from `git --numstat`
    pub diff_text: String,
}

pub struct DiffResult {
    pub files: Vec<FileDiff>,
}

pub fn diff_worktree(
    worktree_path: &Path,
    session_id: &str,
) -> Result<DiffResult, GitError>;

// New private helper (added 2026-06-08).
//
// Returns (added, removed) for a single path. The `worktree` arg is
// the path where git is invoked (current_dir). `path` is repo-relative.
//
// Errors: any subprocess failure (git missing, non-zero exit,
// StdCommand::output() failure) — caller falls back to libgit2
// `patch.line_stats()`.
fn git_numstat(
    worktree: &Path,
    path: &str,
) -> Result<(usize, usize), std::io::Error>;
```

### 3. Contracts

**Request to `git_numstat`**:
- argv: `["diff", "--no-color", "--numstat", "HEAD", "--", <path>]`
- cwd: `worktree` (so `<path>` is repo-relative to that worktree)
- timeout: none (no `Command::timeout` set — local repo, sub-second
  expected; revisit if a remote / huge worktree shows up)

**`HEAD` is explicit** so the result is `workdir + index` against the
session branch tip. With the step 4 no-staging model the index is
empty in practice, so this is equivalent to plain `git diff -- <path>`,
but the explicit `HEAD` is robust to a future Skill that adds staging.

**Response from `git_numstat`**:
- Ok((added, removed)) — both `usize`, parsed from first non-empty
  line of stdout
- Err(io::Error) — git missing, non-zero exit, spawn failure
- Empty stdout (file has no workdir-vs-HEAD diff) — Ok((0, 0))

**`FileDiff.added` / `FileDiff.removed` contract**:
- MUST equal `git --numstat` output for that path
- MUST fall back to `patch.line_stats()` on subprocess error
  (per `Validation & Error Matrix` below)
- For binary files: numstat prints `-` for both columns; parser
  coerces to 0 (UI cannot render a +/- count for binary anyway)
- For untracked files: do NOT route through `git_numstat`. The
  untracked path uses `build_untracked_diff` and counts lines of the
  on-disk file (untracked files are not in libgit2's delta list either,
  so there is no `Patch` to call `line_stats` on)

### 4. Validation & Error Matrix

| Condition | `git_numstat` result | `FileDiff` added/removed | Log level |
|---|---|---|---|
| Plain text, modified (canonical "v1\n"→"v2\n") | Ok((1, 1)) | (1, 1) | — |
| Plain text, pure insertion of N lines | Ok((N, 0)) | (N, 0) | — |
| Binary file | stdout: `-\t-\t<path>` → parser returns Ok((0, 0)) | (0, 0) | — |
| Untracked file | (NOT routed through `git_numstat`) | from `build_untracked_diff` line_count | — |
| Path with no workdir-vs-HEAD diff (shouldn't happen for files in libgit2's delta) | empty stdout → Ok((0, 0)) | (0, 0) | — |
| `git` binary missing from PATH | `StdCommand::output()?` Err | fallback: `patch.line_stats().unwrap_or((0, 0, 0))` | warn (logged once at call-site) |
| `git diff` non-zero exit (corrupt repo, permission, etc.) | Err(io::ErrorKind::Other, ...) | fallback as above | warn |
| Subprocess spawned but stdout malformed (non-utf8, non-numeric) | Ok with parse fallback `unwrap_or(0)` | `(0, 0)` — silent, file is in delta so empty count is suspicious | debug |

The fallback (`patch.line_stats().unwrap_or((0, 0, 0))`) is preserved
deliberately: it is the pre-fix behaviour and is strictly better than
returning an error to the user (the diff card still renders; +/- just
may be wrong). The pre-fix wrong count is the bug we are fixing; the
fallback is for catastrophic git-missing cases only.

### 5. Good / Base / Bad Cases

**Good** — the fix that ships in 2026-06-08:
```rust
let (a, d) = match git_numstat(worktree_path, &path) {
    Ok((a, d)) => (a, d),
    Err(e) => {
        tracing::warn!(
            worktree = %worktree_path.display(),
            path,
            err = %e,
            "diff_worktree: git --numstat failed, falling back to libgit2 line_stats"
        );
        let (la, ld, _) = patch.line_stats().unwrap_or((0, 0, 0));
        (la, ld)
    }
};
```

**Base** — the file is in libgit2's delta but git is missing; we degrade
silently and rely on libgit2's (still-buggy) count. UI shows +/- as
best-effort.

**Bad** — relying on libgit2 alone for the workdir-vs-base diff:
```rust
let (a, d, _) = patch.line_stats().unwrap_or((0, 0, 0));
// added=0, removed=1 for "v1\n"→"v2\n" — UI shows a red -1 with no +1
// for what the user perceives as a one-line replacement.
```

**Bad** — routing untracked files through `git_numstat`:
```rust
// for untracked files, `git diff --numstat` returns nothing (they are
// not in the index), so the parser returns Ok((0, 0)) and the diff
// card shows +0/-0 for a file the user just created. Use the
// `build_untracked_diff` line-count path for untracked entries.
```

### 6. Tests Required

The following are pinned in
`app/src-tauri/src/git/diff.rs::tests` (4 tests, all pass as of
2026-06-08):

| Test | What it pins |
|---|---|
| `diff_worktree_includes_untracked_files` | Bug 1 invariant: untracked files appear in `DiffResult.files` with `status = "untracked"`, `added = line_count`, `removed = 0` |
| `diff_worktree_ignores_gitignored_untracked` | Untracked files matching `.gitignore` are excluded from the diff |
| `diff_worktree_modified_tracked_file_unchanged` | After the numstat switch, a one-line `"v1\n"` → `"v2\n"` replacement reports `added = 1, removed = 1` (was: `added = 0, removed = 1` under libgit2). Also asserts the unified `diff_text` still contains both `-v1` and `+v2` |
| `diff_worktree_insert_lines_purely_added` | A pure append of 2 lines (`"line1\nline2\nline3\n"` → same file + `"line4\nline5\n"`) reports `added = 2, removed = 0`. This is the regression test for the "v1\n"→"v2\n" class of bugs under a different shape (append-only) |

Required test patterns for any future change in this file:
- A pure-replacement case asserting exact `added` / `removed`
- A pure-insertion case asserting `removed = 0`
- A binary-file case (write a `\0` byte to a path) asserting the diff
  does not panic and reports `added = 0, removed = 0` (or whatever
  shape the untracked fallback settles on for binary)
- A git-missing / numstat-failure case is hard to simulate without
  mocking; the `patch.line_stats()` fallback is covered by code
  review of the call site

### 7. Wrong vs Correct

#### Wrong

```rust
// Anti-pattern: trusting libgit2's line_stats for workdir diffs.
let (a, d, _) = patch.line_stats().unwrap_or((0, 0, 0));
// "v1\n" → "v2\n" reports (0, 1) instead of (1, 1). UI shows a
// red "-1" with no "+1" for a perceived one-line replacement.
```

#### Correct

```rust
// Route through git CLI; fall back to libgit2 only on subprocess
// failure (the fallback is for catastrophic git-missing cases).
let (a, d) = match git_numstat(worktree_path, &path) {
    Ok((a, d)) => (a, d),
    Err(e) => {
        tracing::warn!(...);
        let (la, ld, _) = patch.line_stats().unwrap_or((0, 0, 0));
        (la, ld)
    }
};
```

---

## Don't: Use `Patch::line_stats()` for +/- Counts in Workdir-vs-Base Diff

**Problem**:
```rust
let (a, d, _) = patch.line_stats().unwrap_or((0, 0, 0));
```

**Why it's bad**:
For `diff_tree_to_workdir_with_index`, libgit2's `line_stats()` is
known to under-report additions. Canonical case:
`"v1\n"` → `"v2\n"` returns `(0, 1, 0)`, which makes the UI show a
red `-1` next to no `+1` for what the user perceives as a one-line
replacement. This is an upstream libgit2 behaviour for this API; we
do not have a local patch.

**Instead**:
Call `git diff --no-color --numstat HEAD -- <path>` as a subprocess
and parse the output. Fall back to `patch.line_stats()` only on
subprocess failure (git missing, non-zero exit).

---

## Convention: `git --numstat` for Workdir-Vs-Base +/- Counts

**What**:
For each `FileDiff` produced by `diff_worktree`, the `added` and
`removed` line counts MUST come from
`git diff --no-color --numstat HEAD -- <path>`, parsed from the first
non-empty stdout line. On subprocess error, fall back to
`patch.line_stats().unwrap_or((0, 0, 0))`.

**Why**:
libgit2's `Patch::line_stats()` is unreliable for the
workdir-vs-base diff API we use. The git CLI's `--numstat` output is
the same source of truth the user would see in `git diff` /
`git difftool`, so the UI's `+N / -M` numbers match what the user
sees when they run `git` themselves. This eliminates a class of
"why does the UI show -1 when I only added a line?" reports.

**Example** (the helper):
```rust
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
```

**Related**: See `app/src-tauri/src/git/diff.rs` line 363-389 for the
authoritative source; the function signature and parsing are
considered frozen for the 2026-06 step 4 follow-up cycle. If a future
change is needed (e.g. to add a timeout, or to switch to a `git2`
rev-spec argument), update this spec in the same commit.

---

## Out of Scope

- Routing the unified `diff_text` (the `+` / `-` body the UI renders
  inside the diff popup) through git. The libgit2 unified diff text
  is correct and is what the UI currently consumes; switching it
  would change the visible diff body and is not part of this fix.
- Routing untracked-file line counts through `git --numstat`. Untracked
  files are not in libgit2's delta list (so there is no `Patch`), and
  `git diff --numstat` does not list untracked files either. The
  existing `build_untracked_diff` line-count path is correct and
  preserved.
- Replacing the `git` CLI subprocess with a pure-Rust implementation.
  `git2` is the natural candidate but its `line_stats` is the very
  bug we are routing around. Any other crate would be a new
  dependency; keep the subprocess for now.
- Making `git` a hard dependency. The fallback to `patch.line_stats()`
  means the app still functions (with wrong counts) if `git` is
  missing; this matches the pre-fix behaviour for git-missing cases.
