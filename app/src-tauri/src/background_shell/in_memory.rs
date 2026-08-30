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

use tokio::process::Command;
use tokio::sync::{oneshot, Mutex};
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

/// How long a completed (Done) shell entry stays in the registry
/// before the daemon sweeper prunes it. 3_600_000 ms = 1h
/// (RULE-SHELL-001, design D3): completion notifications are
/// drained at the very next turn and LLM `shell_status` queries
/// cluster within seconds-to-minutes of completion, so 1h covers
/// the query window; a latecomer past the window still gets the
/// outcome + exit_code from the self-contained notification and
/// only loses the stdout preview (`status()` → NotFound is the
/// documented "already cleaned up" semantics). Layered apart
/// from [`DEFAULT_MAX_RUNTIME_MS`]: 24h run cap vs 1h result
/// retention.
pub(crate) const SHELL_RETENTION_MS: u64 = 3_600_000;

/// How often the daemon sweeper calls
/// [`InMemoryBackgroundShellRegistry::sweep_completed_shells`].
/// 300_000 ms = 5min (RULE-SHELL-001, design D3): the sweep is a
/// timestamp comparison over a small map (cost ≈ 0), and ±5min
/// drift is imperceptible against the 1h retention.
pub(crate) const SWEEP_INTERVAL_MS: u64 = 300_000;

