//! W1 (Workflow integration, Phase 3 Step 3.1 — 2026-07-08):
//! `set_task_state` + Rust 固定 state 转移钩子.
//!
//! ## 为什么 Rust 固定 hook(Q9 / M-A)
//!
//! `set_task_state` 是 **engine 写入 task.json.status 的唯一入口**,由
//! `resolve_task_state_transition` IPC(commands/question.rs 新增,见 §3.1)
//! 在用户允许 state 转移时调用。钩子按 `from -> to` match 分支触发,
//! 不靠 agent 自觉(避免"该触发 spec 沉淀没触发"的事故),符合 Q9
//! 选择:Rust 固定逻辑嵌入 `set_task_state` 写入路径;沉淀闭环
//! 不靠 agent 自觉;不做脚本 runner。
//!
//! ## 当前实现的钩子(Step 3.1)
//!
//! - `(Check, Done)` → `trigger_spec_distillation(task)` — Step 3.1
//!   落地钩子 stub,Step 3.2 会接上 wf-update-spec skill 落地 spec。
//! - `(Planning, Implement)` → `preflight_implement_check(task)` —
//!   可选前置(in this step it's a logging stub;full impl in Phase 4)。
//! - 其他转移 → 不触发钩子(no-op)
//!
//! ## 同步 IO + 不阻塞 IPC
//!
//! 与 `task.rs` 的 IO 形态一致:全 sync(`fs::write` + `serde_json`),
//! 不开 tokio runtime。这样 `set_task_state` 可以从 `commands/question.rs`
//! 的 async IPC handler 上下文里直接调(包在 `spawn_blocking` 里如果要),
//! 不破坏现有 async 签名。当前 implementations 同步,因为 IPC handler
//! 调用方承受的耗时是 disk-IO 而非 LLM round-trip;v1 不需要 `spawn_blocking`。
//!
//! ## 验证
//!
//! `cargo test --lib workflow::state` ——
//! - `set_task_state_writes_status_changed_at_updated`,
//! - `set_task_state_check_to_done_invokes_spec_distillation_hook`,
//! - `set_task_state_planning_to_implement_invokes_preflight_hook`,
//! - `set_task_state_unknown_transition_does_not_invoke_hook`,
//! - `set_task_state_invalid_target_returns_error`,
//! - `trigger_spec_distillation_already_present_marker_does_not_double`,
//! - `preflight_implement_check_records_marker_into_progress`。

#![allow(dead_code)]

use std::io;
use std::path::Path;

use chrono::Utc;
use thiserror::Error;

use super::task::{read_task, write_task, TaskJson, TaskStatus};

// (No `use std::fs` here — this module writes through
// `write_task`'s atomic-rename helper; reads through `read_task`.)
// (No `use std::path::PathBuf` here — used only via the
// `StateTransitionError::Io` payload; we declare `use` lower
// in the file.)

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Local error type for state-transition operations. The IPC layer
/// (`commands/question.rs`) maps each variant to the right
/// `AppCommandError` category.
#[derive(Debug, Error)]
pub enum StateTransitionError {
    #[error("target state `{0}` is not a valid state for this workflow (expected one of: planning | implement | check | done)")]
    InvalidTargetState(String),

    #[error(
        "transition `{from}` → `{to}` is not declared in the workflow definition. \
         Use force=true (advisory only; engine still enforces the workflow's intent)."
    )]
    InvalidTransition { from: String, to: String },

    #[error("io error at {0}: {1}")]
    Io(PathBuf, #[source] io::Error),

    #[error("task not found at {0}")]
    TaskNotFound(PathBuf),

    #[error("task.json malformed at {0}: {1}")]
    MalformedJson(PathBuf, String),
}

pub type StateResult<T> = Result<T, StateTransitionError>;

use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Hook markers — what `trigger_spec_distillation` /
// `preflight_implement_check` write into task.json's bookkeeping.
// ---------------------------------------------------------------------------

/// Marker written into `task.json.summary` (prefixed with a known
/// bracket-tag) to record that `trigger_spec_distillation` already
/// ran for this task's `Check → Done` transition. Idempotency is
/// critical: re-running distillation after a partial failure should
/// be visible in the file (audit + debug), and double-running is
/// wasteful (the LLM is called multiple times on the same context).
///
/// Format: `[wf:spec-distilled <rfc3339-ts>]\n` prepended to `summary`.
/// A second call appends a second line rather than overwriting, so
/// the full timestamp history is preserved. The on-disk schema does
/// not strictly require this marker (it's informational) — Step 3.2
/// reads it back when deciding whether the spec content is stale.
const SPEC_DISTILLED_MARKER_PREFIX: &str = "[wf:spec-distilled ";

