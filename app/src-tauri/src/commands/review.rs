//! C2 (review visualization view, 2026-07-26): Tauri commands for
//! the review-state.json read API used by the frontend
//! `<ReviewMatrix>` panel.
//!
//! Two read-only commands live here. The backend emits NO event —
//! refresh is 100% frontend driven via `streamController`'s
//! `tool:call` route (write_file hitting `review-state.json` →
//! `useReviewStateStore.handleReviewStateWritten`). See
//! `.trellis/tasks/07-26-review-viz/design.md` §2/§6.
//!
//! - [`get_review_state`] — read
//!   `<project>/.everlasting/tasks/<task_slug>/review-state.json`
//!   and return a three-state payload:
//!   [`ReviewStatePayload::State`] (parse ok) /
//!   [`ReviewStatePayload::Missing`] (file absent — frontend hides
//!   the panel) / [`ReviewStatePayload::Invalid`] (bad JSON —
//!   frontend shows the error state with a SubagentDrawer link).
//! - [`get_current_task_slug`] — reuse the engine's
//!   `resolve_current_task` to return `{slug, id, title, status}`
//!   for the active project, or `None` when no in-flight task
//!   exists. The frontend has no task-slug state of its own.
//!
//! Both commands follow the Q0 single-source-of-truth dual-path
//! pattern: a `#[tauri::command]` wrapper + a shared `_inner`
//! function that the axum handler in
//! `daemon::routes::review` forwards to. Mirrors
//! `list_workflow_plugins` (commands/sessions.rs +
//! daemon/routes/sessions.rs) which also takes `project_path` as
//! a frontend-supplied string param (the frontend reads
//! `currentSession.current_cwd`, the same source PluginSelect
//! uses for plugin discovery).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::agent::workflow::inject::resolve_current_task;
use crate::error::AppCommandError;

// ---------------------------------------------------------------------------
// ReviewState — Rust mirror of the C3 R7 review-state.json schema
// (frozen in `.trellis/tasks/archive/2026-07/07-26-review-plugin-pack/prd.md`).
//
// `#[serde(default)]` is used liberally so a missing optional field
// doesn't fail parse — the frontend handles optionals. Required
// fields (round number, model verdict, finding id/severity/issue)
// stay non-optional: if the writer (C3 wf-synthesize) omits one,
// the file is genuinely broken and the frontend should see
// `Invalid`, not silently render partial data.
// ---------------------------------------------------------------------------

/// Per-finding triage decision recorded by the synthesizing LLM.
/// Mirrors the TS `TriageDecision`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TriageDecision {
    Adopt,
    Reject,
    Defer,
}

/// Per-finding triage block: `{ decision, reason }`. Optional —
/// the writer may leave a finding un-triaged in early rounds.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Triage {
    pub decision: TriageDecision,
    pub reason: String,
}

/// Finding severity. Mirrors the TS `Severity`. Matches the
/// C3 R7 schema exactly (critical/high/medium/low/info).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
    Info,
}

/// Single finding. `finding_id` / `dimension` / `severity` /
/// `issue` / `source_run_id` are required (per C3 R7); the rest
/// are optional.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReviewFinding {
    pub finding_id: String,
    pub dimension: String,
    pub severity: Severity,
    pub issue: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    pub source_run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triage: Option<Triage>,
}

/// Run status of a single model's review pass. Mirrors the DB
/// `subagent_runs.status` CHECK constraint
/// (running/completed/cancelled/error/incomplete). NOT the old
/// failed/timed_out draft.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Running,
    Completed,
    Cancelled,
    Error,
    Incomplete,
}

/// Overall verdict from a model on this round. Mirrors the TS
/// `Verdict` (pass/pass_with_minor/revise/reject).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    PassWithMinor,
    Revise,
    Reject,
}

/// One model's verdict + findings for a round. The `models` map
/// key on the parent [`ReviewRound`] is the stable `model_id`
/// (NOT display name — C3 R7 decision).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelVerdict {
    pub model_display: String,
    pub run_id: String,
    pub status: RunStatus,
    pub verdict: Verdict,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default)]
    pub findings: Vec<ReviewFinding>,
}

