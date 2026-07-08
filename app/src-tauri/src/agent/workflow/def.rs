//! W1 (Workflow integration, Phase 0 Step 0.3 — 2026-07-08):
//! workflow engine's **content** layer — struct definitions,
//! the `default_workflow()` dev-plugin constant, and the **4
//! accessor functions** that are the only way callers
//! (engine, agent loop, sub-agent dispatcher) consult a
//! `WorkflowDef`.
//!
//! ## Why an explicit accessor surface (Q2 of the 2026-07-08
//! workflow-integration design doc)
//!
//! Engine code is forbidden from inlining `match state` /
//! `match role` lookups against a `WorkflowDef` — every state
//! / role question funnels through one of the four accessors
//! below. The rule keeps the contract surface area small
//! (4 functions × 4 plugins → 16 N×M permutations to review)
//! and makes Phase 2's `load_workflow()` extension (swap the
//! constant for a serde-deserialized `workflow.json`)
//! invisible to callers — only the four bodies change.
//!
//! ## Module split (06-23-style "def + accessors in one
//! place, side files in submodules")
//!
//! This file owns TYPES + ACCESSORS + `default_workflow()` +
//! the Phase 2 Step 2.1 `load_workflow()` extension.
//! Sister files (when Phase 2 lands) live alongside:
//! - `task.rs` — task.json read/write (`Step 0.4`)
//! - `state.rs` — `set_task_state` + Rust hook (`Step 3.1`)
//!
//! ## Phase 0 → Phase 2 scope change
//!
//! Phase 2 Step 2.1 (`07-08-workflow-integration`):
//! - `WorkflowDef` + `Transition` now `#[derive(Deserialize)]`.
//! - `Coordination` gets a custom `Deserialize` impl that
//!   delegates to the lenient `from_str_opt` parser
//!   (`"pipeline"` / `"synthesis_round"` /
//!   `"synthesis-round"`, case-insensitive, unknown →
//!   `Pipeline`).
//! - `load_workflow(name, project_path)` reads
//!   `.everlasting/workflow/<name>/workflow.json` and
//!   returns `default_workflow()` on any failure
//!   (file missing / malformed JSON / validation error).
//!   `default_workflow()` is now the **fallback** shape;
//!   the on-disk JSON is the source of truth for a real
//!   plugin.
//!
//! ## Validation contract (Phase 2 Step 2.1, lands here)
//!
//! `validate()` enforces (M6 of design §5.4):
//! - `states` non-empty
//! - `initial ∈ states`
//! - every `Transition.from` / `Transition.to` ∈ `states`
//! - every key of `roles_by_state` ⊆ `states`
//! - every key of `breadcrumb` ⊆ `states`
//!
//! Failure → warn + return `default_workflow()` (the caller
//! gets a working engine; the on-disk plugin is rejected
//! wholesale rather than partially applied — partial
//! application is the worst failure mode because the
//! engine's invariants are violated mid-session). Missing
//! keys in `delegation_templates` / `breadcrumb` are NOT
//! blocking — they map to empty / `None` accessor returns
//! (already documented above).
//!
//! `gather_strategy` MAY be empty when
//! `coordination = Pipeline`.

// Phase 0 Step 0.3 ships the engine surface (`WorkflowDef` +
// accessors + `default_workflow`) BEFORE the first consumer
// lands in Step 0.5 (chat_loop's breadcrumb injection). The
// window between is short but real, and rustc's `dead_code`
// lint is unforgiving — silent warnings make it harder to
// spot genuinely-unused items in future PRs.
//
// The allow is scoped to this file only (rationale in the
// module docs); it's REMOVED in Step 0.5's commit when
// `breadcrumb_for` + `default_workflow` get their first
// engine consumer. The other accessors (`allowed_roles` /
// `can_transition` / `delegation_template_for`) light up in
// Phase 2 — they're flagged here so Step 2.x can audit the
// call sites one-by-one as each adds a consumer.
//
// The tests below use every public item, so they're
// unaffected.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// One edge in the workflow's state graph. The agent cannot
/// self-transition — the user confirms every gate
/// (`requires_user_confirm`; future Phase 3 step adds the
/// dedicated `resolve_task_state_transition` IPC for this).
///
/// `requires_user_confirm = false` is reserved for future
/// auto-progress edges (e.g. an `error` state machine); the
/// dev plugin currently uses `true` for every transition.
///
/// Phase 2 Step 2.1: `Deserialize` lands here so
/// `load_workflow` can read `workflow.json` arrays of
/// transitions. Serde field names match the JSON keys
/// verbatim (`from` / `to` / `requires_user_confirm`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Transition {
    pub from: String,
    pub to: String,
    pub requires_user_confirm: bool,
}

