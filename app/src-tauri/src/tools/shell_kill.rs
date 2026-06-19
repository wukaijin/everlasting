//! L1a — `shell_kill` tool.
//!
//! Force-terminate a background shell started by
//! [`run_background_shell`]. SIGKILLs the entire process group
//! (RULE-E-002), so descendants of `&` / `nohup` / pipelines are
//! reaped along with the direct child.
//!
//! # Idempotency
//!
//! `shell_kill` on an already-completed / already-killed shell
//! returns `Ok` (matches the registry's `kill` contract). The LLM
//! may call it any time — common pattern: call `shell_status`
//! first, then `shell_kill` if the output is no longer needed.
//!
//! # Lifecycle hooks beyond this tool
//!
//! The registry's `kill` is one of three SIGKILL triggers; the other
//! two are out-of-band from the agent loop:
//!
//! - `delete_session` (Tauri command) → `kill_all_for_session`.
//!   Triggered by the user clicking "Delete Session".
//! - App shutdown (`RunEvent::Exit`) → `kill_all`. Triggered when
//!   the Tauri window closes; nothing should leak.
//!
//! See `commands/sessions.rs::delete_session` and `lib.rs::run` for
//! the hook wiring.
//!
//! # Session-scoping (PRD Q7)
//!
//! Like [`shell_status`], the registry keys shells by
//! `(chat_session_id, shell_session_id)`. A wrong-session access
//! returns `is_error: true` so the LLM learns the right scope.

use crate::background_shell::BackgroundShellRegistry;
use crate::llm::types::ToolDef;
use crate::tools::{ToolContext, ToolContextUpdate};

/// `shell_kill` tool definition (registered in `builtin_tools()`).
pub fn definition() -> ToolDef {
    ToolDef {
        name: "shell_kill".to_string(),
        description: Some(
            "Force-terminate a background shell started by `run_background_shell`. \
             Pass the `shell_session_id` (the `bsh_<uuid>` handle). The entire process group \
             is SIGKILLed, so backgrounded descendants are reaped along with the direct child.\n\n\
             Idempotent: killing an already-completed shell is a no-op (returns success). \
             Use this when a long-running command is no longer needed — for example, the \
             build is taking too long and you want to try a different approach."
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
/// `chat_session_id` is the chat session id (the dispatch arg,
/// always provided by the dispatch layer). `input["session_id"]`
/// is the *background shell's* id (LLM-facing — see module doc on
/// [`shell_status`]).
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
                "shell_kill called without a chat session_id; this is a bug.".to_string(),
                true,
                ToolContextUpdate::default(),
            );
        }
    };

    match ctx.background_shells.kill(chat_sid, shell_id).await {
        Ok(()) => (
            format!("Killed background shell {shell_id}."),
            false,
            ToolContextUpdate::default(),
        ),
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
            format!("Failed to kill background shell: {}", e),
            true,
            ToolContextUpdate::default(),
        ),
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
        assert_eq!(definition().name, "shell_kill");
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

    /// Round-trip: start a bg shell, kill it, confirm the registry
    /// surfaces it as `Killed`.
    #[tokio::test]
    async fn execute_round_trip_kills_running_shell() {
        let (ctx, tmp) = test_ctx();
        let shell_id = ctx
            .background_shells
            .start(
                "s1",
                "sleep 60".to_string(),
                tmp.path().to_path_buf(),
                Some(120_000),
            )
            .await
            .expect("start");
        // Give the spawned task a moment to actually start the child.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let (content, is_error, _) = execute(
            &serde_json::json!({"session_id": shell_id}),
            &ctx,
            Some("s1"),
        )
        .await;
        assert!(!is_error, "{}", content);
        assert!(content.contains(&shell_id));
        // After the kill, status should report Killed.
        for _ in 0..40 {
            if matches!(
                ctx.background_shells.status("s1", &shell_id).await,
                Ok(crate::background_shell::BackgroundShellStatus::Killed { .. })
            ) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("shell did not transition to Killed within 2s");
    }

    /// Idempotency: killing an already-completed shell is a no-op
    /// success (no error). Mirrors the registry contract.
    #[tokio::test]
    async fn execute_on_completed_shell_is_idempotent() {
        let (ctx, tmp) = test_ctx();
        let shell_id = ctx
            .background_shells
            .start(
                "s1",
                "true".to_string(),
                tmp.path().to_path_buf(),
                Some(5000),
            )
            .await
            .expect("start");
        // Wait for completion.
        for _ in 0..40 {
            if matches!(
                ctx.background_shells.status("s1", &shell_id).await,
                Ok(crate::background_shell::BackgroundShellStatus::Completed { .. })
            ) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        // Now kill — must not error.
        let (content, is_error, _) = execute(
            &serde_json::json!({"session_id": shell_id}),
            &ctx,
            Some("s1"),
        )
        .await;
        assert!(!is_error, "{}", content);
    }

    /// Cross-session isolation (PRD Q7). The shell belongs to
    /// `chat_session_id = "s1"`. `shell_kill` called with
    /// `chat_session_id = "s2"` returns `is_error: true` and the
    /// shell stays running.
    #[tokio::test]
    async fn execute_cross_session_returns_error_and_does_not_kill() {
        let (ctx, tmp) = test_ctx();
        let shell_id = ctx
            .background_shells
            .start(
                "s1",
                "sleep 60".to_string(),
                tmp.path().to_path_buf(),
                Some(120_000),
            )
            .await
            .expect("start");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let (content, is_error, _) = execute(
            &serde_json::json!({"session_id": shell_id}),
            &ctx,
            Some("s2"),
        )
        .await;
        assert!(is_error, "{}", content);
        // Confirm the shell is still Running (cross-session kill
        // was rejected).
        let status = ctx
            .background_shells
            .status("s1", &shell_id)
            .await
            .expect("status of own shell");
        assert!(
            matches!(
                status,
                crate::background_shell::BackgroundShellStatus::Running { .. }
            ),
            "shell should still be running, got: {:?}",
            status
        );
        // Cleanup.
        let _ = ctx.background_shells.kill("s1", &shell_id).await;
    }
}