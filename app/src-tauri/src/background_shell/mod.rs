//! L1a — Background shell registry.
//!
//! This module provides the cross-request lifetime management for
//! "fire-and-forget" shell commands. The LLM can call
//! [`crate::tools::run_background_shell`] to start a long-running
//! command (build / install / test suite) and continue chatting;
//! when the shell exits the agent loop injects a system-style user
//! message at the start of the next turn, and the LLM can call
//! [`crate::tools::shell_status`] / [`crate::tools::shell_kill`]
//! to inspect / force-kill the background process.
//!
//! ## Design (L1 PRD, 2026-06-19)
//!
//! - **Trait abstraction (Q1, decision C)**: the registry is a
//!   trait so a future daemon-ization can swap the implementation
//!   for a Unix-socket forwarder without touching call sites. The
//!   GUI-process in-memory impl ships in this PR.
//! - **Session-scoped (Q7)**: every shell is keyed by
//!   `(session_id, shell_session_id)`. `status()` / `kill()` from
//!   another session return [`BackgroundShellError::WrongSession`].
//! - **Process-group kill (E-002)**: each background shell is its
//!   own process group leader; `kill()` sends SIGKILL to the
//!   entire group so descendants of `&` / `nohup` / pipelines are
//!   reaped along with the direct child. This reuses the same
//!   `kill_and_collect` logic as the synchronous `shell` tool.
//! - **Safe env (E-001)**: background children inherit only the
//!   curated allowlist (PATH + HOME / USER / LANG-family / TERM /
//!   TZ / TMPDIR). API keys / tokens are NOT inherited.
//! - **Resource bounds (Q6)**: each shell takes an optional
//!   `max_runtime_ms`; default 86_400_000 (24h), no upper cap.
//!   When the timer fires, the process group is killed and a
//!   "timed out" notification is pushed.
//! - **Notification queue (PRD §error-handling)**: each session
//!   has a bounded `VecDeque` of completion notifications (cap
//!   100). When a new notification would overflow, the oldest is
//!   dropped with `tracing::warn!` (no panic).
//!
//! Out of scope for L1a (L1b / future): PTY mode, real-time stdout
//! streaming, parallel subagent + worktree (L3), daemonization
//! itself. The trait's `start()` returns a `shell_session_id`
//! handle that L1b can reuse for `pty_write` / `pty_resize` style
//! extensions without breaking the existing 3-tool surface.
//!
//! See `.trellis/tasks/06-19-l1-shell-pty/prd.md` for the full
//! decision log (Q1-Q7).

pub mod in_memory;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use serde::Serialize;
use thiserror::Error;

/// Monotonic millisecond timestamp relative to process start.
/// Used in lieu of [`Instant`] in serializable types (LLM-visible
/// payloads) because `Instant` doesn't implement `Serialize`.
/// We only need relative ordering + elapsed-time math, both of
/// which work on `u64`.
pub type MonotonicMs = u64;

/// Capture the current monotonic millisecond. Convenience so
/// call sites don't have to spell out the `Instant::now() →
/// .elapsed().as_millis() as u64` dance.
pub fn now_ms() -> MonotonicMs {
    // We anchor at process start (the first call's Instant::now()
    // serves as the "epoch"). Subsequent calls measure elapsed
    // time from that anchor via the static below.
    use std::sync::OnceLock;
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let epoch = EPOCH.get_or_init(Instant::now);
    epoch.elapsed().as_millis() as MonotonicMs
}

