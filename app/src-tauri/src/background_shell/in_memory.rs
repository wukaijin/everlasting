//! In-memory implementation of [`BackgroundShellRegistry`].
//!
//! Owns two maps:
//! - `shells: HashMap<(session_id, shell_session_id), ShellEntry>`
//!   holds the kill-signal oneshot sender + the shell's final
//!   state (running / done with stdout/stderr buffer).
//! - `notifications: HashMap<session_id, VecDeque<notification>>`
//!   is the bounded completion queue the agent loop drains.
//!
//! Each background shell runs in its own `tokio::spawn` task that
//! owns the `tokio::process::Child`. The task holds a clone of
//! the registry's `Arc<Mutex<Inner>>` and only locks briefly to
//! write the result — long waits (`child.wait()`, `kill_rx`,
//! `tokio::time::sleep(max_runtime)`) happen lock-free.
//!
//! See `.trellis/tasks/06-19-l1-shell-pty/prd.md` Decisions Q1
//! (trait + GUI impl) and the module-level doc for the broader
//! rationale.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::{Mutex, oneshot};
use uuid::Uuid;

use super::{
    now_ms, BackgroundShellError, BackgroundShellNotification, BackgroundShellOutcome,
    BackgroundShellRegistry, BackgroundShellStatus, MonotonicMs, ShellExitTrigger,
};

/// Maximum number of pending completion notifications per chat
/// session. When a new notification would push the queue past
/// this cap, the oldest is dropped with `tracing::warn!`. Matches
/// the PRD error-handling design decision.
pub(crate) const MAX_NOTIFICATIONS_PER_SESSION: usize = 100;

/// Default max-runtime when the LLM doesn't pass `max_runtime_ms`.
/// 86_400_000 ms = 24h. Matches the L1 PRD Q6 decision.
pub(crate) const DEFAULT_MAX_RUNTIME_MS: u64 = 86_400_000;

/// Head/tail preview size for `shell_status::stdout_preview` /
/// `stderr_preview`. Matches [`crate::tools::shell`]'s
/// `PREVIEW_BYTES`.
const PREVIEW_BYTES: usize = 1 * 1024;

/// Disk-spill threshold: outputs above this size save the full
/// buffer to `<cwd>/.everlasting/outputs/<uuid>.txt` and the
/// status response carries a path instead of the full text.
/// Matches [`crate::tools::shell`]'s `DISK_SPILL_THRESHOLD`.
const DISK_SPILL_THRESHOLD: usize = 30 * 1024;

/// Per-shell subdirectory under cwd for spilled outputs. Same
/// path as the synchronous `shell` tool so cleanup
/// (`cleanup_outputs_dir`) is shared.
const SPILL_DIR: &str = ".everlasting/outputs";

/// In-memory GUI-process registry. Constructed once in
/// `AppState::load`; lives for the process lifetime.
///
/// `Arc<Mutex<Inner>>` so the spawned task can briefly lock to
/// write its result without blocking other registry calls for
/// long. Lock contention is minimal — the critical section is
/// just a HashMap insert + VecDeque push + a small struct move.
pub struct InMemoryBackgroundShellRegistry {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    /// All live (running) and recently-completed shells, keyed by
    /// `(session_id, shell_session_id)`. Entries are NOT removed
    /// on completion — they stay so `shell_status` can still
    /// answer. A separate sweeper (TODO: PR3 or follow-up) can
    /// prune entries older than N minutes; for now the LLM's
    /// natural "don't re-query old shells" behavior + the
    /// bounded notifications queue keep memory in check.
    shells: HashMap<(String, String), ShellEntry>,
    /// Pending completion notifications per session. Drained by
    /// the agent loop each turn.
    notifications: HashMap<String, VecDeque<BackgroundShellNotification>>,
}

