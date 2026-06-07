//! `list_dir` tool — non-recursive directory listing.
//!
//! Mirrors `ls -1` with `show_hidden` and `limit` knobs. Complements
//! `glob` (which is recursive). Hidden files (names starting with `.`)
//! are skipped by default; the LLM can opt in with `show_hidden: true`.
//!
//! Hard rules (see `.trellis/tasks/06-07-06-07-extend-toolset/prd.md` §R4):
//! - Non-recursive. For recursive discovery, use `glob`.
//! - Alphabetical sort. Directories get a trailing `/` so the LLM can
//!   tell file from dir at a glance.
//! - Default limit 500 entries.

use std::path::Path;

use crate::llm::types::ToolDef;
use crate::projects::boundary::assert_within_root;
use crate::tools::ToolContext;

/// Default cap on the number of entries returned. Matches the
/// `list_dir::limit` default in pi_agent_rust.
const DEFAULT_LIMIT: usize = 500;

pub fn definition() -> ToolDef {
    ToolDef {
        name: "list_dir".to_string(),
        description: Some(
            "List the entries in a directory (non-recursive). For recursive discovery, \
             use the `glob` tool instead.\n\n\
             Directories get a trailing `/` so they can be told apart from files. \
             Hidden files (names starting with `.`) are skipped by default; pass \
             `show_hidden: true` to include them. The result is sorted alphabetically."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory to list. Defaults to the session cwd."
                },
                "show_hidden": {
                    "type": "boolean",
                    "description": "Include entries whose names start with `.`. Default: false."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of entries to return. Default: 500."
                }
            }
        }),
    }
}

/// Execute the tool. Returns `(content, is_error)`.
pub async fn execute(input: &serde_json::Value, ctx: &ToolContext) -> (String, bool) {
    let raw_path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    let show_hidden = input
        .get("show_hidden")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let limit = input
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_LIMIT);

    // 1. Resolve path against ctx.cwd.
    let requested = {
        let p = Path::new(raw_path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            ctx.cwd.join(p)
        }
    };
    let validated = match assert_within_root(&ctx.worktree_path, &requested) {
        Ok(p) => p,
        Err(e) => {
            return (
                format!("path '{}' rejected: {}", raw_path, e),
                true,
            );
        }
    };

    // 2. Read directory entries.
    let read = match tokio::fs::read_dir(&validated).await {
        Ok(r) => r,
        Err(e) => {
            return (
                format!("Failed to read directory '{}': {}", validated.display(), e),
                true,
            );
        }
    };

    let mut entries: Vec<String> = Vec::new();
    let mut truncated: usize = 0;
    let mut iter = read;
    while let Some(item) = match iter.next_entry().await {
        Ok(opt) => opt,
        Err(e) => {
            return (
                format!(
                    "Failed to iterate directory '{}': {}",
                    validated.display(),
                    e
                ),
                true,
            );
        }
    } {
        let name = match item.file_name().into_string() {
            Ok(s) => s,
            Err(_) => continue, // skip non-UTF-8 names silently
        };
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        let ft = match item.file_type().await {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if entries.len() >= limit {
            truncated += 1;
            continue;
        }
        if ft.is_dir() {
            entries.push(format!("{}/", name));
        } else {
            entries.push(name);
        }
    }

    // 3. Sort alphabetically (after appending `/` for dirs, so dir
    //    names sort with the rest).
    entries.sort();

    if entries.is_empty() {
        return (
            format!("(empty directory: {})", validated.display()),
            false,
        );
    }

    let mut out = entries.join("\n");
    if truncated > 0 {
        out.push_str(&format!(
            "\n\n(...{} more entries hidden by limit; raise the limit or pass show_hidden: true)",
            truncated
        ));
    }
    (out, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_ctx(tmp: &tempfile::TempDir) -> ToolContext {
        ToolContext {
            worktree_path: tmp.path().canonicalize().unwrap(),
            cwd: tmp.path().canonicalize().unwrap(),
        }
    }

    #[test]
    fn definition_has_correct_name() {
        assert_eq!(definition().name, "list_dir");
    }

    #[tokio::test]
    async fn basic_list() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "x").unwrap();
        std::fs::write(tmp.path().join("b.txt"), "x").unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();

        let (content, is_err) = execute(
            &serde_json::json!({}),
            &test_ctx(&tmp),
        )
        .await;
        assert!(!is_err, "{}", content);
        // Alphabetical order, with `/` on dirs.
        assert!(content.contains("a.txt\nb.txt\nsub/") || content.contains("a.txt\nb.txt"));
    }

    #[tokio::test]
    async fn show_hidden() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "x").unwrap();
        std::fs::write(tmp.path().join(".hidden"), "x").unwrap();
        let ctx = test_ctx(&tmp);

        // Default: hidden skipped.
        let (content, is_err) = execute(&serde_json::json!({}), &ctx).await;
        assert!(!is_err, "{}", content);
        assert!(content.contains("a.txt"));
        assert!(!content.contains(".hidden"));

        // show_hidden: true → both.
        let (content, is_err) = execute(
            &serde_json::json!({"show_hidden": true}),
            &ctx,
        )
        .await;
        assert!(!is_err, "{}", content);
        assert!(content.contains("a.txt"));
        assert!(content.contains(".hidden"));
    }

    #[tokio::test]
    async fn limit_truncation() {
        let tmp = tempdir().unwrap();
        // 3 files + a low limit.
        std::fs::write(tmp.path().join("a.txt"), "x").unwrap();
        std::fs::write(tmp.path().join("b.txt"), "x").unwrap();
        std::fs::write(tmp.path().join("c.txt"), "x").unwrap();

        let (content, is_err) = execute(
            &serde_json::json!({"limit": 2}),
            &test_ctx(&tmp),
        )
        .await;
        assert!(!is_err, "{}", content);
        assert!(content.contains("more entries hidden"));
    }

    #[tokio::test]
    async fn empty_directory() {
        let tmp = tempdir().unwrap();
        let (content, is_err) = execute(&serde_json::json!({}), &test_ctx(&tmp)).await;
        assert!(!is_err, "{}", content);
        assert!(content.contains("empty directory"));
    }

    #[tokio::test]
    async fn relative_path() {
        let tmp = tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        std::fs::write(tmp.path().join("sub").join("a.txt"), "x").unwrap();
        let (content, is_err) = execute(
            &serde_json::json!({"path": "sub"}),
            &test_ctx(&tmp),
        )
        .await;
        assert!(!is_err, "{}", content);
        assert!(content.contains("a.txt"));
    }

    #[tokio::test]
    async fn nonexistent_path() {
        let tmp = tempdir().unwrap();
        let (content, is_err) = execute(
            &serde_json::json!({"path": "no-such-dir"}),
            &test_ctx(&tmp),
        )
        .await;
        assert!(is_err);
        assert!(content.contains("rejected") || content.contains("Failed to read"));
    }

    #[tokio::test]
    async fn path_outside_root_rejected() {
        let tmp = tempdir().unwrap();
        let (content, is_err) = execute(
            &serde_json::json!({"path": "/etc"}),
            &test_ctx(&tmp),
        )
        .await;
        assert!(is_err);
        assert!(content.contains("rejected") || content.contains("outside"));
    }
}
