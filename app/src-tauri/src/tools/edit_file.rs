//! `edit_file` tool — surgical string replacement in an existing file.
//!
//! The tool mirrors claude-code's `Edit` tool behavior:
//! - The file must have been read in the current session (`ReadGuard`).
//! - The file must not have changed on disk since it was read
//!   (`ReadGuard::verify_fresh`).
//! - `old_string` must appear exactly 0 or 1 times (or `replace_all: true`
//!   is set, in which case all occurrences are replaced).
//! - 0 matches → 0 matches → error with 0-3 most-similar lines as a hint,
//!   claude-code style. We do NOT auto-strip trailing whitespace and retry
//!   (claude-code's own behavior; see `.trellis/tasks/06-07-06-07-extend-toolset/research/01-pi-agent-claude-code.md`).
//! - `old_string == new_string` → rejected as a no-op.
//! - On success, the file's ReadGuard fingerprint is invalidated so the
//!   LLM is forced to re-read on the next edit attempt.
//!
//! Returns `(content, is_error)` like the other tools.

use std::path::{Path, PathBuf};

use crate::llm::types::ToolDef;
use crate::projects::boundary::assert_within_root;
use crate::tools::read_guard::ReadGuard;
use crate::tools::ToolContext;

pub fn definition() -> ToolDef {
    ToolDef {
        name: "edit_file".to_string(),
        description: Some(
            "Apply a surgical edit to an existing file. The file must have been \
             read with read_file in the current session, and must not have been \
             modified on disk since. Replaces `old_string` with `new_string`.\n\n\
             `old_string` must match exactly (whitespace-sensitive). If it appears \
             zero times the tool returns an error with hint lines; if it appears \
             more than once and `replace_all` is not set, the tool returns the line \
             numbers of all matches and asks for more context.\n\n\
             Paths may be relative (resolved against the session cwd) or absolute; \
             the resolved path must be inside the active project root."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute or relative (to the session cwd) path of the file to edit."
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact text to find in the file. Must match exactly, including whitespace."
                },
                "new_string": {
                    "type": "string",
                    "description": "The text to replace `old_string` with. Must be different from `old_string`."
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace all occurrences of `old_string`. Default: false (unique match required)."
                }
            },
            "required": ["path", "old_string", "new_string"]
        }),
    }
}