/// Completion notification for a single background shell, drained
/// at the start of the next agent loop turn and surfaced to the
/// LLM as a `user`-role message ([`crate::agent::chat_loop.rs`]
/// injection point).
///
/// The wire shape (`Serialize`) is what the LLM-visible message
/// template consumes; `serde_json::to_string` on this struct must
/// remain stable across the implementation boundary.
#[derive(Debug, Clone, Serialize)]
pub struct BackgroundShellNotification {
    /// The background shell's session id (`bsh_xxx`). The LLM
    /// passes this to `shell_status` / `shell_kill`.
    pub shell_session_id: String,
    /// The chat session this shell belongs to.
    pub session_id: String,
    /// Final status (terminal only — `running` is never queued).
    pub outcome: BackgroundShellOutcome,
    /// Exit code if the process ran to completion (normal /
    /// killed / timed-out). `None` only for spawn-failure outcomes.
    pub exit_code: Option<i32>,
    /// When the shell was started (ms since process boot).
    pub started_at: MonotonicMs,
    /// When the shell exited (ms since process boot).
    pub completed_at: MonotonicMs,
}

/// The terminal outcome of a background shell. Mirrors the
/// status enum the LLM sees in `shell_status` responses, plus a
/// dedicated `SpawnFailed` for cases the LLM never gets to query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundShellOutcome {
    /// Process exited normally (exit_code may still be non-zero).
    Completed,
    /// Process exited with a non-zero status; the LLM gets the
    /// code in `exit_code`. Distinct from `Killed` so the LLM
    /// can tell "the command itself failed" vs "we killed it".
    Failed,
    /// `shell_kill` or `kill_all_for_session` or app shutdown
    /// killed the process group. exit_code is conventionally -1.
    Killed,
    /// `max_runtime_ms` elapsed and the process group was
    /// killed. exit_code is conventionally -1.
    TimedOut,
    /// The child could not be spawned at all (EACCES / ENOENT).
    /// `exit_code` is `None`.
    SpawnFailed,
}

impl BackgroundShellOutcome {
    /// Convert a `(exited_normally, exit_code)` pair + the trigger
    /// path (`killed` / `timed_out` / normal) into the appropriate
    /// outcome variant. Centralizes the "exit_code == 0 vs != 0"
    /// split so notification producers stay consistent.
    pub fn classify(
        trigger: ShellExitTrigger,
        exit_code: Option<i32>,
    ) -> (BackgroundShellOutcome, Option<i32>) {
        match trigger {
            ShellExitTrigger::Killed => (BackgroundShellOutcome::Killed, Some(-1)),
            ShellExitTrigger::TimedOut => (BackgroundShellOutcome::TimedOut, Some(-1)),
            ShellExitTrigger::Normal => match exit_code {
                Some(0) => (BackgroundShellOutcome::Completed, Some(0)),
                Some(c) => (BackgroundShellOutcome::Failed, Some(c)),
                // Wait returned no status at all — treat as a kill-like
                // failure so the LLM gets a definitive terminal signal.
                None => (BackgroundShellOutcome::Failed, Some(-1)),
            },
        }
    }
}

/// What caused the shell to exit (drives `BackgroundShellOutcome`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShellExitTrigger {
    Normal,
    Killed,
    TimedOut,
}

/// Snapshot of a background shell's current state. Returned by
/// the registry's `status()` for `shell_status` tool responses.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum BackgroundShellStatus {
    /// Process is still running; LLM can poll again or call `shell_kill`.
    Running {
        started_at: MonotonicMs,
        elapsed_ms: u64,
    },
    /// Process exited normally (exit_code may be non-zero).
    Completed {
        exit_code: i32,
        completed_at: MonotonicMs,
        /// `stdout` head + tail preview (≤ 1 KiB on each side).
        stdout_preview: String,
        /// `stderr` head + tail preview (same cap).
        stderr_preview: String,
        /// Set when combined output exceeded the disk-spill
        /// threshold (30 KiB, mirrors [`crate::tools::shell`]).
        full_output_path: Option<String>,
    },
    /// Process was killed (`shell_kill`, session delete, app
    /// exit) or timed out. exit_code is conventionally -1.
    Killed {
        exit_code: i32,
        completed_at: MonotonicMs,
    },
}

