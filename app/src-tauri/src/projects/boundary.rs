//! Project CWD boundary assertion.
//!
//! See `.trellis/spec/backend/project-cwd-boundary.md` for the full contract.
//! Every read/write/shell tool call from the LLM goes through
//! [`assert_within_root`] to guarantee the target is physically located
//! inside the active project's root.
//!
//! Hard rules:
//! 1. Use `canonicalize` (physical path, resolves symlinks) — not logical
//!    path comparison.
//! 2. Use `Path::starts_with` (component-wise) — not string prefix
//!    matching, to defeat the `/repo/foobar` vs `/repo/foo` prefix trap.
//! 3. Nonexistent target → reject (cannot prove it is inside the root).
//! 4. Broken symlink → reject (canonicalize fails on it).

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

/// Assert that `target` is physically located inside `root`. On success
/// returns the canonical absolute path of `target`; on failure returns an
/// error whose message identifies both paths and the cause.
///
/// The implementation follows `.trellis/spec/backend/project-cwd-boundary.md`
/// §3 exactly:
///   1. `canonicalize` both paths to resolve symlinks.
///   2. Use `Path::starts_with` (component-wise) for the containment check.
pub fn assert_within_root(root: &Path, target: &Path) -> Result<PathBuf> {
    let target_real = target.canonicalize().map_err(|e| {
        anyhow!(
            "path '{}' cannot be resolved: {} (does not exist or is a broken symlink)",
            target.display(),
            e
        )
    })?;
    let root_real = root.canonicalize().map_err(|e| {
        anyhow!(
            "project root '{}' cannot be resolved: {}",
            root.display(),
            e
        )
    })?;

    if target_real == root_real || target_real.starts_with(&root_real) {
        Ok(target_real)
    } else {
        Err(anyhow!(
            "path '{}' is outside project root '{}'",
            target_real.display(),
            root_real.display()
        ))
    }
}

/// Non-failing boolean variant of [`assert_within_root`].
///
/// **Purpose**: ⑨ 关 Tier 4 path-based permission layer (A2+B7 re-grill,
/// 2026-06-13) needs to ask "is `target` inside `root`?" WITHOUT
/// committing to the same error contract as the tool layer's pre-call
/// boundary check. Specifically:
///
/// - Permission layer must **tolerate non-existent targets** (the
///   path might be a write target that doesn't exist yet, or a
///   path-glob that no longer exists). The tool layer's
///   `assert_within_root` rejects non-existent paths because it
///   can't prove containment; the permission layer accepts them
///   conservatively (treats them as inside the project if their
///   lexical parent is inside the project).
/// - Permission layer uses **lexical starts_with** (no
///   canonicalize), so symlinks are NOT resolved. This is a
///   conscious trade-off — a path the user typed as
///   `~/Documents/notes.md` should be classified as
///   "outside the project" if `~` is outside, regardless of
///   any symlinks the user has. The tool layer's later
///   `assert_within_root` re-validates with canonicalize
///   before any disk write.
///
/// **Returns** `true` if `target` is the same as `root` or a
/// (lexical) descendant. `false` if `target` is not under `root`
/// (or is empty, or has no parent that is under `root`).
///
/// **Followup hook** (re-grill PRD §"Out of Scope"): a future
/// PR may add a `web_fetch` URL-based path check that uses the
/// URL host instead of a filesystem path. Today only filesystem
/// paths are evaluated.
pub fn is_within_root(root: &Path, target: &Path) -> bool {
    // Strip `..` / `.` components lexically so a path like
    // `root/../sibling/file` reduces to `sibling/file` and is
    // correctly rejected as outside root. We do NOT call
    // `canonicalize` (that would resolve symlinks — see the
    // doc above). `Path::components` lets us iterate without
    // allocating intermediate strings.
    let target_clean = lexical_normalize(target);
    let root_clean = lexical_normalize(root);

    // Same-or-descendant check is sufficient for the permission
    // layer's purpose: "is this in the project?" The path is
    // either equal to root, or its first N components match root's
    // first N components (component-wise; this is what
    // `Path::starts_with` does — it does NOT do a string prefix
    // match, so the `/repo/foobar` vs `/repo/foo` prefix trap is
    // handled correctly).
    if target_clean == root_clean {
        return true;
    }
    if target_clean.starts_with(&root_clean) {
        return true;
    }
    // Defensive: if `target` is a relative path or has a parent
    // that's inside root, accept it (used for not-yet-existing
    // write targets where `target` itself doesn't exist but its
    // parent dir does). The tool layer's `assert_within_root`
    // is still the source of truth for the actual write. We
    // walk up `target_clean`'s parents and check each one
    // against root.
    let mut cur = target_clean.parent();
    while let Some(c) = cur {
        if c.as_os_str().is_empty() {
            break;
        }
        if c == root_clean || c.starts_with(&root_clean) {
            return true;
        }
        cur = c.parent();
    }
    false
}

