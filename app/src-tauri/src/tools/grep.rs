//! `grep` tool — content search via ripgrep.
//!
//! Spawns `rg` with the appropriate flags based on `output_mode`:
//! - `files_with_matches` → `rg -l` (file paths only)
//! - `content`            → `rg -n` (path:line:content)
//! - `count`              → `rg -c` (path:count)
//!
//! Hard rules (see `.trellis/tasks/06-07-06-07-extend-toolset/prd.md` §R2):
//! - Default: respect `.gitignore` (claude-code behavior; `-uu` is
//!   NOT passed).
//! - Per-line cap of 500 chars (`GREP_MAX_LINE_LENGTH`) — long lines are
//!   truncated to keep the agent's context window from blowing up on
//!   minified JS or generated code.
//! - 0 matches → friendly "No matches found" message (rg exit 1 is
//!   translated to an `is_error = false` empty result).
//! - Other non-zero exits → returned as `is_error = true`.

use std::path::Path;
use std::process::Stdio;

use tokio::process::Command;

use crate::llm::types::ToolDef;
use crate::projects::boundary::assert_within_root;
use crate::tools::ToolContext;

/// Per-line cap from pi_agent_rust's `GREP_MAX_LINE_LENGTH`.
const GREP_MAX_LINE_LENGTH: usize = 500;

/// rg exit code 1 = "no matches". 0 = found something, 2 = real error.
const RG_EXIT_NO_MATCH: i32 = 1;

pub fn definition() -> ToolDef {
    ToolDef {
        name: "grep".to_string(),
        description: Some(
            "Search file contents using ripgrep. By default respects .gitignore.\n\n\
             `output_mode` controls the output format:\n\
             - `files_with_matches` (default): file paths only, one per line.\n\
             - `content`: `path:line:text` for each match, with line numbers.\n\
             - `count`: `path:count` per file.\n\n\
             `path` is the search root; if omitted, the session cwd is used. \
             `glob` filters by file name (e.g. `*.rs`). `context` shows N lines \
             around each match. `head_limit` caps the number of matches returned."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regular expression to search for. ripgrep syntax (Rust regex)."
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in. Defaults to the session cwd."
                },
                "glob": {
                    "type": "string",
                    "description": "Filter by file name (e.g. `*.rs`, `**/test_*.py`). Optional."
                },
                "output_mode": {
                    "type": "string",
                    "enum": ["files_with_matches", "content", "count"],
                    "description": "Output format. Default: `files_with_matches`."
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "Case-insensitive search. Default: false."
                },
                "show_line_numbers": {
                    "type": "boolean",
                    "description": "Include 1-based line numbers in `content` output. Default: true. \
                                    Has no effect in `files_with_matches` or `count` mode."
                },
                "context": {
                    "type": "integer",
                    "description": "Number of context lines to include around each match (sets `-C`). Optional."
                },
                "head_limit": {
                    "type": "integer",
                    "description": "Cap on the number of matches returned. Optional."
                }
            },
            "required": ["pattern"]
        }),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputMode {
    FilesWithMatches,
    Content,
    Count,
}

impl OutputMode {
    fn from_input(v: &serde_json::Value) -> Self {
        match v
            .get("output_mode")
            .and_then(|x| x.as_str())
            .unwrap_or("files_with_matches")
        {
            "content" => OutputMode::Content,
            "count" => OutputMode::Count,
            _ => OutputMode::FilesWithMatches,
        }
    }
}

