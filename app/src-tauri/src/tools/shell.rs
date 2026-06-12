//! `shell` tool — execute a shell command.
//!
//! Step 3b-1 changes:
//! - The LLM may optionally pass a `working_directory` field. The
//!   LLM-supplied value is **never trusted**: it is validated through
//!   `projects::boundary::assert_within_root` against
//!   `ctx.worktree_path` before being applied (评审 deepseek §4.1).
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
//!
//! Step toolset-extension changes (claude-code style 30K disk
//! spillover):
//! - If the command's combined output (stdout + stderr) is over 30 KB,
//!   the full output is written to
//!   `<ctx.cwd>/.everlasting/outputs/<uuid>.txt`. The tool_result
//!   that the LLM sees is a short message: a path to the spillover
//!   file plus a 1 KB head+tail preview so the LLM can decide whether
//!   to `read_file` the full output.
//! - The `.everlasting/outputs/` directory is created on demand and
//!   pruned on session delete (see `lib.rs::delete_session`).
//! - Output under 30 KB goes through the legacy head+tail 50 KB
//!   truncation unchanged (the 30K threshold is the claude-code
//!   "spill to disk" trigger; the 50K is the step 2 "still inline but
//!   head+tail" trigger — both apply in order).
//!
//! P0 enhancement (2026-06-12):
//! - `timeout` parameter (int, ms, default 120000, max 600000) lets
//!   the LLM set a per-command execution deadline. On timeout, the
//!   child is killed and partial output is returned with a timeout
//!   marker. This complements C1 CancellationToken (user cancel):
//!   timeout is automatic, cancel is manual.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::llm::types::ToolDef;
use crate::projects::boundary::assert_within_root;
use crate::tools::{ToolContext, ToolContextUpdate};

/// Max output before truncation (matches ARCHITECTURE.md §2.5.3).
const MAX_OUTPUT_BYTES: usize = 50 * 1024;
/// claude-code style threshold: outputs above this size spill to
/// disk and the LLM gets a path instead of the full text.
const DISK_SPILL_THRESHOLD: usize = 30 * 1024;
/// Preview size (head + tail) when we spill to disk. Keeps the
/// tool_result under ~1.5 KB so the agent's context stays small.
const PREVIEW_BYTES: usize = 1 * 1024;
/// Sub-directory under cwd where spilled outputs are written.
const SPILL_DIR: &str = ".everlasting/outputs";
/// Default command timeout in milliseconds (2 minutes).
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
/// Maximum allowed timeout in milliseconds (10 minutes).
const MAX_TIMEOUT_MS: u64 = 600_000;

/// Internal result from child process execution.
struct ShellResult {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit_code: i32,
    cancelled: bool,
    timed_out: bool,
}

/// Kill a child process and collect whatever output was produced.
async fn kill_and_collect(child: &mut Child) -> ShellResult {
    // Best-effort kill.
    let _ = child.kill().await;
    // Wait for the process to exit so we don't leave a zombie.
    let status = child.wait().await.ok();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_end(&mut stdout).await;
    }
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_end(&mut stderr).await;
    }
    ShellResult {
        stdout,
        stderr,
        exit_code: status.and_then(|s| s.code()).unwrap_or(-1),
        cancelled: true,
        timed_out: false,
    }
}

/// Format stdout + stderr into a single string.
fn format_output(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout_str = String::from_utf8_lossy(stdout);
    let stderr_str = String::from_utf8_lossy(stderr);
    let mut result = String::new();
    if !stdout_str.is_empty() {
        result.push_str(&stdout_str);
    }
    if !stderr_str.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str("[stderr]\n");
        result.push_str(&stderr_str);
    }
    result
}

pub fn definition() -> ToolDef {
    ToolDef {
        name: "shell".to_string(),
        description: Some(
            "Execute a shell command and return its stdout and stderr. Runs via `sh -c`.\n\n\
             Optional `working_directory`: an absolute path inside the active project. \
             If omitted, the command runs in the session's current working directory \
             (which itself is inside the project root).\n\n\
             Optional `timeout`: maximum execution time in milliseconds. Default: 120000 (2 min). \
             Maximum: 600000 (10 min). On timeout the command is killed and partial output \
             is returned with a `[timeout after Nms]` marker.\n\n\
             Outputs over 30 KB are saved to `<cwd>/.everlasting/outputs/<id>.txt`; \
             the tool returns the path plus a short preview so you can read the \
             full file with read_file."
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
                },
                "timeout": {
                    "type": "integer",
                    "description": "Optional. Maximum execution time in milliseconds. Default: 120000 (2 min). Max: 600000 (10 min). \
                                    On timeout the command is killed and partial output is returned."
                }
            },
            "required": ["command"]
        }),
    }
}

