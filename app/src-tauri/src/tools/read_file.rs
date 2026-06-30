//! `read_file` tool — read a file's contents.
//!
//! Step 3b-1 changes:
//! - The `path` parameter is resolved relative to `ctx.cwd` if it is
//!   not absolute.
//! - Once resolved to an absolute path, it must be inside
//!   `ctx.worktree_path` — enforced by
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
//!
//! P0 enhancement (2026-06-12):
//! - `offset` (1-indexed, default 1) and `limit` (default 2000) let
//!   the LLM read a specific line range from a large file instead of
//!   getting the full 50KB head+tail truncation.
//! - Line numbers in the output start from `offset` (not 1), so the
//!   LLM can reference the real file line numbers in `edit_file`.
//! - The ReadGuard fingerprint still covers the full file (offset/
//!   limit only affect the output slice, not the guard).


use crate::llm::types::ToolDef;
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
             1-based) to help you reference specific lines in `edit_file`.\n\n\
             For large files, use `offset` and `limit` to read a specific line range \
             instead of getting the full 50KB head+tail truncation. `offset` is \
             1-indexed (the first line is line 1). Line numbers in the output reflect \
             the real file line numbers, not relative to offset."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute or relative (to the session cwd) path of the file to read."
                },
                "offset": {
                    "type": "integer",
                    "description": "Starting line number (1-indexed). Default: 1 (read from the beginning)."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to return. Default: 2000."
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

    // 1. Resolve (with `~` home expansion; see boundary::resolve_path).
    let requested: std::path::PathBuf =
        crate::projects::boundary::resolve_path(raw_path, &ctx.cwd);

    // 2. read-side boundary decouple (2026-07-01): tool-layer
    //    assert_within_root removed for read 族 — project-outside
    //    reads are gated by the permission layer (Tier 2.5 sensitive
    //    deny-list + Tier 4 trusted allow-list + ask_path).
    //    assert_within_root stays for write_file/edit_file.
    let validated = requested;

    // 3. Parse offset and limit parameters.
    let offset = input
        .get("offset")
        .and_then(|v| v.as_u64())
        .unwrap_or(1) as usize;
    let limit = input
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(2000) as usize;

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
            (truncate_output(content, offset, limit), false)
        }
        Err(e) => (
            format!("Failed to read file '{}': {}", validated.display(), e),
            true,
        ),
    }
}

/// Apply offset/limit slicing, then add line numbers, then apply
/// head+tail truncation if the sliced content exceeds MAX_OUTPUT_BYTES.
///
/// Processing order:
/// 1. Split content into lines
/// 2. Slice by offset (1-indexed) and limit
/// 3. Add line numbers starting from `offset`
/// 4. Truncate if the line-numbered output exceeds MAX_OUTPUT_BYTES
fn truncate_output(content: String, offset: usize, limit: usize) -> String {
    // If offset is 1 and limit >= total lines, this is a full read —
    // use the original fast path.
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    // offset is 1-indexed; convert to 0-indexed start.
    let start = if offset == 0 {
        0 // offset=0 treated as 1 (defensive)
    } else {
        offset.saturating_sub(1)
    };

    // If start is beyond the file, return empty.
    if start >= total_lines {
        return String::new();
    }

    let end = (start + limit).min(total_lines);
    let sliced_lines = &lines[start..end];

    // If reading the full file with default params (offset=1, limit=2000
    // and file <= 2000 lines), use the original truncation path.
    if start == 0 && end == total_lines {
        return truncate_full_output(&content);
    }

    // Join sliced lines, add line numbers from `offset`, then truncate.
    let sliced_text: String = sliced_lines.join("\n");
    let numbered = add_line_numbers_with_offset(&sliced_text, offset.max(1));

    // Truncate if the numbered output exceeds MAX_OUTPUT_BYTES.
    if numbered.len() <= MAX_OUTPUT_BYTES {
        return numbered;
    }

    // Head+tail truncation on the line-numbered output. Slice at
    // UTF-8 char boundaries (RULE-E-009) — the line-number prefix
    // is ASCII, but the source lines can be multibyte.
    let head_end = numbered.floor_char_boundary(TRUNCATE_HEAD);
    let tail_start = numbered.ceil_char_boundary(numbered.len().saturating_sub(TRUNCATE_HEAD));
    let omitted = numbered.len() - MAX_OUTPUT_BYTES;
    format!(
        "{}\n<truncated: omitted {} bytes>\n{}",
        &numbered[..head_end],
        omitted,
        &numbered[tail_start..]
    )
}