/// One round of review. `models` is a map keyed by `model_id`.
/// `dimensions` / `models_present` / `change_log` default to
/// empty so an older schema_version still parses.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReviewRound {
    pub round: u32,
    #[serde(default)]
    pub dimensions: Vec<String>,
    #[serde(default)]
    pub models_present: Vec<String>,
    pub models: std::collections::BTreeMap<String, ModelVerdict>,
    #[serde(default)]
    pub change_log: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub convergence_note: Option<String>,
}

/// Top-level review-state.json document. `schema_version` +
/// `task_id` + `current_round` + `rounds[]` are required.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReviewState {
    pub schema_version: String,
    pub task_id: String,
    pub current_round: u32,
    pub rounds: Vec<ReviewRound>,
}

// ---------------------------------------------------------------------------
// ReviewStatePayload — three-state result for get_review_state
// ---------------------------------------------------------------------------

/// Tagged enum so the frontend can switch on `kind`:
/// `"state"` → render the matrix; `"missing"` → hide the panel;
/// `"invalid"` → show the error state with a SubagentDrawer link.
///
/// Serialized via `#[serde(tag = "kind", rename_all = "lowercase")]`
/// so the wire shape is `{ kind: "state", state: {...} }` /
/// `{ kind: "missing" }` / `{ kind: "invalid", detail: "..." }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ReviewStatePayload {
    State {
        state: ReviewState,
    },
    Missing,
    Invalid {
        detail: String,
    },
}

// ---------------------------------------------------------------------------
// CurrentTaskInfo — get_current_task_slug return shape
// ---------------------------------------------------------------------------

/// Minimal info for the frontend to (a) pass `slug` back into
/// `get_review_state` and (b) render a task label. Mirrors the
/// shape `resolve_current_task` returns (`TaskJson`) but trimmed
/// to the fields the frontend actually consumes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurrentTaskInfo {
    pub slug: String,
    pub id: String,
    pub title: String,
    pub status: String,
}

// ---------------------------------------------------------------------------
// get_review_state
// ---------------------------------------------------------------------------

/// Path of the review-state.json file for a task. Centralized so
/// the test + production paths agree.
///
/// `<project>/.everlasting/tasks/<task_slug>/review-state.json`
/// (C3 R7 layout — the wf-synthesize skill writes here).
fn review_state_path(project_path: &std::path::Path, task_slug: &str) -> PathBuf {
    project_path
        .join(".everlasting")
        .join("tasks")
        .join(task_slug)
        .join("review-state.json")
}

/// Phase 2.2 `_inner` (Q0): shared business logic, callable from
/// the Tauri command wrapper above + the axum route handler in
/// `daemon::routes::review`.
///
/// Three-state contract:
/// - file missing → [`ReviewStatePayload::Missing`] (NOT an
///   error — the frontend hides the panel silently; common during
///   the first reviewing round before any revising has happened).
/// - file present but unparseable →
///   [`ReviewStatePayload::Invalid`] with a `detail` string for
///   the frontend's error toast. We log at `warn!` for ops.
/// - file present + parses → [`ReviewStatePayload::State`].
///
/// `project_path` is the frontend-supplied project root (the
/// frontend reads `currentSession.current_cwd`). Empty path →
/// `Missing` (defensive; should not happen in practice).
pub fn get_review_state_inner(
    project_path: &str,
    task_slug: &str,
) -> ReviewStatePayload {
    if project_path.trim().is_empty() || task_slug.trim().is_empty() {
        return ReviewStatePayload::Missing;
    }
    let path = review_state_path(&PathBuf::from(project_path), task_slug);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return ReviewStatePayload::Missing;
        }
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "get_review_state: failed to read review-state.json; treating as Missing",
            );
            // A permission error / IO error is not "invalid JSON"
            // — surfacing it as Missing keeps the panel hidden
            // rather than scaring the user with a parse error.
            return ReviewStatePayload::Missing;
        }
    };
    match serde_json::from_slice::<ReviewState>(&bytes) {
        Ok(state) => ReviewStatePayload::State { state },
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "get_review_state: review-state.json failed to parse; surfacing as Invalid",
            );
            ReviewStatePayload::Invalid {
                detail: format!("parse error: {}", e),
            }
        }
    }
}

#[tauri::command]
pub async fn get_review_state(
    project_path: String,
    task_slug: String,
) -> Result<ReviewStatePayload, AppCommandError> {
    Ok(get_review_state_inner(&project_path, &task_slug))
}

// ---------------------------------------------------------------------------
// get_current_task_slug
// ---------------------------------------------------------------------------