/// Execute the tool. Returns `(content, is_error, ctx_update)`.
///
/// `session_id` is currently unused by the shell tool itself, but we
/// keep it in the signature for parity with the other tools in
/// `mod.rs::execute_tool` — the dispatch is uniform.
///
/// C1 (Cancel): receives a `CancellationToken` so the child process
/// can be killed on cancel. The flow is:
/// 1. Spawn `sh -c <command>` as a background child process
/// 2. `tokio::select!` between `child.wait()` and `cancel.cancelled()`
/// 3. On cancel: `child.kill()` + collect partial stdout/stderr
/// 4. On normal completion: collect full output as before
pub async fn execute(
    input: &serde_json::Value,
    ctx: &ToolContext,
    _session_id: Option<&str>,
    cancel: &CancellationToken,
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

    // 2. Parse timeout parameter. Default 120s, max 600s. Zero or
    //    negative values use the default.
    let raw_timeout = input
        .get("timeout")
        .and_then(|v| v.as_i64())
        .unwrap_or(DEFAULT_TIMEOUT_MS as i64);
    let timeout_ms = if raw_timeout <= 0 {
        DEFAULT_TIMEOUT_MS
    } else {
        (raw_timeout as u64).min(MAX_TIMEOUT_MS)
    };

    // 3. Spawn the command. We use `sh -c` so the LLM can chain
    //    commands (`cmd1 && cmd2`, pipes, redirects). stdout AND
    //    stderr are captured so we can format the result.
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(command)
        .current_dir(&validated_cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return (
                format!("Failed to spawn command: {}", e),
                true,
                ToolContextUpdate::default(),
            );
        }
    };

    let update = ToolContextUpdate {
        new_cwd: Some(validated_cwd.clone()),
    };

    // 4. C1 + timeout: race between child completion, cancellation,
    //    and timeout. On cancel/timeout, kill the child and collect
    //    whatever output was produced so far.
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            tracing::info!("shell: cancellation requested, killing child process");
            kill_and_collect(&mut child).await
        }
        _ = tokio::time::sleep(std::time::Duration::from_millis(timeout_ms)) => {
            tracing::info!("shell: timeout after {}ms, killing child process", timeout_ms);
            let mut r = kill_and_collect(&mut child).await;
            r.timed_out = true;
            r.cancelled = false; // timeout, not cancel
            r
        }
        status = child.wait() => {
            match status {
                Ok(status) => {
                    let mut stdout = Vec::new();
                    let mut stderr = Vec::new();
                    // Best-effort read remaining output.
                    if let Some(mut out) = child.stdout.take() {
                        let _ = out.read_to_end(&mut stdout).await;
                    }
                    if let Some(mut err) = child.stderr.take() {
                        let _ = err.read_to_end(&mut stderr).await;
                    }
                    ShellResult {
                        stdout,
                        stderr,
                        exit_code: status.code().unwrap_or(-1),
                        cancelled: false,
                        timed_out: false,
                    }
                }
                Err(e) => {
                    return (
                        format!("Failed to execute command: {}", e),
                        true,
                        update,
                    );
                }
            }
        }
    };

    // 5. Format output.
    let mut combined = format_output(&result.stdout, &result.stderr);

    let exit_code = result.exit_code;
    if !combined.is_empty() {
        combined.push_str(&format!("\n[exit code: {}]", exit_code));
    } else {
        combined = format!("[exit code: {}]", exit_code);
    }

    let is_error = result.cancelled || result.timed_out || exit_code != 0;

    // 6. If cancelled, prepend marker.
    if result.cancelled {
        combined = format!("[cancelled, partial output]\n{}", combined);
        return (combined, true, update);
    }

    // 7. If timed out, prepend marker with the timeout duration.
    if result.timed_out {
        combined = format!("[timeout after {}ms, partial output]\n{}", timeout_ms, combined);
        return (combined, true, update);
    }

    // 8. Disk-spill: if output exceeds 30 KB, write the FULL output
    //    to a file under `<validated_cwd>/.everlasting/outputs/` and
    //    return a path + preview to the LLM.
    if combined.len() > DISK_SPILL_THRESHOLD {
        match spill_to_disk(&validated_cwd, &combined).await {
            Ok(path) => {
                let preview = head_tail_preview(&combined, PREVIEW_BYTES);
                let msg = format!(
                    "Output saved to {} ({} bytes). First/last {} preview:\n{}\n[exit code: {}]",
                    path.display(),
                    combined.len(),
                    PREVIEW_BYTES,
                    preview,
                    exit_code
                );
                return (msg, is_error, update);
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    cwd = %validated_cwd.display(),
                    "shell: disk spill failed; falling back to inline truncation"
                );
            }
        }
    }

    // 9. Inline path: apply the 50 KB head+tail truncation.
    (truncate_output(combined), is_error, update)
}

