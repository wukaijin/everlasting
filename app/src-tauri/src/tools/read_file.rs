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

use std::path::Path;

use crate::llm::types::ToolDef;
use crate::projects::boundary::assert_within_root;
use crate::tools::ToolContext;

/// Max output before truncation (matches ARCHITECTURE.md §2.5.3).
const MAX_OUTPUT_BYTES: usize = 50 * 1024;

pub fn definition() -> ToolDef {
    ToolDef {
        name: "read_file".to_string(),
        description: Some(
            "Read the contents of a file. Paths may be relative (resolved against \
             the session's current working directory) or absolute. In either case \
             the resolved file must be inside the active project root."
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
pub async fn execute(input: &serde_json::Value, ctx: &ToolContext) -> (String, bool) {
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
        Ok(content) => (truncate_output(content), false),
        Err(e) => (
            format!("Failed to read file '{}': {}", validated.display(), e),
            true,
        ),
    }
}

/// Truncate output exceeding MAX_OUTPUT_BYTES (head + tail, omit middle).
fn truncate_output(content: String) -> String {
    if content.len() <= MAX_OUTPUT_BYTES {
        return content;
    }
    let head_end = 25 * 1024;
    let tail_start = content.len() - 25 * 1024;
    let omitted = content.len() - MAX_OUTPUT_BYTES;
    format!(
        "{}\n<truncated: omitted {} bytes>\n{}",
        &content[..head_end],
        omitted,
        &content[tail_start..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[tokio::test]
    async fn execute_reads_real_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("hello.txt"), "world").unwrap();
        let (content, is_error) = execute(
            &serde_json::json!({"path": tmp.path().join("hello.txt").to_string_lossy()}),
            &test_ctx(&tmp),
        )
        .await;
        assert!(!is_error);
        assert_eq!(content, "world");
    }

    #[tokio::test]
    async fn execute_resolves_relative_path_against_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        std::fs::write(tmp.path().join("sub").join("a.txt"), "relative").unwrap();

        // ctx.cwd points to root; relative path "sub/a.txt" resolves there.
        let (content, is_error) = execute(
            &serde_json::json!({"path": "sub/a.txt"}),
            &test_ctx(&tmp),
        )
        .await;
        assert!(!is_error, "{}", content);
        assert_eq!(content, "relative");
    }

    #[tokio::test]
    async fn execute_rejects_traversal_outside_root() {
        let tmp = tempfile::tempdir().unwrap();
        let (msg, is_error) = execute(
            &serde_json::json!({"path": "/etc/hostname"}),
            &test_ctx(&tmp),
        )
        .await;
        assert!(is_error);
        assert!(msg.contains("rejected") || msg.contains("outside"));
    }

    #[tokio::test]
    async fn execute_rejects_relative_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        // Anchor cwd to root; "../../etc/hostname" would escape the
        // project root.
        let (msg, is_error) = execute(
            &serde_json::json!({"path": "../../etc/hostname"}),
            &test_ctx(&tmp),
        )
        .await;
        assert!(is_error);
        assert!(msg.contains("rejected") || msg.contains("outside") || msg.contains("cannot be resolved"));
    }

    #[tokio::test]
    async fn execute_missing_path_param() {
        let tmp = tempfile::tempdir().unwrap();
        let (content, is_error) = execute(&serde_json::json!({}), &test_ctx(&tmp)).await;
        assert!(is_error);
        assert!(content.contains("Missing"));
    }

    #[tokio::test]
    async fn execute_nonexistent_file() {
        let tmp = tempfile::tempdir().unwrap();
        let (content, is_error) = execute(
            &serde_json::json!({"path": tmp.path().join("nope.txt").to_string_lossy()}),
            &test_ctx(&tmp),
        )
        .await;
        assert!(is_error);
        // Boundary check rejects nonexistent files (canonicalize
        // fails), so the error is "rejected", not "Failed to read".
        assert!(content.contains("rejected") || content.contains("Failed to read"));
    }
}
