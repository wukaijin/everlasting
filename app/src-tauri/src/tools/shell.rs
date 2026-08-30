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
//! spillover; C6 2026-08-30 relocated + unified into
//! `tools/tool_output.rs`):
//! - If the command's combined output (stdout + stderr) is over 30 KB,
//!   the full output is written to
//!   `<ctx.data_dir>/outputs/<session_id>/<uuid>.txt` (out of the
//!   project tree — the old `<cwd>/.everlasting/outputs/` location
//!   polluted the agent's own search space and git status). The
//!   tool_result that the LLM sees is a short message: a path to the
//!   spillover file plus a 1 KB head+tail preview with the unified
//!   truncation marker, so the LLM can page through it with
//!   `read_file` offset/limit.
//! - The `<data_dir>/outputs/<session_id>/` directory is created on
//!   demand and pruned on session delete
//!   (`tool_output::sweep_session_outputs`); the legacy cwd-based
//!   `cleanup_outputs_dir` keeps sweeping pre-C6 spills best-effort.
//! - Output under 30 KB goes through the head+tail 50 KB truncation
//!   unchanged (the 30K threshold is the claude-code "spill to disk"
//!   trigger; the 50K is the "still inline but head+tail" trigger —
//!   both apply in order).
//! - Cancelled / timed-out partial output flows through the same
//!   spill+truncate treatment (pre-C6 those arms returned unbounded).
//!
//! P0 enhancement (2026-06-12):
//! - `timeout` parameter (int, ms, default 120000, max 600000) lets
//!   the LLM set a per-command execution deadline. On timeout, the
//!   child is killed and partial output is returned with a timeout
//!   marker. This complements C1 CancellationToken (user cancel):
//!   timeout is automatic, cancel is manual.
//!
//! P0 enhancement (2026-06-14 — RULE-E-001):
//! - The child process no longer inherits the agent's full
//!   environment. Before spawn we call `apply_safe_env`, which does
//!   `env_clear()` and re-injects only a curated allowlist
//!   (PATH/HOME/USER/LOGNAME/LANG-family/TERM/TZ/TMPDIR). This
//!   closes the leak where an LLM `env`/`printenv` could read
//!   `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / `*_TOKEN` / `*_SECRET`
//!   from the parent. See `.trellis/reviews/DEBT.md §RULE-E-001`.
//!
//! P0 enhancement (2026-06-14 — RULE-E-002):
//! - The child process is started as a new process group leader via
//!   `process_group(0)`. On cancel or timeout we kill the entire
//!   group (PGID = the sh PID) so grandchildren spawned by
//!   `sh -c "sleep 60 &"` / pipelines / `nohup` / `&` are also
//!   reaped, eliminating the orphan-process leak that
//!   `child.kill()` previously left behind. See
//!   `.trellis/reviews/DEBT.md §RULE-E-002`.
//!   Windows behaviour is unchanged (it stays on `child.kill()`);
//!   full Windows `CREATE_NEW_PROCESS_GROUP` is a follow-up.

use std::path::Path;
use std::process::Stdio;

use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;

use crate::llm::types::ToolDef;
use crate::projects::boundary::assert_within_root;
use crate::tools::tool_output::{self, Recovery, Unit};
use crate::tools::{ToolContext, ToolContextUpdate};

/// Legacy pre-C6 spill location under the session cwd. New spills go
/// to `<data_dir>/outputs/<session_id>/` (`tool_output::spill`); this
/// constant stays only for `cleanup_outputs_dir`, which sweeps the
/// legacy directory of sessions that spilled before the relocation.
pub(crate) const SPILL_DIR: &str = ".everlasting/outputs";
/// Default command timeout in milliseconds (2 minutes).
pub(crate) const DEFAULT_TIMEOUT_MS: u64 = 120_000;
/// Maximum allowed timeout in milliseconds (10 minutes).
pub(crate) const MAX_TIMEOUT_MS: u64 = 600_000;

/// Variables re-injected into the child process after `env_clear()`
/// (RULE-E-001). Adding a variable here is an intentional trust
/// decision: it becomes readable by every command the LLM runs.
/// API keys / tokens / secrets MUST stay out of this list.
pub(crate) const SAFE_ENV_VARS: &[&str] = &[
    "HOME", "USER", "LOGNAME", "LANG", "LANGUAGE", "LC_ALL", "TERM", "TZ", "TMPDIR",
];