/// Phase 2.2 `_inner` (Q0): reuse the engine's
/// `resolve_current_task` to find the active task for a project.
/// Returns `Some({slug, id, title, status})` for the first
/// non-terminal task (alphabetical by slug), or `None` if there
/// isn't one.
///
/// `resolve_current_task` is a blocking FS scan; we run it on the
/// blocking pool to keep the async runtime responsive (matches
/// the engine's own usage).
pub async fn get_current_task_slug_inner(
    project_path: String,
) -> Result<Option<CurrentTaskInfo>, AppCommandError> {
    if project_path.trim().is_empty() {
        return Ok(None);
    }
    let path = PathBuf::from(&project_path);
    // `resolve_current_task` is itself `async fn` (it does blocking
    // FS work internally; the engine awaits it directly in
    // `agent/chat_loop.rs:1577`). We mirror that — no extra
    // `spawn_blocking` layer (the function's own `.await` is the
    // yield point).
    let task = resolve_current_task(&path).await;
    Ok(task.map(|t| CurrentTaskInfo {
        slug: t.slug,
        id: t.id,
        title: t.title,
        // `as_str` is the canonical snake_case spelling
        // ("in_progress" / "reviewing" / ...). NOT Debug format
        // (which would give "InProgress").
        status: t.status.as_str().to_string(),
    }))
}

#[tauri::command]
pub async fn get_current_task_slug(
    project_path: String,
) -> Result<Option<CurrentTaskInfo>, AppCommandError> {
    // Mirrors `list_workflow_plugins` (no AppState dependency —
    // the lookup is a pure filesystem scan via
    // `resolve_current_task`). The frontend supplies
    // `project_path` from `currentSession.current_cwd`, the same
    // source PluginSelect uses.
    get_current_task_slug_inner(project_path).await
}

