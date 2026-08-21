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

use crate::attachments;
use crate::llm::types::{AttachmentRef, ToolDef};
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
             the real file line numbers, not relative to offset.\n\n\
             Image files (png / jpg / webp, ≤5MB) are returned as an image block you \
             can actually SEE when the model supports vision — use this to inspect \
             screenshots and diagrams instead of asking the user what they contain."
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

/// Execute the tool. Returns `(content, is_error, images)`.
///
/// `guard` and `session_id` are optional: when both are present, the
/// read is recorded in the guard (so a follow-up `edit_file` can
/// verify freshness). When either is `None`, the read still works
/// but the guard is not updated — the agent loop in `lib.rs::chat`
/// always supplies both.
///
/// R3 (08-21-b1-image-followups): png/jpg/webp files (extension +
/// magic number) are NOT read as text — the bytes are copied into
/// the session attachments dir and returned as an `AttachmentRef`
/// riding the ToolResult (the wire layer delivers it as an image
/// block on vision models; caps/protocol degradation is the wire
/// layer's job, same as user-pasted images). Oversized (>5 MiB)
/// images return a notice instead (`is_error: false` — the file is
/// readable, just too large to attach).
pub async fn execute(
    input: &serde_json::Value,
    ctx: &ToolContext,
    guard: Option<&ReadGuard>,
    session_id: Option<&str>,
) -> (String, bool, Option<Vec<AttachmentRef>>) {
    let raw_path = match input.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return ("Missing required parameter: path".to_string(), true, None),
    };

    // 1. Resolve (with `~` home expansion; see boundary::resolve_path).
    let requested: std::path::PathBuf = crate::projects::boundary::resolve_path(raw_path, &ctx.cwd);

    // 2. read-side boundary decouple (2026-07-01): tool-layer
    //    assert_within_root removed for read 族 — project-outside
    //    reads are gated by the permission layer (Tier 2.5 sensitive
    //    deny-list + Tier 4 trusted allow-list + ask_path).
    //    assert_within_root stays for write_file/edit_file.
    let validated = requested;

    // R3: image arm. Extension whitelist first (cheap), then magic
    // number on the bytes — a "pic.png" that isn't a real image
    // falls through to the UTF-8 path below and surfaces the usual
    // read error instead of poisoning the provider request.
    if let Some(media_type) = attachments::image_media_type_for_path(&validated) {
        return read_image_arm(&validated, media_type, ctx, session_id).await;
    }

    // 3. Parse offset and limit parameters.
    let offset = input.get("offset").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
    let limit = input.get("limit").and_then(|v| v.as_u64()).unwrap_or(2000) as usize;

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
            (truncate_output(content, offset, limit), false, None)
        }
        Err(e) => (
            format!("Failed to read file '{}': {}", validated.display(), e),
            true,
            None,
        ),
    }
}

