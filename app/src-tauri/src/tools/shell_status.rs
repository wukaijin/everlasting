//! L1a — `shell_status` tool.
//!
//! Query a background shell started by [`run_background_shell`]. Returns
//! a human-readable summary the LLM reads to decide whether the
//! process is still running, completed (with stdout / stderr previews +
//! optional spill path), or killed / timed out.
//!
//! # Why a separate tool (vs. folding into `run_background_shell`)
//!
//! Hermes' pattern: a `terminal(background=true)` call returns a
//! session_id; a follow-up `process(action=poll|wait|log|kill|write)`
//! manages the lifecycle. We split it the same way (`shell_status` +
//! `shell_kill`) so each tool's input schema is small, the LLM's
//! tool-selection is unambiguous, and the registry can be queried
//! idempotently (multiple `shell_status` calls in one turn are fine
//! — the registry's status read is a lock-only op).
//!
//! # Wire name (`session_id` vs the chat session id)
//!
//! Per the L1 PRD wording, the LLM-facing field on the input schema
//! is `session_id`. Internally this is the *background shell's* id
//! (NOT the chat session id). The tool layer translates using
//! `ctx.background_shells` + the chat `session_id` arg (always
//! provided by the dispatch). The rename is intentional: the LLM
//! shouldn't have to track two separate "session" concepts for two
//! different namespaces — the background shell is just "the
//! background shell you started".
//!
//! # Session-scoping (PRD Q7)
//!
//! The registry keys shells by `(chat_session_id, shell_session_id)`,
//! so an LLM in session A can never see / kill a shell from session
//! B. `NotFound` is returned for cross-session access — surfaced as
//! `is_error: true` so the LLM learns the right scope.

use crate::background_shell::BackgroundShellRegistry;
use crate::background_shell::BackgroundShellStatus;
use crate::llm::types::ToolDef;
use crate::tools::{ToolContext, ToolContextUpdate};

/// `shell_status` tool definition (registered in `builtin_tools()`).
pub fn definition() -> ToolDef {
    ToolDef {
        name: "shell_status".to_string(),
        description: Some(
            "Query the status of a background shell started by `run_background_shell`. \
             Pass the `shell_session_id` (the `bsh_<uuid>` handle) returned by the start \
             call. Returns the current state: `running` (still executing), `completed` \
             (exited normally; carries stdout/stderr previews plus an optional full-output \
             path when the output exceeded 30 KB), or `killed` (terminated by `shell_kill`, \
             session delete, app shutdown, or `max_runtime_ms` timeout). Output > 30 KB is \
             saved to `<cwd>/.everlasting/outputs/<id>.txt`; the response then includes the \
             path so you can `read_file` the full output.\n\n\
             Call this after the `[system] 后台 shell ... 已完成...` notification arrives, \
             or while the shell is still running to poll progress."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": {
                    "type": "string",
                    "description": "The `shell_session_id` (e.g. `bsh_abc123...`) returned by the `run_background_shell` call that started this background shell."
                }
            },
            "required": ["session_id"]
        }),
    }
}

/// Execute the tool. Returns `(content, is_error, ctx_update)`.
///
/// `session_id` (the dispatch arg) is the chat session id. The
/// LLM-supplied `input["session_id"]` is the *background shell's*
/// id (different namespace — see module doc).
pub async fn execute(
    input: &serde_json::Value,
    ctx: &ToolContext,
    chat_session_id: Option<&str>,
) -> (String, bool, ToolContextUpdate) {
    let shell_id = match input.get("session_id").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => {
            return (
                "Missing required parameter: session_id".to_string(),
                true,
                ToolContextUpdate::default(),
            );
        }
    };

    let chat_sid = match chat_session_id {
        Some(s) => s,
        None => {
            return (
                "shell_status called without a chat session_id; this is a bug.".to_string(),
                true,
                ToolContextUpdate::default(),
            );
        }
    };

    match ctx.background_shells.status(chat_sid, shell_id).await {
        Ok(status) => (format_status(&status, shell_id), false, ToolContextUpdate::default()),
        Err(crate::background_shell::BackgroundShellError::NotFound { .. }) => (
            format!(
                "Background shell {} not found. It may have been cleaned up, never existed, or belong to a different chat session.",
                shell_id
            ),
            true,
            ToolContextUpdate::default(),
        ),
        Err(crate::background_shell::BackgroundShellError::WrongSession { .. }) => (
            format!(
                "Background shell {} is not owned by this chat session.",
                shell_id
            ),
            true,
            ToolContextUpdate::default(),
        ),
        Err(e) => (
            format!("Failed to query background shell: {}", e),
            true,
            ToolContextUpdate::default(),
        ),
    }
}

