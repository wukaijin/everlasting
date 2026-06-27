//! L1a — `run_background_shell` tool.
//!
//! Fire-and-forget alternative to the synchronous `shell` tool: starts
//! a long-running command (`npm run build`, `pnpm install`, large test
//! suite, dev server) and returns immediately with a `shell_session_id`
//! the agent loop + LLM use to query / kill the process later. The
//! completion notification is injected at the start of the next
//! agent-loop turn (see `agent::chat_loop::run_chat_loop`).
//!
//! # Why a separate tool (vs. reusing `shell` with a `background: true` flag)
//!
//! Three reasons, mirroring opencode-pty / Hermes (see
//! `docs/spikes/2026-06-19-async-parallel-tool-research.md` §2.1):
//!
//! 1. **Wire-shape clarity** — the LLM knows the call returns a
//!    handle, not a result. No need for a `background` discriminator
//!    inside the existing shell payload.
//! 2. **Permission symmetry** — `run_background_shell` is registered
//!    as a `ToolKind::Shell` tool in the ⑨ 关 permission layer (see
//!    `permissions::classify_tool`), so the same kill-list / prefix
//!    classification / `permission:ask` flow applies. No new
//!    branch logic.
//! 3. **Lifecycle asymmetry** — background shells survive the
//!    `execute_tool` call and outlive the agent loop. The ⑨ 关
//!    `PermissionStore` lookup needs the SAME `(session_id, tool_name)`
//!    pair as `shell` for "始终允许" grants to apply uniformly; a
//!    different `tool_name` would create a parallel grant namespace.
//!
//! # Boundary / env / process-group invariants
//!
//! - **`working_directory`** is validated through
//!   `projects::boundary::assert_within_root` against the active
//!   project root. A failure returns `is_error: true` so the LLM
//!   self-corrects (matches the synchronous `shell` tool's UX).
//! - **Safe env** (RULE-E-001) — the spawned child inherits only
//!   the curated allowlist (PATH + HOME / USER / LANG-family /
//!   TERM / TZ / TMPDIR). API keys / tokens are NOT inherited.
//! - **Process group** (RULE-E-002) — the child is the leader of a
//!   new process group; `shell_kill` + `kill_all_for_session` +
//!   app shutdown all SIGKILL the entire group so descendants of
//!   `&` / `nohup` / pipelines are reaped along with the direct
//!   child. These rules are owned by the registry impl
//!   (`background_shell::in_memory`) — this tool layer just
//!   delegates.
//!
//! # Lifetime
//!
//! - One call returns one `shell_session_id` (format `bsh_<uuid>`).
//! - The child + the registry entry persist across
//!   `invoke("chat")` boundaries (the registry is held on `AppState`,
//!   not per-request).
//! - Session delete → `kill_all_for_session` (best-effort, doesn't
//!   block the IPC response).
//! - App exit → `RunEvent::Exit` hook calls `kill_all`.

use std::path::Path;

use crate::background_shell::BackgroundShellRegistry;
use crate::llm::types::ToolDef;
use crate::projects::boundary::assert_within_root;
use crate::tools::{ToolContext, ToolContextUpdate};

/// L1a (PRD Q6) — default max-runtime in ms. 24 hours; "过夜 build"
/// covers almost every realistic scenario. The LLM can raise this
/// per-call via the `max_runtime_ms` parameter.
pub(crate) const DEFAULT_MAX_RUNTIME_MS: u64 = 86_400_000;