/// Coordination strategy for sub-agent dispatch across
/// roles in the same state. `Pipeline` = dispatched roles
/// run sequentially in their declared order (dev plugin
/// uses this — single-role states like `planning` → just
/// researcher); `SynthesisRound` = dispatched in parallel
/// then reduced through `gather_strategy` (the future
/// review plugin's likely shape).
///
/// JSON deserialization (Phase 2 Step 2.1) accepts
/// `"pipeline"` / `"synthesis_round"` case-insensitively
/// via the custom `Deserialize` impl below (delegating to
/// `from_str_opt` — same lenient behavior the manual
/// `from_str_opt` exposes for the in-memory constant).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Coordination {
    #[default]
    Pipeline,
    SynthesisRound,
}

impl Coordination {
    /// Lenient parser used by both Phase 2 serde (after
    /// step 2.1 lands) and the accessor's debug helpers.
    /// Unknown strings map to `Pipeline` (matches the dev
    /// plugin's natural default).
    pub fn from_str_opt(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "synthesis_round" | "synthesis-round" | "synthesisround" => {
                Self::SynthesisRound
            }
            _ => Self::Pipeline,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pipeline => "pipeline",
            Self::SynthesisRound => "synthesis_round",
        }
    }
}

/// Custom `Deserialize` for `Coordination` so the JSON form
/// matches the lenient `from_str_opt` parser. Without this,
/// serde's default `#[derive(Deserialize)]` would reject
/// `"synthesis-round"` / mixed case, forcing the on-disk
/// schema to a single canonical form — the design wants
/// `from_str_opt` to own that decision (one source of truth
/// for "what does the loader accept").
impl<'de> serde::Deserialize<'de> for Coordination {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(Self::from_str_opt(&s))
    }
}

/// The engine's view of a workflow plugin. Owned by the
/// session for the duration of the workflow session;
/// reloaded on every turn boundary so plugin hot-swaps
/// (Phase 2 Step 2.2 UI switcher) pick up without restart.
///
/// See `docs/WORKFLOW-INTEGRATION.md §5.4` for the full
/// contract; see `default_workflow()` for the in-memory
/// fallback; see `load_workflow()` (Phase 2 Step 2.1) for
/// the on-disk source of truth.
///
/// Phase 2 Step 2.1: `Deserialize` lands here. JSON field
/// names match the Rust field names 1:1 (no `#[serde(rename)]`).
/// Optional fields use `#[serde(default)]` so a partial
/// `workflow.json` (e.g. one missing `gather_strategy` when
/// `coordination = Pipeline`) still parses — the validator
/// below rejects the structurally-broken cases, but the
/// cosmetic / empty-default cases ride through.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct WorkflowDef {
    /// Stable plugin identifier (e.g. `"dev"`). Matches
    /// the trailing path segment of
    /// `<project>/.everlasting/workflow/<name>/`. ASCII
    /// snake_case; English (per W1 AC §非功能).
    pub name: String,

    /// Short human-readable summary. Surfaced in:
    /// - the UI plugin switcher (Phase 2 Step 2.2)
    /// - the breadcrumb's first line (Phase 0 Step 0.5
    ///   injection seam)
    pub description: String,

    /// Ordered state list. `initial` MUST be the first
    /// element. Transition targets MUST exist in this list.
    pub states: Vec<String>,

    /// Default state for a freshly-started task. Phase 0
    /// Step 0.5 validates `initial ∈ states` defensively
    /// before the first breadcrumb injection.
    pub initial: String,

    /// Directed edges. Duplicates `(from, to)` pairs are
    /// preserved verbatim; the dispatcher uses the first
    /// match in declaration order (matches the
    /// `can_transition` accessor below).
    pub transitions: Vec<Transition>,

    /// Roles permitted to be dispatched when the agent is
    /// in each state. The Phase 2 Step 2.4 gate
    /// (`run_subagent` drop-in check) reads this map and
    /// asks the user via `ask_user_question` on miss.
    #[serde(default)]
    pub roles_by_state: HashMap<String, Vec<String>>,

    /// State → breadcrumb text. Step 0.5's per-turn
    /// injection appends this to `messages[0]`. Missing
    /// keys → empty string (see [`breadcrumb_for`]).
    #[serde(default)]
    pub breadcrumb: HashMap<String, String>,

    /// `role` → worker-system-prompt template. Substitution
    /// placeholders: `{title}`, `{summary}`, `{state}`,
    /// `{relevant_specs}` (M-B). Phase 2 Step 2.5 fills
    /// these in and appends the result to `messages[0]`
    /// on dispatch turns. Missing keys → `None` (template
    /// is optional per role; see [`delegation_template_for`]).
    #[serde(default)]
    pub delegation_templates: HashMap<String, String>,

    /// Cross-role orchestration strategy. See [`Coordination`].
    /// Defaults to `Pipeline` (dev plugin's case) when the
    /// field is absent.
    #[serde(default)]
    pub coordination: Coordination,

    /// `state` → list of role-results to gather when
    /// `coordination = SynthesisRound`. Empty when
    /// `coordination = Pipeline` (dev plugin's case).
    /// `roles_by_state` still defines who MAY dispatch;
    /// `gather_strategy` defines WHO contributes to the
    /// reduce step.
    #[serde(default)]
    pub gather_strategy: HashMap<String, Vec<String>>,
}