/// Full-file truncation path (no offset/limit). Kept for backward
/// compatibility when reading the entire file.
///
/// `pub(crate)` so the B2 PR2 `@file` injection (`agent::at_file`) can
/// reuse the exact same 50 KB head+tail + `cat -n` line-numbering the
/// `read_file` tool produces — injected `@relpath` content and tool
/// output stay format-identical so the model does not see a difference
/// between "user-fed context" and "tool result" (opencode design cue).
pub(crate) fn truncate_full_output(content: &str) -> String {
    if content.len() <= MAX_OUTPUT_BYTES {
        return add_line_numbers(content);
    }
    // RULE-E-009: slice at a UTF-8 char boundary, never the middle
    // of a multi-byte sequence (CJK / emoji in a ≥50KB file would
    // panic on the byte slice). floor = walk back to a char start
    // (head); ceil = walk forward (tail). Mirrors the byte-walk in
    // `git::diff::build_untracked_diff`.
    let head_end = content.floor_char_boundary(TRUNCATE_HEAD);
    let tail_start = content.ceil_char_boundary(content.len() - TRUNCATE_HEAD);
    let omitted = content.len() - MAX_OUTPUT_BYTES;
    let head = add_line_numbers(&content[..head_end]);
    let tail = add_line_numbers(&content[tail_start..]);
    format!(
        "{}\n<truncated: omitted {} bytes>\n{}",
        head, omitted, tail
    )
}

/// Add `cat -n` style line numbers to `text`, starting from line 1.
fn add_line_numbers(text: &str) -> String {
    add_line_numbers_with_offset(text, 1)
}