/// Domain error type for the registry. Every variant maps to an
/// `is_error: true` tool_result string the LLM can act on.
///
/// We use [`thiserror`] (not `anyhow`) because the registry sits
/// in the domain layer; the tool layer wraps this into the
/// LLM-facing string at the very edge (see
/// `.trellis/spec/backend/error-handling.md` "Error Types" —
/// the principle is "domain errors are typed, boundaries use
/// anyhow"). Public variants stay `#[non_exhaustive]` once we
/// know the surface (RULE-A-007 style: don't freeze the enum
/// until a second impl ships).
///
/// `WrongSession` / `InvalidCwd` / `Poisoned` are reserved for the
/// future (cross-session status, daemonization, std-Mutex swap) —
/// see module doc §Design. The current GUI-process impl never
/// constructs them: session-scope is enforced by the registry key
/// (Q7), cwd is pre-validated by `boundary::assert_within_root` in
/// the tool layer, and we use a `tokio::sync::Mutex` (no poison).
/// The variants stay reachable for the daemon impl.
#[derive(Debug, Error)]
#[allow(dead_code)] // reserved variants — see module doc §Design + doc above
pub enum BackgroundShellError {
    #[error("background shell '{shell_session_id}' not found for session '{session_id}'")]
    NotFound {
        session_id: String,
        shell_session_id: String,
    },
    #[error(
        "background shell '{shell_session_id}' is owned by session '{owner_session_id}', not '{caller_session_id}'"
    )]
    WrongSession {
        shell_session_id: String,
        owner_session_id: String,
        caller_session_id: String,
    },
    #[error("failed to spawn background shell: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("invalid working_directory '{path}': {reason}")]
    InvalidCwd { path: String, reason: String },
    #[error("registry poisoned: {0}")]
    Poisoned(String),
}

/// Trait abstraction for the background-shell registry.
///
/// PR1 ships one impl (`in_memory::InMemoryBackgroundShellRegistry`).
/// A future daemon-ization PR will add a second impl that
/// forwards to an external Agent Daemon over a Unix socket; the
/// call sites (Tauri commands + agent loop + `delete_session`
/// hook) stay identical.
///
/// All methods are `async fn`. Native `async fn` in trait has
/// been stable since Rust 1.75; we don't depend on the
/// `async-trait` crate. **Trade-off**: this trait is not
/// `dyn`-compatible (no `dyn BackgroundShellRegistry`); PR1
/// holds the impl as a concrete `Arc<InMemoryBackgroundShellRegistry>`
/// in `AppState`. The daemon-ization PR will either (a) wrap the
/// trait in a `BoxFuture`-shaped API for dyn use, or (b) keep the
/// concrete-type pattern and only swap the concrete type at the
/// `AppState::load` site. Option (b) is simpler and matches
/// every other cross-request state holder in `AppState`
/// (`MemoryCache`, `SkillCache`, `CommandCache`, `ReadGuard`).
pub trait BackgroundShellRegistry: Send + Sync {
    /// Start a background shell under `session_id`. Returns the
    /// new `shell_session_id` (format `bsh_<uuid>`).
    ///
    /// `cwd` MUST already be validated against the session's
    /// project root (callers run `projects::boundary::assert_within_root`
    /// first); the registry does not re-validate. This mirrors
    /// the synchronous `shell` tool's contract.
    async fn start(
        &self,
        session_id: &str,
        command: String,
        cwd: PathBuf,
        max_runtime_ms: Option<u64>,
    ) -> Result<String, BackgroundShellError>;

    /// Query a background shell's status. Returns
    /// [`BackgroundShellError::NotFound`] if the shell doesn't
    /// exist or was already cleaned up.
    async fn status(
        &self,
        session_id: &str,
        shell_session_id: &str,
    ) -> Result<BackgroundShellStatus, BackgroundShellError>;

    /// Force-kill the background shell's process group. Idempotent
    /// (returns `Ok(())` for already-completed shells — LLM may
    /// call after `shell_status` showed `Completed`).
    async fn kill(
        &self,
        session_id: &str,
        shell_session_id: &str,
    ) -> Result<(), BackgroundShellError>;

    /// Kill every background shell belonging to `session_id`.
    /// Called from `delete_session` and from the session-closing
    /// path in `lib.rs::chat` (when the last active request for a
    /// session exits).
    async fn kill_all_for_session(&self, session_id: &str) -> Result<(), BackgroundShellError>;