// ---------------------------------------------------------------------------
// 4 accessor functions — the engine's ONLY entry points
// ---------------------------------------------------------------------------

/// Return the breadcrumb text for `state`. Falls back to
/// an empty `&str` (NOT `None`) when the key is missing —
/// matches the Phase 2 Step 2.1 validate plan (`breadcrumb`
/// MAY have missing keys, treated as "" with a `warn!`).
/// Returning `Option<&str>` would force every caller to
/// `unwrap_or("")` and bloat the per-turn injection site;
/// the infallible surface is the right ergonomic here.
///
/// The empty-string fallback also means Step 0.5 can
/// unconditionally call `breadcrumb_for` without
/// short-circuiting on `None` — the injection seam
/// becomes a single append.
///
/// **Lifetime**: the returned `&str` is borrowed from
/// `def`, NOT from `state` — `state` is just a key. We
/// therefore only tie the lifetime to `def` (the `'a`
/// applies to both).
pub fn breadcrumb_for<'a>(def: &'a WorkflowDef, state: &str) -> &'a str {
    def.breadcrumb.get(state).map(String::as_str).unwrap_or("")
}

/// Return the roles that may be dispatched while the
/// workflow session is in `state`. Empty slice for missing
/// keys (see [`breadcrumb_for`] rationale on infallible
/// defaults). The Phase 2 Step 2.4 gate consumes this
/// directly.
///
/// **Lifetime**: returned slice is borrowed from `def`,
/// not from `state` (just a key). Returning `&'static
/// [String]` would force an unnecessary heap copy on every
/// call; the `'a` (tied to `def`) keeps the borrow cheap.
pub fn allowed_roles<'a>(def: &'a WorkflowDef, state: &str) -> &'a [String] {
    def.roles_by_state
        .get(state)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// `true` if `from → to` is a declared transition in `def`
/// (regardless of `requires_user_confirm`). The user-
/// confirmation check happens one layer up (the
/// state-transition IPC chain lands in Phase 3 Step 3.1);
/// this accessor only answers "is this edge drawn at all".
///
/// Linear scan over `transitions` is fine: the dev plugin
/// declares 3 edges; even a 20-state plugin tops out at
/// O(transitions) which is tiny relative to the LLM
/// round-trip cost.
pub fn can_transition(def: &WorkflowDef, from: &str, to: &str) -> bool {
    def.transitions
        .iter()
        .any(|t| t.from == from && t.to == to)
}

