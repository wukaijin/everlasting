//! `read_file` tool — read a file's contents.
//!
//! Step 3b-1 changes:
//! - The `path` parameter is resolved relative to `ctx.cwd` if it is
//!   not absolute.
//! - Once resolved to an absolute path, it must be inside
//!   `ctx.project_root` — enforced by
//!   `projects::boundary::assert_within_root`.
//! - Both the "is the path inside the project?" check and the
//!   "does the file exist?" failure mode are returned to the LLM as
//!   `is_error = true` with a human-readable message.
//!
//! Step toolset-extension changes:
//! - On a successful read, the result is prefixed with 1-based line
//!   numbers in `cat -n` style (e.g. `\t1\t`). This lets the LLM
//!   reference specific lines back to `edit_file`, which echoes line
//!   numbers in its error hints.
//! - On a successful read, the (session_id, path) pair is recorded in
//!   the `ReadGuard` so a subsequent `edit_file` can verify the file
//!   hasn't drifted on disk. The guard is `Option` so existing
//!   callers (and unit tests that don't care) can pass `None` and
//!   the read still works.

use std::path::Path;

use crate::llm::types::ToolDef;
use crate::projects::boundary::assert_within_root;
use crate::tools::read_guard::ReadGuard;
use crate::tools::ToolContext;

/// Max output before truncation (matches ARCHITECTURE.md §2.5.3).
/// Applies BEFORE the `cat -n` prefix is added, so a 50 KB file still
/// gets 50 KB of line-numbered output.
const MAX_OUTPUT_BYTES: usize = 50 * 1024;

/// Bytes reserved for the head and tail each, when the file is
/// truncated. Matches the 25 KB + 25 KB layout used by the step 2
/// `truncate_output` (we keep the same head/tail split so users who
/// upgrade mid-conversation see the same pattern).
const TRUNCATE_HEAD: usize = 25 * 1024;

pub fn definition() -> ToolDef {
    ToolDef {
        name: "read_file".to_string(),
        description: Some(
            "Read the contents of a file. Paths may be relative (resolved against \
             the session's current working directory) or absolute. In either case \
             the resolved file must be inside the active project root.\n\n\
             Output is prefixed with line numbers in `cat -n` format (tab-separated, \
             1-based) to help you reference specific lines in `edit_file`."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute or relative (to the session cwd) path of the file to read."
                }
            },
            "required": ["path"]
        }),
    }
}

/// Execute the tool. Returns `(content, is_error)`.
///
/// `guard` and `session_id` are optional: when both are present, the
/// read is recorded in the guard (so a follow-up `edit_file` can
/// verify freshness). When either is `None`, the read still works
/// but the guard is not updated — the agent loop in `lib.rs::chat`
/// always supplies both.
pub async fn execute(
    input: &serde_json::Value,
    ctx: &ToolContext,
    guard: Option<&ReadGuard>,
    session_id: Option<&str>,
) -> (String, bool) {
    let raw_path = match input.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return ("Missing required parameter: path".to_string(), true),
    };

    // 1. Resolve to an absolute Path. Relative inputs anchor on
    //    ctx.cwd (the session's current working directory).
    let requested: std::path::PathBuf = {
        let p = Path::new(raw_path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            ctx.cwd.join(p)
        }
    };

    // 2. Boundary check: the resolved path must be physically
    //    located inside the project root. This handles both
    //    "absolute path is outside" and "relative path resolves to
    //    outside (e.g. via ../../)" uniformly.
    let validated = match assert_within_root(&ctx.project_root, &requested) {
        Ok(p) => p,
        Err(e) => {
            return (
                format!("path '{}' rejected: {}", raw_path, e),
                true,
            );
        }
    };

    match tokio::fs::read_to_string(&validated).await {
        Ok(content) => {
            // Record the read in the guard so edit_file can verify
            // freshness later. We do this AFTER the read succeeds
            // (so the recorded fingerprint matches what the LLM saw)
            // and BEFORE the truncation (so the fingerprint covers
            // the full file, not just the head/tail slice).
            if let (Some(g), Some(sid)) = (guard, session_id) {
                g.record_read(sid, &validated).await;
            }
            (truncate_output(content), false)
        }
        Err(e) => (
            format!("Failed to read file '{}': {}", validated.display(), e),
            true,
        ),
    }
}