    /// Drain all pending completion notifications for `session_id`.
    /// Called from the agent loop at the start of every turn
    /// (before `provider.send`); the agent loop prepends each
    /// notification as a user-role message.
    async fn drain_notifications(
        &self,
        session_id: &str,
    ) -> Vec<BackgroundShellNotification>;

    /// Kill EVERY background shell across every session. Called
    /// from the Tauri `RunEvent::Exit` hook so app shutdown
    /// doesn't leave process groups running.
    async fn kill_all(&self) -> Result<(), BackgroundShellError>;
}

/// Convenience: re-export the in-memory impl's constructor as the
/// "default registry for the GUI process". Future daemon-ization
/// swaps this for a different `Arc<dyn BackgroundShellRegistry>`.
pub type DefaultRegistry = Arc<in_memory::InMemoryBackgroundShellRegistry>;

/// Build the default GUI-process registry. Lives here (not in
/// `in_memory`) so `AppState::load` doesn't have to know the
/// concrete module path.
pub fn default_registry() -> DefaultRegistry {
    Arc::new(in_memory::InMemoryBackgroundShellRegistry::new())
}

// ---------------------------------------------------------------------------
// Tests (cross-cutting)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// `BackgroundShellOutcome::classify` is the single source of
    /// truth for "what outcome does this exit look like". Any new
    /// exit path (cancel-via-token, OOM-killed, etc.) MUST be
    /// routed through this function so the LLM-facing outcome
    /// stays consistent.
    #[test]
    fn classify_outcome_normal_zero_is_completed() {
        let (o, code) = BackgroundShellOutcome::classify(ShellExitTrigger::Normal, Some(0));
        assert_eq!(o, BackgroundShellOutcome::Completed);
        assert_eq!(code, Some(0));
    }

    #[test]
    fn classify_outcome_normal_nonzero_is_failed() {
        let (o, code) = BackgroundShellOutcome::classify(ShellExitTrigger::Normal, Some(127));
        assert_eq!(o, BackgroundShellOutcome::Failed);
        assert_eq!(code, Some(127));
    }

    #[test]
    fn classify_outcome_normal_no_code_is_failed_minus_one() {
        let (o, code) = BackgroundShellOutcome::classify(ShellExitTrigger::Normal, None);
        assert_eq!(o, BackgroundShellOutcome::Failed);
        assert_eq!(code, Some(-1));
    }

    #[test]
    fn classify_outcome_killed_is_killed_minus_one() {
        let (o, _) = BackgroundShellOutcome::classify(ShellExitTrigger::Killed, Some(0));
        assert_eq!(o, BackgroundShellOutcome::Killed);
    }

    #[test]
    fn classify_outcome_timed_out_is_timed_out_minus_one() {
        let (o, _) = BackgroundShellOutcome::classify(ShellExitTrigger::TimedOut, Some(0));
        assert_eq!(o, BackgroundShellOutcome::TimedOut);
    }

    /// The error type's `Display` impl is what the tool layer
    /// surfaces to the LLM as `is_error: true` content. Keep it
    /// human-readable (session id always present).
    #[test]
    fn error_display_includes_session_and_shell_id() {
        let e = BackgroundShellError::NotFound {
            session_id: "s1".to_string(),
            shell_session_id: "bsh_a".to_string(),
        };
        let s = format!("{}", e);
        assert!(s.contains("s1"), "got: {}", s);
        assert!(s.contains("bsh_a"), "got: {}", s);
    }

    #[test]
    fn error_display_wrong_session_includes_both_ids() {
        let e = BackgroundShellError::WrongSession {
            shell_session_id: "bsh_a".to_string(),
            owner_session_id: "s1".to_string(),
            caller_session_id: "s2".to_string(),
        };
        let s = format!("{}", e);
        assert!(s.contains("s1"));
        assert!(s.contains("s2"));
        assert!(s.contains("bsh_a"));
    }
}