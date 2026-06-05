//! `shell` tool — execute a shell command.
//!
//! Step 3b-1 changes:
//! - The LLM may optionally pass a `working_directory` field. The
//!   LLM-supplied value is **never trusted**: it is validated through
//!   `projects::boundary::assert_within_root` against
//!   `ctx.project_root` before being applied (评审 deepseek §4.1).
//! - If the LLM did not supply `working_directory`, the command runs
//!   with `ctx.cwd` as its cwd.
//! - The resolved cwd is **emitted** to the caller via a
//!   [`ToolContextUpdate`], so the agent loop can persist the final
//!   value at the end of the turn (per
//!   `docs/PROPOSAL-project-binding-and-top-tabs.md` §4.4 "turn 结束
//!   一次性写").
//!
//! Boundary failures from `working_directory` are returned to the
//! LLM as `is_error = true` so the model can self-correct (or be
//! retried by the user with a different cwd).

use std::path::Path;

use crate::llm::types::ToolDef;
use crate::projects::boundary::assert_within_root;
use crate::tools::{ToolContext, ToolContextUpdate};

/// Max output before truncation (matches ARCHITECTURE.md §2.5.3).
const MAX_OUTPUT_BYTES: usize = 50 * 1024;
/// Command timeout in seconds (matches ARCHITECTURE.md §2.5.2).
const TIMEOUT_SECS: u64 = 300;

pub fn definition() -> ToolDef {
    ToolDef {
        name: "shell".to_string(),
        description: Some(
            "Execute a shell command and return its stdout and stderr. Runs via `sh -c`.\n\n\
             Optional `working_directory`: an absolute path inside the active project. \
             If omitted, the command runs in the session's current working directory \
             (which itself is inside the project root)."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute."
                },
                "working_directory": {
                    "type": "string",
                    "description": "Optional. Absolute path to use as the command's working directory. \
                                    Must be inside the active project root; if it is not, \
                                    the tool returns an error."
                }
            },
            "required": ["command"]
        }),
    }
}

/// Execute the tool. Returns `(content, is_error, ctx_update)`.
pub async fn execute(
    input: &serde_json::Value,
    ctx: &ToolContext,
) -> (String, bool, ToolContextUpdate) {
    let command = match input.get("command").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => {
            return (
                "Missing required parameter: command".to_string(),
                true,
                ToolContextUpdate::default(),
            );
        }
    };

    // 1. Resolve the effective cwd. LLM-supplied wins; otherwise we
    //    use the session's current cwd. Either way it must validate
    //    through `assert_within_root` before we let `sh -c` use it.
    let requested = input
        .get("working_directory")
        .and_then(|v| v.as_str())
        .map(Path::new)
        .unwrap_or(&ctx.cwd);
    let validated_cwd = match assert_within_root(&ctx.project_root, requested) {
        Ok(p) => p,
        Err(e) => {
            return (
                format!(
                    "working_directory '{}' rejected: {}",
                    requested.display(),
                    e
                ),
                true,
                ToolContextUpdate::default(),
            );
        }
    };

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(TIMEOUT_SECS),
        tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&validated_cwd)
            .output(),
    )
    .await;

    let update = ToolContextUpdate {
        new_cwd: Some(validated_cwd),
    };

    match result {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            let mut result = String::new();
            if !stdout.is_empty() {
                result.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str("[stderr]\n");
                result.push_str(&stderr);
            }

            let exit_code = output.status.code().unwrap_or(-1);
            if !result.is_empty() {
                result.push_str(&format!("\n[exit code: {}]", exit_code));
            } else {
                result = format!("[exit code: {}]", exit_code);
            }

            let is_error = !output.status.success();
            (truncate_output(result), is_error, update)
        }
        Ok(Err(e)) => (
            format!("Failed to execute command: {}", e),
            true,
            update,
        ),
        Err(_) => (
            format!("Command timed out after {} seconds", TIMEOUT_SECS),
            true,
            update,
        ),
    }
}