/// Write `contents` to `<cwd>/.everlasting/outputs/<uuid>.txt`,
/// creating the directory if needed. Returns the absolute path.
async fn spill_to_disk(cwd: &Path, contents: &str) -> std::io::Result<PathBuf> {
    let dir = cwd.join(SPILL_DIR);
    tokio::fs::create_dir_all(&dir).await?;
    let filename = format!("{}.txt", Uuid::new_v4());
    let path = dir.join(&filename);
    tokio::fs::write(&path, contents).await?;
    Ok(path)
}

/// Best-effort removal of `<cwd>/.everlasting/outputs/`. Called by
/// `lib.rs::delete_session` per PRD §R8 — when a user deletes a
/// session we sweep the disk-spilled shell outputs that were
/// written into that session's cwd. Failures are logged but never
/// returned: deleting the session is the user's primary intent;
/// disk cleanup is a side effect that should not block the delete
/// or surface a confusing error to the UI.
///
/// A missing directory is a no-op (the session never spilled
/// anything). We use `remove_dir_all` (not `remove_dir`) because
/// the directory may contain many `<uuid>.txt` files.
pub async fn cleanup_outputs_dir(cwd: &Path) {
    let dir = cwd.join(SPILL_DIR);
    if !dir.exists() {
        return;
    }
    if let Err(e) = tokio::fs::remove_dir_all(&dir).await {
        tracing::warn!(
            error = %e,
            cwd = %cwd.display(),
            spill_dir = %dir.display(),
            "shell: failed to clean up disk-spilled outputs on session delete"
        );
    }
}