/// Worker system-prompt template for `role`, or `None` if
/// the plugin didn't define one (the dev plugin defines
/// templates for `researcher` / `implementer` / `checker`
/// only — built-in sub-agents without plugin overrides
/// resolve to `None` and the dispatcher uses the
/// role-default prompt).
///
/// Distinct from the other three accessors' infallible
/// defaults because `None` carries semantic weight here:
/// "this plugin doesn't customize the worker for this role"
/// ≠ "the worker prompt was lost". The dispatcher uses
/// `None` to fall back to the sub-agent's own system
/// prompt.
///
/// **Lifetime**: returned `&str` is borrowed from `def`,
/// not from `role` (just a key).
pub fn delegation_template_for<'a>(def: &'a WorkflowDef, role: &str) -> Option<&'a str> {
    def.delegation_templates.get(role).map(String::as_str)
}

// ---------------------------------------------------------------------------
// dev plugin — the only workflow currently defined
// ---------------------------------------------------------------------------

/// The hard-coded `dev` workflow. Four states — `planning`
/// → `implement` → `check` → `done` — with role-rotated
/// dispatch (`researcher` / `implementer` / `checker`)
/// per the dev plugin's documented shape (see
/// `docs/WORKFLOW-INTEGRATION.md §A.1`).
///
/// Phase 2 will replace this with `load_workflow("dev", project_path)`
/// which serde-deserializes the JSON file with the same
/// shape. Until then, every workflow session runs against
/// this in-memory constant.
///
/// **All three transitions require user confirmation** —
/// the agent cannot self-advance the state; the gate IPC
/// `resolve_task_state_transition` (Phase 3) fires on
/// every requested move.
///
/// **Coordination**: `Pipeline` (single role per state).
/// `gather_strategy` is therefore empty. A future
/// `review` plugin will flip the coordination to
/// `SynthesisRound` and start populating `gather_strategy`.
pub fn default_workflow() -> WorkflowDef {
    WorkflowDef {
        name: "dev".to_string(),
        description: "Standard dev workflow: planning → implement → check → done with researcher / implementer / checker roles".to_string(),
        states: vec![
            "planning".to_string(),
            "implement".to_string(),
            "check".to_string(),
            "done".to_string(),
        ],
        initial: "planning".to_string(),
        transitions: vec![
            Transition {
                from: "planning".to_string(),
                to: "implement".to_string(),
                requires_user_confirm: true,
            },
            Transition {
                from: "implement".to_string(),
                to: "check".to_string(),
                requires_user_confirm: true,
            },
            Transition {
                from: "check".to_string(),
                to: "done".to_string(),
                requires_user_confirm: true,
            },
        ],
        roles_by_state: HashMap::from([
            ("planning".to_string(), vec!["researcher".to_string()]),
            ("implement".to_string(), vec!["implementer".to_string()]),
            ("check".to_string(), vec!["checker".to_string()]),
            // `done` has no per-state role — it triggers the
            // spec-distillation + archive hooks (Phase 3).
            ("done".to_string(), vec![]),
        ]),
        breadcrumb: HashMap::from([
            (
                "planning".to_string(),
                "[Wf · planning · dev] 调研 + 拆 task items;加载 wf-brainstorm skill;完成后请用户确认转 implement".to_string(),
            ),
            (
                "implement".to_string(),
                "[Wf · implement · dev] 推进 task items(update_checklist 改 task.json);用 wf-before-dev 检查规范;完成后请用户确认转 check".to_string(),
            ),
            (
                "check".to_string(),
                "[Wf · check · dev] 校验 + 测试;checker 跑 review(加载 wf-check);完成后请用户确认转 done".to_string(),
            ),
            (
                "done".to_string(),
                "[Wf · done · dev] 触发 spec 沉淀 + task 归档(spec-distillation 在 Phase 3 落地)".to_string(),
            ),
        ]),
        delegation_templates: HashMap::from([
            (
                "researcher".to_string(),
                "你是 dev workflow 的 researcher 子代理。当前 task: {title}\nSummary: {summary}\nState: {state}\n相关 spec 路径: {relevant_specs}\n\n请调研这个 task 的实现方案;返回 findings(关键决策点 + 风险 + 推荐路径)给主 LLM。".to_string(),
            ),
            (
                "implementer".to_string(),
                "你是 dev workflow 的 implementer 子代理。当前 task: {title}\nSummary: {summary}\nState: {state}\n相关 spec 路径: {relevant_specs}\n\n请推进 task.items 里 status=in_progress 的项(用 update_checklist 改写 task.json.items,非 loop-local Vec);改动前 read 相关 spec;改完 cargo check + cargo test --lib 确认不破坏现有行为。".to_string(),
            ),
            (
                "checker".to_string(),
                "你是 dev workflow 的 checker 子代理。当前 task: {title}\nSummary: {summary}\nState: {state}\n相关 spec 路径: {relevant_specs}\n\n请跑 cargo test --lib + cargo clippy + 检查 spec 合规;返回 PASS / FAIL + 具体原因给主 LLM。".to_string(),
            ),
        ]),
        coordination: Coordination::Pipeline,
        gather_strategy: HashMap::new(),
    }
}