// ---------------------------------------------------------------------------
// Tests — IPC layer (file system three-state + resolve_current_task)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_state(dir: &std::path::Path, json: &str) -> std::path::PathBuf {
        fs::create_dir_all(dir).unwrap();
        let p = dir.join("review-state.json");
        fs::write(&p, json).unwrap();
        p
    }

    fn sample_state_json() -> String {
        // Mirrors the C3 R7 schema example.
        r#"{
            "schema_version": "1.0",
            "task_id": "demo-task",
            "current_round": 1,
            "rounds": [
              {
                "round": 1,
                "dimensions": ["清晰度"],
                "models_present": ["model-a"],
                "models": {
                  "model-a": {
                    "model_display": "Model A",
                    "run_id": "run-1",
                    "status": "completed",
                    "verdict": "revise",
                    "findings": [
                      {
                        "finding_id": "f1",
                        "dimension": "清晰度",
                        "severity": "high",
                        "issue": "unclear",
                        "suggestion": "add example",
                        "location": "prd.md§2",
                        "source_run_id": "run-1",
                        "triage": { "decision": "adopt", "reason": "needs fix" }
                      }
                    ]
                  }
                },
                "change_log": [],
                "convergence_note": null
              }
            ]
          }"#
        .to_string()
    }

    #[test]
    fn get_review_state_state_path() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        let task_dir = project
            .join(".everlasting")
            .join("tasks")
            .join("demo-task");
        write_state(&task_dir, &sample_state_json());

        let payload = get_review_state_inner(
            &project.to_string_lossy(),
            "demo-task",
        );
        match payload {
            ReviewStatePayload::State { state } => {
                assert_eq!(state.schema_version, "1.0");
                assert_eq!(state.current_round, 1);
                assert_eq!(state.rounds.len(), 1);
                let round = &state.rounds[0];
                assert_eq!(round.round, 1);
                assert_eq!(round.models.len(), 1);
                let mv = round.models.get("model-a").expect("model-a present");
                assert!(matches!(mv.status, RunStatus::Completed));
                assert!(matches!(mv.verdict, Verdict::Revise));
                assert_eq!(mv.findings.len(), 1);
                let f = &mv.findings[0];
                assert!(matches!(f.severity, Severity::High));
                let triage = f.triage.as_ref().expect("triage present");
                assert!(matches!(triage.decision, TriageDecision::Adopt));
            }
            other => panic!("expected State, got {:?}", other),
        }
    }

    #[test]
    fn get_review_state_missing_when_file_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        // No review-state.json written.
        let payload =
            get_review_state_inner(&project.to_string_lossy(), "no-such-task");
        assert!(matches!(payload, ReviewStatePayload::Missing));
    }

    #[test]
    fn get_review_state_missing_when_empty_args() {
        let payload = get_review_state_inner("", "demo-task");
        assert!(matches!(payload, ReviewStatePayload::Missing));
        let payload = get_review_state_inner("/tmp/x", "");
        assert!(matches!(payload, ReviewStatePayload::Missing));
    }

    #[test]
    fn get_review_state_invalid_when_bad_json() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        let task_dir = project
            .join(".everlasting")
            .join("tasks")
            .join("demo-task");
        write_state(&task_dir, "{not json");

        let payload = get_review_state_inner(&project.to_string_lossy(), "demo-task");
        match payload {
            ReviewStatePayload::Invalid { detail } => {
                assert!(detail.contains("parse error"));
            }
            other => panic!("expected Invalid, got {:?}", other),
        }
    }

    #[test]
    fn get_review_state_invalid_when_missing_required_field() {
        // Round without `round` field — required, so parse fails → Invalid.
        let bad = r#"{
            "schema_version": "1.0",
            "task_id": "demo-task",
            "current_round": 1,
            "rounds": [{ "models": {} }]
        }"#;
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        let task_dir = project
            .join(".everlasting")
            .join("tasks")
            .join("demo-task");
        write_state(&task_dir, bad);

        let payload = get_review_state_inner(&project.to_string_lossy(), "demo-task");
        assert!(matches!(payload, ReviewStatePayload::Invalid { .. }));
    }

    #[test]
    fn get_review_state_tolerates_missing_optional_fields() {
        // Missing `dimensions`, `models_present`, `change_log`,
        // `convergence_note`, `summary`, `suggestion`, `location`,
        // `triage` — all optional via #[serde(default)].
        let minimal = r#"{
            "schema_version": "1.0",
            "task_id": "demo-task",
            "current_round": 2,
            "rounds": [
              {
                "round": 2,
                "models": {
                  "model-a": {
                    "model_display": "Model A",
                    "run_id": "run-1",
                    "status": "running",
                    "verdict": "pass_with_minor",
                    "findings": [
                      { "finding_id": "f1", "dimension": "x", "severity": "info",
                        "issue": "ok", "source_run_id": "run-1" }
                    ]
                  }
                }
              }
            ]
        }"#;
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        let task_dir = project
            .join(".everlasting")
            .join("tasks")
            .join("demo-task");
        write_state(&task_dir, minimal);

        let payload = get_review_state_inner(&project.to_string_lossy(), "demo-task");
        match payload {
            ReviewStatePayload::State { state } => {
                let mv = &state.rounds[0].models["model-a"];
                assert!(matches!(mv.status, RunStatus::Running));
                assert!(matches!(mv.verdict, Verdict::PassWithMinor));
                assert!(mv.findings[0].triage.is_none());
                assert!(state.rounds[0].dimensions.is_empty());
            }
            other => panic!("expected State, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn get_current_task_slug_none_when_no_tasks() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        let info = get_current_task_slug_inner(project.to_string_lossy().to_string())
            .await
            .unwrap();
        assert!(info.is_none());
    }

    #[tokio::test]
    async fn get_current_task_slug_none_when_empty_path() {
        let info = get_current_task_slug_inner(String::new())
            .await
            .unwrap();
        assert!(info.is_none());
    }

    #[tokio::test]
    async fn get_current_task_slug_some_when_active_task_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        let task_dir = project
            .join(".everlasting")
            .join("tasks")
            .join("demo-task");
        fs::create_dir_all(&task_dir).unwrap();
        let task_json = r#"{
            "id": "task-1",
            "title": "Demo",
            "slug": "demo-task",
            "status": "in_progress",
            "created_at": "",
            "updated_at": "",
            "summary": "",
            "items": []
        }"#;
        fs::write(task_dir.join("task.json"), task_json).unwrap();

        let info = get_current_task_slug_inner(project.to_string_lossy().to_string())
            .await
            .unwrap()
            .expect("task should resolve");
        assert_eq!(info.slug, "demo-task");
        assert_eq!(info.id, "task-1");
        assert_eq!(info.title, "Demo");
        assert_eq!(info.status, "in_progress");
    }
}