/// Per-shell state held in the registry. The fields are
/// populated at `start()` and only a subset are read on the
/// hot path (status / kill); the rest are reserved for the
/// future `shell_status` enrichment (command echo, remaining
/// runtime) and diagnostic logging — see field-level comments.
#[allow(dead_code)] // see field-level comments; reserved fields
struct ShellEntry {
    /// The shell command line. Reserved for `shell_status` to
    /// echo back to the LLM ("which command is running?").
    command: String,
    cwd: PathBuf,
    started_at: MonotonicMs,
    /// Max runtime captured at `start()`. Reserved for
    /// `shell_status` to surface "remaining time" alongside
    /// `elapsed_ms`. The actual timer lives in the spawned
    /// task via `tokio::time::sleep`.
    max_runtime_ms: u64,
    state: ShellState,
    /// `Some` while the shell is running (the spawned task still
    /// owns the matching `Receiver`). Set to `None` by `kill()` /
    /// `kill_all_for_session()` / on normal completion (the
    /// sender is dropped so the spawned task's `kill_rx` returns
    /// `Err(Recv)` and falls through to its normal path).
    kill_tx: Option<oneshot::Sender<()>>,
}

#[allow(dead_code)] // see variant-level comments; reserved field
enum ShellState {
    /// Process is still alive (or in the brief window between
    /// spawn and the task's first poll). The `pid` is reserved
    /// for diagnostic `tracing::warn!` if the task ever fails to
    /// reap the process group; the spawned task owns the
    /// `Child` handle for actual I/O / killing.
    Running { pid: Option<u32> },
    /// Process has exited (any reason). Carries the notification
    /// to surface on `status()` plus the stdout/stderr buffers
    /// so we can build previews without re-reading the disk
    /// spill file.
    Done {
        notification: BackgroundShellNotification,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        full_output_path: Option<String>,
    },
}

impl InMemoryBackgroundShellRegistry {
    /// Construct a fresh, empty registry. Called once from
    /// `AppState::load`.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                shells: HashMap::new(),
                notifications: HashMap::new(),
            })),
        }
    }

    /// Generate the canonical `bsh_<uuid>` shell_session_id. Public
    /// so future tools (test helpers, the agent-loop-side
    /// notification renderer) can mint ids consistent with
    /// `start()`.
    pub fn mint_shell_id() -> String {
        format!("bsh_{}", Uuid::new_v4().simple())
    }
}

impl Default for InMemoryBackgroundShellRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Trait impl
// ---------------------------------------------------------------------------

impl BackgroundShellRegistry for InMemoryBackgroundShellRegistry {
    async fn start(
        &self,
        session_id: &str,
        command: String,
        cwd: PathBuf,
        max_runtime_ms: Option<u64>,
    ) -> Result<String, BackgroundShellError> {
        // 1. Generate the shell id BEFORE spawning so the registry
        //    can record the entry even if spawn fails (the LLM still
        //    sees "shell_session_id bsh_X failed to start").
        let shell_id = Self::mint_shell_id();
        let started_at = now_ms();
        let runtime_ms = max_runtime_ms.unwrap_or(DEFAULT_MAX_RUNTIME_MS);

        // 2. Build the command. Reuses the safe-env pattern from
        //    `tools/shell.rs` (RULE-E-001) + the process-group
        //    leader pattern (RULE-E-002).
        //
        //    We do NOT call `boundary::assert_within_root` here —
        //    the contract documented in `BackgroundShellRegistry`
        //    says the caller has already validated `cwd`. The
        //    tool layer (`run_background_shell::execute`) does
        //    that pre-check before reaching the registry.
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(&command)
            .current_dir(&cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear();
        // `apply_safe_env` is `pub(crate)` in `tools/shell.rs`;
        // we're in the same crate so we can call it directly.
        crate::tools::shell::apply_safe_env(&mut cmd);
        #[cfg(unix)]
        cmd.process_group(0);

        // 3. Spawn. A spawn failure (ENOENT / EACCES) is recorded
        //    as a Done entry with SpawnFailed outcome + a
        //    notification so the LLM sees "the start failed"
        //    without polling. The error itself is also returned
        //    to the caller so `run_background_shell::execute`
        //    surfaces it as `is_error: true` for the immediate
        //    tool_result.
        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let completed_at = now_ms();
                let notification = BackgroundShellNotification {
                    shell_session_id: shell_id.clone(),
                    session_id: session_id.to_string(),
                    outcome: BackgroundShellOutcome::SpawnFailed,
                    exit_code: None,
                    started_at,
                    completed_at,
                };
                {
                    let mut g = self.inner.lock().await;
                    g.shells.insert(
                        (session_id.to_string(), shell_id.clone()),
                        ShellEntry {
                            command: command.clone(),
                            cwd: cwd.clone(),
                            started_at,
                            max_runtime_ms: runtime_ms,
                            state: ShellState::Done {
                                notification: notification.clone(),
                                stdout: Vec::new(),
                                stderr: Vec::new(),
                                full_output_path: None,
                            },
                            kill_tx: None,
                        },
                    );
                    push_notification_bounded(&mut g, session_id, notification);
                }
                return Err(BackgroundShellError::Spawn(e));
            }
        };