// ---------------------------------------------------------------------------
// Phase 2 Step 2.1 — `load_workflow` + `validate` + path helper
// ---------------------------------------------------------------------------

/// Path to a workflow plugin's `workflow.json`:
/// `<project>/.everlasting/workflow/<name>/workflow.json`.
///
/// Mirrors the plugin-skills layout from [`crate::skill::loader::plugin_skills_dir`]
/// (same `.everlasting/workflow/<name>/` root) so plugin
/// authors only have to memorize one directory shape.
pub fn workflow_json_path(workflow_name: &str, project_path: &str) -> PathBuf {
    Path::new(project_path)
        .join(".everlasting")
        .join("workflow")
        .join(workflow_name)
        .join("workflow.json")
}

/// Discover available workflow plugins under
/// `<project>/.everlasting/workflow/<dir>/workflow.json`.
///
/// Returns the list of valid plugin names (alphabetical).
/// A directory is a valid plugin iff `workflow.json` exists
/// inside it. Empty directories are ignored without warning
/// (typical scratch state). Missing root dir → empty list
/// (no plugins = just the default dev workflow on next
/// `load_workflow` call).
///
/// Step 2.2: backs `list_workflow_plugins` IPC → the
/// `PluginSelect.vue` popover data source. Lives next to
/// `workflow_json_path` so the discovery logic and the
/// path arithmetic are in the same module (easier to keep
/// in sync if the layout changes — e.g. when Step 3.2
/// adds `.everlasting/spec/`).
pub fn list_plugins(project_path: &str) -> Vec<String> {
    let root = Path::new(project_path)
        .join(".everlasting")
        .join("workflow");
    let entries = match std::fs::read_dir(&root) {
        Ok(it) => it,
        Err(_) => return Vec::new(),
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            let workflow_json = path.join("workflow.json");
            if !workflow_json.is_file() {
                return None;
            }
            entry.file_name().into_string().ok()
        })
        .collect();
    names.sort();
    names
}

/// Validation errors from [`validate`]. Each variant
/// carries enough context for the loader's `warn!` to point
/// at the offending field — the on-disk JSON is the
/// plugin author's first debugging surface.
///
/// Step 2.1 ships 5 error categories (M6). Future variants
/// (e.g. role references outside the role registry) land
/// here as Step 2.x discovers them; the `warn!` in
/// `load_workflow` already iterates the error list, so
/// adding a variant doesn't ripple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    /// `states` was empty — the engine has no concept of
    /// "current state" without at least one.
    StatesEmpty,
    /// `initial` was not present in `states`.
    InitialNotInStates { initial: String },
    /// A transition's `from` / `to` references a state not
    /// in `states`.
    TransitionUnknownState {
        transition: (String, String),
        unknown: String,
    },
    /// A key in `roles_by_state` doesn't correspond to a
    /// declared state.
    RoleKeyUnknownState { key: String },
    /// A key in `breadcrumb` doesn't correspond to a
    /// declared state.
    BreadcrumbKeyUnknownState { key: String },
}

