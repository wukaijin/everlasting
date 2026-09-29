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
//! - N19 (2026-09-29) upgraded the kill to two-stage on Unix:
//!   SIGTERM (grace window, default 3s, env
//!   `EVERLASTING_SHELL_KILL_GRACE_MS`) → SIGKILL. See
//!   [`kill_and_collect`] for the tier rationale; batch kill paths
//!   stay on the Immediate (direct SIGKILL) tier.

use std::path::Path;
use std::process::Stdio;

use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;

use crate::db::Mode;
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

/// Default SIGTERM grace window (N19, 2026-09-29) for the two-stage
/// process-group kill: `kill(-pid, SIGTERM)` → wait up to
/// [`shell_kill_grace_ms`] → `kill(-pid, SIGKILL)`. Gives trap
/// handlers in the script (and well-behaved children like npm /
/// cargo / make, which all trap TERM) a window to run their cleanup
/// — temp files, port teardown, partial reports — before the hard
/// kill. RULE-E-002's "process group must die" invariant is
/// unchanged: the two stages only move the death moment by at most
/// the grace.
pub(crate) const DEFAULT_SHELL_KILL_GRACE_MS: u64 = 3_000;

/// The grace tier for single-kill paths (foreground cancel /
/// timeout arms + the background `shell_kill` tool). Sourced from
/// the `EVERLASTING_SHELL_KILL_GRACE_MS` env var; batch paths
/// (`kill_all_for_session` / `kill_all`) deliberately do NOT consult
/// this — they hard-code 0 (Immediate, direct SIGKILL) because they
/// run under the daemon shutdown budget (SIGTERM→SIGKILL window
/// 15s, drain 8s + axum grace already consume most of it) and the
/// GUI exit must not stall 3s per tier.
///
/// Read once per call (no caching) — same discipline as
/// `delegation_max_concurrent_children`: tests that override the env
/// var in-process see the new value on the next kill, and a test
/// that sets the env var MUST unset it. Unparseable / missing →
/// [`DEFAULT_SHELL_KILL_GRACE_MS`].
pub(crate) fn shell_kill_grace_ms() -> u64 {
    match std::env::var("EVERLASTING_SHELL_KILL_GRACE_MS") {
        Ok(v) => v
            .trim()
            .parse::<u64>()
            .unwrap_or(DEFAULT_SHELL_KILL_GRACE_MS),
        Err(_) => DEFAULT_SHELL_KILL_GRACE_MS,
    }
}

/// Test-only process-wide mutex serializing the tests that WRITE
/// `EVERLASTING_SHELL_KILL_GRACE_MS` (shell.rs's env-override test)
/// against the tests whose tier assertions DEPEND on the default
/// value (in_memory.rs's `kill_single_grace_payload_...` reads the
/// env inside `registry.kill()` and asserts a ≥2.5s floor). cargo
/// test runs tests in parallel per core; without this lock the
/// override's set-var window can shrink the other test's grace to
/// 250ms and flake the floor assertion (N19 check pass, 2026-09-29).
#[cfg(test)]
pub(crate) static GRACE_ENV_TEST_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

/// Send `sig` to the child's whole process group. Shared by both
/// stages of [`kill_and_collect`]. ESRCH (group already exited) is
/// treated as success; other failures are logged at `warn!` but
/// never propagated — the worst case is that a descendant lingers
/// briefly, which the eventual `child.wait()` below will catch once
/// stdout/stderr pipes close.
#[cfg(unix)]
fn kill_group(pid: i32, sig: i32) {
    // Negative pid => "send signal to the process group whose
    // PGID is |pid|". Safe because process_group(0) made
    // `pid` == PGID.
    let ret = unsafe { libc::kill(-pid, sig) };
    if ret != 0 {
        let errno = std::io::Error::last_os_error();
        if errno.raw_os_error() != Some(libc::ESRCH) {
            tracing::warn!(
                error = %errno,
                pid,
                signal = sig,
                "shell: killpg failed (non-ESRCH); descendant may linger"
            );
        }
    }
}