/// Render a [`BackgroundShellStatus`] as an LLM-friendly string.
///
/// Format choice: one line per field, human-readable, prefix-free
/// (no JSON wrapping). The LLM reads it directly; the agent loop
/// just passes it through as `tool_result.content`.
fn format_status(status: &BackgroundShellStatus, shell_id: &str) -> String {
    match status {
        BackgroundShellStatus::Running {
            started_at,
            elapsed_ms,
        } => {
            format!(
                "Background shell {shell_id}: running\n\
                 started_at: {started_at} ms (process boot)\n\
                 elapsed_ms: {elapsed_ms}"
            )
        }
        BackgroundShellStatus::Completed {
            exit_code,
            completed_at,
            stdout_preview,
            stderr_preview,
            full_output_path,
        } => {
            let mut s = format!(
                "Background shell {shell_id}: completed\n\
                 exit_code: {exit_code}\n\
                 completed_at: {completed_at} ms (process boot)\n\
                 stdout_preview:\n{stdout_preview}\n\
                 stderr_preview:\n{stderr_preview}"
            );
            if let Some(path) = full_output_path {
                s.push_str(&format!(
                    "\nfull_output_path: {path}\n\
                     (output exceeded 30 KB; call read_file on this path to see the rest)"
                ));
            }
            s
        }
        BackgroundShellStatus::Killed {
            exit_code,
            completed_at,
        } => {
            format!(
                "Background shell {shell_id}: killed\n\
                 exit_code: {exit_code}\n\
                 completed_at: {completed_at} ms (process boot)\n\
                 (killed by shell_kill, session delete, app shutdown, or max_runtime_ms timeout)"
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_ctx() -> (ToolContext, tempfile::TempDir) {
        let tmp = tempdir().unwrap();
        let ctx = ToolContext {
            worktree_path: tmp.path().canonicalize().unwrap(),
            cwd: tmp.path().canonicalize().unwrap(),
            checklist: crate::tools::update_checklist::new_handle(),
            background_shells: crate::background_shell::default_registry(),
        };
        (ctx, tmp)
    }

    #[test]
    fn definition_has_correct_name() {
        assert_eq!(definition().name, "shell_status");
    }

    #[test]
    fn definition_schema_requires_session_id() {
        let schema = &definition().input_schema;
        let required = schema.get("required").expect("required list");
        assert_eq!(required, &serde_json::json!(["session_id"]));
    }

    #[tokio::test]
    async fn execute_missing_session_id_returns_error() {
        let (ctx, _tmp) = test_ctx();
        let (content, is_error, _) =
            execute(&serde_json::json!({}), &ctx, Some("s1")).await;
        assert!(is_error);
        assert!(content.contains("Missing required parameter"));
    }

    #[tokio::test]
    async fn execute_unknown_shell_returns_not_found_error() {
        let (ctx, _tmp) = test_ctx();
        let (content, is_error, _) = execute(
            &serde_json::json!({"session_id": "bsh_does_not_exist"}),
            &ctx,
            Some("s1"),
        )
        .await;
        assert!(is_error);
        assert!(content.contains("not found"));
    }

    #[tokio::test]
    async fn execute_without_chat_session_id_returns_bug_marker() {
        let (ctx, _tmp) = test_ctx();
        let (content, is_error, _) = execute(
            &serde_json::json!({"session_id": "bsh_anything"}),
            &ctx,
            None,
        )
        .await;
        assert!(is_error);
        assert!(content.contains("without a chat session_id"));
    }

    /// Round-trip: start a background shell via the registry, then
    /// `shell_status` it from the same `ToolContext`. The status
    /// must report `running` (echo finishes fast but the call may
    /// race — accept either Running or Completed).
    #[tokio::test]
    async fn execute_round_trip_returns_running_or_completed() {
        let (ctx, tmp) = test_ctx();
        let shell_id = ctx
            .background_shells
            .start(
                "s1",
                "echo round-trip".to_string(),
                tmp.path().to_path_buf(),
                Some(5000),
            )
            .await
            .expect("start");
        let (content, is_error, _) = execute(
            &serde_json::json!({"session_id": shell_id}),
            &ctx,
            Some("s1"),
        )
        .await;
        assert!(!is_error, "{}", content);
        assert!(content.contains(&shell_id));
        assert!(
            content.contains("running") || content.contains("completed"),
            "expected running/completed, got: {}",
            content
        );
    }

    /// Cross-session isolation (PRD Q7). The shell belongs to
    /// `chat_session_id = "s1"`. `shell_status` called with
    /// `chat_session_id = "s2"` returns `is_error: true`.
    #[tokio::test]
    async fn execute_cross_session_returns_error() {
        let (ctx, tmp) = test_ctx();
        let shell_id = ctx
            .background_shells
            .start(
                "s1",
                "sleep 30".to_string(),
                tmp.path().to_path_buf(),
                Some(60_000),
            )
            .await
            .expect("start");
        let (content, is_error, _) = execute(
            &serde_json::json!({"session_id": shell_id}),
            &ctx,
            Some("s2"),
        )
        .await;
        assert!(is_error, "{}", content);
        assert!(
            content.contains("not found"),
            "cross-session access should look like not-found, got: {}",
            content
        );
        // Cleanup.
        let _ = ctx.background_shells.kill("s1", &shell_id).await;
    }

    #[test]
    fn format_running_status_includes_id_and_elapsed() {
        let s = format_status(
            &BackgroundShellStatus::Running {
                started_at: 1000,
                elapsed_ms: 2500,
            },
            "bsh_abc",
        );
        assert!(s.contains("bsh_abc"));
        assert!(s.contains("running"));
        assert!(s.contains("2500"));
    }

    #[test]
    fn format_completed_status_includes_stdout_preview_and_path() {
        let s = format_status(
            &BackgroundShellStatus::Completed {
                exit_code: 0,
                completed_at: 5000,
                stdout_preview: "hello".into(),
                stderr_preview: String::new(),
                full_output_path: Some("/tmp/.everlasting/outputs/abc.txt".into()),
            },
            "bsh_abc",
        );
        assert!(s.contains("completed"));
        assert!(s.contains("exit_code: 0"));
        assert!(s.contains("hello"));
        assert!(s.contains("/tmp/.everlasting/outputs/abc.txt"));
    }

    #[test]
    fn format_killed_status_includes_exit_code() {
        let s = format_status(
            &BackgroundShellStatus::Killed {
                exit_code: -1,
                completed_at: 9000,
            },
            "bsh_abc",
        );
        assert!(s.contains("killed"));
        assert!(s.contains("exit_code: -1"));
    }
}