/// Enforce the structural invariants documented in the
/// module-level "Validation contract" section. Pure
/// function — no I/O, no logging — so the test layer can
/// assert against it directly.
///
/// Missing keys in `delegation_templates` / `breadcrumb` /
/// `gather_strategy` are NOT validation errors (they map
/// to empty / `None` accessor returns, per the accessor
/// docstrings). The validator only catches structurally-
/// broken defs.
pub fn validate(def: &WorkflowDef) -> Result<(), Vec<ValidationError>> {
    let mut errs = Vec::new();

    if def.states.is_empty() {
        errs.push(ValidationError::StatesEmpty);
    }

    // Initial ∈ states (only meaningful when states is non-empty).
    if !def.states.is_empty() && !def.states.iter().any(|s| s == &def.initial) {
        errs.push(ValidationError::InitialNotInStates {
            initial: def.initial.clone(),
        });
    }

    // Transitions: every from/to ∈ states.
    for t in &def.transitions {
        if !def.states.iter().any(|s| s == &t.from) {
            errs.push(ValidationError::TransitionUnknownState {
                transition: (t.from.clone(), t.to.clone()),
                unknown: t.from.clone(),
            });
        }
        if !def.states.iter().any(|s| s == &t.to) {
            errs.push(ValidationError::TransitionUnknownState {
                transition: (t.from.clone(), t.to.clone()),
                unknown: t.to.clone(),
            });
        }
    }

    // roles_by_state keys ⊆ states.
    for key in def.roles_by_state.keys() {
        if !def.states.iter().any(|s| s == key) {
            errs.push(ValidationError::RoleKeyUnknownState { key: key.clone() });
        }
    }

    // breadcrumb keys ⊆ states. Missing keys are fine (the
    // accessor returns ""), but keys claiming a state that
    // doesn't exist would silently never fire — that's
    // validation-worthy.
    for key in def.breadcrumb.keys() {
        if !def.states.iter().any(|s| s == key) {
            errs.push(ValidationError::BreadcrumbKeyUnknownState {
                key: key.clone(),
            });
        }
    }

    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs)
    }
}

/// Load a workflow plugin from
/// `<project>/.everlasting/workflow/<name>/workflow.json`.
///
/// **Failure mode**: returns `default_workflow()` on any
/// failure (file missing / I/O error / malformed JSON /
/// validation error). Each failure logs a `warn!` with
/// enough context for the plugin author to debug —
/// `name`, the offending JSON field if available, and a
/// short reason. **Never panics** — the engine consumer
/// (chat_loop) needs a working `WorkflowDef` for every
/// turn, even if the on-disk plugin is broken.
///
/// Why wholesale fallback (not partial application):
/// partial application violates the engine's invariants
/// mid-session (e.g. transitions to non-existent states),
/// and a session that re-enters with a different code path
/// depending on which JSON fields parsed would be very
/// hard to debug. Rejecting the file wholesale matches
/// the "engine never panics on a malformed WorkflowDef"
/// rule from Phase 0.
///
/// Step 2.1 ships the read + validate + fallback; the
/// hot-reload seam (reload on every turn boundary) is
/// wired in Step 2.2 (UI plugin switcher).
pub fn load_workflow(workflow_name: &str, project_path: &str) -> WorkflowDef {
    let path = workflow_json_path(workflow_name, project_path);

    // Read attempt — missing file is the common case
    // (projects without `.everlasting/workflow/` simply get
    // the default dev workflow). Trace at debug, not warn,
    // because missing-file is expected for new projects.
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(
                workflow = %workflow_name,
                path = %path.display(),
                "load_workflow: workflow.json not found, falling back to default_workflow()",
            );
            return default_workflow();
        }
        Err(e) => {
            tracing::warn!(
                workflow = %workflow_name,
                path = %path.display(),
                error = %e,
                "load_workflow: read failed, falling back to default_workflow()",
            );
            return default_workflow();
        }
    };

    // JSON parse.
    let parsed: WorkflowDef = match serde_json::from_str(&raw) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(
                workflow = %workflow_name,
                path = %path.display(),
                error = %e,
                "load_workflow: JSON parse failed, falling back to default_workflow()",
            );
            return default_workflow();
        }
    };

    // Structural validation.
    if let Err(errs) = validate(&parsed) {
        for err in &errs {
            tracing::warn!(
                workflow = %workflow_name,
                path = %path.display(),
                error = ?err,
                "load_workflow: validation failed, falling back to default_workflow()",
            );
        }
        return default_workflow();
    }

    parsed
}