/// Produce a head+tail preview of `s` for the disk-spill tool
/// result. Format: first `cap` bytes, then `\n...<truncated: N bytes>...\n`,
/// then last `cap` bytes.
fn head_tail_preview(s: &str, cap: usize) -> String {
    let len = s.len();
    if len <= cap * 2 + 64 {
        return s.to_string();
    }
    let head_end = cap;
    let tail_start = len - cap;
    let omitted = len - cap * 2;
    format!(
        "{}\n...<truncated: omitted {} bytes>...\n{}",
        &s[..head_end],
        omitted,
        &s[tail_start..]
    )
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
    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    fn test_ctx(tmp: &tempfile::TempDir) -> ToolContext {
        ToolContext {
            worktree_path: tmp.path().canonicalize().unwrap(),
            cwd: tmp.path().canonicalize().unwrap(),
        }
    }

    fn fresh_token() -> CancellationToken {
        CancellationToken::new()
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
        let tmp = tempdir().unwrap();
        let (content, is_error, update) = execute(
            &serde_json::json!({"command": "echo hello"}),
            &test_ctx(&tmp),
            None,
            &fresh_token(),
        )
        .await;
        assert!(!is_error);
        assert!(content.contains("hello"));
        assert!(content.contains("[exit code: 0]"));
        // Update carries the validated cwd.
        assert!(update.new_cwd.is_some());
    }

    #[tokio::test]
    async fn execute_stderr_command() {
        let tmp = tempdir().unwrap();
        let (content, is_error, _) = execute(
            &serde_json::json!({"command": "echo error >&2 && false"}),
            &test_ctx(&tmp),
            None,
            &fresh_token(),
        )
        .await;
        assert!(is_error);
        assert!(content.contains("error"));
    }

    #[tokio::test]
    async fn execute_missing_command_param() {
        let tmp = tempdir().unwrap();
        let (msg, is_error, _) = execute(
            &serde_json::json!({}),
            &test_ctx(&tmp),
            None,
            &fresh_token(),
        )
        .await;
        assert!(is_error);
        assert!(msg.contains("Missing required parameter"));
    }

    #[tokio::test]
    async fn execute_respects_working_directory_inside_root() {
        let tmp = tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        let ctx = test_ctx(&tmp);

        let (content, is_error, update) = execute(
            &serde_json::json!({
                "command": "pwd",
                "working_directory": ctx.worktree_path.join("sub").to_string_lossy(),
            }),
            &ctx,
            None,
            &fresh_token(),
        )
        .await;
        assert!(!is_error, "{}", content);
        let update_cwd = update.new_cwd.expect("update carries new cwd");
        assert_eq!(
            update_cwd,
            ctx.worktree_path.join("sub").canonicalize().unwrap()
        );
    }

    #[tokio::test]
    async fn execute_rejects_working_directory_outside_root() {
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(&tmp);
        let (msg, is_error, update) = execute(
            &serde_json::json!({
                "command": "ls",
                "working_directory": "/etc",
            }),
            &ctx,
            None,
            &fresh_token(),
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
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(&tmp);
        let (msg, is_error, _) = execute(
            &serde_json::json!({
                "command": "ls",
                "working_directory": ctx
                    .worktree_path
                    .join("nope")
                    .to_string_lossy()
                    .into_owned(),
            }),
            &ctx,
            None,
            &fresh_token(),
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
        let tmp = tempdir().unwrap();
        let ctx = ToolContext {
            worktree_path: tmp.path().canonicalize().unwrap(),
            cwd: PathBuf::from("/etc"),
        };
        let (msg, is_error, _) = execute(
            &serde_json::json!({"command": "pwd"}),
            &ctx,
            None,
            &fresh_token(),
        )
        .await;
        assert!(is_error);
        assert!(msg.contains("rejected") || msg.contains("outside"));
    }

    /// AC7.1: small output (< 30 KB) goes inline as before.
    #[tokio::test]
    async fn small_output_inline() {
        let tmp = tempdir().unwrap();
        let (content, is_error, _) = execute(
            &serde_json::json!({"command": "echo hello world"}),
            &test_ctx(&tmp),
            None,
            &fresh_token(),
        )
        .await;
        assert!(!is_error);
        assert!(content.contains("hello world"));
        // No disk-spill path in the message.
        assert!(!content.contains("Output saved to"));
    }

    /// AC7.2: output > 30 KB spills to disk and the result mentions
    /// the path and preview.
    #[tokio::test]
    async fn large_output_spills_to_disk() {
        let tmp = tempdir().unwrap();
        // Generate ~40 KB of stdout.
        let (content, is_error, _) = execute(
            &serde_json::json!({"command": "yes line | head -c 40000"}),
            &test_ctx(&tmp),
            None,
            &fresh_token(),
        )
        .await;
        assert!(!is_error, "{}", &content[..200.min(content.len())]);
        // Tool result should mention the saved file.
        assert!(content.contains("Output saved to"), "got: {}", &content[..300.min(content.len())]);
        assert!(content.contains(".everlasting/outputs/"));
        assert!(content.contains("preview"));
        // The actual file should exist on disk.
        // Parse the path out of the message.
        let path_line = content
            .lines()
            .find(|l| l.starts_with("Output saved to "))
            .expect("path line present");
        let path_str = path_line
            .trim_start_matches("Output saved to ")
            .split_whitespace()
            .next()
            .unwrap();
        let path = std::path::Path::new(path_str);
        assert!(path.exists(), "spill file should exist at {}", path.display());
        // The file should contain all the original output (not just preview).
        let saved = tokio::fs::read_to_string(path).await.unwrap();
        assert!(saved.len() > DISK_SPILL_THRESHOLD);
    }

    /// AC7.3: the `.everlasting/outputs/` directory is created on demand.
    #[tokio::test]
    async fn spill_creates_outputs_directory() {
        let tmp = tempdir().unwrap();
        let _ = execute(
            &serde_json::json!({"command": "yes x | head -c 40000"}),
            &test_ctx(&tmp),
            None,
            &fresh_token(),
        )
        .await;
        let dir = tmp.path().join(".everlasting/outputs");
        assert!(dir.exists());
        assert!(dir.is_dir());
    }

    /// C1: cancelling a long-running shell command kills the child
    /// and returns partial output with a cancellation marker.
    #[tokio::test]
    async fn cancel_kills_child_process() {
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(&tmp);
        let token = CancellationToken::new();
        let token_clone = token.clone();
        // Spawn a command that runs for 60 seconds.
        let handle = tokio::spawn(async move {
            execute(
                &serde_json::json!({"command": "sleep 60"}),
                &ctx,
                None,
                &token_clone,
            )
            .await
        });
        // Give the child a moment to start, then cancel.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        token.cancel();
        let (content, is_error, _) = handle.await.unwrap();
        assert!(is_error);
        assert!(
            content.contains("[cancelled, partial output]"),
            "expected cancel marker, got: {}",
            content
        );
    }

    /// C1: cancelling before the child even starts returns the
    /// cancel marker immediately.
    #[tokio::test]
    async fn cancel_before_spawn() {
        let tmp = tempdir().unwrap();
        let token = CancellationToken::new();
        token.cancel();
        let (content, is_error, _) = execute(
            &serde_json::json!({"command": "sleep 60"}),
            &test_ctx(&tmp),
            None,
            &token,
        )
        .await;
        assert!(is_error);
        assert!(
            content.contains("[cancelled, partial output]"),
            "expected cancel marker, got: {}",
            content
        );
    }

    /// head_tail_preview unit test — short input passes through.
    #[test]
    fn head_tail_preview_short() {
        let p = head_tail_preview("hello world", 100);
        assert_eq!(p, "hello world");
    }

    /// head_tail_preview unit test — long input gets head + tail.
    #[test]
    fn head_tail_preview_long() {
        let s = "a".repeat(5000);
        let p = head_tail_preview(&s, 100);
        assert!(p.contains("truncated"));
        assert!(p.starts_with('a'));
    }

    /// spill_to_disk creates the file and the parent directory.
    #[tokio::test]
    async fn spill_to_disk_creates_file() {
        let tmp = tempdir().unwrap();
        let path = spill_to_disk(tmp.path(), "the contents").await.unwrap();
        assert!(path.exists());
        let read = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(read, "the contents");
    }

    /// AC7.4: `cleanup_outputs_dir` removes the spill directory and
    /// everything under it. Mirrors the cleanup hook that
    /// `lib.rs::delete_session` invokes on session delete (PRD §R8).
    #[tokio::test]
    async fn cleanup_outputs_dir_removes_spill_dir() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join(".everlasting/outputs");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("a.txt"), "first").await.unwrap();
        tokio::fs::write(dir.join("b.txt"), "second").await.unwrap();
        // A nested file too — remove_dir_all must wipe the whole tree.
        tokio::fs::create_dir_all(dir.join("nested")).await.unwrap();
        tokio::fs::write(dir.join("nested").join("c.txt"), "third")
            .await
            .unwrap();
        assert!(dir.exists());
        assert!(dir.join("a.txt").exists());
        assert!(dir.join("nested").join("c.txt").exists());

        cleanup_outputs_dir(tmp.path()).await;

        assert!(!dir.exists(), "spill dir should be removed");
        assert!(!dir.join("a.txt").exists());
        assert!(!dir.join("nested").join("c.txt").exists());
    }

    /// AC7.4: `cleanup_outputs_dir` is a silent no-op when the
    /// spill directory doesn't exist (the session never spilled
    /// anything, or it was already cleaned up). Must not panic and
    /// must not log at error level.
    #[tokio::test]
    async fn cleanup_outputs_dir_noop_when_absent() {
        let tmp = tempdir().unwrap();
        // The `.everlasting/outputs/` directory was never created.
        // Calling cleanup must return immediately.
        cleanup_outputs_dir(tmp.path()).await;
        // Sanity: the (non-existent) dir is still non-existent.
        assert!(!tmp.path().join(".everlasting/outputs").exists());
    }

    /// AC7.4: `cleanup_outputs_dir` is best-effort and never panics,
    /// even when invoked on a cwd that itself doesn't exist
    /// (defensive: covers the race where the project was deleted
    /// between session creation and session delete).
    #[tokio::test]
    async fn cleanup_outputs_dir_silent_on_missing_cwd() {
        // Point at a path that is guaranteed not to exist.
        let bogus = std::path::Path::new("/this/path/does/not/exist/anywhere");
        // Must not panic; tracing::warn! fires (we don't assert on
        // it — log capture is out of scope for unit tests).
        cleanup_outputs_dir(bogus).await;
    }

    // Suppress "tempdir import not used in some branches" — every
    // test above uses it via test_ctx.
    #[allow(dead_code)]
    fn _unused() -> tempfile::TempDir {
        tempdir().unwrap()
    }

    // --- P0: timeout tests ---

    /// Shell with short timeout kills a long-running command.
    #[tokio::test]
    async fn timeout_kills_long_command() {
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(&tmp);
        let token = fresh_token();
        let (content, is_error, _) = execute(
            &serde_json::json!({
                "command": "sleep 60",
                "timeout": 500
            }),
            &ctx,
            None,
            &token,
        )
        .await;
        assert!(is_error);
        assert!(
            content.contains("[timeout after 500ms"),
            "expected timeout marker, got: {}",
            content
        );
        assert!(
            content.contains("partial output"),
            "expected partial output marker, got: {}",
            content
        );
    }

    /// Shell without timeout uses the default (120s). A fast command
    /// completes normally.
    #[tokio::test]
    async fn no_timeout_uses_default() {
        let tmp = tempdir().unwrap();
        let (content, is_error, _) = execute(
            &serde_json::json!({"command": "echo hello"}),
            &test_ctx(&tmp),
            None,
            &fresh_token(),
        )
        .await;
        assert!(!is_error, "{}", content);
        assert!(content.contains("hello"));
        assert!(!content.contains("timeout"), "should not have timeout marker");
    }

    /// timeout=0 is treated as default (120s). Fast command completes.
    #[tokio::test]
    async fn timeout_zero_treated_as_default() {
        let tmp = tempdir().unwrap();
        let (content, is_error, _) = execute(
            &serde_json::json!({
                "command": "echo ok",
                "timeout": 0
            }),
            &test_ctx(&tmp),
            None,
            &fresh_token(),
        )
        .await;
        assert!(!is_error, "{}", content);
        assert!(content.contains("ok"));
    }

    /// timeout=-1 is treated as default. Fast command completes.
    #[tokio::test]
    async fn timeout_negative_treated_as_default() {
        let tmp = tempdir().unwrap();
        let (content, is_error, _) = execute(
            &serde_json::json!({
                "command": "echo ok",
                "timeout": -1
            }),
            &test_ctx(&tmp),
            None,
            &fresh_token(),
        )
        .await;
        assert!(!is_error, "{}", content);
        assert!(content.contains("ok"));
    }

    /// timeout exceeding max is clamped. A fast command still completes.
    #[tokio::test]
    async fn timeout_exceeds_max_clamped() {
        let tmp = tempdir().unwrap();
        let (content, is_error, _) = execute(
            &serde_json::json!({
                "command": "echo clamped",
                "timeout": 999999999
            }),
            &test_ctx(&tmp),
            None,
            &fresh_token(),
        )
        .await;
        assert!(!is_error, "{}", content);
        assert!(content.contains("clamped"));
    }

    /// Timeout and cancel are distinct: timeout fires first, no cancel
    /// marker.
    #[tokio::test]
    async fn timeout_fires_before_cancel() {
        let tmp = tempdir().unwrap();
        let ctx = test_ctx(&tmp);
        let token = fresh_token();
        // Use a short timeout. Don't cancel — let timeout fire.
        let (content, is_error, _) = execute(
            &serde_json::json!({
                "command": "sleep 10",
                "timeout": 300
            }),
            &ctx,
            None,
            &token,
        )
        .await;
        assert!(is_error);
        assert!(
            content.contains("[timeout after 300ms"),
            "expected timeout marker, got: {}",
            content
        );
        assert!(
            !content.contains("[cancelled"),
            "should not have cancel marker when timeout fires"
        );
    }
}
