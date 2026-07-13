//! Hand-written unified-diff parser + hunk applier (B9+ D4, 2026-07-13).
//!
//! Pure functions, no I/O. The IPC handler
//! `commands::ui::apply_ui_diff` reads the file, calls
//! [`apply_to_file`], and writes back. This module owns only the
//! structural / textual transformation.
//!
//! # Zero new dependency
//!
//! Per TECH §1.4 "零新增依赖" we deliberately do NOT pull in the `diffy`
//! crate. The algorithm is small enough (~250 LOC) and 100% covered by
//! unit tests. Project convention is "hand-roll the primitive we
//! control" (mirrors `read_file::cat_n_format`, `llm::sse` hand-written
//! state machine, etc.).
//!
//! # MVP 能力边界
//!
//! - ✅ Standard unified diff, multi-file, multi-hunk
//! - ✅ `@@` line number + context-line verification for positioning
//! - ✅ Conflict fail-fast (context mismatch = whole diff fails, no
//!   partial writes)
//! - ❌ Binary diff / new empty file / rename / mode change
//! - ❌ LLM-style headerless `+/-` fragments (DiffPrimitive already
//!   disables the apply button on those)
//!
//! # Line number semantics
//!
//! `@@ -oldStart,oldLines +newStart,newLines @@` — `oldStart` is the
//! 1-indexed line in the **original** file, not shifted by previous
//! hunks. Apply-time we track `cumulative_offset =
//! Σ (newLines - oldLines)` over previously applied hunks; in the
//! modified buffer the hunk starts at
//! `oldStart - 1 + cumulative_offset`.
//!
//! # Path cleaning
//!
//! Standard unified-diff headers look like `--- a/path` / `+++ b/path`
//! (the `a/` / `b/` prefix is git convention). [`parse_unified_diff`]
//! strips that prefix; leading / trailing `"` quotes (also legal) are
//! stripped. The frontend's `DiffPrimitive.cleanPath` does the same
//! cleanup for display; we mirror it here so the path that reaches
//! `assert_within_root` is the canonical-form path.

use std::fmt;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// One file's worth of hunks. `path` is post-cleanup (no `a/` / `b/`
/// prefix, no surrounding quotes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePatch {
    pub path: String,
    pub hunks: Vec<Hunk>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// 1-indexed line in the ORIGINAL file where this hunk starts.
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    pub lines: Vec<LineOp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineOp {
    pub kind: LineKind,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Remove,
    Add,
}

/// Per-file apply stats. Returned alongside the new content by
/// [`apply_to_file`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileStats {
    pub added: usize,
    pub removed: usize,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Failure modes for [`parse_unified_diff`]. Carries the offending line
/// number (1-indexed) + the raw line text so the IPC handler can
/// surface a precise error message back to the LLM / UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// Empty input (whitespace-only).
    Empty,
    /// No `--- a/path` / `+++ b/path` headers at all — the input is an
    /// LLM-style `+/-` fragment without diff structure, which the
    /// DiffPrimitive apply button already disables. Surfaces as
    /// `kind = "parse"` in the IPC response.
    MissingHeader,
    /// `@@` hunk header is malformed (couldn't parse `-A,B +C,D` form).
    /// Surfaces as `kind = "parse"`.
    InvalidHunk { line: usize, text: String },
    /// `---` / `+++` headers don't agree (one or both missing).
    IncompleteHeader { line: usize, text: String },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "diff is empty"),
            Self::MissingHeader => write!(
                f,
                "diff has no `---` / `+++` headers; only standard unified diffs can be applied"
            ),
            Self::InvalidHunk { line, text } => {
                write!(f, "invalid hunk header at line {}: {}", line, text)
            }
            Self::IncompleteHeader { line, text } => {
                write!(f, "incomplete header at line {}: {}", line, text)
            }
        }
    }
}

impl std::error::Error for ParseError {}

/// Failure modes for [`apply_to_file`]. Conflict is the only realistic
/// one — a hunk's context lines didn't match the file content at the
/// expected position (file changed on disk between diff generation and
/// apply, or diff is wrong). Per design §2.3, the IPC handler treats
/// ANY ApplyError as `kind = "conflict"` and fail-fasts: no writes
/// happen at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyError {
    /// Hunk's expected context lines didn't match the file at the
    /// computed position.
    Conflict { detail: String },
    /// Hunk header's `oldStart` + cumulative offset puts the hunk past
    /// EOF (rare — implies a malformed diff).
    OutOfRange { line: usize },
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict { detail } => write!(f, "context mismatch: {}", detail),
            Self::OutOfRange { line } => write!(f, "hunk at line {} is past EOF", line),
        }
    }
}