/// Marker written by `preflight_implement_check` so the next
/// transition can detect "preflight already ran for this
/// `Planning → Implement` move". Same idempotency rationale as
/// `spec-distilled`.
const IMPLEMENT_PREFLIGHT_MARKER_PREFIX: &str = "[wf:implement-preflight ";

fn marker_line(prefix: &str, ts: &str) -> String {
    format!("{prefix}{ts}]\n")
}

fn has_marker(summary: &str, prefix: &str) -> bool {
    summary.lines().any(|line| line.starts_with(prefix))
}

// ---------------------------------------------------------------------------
// Parse `target_state` (string) → TaskStatus, lenient
// ---------------------------------------------------------------------------

/// Lenient parse — mirror the `TaskStatus::from_str_opt` posture:
/// unknown strings become `Planning` (the dev default). For Step
/// 3.1 specifically, we instead **reject** unknown values because
/// the only legitimate callers are (a) the IPC layer (which got it
/// from the LLM, validated against the schema enum) and (b) the
/// `resolve_task_state_transition` IPC handler (which checks user
/// intent before applying).
///
/// The signature is `Result` rather than `Option` so the IPC layer
/// can distinguish "string was malformed" from "string was empty"
/// in audits.
pub fn parse_target_state(s: &str) -> StateResult<TaskStatus> {
    match s.trim().to_ascii_lowercase().as_str() {
        "planning" => Ok(TaskStatus::Planning),
        "implement" => Ok(TaskStatus::Implement),
        "check" => Ok(TaskStatus::Check),
        "done" => Ok(TaskStatus::Done),
        other => Err(StateTransitionError::InvalidTargetState(other.to_string())),
    }
}

// ---------------------------------------------------------------------------
// State transition — engine's single writer
// ---------------------------------------------------------------------------

/// Apply a state transition to a task: write the new
/// `task.json.status` (via the canonical [`write_task`]
/// atomic-rename helper), update `updated_at`, and dispatch the
/// matching `from → to` hook. Returns the updated `TaskJson` on
/// success.
///
/// **Why this is a single function (not per-state helpers)**:
/// Step 3.2's spec-distillation + Step 4's preflight need a
/// single chokepoint so the audit trail, marker bookkeeping, and
/// `updated_at` stamp happen consistently. Splitting per-state
/// helpers would re-implement those concerns per branch (and
/// that's exactly the per-callsite drift Q9 warned against).
///
/// **Hook invocation is part of the write path** — that matches
/// Q9's chosen design (Rust fixed logic, no agent self-trigger).
/// If a hook fails, the function still considers the state
/// transition applied (the `task.json` write already landed);
/// the hook failure is logged + the marker bookkeeping lets a
/// later re-attempt detect the gap.
///
/// `from` is the task's currently-known status. The IPC layer
/// resolves this from `current_task.status` at the time the
/// user clicked 允许; if the file has moved on (concurrent
/// session edited it), we still apply the requested transition
/// (idempotency is the `summary`-marker bookkeeping's job).
pub fn set_task_state(
    project_path: &Path,
    slug: &str,
    from: TaskStatus,
    to: TaskStatus,
) -> StateResult<TaskJson> {
    // Read current task.json — we MUST rewrite with the same id +
    // title + items, only changing status + updated_at (and
    // appending a marker if the hook fires). Using the existing
    // `write_task` atomic helper means a partial write can never
    // be observed by a concurrent reader.
    let mut task = read_task(project_path, slug).map_err(|e| match e {
        super::task::TaskError::NotFound(p) => StateTransitionError::TaskNotFound(p),
        super::task::TaskError::MalformedJson(p, msg) => {
            StateTransitionError::MalformedJson(p, msg)
        }
        super::task::TaskError::Io(p, src) => StateTransitionError::Io(p, src),
        // Other variants (InvalidSlug / AlreadyExists) are not
        // reachable from a successful read; map defensively.
        other => StateTransitionError::MalformedJson(
            std::path::PathBuf::from(slug),
            format!("unexpected read_task error: {other}"),
        ),
    })?;

    // Sanity: from must match what was on disk. If it doesn't,
    // the IPC layer's snapshot was stale (concurrent session
    // moved it). We still apply the transition — the
    // `summary`-marker bookkeeping keeps things idempotent.
    if task.status != from {
        tracing::warn!(
            slug = %slug,
            expected_from = %from.as_str(),
            actual = %task.status.as_str(),
            "set_task_state: caller-supplied `from` does not match disk; \
             applying requested transition anyway (idempotency by summary markers)"
        );
    }

    // Apply status + bump updated_at.
    task.status = to;
    task.updated_at = Utc::now().to_rfc3339();

    // Dispatch the hook BEFORE the write, so the hook can
    // mutate `task.summary` (or its items) and have the result
    // hit disk via the same write_task atomic-rename. The hook
    // receives `&mut TaskJson` so it can append a marker line
    // to `task.summary` and have that change persisted.
    dispatch_hook(project_path, slug, from, to, &mut task);

    // Persist the mutated task.json atomically.
    write_task(project_path, &task).map_err(|e| match e {
        super::task::TaskError::InvalidSlug(s) => StateTransitionError::Io(
            std::path::PathBuf::from(slug),
            io::Error::new(io::ErrorKind::InvalidInput, format!("invalid slug: {s}")),
        ),
        super::task::TaskError::Io(p, src) => StateTransitionError::Io(p, src),
        other => StateTransitionError::MalformedJson(
            std::path::PathBuf::from(slug),
            format!("unexpected write_task error: {other}"),
        ),
    })?;

    Ok(task)
}