/// Truncate output exceeding MAX_OUTPUT_BYTES (head + tail, omit middle).
fn truncate_output(s: String) -> String {
    if s.len() <= MAX_OUTPUT_BYTES {
        return s;
    }
    let head_end = 25 * 1024;
    let tail_start = s.len() - 25 * 1024;
    let omitted = s.len() - MAX_OUTPUT_BYTES;
    format!(
        "{}\n<truncated: omitted {} bytes>\n{}",
        &s[..head_end],
        omitted,
        &s[tail_start..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_ctx(tmp: &tempfile::TempDir) -> ToolContext {
        ToolContext {
            project_root: tmp.path().canonicalize().unwrap(),
            cwd: tmp.path().canonicalize().unwrap(),
        }
    }

    #[test]
    fn definition_has_correct_name() {
        assert_eq!(definition().name, "shell");
    }

    #[test]
    fn definition_documents_working_directory() {
        let schema = &definition().input_schema;
        let props = schema.get("properties").unwrap();
        assert!(props.get("working_directory").is_some());
    }

    #[tokio::test]
    async fn execute_echo() {
        let tmp = tempfile::tempdir().unwrap();
        let (content, is_error, update) =
            execute(&serde_json::json!({"command": "echo hello"}), &test_ctx(&tmp)).await;
        assert!(!is_error);
        assert!(content.contains("hello"));
        assert!(content.contains("[exit code: 0]"));
        // Update carries the validated cwd.
        assert!(update.new_cwd.is_some());
    }

    #[tokio::test]
    async fn execute_stderr_command() {
        let tmp = tempfile::tempdir().unwrap();
        let (content, is_error, _) = execute(
            &serde_json::json!({"command": "echo error >&2 && false"}),
            &test_ctx(&tmp),
        )
        .await;
        assert!(is_error);
        assert!(content.contains("error"));
    }

    #[tokio::test]
    async fn execute_missing_command_param() {
        let tmp = tempfile::tempdir().unwrap();
        let (msg, is_error, _) = execute(&serde_json::json!({}), &test_ctx(&tmp)).await;
        assert!(is_error);
        assert!(msg.contains("Missing required parameter"));
    }

    #[tokio::test]
    async fn execute_respects_working_directory_inside_root() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        let ctx = test_ctx(&tmp);

        let (content, is_error, update) = execute(
            &serde_json::json!({
                "command": "pwd",
                "working_directory": ctx.project_root.join("sub").to_string_lossy(),
            }),
            &ctx,
        )
        .await;
        assert!(!is_error, "{}", content);
        let update_cwd = update.new_cwd.expect("update carries new cwd");
        assert_eq!(
            update_cwd,
            ctx.project_root.join("sub").canonicalize().unwrap()
        );
    }

    #[tokio::test]
    async fn execute_rejects_working_directory_outside_root() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = test_ctx(&tmp);
        let (msg, is_error, update) = execute(
            &serde_json::json!({
                "command": "ls",
                "working_directory": "/etc",
            }),
            &ctx,
        )
        .await;
        assert!(is_error);
        assert!(
            msg.contains("outside project root") || msg.contains("rejected"),
            "expected rejection, got: {}",
            msg
        );
        // Update must be empty so the agent loop does not persist
        // a bogus cwd.
        assert!(update.new_cwd.is_none());
    }

    #[tokio::test]
    async fn execute_rejects_nonexistent_working_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = test_ctx(&tmp);
        let (msg, is_error, _) = execute(
            &serde_json::json!({
                "command": "ls",
                "working_directory": ctx
                    .project_root
                    .join("nope")
                    .to_string_lossy()
                    .into_owned(),
            }),
            &ctx,
        )
        .await;
        assert!(is_error);
        assert!(msg.contains("rejected") || msg.contains("cannot be resolved"));
    }

    /// Defensive: when ctx.cwd is itself outside the project root
    /// (which the agent loop should never construct), the boundary
    /// check still rejects the operation. This guards against a
    /// future regression where some caller passes a stale ctx.
    #[tokio::test]
    async fn execute_rejects_when_ctx_cwd_outside_root() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ToolContext {
            project_root: tmp.path().canonicalize().unwrap(),
            cwd: PathBuf::from("/etc"),
        };
        let (msg, is_error, _) = execute(&serde_json::json!({"command": "pwd"}), &ctx).await;
        assert!(is_error);
        assert!(msg.contains("rejected") || msg.contains("outside"));
    }
}
