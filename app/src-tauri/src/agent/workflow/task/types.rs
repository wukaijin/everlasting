//! Workflow task types: `TaskStatus`, `TaskItem`, `TaskJson`, and `TaskError`.
//!
//! Split out of `agent/workflow/task.rs` (2026-08-08 batch3). Pure data +
//! serde impls, no IO.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Workflow task's authoritative status. Mirrors
/// `WorkflowDef::states` (planning / in_progress / done) —
/// kept as an explicit enum so the JSON validator can reject
/// typos upfront.
///
/// **Merge (2026-07-10)**: the former `Implement` + `Check`
/// variants collapsed into a single `InProgress`. The dev
/// workflow became 3 states (`planning → in_progress → done`);
/// within `in_progress` the main LLM orchestrates
/// implementer + checker roles (adversarial review per item).
/// Old `task.json` files with `"status": "implement"` or
/// `"status": "check"` are silently migrated to `InProgress`
/// by [`TaskStatus::from_str_opt`] (lenient parse).
///
/// **Step 3.3 (2026-07-09)**: added the `Completed` terminal
/// variant, set by [`super::archive_task_init`] after moving the
/// task to `.everlasting/tasks/archive/<YYYY-MM>/<slug>/`.
/// `Completed` is distinct from `Done`:
/// - `Done`    = state machine's terminal state (checklist
///               closed, spec distilled by `wf-update-spec`).
/// - `Completed` = **archived** — moved to the archive
///               tree, no longer resolvable by
///               `inject::resolve_current_task`. `Completed`
///               is a one-way terminal: there is no
///               `Completed → X` transition; un-archive is
///               a manual git operation.
///
/// The Step 0.5 chat_loop per-turn injection deserializes
/// the field via [`TaskStatus::from_str_opt`].
///
/// **C0 (2026-07-26, `07-26-taskstatus-custom-state`)**:
/// `from_str_opt` NO LONGER falls back to `Planning` on
/// unknown values — instead it captures them in the new
/// [`TaskStatus::Custom`] variant so plugin-defined states
/// (review's `intake`/`reviewing`/`revising`/`reported`,
/// etc.) round-trip through `task.json.status` instead of
/// silently collapsing. `Completed` is in the
/// `from_str_opt` accept-list so an archive file re-read by
/// the chat loop still resolves to `Completed`.
/// `Custom(String)` carries a `String` so the enum lost
/// `Copy` — callers pass `&TaskStatus` (preferred) or
/// `.clone()` (cheap: at most one short `String`).
#[derive(Debug, Clone, PartialEq, Eq)] // no Copy (Custom holds String); no derive Serialize (manual impl below)
pub enum TaskStatus {
    Planning,
    InProgress,
    Done,
    Completed,
    /// Plugin-defined workflow state (e.g. review's
    /// `intake` / `reviewing` / `revising` / `reported`).
    /// Populated by [`TaskStatus::from_str_opt`] for any
    /// value not in the dev accept-list, instead of
    /// demoting to `Planning`. [`TaskStatus::as_str`]
    /// returns the captured string verbatim, and the
    /// manual [`serde::Serialize`] impl writes it as a bare
    /// JSON string (NOT `{"Custom": ...}`) so the on-disk
    /// shape matches the plugin's `workflow.json` state
    /// names and `roles_by_state` lookups succeed.
    Custom(String),
}

impl TaskStatus {
    pub fn from_str_opt(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "planning" => Self::Planning,
            "in_progress" => Self::InProgress,
            // Legacy values from the pre-merge 4-state
            // workflow — migrated to InProgress (see the enum
            // doc comment on the 2026-07-10 merge).
            "implement" | "check" => Self::InProgress,
            "done" => Self::Done,
            "completed" => Self::Completed,
            // C0: unknown value → Custom (lowercased to match
            // the dev accept-list's case handling). Plugin
            // workflow.json states are lowercase by spec, so
            // the round-trip is identity for well-formed
            // plugins; a stray uppercase is normalized here.
            other => Self::Custom(other.to_string()),
        }
    }

    /// Canonical snake_case string for the dev variants;
    /// the captured string for [`TaskStatus::Custom`].
    ///
    /// **C0**: the return lifetime is now `&str` (borrowed
    /// from `self`, not `&'static str`) because `Custom`
    /// borrows its inner `String`. All existing call sites
    /// use the result transiently (format!, map lookups,
    /// logging) — no `'static` dependency to migrate.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Planning => "planning",
            Self::InProgress => "in_progress",
            Self::Done => "done",
            Self::Completed => "completed",
            Self::Custom(s) => s,
        }
    }
}

/// Manual `Serialize` so the wire / on-disk shape is the
/// bare snake_case string `"reviewing"` (NOT the
/// derived-enum shape `{"Custom": "reviewing"}`). Mirrors
/// the existing manual [`Deserialize`] impl and keeps
/// [`TaskStatus::as_str`] as the single source of truth for
/// the canonical spelling. C0 (`07-26-taskstatus-custom-state`):
/// the derive `Serialize` + `rename_all` could not express
/// `Custom(String)` as a bare string, so we replaced it
/// with this impl. The dev variants serialize identically
/// to before (`"planning"` / `"in_progress"` / `"done"` /
/// `"completed"`).
impl serde::Serialize for TaskStatus {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

/// Lenient `Deserialize` — unknown / typo'd status strings fall back
/// to `Planning` via [`TaskStatus::from_str_opt`], so a hand-written
/// `task.json` with e.g. `"status": "pending"` or `"blocked"`
/// (checklist-style values the LLM naturally emits) does NOT break
/// `read_task`. 07-10-workflow-task-json-hardening R1: the derive
/// `Deserialize` was strict and rejected any variant outside the
/// enum, which made every direct `write_file` of task.json
/// fatal. Resilience lives on the read side, not by gating writes.
/// Mirrors `def.rs::Coordination`'s custom Deserialize posture.
///
/// Note: the legacy `"implement"` / `"check"` values are accepted and
/// migrated to `InProgress` (2026-07-10 merge), NOT demoted — see
/// [`TaskStatus::from_str_opt`].
impl<'de> serde::Deserialize<'de> for TaskStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(Self::from_str_opt(&s))
    }
}