impl std::error::Error for ApplyError {}

// ---------------------------------------------------------------------------
// Path cleaning (mirrors DiffPrimitive.cleanPath)
// ---------------------------------------------------------------------------

/// Strip `a/` / `b/` prefix and surrounding `"` quotes (the two forms
/// git's unified-diff header uses).
fn clean_path(raw: &str) -> String {
    let trimmed = raw.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(trimmed);
    unquoted
        .strip_prefix("a/")
        .or_else(|| unquoted.strip_prefix("b/"))
        .map(|s| s.to_string())
        .unwrap_or_else(|| unquoted.to_string())
}

// ---------------------------------------------------------------------------
// Parse
// ---------------------------------------------------------------------------

/// Parse a unified-diff blob into a list of [`FilePatch`]es.
///
/// MVP: only standard unified diffs (with `--- a/` / `+++ b/` / `@@`
/// headers). LLM-style headerless `+/-` fragments return
/// [`ParseError::MissingHeader`] — the DiffPrimitive apply button
/// already disables for those, so this is defense-in-depth.
///
/// `\ No newline at end of file` markers (legal in GNU unified diff)
/// are silently consumed.
///
/// # Multi-hunk-per-file (B9+ D4 regression 2026-07-13)
///
/// Hunks for the SAME file all land in ONE `FilePatch` (hunks appended
/// in encounter order). This is load-bearing for
/// [`apply_to_file`]'s `cumulative_offset` logic — it expects all
/// hunks of a file in a single `FilePatch` so the line-position math
/// composes correctly across hunks. Splitting them into N separate
/// `FilePatch`es with the same path would silently corrupt files on
/// apply (each `FilePatch` would re-read the ORIGINAL file + apply
/// only its single hunk; the IPC's write loop would then have the
/// last write win, dropping earlier hunks' changes). Regression:
/// `parse_then_apply_multi_hunk_same_file`.
pub fn parse_unified_diff(text: &str) -> Result<Vec<FilePatch>, ParseError> {
    if text.trim().is_empty() {
        return Err(ParseError::Empty);
    }

    let mut patches: Vec<FilePatch> = Vec::new();
    // Builder for the file we're currently parsing.
    let mut pending_old_path: Option<String> = None;
    let mut pending_new_path: Option<String> = None;
    let mut current_hunk: Option<Hunk> = None;

    for (idx, raw_line) in text.lines().enumerate() {
        let line_no = idx + 1;

        if let Some(rest) = raw_line.strip_prefix("--- ") {
            // Finalize any in-progress hunk for the PREVIOUS file
            // before starting a new file. Does NOT reset pending paths
            // — that happens via the assignment below.
            push_or_merge_hunk(
                &mut patches,
                &current_hunk.take(),
                &pending_new_path,
                &pending_old_path,
            );
            pending_old_path = Some(rest.trim().to_string());
            pending_new_path = None;
        } else if let Some(rest) = raw_line.strip_prefix("+++ ") {
            if pending_old_path.is_none() {
                return Err(ParseError::IncompleteHeader {
                    line: line_no,
                    text: raw_line.to_string(),
                });
            }
            pending_new_path = Some(rest.trim().to_string());
        } else if let Some(rest) = raw_line.strip_prefix("@@") {
            if pending_old_path.is_none() || pending_new_path.is_none() {
                return Err(ParseError::IncompleteHeader {
                    line: line_no,
                    text: raw_line.to_string(),
                });
            }
            // Close previous hunk (if any) by appending it to its
            // file's FilePatch. Does NOT reset pending paths —
            // subsequent `@@` for the same file must keep landing in
            // the same FilePatch (see "Multi-hunk-per-file" above).
            push_or_merge_hunk(
                &mut patches,
                &current_hunk.take(),
                &pending_new_path,
                &pending_old_path,
            );
            current_hunk = Some(parse_hunk_header(rest, line_no, raw_line)?);
        } else if let Some(rest) = raw_line.strip_prefix('+') {
            // Added line.
            let h = current_hunk.as_mut().ok_or(ParseError::IncompleteHeader {
                line: line_no,
                text: raw_line.to_string(),
            })?;
            h.lines.push(LineOp {
                kind: LineKind::Add,
                content: rest.to_string(),
            });
        } else if let Some(rest) = raw_line.strip_prefix('-') {
            // Removed line.
            let h = current_hunk.as_mut().ok_or(ParseError::IncompleteHeader {
                line: line_no,
                text: raw_line.to_string(),
            })?;
            h.lines.push(LineOp {
                kind: LineKind::Remove,
                content: rest.to_string(),
            });
        } else if raw_line.starts_with("\\ No newline at end of file") {
            // GNU marker; ignore.
        } else {
            // Context line — starts with ' ' (or is exactly "" for empty
            // context lines inside a hunk).
            let h = current_hunk.as_mut().ok_or(ParseError::IncompleteHeader {
                line: line_no,
                text: raw_line.to_string(),
            })?;
            let content = raw_line.strip_prefix(' ').unwrap_or(raw_line);
            h.lines.push(LineOp {
                kind: LineKind::Context,
                content: content.to_string(),
            });
        }
    }

    // Finalize trailing hunk.
    push_or_merge_hunk(
        &mut patches,
        &current_hunk.take(),
        &pending_new_path,
        &pending_old_path,
    );

    if patches.is_empty() {
        return Err(ParseError::MissingHeader);
    }
    Ok(patches)
}

