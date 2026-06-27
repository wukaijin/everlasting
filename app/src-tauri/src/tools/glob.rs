//! `glob` tool — file path pattern matching.
//!
//! Wraps the [`globset`] crate. The default is to **not** enforce
//! `.gitignore` (matching claude-code's `glob` tool behavior), because
//! the LLM sometimes wants to find a config file the project has marked
//! as ignored (e.g. a `.env.example`).
//!
//! Hard rules (see `.trellis/tasks/06-07-06-07-extend-toolset/prd.md` §R3):
//! - Cap of 100 entries. Beyond the cap, a truncation hint is appended.
//! - Sort by mtime descending (most-recently-changed first).
//! - If the user wants recursive discovery of hidden files, they should
//!   use a pattern like `**/.gitignore` directly — we don't pre-filter.
//!
//! Returns `(content, is_error)` like the other tools.

use std::path::Path;
use std::time::SystemTime;

use globset::Glob;

use crate::llm::types::ToolDef;
use crate::projects::boundary::assert_within_root;
use crate::tools::ToolContext;
/// Match claude-code: cap at 100 results to keep the agent's context
/// from blowing up on overly-broad patterns.
const MAX_RESULTS: usize = 100;

pub fn definition() -> ToolDef {
    ToolDef {
        name: "glob".to_string(),
        description: Some(
            "Find file paths matching a glob pattern. **Recursive** by default — \
             `**` matches any number of nested directories. The pattern is relative \
             to the search root unless absolute.\n\n\
             Returns up to 100 matches, sorted by mtime (most recent first). Does not \
             enforce `.gitignore` — to find files the project has marked as ignored, \
             pass the path explicitly. Hidden files (starting with `.`) are included \
             if they match the pattern."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern, e.g. `src/**/*.rs`, `**/Cargo.toml`, `*.json`."
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in. Defaults to the session cwd."
                }
            },
            "required": ["pattern"]
        }),
    }
}

#[derive(Debug, Clone)]
struct Match {
    path: String,
    mtime: Option<SystemTime>,
}

/// Execute the tool. Returns `(content, is_error)`.
pub async fn execute(input: &serde_json::Value, ctx: &ToolContext) -> (String, bool) {
    let pattern = match input.get("pattern").and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => s,
        _ => return ("Missing required parameter: pattern".to_string(), true),
    };

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
    let validated_root = match assert_within_root(&ctx.worktree_path, &requested) {
        Ok(p) => p,
        Err(e) => {
            return (
                format!("path '{}' rejected: {}", raw_path, e),
                true,
            );
        }
    };

    // 2. Build the glob matcher. The pattern is relative to the search
    //    root, so we anchor it there.
    let glob = match Glob::new(pattern) {
        Ok(g) => g.compile_matcher(),
        Err(e) => {
            return (
                format!("Invalid glob pattern '{}': {}", pattern, e),
                true,
            );
        }
    };

    tracing::debug!(
        pattern = %pattern,
        path = %validated_root.display(),
        "glob: walking tree"
    );

    // 3. Walk + match + collect OFF the tokio worker (RULE-E-004,
    //    2026-06-16). `walk_dir` uses sync `std::fs::read_dir`,
    //    which blocks the async runtime on large repos (a
    //    Chromium checkout / Linux kernel tree). Offload the
    //    entire walk + glob match + mtime read to the blocking
    //    pool so one big glob can't starve other sessions sharing
    //    the runtime. Sort + output formatting stay on the async
    //    side (pure CPU, no syscall).
    let root = validated_root.clone();
    let worktree = ctx.worktree_path.clone();
    let join = tokio::task::spawn_blocking(
        move || -> Result<(Vec<Match>, usize), std::io::Error> {
            let walker = walk_dir(&root)?;
            let mut matches: Vec<Match> = Vec::new();
            let mut truncated = 0usize;
            for entry in walker {
                let ft = match entry.file_type() {
                    Ok(ft) => ft,
                    Err(_) => continue,
                };
                if !ft.is_file() {
                    continue;
                }
                let abs = entry.path();
                // Compute the path RELATIVE to the search root. The
                // LLM supplies patterns relative to `path` (or cwd),
                // so the pattern only makes sense against the
                // relative form.
                let rel_for_match = abs
                    .strip_prefix(&root)
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|_| abs.clone());
                if !glob.is_match(&rel_for_match) {
                    continue;
                }
                let mtime = entry.metadata().ok().and_then(|m| m.modified().ok());
                // Display path relative to the project root for the LLM.
                let rel = abs
                    .strip_prefix(&worktree)
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|_| abs.to_path_buf());
                if matches.len() >= MAX_RESULTS {
                    truncated += 1;
                    continue;
                }
                matches.push(Match {
                    path: rel.to_string_lossy().to_string(),
                    mtime,
                });
            }
            Ok((matches, truncated))
        },
    );
    let (mut matches, truncated) = match join.await {
        Ok(Ok(tuple)) => tuple,
        Ok(Err(e)) => {
            return (
                format!("Failed to walk directory '{}': {}", validated_root.display(), e),
                true,
            );
        }
        Err(e) => {
            // spawn_blocking task panicked or was cancelled.
            return (format!("glob walk task failed: {}", e), true);
        }
    };

    // 4. Sort by mtime descending (most recent first). Files without
    //    an mtime sink to the bottom.
    matches.sort_by(|a, b| match (a.mtime, b.mtime) {
        (Some(x), Some(y)) => y.cmp(&x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.path.cmp(&b.path),
    });

    if matches.is_empty() {
        return (
            format!(
                "No files matched pattern '{}' in {}.",
                pattern,
                validated_root.display()
            ),
            false,
        );
    }

    let mut out = String::new();
    for m in &matches {
        out.push_str(&m.path);
        out.push('\n');
    }
    if truncated > 0 {
        out.push_str(&format!(
            "\n(...and {} more matches; narrow your pattern to see them)",
            truncated
        ));
    } else if matches.len() == MAX_RESULTS {
        out.push_str(&format!(
            "\n(showing the {} most recent matches; narrow your pattern for the rest)",
            MAX_RESULTS
        ));
    }

    (out.trim_end_matches('\n').to_string(), false)
}

