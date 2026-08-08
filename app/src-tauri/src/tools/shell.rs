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
pub(crate) const MAX_OUTPUT_BYTES: usize = 50 * 1024;
/// claude-code style threshold: outputs above this size spill to
/// disk and the LLM gets a path instead of the full text.
pub(crate) const DISK_SPILL_THRESHOLD: usize = 30 * 1024;
/// Preview size (head + tail) when we spill to disk. Keeps the
/// tool_result under ~1.5 KB so the agent's context stays small.
pub(crate) const PREVIEW_BYTES: usize = 1024;
/// Sub-directory under cwd where spilled outputs are written.
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

/// Kill the child process and collect whatever output was produced.
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
        // fall back to tokio's `child.kill()` which only reaches
        // the direct child — the same orphan-leak window the Unix
        // fix closes remains open here until `CREATE_NEW_PROCESS_GROUP`
        // is wired up.
        let _ = child.kill().await;
    }

    // 2. Wait for the process to exit so we don't leave a zombie.
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
             Outputs over 30 KB are saved to `<cwd>/.everlasting/outputs/<id>.txt`; \
             the tool returns the path plus a short preview so you can read the \
             full file with read_file.\n\n\
             Environment is restricted to a safe allowlist; API keys and tokens \
             from the agent process are NOT inherited.\n\n\
             Avoid `find -exec` / `-execdir`: they are blocked by the permission \
             kill list (find would run an arbitrary command). To act on find's \
             results, pipe with `-print0 | xargs -0` — e.g. `find . -name '*.ts' \
             -print0 | xargs -0 wc -l` — which also handles filenames with spaces."
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
    _session_id: Option<&str>,
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
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            tracing::info!("shell: cancellation requested, killing process group");
            kill_and_collect(&mut child).await
        }
        _ = tokio::time::sleep(std::time::Duration::from_millis(timeout_ms)) => {
            tracing::info!("shell: timeout after {}ms, killing process group", timeout_ms);
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

    // 6. If cancelled, prepend marker.
    if result.cancelled {
        combined = format!("[cancelled, partial output]\n{}", combined);
        return (combined, true, update, reported_exit_code);
    }

    // 7. If timed out, prepend marker with the timeout duration.
    if result.timed_out {
        combined = format!(
            "[timeout after {}ms, partial output]\n{}",
            timeout_ms, combined
        );
        return (combined, true, update, reported_exit_code);
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
                return (msg, is_error, update, reported_exit_code);
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
    (
        truncate_output(combined),
        is_error,
        update,
        reported_exit_code,
    )
}

/// Write `contents` to `<cwd>/.everlasting/outputs/<uuid>.txt`,
/// creating the directory if needed. Returns the absolute path.
pub(crate) async fn spill_to_disk(cwd: &Path, contents: &str) -> std::io::Result<PathBuf> {
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
pub(crate) fn head_tail_preview(s: &str, cap: usize) -> String {
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
pub(crate) fn truncate_output(s: String) -> String {
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