/// Add `cat -n` style line numbers to `text`, starting from
/// `start_line`. Each output line is `<tab><line_num><tab><text>`.
fn add_line_numbers_with_offset(text: &str, start_line: usize) -> String {
    let mut out = String::with_capacity(text.len() + text.lines().count() * 8);
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push('\t');
        out.push_str(&(start_line + i).to_string());
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
            worktree_path: tmp.path().canonicalize().unwrap(),
            cwd: tmp.path().canonicalize().unwrap(),
            checklist: crate::tools::update_checklist::new_handle(),
            background_shells: crate::background_shell::default_registry(),
            db: crate::tools::test_default_pool(),
            project_id: "test-proj".to_string(),
            data_dir: tmp.path().to_path_buf(),
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
    async fn execute_reads_outside_root_file() {
        // read-side boundary decouple (2026-07-01): tool layer no longer
        // rejects project-outside paths — the permission layer gates them
        // (Tier 2.5 deny / Tier 4 allow / ask_path). /etc/hostname exists
        // and is outside the tempdir project root, so the read succeeds.
        let tmp = tempdir().unwrap();
        let (msg, is_error) = execute(
            &serde_json::json!({"path": "/etc/hostname"}),
            &test_ctx(&tmp),
            None,
            None,
        )
        .await;
        assert!(!is_error, "project-outside read must succeed (boundary moved to permission layer): {msg}");
        assert!(!msg.is_empty());
    }

    #[tokio::test]
    async fn execute_relative_traversal_not_boundary_rejected() {
        // read-side decouple: relative traversal is no longer tool-layer
        // boundary-rejected. The path may resolve to a missing file
        // (tempdir-depth dependent) → IO error is fine; a boundary error
        // ("outside project root" / "rejected") is NOT — boundary is gone
        // for read 族.
        let tmp = tempdir().unwrap();
        let (msg, _is_error) = execute(
            &serde_json::json!({"path": "../../etc/hostname"}),
            &test_ctx(&tmp),
            None,
            None,
        )
        .await;
        assert!(
            !msg.contains("outside project root") && !msg.contains("rejected"),
            "tool layer must not boundary-reject relative traversal; got: {msg}"
        );
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
        // read-side decouple (2026-07-01): tool-layer boundary gone →
        // nonexistent files now surface as a tokio IO error ("Failed to
        // read"), not a boundary "rejected".
        assert!(content.contains("Failed to read"));
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

    // --- P0: offset + limit tests ---

    /// offset=3, limit=2 on a 5-line file → lines 3-4 only, numbered 3,4.
    #[tokio::test]
    async fn offset_limit_reads_correct_range() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "line1\nline2\nline3\nline4\nline5\n").unwrap();
        let (content, is_error) = execute(
            &serde_json::json!({
                "path": tmp.path().join("a.txt").to_string_lossy(),
                "offset": 3,
                "limit": 2
            }),
            &test_ctx(&tmp),
            None,
            None,
        )
        .await;
        assert!(!is_error, "{}", content);
        assert!(content.contains("\t3\tline3"), "got: {:?}", content);
        assert!(content.contains("\t4\tline4"), "got: {:?}", content);
        assert!(!content.contains("line1"), "should not contain line1");
        assert!(!content.contains("line2"), "should not contain line2");
        assert!(!content.contains("line5"), "should not contain line5");
    }

    /// offset beyond file length → empty output (is_error: false).
    #[tokio::test]
    async fn offset_beyond_file_returns_empty() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "only one line\n").unwrap();
        let (content, is_error) = execute(
            &serde_json::json!({
                "path": tmp.path().join("a.txt").to_string_lossy(),
                "offset": 100,
                "limit": 10
            }),
            &test_ctx(&tmp),
            None,
            None,
        )
        .await;
        assert!(!is_error, "{}", content);
        assert!(content.is_empty(), "expected empty, got: {:?}", content);
    }

    /// limit extends past file end → returns up to EOF.
    #[tokio::test]
    async fn limit_beyond_eof_returns_to_end() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "a\nb\nc\n").unwrap();
        let (content, is_error) = execute(
            &serde_json::json!({
                "path": tmp.path().join("a.txt").to_string_lossy(),
                "offset": 2,
                "limit": 9999
            }),
            &test_ctx(&tmp),
            None,
            None,
        )
        .await;
        assert!(!is_error, "{}", content);
        assert!(content.contains("\t2\tb"), "got: {:?}", content);
        assert!(content.contains("\t3\tc"), "got: {:?}", content);
        assert!(!content.contains("\t1\ta"), "should not contain line 1");
    }

    /// No offset/limit → full file read, backward compatible.
    #[tokio::test]
    async fn no_offset_limit_reads_full_file() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "x\ny\nz\n").unwrap();
        let (content, is_error) = execute(
            &serde_json::json!({"path": tmp.path().join("a.txt").to_string_lossy()}),
            &test_ctx(&tmp),
            None,
            None,
        )
        .await;
        assert!(!is_error, "{}", content);
        assert!(content.contains("\t1\tx"));
        assert!(content.contains("\t2\ty"));
        assert!(content.contains("\t3\tz"));
    }

    /// ReadGuard fingerprint covers the full file even with offset/limit.
    #[tokio::test]
    async fn read_guard_covers_full_file_with_offset() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("a.txt");
        std::fs::write(&path, "line1\nline2\nline3\n").unwrap();
        let guard = ReadGuard::new();

        // Read with offset=2 — only get line2 and line3.
        let (content, is_error) = execute(
            &serde_json::json!({
                "path": path.to_string_lossy(),
                "offset": 2,
                "limit": 2
            }),
            &test_ctx(&tmp),
            Some(&guard),
            Some("s1"),
        )
        .await;
        assert!(!is_error, "{}", content);
        // Guard should still recognize the file (fingerprint from full read).
        guard.verify_read("s1", &path).await.unwrap();
    }

    /// add_line_numbers_with_offset starts numbering from the given offset.
    #[test]
    fn add_line_numbers_with_offset_works() {
        let out = add_line_numbers_with_offset("alpha\nbeta", 10);
        assert_eq!(out, "\t10\talpha\n\t11\tbeta");
    }

    /// offset=0 is treated as offset=1 (defensive).
    #[tokio::test]
    async fn offset_zero_treated_as_one() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "first\nsecond\n").unwrap();
        let (content, is_error) = execute(
            &serde_json::json!({
                "path": tmp.path().join("a.txt").to_string_lossy(),
                "offset": 0,
                "limit": 1
            }),
            &test_ctx(&tmp),
            None,
            None,
        )
        .await;
        assert!(!is_error, "{}", content);
        assert!(content.contains("\t1\tfirst"), "got: {:?}", content);
        assert!(!content.contains("second"), "limit=1 should only return 1 line");
    }

    /// RULE-E-009: truncating a >50KB multibyte (CJK) file must not
    /// panic on a half-character byte boundary. Pre-fix,
    /// `&content[..TRUNCATE_HEAD]` split a 3-byte sequence and
    /// panicked.
    #[test]
    fn truncate_full_output_multibyte_no_panic() {
        // 72 KB of a single CJK glyph (3 bytes/char) — the 25 KB
        // head end lands mid-character without floor_char_boundary.
        let content = "中".repeat(24_000);
        let out = truncate_full_output(&content);
        assert!(
            out.contains("<truncated:"),
            "should truncate, got len {}",
            out.len()
        );
    }

    /// RULE-E-009: the offset/limit numbered-output truncation path
    /// must also slice at char boundaries (the prefix is ASCII but
    /// source lines can be multibyte).
    #[test]
    fn truncate_output_offset_multibyte_no_panic() {
        // ~1.2 KB/line of CJK; 100 lines. offset=2 forces the
        // numbered (non-full) path; 99 numbered lines ≈ 119 KB > 50 KB.
        let line: String = "中".repeat(400);
        let content = format!("{}\n", line).repeat(100);
        let out = truncate_output(content, 2, 100);
        assert!(
            out.contains("<truncated:"),
            "should truncate, got len {}",
            out.len()
        );
    }
}