/// Walk `root` recursively, yielding every entry. We do not use
/// `walkdir` to avoid pulling in another crate; `std::fs::read_dir`
/// + a manual stack gives us the same with std + tokio.
fn walk_dir(root: &Path) -> std::io::Result<Vec<std::fs::DirEntry>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let read = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue, // skip dirs we can't read (perm, etc.)
        };
        for entry in read.flatten() {
            let path = entry.path();
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            out.push(entry);
            if ft.is_dir() {
                stack.push(path);
            }
        }
    }
    Ok(out)
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
        }
    }

    #[test]
    fn definition_has_correct_name() {
        assert_eq!(definition().name, "glob");
    }

    #[tokio::test]
    async fn basic_match() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "x").unwrap();
        std::fs::write(tmp.path().join("b.txt"), "x").unwrap();
        let (content, is_err) = execute(
            &serde_json::json!({"pattern": "*.rs"}),
            &test_ctx(&tmp),
        )
        .await;
        assert!(!is_err, "{}", content);
        assert!(content.contains("a.rs"));
        assert!(!content.contains("b.txt"));
    }

    #[tokio::test]
    async fn recursive_pattern() {
        let tmp = tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("src/sub")).unwrap();
        std::fs::write(tmp.path().join("src/a.rs"), "x").unwrap();
        std::fs::write(tmp.path().join("src/sub/b.rs"), "x").unwrap();
        std::fs::write(tmp.path().join("c.rs"), "x").unwrap();
        let (content, is_err) = execute(
            &serde_json::json!({"pattern": "src/**/*.rs"}),
            &test_ctx(&tmp),
        )
        .await;
        assert!(!is_err, "{}", content);
        assert!(content.contains("src/a.rs"));
        assert!(content.contains("src/sub/b.rs"));
    }

    #[tokio::test]
    async fn no_matches_friendly() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "x").unwrap();
        let (content, is_err) = execute(
            &serde_json::json!({"pattern": "*.nonexistent"}),
            &test_ctx(&tmp),
        )
        .await;
        assert!(!is_err, "{}", content);
        assert!(content.contains("No files matched"));
    }

    #[tokio::test]
    async fn cap_100_truncation_hint() {
        let tmp = tempdir().unwrap();
        // Create 110 files to trip the cap.
        for i in 0..110 {
            std::fs::write(tmp.path().join(format!("f{:03}.txt", i)), "x").unwrap();
        }
        let (content, is_err) = execute(
            &serde_json::json!({"pattern": "*.txt"}),
            &test_ctx(&tmp),
        )
        .await;
        assert!(!is_err, "{}", content);
        assert!(content.contains("more matches"));
    }

    #[tokio::test]
    async fn relative_path_works() {
        let tmp = tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        std::fs::write(tmp.path().join("sub").join("a.rs"), "x").unwrap();
        let (content, is_err) = execute(
            &serde_json::json!({
                "pattern": "a.rs",
                "path": "sub",
            }),
            &test_ctx(&tmp),
        )
        .await;
        assert!(!is_err, "{}", content);
        assert!(content.contains("a.rs"));
    }

    #[tokio::test]
    async fn invalid_pattern() {
        let tmp = tempdir().unwrap();
        let (content, is_err) = execute(
            &serde_json::json!({"pattern": "[invalid"}),
            &test_ctx(&tmp),
        )
        .await;
        assert!(is_err);
        assert!(content.contains("Invalid glob pattern"));
    }

    #[tokio::test]
    async fn path_outside_root_rejected() {
        let tmp = tempdir().unwrap();
        let (content, is_err) = execute(
            &serde_json::json!({
                "pattern": "*.rs",
                "path": "/etc",
            }),
            &test_ctx(&tmp),
        )
        .await;
        assert!(is_err);
        assert!(content.contains("rejected") || content.contains("outside"));
    }
}
