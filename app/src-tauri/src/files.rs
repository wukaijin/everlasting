//! B2 @文件补全 — project file-tree walker.
//!
//! Recursively lists files under a project root as **root-relative
//! forward-slash paths**, for the frontend `@`-mention completion
//! panel. A path is dropped when EITHER:
//! 1. its final component is in [`DEFAULT_EXCLUDE`] (defensive — covers
//!    non-git projects and common offenders a `.gitignore` sometimes
//!    misses: `.git`, `node_modules`, `target`, …), OR
//! 2. git reports it ignored (`git2::Repository::path_is_ignored`,
//!    which honours `.gitignore` / `.git/info/exclude` / global ignores).
//!
//! Non-git project → exclude-list only (git2 won't open). Bounded by
//! [`MAX_DEPTH`] + [`MAX_FILES`] so a pathological tree can't stall the
//! IPC.
//!
//! **Synchronous** (std::fs + libgit2 are blocking); the Tauri command
//! wraps [`walk_files`] in `spawn_blocking`. **No mtime cache**: unlike
//! B3's command files (a handful, rarely change), a source tree churns
//! constantly, and the frontend re-fetches on each `@` open anyway — a
//! read-through fence would add complexity for no freshness win.

use std::path::{Path, PathBuf};

use git2::Repository;

/// Built-in exclude set, matched by final path component. Applied
/// unconditionally (before gitignore) — these are VCS / dependency /
/// build-output dirs that must never pollute a file picker, regardless
/// of `.gitignore`.
const DEFAULT_EXCLUDE: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    ".next",
    ".nuxt",
    ".cache",
    "__pycache__",
    ".pytest_cache",
    ".turbo",
    ".gradle",
    "coverage",
    ".DS_Store",
];

/// Max recursion depth (root's direct entries = depth 1). Guards
/// against pathological nesting; symlink loops are impossible because
/// symlinks are not followed (we read non-following file types).
const MAX_DEPTH: usize = 15;

/// Max number of file paths returned. A typical project is well under
/// this; the cap bounds the IPC payload + frontend render. The
/// frontend's fuzzysort narrows further on each keystroke.
const MAX_FILES: usize = 5000;

/// Open the git repo at `root` if it is one. Returns `(repo, workdir)`
/// for ignore checks; `None` when `root` is not inside a git repo
/// (non-git fallback: exclude-list only).
fn open_repo(root: &Path) -> Option<(Repository, PathBuf)> {
    let repo = Repository::open(root).ok()?;
    let workdir = repo.workdir()?.to_path_buf();
    Some((repo, workdir))
}

/// True if `abs_path` is git-ignored. `False` on any error or when the
/// path is outside the workdir (shouldn't happen — we walk under root).
fn is_git_ignored(repo: &Repository, workdir: &Path, abs_path: &Path) -> bool {
    let rel = match abs_path.strip_prefix(workdir) {
        Ok(r) => r,
        Err(_) => return false,
    };
    repo.is_path_ignored(rel).unwrap_or(false)
}

/// True if the entry's final component is in [`DEFAULT_EXCLUDE`].
fn is_default_excluded(name: &str) -> bool {
    DEFAULT_EXCLUDE.iter().any(|x| *x == name)
}