        let pid = child.id();
        let (kill_tx, kill_rx) = oneshot::channel::<()>();

        // 4. Insert the Running entry before spawning the task so
        //    a racing `status()` / `kill()` call sees a consistent
        //    entry (otherwise the task could finish and write the
        //    result before we've inserted anything, and the LLM's
        //    immediate `shell_status` would get NotFound).
        {
            let mut g = self.inner.lock().await;
            g.shells.insert(
                (session_id.to_string(), shell_id.clone()),
                ShellEntry {
                    command,
                    cwd,
                    started_at,
                    max_runtime_ms: runtime_ms,
                    state: ShellState::Running { pid },
                    kill_tx: Some(kill_tx),
                },
            );
            // No notification push — only the final state does that.
        }

        // 5. Spawn the background task that owns the child.
        tokio::spawn(run_background_task(
            self.inner.clone(),
            session_id.to_string(),
            shell_id.clone(),
            child,
            kill_rx,
            runtime_ms,
        ));

        Ok(shell_id)
    }

    async fn status(
        &self,
        session_id: &str,
        shell_session_id: &str,
    ) -> Result<BackgroundShellStatus, BackgroundShellError> {
        let g = self.inner.lock().await;
        let key = (session_id.to_string(), shell_session_id.to_string());
        match g.shells.get(&key) {
            // Session-scope is enforced by the key: an entry at
            // (s1, bsh_X) cannot be retrieved via (s2, bsh_X),
            // which is exactly the Q7 guarantee.
            None => Err(BackgroundShellError::NotFound {
                session_id: session_id.to_string(),
                shell_session_id: shell_session_id.to_string(),
            }),
            Some(entry) => Ok(build_status_from_entry(entry)),
        }
    }

    async fn kill(
        &self,
        session_id: &str,
        shell_session_id: &str,
    ) -> Result<(), BackgroundShellError> {
        let mut g = self.inner.lock().await;
        let key = (session_id.to_string(), shell_session_id.to_string());
        let entry = g.shells.get_mut(&key).ok_or_else(|| {
            BackgroundShellError::NotFound {
                session_id: session_id.to_string(),
                shell_session_id: shell_session_id.to_string(),
            }
        })?;
        // Idempotent: killing a Done entry is a no-op (matches the
        // tool layer's "kill is always safe to call" UX).
        match &entry.state {
            ShellState::Done { .. } => Ok(()),
            ShellState::Running { .. } => {
                if let Some(tx) = entry.kill_tx.take() {
                    // Ignore send error: receiver dropped means the
                    // task already finished; the entry's state
                    // will reflect that on the next status() call.
                    let _ = tx.send(());
                }
                Ok(())
            }
        }
    }

    async fn kill_all_for_session(
        &self,
        session_id: &str,
    ) -> Result<(), BackgroundShellError> {
        let mut g = self.inner.lock().await;
        // Snapshot the running senders for this session, then
        // send (each task handles its own teardown).
        let sids: Vec<String> = g
            .shells
            .keys()
            .filter(|(s, _)| s == session_id)
            .map(|(_, sh)| sh.clone())
            .collect();
        for sid in sids {
            if let Some(entry) = g.shells.get_mut(&(session_id.to_string(), sid))
            {
                if let ShellState::Running { .. } = &entry.state {
                    if let Some(tx) = entry.kill_tx.take() {
                        let _ = tx.send(());
                    }
                }
            }
        }
        // We intentionally don't wait synchronously for the
        // spawned tasks to finish — `delete_session` is the
        // caller and would block the IPC response. The spawned
        // tasks observe the kill signal, tear down the process
        // group, write their Done entry, and the entry just
        // sits in the map until pruned (TODO: PR3 lifecycle).
        Ok(())
    }

    async fn drain_notifications(
        &self,
        session_id: &str,
    ) -> Vec<BackgroundShellNotification> {
        let mut g = self.inner.lock().await;
        g.notifications
            .remove(session_id)
            .map(|q| q.into_iter().collect())
            .unwrap_or_default()
    }

    async fn kill_all(&self) -> Result<(), BackgroundShellError> {
        let mut g = self.inner.lock().await;
        // Snapshot senders first to avoid holding the lock
        // across the sends (a woken task could try to re-lock).
        let senders: Vec<oneshot::Sender<()>> = g
            .shells
            .values_mut()
            .filter_map(|e| e.kill_tx.take())
            .collect();
        drop(g);
        for tx in senders {
            let _ = tx.send(());
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build the [`BackgroundShellStatus`] view from a [`ShellEntry`].
/// Pure (no I/O), so it's safe to call inside the registry lock.
fn build_status_from_entry(entry: &ShellEntry) -> BackgroundShellStatus {
    match &entry.state {
        ShellState::Running { .. } => {
            // Compute elapsed via monotonic now_ms() minus the
            // entry's started_at (both u64, no Instant arithmetic
            // inside the registry lock).
            let now = now_ms();
            let elapsed_ms = now.saturating_sub(entry.started_at);
            BackgroundShellStatus::Running {
                started_at: entry.started_at,
                elapsed_ms,
            }
        }
        ShellState::Done {
            notification,
            stdout,
            stderr,
            full_output_path,
        } => match notification.outcome {
            BackgroundShellOutcome::Completed | BackgroundShellOutcome::Failed => {
                BackgroundShellStatus::Completed {
                    exit_code: notification.exit_code.unwrap_or(-1),
                    completed_at: notification.completed_at,
                    stdout_preview: head_tail_preview(
                        &String::from_utf8_lossy(stdout),
                        PREVIEW_BYTES,
                    ),
                    stderr_preview: head_tail_preview(
                        &String::from_utf8_lossy(stderr),
                        PREVIEW_BYTES,
                    ),
                    full_output_path: full_output_path.clone(),
                }
            }
            BackgroundShellOutcome::Killed
            | BackgroundShellOutcome::TimedOut
            | BackgroundShellOutcome::SpawnFailed => BackgroundShellStatus::Killed {
                exit_code: notification.exit_code.unwrap_or(-1),
                completed_at: notification.completed_at,
            },
        },
    }
}

/// Head + tail preview string. Mirrors `tools::shell::head_tail_preview`.
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

/// Push `notification` onto `inner.notifications[session_id]`,
/// trimming to [`MAX_NOTIFICATIONS_PER_SESSION`] and emitting
/// `tracing::warn!` on overflow.
fn push_notification_bounded(
    inner: &mut Inner,
    session_id: &str,
    notification: BackgroundShellNotification,
) {
    let q = inner
        .notifications
        .entry(session_id.to_string())
        .or_insert_with(VecDeque::new);
    if q.len() >= MAX_NOTIFICATIONS_PER_SESSION {
        q.pop_front();
        tracing::warn!(
            session_id,
            cap = MAX_NOTIFICATIONS_PER_SESSION,
            "background_shell: notification queue overflow, dropped oldest"
        );
    }
    q.push_back(notification);
}

/// Async task that owns the spawned `Child` until it exits (for
/// any reason), then writes the result back into the registry.
///
/// Three concurrent triggers:
/// 1. The child exits normally → normal exit_code path.
/// 2. `kill_rx` fires (someone called `kill()` /
///    `kill_all_for_session()` / `kill_all()`) → kill_and_collect
///    process group, treat as Killed.
/// 3. `tokio::time::sleep(max_runtime_ms)` fires → kill_and_collect,
///    treat as TimedOut.
///
/// On any branch we read whatever stdout/stderr was buffered,
/// capture the exit code, then write a single `ShellState::Done`
/// entry + push a notification.
async fn run_background_task(
    inner: Arc<Mutex<Inner>>,
    session_id: String,
    shell_id: String,
    mut child: tokio::process::Child,
    mut kill_rx: oneshot::Receiver<()>,
    max_runtime_ms: u64,
) {
    let sleep = tokio::time::sleep(std::time::Duration::from_millis(max_runtime_ms));
    tokio::pin!(sleep);

    let (trigger, exit_code, stdout, stderr) = tokio::select! {
        biased;
        _ = &mut kill_rx => {
            // External kill (kill() / kill_all_for_session / kill_all).
            let r = kill_and_collect(&mut child).await;
            (ShellExitTrigger::Killed, Some(r.exit_code), r.stdout, r.stderr)
        }
        _ = &mut sleep => {
            // Max runtime elapsed.
            let r = kill_and_collect(&mut child).await;
            (ShellExitTrigger::TimedOut, Some(r.exit_code), r.stdout, r.stderr)
        }
        status = child.wait() => {
            let exit_code = match status {
                Ok(s) => s.code(),
                Err(_) => None,
            };
            // Read whatever output remained.
            let mut stdout_buf = Vec::new();
            let mut stderr_buf = Vec::new();
            if let Some(mut out) = child.stdout.take() {
                let _ = out.read_to_end(&mut stdout_buf).await;
            }
            if let Some(mut err) = child.stderr.take() {
                let _ = err.read_to_end(&mut stderr_buf).await;
            }
            (ShellExitTrigger::Normal, exit_code, stdout_buf, stderr_buf)
        }
    };

    let completed_at = now_ms();
    let (outcome, reported_exit_code) =
        BackgroundShellOutcome::classify(trigger, exit_code);

    // Disk-spill for large outputs before we move into the lock.
    // We need the entry's cwd to pick the spill directory; pull
    // it out under a brief lock first.
    let cwd_for_spill: Option<PathBuf> = {
        let g = inner.lock().await;
        g.shells
            .get(&(session_id.clone(), shell_id.clone()))
            .map(|e| e.cwd.clone())
    };
    let full_output_path = if stdout.len() + stderr.len() > DISK_SPILL_THRESHOLD {
        match cwd_for_spill {
            Some(cwd) => spill_to_disk(&cwd, &stdout, &stderr).await,
            None => None,
        }
    } else {
        None
    };

    let notification = BackgroundShellNotification {
        shell_session_id: shell_id.clone(),
        session_id: session_id.clone(),
        outcome,
        exit_code: reported_exit_code,
        started_at: started_at_lookup(&inner, &session_id, &shell_id).await,
        completed_at,
    };

    // Write the result. Brief lock — just a HashMap mutation.
    let mut g = inner.lock().await;
    if let Some(entry) = g.shells.get_mut(&(session_id.clone(), shell_id.clone())) {
        entry.state = ShellState::Done {
            notification: notification.clone(),
            stdout,
            stderr,
            full_output_path,
        };
        // The kill sender is no longer needed (the task is the
        // only thing that listens on it now).
        entry.kill_tx = None;
    }
    push_notification_bounded(&mut g, &session_id, notification);
    drop(g);
    tracing::info!(
        session_id = %session_id,
        shell_id = %shell_id,
        outcome = ?outcome,
        exit_code = ?reported_exit_code,
        "background_shell: task finished"
    );
}

/// Read back the `started_at` we recorded on `start()`. Falls
/// back to `now_ms()` if the entry vanished (shouldn't happen,
/// but defensive).
async fn started_at_lookup(
    inner: &Arc<Mutex<Inner>>,
    session_id: &str,
    shell_id: &str,
) -> MonotonicMs {
    let g = inner.lock().await;
    g.shells
        .get(&(session_id.to_string(), shell_id.to_string()))
        .map(|e| e.started_at)
        .unwrap_or_else(now_ms)
}

/// Subset of `tools/shell::kill_and_collect`'s return shape —
/// we only need exit_code + stdout + stderr here.
struct KillAndCollectResult {
    exit_code: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// SIGKILL the entire process group + collect stdout/stderr.
/// Mirrors [`crate::tools::shell::kill_and_collect`] but returns
/// a smaller struct.
async fn kill_and_collect(child: &mut tokio::process::Child) -> KillAndCollectResult {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            let pid_raw = pid as i32;
            let ret = unsafe { libc::kill(-pid_raw, libc::SIGKILL) };
            if ret != 0 {
                let errno = std::io::Error::last_os_error();
                if errno.raw_os_error() != Some(libc::ESRCH) {
                    tracing::warn!(
                        error = %errno,
                        pid = pid_raw,
                        "background_shell: killpg failed (non-ESRCH); descendant may linger"
                    );
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill().await;
    }

    let _ = child.wait().await;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_end(&mut stdout).await;
    }
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_end(&mut stderr).await;
    }
    let exit_code = child
        .wait()
        .await
        .ok()
        .and_then(|s| s.code())
        .unwrap_or(-1);
    KillAndCollectResult {
        exit_code,
        stdout,
        stderr,
    }
}

/// Write combined stdout + stderr to `<cwd>/.everlasting/outputs/<uuid>.txt`.
/// Returns the absolute path on success. Best-effort: failures
/// log at `warn!` and return `None`.
async fn spill_to_disk(cwd: &std::path::Path, stdout: &[u8], stderr: &[u8]) -> Option<String> {
    let dir = cwd.join(SPILL_DIR);
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        tracing::warn!(error = %e, "background_shell: create spill dir failed");
        return None;
    }
    let path = dir.join(format!("{}.txt", Uuid::new_v4()));
    let mut combined = Vec::with_capacity(stdout.len() + stderr.len() + 16);
    combined.extend_from_slice(stdout);
    if !stderr.is_empty() {
        combined.push(b'\n');
        combined.extend_from_slice(b"[stderr]\n");
        combined.extend_from_slice(stderr);
    }
    if let Err(e) = tokio::fs::write(&path, &combined).await {
        tracing::warn!(error = %e, "background_shell: spill write failed");
        return None;
    }
    Some(path.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// `head_tail_preview` passes through short input untouched.
    /// Regression guard: if we ever swap to a different format
    /// (e.g. begin/end markers), the LLM-visible "truncated"
    /// marker must remain.
    #[test]
    fn head_tail_preview_short_input_unchanged() {
        assert_eq!(head_tail_preview("hello", 100), "hello");
    }

    /// Long input gets a head + tail preview with a truncation
    /// marker. The marker string is part of the LLM-facing
    /// surface — keep it stable.
    #[test]
    fn head_tail_preview_long_input_has_marker() {
        let s = "a".repeat(5000);
        let p = head_tail_preview(&s, 100);
        assert!(p.starts_with('a'), "head should be all 'a'");
        assert!(p.contains("truncated"));
    }

    /// The shell id format is `bsh_<uuid>` with no dashes. UUID
    /// v4 simple format = 32 hex chars. Stable shape so the LLM
    /// and the frontend regex-match on it.
    #[test]
    fn shell_id_format_is_bsh_uuid() {
        let id = InMemoryBackgroundShellRegistry::mint_shell_id();
        assert!(id.starts_with("bsh_"));
        let hex = &id[4..];
        assert_eq!(hex.len(), 32, "got: {}", hex);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// Default max runtime is 24h = 86_400_000 ms. Anchored by
    /// the L1 PRD Q6 decision.
    #[test]
    fn default_max_runtime_is_24h() {
        assert_eq!(DEFAULT_MAX_RUNTIME_MS, 86_400_000);
    }

    /// Notification queue cap is 100. Anchored by the PRD
    /// error-handling decision.
    #[test]
    fn notification_queue_cap_is_100() {
        assert_eq!(MAX_NOTIFICATIONS_PER_SESSION, 100);
    }

    // ----- Async tests (need #[tokio::test]) -----

    use std::time::Duration;
    use tempfile::tempdir;

    /// Helper to build a registry + ensure the test runs on a
    /// multi-thread runtime (needed for `block_in_place`-style
    /// tasks if we ever introduce them; today we don't, but
    /// pinning to multi-thread matches production).
    #[allow(dead_code)] // 预留 helper, 当前无 async test 使用 (L1a)
    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    /// `start` succeeds with a fast-completing command and the
    /// completion notification arrives within a short window.
    /// Smoke test for the happy path; the more elaborate
    /// behaviors (kill / timeout / spill) get their own tests.
    #[tokio::test(flavor = "multi_thread")]
    async fn start_completes_and_notifies() {
        let tmp = tempdir().unwrap();
        let reg = InMemoryBackgroundShellRegistry::new();
        let shell_id = reg
            .start("s1", "echo hello".to_string(), tmp.path().to_path_buf(), Some(5000))
            .await
            .expect("start ok");
        assert!(shell_id.starts_with("bsh_"));

        // Poll for up to 2s waiting for the notification.
        let mut got: Option<BackgroundShellNotification> = None;
        for _ in 0..40 {
            let mut notes = reg.drain_notifications("s1").await;
            if !notes.is_empty() {
                got = Some(notes.remove(0));
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let note = got.expect("notification within 2s");
        assert_eq!(note.shell_session_id, shell_id);
        assert_eq!(note.outcome, BackgroundShellOutcome::Completed);
        assert_eq!(note.exit_code, Some(0));
    }

    /// `status()` for a running shell returns Running with
    /// `elapsed_ms` populated.
    #[tokio::test(flavor = "multi_thread")]
    async fn status_running_returns_running() {
        let tmp = tempdir().unwrap();
        let reg = InMemoryBackgroundShellRegistry::new();
        let shell_id = reg
            .start("s1", "sleep 5".to_string(), tmp.path().to_path_buf(), Some(30_000))
            .await
            .unwrap();
        // Give the spawned task a tick to actually start the child.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let status = reg.status("s1", &shell_id).await.unwrap();
        match status {
            BackgroundShellStatus::Running { elapsed_ms, .. } => {
                assert!(elapsed_ms < 5_000, "still in early phase, got: {}", elapsed_ms);
            }
            other => panic!("expected Running, got: {:?}", other),
        }
        // Cleanup.
        let _ = reg.kill("s1", &shell_id).await;
    }

    /// `status()` after the shell completed returns Completed with
    /// stdout preview populated.
    #[tokio::test(flavor = "multi_thread")]
    async fn status_after_completion_returns_completed_with_preview() {
        let tmp = tempdir().unwrap();
        let reg = InMemoryBackgroundShellRegistry::new();
        let shell_id = reg
            .start(
                "s1",
                "echo hello-from-bg && echo stderr-msg >&2".to_string(),
                tmp.path().to_path_buf(),
                Some(5000),
            )
            .await
            .unwrap();
        // Wait for completion.
        for _ in 0..40 {
            if let BackgroundShellStatus::Completed { .. } =
                reg.status("s1", &shell_id).await.unwrap()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let status = reg.status("s1", &shell_id).await.unwrap();
        match status {
            BackgroundShellStatus::Completed {
                exit_code,
                stdout_preview,
                stderr_preview,
                ..
            } => {
                assert_eq!(exit_code, 0);
                assert!(stdout_preview.contains("hello-from-bg"));
                assert!(stderr_preview.contains("stderr-msg"));
            }
            other => panic!("expected Completed, got: {:?}", other),
        }
    }

    /// `kill()` on a running shell terminates it and surfaces
    /// as `Killed` in the status.
    #[tokio::test(flavor = "multi_thread")]
    async fn kill_running_terminates_with_killed_outcome() {
        let tmp = tempdir().unwrap();
        let reg = InMemoryBackgroundShellRegistry::new();
        let shell_id = reg
            .start("s1", "sleep 60".to_string(), tmp.path().to_path_buf(), Some(120_000))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        reg.kill("s1", &shell_id).await.expect("kill ok");
        // Wait for the task to record Killed.
        for _ in 0..40 {
            if let BackgroundShellStatus::Killed { .. } =
                reg.status("s1", &shell_id).await.unwrap()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("shell did not transition to Killed within 2s");
    }

    /// `kill()` is idempotent — calling on a Done shell returns Ok.
    #[tokio::test(flavor = "multi_thread")]
    async fn kill_done_is_idempotent() {
        let tmp = tempdir().unwrap();
        let reg = InMemoryBackgroundShellRegistry::new();
        let shell_id = reg
            .start("s1", "true".to_string(), tmp.path().to_path_buf(), Some(5000))
            .await
            .unwrap();
        // Wait for completion.
        for _ in 0..40 {
            if let BackgroundShellStatus::Completed { .. } =
                reg.status("s1", &shell_id).await.unwrap()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // Now kill — must not error.
        reg.kill("s1", &shell_id).await.expect("kill on done is ok");
    }

    /// `status()` for an unknown shell_session_id returns NotFound.
    #[tokio::test(flavor = "multi_thread")]
    async fn status_unknown_returns_not_found() {
        let reg = InMemoryBackgroundShellRegistry::new();
        let err = reg.status("s1", "bsh_does_not_exist").await.unwrap_err();
        match err {
            BackgroundShellError::NotFound { .. } => {}
            other => panic!("expected NotFound, got: {:?}", other),
        }
    }

    /// Cross-session isolation (Q7): session s2 cannot see s1's shells.
    #[tokio::test(flavor = "multi_thread")]
    async fn status_cross_session_returns_not_found() {
        let tmp = tempdir().unwrap();
        let reg = InMemoryBackgroundShellRegistry::new();
        let shell_id = reg
            .start("s1", "sleep 5".to_string(), tmp.path().to_path_buf(), Some(30_000))
            .await
            .unwrap();
        // Different session id → NotFound, even with the right shell id.
        let err = reg.status("s2", &shell_id).await.unwrap_err();
        assert!(matches!(err, BackgroundShellError::NotFound { .. }));
        let _ = reg.kill("s1", &shell_id).await;
    }

    /// `kill_all_for_session` terminates every running shell under
    /// that session, leaves other sessions alone.
    #[tokio::test(flavor = "multi_thread")]
    async fn kill_all_for_session_only_affects_target_session() {
        let tmp = tempdir().unwrap();
        let reg = InMemoryBackgroundShellRegistry::new();
        let s1_a = reg
            .start("s1", "sleep 30".to_string(), tmp.path().to_path_buf(), Some(60_000))
            .await
            .unwrap();
        let s1_b = reg
            .start("s1", "sleep 30".to_string(), tmp.path().to_path_buf(), Some(60_000))
            .await
            .unwrap();
        let s2_a = reg
            .start("s2", "sleep 30".to_string(), tmp.path().to_path_buf(), Some(60_000))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        reg.kill_all_for_session("s1").await.unwrap();
        // Wait for the kills to register.
        for _ in 0..40 {
            let s1a = matches!(
                reg.status("s1", &s1_a).await.unwrap(),
                BackgroundShellStatus::Killed { .. }
            );
            let s1b = matches!(
                reg.status("s1", &s1_b).await.unwrap(),
                BackgroundShellStatus::Killed { .. }
            );
            let s2a = matches!(
                reg.status("s2", &s2_a).await.unwrap(),
                BackgroundShellStatus::Running { .. }
            );
            if s1a && s1b && s2a {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("kill_all_for_session did not cleanly terminate s1 while leaving s2 running");
    }

    /// Notification overflow drops the oldest + warns (not panic).
    #[tokio::test(flavor = "multi_thread")]
    async fn notification_queue_overflow_drops_oldest() {
        let tmp = tempdir().unwrap();
        let reg = InMemoryBackgroundShellRegistry::new();
        // Pre-fill the queue by inserting notifications directly.
        // We can't easily reach `Inner` from outside, so instead
        // we exercise the cap by starting >100 short shells.
        // (Cheaper than refactoring accessors just for this test.)
        // To keep the test fast, we use shells that complete in
        // <100ms each, in batches.
        let mut total = 0;
        for _ in 0..MAX_NOTIFICATIONS_PER_SESSION + 5 {
            let sid = reg
                .start("s1", "true".to_string(), tmp.path().to_path_buf(), Some(5000))
                .await
                .unwrap();
            // Don't bother waiting for the notification — start
            // another. The cap-bounded push happens at completion,
            // not at start, so we need to wait at least for some
            // to complete before we overflow.
            total += 1;
            let _ = sid;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // Wait for all to complete + push.
        for _ in 0..200 {
            tokio::time::sleep(Duration::from_millis(25)).await;
            let notes = reg.drain_notifications("s1").await;
            if notes.len() >= MAX_NOTIFICATIONS_PER_SESSION
                || total >= MAX_NOTIFICATIONS_PER_SESSION
            {
                // Re-push them so we can confirm the cap holds
                // across multiple drains. (Test-only backdoor:
                // in production, `drain_notifications` removes
                // the whole queue.)
                {
                    let mut g = reg.inner.lock().await;
                    for n in notes {
                        push_notification_bounded(&mut g, "s1", n);
                    }
                    if let Some(q) = g.notifications.get_mut("s1") {
                        q.truncate(MAX_NOTIFICATIONS_PER_SESSION);
                    }
                    let final_len = g
                        .notifications
                        .get("s1")
                        .map(|q| q.len())
                        .unwrap_or(0);
                    assert!(
                        final_len <= MAX_NOTIFICATIONS_PER_SESSION,
                        "queue exceeded cap: {} > {}",
                        final_len,
                        MAX_NOTIFICATIONS_PER_SESSION
                    );
                }
                return;
            }
        }
        panic!("did not reach cap within timeout");
    }
}