/// `run_background_shell` tool definition (registered in
/// `builtin_tools()`).
pub fn definition() -> ToolDef {
    ToolDef {
        name: "run_background_shell".to_string(),
        description: Some(
            "Start a shell command in the background and return immediately with a \
             `shell_session_id` handle. Use this for long-running commands (full builds, \
             package installs, large test suites, dev servers) that exceed the synchronous \
             `shell` tool's 600-second timeout cap. The command runs in the project root by \
             default, or in `working_directory` if provided (must be inside the active \
             project).\n\n\
             When the background command exits, the agent loop injects a system-style user \
             message at the start of the next turn — you then call `shell_status` to read \
             the output, or `shell_kill` to terminate it early.\n\n\
             Optional `max_runtime_ms`: maximum execution time in milliseconds before the \
             process group is automatically killed. Default: 86400000 (24 hours, no upper \
             cap; the timer exists so a forgotten background build doesn't run forever).\n\n\
             Output handling (same as the synchronous `shell` tool): outputs > 30 KB are \
             saved to `<cwd>/.everlasting/outputs/<id>.txt`; the status response then carries \
             the path plus a 1 KB head+tail preview.\n\n\
             Environment is restricted to a safe allowlist \
             (PATH/HOME/USER/LOGNAME/LANG/LANGUAGE/LC_ALL/TERM/TZ/TMPDIR). \
             API keys and tokens from the agent process are NOT inherited."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute in the background. Runs via `sh -c`."
                },
                "working_directory": {
                    "type": "string",
                    "description": "Optional. Absolute path to use as the command's working directory. \
                                    Must be inside the active project root; if it is not, \
                                    the tool returns an error."
                },
                "max_runtime_ms": {
                    "type": "integer",
                    "description": "Optional. Maximum execution time in milliseconds before the \
                                    process group is automatically killed. Default: 86400000 \
                                    (24 hours). No upper cap; set a lower value for known-short \
                                    commands."
                }
            },
            "required": ["command"]
        }),
    }
}