/// One to-do entry. The struct's shape mirrors what B12's
/// `ChecklistItem` will become in Phase 2 Step 2.6; for
/// Phase 0 the JSON is just persisted and read, no
/// `update_checklist` writes to it yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskItem {
    /// Stable id (e.g. `"backend-impl"`). Phase 2 Step 2.6
    /// uses this as the B12 checklist key for
    /// cross-session persistence.
    pub id: String,
    #[serde(default)]
    pub content: String,
    pub status: TaskStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tdd: Option<bool>,
}

/// On-disk `.everlasting/tasks/<slug>/task.json` schema.
/// See module-level doc comment for the field-level rules.
///
/// **Step 3.3 (2026-07-09)**: added `completed_at: Option<String>`
/// — set by [`super::archive_task_init`] (RFC3339 timestamp of the
/// archive move). The field is optional and `None` for
/// pre-archive tasks; serde defaults + skip-serializing keep
/// the v1 schema forward-compatible.
///
/// **C5 (2026-07-28)**: added `workflow_plugin: String` — records
/// which plugin's state machine this task's `status` belongs to.
/// Without it, switching the session plugin made role gate /
/// transition look up the *new* plugin's state table with the *old*
/// plugin's status string, dead-locking cross-plugin flows
/// (dev→review→dev in one session). `#[serde(default = "dev_plugin")]`
/// back-fills `"dev"` for pre-C5 task.json files (they were all
/// created by the dev plugin, so `"dev"` is the correct retroactive
/// attribution).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskJson {
    pub id: String,
    pub title: String,
    pub slug: String,
    pub status: TaskStatus,
    /// C5: the plugin whose state machine `status` belongs to. Role
    /// gate / transition / breadcrumb resolve the workflow_def via
    /// this field, NOT the session's plugin_name — so switching the
    /// session plugin (to surface different skills/tools) does not
    /// corrupt the task's state-machine invariants. `set_session_
    /// plugin_name` re-points this field + remaps `status` to the
    /// new plugin's `initial` when the user switches mid-task.
    #[serde(
        default = "default_workflow_plugin",
        skip_serializing_if = "is_default_plugin"
    )]
    pub workflow_plugin: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    /// `None` for top-level tasks; non-empty for child
    /// tasks (B8 DAG, Phase 3+). The slug of the parent
    /// task is sufficient — not a UUID — because parent
    /// references are most naturally project-relative.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// One-line task summary. Empty on creation; the
    /// Phase 1 `wf-brainstorm` skill populates it.
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub items: Vec<TaskItem>,
    /// RFC3339 timestamp set when the task was archived
    /// via [`super::archive_task_init`]. `None` for in-flight
    /// tasks; on archive the status flips to
    /// [`TaskStatus::Completed`] AND this field is filled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

/// C5: serde default for `TaskJson::workflow_plugin`. Pre-C5
/// task.json files lack the field; they were all created by the dev
/// plugin, so `"dev"` is the correct retroactive attribution.
pub(crate) fn default_workflow_plugin() -> String {
    "dev".to_string()
}

/// C5: skip-serializing helper — omit `workflow_plugin` when it's
/// `"dev"` so task.json written by C5 stays byte-identical to the
/// pre-C5 schema for the common (dev-only) case. Review tasks always
/// serialize the field (non-default value). Keeps git diffs on
/// existing dev task.json noise-free after the upgrade.
pub(crate) fn is_default_plugin(s: &String) -> bool {
    s == "dev"
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Local error type; the IPC layer in `commands::task`
/// maps each variant to the right `AppCommandError`
/// category.
///
/// **Step 3.3 (2026-07-09)**: added `AlreadyArchived` /
/// `NotInDoneStatus` to support [`super::archive_task_init`]
/// without widening the pre-existing variants. Each maps
/// to `InvalidRequest` at the IPC boundary — archive is
/// user-correctable.
#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    #[error("invalid slug: {0} (expected [a-z0-9-]{{1,64}})")]
    InvalidSlug(String),

    #[error("task already exists at {0}")]
    AlreadyExists(PathBuf),

    #[error("task directory not found: {0}")]
    NotFound(PathBuf),

    /// Step 3.3: archive target already occupied. Refuse
    /// to overwrite an existing archive entry so the
    /// command is idempotent on retry (a partially-written
    /// archive followed by a retry won't clobber the prior
    /// state). The caller can choose to delete the
    /// conflicting archive dir manually if they really want
    /// to re-archive.
    #[error("task already archived at {0}")]
    AlreadyArchived(PathBuf),

    /// Step 3.3: archive requires `status == Done`. We
    /// refuse to archive a planning / in_progress
    /// task because (a) the workflow hasn't finished
    /// producing spec content yet, and (b) archiving
    /// would orphan in-flight items + progress.md.
    #[error("cannot archive task in status `{0}` (must be `done`)")]
    NotInDoneStatus(String),

    #[error("task.json malformed at {0}: {1}")]
    MalformedJson(PathBuf, String),

    #[error("io error at {0}: {1}")]
    Io(PathBuf, #[source] std::io::Error),
}

pub type TaskResult<T> = Result<T, TaskError>;
