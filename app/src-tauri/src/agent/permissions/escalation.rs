//! P3c escalation loop (design §5) — sandbox failure → user approval
//! → one-shot unsandboxed rerun, for the foreground `shell` tool.
//!
//! When a sandboxed command fails with a stderr that smells like a
//! sandbox denial (out-of-face write / seccomp network block), the
//! tool escalates instead of only appending guidance:
//!
//! 1. **Prefix-grant hit** (same gate as Tier 4: compound commands
//!    with structural metacharacters never enjoy the grant) → the
//!    user already always-allowed this command prefix → rerun
//!    unsandboxed directly, no card.
//! 2. **No grant** → an Ask card via `ask_path` (QuestionStore
//!    oneshot, 120s timeout, cancel-safe — the identical machinery
//!    Tier 4 uses). AllowOnce / AllowBoth-Always rerun the EXACT
//!    command text once without the sandbox; Deny returns the
//!    failure + guidance to the model.
//!
//! Why approval binds the exact command text (D4): the rerun is the
//! command the user saw on the card — no LLM paraphrasing drift, and
//! the audit rows (ask-side `tool_permission_ask` /
//! `permission_granted` / `tool_denied`) describe a real thing.
//!
//! Double-execution boundary (accepted, D4): escalation only fires
//! when the sandboxed first attempt FAILED on an out-of-face denial —
//! the dangerous part never ran once; the in-face part re-runs the
//! same way any approved command would. The rerun itself never
//! re-escalates (at most one escalation per tool call — the loop is
//! structured as "first run → maybe one rerun", not a while-loop).
//!
//! Plan mode NEVER escalates (D3: Plan's value is the deterministic
//! read-only face) — the shell tool gates the whole block on
//! `mode != Plan` before consulting the handle.
//!
//! Worker subagents: `ask_path` already supports the worker
//! round-trip (store keying `worker:<run_id>`, transcript-only
//! audit); the handle carries the worker's `PermissionContext` so
//! escalation cards work there with no special-casing.

use std::sync::Arc;

use sqlx::SqlitePool;

use crate::sandbox::SandboxBlockKind;
use crate::state::ChatEventSink;

use super::ask::ask_path;
use super::shell_trust::{first_token_for_allow_always, has_structural_metachar};
use super::types::PermissionContext;
use super::PermissionStore;

/// Per-tool-call handle injected into `ToolContext.escalation` by the
/// chat loop's serial dispatch (only for `shell`; the background
/// shell keeps model-mediated guidance this iteration). `None`
/// (test paths) → the escalation degrades to guidance-only — the
/// shell tool's `failure_guidance` append.
#[derive(Clone, Default)]
pub struct EscalationHandle {
    inner: Option<Arc<EscalationInner>>,
}

struct EscalationInner {
    sink: Arc<dyn ChatEventSink>,
    store: PermissionStore,
    ctx: PermissionContext,
    /// Pool clone for `ask_path`'s audit rows (the shell tool owns
    /// the canonical `ctx.db`; this is the same pool).
    db: SqlitePool,
    token: tokio_util::sync::CancellationToken,
    tool_use_id: String,
}

impl EscalationHandle {
    pub fn new(
        sink: Arc<dyn ChatEventSink>,
        store: PermissionStore,
        ctx: PermissionContext,
        db: SqlitePool,
        token: tokio_util::sync::CancellationToken,
        tool_use_id: String,
    ) -> Self {
        EscalationHandle {
            inner: Some(Arc::new(EscalationInner {
                sink,
                store,
                ctx,
                db,
                token,
                tool_use_id,
            })),
        }
    }

    /// True when no handle was injected (tests / non-serial paths) —
    /// the caller falls back to guidance-only.
    pub fn is_none(&self) -> bool {
        self.inner.is_none()
    }