/// Kill the child process — two-stage when `grace_ms > 0`, hard
/// when `grace_ms == 0` (N19, 2026-09-29). Output collection is the
/// caller's job: the pipes are taken out and drained on spawned
/// tasks BEFORE the wait/kill select (see `execute`), so after the
/// group kill closes the write ends, those tasks complete with the
/// partial output.
///
/// On Unix the child was spawned with `process_group(0)`, so the
/// `sh` process is the leader of a new process group whose PGID
/// equals `child.id()` — every signal below targets the group so
/// descendants of `&` / pipelines / `nohup` die along with the
/// direct child (RULE-E-002).
///
/// **Two-stage (grace_ms > 0)**: `kill(-pid, SIGTERM)` first, then
/// wait up to `grace_ms`. A group whose scripts trap TERM (npm /
/// cargo / make all do) gets exactly one window to run cleanup
/// (temp files, port teardown, lock release) before the hard kill —
/// the mcode/pi-mono lesson: under unconditional SIGKILL that
/// cleanup logic NEVER gets a chance to execute. A group that
/// ignores TERM (or a wedged trap) escalates to SIGKILL after the
/// window: worst case = the old behavior plus `grace_ms`, so the
/// "必死" invariant is preserved, only deferred by at most the
/// grace.
///
/// **Hard (grace_ms == 0)**: direct SIGKILL — byte-identical to the
/// pre-N19 behavior. The batch paths (kill_all_for_session /
/// kill_all / daemon shutdown) pin this tier: they run under the
/// daemon's 15s SIGTERM→SIGKILL budget where determinism beats
/// politeness.
///
/// The caller-visible semantics do NOT distinguish the stages:
/// this returns `cancelled: true` either way ("killed is killed").
/// The reported `exit_code` may differ — a graceful TERM exit
/// carries the script's own code (e.g. a trap's `exit 0`), a hard
/// kill reports -1 — that's data for the `[exit code: N]` line, not
/// a different outcome.
pub(crate) async fn kill_and_collect(child: &mut Child, grace_ms: u64) -> ShellResult {
    // 1a. Grace stage (only when armed): TERM the group, wait once
    //     inside the window.
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            let pid_raw = pid as i32;
            if grace_ms > 0 {
                kill_group(pid_raw, libc::SIGTERM);
                match tokio::time::timeout(std::time::Duration::from_millis(grace_ms), child.wait())
                    .await
                {
                    // Group exited within the grace window — trap
                    // cleanup ran; skip the hard kill entirely.
                    Ok(status) => {
                        return ShellResult {
                            stdout: Vec::new(),
                            stderr: Vec::new(),
                            exit_code: status.ok().and_then(|s| s.code()).unwrap_or(-1),
                            cancelled: true,
                            timed_out: false,
                        };
                    }
                    Err(_) => {
                        tracing::info!(
                            pid = pid_raw,
                            grace_ms,
                            "shell: SIGTERM grace expired, escalating to SIGKILL"
                        );
                    }
                }
            }
            // 1b. Hard stage: reached directly (grace_ms == 0) or
            //     after the window expired.
            kill_group(pid_raw, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    {
        // Windows path (MVP, not yet hardened per RULE-E-002). We
        // fall back to tokio's `child.kill()` which only reaches the
        // direct child — the same orphan-leak window the Unix fix
        // closes remains open here until `CREATE_NEW_PROCESS_GROUP`
        // is wired up. `grace_ms` is deliberately IGNORED: tokio's
        // kill maps to an unconditional TerminateProcess; there is
        // no group-wide TERM stage to run first on this platform.
        let _ = grace_ms;
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

/// Configure-free core of both spawn paths: spawn the (already
/// configured) command, then race completion / cancellation / timeout.
/// On cancel/timeout the process GROUP is killed (RULE-E-002) and
/// partial output collected. The pipes are drained on spawned tasks
/// BEFORE the select: `child.wait()` alone never reads stdout/stderr,
/// so a child producing more than the pipe capacity (~64 KB on Linux)
/// would block on write, never exit, and burn the whole timeout
/// (pre-C6 latent deadlock — found via the C6 spill test).
///
/// Shared by the sandboxed first spawn and the P3c unsandboxed
/// escalation rerun (identical wait semantics; P3c design §5.2).
async fn spawn_and_collect(
    cmd: &mut Command,
    timeout_ms: u64,
    cancel: &CancellationToken,
) -> std::io::Result<ShellResult> {
    let mut child = cmd.spawn()?;
    let stdout_task = spawn_pipe_drain(child.stdout.take());
    let stderr_task = spawn_pipe_drain(child.stderr.take());
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            tracing::info!("shell: cancellation requested, killing process group");
            let mut r = kill_and_collect(&mut child, shell_kill_grace_ms()).await;
            r.stdout = collect_drain(stdout_task).await;
            r.stderr = collect_drain(stderr_task).await;
            r
        }
        _ = tokio::time::sleep(std::time::Duration::from_millis(timeout_ms)) => {
            tracing::info!("shell: timeout after {}ms, killing process group", timeout_ms);
            let mut r = kill_and_collect(&mut child, shell_kill_grace_ms()).await;
            r.stdout = collect_drain(stdout_task).await;
            r.stderr = collect_drain(stderr_task).await;
            r.timed_out = true;
            r.cancelled = false; // timeout, not cancel
            r
        }
        status = child.wait() => {
            let status = status?;
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
    };
    Ok(result)
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
             Non-zero exit codes are data, not tool errors — read the \
             trailing `[exit code: N]` line and judge from the output.\n\n\
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

    // P3b (08-31-a2-p3b) + P3c: execution-time sandbox — the damage
    // limiter UNDER the classification layer. `decide` resolves the
    // policy (`resolve_policy`: capability → Yolo → project off →
    // kill-switch → Plan → project face); under a face EVERY command
    // sandboxes (classification no longer gates the trigger). Its
    // result is reused below for the post-hoc guidance and the audit
    // row (W3: no second query). Fail-open on capability probe
    // failure (R5); fail-closed on prepare/pre-exec failure
    // (`[sandbox]`-prefixed spawn error, design §2.3).
    let sandbox_decision = crate::sandbox::decide(ctx, command, session_id).await;
    // Durable prefix-grant hit (09-21-durable-prefix-grant R5): the
    // Skip carries a structured reason — write the audit row here in
    // the tool layer (the sandbox module stays audit-free by
    // contract). Best-effort, like every audit write around spawn.
    if let crate::sandbox::Decision::Skip { reason } = &sandbox_decision {
        if matches!(reason, crate::sandbox::SkipReason::DurableGrant) {
            if let Some(sid) = session_id {
                let sha = crate::sandbox::command_sha_prefix(command);
                if let Err(e) = crate::agent::permissions::audit::record_durable_grant_hit_audit(
                    &ctx.db, sid, "shell", &sha, None,
                )
                .await
                {
                    tracing::warn!(error = %e, "shell: durable grant-hit audit write failed");
                }
            }
        }
    }
    // N5 (09-26): terminal sandbox attribution, seeded HERE (right
    // after the decision) so every exit path — including the
    // fail-closed prepare/apply/spawn errors below — carries a
    // truthful value into the `tool_executed` payload. Seeded value
    // = what the call WOULD run under; the §4b escalation approval
    // below overwrites it to `Escalation` at the one point where the
    // terminal state actually flips (unsandboxed rerun approved).
    // A fail-closed `[sandbox]` error path keeps `Sandboxed`: the
    // decision was a sandbox face and nothing ran bare.
    let mut update = ToolContextUpdate {
        new_cwd: Some(validated_cwd.clone()),
        sandbox_attribution: Some(match &sandbox_decision {
            crate::sandbox::Decision::Skip { reason } => reason.into(),
            crate::sandbox::Decision::Sandbox(_) => crate::sandbox::SandboxAttribution::Sandboxed,
        }),
    };
    // R9 conjunction input: which net enforcer the (potential) spawn
    // installs — computed once from the same decision (W3 spirit).
    let net_enf = match &sandbox_decision {
        crate::sandbox::Decision::Sandbox(spec) => spec.net_enforcement(),
        crate::sandbox::Decision::Skip { .. } => crate::sandbox::NetEnforcement::None,
    };
    let mut prepared: Option<crate::sandbox::PreparedSandbox> = None;
    if let crate::sandbox::Decision::Sandbox(spec) = &sandbox_decision {
        match crate::sandbox::prepare(spec) {
            Ok(p) => {
                if let Err(e) = crate::sandbox::apply(&mut cmd, &p) {
                    return (
                        format!("[sandbox] Failed to apply sandbox: {}", e),
                        true,
                        update.clone(),
                        None,
                    );
                }
                prepared = Some(p);
            }
            Err(e) => {
                return (
                    format!("[sandbox] Failed to prepare sandbox: {}", e),
                    true,
                    update.clone(),
                    None,
                );
            }
        }
    }

    // 4. C1 + timeout: race between child completion, cancellation,
    //    and timeout (helper shared with the P3c escalation rerun).
    //    On cancel/timeout, kill the entire process group (Unix) or
    //    the direct child (Windows) and collect partial output.
    let mut result = match spawn_and_collect(&mut cmd, timeout_ms, cancel).await {
        Ok(r) => r,
        Err(e) => {
            let prefix = if prepared.is_some() {
                // pre_exec closure failed inside the forked child —
                // never run the command unsandboxed by accident.
                "[sandbox] "
            } else {
                ""
            };
            return (
                format!("{prefix}Failed to spawn command: {}", e),
                true,
                update.clone(),
                None,
            );
        }
    };

    // P3b (D2): audit the sandboxed execution once the child is
    // actually running (spawn succeeded). Best-effort — an audit
    // failure never breaks the command. The payload carries a command
    // hash + ruleset summary, not the command text (the sibling
    // `tool_executed` row already has the full input). W3: the spec
    // comes from the decision computed before spawn — no re-gate.
    // The escalation rerun (4b below, unsandboxed) adds no second row
    // — its provenance rides the ask-side audit + the final
    // `tool_executed` outcome.
    if prepared.is_some() {
        if let Some(sid) = session_id {
            let ruleset = match &sandbox_decision {
                crate::sandbox::Decision::Sandbox(spec) => spec.summary(),
                _ => unreachable!("prepared.is_some() implies a Sandbox decision"),
            };
            let sha = crate::sandbox::command_sha_prefix(command);
            if let Err(e) = crate::agent::permissions::audit::record_sandboxed_shell_audit(
                &ctx.db, sid, "shell", &sha, &ruleset, None,
            )
            .await
            {
                tracing::warn!(error = %e, "shell: sandboxed-shell audit write failed");
            }
        }
    }

    // 4b. P3c escalation loop (design §5): at most ONE unsandboxed
    //     rerun per tool call. Fires only when the sandboxed first
    //     attempt failed on an out-of-face denial (write / network),
    //     never in Plan mode (D3: no escalation exit there) and never
    //     when no handle was injected (tests → guidance-only). The
    //     double-execution boundary is accepted by design (D4): the
    //     dangerous part never ran once — the first attempt was
    //     stopped at the failure. Rerun audit provenance = the
    //     ask-side rows (tool_permission_ask / permission_granted /
    //     tool_allowed / tool_denied) + the sibling tool_executed row
    //     carrying the FINAL outcome — no new audit kind (design §3.5
    //     kind reuse).
    let mut reran_unsandboxed = false;
    if prepared.is_some()
        && !result.cancelled
        && !result.timed_out
        && result.exit_code != 0
        && ctx.mode != Mode::Plan
        && !ctx.escalation.is_none()
    {
        // Note: `!result.timed_out` — a timeout kill reports exit -1
        // with PARTIAL stderr; a denial string in that partial output
        // would fire a card whose rerun just times out again. The
        // timeout marker is the user-visible signal for that path.
        let stderr_str = String::from_utf8_lossy(&result.stderr);
        // 2026-09-21: listen-class denials (dev servers) print their
        // EPERM to stdout with an empty stderr — classify needs both
        // streams (sandbox-executor.md §10a).
        let stdout_str = String::from_utf8_lossy(&result.stdout);
        if let Some(kind) = crate::sandbox::classify_block(
            &stderr_str,
            &stdout_str,
            Some(result.exit_code),
            net_enf,
        ) {
            // (a) prefix-grant hit (AC6) → rerun directly, no card.
            //     Same compound-command gate as Tier 4 (the grant only
            //     ever covers a single-segment command).
            let grant_hit = match session_id {
                Some(sid) => {
                    crate::agent::permissions::escalation::prefix_grant_hit(
                        &ctx.db,
                        sid,
                        &ctx.worktree_path,
                        command,
                    )
                    .await
                }
                None => false,
            };
            let approved = if grant_hit {
                tracing::info!(
                    command_sha = %crate::sandbox::command_sha_prefix(command),
                    "shell: sandbox escalation via prefix-grant hit"
                );
                // design §5.2「重跑 + 审计」: best-effort ToolAllowed
                // row so the grant-hit rerun is distinguishable in the
                // audit trail from a plain sandboxed failure.
                if let Err(e) = ctx.escalation.audit_grant_rerun("shell", input).await {
                    tracing::warn!(error = %e, "shell: grant-rerun audit write failed");
                }
                true
            } else {
                // (b) Ask card: command text + interception cause +
                //     evidence line. AllowOnce / AllowAlways (grant
                //     persisted by ask_path) → rerun. Evidence: stderr
                //     as-is; when stderr is empty (listen failures live
                //     in stdout) fall back to the extracted stdout line.
                let evidence_str: std::borrow::Cow<'_, str> = if stderr_str.trim().is_empty() {
                    std::borrow::Cow::Owned(
                        crate::agent::permissions::escalation::stdout_net_evidence_line(
                            &stdout_str,
                        ),
                    )
                } else {
                    std::borrow::Cow::Borrowed(&stderr_str)
                };
                matches!(
                    ctx.escalation
                        .ask("shell", input, command, kind, &evidence_str)
                        .await,
                    crate::agent::permissions::escalation::EscalationOutcome::Approved
                )
            };
            if approved {
                // N5: the terminal execution flips to unsandboxed HERE
                // — the one overwrite point (the seed above assumed the
                // sandboxed path). The rerun's spawn-error return below
                // also carries `Escalation`: the approval happened, the
                // terminal state is the approved rerun regardless of
                // how that rerun ended.
                update.sandbox_attribution = Some(crate::sandbox::SandboxAttribution::Escalation);
                let mut retry = Command::new("sh");
                retry
                    .arg("-c")
                    .arg(command)
                    .current_dir(&validated_cwd)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                // Byte-identical env + process group as the first
                // attempt (RULE-E-001 / RULE-E-002) — only the sandbox
                // is absent.
                apply_safe_env(&mut retry);
                #[cfg(unix)]
                retry.process_group(0);
                match spawn_and_collect(&mut retry, timeout_ms, cancel).await {
                    Ok(r) => {
                        result = r;
                        reran_unsandboxed = true;
                    }
                    Err(e) => {
                        return (
                            format!("Failed to spawn rerun command: {}", e),
                            true,
                            update,
                            None,
                        );
                    }
                }
            }
        }
    }

    // 5. Format output.
    let mut combined = format_output(&result.stdout, &result.stderr);

    let exit_code = result.exit_code;
    if !combined.is_empty() {
        combined.push_str(&format!("\n[exit code: {}]", exit_code));
    } else {
        combined = format!("[exit code: {}]", exit_code);
    }

    // 2026-09-02: exit code semantics — a completed run's exit code
    // is DATA, not a tool failure. The `[exit code: N]` line above
    // reports it and the model judges from the output (many commands
    // use non-zero exits as information: `grep` no-match, `diff`
    // differences-found, `test`). Only infrastructure failures —
    // user cancel / timeout / spawn error — are tool errors. Same
    // philosophy as `grep`'s "rg exit1 = no matches is not an error"
    // (tool-contract spec §is_error semantics). Pre-change every
    // non-zero exit flipped the UI card into the red error state,
    // punishing intentional exit-1 commands.
    let is_error = result.cancelled || result.timed_out;
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

    // 6b. P3b (§2.5, R7) + P3c (§5.3): post-hoc failure guidance,
    //     mode-aware. When this command WAS sandboxed (W3: reuse the
    //     decision above — no second gate query) and failed with a
    //     stderr that smells like a sandbox denial, append one
    //     guidance line so the model knows the failure is ours and
    //     what to do about it. Append-only — the command's own output
    //     is untouched.
    let sandbox_applied = prepared.is_some();
    if sandbox_applied && !reran_unsandboxed && !result.cancelled && exit_code != 0 {
        let stderr_str = String::from_utf8_lossy(&result.stderr);
        let stdout_str = String::from_utf8_lossy(&result.stdout);
        if let Some(guidance) = crate::sandbox::failure_guidance(
            &stderr_str,
            &stdout_str,
            Some(exit_code),
            net_enf,
            ctx.mode,
        ) {
            combined.push('\n');
            combined.push_str(guidance);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Spawn `sh -c <script>` in its own process group — the same
    /// RULE-E-002 spawn shape as `execute`, minus the sandbox / env
    /// layers (`kill_and_collect`'s contract only depends on the
    /// PGID setup, not the env allowlist).
    fn spawn_script(script: &str) -> Child {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(script)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        cmd.process_group(0);
        cmd.spawn().expect("spawn sh -c")
    }

    /// Poll until `marker` exists (the script's readiness signal —
    /// `touch` after installing its trap). Keeps the TERM-delivery
    /// assertions race-free on slow CI: without it, a kill could
    /// land before sh parsed the trap line.
    async fn wait_ready(marker: &Path) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !marker.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "script never touched its readiness marker"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// N19 ①: grace-armed kill delivers SIGTERM first — a script
    /// that traps TERM runs its handler and exits WITHIN the window.
    /// The trap's own exit code (42) must surface through the
    /// `ShellResult` (a graceful exit carries the script's code; a
    /// hard kill reports -1 — that difference is the observable
    /// proof the TERM stage ran).
    #[cfg(unix)]
    #[tokio::test]
    async fn kill_grace_trap_term_exits_within_window() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("ready");
        let mut child = spawn_script(&format!(
            "trap 'exit 42' TERM; touch {}; while true; do sleep 0.1; done",
            marker.display()
        ));
        wait_ready(&marker).await;

        let grace = 2_000u64;
        let started = std::time::Instant::now();
        let r = kill_and_collect(&mut child, grace).await;
        assert!(r.cancelled, "graceful exit is still a kill outcome");
        assert!(!r.timed_out);
        assert_eq!(
            r.exit_code, 42,
            "trap exit code must surface on the grace path"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_millis(grace),
            "trap-armed script must exit inside the grace window, took {:?}",
            started.elapsed()
        );
    }

    /// N19 ②: a group that IGNORES TERM (`trap ''` — and children
    /// inherit the SIG_IGN disposition) survives the grace window
    /// and is then hard-killed by the SIGKILL escalation. RULE-E-002
    /// "必死" preserved: worst case = old behavior + grace.
    #[cfg(unix)]
    #[tokio::test]
    async fn kill_grace_ignoring_term_escalates_to_sigkill() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("ready");
        let mut child = spawn_script(&format!(
            "trap '' TERM; touch {}; while true; do sleep 0.1; done",
            marker.display()
        ));
        wait_ready(&marker).await;

        let grace = 800u64;
        let started = std::time::Instant::now();
        let r = kill_and_collect(&mut child, grace).await;
        assert!(r.cancelled);
        // The full window must elapse before SIGKILL lands. If the
        // implementation skipped the TERM stage (direct SIGKILL),
        // elapsed collapses to ~0ms — the discriminating assertion
        // of the two-stage wiring. 100ms slack for scheduler jitter.
        assert!(
            started.elapsed() >= std::time::Duration::from_millis(grace - 100),
            "grace must elapse before SIGKILL escalation, took {:?}",
            started.elapsed()
        );
        // Hard-killed: death by signal has no exit code → -1.
        assert_eq!(r.exit_code, -1);
    }

    /// N19 ③: grace=0 is the Immediate tier — byte-identical to the
    /// pre-N19 direct SIGKILL. Even a TERM-ignoring group dies
    /// without waiting: the batch paths (kill_all_for_session /
    /// kill_all / daemon shutdown) pin this tier and must not
    /// smuggle in a hidden default grace.
    #[cfg(unix)]
    #[tokio::test]
    async fn kill_grace_zero_is_immediate() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("ready");
        let mut child = spawn_script(&format!(
            "trap '' TERM; touch {}; while true; do sleep 0.1; done",
            marker.display()
        ));
        wait_ready(&marker).await;

        let started = std::time::Instant::now();
        let r = kill_and_collect(&mut child, 0).await;
        assert!(r.cancelled);
        assert!(
            started.elapsed() < std::time::Duration::from_millis(1_000),
            "grace=0 must kill immediately, took {:?}",
            started.elapsed()
        );
        assert_eq!(r.exit_code, -1);
    }

    /// Env override hook parses and falls back to the default on
    /// garbage. No caching (same discipline as
    /// `delegation_max_concurrent_children`) — read per call.
    /// Holds [`GRACE_ENV_TEST_MUTEX`] for the whole body: the
    /// set-var window is process-global, and in_memory.rs's
    /// single-tier wiring test asserts a floor derived from the
    /// DEFAULT grace — parallel pollution would flake it.
    #[tokio::test]
    async fn shell_kill_grace_env_override() {
        let _guard = GRACE_ENV_TEST_MUTEX.lock().await;
        assert_eq!(shell_kill_grace_ms(), DEFAULT_SHELL_KILL_GRACE_MS);
        std::env::set_var("EVERLASTING_SHELL_KILL_GRACE_MS", "250");
        assert_eq!(shell_kill_grace_ms(), 250);
        std::env::set_var("EVERLASTING_SHELL_KILL_GRACE_MS", "not-a-number");
        assert_eq!(shell_kill_grace_ms(), DEFAULT_SHELL_KILL_GRACE_MS);
        std::env::remove_var("EVERLASTING_SHELL_KILL_GRACE_MS");
        assert_eq!(shell_kill_grace_ms(), DEFAULT_SHELL_KILL_GRACE_MS);
    }
}