/// R3 image arm: magic-check, cap, copy into attachments, estimate
/// tokens via the header probe (`imagesize`, never a pixel decode —
/// same as the @-file path). Requires `session_id`; without one the
/// image can't be persisted so we return a notice instead of a
/// text-garbled error.
async fn read_image_arm(
    path: &std::path::Path,
    media_type: &str,
    ctx: &ToolContext,
    session_id: Option<&str>,
) -> (String, bool, Option<Vec<AttachmentRef>>) {
    let bytes = match tokio::fs::read(path).await {
        Ok(b) => b,
        Err(e) => {
            return (
                format!("Failed to read file '{}': {}", path.display(), e),
                true,
                None,
            )
        }
    };
    // Magic-number sanity (same defense as the @-file path): a
    // "pic.png" whose bytes aren't a real image must not reach the
    // provider as an image block — report the mismatch instead.
    if !attachments::image_magic_matches(bytes.as_slice(), media_type) {
        return (
            format!(
                "Failed to read file '{}': file is not a valid {} image (magic mismatch)",
                path.display(),
                media_type
            ),
            true,
            None,
        );
    }
    if bytes.len() > attachments::MAX_IMAGE_BYTES {
        return (
            format!(
                "[image: {} — 超过 5MB 上限未读取；请压缩或转换后重试]",
                path.display()
            ),
            false,
            None,
        );
    }
    let Some(sid) = session_id else {
        return (
            format!(
                "[image: {} — 当前上下文无法保存附件，未读取]",
                path.display()
            ),
            false,
            None,
        );
    };
    let file = match attachments::save_image(&ctx.data_dir, sid, media_type, &bytes).await {
        Ok(f) => f,
        Err(e) => {
            return (
                format!("[image: {} — 附件保存失败，未发送: {}]", path.display(), e),
                false,
                None,
            )
        }
    };
    let dims = imagesize::blob_size(bytes.as_slice()).ok();
    let tokens_est = dims.map(|d| (d.width as u64 * d.height as u64 / 750) as u32);
    let dims_note = match dims {
        Some(d) => format!(" ({}×{})", d.width, d.height),
        None => String::new(),
    };
    (
        format!(
            "[image: {}{} — 已作为图片块发送]",
            path.display(),
            dims_note
        ),
        false,
        Some(vec![AttachmentRef {
            file,
            media_type: media_type.to_string(),
            source: "read_file".to_string(),
            tokens_est,
        }]),
    )
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
    format!("{}\n<truncated: omitted {} bytes>\n{}", head, omitted, tail)
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
            workflow_name: None,
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
        let (content, is_error, _images) = execute(
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
        let (content, is_error, _images) = execute(
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
        let (content, is_error, _images) = execute(
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
        let (content, is_error, _images) = execute(
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
        let (content, is_error, _images) = execute(
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
        let (msg, is_error, _images) = execute(
            &serde_json::json!({"path": "/etc/hostname"}),
            &test_ctx(&tmp),
            None,
            None,
        )
        .await;
        assert!(
            !is_error,
            "project-outside read must succeed (boundary moved to permission layer): {msg}"
        );
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
        let (msg, _is_error, _images) = execute(
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
        let (content, is_error, _images) =
            execute(&serde_json::json!({}), &test_ctx(&tmp), None, None).await;
        assert!(is_error);
        assert!(content.contains("Missing"));
    }

    #[tokio::test]
    async fn execute_nonexistent_file() {
        let tmp = tempdir().unwrap();
        let (content, is_error, _images) = execute(
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
        let (content, is_error, _images) = execute(
            &serde_json::json!({"path": tmp.path().join("a.txt").to_string_lossy()}),
            &test_ctx(&tmp),
            Some(&guard),
            Some("s1"),
        )
        .await;
        assert!(!is_error, "{}", content);
        // The guard should now know about this path.
        guard
            .verify_read("s1", &tmp.path().join("a.txt"))
            .await
            .unwrap();
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
        std::fs::write(
            tmp.path().join("a.txt"),
            "line1\nline2\nline3\nline4\nline5\n",
        )
        .unwrap();
        let (content, is_error, _images) = execute(
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
        let (content, is_error, _images) = execute(
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
        let (content, is_error, _images) = execute(
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
        let (content, is_error, _images) = execute(
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
        let (content, is_error, _images) = execute(
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
        let (content, is_error, _images) = execute(
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
        assert!(
            !content.contains("second"),
            "limit=1 should only return 1 line"
        );
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

    // ------------------------------------------------------------------
    // R3 (08-21-b1-image-followups): image arm — 五态。
    // ------------------------------------------------------------------

    /// 标准 1x1 透明 PNG(魔数 + IHDR 可被 imagesize 头探测)。
    fn one_px_png() -> Vec<u8> {
        hex_literal("89504e470d0a1a0a0000000d4948445200000001000000010806000000")
            .into_iter()
            .chain(hex_literal("1f15c4890000000a49444154789c6300010000050001"))
            .chain(hex_literal("0d0a2db40000000049454e44ae426082"))
            .collect()
    }

    fn hex_literal(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// 白名单命中:魔数过、≤5MiB → 副本落盘 + tokens_est + 图片块文本行。
    #[tokio::test]
    async fn image_file_returns_attachment_ref() {
        let tmp = tempdir().unwrap();
        let png = one_px_png();
        std::fs::write(tmp.path().join("shot.png"), &png).unwrap();
        let (content, is_error, images) = execute(
            &serde_json::json!({"path": tmp.path().join("shot.png").to_string_lossy()}),
            &test_ctx(&tmp),
            None,
            Some("sessImgRead1"),
        )
        .await;
        assert!(!is_error, "{content}");
        assert!(content.contains("[image:"), "{content}");
        assert!(content.contains("(1×1)"), "{content}");
        let imgs = images.expect("image arm must return refs");
        assert_eq!(imgs.len(), 1);
        assert_eq!(imgs[0].media_type, "image/png");
        assert_eq!(imgs[0].source, "read_file");
        // tokens_est = (1×1)/750 → 0(维度可读但积为零,允许 0)。
        assert_eq!(imgs[0].tokens_est, Some(0));
        // 副本真的落盘。
        let dir = tmp.path().join("attachments").join("sessImgRead1");
        assert!(dir.exists(), "attachment copy must exist");
    }

    /// 魔数不符(文本改名 .png)→ is_error,提示 magic mismatch(不产图)。
    #[tokio::test]
    async fn mislabeled_image_magic_mismatch_errors() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("fake.png"), "just text, not a png").unwrap();
        let (content, is_error, images) = execute(
            &serde_json::json!({"path": tmp.path().join("fake.png").to_string_lossy()}),
            &test_ctx(&tmp),
            None,
            Some("sessImgRead2"),
        )
        .await;
        assert!(is_error, "{content}");
        assert!(content.contains("magic mismatch"), "{content}");
        assert!(images.is_none());
    }

    /// >5MiB:非 error 占位提示,不落盘不产图。
    #[tokio::test]
    async fn oversize_image_returns_notice_not_error() {
        let tmp = tempdir().unwrap();
        let mut png = one_px_png();
        png.resize(crate::attachments::MAX_IMAGE_BYTES + 1, 0);
        std::fs::write(tmp.path().join("big.png"), &png).unwrap();
        let (content, is_error, images) = execute(
            &serde_json::json!({"path": tmp.path().join("big.png").to_string_lossy()}),
            &test_ctx(&tmp),
            None,
            Some("sessImgRead3"),
        )
        .await;
        assert!(!is_error, "{content}");
        assert!(content.contains("超过 5MB"), "{content}");
        assert!(images.is_none());
    }

    /// 扩展名非图:走既有 UTF-8 老路(cat -n 正常输出)。
    #[tokio::test]
    async fn non_image_extension_takes_text_path() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("notes.md"), "hello").unwrap();
        let (content, is_error, images) = execute(
            &serde_json::json!({"path": tmp.path().join("notes.md").to_string_lossy()}),
            &test_ctx(&tmp),
            None,
            Some("sessImgRead4"),
        )
        .await;
        assert!(!is_error);
        assert!(content.starts_with("\t1\thello"), "{content}");
        assert!(images.is_none());
    }

    /// 无 session_id:图片无法落盘 → 非 error 占位(不产 ref)。
    #[tokio::test]
    async fn image_without_session_id_degrades() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("shot.png"), one_px_png()).unwrap();
        let (content, is_error, images) = execute(
            &serde_json::json!({"path": tmp.path().join("shot.png").to_string_lossy()}),
            &test_ctx(&tmp),
            None,
            None,
        )
        .await;
        assert!(!is_error, "{content}");
        assert!(content.contains("无法保存附件"), "{content}");
        assert!(images.is_none());
    }
}