/// Apply a safe-allowlist environment to `cmd`.
///
/// `pub(crate)` because L1's `background_shell::in_memory` reuses
/// the same env-allowlist rules for spawned background children —
/// the trait + impl share `apply_safe_env` so a future
/// safe-list change automatically applies to both sync `shell`
/// and `run_background_shell`.
///
/// `env_clear()` removes every inherited variable from the parent
/// (including `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / `*_TOKEN` /
/// `*_SECRET`). We then re-inject `PATH` (required for command
/// resolution) and the variables in [`SAFE_ENV_VARS`] (identity /
/// locale / terminal / timezone / temp-dir — most common dev
/// commands probe these).
///
/// The allowlist is intentionally minimal. Anything the LLM does
/// not need should not be readable by an arbitrary `sh -c`. Add
/// a variable to [`SAFE_ENV_VARS`] only when a concrete dev
/// command (`npm`, `cargo`, `pnpm`, `make`, `git`, `ls`, …) breaks
/// without it; document the reason in the commit message and add a
/// note to `docs/ARCHITECTURE.md` §"Tool execution" / §"Shell
/// env isolation" (this file currently has no dedicated subsection —
/// a new one will be added in a follow-up spec pass alongside
/// RULE-E-002 `process_group`).
pub(crate) fn apply_safe_env(cmd: &mut Command) {
    cmd.env_clear();
    // PATH is required for command resolution. Inherit from parent
    // when present; if missing (rare), the child inherits no PATH,
    // which will surface as "command not found" — acceptable since
    // the alternative is guessing a path that may not exist on this
    // machine.
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    for var in SAFE_ENV_VARS {
        if let Ok(v) = std::env::var(var) {
            cmd.env(var, v);
        }
    }
}

/// Internal result from child process execution.
pub(crate) struct ShellResult {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) exit_code: i32,
    pub(crate) cancelled: bool,
    pub(crate) timed_out: bool,
}

/// Kill the child process. Output collection is the caller's job:
/// the pipes are taken out and drained on spawned tasks BEFORE the
/// wait/kill select (see `execute`), so after the group kill closes
/// the write ends, those tasks complete with the partial output.
///
/// On Unix the child was spawned with `process_group(0)`, so the
/// `sh` process is the leader of a new process group whose PGID
/// equals `child.id()`. Killing the group with `kill(-pid, SIGKILL)`
/// reaches the `sh` shell AND any descendants it forked (`&` /
/// pipelines / `nohup`), closing the RULE-E-002 orphan-process
/// leak that the plain `child.kill().await` left behind. ESRCH
/// (process already exited) is treated as success; other kill
/// failures are logged at `warn!` level but never propagated to
/// the caller — the worst case is that a descendant lingers
/// briefly, which the eventual `child.wait()` below will
/// catch once stdout/stderr pipes close.
pub(crate) async fn kill_and_collect(child: &mut Child) -> ShellResult {
    // 1. Send the kill signal.
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            let pid_raw = pid as i32;
            // Negative pid => "send signal to the process group whose
            // PGID is |pid|". Safe because process_group(0) made
            // `pid` == PGID.
            let ret = unsafe { libc::kill(-pid_raw, libc::SIGKILL) };
            if ret != 0 {
                let errno = std::io::Error::last_os_error();
                if errno.raw_os_error() != Some(libc::ESRCH) {
                    tracing::warn!(
                        error = %errno,
                        pid = pid_raw,
                        "shell: killpg failed (non-ESRCH); descendant may linger"
                    );
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        // Windows path (MVP, not yet hardened per RULE-E-002). We
        // fall back to tokio's `child.kill()` which only reaches the
        // direct child — the same orphan-leak window the Unix
        // fix closes remains open here until `CREATE_NEW_PROCESS_GROUP`
        // is wired up.
        let _ = child.kill().await;
    }

    // 2. Wait for the process to exit so we don't leave a zombie.
    let status = child.wait().await.ok();
    ShellResult {
        stdout: Vec::new(),
        stderr: Vec::new(),
        exit_code: status.and_then(|s| s.code()).unwrap_or(-1),
        cancelled: true,
        timed_out: false,
    }
}

/// Drain one child pipe on a spawned task. Returning `None` keeps
/// the select arms uniform whether or not the pipe was piped.
/// Shared with `background_shell` (single implementation, no
/// per-module copies).
pub(crate) fn spawn_pipe_drain<R>(pipe: Option<R>) -> Option<tokio::task::JoinHandle<Vec<u8>>>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    Some(tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(mut p) = pipe {
            let _ = p.read_to_end(&mut buf).await;
        }
        buf
    }))
}

pub(crate) async fn collect_drain(task: Option<tokio::task::JoinHandle<Vec<u8>>>) -> Vec<u8> {
    match task {
        Some(h) => h.await.unwrap_or_default(),
        None => Vec::new(),
    }
}