/// Execute the tool. Returns `(content, is_error, ctx_update)`.
///
/// Flow:
/// 1. Parse `command` (required) + optional `working_directory` +
///    optional `max_runtime_ms`.
/// 2. Resolve the effective cwd via `assert_within_root` against
///    the active project's root (mirrors the synchronous `shell`
///    tool's pre-check).
/// 3. Pull the `DefaultRegistry` out of `ctx.background_shells`
///    (threaded from `AppState` by the agent loop).
/// 4. `registry.start(session_id, command, validated_cwd,
///    max_runtime_ms)` returns the new `shell_session_id`.
/// 5. Format a confirmation string the LLM reads.
///
/// `session_id` is the chat session id (NOT the bg shell id);
/// the registry uses it as the outer key of the
/// `(session_id, shell_id)` map, enforcing Q7's session-scoping.
pub async fn execute(
    input: &serde_json::Value,
    ctx: &ToolContext,
    session_id: Option<&str>,
) -> (String, bool, ToolContextUpdate) {
    let command = match input.get("command").and_then(|v| v.as_str()) {
        Some(c) => c.to_string(),
        None => {
            return (
                "Missing required parameter: command".to_string(),
                true,
                ToolContextUpdate::default(),
            );
        }
    };

    // 1. Resolve effective cwd (mirrors `shell.rs`).
    let requested = input
        .get("working_directory")
        .and_then(|v| v.as_str())
        .map(Path::new)
        .unwrap_or(&ctx.cwd);
    let validated_cwd = match assert_within_root(&ctx.worktree_path, requested) {
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

    // 2. Parse max_runtime_ms. Zero or negative → default; no
    //    upper clamp (PRD Q6 decision: "no upper cap").
    let max_runtime_ms: Option<u64> = input
        .get("max_runtime_ms")
        .and_then(|v| v.as_i64())
        .map(|n| if n <= 0 { DEFAULT_MAX_RUNTIME_MS } else { n as u64 });

    // 3. Session-scope check: the tool layer always has `session_id`
    //    from the dispatch (`tools/mod.rs::execute_tool` passes it
    //    through). A None is a defensive no-op (we cannot
    //    session-scope without it; the registry would reject too).
    let chat_session_id = match session_id {
        Some(s) => s,
        None => {
            return (
                "run_background_shell called without a session_id; this is a bug."
                    .to_string(),
                true,
                ToolContextUpdate::default(),
            );
        }
    };

    // 4. Start the background shell.
    match ctx
        .background_shells
        .start(chat_session_id, command.clone(), validated_cwd.clone(), max_runtime_ms)
        .await
    {
        Ok(shell_session_id) => (
            format!(
                "Started background shell {shell_session_id} (cwd: {}). Use \
                 `shell_status` to query progress, or `shell_kill` to terminate. \
                 When it finishes, you will see a `[system] 后台 shell ... 已完成...` \
                 message at the start of your next turn.",
                validated_cwd.display()
            ),
            false,
            ToolContextUpdate {
                // Mirror `shell::execute`: surface the validated cwd so
                // the agent loop persists it on turn end.
                new_cwd: Some(validated_cwd),
            },
        ),
        Err(crate::background_shell::BackgroundShellError::Spawn(e)) => (
            format!("Failed to spawn background shell: {}", e),
            true,
            ToolContextUpdate::default(),
        ),
        Err(crate::background_shell::BackgroundShellError::InvalidCwd { path, reason }) => (
            format!("Invalid working_directory '{}': {}", path, reason),
            true,
            ToolContextUpdate::default(),
        ),
        Err(e) => (
            format!("Failed to start background shell: {}", e),
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
        assert_eq!(definition().name, "run_background_shell");
    }

    #[test]
    fn definition_documents_max_runtime() {
        let def = definition();
        let schema = &def.input_schema;
        let props = schema.get("properties").expect("schema has properties");
        assert!(props.get("max_runtime_ms").is_some());
        assert!(props.get("command").is_some());
        assert!(props.get("working_directory").is_some());
        // `command` is the only required key.
        let required = schema.get("required").expect("required list");
        assert_eq!(required, &serde_json::json!(["command"]));
    }

    #[tokio::test]
    async fn execute_starts_a_background_shell() {
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(&tmp);
        let (content, is_error, update) = execute(
            &serde_json::json!({"command": "echo hello-bg"}),
            &ctx,
            Some("chat-session-1"),
        )
        .await;
        assert!(!is_error, "{}", content);
        assert!(content.contains("Started background shell"));
        assert!(content.contains("bsh_"));
        assert!(content.contains(tmp.path().canonicalize().unwrap().to_string_lossy().as_ref()));
        // Update carries the validated cwd (matches shell::execute UX).
        assert!(update.new_cwd.is_some());
    }

    #[tokio::test]
    async fn execute_missing_command_returns_error() {
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(&tmp);
        let (content, is_error, update) = execute(&serde_json::json!({}), &ctx, Some("s1")).await;
        assert!(is_error);
        assert!(content.contains("Missing required parameter"));
        // No update when the call fails early.
        assert!(update.new_cwd.is_none());
    }

    #[tokio::test]
    async fn execute_rejects_outside_root_cwd() {
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(&tmp);
        let (content, is_error, _) = execute(
            &serde_json::json!({
                "command": "ls",
                "working_directory": "/etc",
            }),
            &ctx,
            Some("s1"),
        )
        .await;
        assert!(is_error, "{}", content);
        assert!(
            content.contains("rejected") || content.contains("outside"),
            "expected rejection, got: {}",
            content
        );
    }

    #[tokio::test]
    async fn execute_without_session_id_returns_bug_marker() {
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(&tmp);
        let (content, is_error, _) = execute(
            &serde_json::json!({"command": "echo hi"}),
            &ctx,
            None,
        )
        .await;
        assert!(is_error);
        assert!(content.contains("without a session_id"));
    }

    /// The shell_id returned by `start` is a real, queryable
    /// background shell — calling `shell_status` (or
    /// `BackgroundShellRegistry::status` directly) on it from the
    /// same `ToolContext` registry must succeed.
    #[tokio::test]
    async fn returned_shell_id_is_queryable_via_registry() {
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(&tmp);
        let (content, is_error, _) = execute(
            &serde_json::json!({"command": "echo hi"}),
            &ctx,
            Some("s1"),
        )
        .await;
        assert!(!is_error, "{}", content);
        // The format string is `"Started background shell {id} (cwd: ...)"`,
        // so the shell id is the 4th whitespace-separated token
        // ("Started" / "background" / "shell" / "{id}" / "(cwd:" ...).
        let shell_id = content
            .split_whitespace()
            .nth(3)
            .filter(|tok| tok.starts_with("bsh_"))
            .map(str::to_string)
            .expect("shell id should appear in content");
        let status = ctx
            .background_shells
            .status("s1", &shell_id)
            .await
            .expect("status of freshly-started shell");
        // Either Running or Completed (echo finishes in <50ms; the
        // status is racy either way, both are valid "shell exists").
        assert!(
            matches!(
                status,
                crate::background_shell::BackgroundShellStatus::Running { .. }
                    | crate::background_shell::BackgroundShellStatus::Completed { .. }
            ),
            "expected Running or Completed, got: {:?}",
            status
        );
    }
}