/// Truncate output exceeding MAX_OUTPUT_BYTES (head + tail, omit middle),
/// then prefix every line with its 1-based line number (`cat -n` style).
///
/// Truncation runs **first** so the byte budget measures the raw file
/// (consistent with the step 2 contract), and the line numbers are
/// computed on the truncated text. For a 60 KB file, the user gets
/// the first 25 KB of line-numbered text, a truncation marker, and
/// the last 25 KB of line-numbered text — the middle's line numbers
/// are not echoed (the marker carries the missing byte count instead).
fn truncate_output(content: String) -> String {
    if content.len() <= MAX_OUTPUT_BYTES {
        return add_line_numbers(&content);
    }
    let head_end = TRUNCATE_HEAD;
    let tail_start = content.len() - TRUNCATE_HEAD;
    let omitted = content.len() - MAX_OUTPUT_BYTES;
    let head = add_line_numbers(&content[..head_end]);
    let tail = add_line_numbers(&content[tail_start..]);
    format!(
        "{}\n<truncated: omitted {} bytes>\n{}",
        head, omitted, tail
    )
}

/// Add `cat -n` style line numbers to `text`. Each output line is
/// `<tab><line_num><tab><text>`. Lines are split on `\n`; an empty
/// trailing string (text ending in `\n`) does not produce a phantom
/// line number, matching `cat -n`.
fn add_line_numbers(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.lines().count() * 8);
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push('\t');
        out.push_str(&(i + 1).to_string());
        out.push('\t');
        out.push_str(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_ctx(tmp: &tempfile::TempDir) -> ToolContext {
        ToolContext {
            project_root: tmp.path().canonicalize().unwrap(),
            cwd: tmp.path().canonicalize().unwrap(),
        }
    }

    #[test]
    fn definition_has_correct_name() {
        assert_eq!(definition().name, "read_file");
    }

    #[test]
    fn definition_schema_requires_path() {
        let schema = &definition().input_schema;
        let required = schema.get("required").unwrap().as_array().unwrap();
        assert!(required.iter().any(|r| r.as_str() == Some("path")));
    }

    /// AC6.1: simple file gets `cat -n` line numbers.
    #[tokio::test]
    async fn execute_reads_real_file_with_line_numbers() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("hello.txt"), "world").unwrap();
        let (content, is_error) = execute(
            &serde_json::json!({"path": tmp.path().join("hello.txt").to_string_lossy()}),
            &test_ctx(&tmp),
            None,
            None,
        )
        .await;
        assert!(!is_error);
        // Format: \t1\tworld
        assert!(content.starts_with("\t1\tworld"), "got: {:?}", content);
    }

    /// AC6.2: multi-line file — line numbers are 1-based and per-line.
    #[tokio::test]
    async fn execute_reads_multiline_with_consecutive_line_numbers() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "first\nsecond\nthird\n").unwrap();
        let (content, is_error) = execute(
            &serde_json::json!({"path": tmp.path().join("a.txt").to_string_lossy()}),
            &test_ctx(&tmp),
            None,
            None,
        )
        .await;
        assert!(!is_error);
        assert!(content.contains("\t1\tfirst"));
        assert!(content.contains("\t2\tsecond"));
        assert!(content.contains("\t3\tthird"));
    }

    /// AC6.3: empty lines still get a line number.
    #[tokio::test]
    async fn execute_empty_lines_have_line_numbers() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "a\n\nb\n").unwrap();
        let (content, is_error) = execute(
            &serde_json::json!({"path": tmp.path().join("a.txt").to_string_lossy()}),
            &test_ctx(&tmp),
            None,
            None,
        )
        .await;
        assert!(!is_error);
        // Three lines: a, (empty), b. Each prefixed.
        assert!(content.contains("\t1\ta"));
        assert!(content.contains("\t2\t"));
        assert!(content.contains("\t3\tb"));
    }

    /// AC6.4: truncation preserves line numbers on both head and tail.
    #[tokio::test]
    async fn execute_truncation_preserves_line_numbers() {
        let tmp = tempdir().unwrap();
        // Build a file > 50 KB so truncation kicks in.
        let line = "x".repeat(80) + "\n";
        let big = line.repeat(700); // ~56 KB
        std::fs::write(tmp.path().join("big.txt"), &big).unwrap();
        let (content, is_error) = execute(
            &serde_json::json!({"path": tmp.path().join("big.txt").to_string_lossy()}),
            &test_ctx(&tmp),
            None,
            None,
        )
        .await;
        assert!(!is_error);
        // The truncation marker should be present.
        assert!(content.contains("<truncated:"));
        // The head should be line-numbered (starts with \t1\t).
        assert!(content.starts_with("\t1\t"), "got: {:?}", &content[..40]);
    }

    #[tokio::test]
    async fn execute_resolves_relative_path_against_cwd() {
        let tmp = tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        std::fs::write(tmp.path().join("sub").join("a.txt"), "relative").unwrap();

        // ctx.cwd points to root; relative path "sub/a.txt" resolves there.
        let (content, is_error) = execute(
            &serde_json::json!({"path": "sub/a.txt"}),
            &test_ctx(&tmp),
            None,
            None,
        )
        .await;
        assert!(!is_error, "{}", content);
        assert!(content.contains("relative"));
        assert!(content.contains("\t1\t"));
    }

    #[tokio::test]
    async fn execute_rejects_traversal_outside_root() {
        let tmp = tempdir().unwrap();
        let (msg, is_error) = execute(
            &serde_json::json!({"path": "/etc/hostname"}),
            &test_ctx(&tmp),
            None,
            None,
        )
        .await;
        assert!(is_error);
        assert!(msg.contains("rejected") || msg.contains("outside"));
    }

    #[tokio::test]
    async fn execute_rejects_relative_traversal() {
        let tmp = tempdir().unwrap();
        let (msg, is_error) = execute(
            &serde_json::json!({"path": "../../etc/hostname"}),
            &test_ctx(&tmp),
            None,
            None,
        )
        .await;
        assert!(is_error);
        assert!(msg.contains("rejected") || msg.contains("outside") || msg.contains("cannot be resolved"));
    }

    #[tokio::test]
    async fn execute_missing_path_param() {
        let tmp = tempdir().unwrap();
        let (content, is_error) = execute(&serde_json::json!({}), &test_ctx(&tmp), None, None).await;
        assert!(is_error);
        assert!(content.contains("Missing"));
    }

    #[tokio::test]
    async fn execute_nonexistent_file() {
        let tmp = tempdir().unwrap();
        let (content, is_error) = execute(
            &serde_json::json!({"path": tmp.path().join("nope.txt").to_string_lossy()}),
            &test_ctx(&tmp),
            None,
            None,
        )
        .await;
        assert!(is_error);
        // Boundary check rejects nonexistent files (canonicalize
        // fails), so the error is "rejected", not "Failed to read".
        assert!(content.contains("rejected") || content.contains("Failed to read"));
    }

    /// When a guard + session_id are provided, the read is recorded.
    #[tokio::test]
    async fn execute_records_read_in_guard() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "hello").unwrap();
        let guard = ReadGuard::new();
        let (content, is_error) = execute(
            &serde_json::json!({"path": tmp.path().join("a.txt").to_string_lossy()}),
            &test_ctx(&tmp),
            Some(&guard),
            Some("s1"),
        )
        .await;
        assert!(!is_error, "{}", content);
        // The guard should now know about this path.
        guard.verify_read("s1", &tmp.path().join("a.txt")).await.unwrap();
    }

    /// add_line_numbers unit test — empty trailing newline doesn't add a phantom line.
    #[test]
    fn add_line_numbers_no_phantom_line() {
        let out = add_line_numbers("a\nb\n");
        assert_eq!(out, "\t1\ta\n\t2\tb");
    }

    /// add_line_numbers unit test — single line.
    #[test]
    fn add_line_numbers_single_line() {
        let out = add_line_numbers("hello");
        assert_eq!(out, "\t1\thello");
    }
}