/// Append `hunk` (if `Some`) to the [`FilePatch`] in `patches` whose
/// cleaned path matches; create a new `FilePatch` if none exists yet.
///
/// Path resolution mirrors the original single-push logic: `+++ b/path`
/// wins over `--- a/path` (the new path represents the file as it
/// exists post-apply). The `a/` / `b/` prefix and surrounding `"`
/// quotes are stripped via [`clean_path`].
///
/// This helper is the heart of multi-hunk-per-file support: it
/// guarantees the parser emits ONE `FilePatch` per file path even when
/// the diff has multiple `@@` hunks for that file (see
/// `parse_unified_diff`'s "Multi-hunk-per-file" docstring).
fn push_or_merge_hunk(
    patches: &mut Vec<FilePatch>,
    hunk: &Option<Hunk>,
    pending_new_path: &Option<String>,
    pending_old_path: &Option<String>,
) {
    let Some(h) = hunk else {
        return;
    };
    let raw_path = pending_new_path
        .clone()
        .or_else(|| pending_old_path.clone())
        .unwrap_or_default();
    let cleaned = clean_path(&raw_path);
    if let Some(existing) = patches.iter_mut().find(|p| p.path == cleaned) {
        existing.hunks.push(h.clone());
    } else {
        patches.push(FilePatch {
            path: cleaned,
            hunks: vec![h.clone()],
        });
    }
}

/// Parse the `@@ -A,B +C,D @@` portion (the part AFTER the leading
/// `@@`). Returns a fresh `Hunk` with `lines` empty.
fn parse_hunk_header(rest: &str, line_no: usize, raw_line: &str) -> Result<Hunk, ParseError> {
    // Strip leading whitespace, then split at the trailing `@@`.
    let trimmed = rest.trim_start();
    let (numbers, _trailing) = trimmed.split_once("@@").ok_or(ParseError::InvalidHunk {
        line: line_no,
        text: raw_line.to_string(),
    })?;
    let mut parts = numbers.split_whitespace();
    let old_part =
        parts
            .next()
            .and_then(|p| p.strip_prefix('-'))
            .ok_or(ParseError::InvalidHunk {
                line: line_no,
                text: raw_line.to_string(),
            })?;
    let new_part =
        parts
            .next()
            .and_then(|p| p.strip_prefix('+'))
            .ok_or(ParseError::InvalidHunk {
                line: line_no,
                text: raw_line.to_string(),
            })?;
    let (old_start, old_lines) = parse_hunk_range(old_part).ok_or(ParseError::InvalidHunk {
        line: line_no,
        text: raw_line.to_string(),
    })?;
    let (new_start, new_lines) = parse_hunk_range(new_part).ok_or(ParseError::InvalidHunk {
        line: line_no,
        text: raw_line.to_string(),
    })?;
    Ok(Hunk {
        old_start,
        old_lines,
        new_start,
        new_lines,
        lines: Vec::new(),
    })
}

/// Parse `42` (start, count=1) or `42,7` (start, count=7).
fn parse_hunk_range(s: &str) -> Option<(usize, usize)> {
    if let Some((start, lines)) = s.split_once(',') {
        Some((start.parse().ok()?, lines.parse().ok()?))
    } else {
        Some((s.parse().ok()?, 1))
    }
}