/// Format stdout + stderr into a single string.
pub(crate) fn format_output(stdout: &[u8], stderr: &[u8]) -> String {
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
             is returned with a `[timeout after Nms]` marker. For commands you expect to run \
             longer (full builds, package installs, large test suites), set a larger timeout \
             (e.g. 300000-600000) so the work is not cut off. Long-running services (dev \
             servers, `--watch`) must still finish within the timeout, split them or poll \
             in separate calls.\n\n\
             Outputs over 30 KB are saved to a spill file under the app data dir \
             (the tool result shows the exact absolute path); page through it \
             with read_file offset/limit when you need the full content.\n\n\
             Environment is restricted to a safe allowlist; API keys and tokens \
             from the agent process are NOT inherited.\n\n\
             Avoid `find -exec` / `-execdir`: they are blocked by the permission \
             kill list (find would run an arbitrary command). To act on find's \
             results, pipe with `-print0 | xargs -0` — e.g. `find . -name '*.ts' \
             -print0 | xargs -0 wc -l` — which also handles filenames with spaces.\n\n\
             Optional `description`: a short (aim for 10 words or fewer), \
             active-voice summary of what the command does and why (not a \
             restatement of the command itself). It is display-only — shown to \
             the user in the tool call header and permission prompt; it never \
             affects execution."
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
                                    On timeout the command is killed and partial output is returned. For long commands (full builds, installs, large test suites) set a larger value (e.g. 300000-600000)."
                },
                "description": {
                    "type": "string",
                    "description": "Optional. A short (aim for 10 words or fewer), active-voice \
                                    summary of what this command does and why — e.g. \"Run unit \
                                    tests for the shell tool\". Shown to the user in the tool call \
                                    header and permission prompt. Do not restate the command itself."
                }
            },
            "required": ["command"]
        }),
    }
}