/// Execute the tool. Returns `(content, is_error)`.
pub async fn execute(input: &serde_json::Value, ctx: &ToolContext) -> (String, bool) {
    let pattern = match input.get("pattern").and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => s,
        _ => return ("Missing required parameter: pattern".to_string(), true),
    };

    let output_mode = OutputMode::from_input(input);
    let case_insensitive = input
        .get("case_insensitive")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let show_line_numbers = input
        .get("show_line_numbers")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let context = input
        .get("context")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);
    let head_limit = input
        .get("head_limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);
    let glob_filter = input.get("glob").and_then(|v| v.as_str());

    // 1. Resolve the search root against ctx.cwd.
    let raw_path = input
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    let requested = {
        let p = Path::new(raw_path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            ctx.cwd.join(p)
        }
    };
    let validated_root = match assert_within_root(&ctx.project_root, &requested) {
        Ok(p) => p,
        Err(e) => {
            return (
                format!("path '{}' rejected: {}", raw_path, e),
                true,
            );
        }
    };

    // 2. Build the rg command. The default is to respect .gitignore
    //    (no -u / -uu / -uuu flags), matching claude-code behavior.
    let mut cmd = Command::new("rg");
    match output_mode {
        OutputMode::FilesWithMatches => {
            cmd.arg("--files-with-matches");
        }
        OutputMode::Content => {
            // `-n` adds file:line:content format. In content mode
            // show_line_numbers is on by default; honor false to drop
            // the line number prefix.
            if show_line_numbers {
                cmd.arg("--line-number");
            } else {
                cmd.arg("--no-line-number");
            }
        }
        OutputMode::Count => {
            cmd.arg("--count");
        }
    }
    if case_insensitive {
        cmd.arg("-i");
    }
    if let Some(c) = context {
        cmd.arg("-C").arg(c.to_string());
    }
    if let Some(g) = glob_filter {
        cmd.arg("--glob").arg(g);
    }
    if let Some(limit) = head_limit {
        cmd.arg("--max-count").arg(limit.to_string());
    }
    // Make rg deterministic: sort output, treat binary as text (don't
    // skip matches in minified bundles), disable the pager.
    cmd.arg("--sort").arg("path");
    cmd.arg("--no-messages");
    cmd.arg("--").arg(pattern).arg(&validated_root);

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    tracing::debug!(
        pattern = %pattern,
        path = %validated_root.display(),
        output_mode = ?output_mode,
        case_insensitive,
        show_line_numbers,
        context = ?context,
        head_limit = ?head_limit,
        glob = ?glob_filter,
        "grep: spawning rg"
    );

    // 3. Spawn and collect output.
    let output = match cmd.output().await {
        Ok(o) => o,
        Err(e) => {
            return (
                format!("Failed to spawn ripgrep (is `rg` on PATH?): {}", e),
                true,
            );
        }
    };

    let exit_code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    // 4. Translate exit codes.
    if exit_code == RG_EXIT_NO_MATCH || (exit_code == 0 && stdout.is_empty()) {
        return (
            format!(
                "No matches found for pattern '{}' in {}.",
                pattern,
                validated_root.display()
            ),
            false,
        );
    }
    if !output.status.success() {
        // Real error (rg exit 2 or signal). Include stderr.
        let snippet = stderr.trim();
        let first_lines: String = snippet
            .lines()
            .take(5)
            .collect::<Vec<_>>()
            .join("\n");
        return (
            format!(
                "ripgrep failed (exit {}): {}{}",
                exit_code,
                first_lines,
                if snippet.lines().count() > 5 { "\n..." } else { "" }
            ),
            true,
        );
    }

    // 5. Apply per-line cap. For content mode this is critical to
    //    prevent minified JS / generated code from blowing up the
    //    agent's context.
    let capped = cap_line_lengths(&stdout, GREP_MAX_LINE_LENGTH);

    // 6. For content mode, rg emits `path:line:content`; the path
    //    portion is the canonical absolute path. To keep results
    //    human-friendly, we rewrite it back to a relative path
    //    against the project root.
    let formatted = if output_mode == OutputMode::Content {
        rewrite_paths_to_relative(&capped, &validated_root, &ctx.project_root)
    } else {
        capped
    };

    let line_count = formatted.lines().count();
    let tail = if let Some(limit) = head_limit {
        if line_count >= limit {
            format!(
                "\n(hit head_limit of {}; narrow your pattern or raise the limit)",
                limit
            )
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    (format!("{}{}", formatted.trim_end_matches('\n'), tail), false)
}

/// Truncate any line longer than `cap` to `cap` chars (with a marker).
fn cap_line_lengths(s: &str, cap: usize) -> String {
    if cap == 0 {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    for (i, line) in s.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if line.chars().count() > cap {
            let truncated: String = line.chars().take(cap).collect();
            out.push_str(&truncated);
            out.push_str("… <truncated>");
        } else {
            out.push_str(line);
        }
    }
    if s.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Rewrite absolute paths in `text` to be relative to `project_root`,
/// so the LLM sees clean `src/foo.rs:42:...` style results.
fn rewrite_paths_to_relative(text: &str, search_root: &Path, project_root: &Path) -> String {
    let search_str = search_root.to_string_lossy();
    let project_str = project_root.to_string_lossy();
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if let Some(stripped) = line.strip_prefix(search_str.as_ref()) {
            // search_root is typically a subdir of project_root;
            // keep the relative-from-search form.
            out.push_str(stripped.trim_start_matches('/'));
        } else if let Some(stripped) = line.strip_prefix(project_str.as_ref()) {
            out.push_str(stripped.trim_start_matches('/'));
        } else {
            out.push_str(line);
        }
    }
    if text.ends_with('\n') {
        out.push('\n');
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

    /// Helper: `rg` may not be installed in every CI environment.
    /// These tests skip if `rg` is not on PATH so the suite is portable.
    fn rg_available() -> bool {
        std::process::Command::new("rg")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[test]
    fn definition_has_correct_name() {
        assert_eq!(definition().name, "grep");
    }

    #[tokio::test]
    async fn files_with_matches_default() {
        if !rg_available() {
            eprintln!("rg not available, skipping");
            return;
        }
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "fn foo() {}\nfn bar() {}\n").unwrap();
        std::fs::write(tmp.path().join("b.rs"), "fn baz() {}\n").unwrap();
        let (content, is_err) = execute(
            &serde_json::json!({"pattern": "fn foo"}),
            &test_ctx(&tmp),
        )
        .await;
        assert!(!is_err, "{}", content);
        assert!(content.contains("a.rs"));
        assert!(!content.contains("b.rs"));
    }

    #[tokio::test]
    async fn content_mode_includes_line_numbers() {
        if !rg_available() {
            return;
        }
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "first\nfoo target\nthird\n").unwrap();
        let (content, is_err) = execute(
            &serde_json::json!({
                "pattern": "foo target",
                "output_mode": "content",
            }),
            &test_ctx(&tmp),
        )
        .await;
        assert!(!is_err, "{}", content);
        // Default: line numbers shown. Format is `path:line:text`.
        assert!(content.contains(":2:") || content.contains(":2\t"));
    }

    #[tokio::test]
    async fn count_mode_returns_counts() {
        if !rg_available() {
            return;
        }
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "foo\nfoo\nfoo\n").unwrap();
        let (content, is_err) = execute(
            &serde_json::json!({
                "pattern": "foo",
                "output_mode": "count",
            }),
            &test_ctx(&tmp),
        )
        .await;
        assert!(!is_err, "{}", content);
        assert!(content.contains("3"));
    }

    #[tokio::test]
    async fn no_matches_friendly_message() {
        if !rg_available() {
            return;
        }
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "hello\n").unwrap();
        let (content, is_err) = execute(
            &serde_json::json!({"pattern": "this_does_not_appear"}),
            &test_ctx(&tmp),
        )
        .await;
        assert!(!is_err, "{}", content);
        assert!(content.contains("No matches found"));
    }

    #[tokio::test]
    async fn case_insensitive_flag() {
        if !rg_available() {
            return;
        }
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "FOO\nfoo\nFoo\n").unwrap();
        let (content, is_err) = execute(
            &serde_json::json!({
                "pattern": "foo",
                "case_insensitive": true,
            }),
            &test_ctx(&tmp),
        )
        .await;
        assert!(!is_err, "{}", content);
        assert!(content.contains("a.txt"));
    }

    #[tokio::test]
    async fn relative_path_works() {
        if !rg_available() {
            return;
        }
        let tmp = tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        std::fs::write(tmp.path().join("sub").join("a.txt"), "needle\n").unwrap();
        let (content, is_err) = execute(
            &serde_json::json!({
                "pattern": "needle",
                "path": "sub",
            }),
            &test_ctx(&tmp),
        )
        .await;
        assert!(!is_err, "{}", content);
        assert!(content.contains("a.txt"));
    }

    #[tokio::test]
    async fn long_line_is_capped() {
        let s = "a".repeat(GREP_MAX_LINE_LENGTH + 50);
        let capped = cap_line_lengths(&s, GREP_MAX_LINE_LENGTH);
        // The marker should be present.
        assert!(capped.contains("<truncated>"));
        // The original full string should NOT be present.
        assert!(!capped.contains(&"a".repeat(GREP_MAX_LINE_LENGTH + 50)));
    }

    #[tokio::test]
    async fn missing_pattern_param() {
        let tmp = tempdir().unwrap();
        let (content, is_err) = execute(&serde_json::json!({}), &test_ctx(&tmp)).await;
        assert!(is_err);
        assert!(content.contains("Missing required parameter"));
    }

    #[tokio::test]
    async fn head_limit_announces_truncation() {
        if !rg_available() {
            return;
        }
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "x\n".repeat(10)).unwrap();
        let (content, is_err) = execute(
            &serde_json::json!({
                "pattern": "x",
                "output_mode": "files_with_matches",
                "head_limit": 1,
            }),
            &test_ctx(&tmp),
        )
        .await;
        assert!(!is_err, "{}", content);
        assert!(content.contains("head_limit"));
    }
}