/// Execute the tool. Returns `(content, is_error)`.
pub async fn execute(
    input: &serde_json::Value,
    ctx: &ToolContext,
    guard: &ReadGuard,
    session_id: &str,
) -> (String, bool) {
    let raw_path = match input.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return ("Missing required parameter: path".to_string(), true),
    };
    let old_string = match input.get("old_string").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return ("Missing required parameter: old_string".to_string(), true),
    };
    let new_string = match input.get("new_string").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return ("Missing required parameter: new_string".to_string(), true),
    };
    let replace_all = input
        .get("replace_all")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // 1. Argument-level pre-checks.
    if old_string.is_empty() {
        return (
            "old_string must not be empty. Supply the exact text you want to replace.".to_string(),
            true,
        );
    }
    if old_string == new_string {
        return (
            "old_string and new_string are identical — this would be a no-op edit.".to_string(),
            true,
        );
    }

    // 2. Resolve to an absolute path (relative → ctx.cwd + path).
    let requested: PathBuf = {
        let p = Path::new(raw_path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            ctx.cwd.join(p)
        }
    };

    // 3. Boundary check. We must canonicalize the existing file to a
    //    real path; assert_within_root will reject nonexistent paths,
    //    but a file that already exists canonicalizes fine.
    let validated = match assert_within_root(&ctx.worktree_path, &requested) {
        Ok(p) => p,
        Err(e) => {
            return (
                format!("path '{}' rejected: {}", raw_path, e),
                true,
            );
        }
    };

    // 4. ReadGuard: must have been read in this session.
    if let Err(e) = guard.verify_read(session_id, &validated).await {
        return (e, true);
    }

    // 5. ReadGuard: must be unchanged on disk since the read.
    if let Err(e) = guard.verify_fresh(session_id, &validated).await {
        return (e, true);
    }

    // 6. Read the current contents and apply the match logic.
    let current = match tokio::fs::read_to_string(&validated).await {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(
                path = %validated.display(),
                error = %e,
                "edit_file: failed to read current file contents"
            );
            return (
                format!("Failed to read file '{}': {}", validated.display(), e),
                true,
            );
        }
    };

    let occurrences = count_occurrences(&current, old_string);

    if occurrences == 0 {
        let hints = find_similar_lines(&current, old_string, 3);
        let hint_section = if hints.is_empty() {
            String::new()
        } else {
            format!(
                "\nClosest match{} (line{}):\n{}",
                if hints.len() == 1 { "" } else { "es" },
                if hints.len() == 1 { "" } else { "s" },
                hints
                    .iter()
                    .map(|(n, l)| format!("  line {}: {}", n, l))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };
        return (
            format!(
                "old_string not found in '{}'. Read the file again to see the current contents.{}",
                validated.display(),
                hint_section
            ),
            true,
        );
    }

    if occurrences > 1 && !replace_all {
        // Find line numbers of every match so the LLM can disambiguate.
        let lines: Vec<usize> = line_numbers_of_matches(&current, old_string)
            .into_iter()
            .take(20) // cap the line list to keep the message small
            .collect();
        return (
            format!(
                "old_string appears {} times in '{}' (at lines: {}). Add more context to make it unique, or pass replace_all: true.",
                occurrences,
                validated.display(),
                lines
                    .iter()
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            true,
        );
    }

    // 7. Apply the edit. If replace_all, replace every occurrence;
    //    otherwise the unique match.
    let new_contents = if replace_all {
        current.replace(old_string, new_string)
    } else {
        current.replacen(old_string, new_string, 1)
    };

    if let Err(e) = tokio::fs::write(&validated, &new_contents).await {
        tracing::debug!(
            path = %validated.display(),
            error = %e,
            "edit_file: write failed"
        );
        return (
            format!("Failed to write file '{}': {}", validated.display(), e),
            true,
        );
    }

    // 8. Invalidate the fingerprint so the LLM re-reads on the next edit.
    guard.invalidate(session_id, &validated).await;

    let summary = if replace_all {
        format!(
            "Successfully edited '{}': replaced {} occurrences.",
            validated.display(),
            occurrences
        )
    } else {
        format!("Successfully edited '{}'.", validated.display())
    };
    (summary, false)
}

/// Count non-overlapping occurrences of `needle` in `haystack`.
fn count_occurrences(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    haystack.match_indices(needle).count()
}

/// Return the 1-based line numbers of every match.
fn line_numbers_of_matches(haystack: &str, needle: &str) -> Vec<usize> {
    if needle.is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut line_num = 1usize;
    let mut last_idx = 0usize;
    for (idx, _) in haystack.match_indices(needle) {
        // count newlines between last_idx and idx
        while last_idx < idx {
            if haystack.as_bytes()[last_idx] == b'\n' {
                line_num += 1;
            }
            last_idx += 1;
        }
        // Match starts at `line_num`; the match may span multiple
        // lines but we report the starting line.
        lines.push(line_num);
        last_idx = idx + needle.len();
        // Advance line_num for any newlines inside the match.
        for byte in &haystack.as_bytes()[idx..last_idx] {
            if *byte == b'\n' {
                line_num += 1;
            }
        }
    }
    lines
}

/// Return up to `limit` lines from `haystack` that share the most
/// whitespace-stripped characters with `needle`. Used as a hint when
/// `old_string` is not found.
fn find_similar_lines(haystack: &str, needle: &str, limit: usize) -> Vec<(usize, String)> {
    let needle_norm: String = needle.split_whitespace().collect::<Vec<_>>().join(" ");
    if needle_norm.is_empty() {
        return Vec::new();
    }
    let needle_chars: std::collections::HashSet<char> = needle_norm.chars().collect();
    let mut scored: Vec<(usize, String, usize)> = Vec::new();
    for (i, line) in haystack.lines().enumerate() {
        let line_norm: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line_norm.is_empty() {
            continue;
        }
        let line_chars: std::collections::HashSet<char> = line_norm.chars().collect();
        let intersection = needle_chars.intersection(&line_chars).count();
        let union = needle_chars.union(&line_chars).count();
        let jaccard = if union == 0 {
            0
        } else {
            intersection * 1000 / union
        };
        scored.push((i + 1, line.to_string(), jaccard));
    }
    scored.sort_by(|a, b| b.2.cmp(&a.2));
    scored
        .into_iter()
        .filter(|(_, _, s)| *s > 0)
        .take(limit)
        .map(|(n, l, _)| (n, l))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::read_guard::ReadGuard;
    use tempfile::tempdir;

    fn test_ctx(tmp: &tempfile::TempDir) -> ToolContext {
        ToolContext {
            worktree_path: tmp.path().canonicalize().unwrap(),
            cwd: tmp.path().canonicalize().unwrap(),
            checklist: crate::tools::update_checklist::new_handle(),
        }
    }

    /// Helper: record a read so the guard considers the file "read".
    async fn mark_read(guard: &ReadGuard, session_id: &str, path: &Path) {
        guard.record_read(session_id, path).await;
    }

    #[test]
    fn definition_has_correct_name() {
        assert_eq!(definition().name, "edit_file");
    }

    /// AC1.1: happy path — read + edit + verify file content changed.
    #[tokio::test]
    async fn happy_path() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("a.txt");
        std::fs::write(&p, "hello world\n").unwrap();
        let ctx = test_ctx(&tmp);
        let guard = ReadGuard::new();
        mark_read(&guard, "s1", &p).await;

        let (msg, is_err) = execute(
            &serde_json::json!({
                "path": p.to_string_lossy(),
                "old_string": "hello",
                "new_string": "goodbye",
            }),
            &ctx,
            &guard,
            "s1",
        )
        .await;
        assert!(!is_err, "{}", msg);
        assert!(msg.contains("Successfully edited"));
        assert_eq!(tokio::fs::read_to_string(&p).await.unwrap(), "goodbye world\n");
    }

    /// AC1.2: edit before read — ReadGuard rejects.
    #[tokio::test]
    async fn edit_before_read_rejected() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("a.txt");
        std::fs::write(&p, "hello\n").unwrap();
        let ctx = test_ctx(&tmp);
        let guard = ReadGuard::new();
        // No record_read call.

        let (msg, is_err) = execute(
            &serde_json::json!({
                "path": p.to_string_lossy(),
                "old_string": "hello",
                "new_string": "bye",
            }),
            &ctx,
            &guard,
            "s1",
        )
        .await;
        assert!(is_err);
        assert!(msg.contains("read_file"));
    }

    /// AC1.3: edit after external modify — ReadGuard freshness check rejects.
    #[tokio::test]
    async fn edit_after_external_modify_rejected() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("a.txt");
        std::fs::write(&p, "hello\n").unwrap();
        let ctx = test_ctx(&tmp);
        let guard = ReadGuard::new();
        mark_read(&guard, "s1", &p).await;

        // External modify.
        std::fs::write(&p, "hello world\n").unwrap();
        // Sleep a bit so mtime is at least 1ms newer on slow filesystems.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let (msg, is_err) = execute(
            &serde_json::json!({
                "path": p.to_string_lossy(),
                "old_string": "hello",
                "new_string": "bye",
            }),
            &ctx,
            &guard,
            "s1",
        )
        .await;
        assert!(is_err);
        assert!(msg.contains("changed on disk") || msg.contains("Re-read"));
    }

    /// AC1.4: 0 matches → error with hint lines.
    #[tokio::test]
    async fn old_string_not_found() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("a.txt");
        std::fs::write(&p, "first line\nsecond line\nthird line\n").unwrap();
        let ctx = test_ctx(&tmp);
        let guard = ReadGuard::new();
        mark_read(&guard, "s1", &p).await;

        let (msg, is_err) = execute(
            &serde_json::json!({
                "path": p.to_string_lossy(),
                "old_string": "nonexistent text that is not in the file",
                "new_string": "replacement",
            }),
            &ctx,
            &guard,
            "s1",
        )
        .await;
        assert!(is_err);
        assert!(msg.contains("not found"));
        // Hint section is present.
        assert!(msg.contains("Closest match"));
    }

    /// AC1.5: N>1 matches without replace_all → error with line numbers.
    #[tokio::test]
    async fn old_string_ambiguous() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("a.txt");
        std::fs::write(&p, "foo\nbar\nfoo\nbaz\nfoo\n").unwrap();
        let ctx = test_ctx(&tmp);
        let guard = ReadGuard::new();
        mark_read(&guard, "s1", &p).await;

        let (msg, is_err) = execute(
            &serde_json::json!({
                "path": p.to_string_lossy(),
                "old_string": "foo",
                "new_string": "qux",
            }),
            &ctx,
            &guard,
            "s1",
        )
        .await;
        assert!(is_err);
        assert!(msg.contains("appears 3 times"));
        assert!(msg.contains("lines:"));
    }

    /// AC1.5b: N>1 matches WITH replace_all → all replaced.
    #[tokio::test]
    async fn replace_all_works() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("a.txt");
        std::fs::write(&p, "foo\nbar\nfoo\n").unwrap();
        let ctx = test_ctx(&tmp);
        let guard = ReadGuard::new();
        mark_read(&guard, "s1", &p).await;

        let (msg, is_err) = execute(
            &serde_json::json!({
                "path": p.to_string_lossy(),
                "old_string": "foo",
                "new_string": "qux",
                "replace_all": true,
            }),
            &ctx,
            &guard,
            "s1",
        )
        .await;
        assert!(!is_err, "{}", msg);
        assert!(msg.contains("replaced 2 occurrences"));
        assert_eq!(tokio::fs::read_to_string(&p).await.unwrap(), "qux\nbar\nqux\n");
    }

    /// AC1.6: no-op (old == new) → rejected.
    #[tokio::test]
    async fn no_op_rejected() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("a.txt");
        std::fs::write(&p, "hello\n").unwrap();
        let ctx = test_ctx(&tmp);
        let guard = ReadGuard::new();
        mark_read(&guard, "s1", &p).await;

        let (msg, is_err) = execute(
            &serde_json::json!({
                "path": p.to_string_lossy(),
                "old_string": "hello",
                "new_string": "hello",
            }),
            &ctx,
            &guard,
            "s1",
        )
        .await;
        assert!(is_err);
        assert!(msg.contains("no-op"));
    }

    /// AC1.7: relative path resolves against ctx.cwd.
    #[tokio::test]
    async fn relative_path_works() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("a.txt");
        std::fs::write(&p, "hello\n").unwrap();
        let ctx = test_ctx(&tmp);
        let guard = ReadGuard::new();
        // Read via absolute path so the guard key is canonical.
        mark_read(&guard, "s1", &p).await;

        let (msg, is_err) = execute(
            &serde_json::json!({
                "path": "a.txt",
                "old_string": "hello",
                "new_string": "bye",
            }),
            &ctx,
            &guard,
            "s1",
        )
        .await;
        assert!(!is_err, "{}", msg);
        assert_eq!(tokio::fs::read_to_string(&p).await.unwrap(), "bye\n");
    }

    /// AC1.7b: path outside project root → boundary check rejects.
    #[tokio::test]
    async fn path_outside_root_rejected() {
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(&tmp);
        let guard = ReadGuard::new();
        let (msg, is_err) = execute(
            &serde_json::json!({
                "path": "/etc/hostname",
                "old_string": "x",
                "new_string": "y",
            }),
            &ctx,
            &guard,
            "s1",
        )
        .await;
        assert!(is_err);
        assert!(msg.contains("rejected") || msg.contains("outside"));
    }

    /// Edit invalidates the fingerprint so the next edit must re-read.
    #[tokio::test]
    async fn successful_edit_invalidates_fingerprint() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("a.txt");
        std::fs::write(&p, "hello\n").unwrap();
        let ctx = test_ctx(&tmp);
        let guard = ReadGuard::new();
        mark_read(&guard, "s1", &p).await;

        // First edit succeeds.
        let (_, is_err) = execute(
            &serde_json::json!({
                "path": p.to_string_lossy(),
                "old_string": "hello",
                "new_string": "bye",
            }),
            &ctx,
            &guard,
            "s1",
        )
        .await;
        assert!(!is_err);
        // Second edit on the same file fails: fingerprint was invalidated.
        let (msg, is_err) = execute(
            &serde_json::json!({
                "path": p.to_string_lossy(),
                "old_string": "bye",
                "new_string": "later",
            }),
            &ctx,
            &guard,
            "s1",
        )
        .await;
        assert!(is_err);
        assert!(msg.contains("read_file"));
    }

    /// Empty old_string → rejected at argument level.
    #[tokio::test]
    async fn empty_old_string_rejected() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("a.txt");
        std::fs::write(&p, "hello\n").unwrap();
        let ctx = test_ctx(&tmp);
        let guard = ReadGuard::new();
        mark_read(&guard, "s1", &p).await;

        let (msg, is_err) = execute(
            &serde_json::json!({
                "path": p.to_string_lossy(),
                "old_string": "",
                "new_string": "x",
            }),
            &ctx,
            &guard,
            "s1",
        )
        .await;
        assert!(is_err);
        assert!(msg.contains("must not be empty"));
    }

    /// Unit test for line_numbers_of_matches.
    #[test]
    fn line_numbers_basic() {
        let lines = line_numbers_of_matches("a\nb\nfoo\nc\nfoo\n", "foo");
        assert_eq!(lines, vec![3, 5]);
    }

    /// Unit test for find_similar_lines (smoke test).
    #[test]
    fn similar_lines_returns_up_to_limit() {
        let src = "alpha beta\nbeta gamma\nbeta delta\n";
        let hints = find_similar_lines(src, "alpha beta", 2);
        assert!(!hints.is_empty());
        // First hit should be the exact-match line.
        assert!(hints[0].1.contains("alpha beta"));
    }
}