// ---------------------------------------------------------------------------
// Apply
// ---------------------------------------------------------------------------

/// Apply a parsed [`FilePatch`] to a string of file content. Returns the
/// new content + [`FileStats`].
///
/// # Algorithm
///
/// For each hunk, in order:
/// 1. Compute the start position in the (currently modified) buffer:
///    `cumulative_offset = Σ (newLines - oldLines)` of previously
///    applied hunks. Position = `oldStart - 1 + cumulative_offset`.
/// 2. Walk the hunk's `lines` and build `expected_old` (Context + Remove)
///    and `expected_new` (Context + Add).
/// 3. Verify `expected_old` matches `buffer[start..start + expected_old.len()]`
///    exactly. Mismatch → [`ApplyError::Conflict`].
/// 4. `splice(start..start + old_count, new_count)` to replace in place.
/// 5. Update `cumulative_offset += new_count - old_count`.
///
/// Returns the joined buffer + per-hunk `added` / `removed` counts.
pub fn apply_to_file(patch: &FilePatch, current: &str) -> Result<(String, FileStats), ApplyError> {
    // Split on '\n'. Trailing newline → trailing empty string;
    // empty file → [""]. Join with '\n' reconstitutes the original
    // (modulo CRLF, which we don't handle in MVP).
    let mut lines: Vec<String> = current.split('\n').map(|s| s.to_string()).collect();

    let mut cumulative_offset: i64 = 0;
    let mut total_added = 0usize;
    let mut total_removed = 0usize;

    for hunk in &patch.hunks {
        // Compute start position in the MODIFIED buffer.
        let start_in_modified = (hunk.old_start as i64 - 1) + cumulative_offset;
        if start_in_modified < 0 || start_in_modified as usize > lines.len() {
            return Err(ApplyError::OutOfRange {
                line: hunk.old_start,
            });
        }
        let start = start_in_modified as usize;

        // Build expected_old (Context + Remove) and new (Context + Add).
        let mut expected_old: Vec<&str> = Vec::new();
        let mut expected_new: Vec<&str> = Vec::new();
        for op in &hunk.lines {
            match op.kind {
                LineKind::Context => {
                    expected_old.push(&op.content);
                    expected_new.push(&op.content);
                }
                LineKind::Remove => {
                    expected_old.push(&op.content);
                    total_removed += 1;
                }
                LineKind::Add => {
                    expected_new.push(&op.content);
                    total_added += 1;
                }
            }
        }

        // Verify expected_old matches lines[start..start+expected_old.len()].
        if start + expected_old.len() > lines.len() {
            return Err(ApplyError::Conflict {
                detail: format!(
                    "hunk at line {} extends past EOF (need {} lines, have {})",
                    hunk.old_start,
                    expected_old.len(),
                    lines.len().saturating_sub(start)
                ),
            });
        }
        for (i, expected) in expected_old.iter().enumerate() {
            let actual = &lines[start + i];
            if actual != expected {
                return Err(ApplyError::Conflict {
                    detail: format!(
                        "hunk at line {}: context line {} mismatch (expected `{}`, got `{}`)",
                        hunk.old_start,
                        start + i + 1,
                        expected,
                        actual
                    ),
                });
            }
        }

        // Apply: replace lines[start..start + old_count] with new_count.
        let old_count = expected_old.len();
        let new_count = expected_new.len();
        let new_owned: Vec<String> = expected_new.iter().map(|s| s.to_string()).collect();
        lines.splice(start..start + old_count, new_owned);

        // Update offset for subsequent hunks.
        cumulative_offset += new_count as i64 - old_count as i64;
    }

    let new_content = lines.join("\n");
    Ok((
        new_content,
        FileStats {
            added: total_added,
            removed: total_removed,
        },
    ))
}