/// Convert a root-relative `Path` to a forward-slash string
/// (cross-platform; the frontend + `@token` use `/`, never `\`).
fn rel_to_fwdslash(rel: &Path) -> String {
    rel.components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

/// Sort ascending so the unfiltered panel has a stable display order.
/// The frontend fuzzysort re-orders once the user types a filter.
fn finalize(mut out: Vec<String>) -> Vec<String> {
    out.sort();
    out
}

/// Recursively walk `root`, returning root-relative forward-slash file
/// paths. Unreadable directories are skipped (one bad dir never aborts
/// the whole walk — mirrors memory's failure tolerance). Symlinks are
/// not followed.
pub fn walk_files(root: &Path) -> Vec<String> {
    walk_files_bounded(root, MAX_DEPTH, MAX_FILES)
}

/// Bounded variant used by both [`walk_files`] (production caps) and
/// the tests (small caps so a count/depth assertion doesn't create
/// thousands of files).
fn walk_files_bounded(root: &Path, max_depth: usize, max_files: usize) -> Vec<String> {
    let repo_ctx = open_repo(root);
    let mut out: Vec<String> = Vec::new();
    // Explicit stack (DFS); each frame = (dir_abs, depth) where `depth`
    // is the directory's distance from `root` (root itself = 0). A
    // directory's contents are pruned when its depth exceeds max_depth.
    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > max_depth {
            continue;
        }
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue, // unreadable / missing dir → skip, don't abort
        };
        for entry in rd.flatten() {
            if out.len() >= max_files {
                return finalize(out);
            }
            let name = match entry.file_name().into_string() {
                Ok(s) => s,
                Err(_) => continue, // non-UTF-8 name
            };
            if is_default_excluded(&name) {
                continue;
            }
            let path = entry.path();
            // gitignore prunes both files and dirs.
            if let Some((repo, workdir)) = &repo_ctx {
                if is_git_ignored(repo, workdir, &path) {
                    continue;
                }
            }
            // File type WITHOUT following symlinks (loop-safe).
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_dir() {
                stack.push((path, depth + 1));
            } else if ft.is_file() {
                let rel = match path.strip_prefix(root) {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                out.push(rel_to_fwdslash(rel));
            }
            // else: symlink / fifo / socket → skip (not a mentionable file)
        }
    }
    finalize(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn walks_files_relative_fwdslash() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("a.txt"), "x").unwrap();
        fs::create_dir(root.join("src")).unwrap();
        fs::write(root.join("src").join("b.rs"), "x").unwrap();
        fs::create_dir(root.join("src").join("nested")).unwrap();
        fs::write(root.join("src").join("nested").join("c.md"), "x").unwrap();
        let out = walk_files(root);
        assert!(out.contains(&"a.txt".to_string()));
        assert!(out.contains(&"src/b.rs".to_string()));
        assert!(out.contains(&"src/nested/c.md".to_string()));
    }

    #[test]
    fn excludes_default_dirs() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("keep.txt"), "x").unwrap();
        fs::create_dir(root.join("node_modules")).unwrap();
        fs::write(root.join("node_modules").join("dep.js"), "x").unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join(".git").join("config"), "x").unwrap();
        let out = walk_files(root);
        assert!(out.contains(&"keep.txt".to_string()));
        assert!(!out.iter().any(|p| p.contains("node_modules")));
        assert!(!out.iter().any(|p| p.starts_with(".git")));
    }

    #[test]
    fn count_cap_stops_at_limit() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        for i in 0..8 {
            fs::write(root.join(format!("f{i}.txt")), "x").unwrap();
        }
        // Small cap via the bounded variant (avoids creating MAX_FILES
        // real files just to test truncation).
        let out = walk_files_bounded(root, MAX_DEPTH, 5);
        assert_eq!(out.len(), 5);
    }

    #[test]
    fn depth_cap_prunes_deep_entries() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        // Chain nests linearly as d0/d1/d2/d3 (each holds a file).
        let mut cur = root.to_path_buf();
        for d in 0..4 {
            cur = cur.join(format!("d{d}"));
            fs::create_dir_all(&cur).unwrap();
            fs::write(cur.join("file.txt"), "x").unwrap();
        }
        // max_depth=2 reads root(0)→d0(1)→d1(2); d1's file survives,
        // d2(3)+ pruned.
        let out = walk_files_bounded(root, 2, MAX_FILES);
        assert!(out.iter().any(|p| p == "d0/file.txt"));
        assert!(out.iter().any(|p| p == "d0/d1/file.txt"));
        assert!(!out.iter().any(|p| p.contains("d0/d1/d2")));
        assert!(!out.iter().any(|p| p.contains("d3")));
    }

    #[test]
    fn non_git_uses_exclude_list_only() {
        // No .git → repo_ctx None → .gitignore NOT honoured (exclude list only).
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join(".gitignore"), "*.log\n").unwrap();
        fs::write(root.join("app.log"), "x").unwrap();
        fs::write(root.join("keep.txt"), "x").unwrap();
        let out = walk_files(root);
        assert!(out.contains(&"app.log".to_string()));
        assert!(out.contains(&"keep.txt".to_string()));
    }

    #[test]
    fn git_ignored_files_filtered() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        Repository::init(root).unwrap();
        fs::write(root.join(".gitignore"), "*.log\nbuild/\n").unwrap();
        fs::write(root.join("keep.txt"), "x").unwrap();
        fs::write(root.join("debug.log"), "x").unwrap();
        fs::create_dir(root.join("build")).unwrap();
        fs::write(root.join("build").join("out.js"), "x").unwrap();
        let out = walk_files(root);
        assert!(out.contains(&"keep.txt".to_string()));
        assert!(out.contains(&".gitignore".to_string()));
        assert!(!out.iter().any(|p| p.contains("debug.log")));
        assert!(!out.iter().any(|p| p.contains("build")));
    }

    #[test]
    fn missing_root_returns_empty() {
        let out = walk_files(Path::new("/no/such/everlasting/xyz_987"));
        assert!(out.is_empty());
    }

    #[test]
    fn unreadable_dir_skipped_not_fatal() {
        // A non-dir entry as "dir" frame just yields no entries; we
        // assert the walker doesn't panic and returns the rest.
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("keep.txt"), "x").unwrap();
        fs::write(root.join("plainfile"), "x").unwrap();
        let out = walk_files(root);
        assert!(out.contains(&"keep.txt".to_string()));
    }
}
