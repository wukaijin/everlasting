// C2 (review visualization view, 2026-07-26): TypeScript types
// for review-state.json — the data source for the frontend
// `<ReviewMatrix>` panel.
//
// These types MUST mirror the C3 R7 schema exactly
// (`.trellis/tasks/archive/2026-07/07-26-review-plugin-pack/prd.md`
// §R7) — cross-task contract. C3's `wf-synthesize` skill is the
// writer; C2's `get_review_state` IPC + these types are the
// reader. Any schema drift between the two breaks the panel.
//
// The Rust mirror lives in
// `app/src-tauri/src/commands/review.rs` (same field names +
// enum spellings). All three layers (C3 JSON, Rust struct, TS
// interface) must agree.

/**
 * Overall verdict from one model on one round.
 * C3 R7: `pass` / `pass_with_minor` / `revise` / `reject`.
 */
export type Verdict = "pass" | "pass_with_minor" | "revise" | "reject";

/**
 * Finding severity. C3 R7: `critical` / `high` / `medium` /
 * `low` / `info`. Matches the Rust `Severity` enum's
 * `#[serde(rename_all = "lowercase")]`.
 */
export type Severity = "critical" | "high" | "medium" | "low" | "info";

/**
 * Run status of a single model's review pass. C3 R7 aligned to
 * the DB `subagent_runs.status` CHECK constraint:
 * `running` / `completed` / `cancelled` / `error` /
 * `incomplete`. NOT the old `failed` / `timed_out` draft.
 */
export type RunStatus =
  | "running"
  | "completed"
  | "cancelled"
  | "error"
  | "incomplete";

/**
 * Per-finding triage decision recorded by the synthesizing
 * LLM. C3 R7: `adopt` / `reject` / `defer`.
 */
export type TriageDecision = "adopt" | "reject" | "defer";

/** Per-finding triage block. */
export interface Triage {
  decision: TriageDecision;
  reason: string;
}

/** Single finding. `finding_id` / `dimension` / `severity` /
 *  `issue` / `source_run_id` are required (C3 R7); the rest
 *  are optional. */
export interface ReviewFinding {
  finding_id: string;
  dimension: string;
  severity: Severity;
  issue: string;
  suggestion?: string;
  location?: string;
  source_run_id: string;
  triage?: Triage;
}

/** One model's verdict + findings for a round. The parent
 *  `ReviewRound.models` map is keyed by the stable `model_id`
 *  (NOT display name — C3 R7 decision, so re-labels don't
 *  break the matrix). */
export interface ModelVerdict {
  model_display: string;
  run_id: string;
  status: RunStatus;
  verdict: Verdict;
  summary?: string;
  findings: ReviewFinding[];
}

/** One round of review. `models` is a `Record<model_id,
 *  ModelVerdict>`. Optional fields fall back to empty arrays /
 *  undefined so older `schema_version`s still render. */
export interface ReviewRound {
  round: number;
  dimensions: string[];
  models_present: string[];
  /** key = stable `model_id` (NOT `model_display`). */
  models: Record<string, ModelVerdict>;
  change_log?: string[];
  convergence_note?: string;
}

/** Top-level review-state.json document. */
export interface ReviewState {
  schema_version: string;
  task_id: string;
  current_round: number;
  rounds: ReviewRound[];
}

/**
 * Three-state payload from `get_review_state` IPC. Mirrors the
 * Rust `ReviewStatePayload` enum's
 * `#[serde(tag = "kind", rename_all = "lowercase")]` wire shape:
 * - `{ kind: "state", state: {...} }` — render the matrix.
 * - `{ kind: "missing" }` — hide the panel (silent).
 * - `{ kind: "invalid", detail: "..." }` — show the error state.
 */
export type ReviewStatePayload =
  | { kind: "state"; state: ReviewState }
  | { kind: "missing" }
  | { kind: "invalid"; detail: string };

/**
 * `get_current_task_slug` IPC return shape. `null` when no
 * active (non-terminal) task exists for the project.
 */
export interface CurrentTaskInfo {
  slug: string;
  id: string;
  title: string;
  status: string;
}