// ---------------------------------------------------------------------------
// Tests — parser
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- path cleaning ----

    #[test]
    fn clean_path_strips_a_prefix() {
        assert_eq!(clean_path("a/foo.rs"), "foo.rs");
    }

    #[test]
    fn clean_path_strips_b_prefix() {
        assert_eq!(clean_path("b/src/lib.rs"), "src/lib.rs");
    }

    #[test]
    fn clean_path_strips_quotes() {
        assert_eq!(clean_path("\"a/foo.rs\""), "foo.rs");
    }

    #[test]
    fn clean_path_keeps_unprefixed() {
        assert_eq!(clean_path("foo.rs"), "foo.rs");
        assert_eq!(clean_path("src/foo.rs"), "src/foo.rs");
    }

    // ---- parse: happy paths ----

    #[test]
    fn parse_single_file_single_hunk() {
        let diff =
            "--- a/foo.rs\n+++ b/foo.rs\n@@ -1,3 +1,3 @@\n line1\n-line2\n+line2-changed\n line3\n";
        let patches = parse_unified_diff(diff).expect("parses");
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].path, "foo.rs");
        assert_eq!(patches[0].hunks.len(), 1);
        let h = &patches[0].hunks[0];
        assert_eq!(h.old_start, 1);
        assert_eq!(h.old_lines, 3);
        assert_eq!(h.new_start, 1);
        assert_eq!(h.new_lines, 3);
        assert_eq!(h.lines.len(), 4);
        assert_eq!(h.lines[0].kind, LineKind::Context);
        assert_eq!(h.lines[0].content, "line1");
        assert_eq!(h.lines[1].kind, LineKind::Remove);
        assert_eq!(h.lines[2].kind, LineKind::Add);
        assert_eq!(h.lines[3].kind, LineKind::Context);
    }

    #[test]
    fn parse_multi_file() {
        let diff = "\
--- a/foo.rs
+++ b/foo.rs
@@ -1,1 +1,1 @@
-old
+new
--- a/bar.rs
+++ b/bar.rs
@@ -1,1 +1,1 @@
-x
+y
";
        let patches = parse_unified_diff(diff).expect("parses");
        assert_eq!(patches.len(), 2);
        assert_eq!(patches[0].path, "foo.rs");
        assert_eq!(patches[1].path, "bar.rs");
    }

    #[test]
    fn parse_multi_hunk_per_file() {
        // B9+ D4 regression (2026-07-13): two hunks for the same file
        // MUST land in ONE FilePatch (hunks appended in encounter
        // order). Splitting them into N separate FilePatches with the
        // same path silently corrupts files on apply — the IPC's write
        // loop would have the last write win, dropping earlier hunks'
        // changes. See `parse_then_apply_multi_hunk_same_file` for the
        // parser→apply end-to-end guard.
        let diff = "\
--- a/foo.rs
+++ b/foo.rs
@@ -1,1 +1,1 @@
-a
+A
@@ -10,1 +10,1 @@
-b
+B
";
        let patches = parse_unified_diff(diff).expect("parses");
        assert_eq!(
            patches.len(),
            1,
            "two hunks for one file → ONE FilePatch (multi-hunk merge)"
        );
        assert_eq!(patches[0].path, "foo.rs");
        assert_eq!(
            patches[0].hunks.len(),
            2,
            "both hunks land in the same FilePatch"
        );
        assert_eq!(patches[0].hunks[0].old_start, 1);
        assert_eq!(patches[0].hunks[1].old_start, 10);
    }

    /// B9+ D4 regression (2026-07-13, task `07-13-b9plus-generative-ui-
    /// followup`): parser→apply end-to-end with two hunks for the same
    /// file. BOTH hunks must take effect on the resulting content.
    ///
    /// This is the direct regression for the silent-corruption P0 that
    /// slipped through the initial implementation: the parser used to
    /// emit N separate `FilePatch`es for N same-file hunks, and the
    /// IPC's read-each-patch-against-original + write-each-prepared
    /// loop would let the last write win (earlier hunks' changes
    /// dropped). The fix merges same-path hunks into one FilePatch so
    /// `apply_to_file`'s `cumulative_offset` logic runs over all
    /// hunks in one pass.
    #[test]
    fn parse_then_apply_multi_hunk_same_file() {
        // Two hunks for foo.rs: line 1 swaps a→A, line 3 swaps c→C.
        // After apply, BOTH lines must reflect the change. Pre-fix the
        // second hunk would clobber the first (last-write-wins on the
        // original content).
        let diff = "\
--- a/foo.rs
+++ b/foo.rs
@@ -1,1 +1,1 @@
-a
+A
@@ -3,1 +3,1 @@
-c
+C
";
        let patches = parse_unified_diff(diff).expect("parses");
        assert_eq!(patches.len(), 1, "parser merges same-path hunks");
        assert_eq!(patches[0].hunks.len(), 2);
        let original = "a\nb\nc\nd\n";
        let (new, stats) = apply_to_file(&patches[0], original).expect("applies");
        assert_eq!(
            new, "A\nb\nC\nd\n",
            "BOTH hunks must take effect (no silent drop)"
        );
        assert_eq!(stats.added, 2);
        assert_eq!(stats.removed, 2);
    }

    #[test]
    fn parse_short_hunk_header_no_count_defaults_to_one() {
        // `@@ -5 +5 @@` means 1 line old, 1 line new.
        let diff = "--- a/x\n+++ b/x\n@@ -5 +5 @@\n-a\n+b\n";
        let patches = parse_unified_diff(diff).expect("parses");
        assert_eq!(patches[0].hunks[0].old_lines, 1);
        assert_eq!(patches[0].hunks[0].new_lines, 1);
    }

    #[test]
    fn parse_handles_no_newline_marker() {
        let diff = "--- a/x\n+++ b/x\n@@ -1,1 +1,1 @@\n-old\n+new\n\\ No newline at end of file\n";
        let patches = parse_unified_diff(diff).expect("parses");
        assert_eq!(patches[0].hunks[0].lines.len(), 2);
        assert_eq!(patches[0].hunks[0].lines[0].kind, LineKind::Remove);
    }

    #[test]
    fn parse_empty_context_line_stays_context() {
        // A blank line inside a hunk is a context line with empty content.
        let diff = "--- a/x\n+++ b/x\n@@ -1,3 +1,3 @@\n a\n\n b\n";
        let patches = parse_unified_diff(diff).expect("parses");
        let h = &patches[0].hunks[0];
        assert_eq!(h.lines.len(), 3);
        assert_eq!(h.lines[0].kind, LineKind::Context);
        assert_eq!(h.lines[0].content, "a");
        assert_eq!(h.lines[1].kind, LineKind::Context);
        assert_eq!(h.lines[1].content, "");
        assert_eq!(h.lines[2].kind, LineKind::Context);
        assert_eq!(h.lines[2].content, "b");
    }

    // ---- parse: errors ----

    #[test]
    fn parse_empty_returns_empty() {
        assert_eq!(parse_unified_diff(""), Err(ParseError::Empty));
        assert_eq!(parse_unified_diff("   \n  \n"), Err(ParseError::Empty));
    }

    #[test]
    fn parse_missing_headers_returns_parse_error() {
        // LLM-style +/- fragment without `---` / `+++` headers.
        // The parser sees the leading `-old` line and tries to
        // treat it as a removed hunk line, but there's no hunk in
        // progress → `IncompleteHeader`. Either way the IPC handler
        // maps the error to `kind="parse"` (see
        // `commands::ui::apply_ui_diff`). The frontend message is
        // generic ("无法解析 diff"), so the exact ParseError variant
        // doesn't matter for UX.
        let llm_style = "-old\n+new\n";
        let err = parse_unified_diff(llm_style).expect_err("must fail");
        assert!(matches!(
            err,
            ParseError::IncompleteHeader { .. } | ParseError::MissingHeader
        ));
    }

    #[test]
    fn parse_invalid_hunk_header() {
        let diff = "--- a/x\n+++ b/x\n@@ this is not a valid header @@\n";
        match parse_unified_diff(diff) {
            Err(ParseError::InvalidHunk { line, .. }) => assert_eq!(line, 3),
            other => panic!("expected InvalidHunk, got {:?}", other),
        }
    }

    #[test]
    fn parse_plus_without_old_new_header() {
        // `+++ b/x` without preceding `---` — incomplete header.
        let diff = "+++ b/x\n@@ -1 +1 @@\n";
        assert!(matches!(
            parse_unified_diff(diff),
            Err(ParseError::IncompleteHeader { .. })
        ));
    }

    // ---- apply: happy paths ----

    #[test]
    fn apply_simple_replacement() {
        let patch = FilePatch {
            path: "foo.rs".into(),
            hunks: vec![Hunk {
                old_start: 1,
                old_lines: 3,
                new_start: 1,
                new_lines: 3,
                lines: vec![
                    LineOp {
                        kind: LineKind::Context,
                        content: "line1".into(),
                    },
                    LineOp {
                        kind: LineKind::Remove,
                        content: "line2".into(),
                    },
                    LineOp {
                        kind: LineKind::Add,
                        content: "line2-new".into(),
                    },
                    LineOp {
                        kind: LineKind::Context,
                        content: "line3".into(),
                    },
                ],
            }],
        };
        let current = "line1\nline2\nline3\n";
        let (new, stats) = apply_to_file(&patch, current).expect("applies");
        assert_eq!(new, "line1\nline2-new\nline3\n");
        assert_eq!(stats.added, 1);
        assert_eq!(stats.removed, 1);
    }

    #[test]
    fn apply_pure_addition() {
        // Old had 1 line at line 1, new has 3 lines (1 context + 2 added).
        let patch = FilePatch {
            path: "x".into(),
            hunks: vec![Hunk {
                old_start: 1,
                old_lines: 1,
                new_start: 1,
                new_lines: 3,
                lines: vec![
                    LineOp {
                        kind: LineKind::Context,
                        content: "a".into(),
                    },
                    LineOp {
                        kind: LineKind::Add,
                        content: "b".into(),
                    },
                    LineOp {
                        kind: LineKind::Add,
                        content: "c".into(),
                    },
                ],
            }],
        };
        let (new, stats) = apply_to_file(&patch, "a\n").expect("applies");
        assert_eq!(new, "a\nb\nc\n");
        assert_eq!(stats.added, 2);
        assert_eq!(stats.removed, 0);
    }

    #[test]
    fn apply_pure_deletion() {
        let patch = FilePatch {
            path: "x".into(),
            hunks: vec![Hunk {
                old_start: 1,
                old_lines: 3,
                new_start: 1,
                new_lines: 1,
                lines: vec![
                    LineOp {
                        kind: LineKind::Context,
                        content: "a".into(),
                    },
                    LineOp {
                        kind: LineKind::Remove,
                        content: "b".into(),
                    },
                    LineOp {
                        kind: LineKind::Remove,
                        content: "c".into(),
                    },
                ],
            }],
        };
        let (new, stats) = apply_to_file(&patch, "a\nb\nc\n").expect("applies");
        assert_eq!(new, "a\n");
        assert_eq!(stats.added, 0);
        assert_eq!(stats.removed, 2);
    }

    #[test]
    fn apply_multi_hunk_with_offset() {
        // Hunk 1: line 1: replace "a" with "A" (adds 0, removes 0, net 0).
        // Hunk 2: line 3: replace "c" with "C" (adds 0, removes 0).
        // Original file lines: 1=a, 2=b, 3=c, 4=d.
        let patch = FilePatch {
            path: "x".into(),
            hunks: vec![
                Hunk {
                    old_start: 1,
                    old_lines: 1,
                    new_start: 1,
                    new_lines: 1,
                    lines: vec![
                        LineOp {
                            kind: LineKind::Remove,
                            content: "a".into(),
                        },
                        LineOp {
                            kind: LineKind::Add,
                            content: "A".into(),
                        },
                    ],
                },
                Hunk {
                    old_start: 3,
                    old_lines: 1,
                    new_start: 3,
                    new_lines: 1,
                    lines: vec![
                        LineOp {
                            kind: LineKind::Remove,
                            content: "c".into(),
                        },
                        LineOp {
                            kind: LineKind::Add,
                            content: "C".into(),
                        },
                    ],
                },
            ],
        };
        let (new, _) = apply_to_file(&patch, "a\nb\nc\nd\n").expect("applies");
        assert_eq!(new, "A\nb\nC\nd\n");
    }

    #[test]
    fn apply_multi_hunk_with_real_offset() {
        // Hunk 1: at line 1, replace 1 line with 3 (net +2).
        // Hunk 2: at line 5 (original), context + 1 add (net +1).
        // Without offset tracking, the second hunk would look at line 5
        // in the modified buffer and find the WRONG content.
        let patch = FilePatch {
            path: "x".into(),
            hunks: vec![
                Hunk {
                    old_start: 1,
                    old_lines: 1,
                    new_start: 1,
                    new_lines: 3,
                    lines: vec![
                        LineOp {
                            kind: LineKind::Remove,
                            content: "a".into(),
                        },
                        LineOp {
                            kind: LineKind::Add,
                            content: "X".into(),
                        },
                        LineOp {
                            kind: LineKind::Add,
                            content: "Y".into(),
                        },
                        LineOp {
                            kind: LineKind::Add,
                            content: "Z".into(),
                        },
                    ],
                },
                Hunk {
                    old_start: 5,
                    old_lines: 2,
                    new_start: 7,
                    new_lines: 3,
                    lines: vec![
                        LineOp {
                            kind: LineKind::Context,
                            content: "before".into(),
                        },
                        LineOp {
                            kind: LineKind::Remove,
                            content: "old-end".into(),
                        },
                        LineOp {
                            kind: LineKind::Add,
                            content: "new-end".into(),
                        },
                        LineOp {
                            kind: LineKind::Context,
                            content: "after".into(),
                        },
                    ],
                },
            ],
        };
        let original = "a\nb\nc\nd\nbefore\nold-end\nafter\n";
        let (new, stats) = apply_to_file(&patch, original).expect("applies");
        // Hunk 1 turns "a\nb\nc\nd\nbefore\n..." into "X\nY\nZ\nb\nc\nd\nbefore\n..."
        // (inserts at position 0, replaces 1 line with 3)
        // Hunk 2 then needs to find "before" at original line 5 = position 4 in
        // ORIGINAL = position 6 in MODIFIED (offset = 3-1 = +2).
        assert_eq!(new, "X\nY\nZ\nb\nc\nd\nbefore\nnew-end\nafter\n");
        assert_eq!(stats.added, 4);
        assert_eq!(stats.removed, 2);
    }

    #[test]
    fn apply_preserves_trailing_newline() {
        let patch = FilePatch {
            path: "x".into(),
            hunks: vec![Hunk {
                old_start: 1,
                old_lines: 1,
                new_start: 1,
                new_lines: 1,
                lines: vec![
                    LineOp {
                        kind: LineKind::Remove,
                        content: "old".into(),
                    },
                    LineOp {
                        kind: LineKind::Add,
                        content: "new".into(),
                    },
                ],
            }],
        };
        let (new, _) = apply_to_file(&patch, "old\n").expect("applies");
        assert_eq!(new, "new\n");
    }

    #[test]
    fn apply_handles_no_trailing_newline() {
        let patch = FilePatch {
            path: "x".into(),
            hunks: vec![Hunk {
                old_start: 1,
                old_lines: 1,
                new_start: 1,
                new_lines: 1,
                lines: vec![
                    LineOp {
                        kind: LineKind::Remove,
                        content: "old".into(),
                    },
                    LineOp {
                        kind: LineKind::Add,
                        content: "new".into(),
                    },
                ],
            }],
        };
        let (new, _) = apply_to_file(&patch, "old").expect("applies");
        assert_eq!(new, "new");
    }

    // ---- apply: errors ----

    #[test]
    fn apply_context_mismatch_returns_conflict() {
        let patch = FilePatch {
            path: "x".into(),
            hunks: vec![Hunk {
                old_start: 1,
                old_lines: 2,
                new_start: 1,
                new_lines: 2,
                lines: vec![
                    LineOp {
                        kind: LineKind::Context,
                        content: "wrong".into(),
                    },
                    LineOp {
                        kind: LineKind::Remove,
                        content: "b".into(),
                    },
                    LineOp {
                        kind: LineKind::Add,
                        content: "B".into(),
                    },
                ],
            }],
        };
        match apply_to_file(&patch, "actual\nb\n") {
            Err(ApplyError::Conflict { detail }) => {
                assert!(detail.contains("context"), "{}", detail);
                assert!(detail.contains("line 1"), "{}", detail);
            }
            other => panic!("expected Conflict, got {:?}", other),
        }
    }

    #[test]
    fn apply_past_eof_returns_conflict() {
        let patch = FilePatch {
            path: "x".into(),
            hunks: vec![Hunk {
                old_start: 100,
                old_lines: 2,
                new_start: 100,
                new_lines: 2,
                lines: vec![
                    LineOp {
                        kind: LineKind::Context,
                        content: "a".into(),
                    },
                    LineOp {
                        kind: LineKind::Context,
                        content: "b".into(),
                    },
                ],
            }],
        };
        assert!(matches!(
            apply_to_file(&patch, "short\n"),
            Err(ApplyError::OutOfRange { line: 100 })
        ));
    }

    // ---- end-to-end: parse + apply ----

    #[test]
    fn parse_then_apply_round_trip() {
        let diff =
            "--- a/foo.rs\n+++ b/foo.rs\n@@ -1,3 +1,3 @@\n line1\n-line2\n+line2-new\n line3\n";
        let patches = parse_unified_diff(diff).expect("parses");
        assert_eq!(patches.len(), 1);
        let (new, stats) = apply_to_file(&patches[0], "line1\nline2\nline3\n").expect("applies");
        assert_eq!(new, "line1\nline2-new\nline3\n");
        assert_eq!(stats.added, 1);
        assert_eq!(stats.removed, 1);
    }
}