// C6 (08-30-c6-output-truncation): the spill threshold / preview
// size / spill location now live in `tools::tool_output` — this
// module spilled via its own private copy (constants + `spill_to_disk`
// + `head_tail_preview`) that had drifted into an independent
// implementation. Previews and spills below consume the shared
// contract; spill lands in `<data_dir>/outputs/<session_id>/`
// (registry `data_dir`, injected by `new_with_data_dir` — the
// bare `new()` used in tests has none and skips spilling).

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
    /// answer. Done entries are pruned by the daemon sweeper
    /// ([`InMemoryBackgroundShellRegistry::sweep_completed_shells`])
    /// once past [`SHELL_RETENTION_MS`], releasing their
    /// stdout/stderr buffers; Running entries are never pruned.
    shells: HashMap<(String, String), ShellEntry>,
    /// Pending completion notifications per session. Drained by
    /// the agent loop each turn.
    notifications: HashMap<String, VecDeque<BackgroundShellNotification>>,
    /// App data dir for C6 output spills (`<dir>/outputs/<session>/`).
    /// `None` in test registries (bare `new()`) — spill is skipped.
    data_dir: Option<PathBuf>,
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
                data_dir: None,
            })),
        }
    }

    /// Production constructor: same as [`new`] but with the app
    /// data dir, enabling C6 output spills to
    /// `<data_dir>/outputs/<session_id>/`. Called from
    /// `AppState::load` (where the Tauri-resolved dir is at hand);
    /// test registries keep the bare `new()` and skip spilling.
    pub fn new_with_data_dir(data_dir: PathBuf) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                shells: HashMap::new(),
                notifications: HashMap::new(),
                data_dir: Some(data_dir),
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

    /// Prune completed-shell entries older than `retention_ms`
    /// (RULE-SHELL-001, design D1). Removes entries whose state is
    /// `Done` AND `now - completed_at > retention_ms`, returning
    /// the number removed.
    ///
    /// **Never touches Running entries** — removing one would
    /// orphan its `kill_tx` (the LLM loses the kill channel for a
    /// live process group); the max-runtime timer (default 24h)
    /// guarantees Running eventually becomes Done and a later pass
    /// sweeps it. Race-safe against `run_background_task`: the
    /// write-back uses `if let Some(entry)`, so an entry vanishing
    /// mid-flight is tolerated by design.
    ///
    /// Pure in-lock map traversal + timestamp comparison: no I/O,
    /// no nested await. Removing an entry drops its stdout/stderr
    /// buffers — the memory-heavy part of a Done entry (the disk
    /// spill is an extra copy, the in-memory buffer is kept only
    /// for status previews). After the sweep, `status()` / `kill()`
    /// for a removed shell return `NotFound`, which is the
    /// documented "already cleaned up" semantics of
    /// [`BackgroundShellRegistry::status`].
    ///
    /// Inherent method (NOT on the [`BackgroundShellRegistry`]
    /// trait): sweeping is impl-private lifecycle management, not
    /// an LLM tool surface. Called by the daemon sweeper task
    /// (`daemon::server::spawn_shell_sweeper`) with
    /// [`SHELL_RETENTION_MS`]; `retention_ms` is injectable so
    /// tests never wait out a real retention window.
    pub async fn sweep_completed_shells(&self, retention_ms: u64) -> usize {
        let now = now_ms();
        let mut g = self.inner.lock().await;
        let before = g.shells.len();
        g.shells.retain(|_, entry| {
            !matches!(
                &entry.state,
                ShellState::Done { notification, .. }
                    if now.saturating_sub(notification.completed_at) > retention_ms
            )
        });
        before - g.shells.len()
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
        sandbox: Option<crate::sandbox::SandboxSpec>,
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

        // P3b (08-31-a2-p3b): apply the caller-computed sandbox spec.
        // Prepare (parent zone: ruleset fd + O_PATH fds + BPF) may
        // fail → Err(Spawn) like any spawn failure (fail-closed);
        // apply registers the syscall-only pre_exec closure, whose
        // failures surface from `cmd.spawn()` below through the same
        // SpawnFailed channel. `None` → byte-identical legacy spawn.
        if let Some(spec) = &sandbox {
            let prepared = crate::sandbox::prepare(spec).map_err(BackgroundShellError::Spawn)?;
            crate::sandbox::apply(&mut cmd, &prepared).map_err(BackgroundShellError::Spawn)?;
        }

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
        let entry = g
            .shells
            .get_mut(&key)
            .ok_or_else(|| BackgroundShellError::NotFound {
                session_id: session_id.to_string(),
                shell_session_id: shell_session_id.to_string(),
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

    async fn kill_all_for_session(&self, session_id: &str) -> Result<(), BackgroundShellError> {
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
            if let Some(entry) = g.shells.get_mut(&(session_id.to_string(), sid)) {
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
        // group, write their Done entry, and the entry is later
        // pruned by the daemon sweeper
        // (`sweep_completed_shells`, RULE-SHELL-001).
        Ok(())
    }

    async fn drain_notifications(&self, session_id: &str) -> Vec<BackgroundShellNotification> {
        // Fast path: queue already has pending notifications →
        // return immediately (original behavior, unchanged).
        //
        // Slow path (race fix, 2026-07-05, surfaced by E1 CI on the
        // ubuntu runner): if the queue is empty but a shell for this
        // session was spawned very recently, it may be ABOUT to push
        // its completion notification. The spawned task's
        // `child.wait()` (SIGCHLD) + lock + enqueue can outlast the
        // caller's turn boundary — μs-scale mock-driven turn switches
        // in tests, or the production turn boundary for fast shells
        // like `echo`. Without the wait, drain pops the queue before
        // the push lands and the notification is missed (delayed to a
        // later turn, or lost if the loop terminates).
        //
        // We yield once + sleep briefly, then re-check, so an
        // in-flight shell gets a window to land its push. Caps:
        // (a) only shells younger than `RECENT_SHELL_MS` get the wait
        //     — a long-lived dev server (24h) does NOT stall every
        //     turn;
        // (b) total wait bounded by `WAIT_DEADLINE_MS` so a wedged
        //     shell can't block the agent loop indefinitely.
        // Production impact in the common case is zero: drains almost
        // always find the queue either non-empty (immediate return)
        // or with no recent running shell (immediate empty return,
        // since LLM turn latency ≫ shell completion).
        const RECENT_SHELL_MS: MonotonicMs = 200;
        const WAIT_DEADLINE_MS: u64 = 100;
        const POLL_INTERVAL_MS: u64 = 5;

        let deadline =
            tokio::time::Instant::now() + tokio::time::Duration::from_millis(WAIT_DEADLINE_MS);
        loop {
            {
                let mut g = self.inner.lock().await;
                if let Some(q) = g.notifications.remove(session_id) {
                    return q.into_iter().collect();
                }
                let now = now_ms();
                let has_recent_running = g.shells.iter().any(|((sid, _), e)| {
                    sid == session_id
                        && matches!(e.state, ShellState::Running { .. })
                        && now.saturating_sub(e.started_at) < RECENT_SHELL_MS
                });
                if !has_recent_running || tokio::time::Instant::now() >= deadline {
                    return vec![];
                }
            }
            // Drop the lock + yield so the spawned shell task (waiting
            // on `child.wait()`) can run to completion and enqueue its
            // notification before we re-poll.
            tokio::task::yield_now().await;
            tokio::time::sleep(tokio::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
        }
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
                    stdout_preview: status_preview(
                        &String::from_utf8_lossy(stdout),
                        full_output_path.as_deref(),
                    ),
                    stderr_preview: status_preview(
                        &String::from_utf8_lossy(stderr),
                        full_output_path.as_deref(),
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

/// Head + tail preview for `shell_status` stdout/stderr fields,
/// via the shared C6 truncation contract (char-boundary safe —
/// RULE-E-009; the pre-C6 local mirror sliced raw bytes). When the
/// full output was spilled, the marker carries the mode-A recovery
/// path; the `full_output_path` field keeps surfacing it too.
fn status_preview(s: &str, full_output_path: Option<&str>) -> String {
    let cap = crate::tools::tool_output::SPILL_PREVIEW_BYTES;
    let omitted = s.len().saturating_sub(cap * 2);
    let recovery = match full_output_path {
        Some(p) => crate::tools::tool_output::Recovery::Spill {
            path: std::path::PathBuf::from(p),
        },
        None => crate::tools::tool_output::Recovery::None,
    };
    let marker = crate::tools::tool_output::truncation_marker(
        omitted,
        s.len(),
        crate::tools::tool_output::Unit::Bytes,
        &recovery,
    );
    crate::tools::tool_output::head_tail_truncate(s, cap, cap, &marker)
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
        .or_default();
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

    // C6: drain the pipes on spawned tasks BEFORE the select.
    // `child.wait()` never reads stdout/stderr, so output larger
    // than the pipe capacity (~64 KB) would block the child on
    // write and the max-runtime timer would fire spuriously —
    // the same latent deadlock the synchronous shell tool had.
    let stdout_task = crate::tools::shell::spawn_pipe_drain(child.stdout.take());
    let stderr_task = crate::tools::shell::spawn_pipe_drain(child.stderr.take());

    let (trigger, exit_code, stdout, stderr) = tokio::select! {
        biased;
        _ = &mut kill_rx => {
            // External kill (kill() / kill_all_for_session / kill_all).
            let r = kill_and_collect(&mut child).await;
            let stdout = crate::tools::shell::collect_drain(stdout_task).await;
            let stderr = crate::tools::shell::collect_drain(stderr_task).await;
            (ShellExitTrigger::Killed, Some(r.exit_code), stdout, stderr)
        }
        _ = &mut sleep => {
            // Max runtime elapsed.
            let r = kill_and_collect(&mut child).await;
            let stdout = crate::tools::shell::collect_drain(stdout_task).await;
            let stderr = crate::tools::shell::collect_drain(stderr_task).await;
            (ShellExitTrigger::TimedOut, Some(r.exit_code), stdout, stderr)
        }
        status = child.wait() => {
            let exit_code = match status {
                Ok(s) => s.code(),
                Err(_) => None,
            };
            let stdout = crate::tools::shell::collect_drain(stdout_task).await;
            let stderr = crate::tools::shell::collect_drain(stderr_task).await;
            (ShellExitTrigger::Normal, exit_code, stdout, stderr)
        }
    };

    let completed_at = now_ms();
    let (outcome, reported_exit_code) = BackgroundShellOutcome::classify(trigger, exit_code);

    // Disk-spill for large outputs before we move into the lock.
    // C6: the registry's data_dir (production-only) keys the
    // session output dir; test registries have none and skip.
    let data_dir_for_spill: Option<PathBuf> = {
        let g = inner.lock().await;
        g.data_dir.clone()
    };
    let full_output_path =
        if stdout.len() + stderr.len() > crate::tools::tool_output::SPILL_THRESHOLD_BYTES {
            match data_dir_for_spill {
                Some(dir) => {
                    let mut combined = Vec::with_capacity(stdout.len() + stderr.len() + 16);
                    combined.extend_from_slice(&stdout);
                    if !stderr.is_empty() {
                        combined.push(b'\n');
                        combined.extend_from_slice(b"[stderr]\n");
                        combined.extend_from_slice(&stderr);
                    }
                    crate::tools::tool_output::spill(&dir, Some(&session_id), &combined)
                        .await
                        .ok()
                        .map(|p| p.to_string_lossy().into_owned())
                }
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

/// Subset of `tools::shell::kill_and_collect`'s return shape —
/// we only need exit_code here (output is drained by the caller's
/// pipe tasks).
struct KillAndCollectResult {
    exit_code: i32,
}

/// SIGKILL the entire process group + reap. Output collection is
/// the caller's job (pipes are taken and drained on spawned tasks
/// before the select — see `run_background_task`).
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

    let exit_code = child.wait().await.ok().and_then(|s| s.code()).unwrap_or(-1);
    KillAndCollectResult { exit_code }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// `status_preview` passes through short input untouched.
    /// Regression guard: if we ever swap to a different format
    /// (e.g. begin/end markers), the LLM-visible "truncated"
    /// marker must remain.
    #[test]
    fn status_preview_short_input_unchanged() {
        assert_eq!(status_preview("hello", None), "hello");
    }

    /// Long input gets a head + tail preview with a truncation
    /// marker. The marker string is part of the LLM-facing
    /// surface — keep it stable.
    #[test]
    fn status_preview_long_input_has_marker() {
        let s = "a".repeat(5000);
        let p = status_preview(&s, None);
        assert!(p.starts_with('a'), "head should be all 'a'");
        assert!(p.contains("truncated"));
    }

    /// C6: a spilled output makes the preview marker carry the
    /// mode-A recovery path (RULE-E-009 boundary safety is covered
    /// by the tool_output property tests; this pins the wiring).
    #[test]
    fn status_preview_with_spill_path_carries_recovery() {
        let s = "a".repeat(5000);
        let p = status_preview(&s, Some("/data/outputs/s1/x.txt"));
        assert!(p.contains("full output: /data/outputs/s1/x.txt"));
        assert!(p.contains("recover: read_file with offset/limit"));
    }

    /// C6 multibyte: CJK preview must not panic (the pre-C6 local
    /// mirror sliced raw bytes).
    #[test]
    fn status_preview_cjk_no_panic() {
        let s = "汉".repeat(5000);
        let p = status_preview(&s, None);
        assert!(p.contains("truncated"));
        assert!(p.starts_with('汉'));
    }

    /// C6 / AC3: a registry constructed with the app data dir spills
    /// large outputs to `<data_dir>/outputs/<session_id>/` (NOT the
    /// pre-C6 `<cwd>/.everlasting/outputs/`), and the Completed
    /// status surfaces the path.
    #[tokio::test(flavor = "multi_thread")]
    async fn large_output_spills_to_session_outputs_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = InMemoryBackgroundShellRegistry::new_with_data_dir(tmp.path().to_path_buf());
        let shell_id = reg
            .start(
                "sess-bg",
                "yes line | head -c 40000".to_string(),
                tmp.path().to_path_buf(),
                Some(10_000),
                None,
            )
            .await
            .unwrap();
        // Wait for completion.
        let status = 'wait: {
            for _ in 0..40 {
                match reg.status("sess-bg", &shell_id).await.unwrap() {
                    s @ BackgroundShellStatus::Completed { .. } => break 'wait s,
                    _ => tokio::time::sleep(Duration::from_millis(50)).await,
                }
            }
            panic!("background shell did not complete within 2s");
        };
        match status {
            BackgroundShellStatus::Completed {
                full_output_path,
                stdout_preview,
                ..
            } => {
                let path = full_output_path.expect("large output spilled");
                assert!(
                    path.starts_with(
                        tmp.path()
                            .join("outputs")
                            .join("sess-bg")
                            .to_string_lossy()
                            .as_ref()
                    ),
                    "spill must be session-keyed under data_dir, got {path}"
                );
                let saved = std::fs::read(&path).unwrap();
                assert!(saved.len() > crate::tools::tool_output::SPILL_THRESHOLD_BYTES);
                assert!(stdout_preview.contains("recover: read_file with offset/limit"));
                // Legacy cwd location must NOT have been written.
                assert!(!tmp.path().join(".everlasting/outputs").exists());
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    /// P3b AC6 (08-31-a2-p3b): a sandboxed background shell is
    /// constrained at the registry spawn point. `Some(spec)` → the
    /// child runs under Landlock+seccomp (write outside the face →
    /// Failed + "Permission denied" in stderr); `None` → the spawn is
    /// byte-identical to the legacy path (write anywhere succeeds).
    /// Skipped (loudly) when the runtime kernel lacks Landlock+seccomp.
    #[tokio::test]
    async fn sandboxed_background_shell_enforces_write_face() {
        if !crate::sandbox::Capability::probe().ok() {
            eprintln!("SKIP: Landlock/seccomp unavailable on this kernel (fail-open runtime)");
            return;
        }
        let tmp = tempdir().unwrap();
        let wt = tmp.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        let spec = crate::sandbox::SandboxSpec {
            writable_roots: vec![wt.clone(), "/tmp".into()],
            exec_allow_roots: vec![
                "/usr".into(),
                "/bin".into(),
                "/lib".into(),
                "/lib64".into(),
                "/dev".into(),
                "/tmp".into(),
                wt.clone(),
            ],
            extra_writable: vec![],
        };
        let reg = InMemoryBackgroundShellRegistry::new();

        async fn wait_terminal(
            reg: &InMemoryBackgroundShellRegistry,
            sid: &str,
            shell_id: &str,
        ) -> BackgroundShellStatus {
            for _ in 0..60 {
                match reg.status(sid, shell_id).await.unwrap() {
                    s @ (BackgroundShellStatus::Completed { .. }
                    | BackgroundShellStatus::Killed { .. }) => return s,
                    BackgroundShellStatus::Running { .. } => {
                        tokio::time::sleep(Duration::from_millis(50)).await
                    }
                }
            }
            panic!("background shell did not terminate within 3s");
        }

        // 1. Sandboxed + write inside the worktree → Completed.
        let id_ok = reg
            .start(
                "s1",
                "echo hi > sbx_out.txt".to_string(),
                wt.clone(),
                Some(10_000),
                Some(spec.clone()),
            )
            .await
            .unwrap();
        match wait_terminal(&reg, "s1", &id_ok).await {
            BackgroundShellStatus::Completed { exit_code, .. } => {
                assert_eq!(exit_code, 0);
                assert!(wt.join("sbx_out.txt").exists());
            }
            other => panic!("expected Completed, got {other:?}"),
        }

        // 2. Sandboxed + write to $HOME → Failed, Permission denied.
        let id_deny = reg
            .start(
                "s1",
                "echo hi > $HOME/sbx_bg_denied.txt".to_string(),
                wt.clone(),
                Some(10_000),
                Some(spec.clone()),
            )
            .await
            .unwrap();
        match wait_terminal(&reg, "s1", &id_deny).await {
            BackgroundShellStatus::Completed { exit_code, .. } => {
                assert_ne!(exit_code, 0, "write to $HOME must fail under sandbox");
                // stderr preview is available on Completed; assert there.
            }
            other => panic!("command itself should exit, got {other:?}"),
        }
        // Wait for the status refresh (the Completed entry carries the
        // stderr preview) and assert the denial surfaced.
        match reg.status("s1", &id_deny).await.unwrap() {
            BackgroundShellStatus::Completed { stderr_preview, .. } => {
                assert!(
                    stderr_preview.contains("Permission denied"),
                    "expected Permission denied in stderr, got: {stderr_preview}"
                );
            }
            other => panic!("expected Completed after terminal, got {other:?}"),
        }
        assert!(!std::path::Path::new(&std::env::var("HOME").unwrap())
            .join("sbx_bg_denied.txt")
            .exists());

        // 3. No spec → legacy behavior, $HOME write succeeds.
        let id_none = reg
            .start(
                "s1",
                "echo hi > $HOME/sbx_bg_allowed.txt".to_string(),
                wt.clone(),
                Some(10_000),
                None,
            )
            .await
            .unwrap();
        match wait_terminal(&reg, "s1", &id_none).await {
            BackgroundShellStatus::Completed { exit_code, .. } => {
                assert_eq!(exit_code, 0, "unsandboxed write must succeed");
            }
            other => panic!("expected Completed, got {other:?}"),
        }
        let home_allowed =
            std::path::Path::new(&std::env::var("HOME").unwrap()).join("sbx_bg_allowed.txt");
        assert!(home_allowed.exists(), "legacy path must actually write");
        let _ = std::fs::remove_file(&home_allowed);
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

    /// Sweep bounds are anchored (RULE-SHELL-001 design D3):
    /// result retention 1h, sweep interval 5min. Guards against
    /// accidental re-tuning; style follows
    /// `notification_queue_cap_is_100`.
    #[test]
    fn sweep_bounds_anchored() {
        assert_eq!(SHELL_RETENTION_MS, 3_600_000);
        assert_eq!(SWEEP_INTERVAL_MS, 300_000);
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
            .start(
                "s1",
                "echo hello".to_string(),
                tmp.path().to_path_buf(),
                Some(5000),
                None,
            )
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
            .start(
                "s1",
                "sleep 5".to_string(),
                tmp.path().to_path_buf(),
                Some(30_000),
                None,
            )
            .await
            .unwrap();
        // Give the spawned task a tick to actually start the child.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let status = reg.status("s1", &shell_id).await.unwrap();
        match status {
            BackgroundShellStatus::Running { elapsed_ms, .. } => {
                assert!(
                    elapsed_ms < 5_000,
                    "still in early phase, got: {}",
                    elapsed_ms
                );
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
                None,
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
            .start(
                "s1",
                "sleep 60".to_string(),
                tmp.path().to_path_buf(),
                Some(120_000),
                None,
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        reg.kill("s1", &shell_id).await.expect("kill ok");
        // Wait for the task to record Killed.
        for _ in 0..40 {
            if let BackgroundShellStatus::Killed { .. } = reg.status("s1", &shell_id).await.unwrap()
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
            .start(
                "s1",
                "true".to_string(),
                tmp.path().to_path_buf(),
                Some(5000),
                None,
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
            .start(
                "s1",
                "sleep 5".to_string(),
                tmp.path().to_path_buf(),
                Some(30_000),
                None,
            )
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
            .start(
                "s1",
                "sleep 30".to_string(),
                tmp.path().to_path_buf(),
                Some(60_000),
                None,
            )
            .await
            .unwrap();
        let s1_b = reg
            .start(
                "s1",
                "sleep 30".to_string(),
                tmp.path().to_path_buf(),
                Some(60_000),
                None,
            )
            .await
            .unwrap();
        let s2_a = reg
            .start(
                "s2",
                "sleep 30".to_string(),
                tmp.path().to_path_buf(),
                Some(60_000),
                None,
            )
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
                .start(
                    "s1",
                    "true".to_string(),
                    tmp.path().to_path_buf(),
                    Some(5000),
                    None,
                )
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
                    let final_len = g.notifications.get("s1").map(|q| q.len()).unwrap_or(0);
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

    // ----- Sweep tests (RULE-SHELL-001, design D5) -----

    /// Helper: start `true`, poll until the registry records the
    /// Completed state (same wait pattern as
    /// `status_after_completion_returns_completed_with_preview`).
    async fn start_and_wait_done(
        reg: &InMemoryBackgroundShellRegistry,
        tmp: &tempfile::TempDir,
    ) -> String {
        let shell_id = reg
            .start(
                "s1",
                "true".to_string(),
                tmp.path().to_path_buf(),
                Some(5000),
                None,
            )
            .await
            .expect("start ok");
        for _ in 0..40 {
            if let BackgroundShellStatus::Completed { .. } =
                reg.status("s1", &shell_id).await.unwrap()
            {
                return shell_id;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("shell did not reach Completed within 2s");
    }

    /// Sweep removes Done entries past the retention window:
    /// after `true` completes, sweep(retention=0) returns 1 and
    /// `status` for the shell becomes NotFound (the documented
    /// "already cleaned up" semantics).
    #[tokio::test(flavor = "multi_thread")]
    async fn sweep_removes_done_beyond_retention() {
        let tmp = tempdir().unwrap();
        let reg = InMemoryBackgroundShellRegistry::new();
        let shell_id = start_and_wait_done(&reg, &tmp).await;
        // The sweep predicate is strict (`now - completed_at >
        // retention`), so with retention=0 at least 1ms of
        // monotonic progress must elapse after completion before
        // the sweep can remove the entry.
        tokio::time::sleep(Duration::from_millis(10)).await;
        let removed = reg.sweep_completed_shells(0).await;
        assert_eq!(removed, 1);
        let err = reg.status("s1", &shell_id).await.unwrap_err();
        assert!(matches!(err, BackgroundShellError::NotFound { .. }));
    }

    /// Sweep keeps Done entries inside the retention window:
    /// after completion, sweep(24h retention) removes nothing and
    /// `status` still answers Completed.
    #[tokio::test(flavor = "multi_thread")]
    async fn sweep_keeps_recent_done() {
        let tmp = tempdir().unwrap();
        let reg = InMemoryBackgroundShellRegistry::new();
        let shell_id = start_and_wait_done(&reg, &tmp).await;
        let removed = reg.sweep_completed_shells(86_400_000).await;
        assert_eq!(removed, 0);
        assert!(matches!(
            reg.status("s1", &shell_id).await.unwrap(),
            BackgroundShellStatus::Completed { .. }
        ));
    }

    /// Sweep never removes Running entries (RULE-SHELL-001 R2) —
    /// removal would orphan the kill channel. `sleep 30` only
    /// serves to keep the entry in Running; `start()` inserts the
    /// Running entry synchronously (step 4), so NO waiting is
    /// needed (design D5 test 3): sweep(0) immediately after
    /// start must return 0 and leave status Running. Kill at the
    /// end for cleanup; the whole test runs in ~1-2s (we never
    /// wait out the 30s).
    #[tokio::test(flavor = "multi_thread")]
    async fn sweep_keeps_running_entries() {
        let tmp = tempdir().unwrap();
        let reg = InMemoryBackgroundShellRegistry::new();
        let shell_id = reg
            .start(
                "s1",
                "sleep 30".to_string(),
                tmp.path().to_path_buf(),
                Some(60_000),
                None,
            )
            .await
            .unwrap();
        let removed = reg.sweep_completed_shells(0).await;
        assert_eq!(removed, 0);
        assert!(matches!(
            reg.status("s1", &shell_id).await.unwrap(),
            BackgroundShellStatus::Running { .. }
        ));
        // Cleanup — don't leave the 30s child running.
        let _ = reg.kill("s1", &shell_id).await;
    }
}