/// Execute the tool. Returns `(content, is_error, ctx_update)`.
///
/// `session_id` keys the C6 disk-spill directory
/// (`<data_dir>/outputs/<session_id>/`) so session delete can sweep
/// the whole directory.
///
/// C1 (Cancel): receives a `CancellationToken` so the child process
/// can be killed on cancel. The flow is:
/// 1. Spawn `sh -c <command>` as a background child process (Unix:
///    in its own process group via `process_group(0)`, PGID = sh PID)
/// 2. `tokio::select!` between `child.wait()` and `cancel.cancelled()`
/// 3. On cancel: send `SIGKILL` to the entire process group (Unix)
///    or `child.kill()` (Windows, MVP) + collect partial stdout/stderr
/// 4. On normal completion: collect full output as before
///
/// **C4 PR1 (2026-06-14)**: returns a 4-tuple
/// `(content, is_error, update, exit_code)`. The `exit_code` is
/// `Some(code)` once the child process has run (the `[exit code: N]`
/// line the formatted content carries is sourced from here). The
/// early-out paths that never spawn a child (`Missing required
/// parameter`, `working_directory rejected`, `Failed to spawn`)
/// return `None` — there's no process to ask. The agent loop feeds
/// the value into the `tool_executed` audit row.
pub async fn execute(
    input: &serde_json::Value,
    ctx: &ToolContext,
    session_id: Option<&str>,
    cancel: &CancellationToken,
) -> (String, bool, ToolContextUpdate, Option<i32>) {
    let command = match input.get("command").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => {
            return (
                "Missing required parameter: command".to_string(),
                true,
                ToolContextUpdate::default(),
                None,
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
                None,
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
    // RULE-E-001: clear the inherited env so API keys / tokens from
    // the parent process are NOT visible to the child. The agent
    // loop's permission system (Tier 4) gates whether a shell call
    // should execute at all; this layer is the *execution-context*
    // hardening that prevents the child from leaking credentials
    // back to the LLM via `env` / `printenv` / `cat /proc/self/...`.
    apply_safe_env(&mut cmd);
    // RULE-E-002: make the child the leader of a brand-new process
    // group. `kill_and_collect` will then send SIGKILL to the whole
    // group on cancel/timeout, so descendants of `&` / pipelines /
    // `nohup` are reaped along with the direct `sh` child. On
    // non-Unix platforms the flag is a no-op and we fall back to
    // `child.kill()` (which leaves the orphan window open — the
    // Windows fix is intentionally deferred).
    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return (
                format!("Failed to spawn command: {}", e),
                true,
                ToolContextUpdate::default(),
                None,
            );
        }
    };

    let update = ToolContextUpdate {
        new_cwd: Some(validated_cwd.clone()),
    };

    // 4. C1 + timeout: race between child completion, cancellation,
    //    and timeout. On cancel/timeout, kill the entire process group
    //    (Unix) or the direct child (Windows) and collect whatever
    //    output was produced so far.
    //
    //    The pipes are taken out and drained on spawned tasks BEFORE
    //    the select: `child.wait()` alone never reads stdout/stderr,
    //    so a child producing more than the pipe capacity (~64 KB on
    //    Linux) would block on write, never exit, and burn the whole
    //    timeout (pre-C6 latent deadlock — every >64 KB command
    //    effectively timed out; found via the C6 spill test).
    let stdout_task = spawn_pipe_drain(child.stdout.take());
    let stderr_task = spawn_pipe_drain(child.stderr.take());
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            tracing::info!("shell: cancellation requested, killing process group");
            let mut r = kill_and_collect(&mut child).await;
            r.stdout = collect_drain(stdout_task).await;
            r.stderr = collect_drain(stderr_task).await;
            r
        }
        _ = tokio::time::sleep(std::time::Duration::from_millis(timeout_ms)) => {
            tracing::info!("shell: timeout after {}ms, killing process group", timeout_ms);
            let mut r = kill_and_collect(&mut child).await;
            r.stdout = collect_drain(stdout_task).await;
            r.stderr = collect_drain(stderr_task).await;
            r.timed_out = true;
            r.cancelled = false; // timeout, not cancel
            r
        }
        status = child.wait() => {
            match status {
                Ok(status) => {
                    let stdout = collect_drain(stdout_task).await;
                    let stderr = collect_drain(stderr_task).await;
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
                        None,
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
    // The child ran; surface the exit code so the agent loop can
    // audit it (C4 PR1). `result.exit_code` is `-1` only on the
    // kill-and-collect path when the wait returned no status —
    // we still surface it rather than collapsing to None so the
    // audit row records "killed (-1)" distinct from "no exit code".
    let reported_exit_code = Some(exit_code);

    // 6. Cancel / timeout markers. C6: these arms no longer return
    //    early — the (potentially huge) partial output flows through
    //    the same spill/truncate finalize below. Pre-C6 a timed-out
    //    `cat huge.log` returned its partial output unbounded.
    if result.cancelled {
        combined = format!("[cancelled, partial output]\n{}", combined);
    } else if result.timed_out {
        combined = format!(
            "[timeout after {}ms, partial output]\n{}",
            timeout_ms, combined
        );
    }

    // 7. Disk-spill: if output exceeds the threshold, write the FULL
    //    output to `<ctx.data_dir>/outputs/<session_id>/` (C6: out
    //    of the project tree) and return a path + preview to the LLM.
    if combined.len() > tool_output::SPILL_THRESHOLD_BYTES {
        match tool_output::spill(&ctx.data_dir, session_id, combined.as_bytes()).await {
            Ok(path) => {
                let omitted = combined
                    .len()
                    .saturating_sub(tool_output::SPILL_PREVIEW_BYTES * 2);
                let marker = tool_output::truncation_marker(
                    omitted,
                    combined.len(),
                    Unit::Bytes,
                    &Recovery::Spill { path: path.clone() },
                );
                let preview = tool_output::head_tail_truncate(
                    &combined,
                    tool_output::SPILL_PREVIEW_BYTES,
                    tool_output::SPILL_PREVIEW_BYTES,
                    &marker,
                );
                let msg = format!(
                    "Output saved to {} ({} bytes). First/last {} preview:\n{}\n[exit code: {}]",
                    path.display(),
                    combined.len(),
                    tool_output::SPILL_PREVIEW_BYTES,
                    preview,
                    exit_code
                );
                return (msg, is_error, update, reported_exit_code);
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    data_dir = %ctx.data_dir.display(),
                    "shell: disk spill failed; falling back to inline truncation"
                );
            }
        }
    }

    // 8. Inline path: apply the 50 KB head+tail truncation. No
    //    recovery segment — the spill copy doesn't exist and shell
    //    output is not replayable.
    let omitted = combined.len().saturating_sub(tool_output::INLINE_CAP_BYTES);
    let marker =
        tool_output::truncation_marker(omitted, combined.len(), Unit::Bytes, &Recovery::None);
    (
        tool_output::head_tail_truncate(
            &combined,
            tool_output::INLINE_CAP_BYTES / 2,
            tool_output::INLINE_CAP_BYTES / 2,
            &marker,
        ),
        is_error,
        update,
        reported_exit_code,
    )
}

/// Best-effort removal of the LEGACY pre-C6 spill location
/// `<cwd>/.everlasting/outputs/`. Called by `delete_session` for
/// sessions created before the C6 relocation — new spills live in
/// `<data_dir>/outputs/<session_id>/` and are swept by
/// `tool_output::sweep_session_outputs`. Failures are logged but
/// never returned: deleting the session is the user's primary
/// intent; disk cleanup is a side effect that should not block the
/// delete or surface a confusing error to the UI.
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
            "shell: failed to clean up legacy disk-spilled outputs on session delete"
        );
    }
}