    /// The full ask round-trip (design §5.2). The card carries the
    /// original command text + the interception cause + the stderr
    /// line, so the user can judge without re-running anything.
    /// Emits `permission:ask`, writes the ask-side audit rows, and
    /// (on AllowAlways) persists the prefix grant through `ask_path`'s
    /// existing channel — the same rows/kinds the Tier 4 flow writes.
    pub async fn ask(
        &self,
        tool_input: &serde_json::Value,
        command: &str,
        kind: SandboxBlockKind,
        stderr: &str,
    ) -> EscalationOutcome {
        let inner = match &self.inner {
            Some(i) => i,
            None => return EscalationOutcome::Unavailable,
        };
        let reason = escalation_reason(kind, command, stderr);
        let decision = ask_path(
            &inner.sink,
            &inner.db,
            &inner.store,
            &inner.ctx,
            "shell",
            tool_input,
            command,
            // Shell asks render the command inline; no path-scope row.
            None,
            &inner.tool_use_id,
            &inner.token,
            Some(&reason),
        )
        .await;
        match decision {
            super::types::Decision::Allow => EscalationOutcome::Approved,
            // Timeout / cancel / user denial all collapse to Denied —
            // the model gets the failure + guidance either way.
            _ => EscalationOutcome::Denied,
        }
    }
}

/// Result of the escalation round-trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscalationOutcome {
    /// User approved (AllowOnce or AllowAlways — the grant write is
    /// `ask_path`'s business) → rerun the command unsandboxed.
    Approved,
    /// User denied / timed out / cancelled → keep the failed result
    /// and append the mode-aware guidance.
    Denied,
    /// No handle (test paths) → guidance-only; never escalates.
    Unavailable,
}

/// True when the user's earlier AllowAlways on this command prefix
/// already covers the rerun (design §5.2 top branch / AC6). Same gate
/// as Tier 4: a compound command (structural metacharacters) never
/// enjoys the grant — the prefix would only cover the first segment.
/// The check mirrors `check::permission::check_prefix_grant`
/// (read-side `tool_name IN ('shell','run_background_shell')`).
pub async fn prefix_grant_hit(db: &SqlitePool, session_id: &str, command: &str) -> bool {
    if has_structural_metachar(command) {
        return false;
    }
    let first_token = first_token_for_allow_always(command);
    if first_token.is_empty() {
        return false;
    }
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        SELECT 1 FROM session_tool_permissions
        WHERE session_id = ?
          AND tool_name IN ('shell', 'run_background_shell')
          AND match_kind = 'prefix'
          AND match_value = ?
        LIMIT 1
        "#,
    )
    .bind(session_id)
    .bind(first_token)
    .fetch_optional(db)
    .await
    .unwrap_or(None);
    row.is_some()
}

/// Card reason text (design §5.2 payload): interception cause +
/// original command + the stderr line that triggered the card.
fn escalation_reason(kind: SandboxBlockKind, command: &str, stderr: &str) -> String {
    let cause = match kind {
        SandboxBlockKind::Write => "an out-of-face write was blocked by the sandbox",
        SandboxBlockKind::Network => "outbound network was blocked by the sandbox",
    };
    format!(
        "The command was stopped because {cause}. Approving re-runs this exact command \
         ONCE without the sandbox.\nCommand: {command}\nstderr: {}",
        stderr_evidence_line(stderr),
    )
}

/// Pick the stderr line that proves the denial (first line containing
/// a known denial string, else the last non-empty line), truncated —
/// the card stays readable.
fn stderr_evidence_line(stderr: &str) -> String {
    const MARKERS: [&str; 3] = [
        "Permission denied",
        "Read-only file system",
        "Operation not permitted",
    ];
    let line = stderr
        .lines()
        .find(|l| MARKERS.iter().any(|m| l.contains(m)))
        .or_else(|| stderr.lines().rev().find(|l| !l.trim().is_empty()))
        .unwrap_or("");
    line.chars().take(200).collect()
}
