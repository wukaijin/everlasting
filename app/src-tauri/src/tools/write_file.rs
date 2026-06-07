//! `write_file` tool — write content to a file.
//!
//! Step 3b-1 changes:
//! - Like `read_file`, the `path` is resolved relative to `ctx.cwd`
//!   if it is not absolute, and the resolved absolute path must be
//!   inside `ctx.worktree_path`.
//! - Parent directories are created on demand (preserved from step 2).
//! - The boundary check happens **before** any directory creation so
//!   the LLM cannot trick the tool into mkdir'ing outside the
//!   project root.

use std::path::Path;

use crate::llm::types::ToolDef;
use crate::projects::boundary::assert_within_root;
use crate::tools::ToolContext;

pub fn definition() -> ToolDef {
    ToolDef {
        name: "write_file".to_string(),
        description: Some(
            "Write content to a file. Paths may be relative (resolved against \
             the session's current working directory) or absolute. In either case \
             the resolved file must be inside the active project root. Creates \
             parent directories on demand."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to write."
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file."
                }
            },
            "required": ["path", "content"]
        }),
    }
}

/// Execute the tool. Returns `(content, is_error)`.
pub async fn execute(input: &serde_json::Value, ctx: &ToolContext) -> (String, bool) {
    let raw_path = match input.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return ("Missing required parameter: path".to_string(), true),
    };
    let content = match input.get("content").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return ("Missing required parameter: content".to_string(), true),
    };

    // 1. Resolve to an absolute Path. Relative inputs anchor on
    //    ctx.cwd.
    let requested: std::path::PathBuf = {
        let p = Path::new(raw_path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            ctx.cwd.join(p)
        }
    };

    // Diagnostic: log the LLM-supplied inputs at the entry point so that
    // intermittent write_file failures (see spike-005 #3) can be traced
    // back to the exact arguments the model emitted. Off by default;
    // visible under `RUST_LOG=debug pnpm tauri dev`.
    tracing::debug!(
        raw_path = %raw_path,
        content_len = content.len(),
        is_existing = requested.exists(),
        "write_file called"
    );

    // 2. Boundary check. Two cases:
    //    a) The full path (including any non-existing tail) does
    //       NOT exist — walk up to the first existing ancestor,
    //       validate that ancestor, then re-attach the missing
    //       tail (so we can later `create_dir_all` + `write`).
    //    b) The full path exists (file overwrite case) — we
    //       canonicalize the full path directly and check it.
    //
    // This avoids the trap where `assert_within_root` rejects a
    // non-existing target with "cannot be resolved", which would
    // prevent the user from ever creating a new file under a new
    // directory.
    let validated = if requested.exists() {
        match assert_within_root(&ctx.worktree_path, &requested) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(
                    raw_path = %raw_path,
                    worktree_path = %ctx.worktree_path.display(),
                    error = %e,
                    "write_file path rejected: outside project root (existing target)"
                );
                return (
                    format!("path '{}' rejected: {}", raw_path, e),
                    true,
                );
            }
        }
    } else {
        // Walk up to the first existing ancestor, collecting the
        // missing tail components (in reverse order).
        let mut check: &std::path::Path = requested.as_path();
        let mut tail: Vec<std::ffi::OsString> = Vec::new();
        loop {
            if check.exists() {
                break;
            }
            let Some(parent) = check.parent() else {
                break;
            };
            if parent.as_os_str().is_empty() {
                break;
            }
            let Some(name) = check.file_name() else {
                break;
            };
            tail.push(name.to_os_string());
            check = parent;
        }
        let validated_parent = match assert_within_root(&ctx.worktree_path, check) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(
                    raw_path = %raw_path,
                    worktree_path = %ctx.worktree_path.display(),
                    error = %e,
                    "write_file path rejected: outside project root (missing target)"
                );
                return (
                    format!("path '{}' rejected: {}", raw_path, e),
                    true,
                );
            }
        };
        // Re-attach the missing tail in original order.
        tail.reverse();
        let mut p = validated_parent;
        for c in &tail {
            p = p.join(c);
        }
        p
    };

    // 3. Create parent directories if needed. We only do this for
    //    the *validated* parent, so the LLM cannot escape via
    //    "create the path inside root, but then use the parent
    //    to write elsewhere".
    if let Some(grand) = validated.parent() {
        if !grand.as_os_str().is_empty() {
            if let Err(e) = tokio::fs::create_dir_all(grand).await {
                tracing::debug!(
                    raw_path = %raw_path,
                    parent = %grand.display(),
                    error = %e,
                    "write_file failed to create parent directories"
                );
                return (
                    format!("Failed to create parent directories: {}", e),
                    true,
                );
            }
        }
    }

    match tokio::fs::write(&validated, content).await {
        Ok(()) => (
            format!(
                "Successfully wrote {} bytes to {}",
                content.len(),
                validated.display()
            ),
            false,
        ),
        Err(e) => {
            tracing::debug!(
                raw_path = %raw_path,
                resolved_path = %validated.display(),
                content_len = content.len(),
                error = %e,
                "write_file write failed"
            );
            (
                format!("Failed to write file '{}': {}", validated.display(), e),
                true,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx(tmp: &tempfile::TempDir) -> ToolContext {
        ToolContext {
            worktree_path: tmp.path().canonicalize().unwrap(),
            cwd: tmp.path().canonicalize().unwrap(),
        }
    }

    #[test]
    fn definition_has_correct_name() {
        assert_eq!(definition().name, "write_file");
    }

    #[tokio::test]
    async fn execute_writes_and_reads_back() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.txt");

        let input = serde_json::json!({
            "path": path.to_string_lossy(),
            "content": "hello world",
        });
        let (msg, is_error) = execute(&input, &test_ctx(&tmp)).await;
        assert!(!is_error, "{}", msg);
        assert!(msg.contains("Successfully wrote"));

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "hello world");
    }

    #[tokio::test]
    async fn execute_writes_relative_path() {
        let tmp = tempfile::tempdir().unwrap();
        let input = serde_json::json!({
            "path": "rel.txt",
            "content": "relative content",
        });
        let (msg, is_error) = execute(&input, &test_ctx(&tmp)).await;
        assert!(!is_error, "{}", msg);

        let path = tmp.path().join("rel.txt");
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "relative content");
    }

    #[tokio::test]
    async fn execute_creates_parent_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a/b/c/file.txt");
        let input = serde_json::json!({
            "path": path.to_string_lossy(),
            "content": "nested",
        });
        let (msg, is_error) = execute(&input, &test_ctx(&tmp)).await;
        assert!(!is_error, "{}", msg);
        assert!(tokio::fs::read_to_string(&path).await.is_ok());
    }

    #[tokio::test]
    async fn execute_rejects_outside_root() {
        let tmp = tempfile::tempdir().unwrap();
        let input = serde_json::json!({
            "path": "/etc/hostname",
            "content": "x",
        });
        let (msg, is_error) = execute(&input, &test_ctx(&tmp)).await;
        assert!(is_error);
        assert!(msg.contains("rejected") || msg.contains("outside"));
    }

    #[tokio::test]
    async fn execute_missing_content_param() {
        let tmp = tempfile::tempdir().unwrap();
        let input = serde_json::json!({
            "path": tmp.path().join("x.txt").to_string_lossy(),
        });
        let (msg, is_error) = execute(&input, &test_ctx(&tmp)).await;
        assert!(is_error);
        assert!(msg.contains("Missing required parameter: content"));
    }
}