/// Lexical normalization that strips `.` and `..` components
/// from a path **without** touching the filesystem. This is the
/// "syntactic" sibling of `Path::canonicalize` — the OS doesn't
/// see it, symlinks aren't resolved, and a non-existent target
/// is still accepted (its components are walked lexically).
///
/// Rules:
/// - `.` components are dropped.
/// - `..` components pop the previous component (if any); if
///   there's no previous component, the `..` is dropped too
///   (i.e. `..` at the start of a relative path is a no-op,
///   same as the shell's behavior).
/// - Everything else is kept verbatim.
fn lexical_normalize(p: &Path) -> std::borrow::Cow<'_, Path> {
    use std::borrow::Cow;
    use std::path::Component;

    let mut stack: Vec<&std::ffi::OsStr> = Vec::new();
    let mut changed = false;
    for comp in p.components() {
        match comp {
            Component::CurDir => {
                changed = true;
                // drop
            }
            Component::ParentDir => {
                changed = true;
                if let Some(top) = stack.last() {
                    if *top != std::ffi::OsStr::new("..") {
                        // Pop the previous component (handles
                        // /foo/.. -> /, /foo/bar/.. -> /foo).
                        stack.pop();
                    } else {
                        // Previous is itself `..` — keep stacking
                        // (e.g. ../../foo).
                        stack.push(comp.as_os_str());
                    }
                }
                // else: no previous, drop the ..
            }
            Component::Normal(_) | Component::RootDir | Component::Prefix(_) => {
                stack.push(comp.as_os_str());
            }
        }
    }
    if !changed {
        return Cow::Borrowed(p);
    }
    // Reassemble from the stack.
    let mut out = std::path::PathBuf::new();
    for s in &stack {
        out.push(s);
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Helper: create a directory layout under `root`:
    ///   root/
    ///     file
    ///     subdir/
    ///       nested
    ///     foobar       (for prefix-trap test)
    ///     sibling/     (a sibling of root, used to test the
    ///                   "root/../sibling" canonicalization case)
    fn make_layout() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        let root = dir.path();

        fs::write(root.join("file"), "x").unwrap();
        fs::create_dir(root.join("subdir")).unwrap();
        fs::write(root.join("subdir").join("nested"), "x").unwrap();
        fs::write(root.join("foobar"), "x").unwrap();
        fs::create_dir(root.join("sibling")).unwrap();

        dir
    }

    /// Edge case 1: cwd == project_root  →  ✅
    #[test]
    fn case1_cwd_equals_root() {
        let dir = make_layout();
        let root = dir.path();
        let real = assert_within_root(root, root).expect("root contains itself");
        assert_eq!(
            real.canonicalize().unwrap(),
            root.canonicalize().unwrap()
        );
    }

    /// Edge case 2: cwd == root/subdir  →  ✅
    #[test]
    fn case2_cwd_is_subdir() {
        let dir = make_layout();
        let root = dir.path();
        let sub = root.join("subdir");
        let real = assert_within_root(root, &sub).expect("subdir is inside root");
        assert!(real.starts_with(root.canonicalize().unwrap()));
    }

    /// Edge case 3: cwd = root/../sibling (canonicalizes to a sibling
    ///   of root). Must be rejected.
    #[test]
    fn case3_traversal_outside_root_rejected() {
        // Two-sibling layout: root and sibling are both children of
        // the tempdir. `root/../sibling` canonicalizes to the sibling
        // (which is physically outside root) and must be rejected.
        let dir = tempdir().unwrap();
        let root = dir.path().join("root");
        let sibling = dir.path().join("sibling");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&sibling).unwrap();

        let traversal = root.join("../sibling");
        // canonicalize the traversal first so we know it resolves to
        // a real path; the actual rejection then is "outside root".
        let canonical_traversal = traversal
            .canonicalize()
            .expect("sibling exists, traversal should resolve");
        // Sanity: canonical traversal IS a sibling of canonical root.
        assert!(canonical_traversal.starts_with(dir.path().canonicalize().unwrap()));
        assert!(
            !canonical_traversal.starts_with(root.canonicalize().unwrap()),
            "sibling must not be inside root"
        );

        let res = assert_within_root(&root, &traversal);
        assert!(res.is_err(), "sibling must be rejected");
        let msg = format!("{}", res.unwrap_err());
        assert!(
            msg.contains("outside project root"),
            "error message should explain the rejection: {}",
            msg
        );
    }

    /// Edge case 4: prefix-trap — root is `/repo/foo`, target is
    /// `/repo/foobar`. Must reject (component-wise, not string prefix).
    #[test]
    fn case4_prefix_trap_rejected() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        // root = ".../foo"
        let root_foo = root.join("foo");
        fs::create_dir(&root_foo).unwrap();
        // Sibling "foobar" at the same level as "foo" — its canonical
        // path shares the "/foo" string prefix with root_foo, but is
        // not inside foo.
        let foobar = root.join("foobar");
        fs::create_dir(&foobar).unwrap();

        let res = assert_within_root(&root_foo, &foobar);
        assert!(res.is_err(), "prefix trap must be rejected");
    }

    /// Edge case 5: symlink inside root points outside root  →  ❌
    /// (canonicalize resolves the symlink, so the post-canonicalize
    ///  path is no longer under root.)
    #[test]
    fn case5_symlink_outside_root_rejected() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("root");
        let outside = dir.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("file"), "secret").unwrap();

        // Symlink: root/esc → ../outside
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("../outside", root.join("esc")).unwrap();
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(
                outside.to_str().unwrap(),
                root.join("esc").to_str().unwrap(),
            )
            .unwrap();
        }

        // Walking through the symlink should canonicalize to `outside`,
        // which is not under `root`.
        let res = assert_within_root(&root, &root.join("esc"));
        assert!(res.is_err(), "symlink escaping root must be rejected");
    }

    /// Edge case 6: target does not exist  →  ❌ (cannot prove it is
    /// inside the root if it does not exist).
    #[test]
    fn case6_nonexistent_target_rejected() {
        let dir = make_layout();
        let root = dir.path();
        let ghost = root.join("no-such-dir");
        let res = assert_within_root(root, &ghost);
        assert!(res.is_err(), "nonexistent path must be rejected");
        let msg = format!("{}", res.unwrap_err());
        assert!(
            msg.contains("cannot be resolved")
                || msg.contains("does not exist")
                || msg.contains("No such file"),
            "error message should explain the rejection: {}",
            msg
        );
    }

    /// Edge case 7: broken symlink  →  ❌ (canonicalize fails).
    #[test]
    fn case7_broken_symlink_rejected() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        // `tempdir` already created the directory, so just symlink.
        // Symlink pointing at a path that does not exist.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("does-not-exist", root.join("dead")).unwrap();
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file(
                "does-not-exist",
                root.join("dead").to_str().unwrap(),
            )
            .unwrap();
        }

        let res = assert_within_root(root, &root.join("dead"));
        assert!(res.is_err(), "broken symlink must be rejected");
    }

    /// Edge case 5 + sub: a symlink that points *inside* root must be
    /// accepted (we're not anti-symlink in general — we only block
    /// escapes).
    #[test]
    fn symlink_pointing_inside_root_accepted() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        // `tempdir` already created the directory.
        fs::create_dir(root.join("real")).unwrap();
        fs::write(root.join("real").join("file"), "x").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("real", root.join("link")).unwrap();
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(
                root.join("real").to_str().unwrap(),
                root.join("link").to_str().unwrap(),
            )
            .unwrap();
        }

        // Walking through the symlink: the post-canonicalize path is
        // .../root/real, which is inside root.
        let res = assert_within_root(root, &root.join("link"));
        assert!(
            res.is_ok(),
            "symlink staying inside root must be accepted: {:?}",
            res.err()
        );
    }

    /// Sub-case: a path identical to root after canonicalize returns
    /// the canonical path.
    #[test]
    fn returns_canonical_path() {
        let dir = make_layout();
        let root = dir.path();
        // root/.  -> canonicalize -> root
        let dotted = root.join(".");
        let real = assert_within_root(root, &dotted).expect("self via .");
        assert_eq!(real, root.canonicalize().unwrap());
    }

    // -----------------------------------------------------------------------
    // is_within_root — non-failing boolean variant (A2+B7 re-grill, 2026-06-13)
    // -----------------------------------------------------------------------

    /// Same path → inside.
    #[test]
    fn is_within_root_self_is_inside() {
        let dir = tempdir().unwrap();
        assert!(is_within_root(dir.path(), dir.path()));
    }

    /// Direct child → inside.
    #[test]
    fn is_within_root_child_is_inside() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let child = root.join("sub");
        assert!(is_within_root(root, &child));
    }

    /// Grandchild → inside.
    #[test]
    fn is_within_root_grandchild_is_inside() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let grand = root.join("a").join("b").join("c.txt");
        assert!(is_within_root(root, &grand));
    }

    /// Sibling → outside.
    #[test]
    fn is_within_root_sibling_is_outside() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let sibling = root.join("..").join("sibling");
        assert!(!is_within_root(root, &sibling));
    }

    /// **Prefix-trap**: `/repo/foo` (root) vs `/repo/foobar` (target).
    /// Must NOT match (component-wise, not string-prefix).
    #[test]
    fn is_within_root_prefix_trap_rejected() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let root_foo = root.join("foo");
        let foobar = root.join("foobar");
        // foobar is NOT under foo (siblings).
        assert!(!is_within_root(&root_foo, &foobar));
    }

    /// Non-existent target inside the project → accepted (parent
    /// is inside). The tool layer's `assert_within_root` would
    /// reject this; the permission layer accepts because the
    /// user might be writing a new file.
    #[test]
    fn is_within_root_nonexistent_child_accepted() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let ghost = root.join("not-yet-created.txt");
        assert!(!ghost.exists());
        assert!(is_within_root(root, &ghost));
    }

    /// Non-existent target outside the project → rejected.
    #[test]
    fn is_within_root_nonexistent_outside_rejected() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let ghost = root.join("..").join("nonexistent.txt");
        assert!(!is_within_root(root, &ghost));
    }

    /// Empty target → not a valid path, returns false.
    #[test]
    fn is_within_root_empty_target_rejected() {
        let dir = tempdir().unwrap();
        let empty = std::path::Path::new("");
        assert!(!is_within_root(dir.path(), empty));
    }
}