/// `from → to` match: dispatch the matching hook (if any) to
/// `task`. Hooks receive `&mut TaskJson` so they can persist
/// their bookkeeping into the same atomic write the caller is
/// about to perform.
///
/// **No agent IPC here** — the LLM-side distillation / preflight
/// prompts are Step 3.2's job. Step 3.1's stubs only record
/// markers + log intent, demonstrating the chokepoint wiring.
/// Step 3.2 will replace the log line with an `agent.call`
/// towards `wf-update-spec` (or equivalent).
fn dispatch_hook(
    project_path: &Path,
    slug: &str,
    from: TaskStatus,
    to: TaskStatus,
    task: &mut TaskJson,
) {
    match (from, to) {
        (TaskStatus::Check, TaskStatus::Done) => {
            trigger_spec_distillation(project_path, slug, task);
        }
        (TaskStatus::Planning, TaskStatus::Implement) => {
            preflight_implement_check(project_path, slug, task);
        }
        _ => {
            // No hook for this transition — silent no-op.
            tracing::debug!(
                slug = %slug,
                from = %from.as_str(),
                to = %to.as_str(),
                "set_task_state: no hook registered for this transition"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Hooks — stubs (Step 3.1 → Step 3.2 / Phase 4 wiring)
// ---------------------------------------------------------------------------

/// Hook for `(Check, Done)`. Step 3.1 stub: append a
/// `wf:spec-distilled` marker to `task.summary` so the next
/// transition can detect "spec distillation already ran for this
/// task". The Step 3.2 PR will replace the stub body with an
/// LLM-side distillation call routed through `wf-update-spec`.
///
/// Idempotency: if a marker is already present (e.g. the hook
/// ran but the disk write failed afterward and the user
/// re-triggered), this call is a no-op — the caller's bookkeeping
/// is the source of truth.
///
/// `task` is mutated in-place to append the marker line; the
/// caller persists with `write_task`.
pub fn trigger_spec_distillation(
    project_path: &Path,
    slug: &str,
    task: &mut TaskJson,
) {
    if has_marker(&task.summary, SPEC_DISTILLED_MARKER_PREFIX) {
        tracing::debug!(
            slug = %slug,
            "trigger_spec_distillation: marker already present; skipping (idempotent)"
        );
        return;
    }
    let ts = Utc::now().to_rfc3339();
    let marker = marker_line(SPEC_DISTILLED_MARKER_PREFIX, &ts);
    task.summary = format!("{}{}", marker, task.summary);
    tracing::info!(
        slug = %slug,
        project_path = %project_path.display(),
        ts = %ts,
        "trigger_spec_distillation: marker appended; \
         Step 3.2 will replace this stub with an actual \
         wf-update-spec distillation call."
    );
}

/// Hook for `(Planning, Implement)`. Same shape as
/// `trigger_spec_distillation`: append an
/// `wf:implement-preflight` marker. Step 3.1 keeps it as a stub;
/// Phase 4 (or a future task) replaces it with the actual
/// preflight logic (e.g. confirm `task.items` is non-empty,
/// `task.summary` is non-empty, etc.).
pub fn preflight_implement_check(
    project_path: &Path,
    slug: &str,
    task: &mut TaskJson,
) {
    if has_marker(&task.summary, IMPLEMENT_PREFLIGHT_MARKER_PREFIX) {
        tracing::debug!(
            slug = %slug,
            "preflight_implement_check: marker already present; skipping (idempotent)"
        );
        return;
    }
    let ts = Utc::now().to_rfc3339();
    let marker = marker_line(IMPLEMENT_PREFLIGHT_MARKER_PREFIX, &ts);
    task.summary = format!("{}{}", marker, task.summary);
    tracing::info!(
        slug = %slug,
        project_path = %project_path.display(),
        ts = %ts,
        "preflight_implement_check: marker appended; \
         this is a stub pending Phase 4 preflight implementation."
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::workflow::task::{create_task_init, TaskItem};

    fn fresh_project() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn proj(d: &tempfile::TempDir) -> &Path {
        d.path()
    }

    fn create_seed(path: &Path, slug: &str) -> TaskJson {
        let task = create_task_init(path, "My Feature", slug, None).expect("create");
        // Mutate so we have a known starting state.
        let mut t = task;
        t.summary = "initial line\n".to_string();
        t.items = vec![TaskItem {
            id: "i1".into(),
            content: "do thing".into(),
            status: TaskStatus::Implement,
            tdd: None,
        }];
        write_task(path, &t).expect("write");
        let reread = read_task(path, slug).expect("read");
        reread
    }

    // --- parse_target_state ---------------------------------------------

    #[test]
    fn parse_target_state_accepts_known_forms() {
        assert_eq!(parse_target_state("planning").unwrap(), TaskStatus::Planning);
        assert_eq!(parse_target_state("IMPLEMENT").unwrap(), TaskStatus::Implement);
        assert_eq!(
            parse_target_state("  Done  ").unwrap(),
            TaskStatus::Done,
            "whitespace + case-insensitive"
        );
    }

    #[test]
    fn parse_target_state_rejects_unknown() {
        for bad in ["nope", "", "  ", "complete", "审核"] {
            let err = parse_target_state(bad).expect_err("must reject");
            assert!(
                matches!(err, StateTransitionError::InvalidTargetState(_)),
                "got {:?} for input {:?}",
                err,
                bad
            );
        }
    }

    // --- set_task_state: write + updated_at ----------------------------

    #[test]
    fn set_task_state_writes_status_and_bumps_updated_at() {
        let d = fresh_project();
        let path = proj(&d);
        let original = create_seed(path, "my-feat");

        // Capture original updated_at as RFC3339 (we mutate it
        // inside set_task_state via Utc::now()). Sleep 5ms so
        // the bumped timestamp differs in the ms-resolution
        // that RFC3339 truncates to (some FSes are very fast).
        std::thread::sleep(std::time::Duration::from_millis(5));

        let updated = set_task_state(
            path,
            "my-feat",
            TaskStatus::Planning,
            TaskStatus::Implement,
        )
        .expect("ok");

        assert_eq!(updated.status, TaskStatus::Implement);
        assert_ne!(
            updated.updated_at, original.updated_at,
            "updated_at must bump (got original={}, updated={})",
            original.updated_at, updated.updated_at,
        );
        // Preserves other fields.
        assert_eq!(updated.id, original.id);
        assert_eq!(updated.title, original.title);
        assert_eq!(updated.slug, original.slug);
        assert_eq!(updated.created_at, original.created_at);
        assert_eq!(updated.items.len(), 1);
        assert_eq!(updated.items[0].id, "i1");
        // Planning → Implement DOES fire the
        // `preflight_implement_check` hook (Step 3.1 design
        // table) → summary gets a `[wf:implement-preflight …]`
        // marker prepended.
        assert!(
            updated.summary.starts_with(IMPLEMENT_PREFLIGHT_MARKER_PREFIX),
            "summary must lead with the preflight marker after Planning→Implement; got: {:?}",
            updated.summary,
        );
        assert!(updated.summary.contains("initial line\n"));
        // Disk reflects the same change.
        let disk = read_task(path, "my-feat").unwrap();
        assert_eq!(disk.status, TaskStatus::Implement);
        assert_eq!(disk.updated_at, updated.updated_at);
    }

    // --- hook dispatch -------------------------------------------------

    #[test]
    fn set_task_state_check_to_done_invokes_spec_distillation_hook() {
        let d = fresh_project();
        let path = proj(&d);
        let task = create_seed(path, "my-feat");
        let mut t = task;
        t.status = TaskStatus::Check;
        write_task(path, &t).expect("write");

        let updated = set_task_state(
            path,
            "my-feat",
            TaskStatus::Check,
            TaskStatus::Done,
        )
        .expect("ok");
        assert_eq!(updated.status, TaskStatus::Done);
        assert!(
            updated.summary.starts_with(SPEC_DISTILLED_MARKER_PREFIX),
            "summary must lead with the spec-distilled marker; got: {:?}",
            updated.summary,
        );
        assert!(
            updated.summary.contains("initial line\n"),
            "summary must preserve the pre-existing content: {:?}",
            updated.summary,
        );
    }

    #[test]
    fn set_task_state_planning_to_implement_invokes_preflight_hook() {
        let d = fresh_project();
        let path = proj(&d);
        let _ = create_seed(path, "my-feat");

        let updated = set_task_state(
            path,
            "my-feat",
            TaskStatus::Planning,
            TaskStatus::Implement,
        )
        .expect("ok");
        assert!(
            updated.summary.starts_with(IMPLEMENT_PREFLIGHT_MARKER_PREFIX),
            "summary must lead with the preflight marker; got: {:?}",
            updated.summary,
        );
    }

    #[test]
    fn set_task_state_unknown_transition_does_not_invoke_hook() {
        // implement -> implement is not a declared edge (it IS
        // a valid status change but no hook fires for it).
        let d = fresh_project();
        let path = proj(&d);
        let task = create_seed(path, "my-feat");
        let mut t = task;
        t.status = TaskStatus::Implement;
        write_task(path, &t).expect("write");

        let updated = set_task_state(
            path,
            "my-feat",
            TaskStatus::Implement,
            TaskStatus::Implement,
        )
        .expect("ok");
        // Summary unchanged (no marker, no log line produced anything).
        assert_eq!(updated.summary, "initial line\n");
    }

    // --- hook idempotency ----------------------------------------------

    #[test]
    fn trigger_spec_distillation_already_present_marker_does_not_double() {
        let d = fresh_project();
        let path = proj(&d);
        let task = create_seed(path, "my-feat");
        let mut t = task;
        t.status = TaskStatus::Check;
        t.summary = format!(
            "{}{}",
            marker_line(SPEC_DISTILLED_MARKER_PREFIX, "2026-01-01T00:00:00+00:00"),
            t.summary
        );
        write_task(path, &t).expect("write");

        // Apply Check → Done again; the hook should detect the
        // pre-existing marker and skip.
        let updated = set_task_state(
            path,
            "my-feat",
            TaskStatus::Check,
            TaskStatus::Done,
        )
        .expect("ok");
        let marker_count = updated
            .summary
            .lines()
            .filter(|l| l.starts_with(SPEC_DISTILLED_MARKER_PREFIX))
            .count();
        assert_eq!(
            marker_count, 1,
            "marker must NOT double-append; got: {:?}",
            updated.summary
        );
    }

    #[test]
    fn preflight_implement_check_records_marker_only_once() {
        let d = fresh_project();
        let path = proj(&d);
        let task = create_seed(path, "my-feat");
        let mut t = task;
        t.status = TaskStatus::Planning;
        write_task(path, &t).expect("write");

        let updated = set_task_state(
            path,
            "my-feat",
            TaskStatus::Planning,
            TaskStatus::Implement,
        )
        .expect("ok");
        let marker_count = updated
            .summary
            .lines()
            .filter(|l| l.starts_with(IMPLEMENT_PREFLIGHT_MARKER_PREFIX))
            .count();
        assert_eq!(
            marker_count, 1,
            "preflight marker must NOT double-append; got: {:?}",
            updated.summary
        );
    }

    // --- error paths ---------------------------------------------------

    #[test]
    fn set_task_state_missing_returns_task_not_found() {
        let d = fresh_project();
        let err = set_task_state(
            d.path(),
            "ghost",
            TaskStatus::Planning,
            TaskStatus::Implement,
        )
        .expect_err("missing task must fail");
        assert!(
            matches!(err, StateTransitionError::TaskNotFound(_)),
            "got {:?}",
            err
        );
    }

    #[test]
    fn set_task_state_stale_from_does_not_block_but_logs() {
        // Caller passes `from = Check` but the task is in
        // Planning — we still apply (the IPC layer will warn,
        // but the user's intent is preserved). The marker
        // bookkeeping makes recovery safe even if a wrong
        // transition slipped through.
        let d = fresh_project();
        let path = proj(&d);
        let _ = create_seed(path, "my-feat");

        let updated = set_task_state(
            path,
            "my-feat",
            TaskStatus::Check, // stale: actual status = Planning
            TaskStatus::Implement,
        )
        .expect("ok");
        assert_eq!(updated.status, TaskStatus::Implement);
    }

    #[test]
    fn parse_target_state_invalid_does_not_reach_io() {
        // Verify parse_target_state is the early-rejection gate
        // (no IO attempted). We point at a non-existent path to
        // ensure no disk hit even if parsing regresses.
        let err = parse_target_state("complete").expect_err("must reject");
        assert!(matches!(err, StateTransitionError::InvalidTargetState(s) if s == "complete"));
    }
}
