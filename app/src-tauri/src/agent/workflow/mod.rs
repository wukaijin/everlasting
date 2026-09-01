//! W1 (Workflow integration, 2026-07-08) — engine's content
//! layer. Phase 0 (Step 0.3) ships the in-memory `WorkflowDef`
//! type + 4 accessors + the `dev` plugin's `default_workflow()`
//! constant; Phase 2 (Step 2.1) extends `def.rs` with serde +
//! `load_workflow()` to read `.everlasting/workflow/<name>/workflow.json`.
//!
//! ## Layout (post-Phase-0)
//!
//! - [`def`] — types (`Transition` / `WorkflowDef` / `Coordination`) +
//!   4 accessors + `default_workflow()`. **Pure data — no I/O, no UI.**
//! - [`task`](self::task) — `TaskJson` read/write helpers +
//!   `.everlasting/tasks/<slug>/` directory layout (Phase 0
//!   Step 0.4).
//! - [`state`](self::state) — `set_task_state` + Rust
//!   transition hooks (Phase 3 Step 3.1).
//!
//! ## Engine contract
//!
//! Callers MUST funnel every `WorkflowDef` lookup through the
//! 4 accessors in [`def`]. Inlining `match state` against the
//! struct fields is a design violation — it skips the
//! infallible defaults and re-invents the missing-key
//! contract in every callsite.
//!
//! Phase 0 has **no plugin hot-reload**: the `WorkflowDef`
//! is a `default_workflow()` constant in memory for the
//! entire agent loop. Phase 2's `load_workflow(name, path)`
//! makes it swappable without changing this module's surface.
//!
//! ## Phase 0 test surface
//!
//! `#[cfg(test)] mod tests` lives at the bottom of this file
//! (NOT in `def.rs`) so the accessors' behavior is the
//! module's contract — the struct definitions in `def.rs`
//! are pure data and don't need their own test file.
//!
//! See `.trellis/tasks/07-08-workflow-integration/design.md §3.1`
//! and `.trellis/tasks/07-08-workflow-integration/design.md §5`
//! for the full design rationale.

/// `WorkflowDef` + accessors + `default_workflow()`.
pub mod def;

/// `TaskJson` + read / write + `create_task_init()`. Phase 0
/// ships this; Phase 2 Step 2.6 adds the partial-write path
/// that B12's `update_checklist` (workflow session branch)
/// will reuse to mutate `task.json.items` instead of the
/// loop-local `Vec<ChecklistItem>`.
pub mod task;

/// Per-session workflow context + per-turn breadcrumb
/// injection seam (`WorkflowCtx` / `build_workflow_ctx` /
/// `append_workflow_breadcrumb`). Phase 0 Step 0.5 shipped
/// this; Phase 2 Step 2.5 added
/// `append_delegation_template` as a sibling helper. D1
/// (08-31-cache-head-volatility) moved both injectors from
/// `messages[0]` to the request TAIL — see `inject.rs`'s
/// module doc for the prefix-cache rationale.
pub mod inject;

/// State transition + Rust 固定 hook (`set_task_state` +
/// `trigger_spec_distillation` + `preflight_implement_check`).
/// Phase 3 Step 3.1 ships this; the LLM-side distillation call
/// (Step 3.2) replaces the hook stub bodies.
pub mod state;

/// app 内置 plugin 源(`include_str!` 编译期常量)。07-09-workflow-builtin-plugin。
pub mod builtin;

// `task` + `state` slots are reserved for Phase 0 Step 0.4
// and Phase 3 Step 3.1 respectively. The first (`task`) is
// live since Step 0.4; the second (`state`) lands here.

// Re-export the public surface at the `agent::workflow::*`
// path so callers don't need to know about the `def` split.
// Convention follows `agent::permissions::*` (re-exported
// from `permissions/mod.rs`).
//
// Callers use the 4 accessor functions directly:
//   - `use agent::workflow::{default_workflow, breadcrumb_for,
//      allowed_roles, can_transition, delegation_template_for}`
// The `Coordination` enum is public so Phase 2's serde layer
// can call `Coordination::from_str_opt(&json_str)` directly
// without any helper indirection.
//
// `#[allow(unused_imports)]`: Phase 0 ships the surface but
// nothing in `agent/` consumes it yet — the engine consumer
// lands in Step 0.5 (chat_loop.rs's breadcrumb injection).
// Silencing the warning keeps `cargo check` clean and
// documents the intentional dead-import status. Phase 0.5
// removes the allow when the first consumer appears.
#[allow(unused_imports)]
pub use def::{
    allowed_roles, breadcrumb_for, can_transition, default_workflow, delegation_template_for,
    list_plugins, load_workflow, validate, workflow_json_path, Coordination, Transition,
    ValidationError, WorkflowDef,
};

// Re-export the task types + helpers at the workflow root.
// Tauri commands and chat_loop's per-turn injection
// consume these via `agent::workflow::{read_task, ...}` —
// same flatten-the-surface principle as the def side above.
//
// `task_json_path` / `task_prd_path` / `task_dir` are kept
// as `pub use`s so the IPC layer (`commands::task`) and
// test code can build paths without re-namespace hunting.
#[allow(unused_imports)]
pub use task::{
    archive_task_init, create_task_init, read_task, task_dir, task_json_path, task_prd_path,
    validate_slug, write_task, TaskError, TaskItem, TaskJson, TaskResult, TaskStatus,
    PROJ_NS_TASKS_ARCHIVE_DIR,
};
// C5: resolve_current_task is needed by set_session_plugin_name's
// task-remap path; re-export alongside the other task helpers.
pub use inject::resolve_current_task;

// Re-export the injection surface at the workflow root.
// `agent::chat::chat` (the IPC entry) imports
// `build_workflow_ctx`; `agent::chat_loop::run_chat_loop`
// imports `append_workflow_breadcrumb`. Keeping both
// under the same `agent::workflow` namespace simplifies
// the consumers' use-list.
#[allow(unused_imports)]
pub use inject::{
    append_delegation_template, append_workflow_breadcrumb, build_workflow_ctx,
    compute_delegation_template, WorkflowCtx,
};

// Re-export the state-transition surface at the workflow
// root. The IPC layer (`commands::question`) and the new
// `request_task_state_transition` tool consume these via
// `agent::workflow::{set_task_state, ...}` — same
// flatten-the-surface principle as the `def` / `task` /
// `inject` sides above.
#[allow(unused_imports)]
pub use state::{
    parse_target_state, preflight_implement_check, set_task_state, trigger_spec_distillation,
    StateResult, StateTransitionError,
};

// Re-export 内置源常量,供 skill/subagent loader 消费(07-09-workflow-builtin-plugin;
// 07-26-workflow-review-plugin C3 追加 review 组)。
#[allow(unused_imports)]
pub use builtin::{
    builtin_workflow_json, BUILTIN_DEV_AGENTS, BUILTIN_DEV_SKILLS, BUILTIN_PLUGIN_NAMES,
    BUILTIN_REVIEW_AGENTS, BUILTIN_REVIEW_SKILLS, BUILTIN_REVIEW_WORKFLOW_JSON,
};
// ---------------------------------------------------------------------------
// Tests — Phase 0 Step 0.3 acceptance: `cargo test --lib workflow`
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests_inject;
#[cfg(test)]
mod tests_task;

#[cfg(test)]
mod tests {
    use super::*;

    /// Tiny wrapper: pull `default_workflow()` ONCE per test
    /// (the constant shape is identical for every call).
    fn dev() -> WorkflowDef {
        default_workflow()
    }

    // --- shape ----------------------------------------------------------

    #[test]
    fn default_workflow_is_dev_with_three_states_in_order() {
        let w = dev();
        assert_eq!(w.name, "dev");
        assert_eq!(w.initial, "planning");
        assert_eq!(
            w.states,
            vec![
                "planning".to_string(),
                "in_progress".to_string(),
                "done".to_string(),
            ],
            "states MUST be ordered so the UI breadcrumb sequence reads top-down",
        );
    }

    #[test]
    fn default_workflow_has_two_declarative_transitions() {
        let w = dev();
        assert_eq!(w.transitions.len(), 2);
        for t in &w.transitions {
            assert!(
                t.requires_user_confirm,
                "every dev transition gates on user confirmation (state machine never self-advances)",
            );
        }
    }

    #[test]
    fn default_workflow_maps_each_state_to_expected_role() {
        let w = dev();
        assert_eq!(allowed_roles(&w, "planning"), &["researcher".to_string()]);
        // Post-merge: in_progress allows BOTH implementer + checker
        // (orchestrator LLM rotates them for adversarial review).
        assert_eq!(
            allowed_roles(&w, "in_progress"),
            &["implementer".to_string(), "checker".to_string()]
        );
        let done_roles = allowed_roles(&w, "done");
        assert!(done_roles.is_empty(), "done has no dispatchable roles");
    }

    #[test]
    fn default_workflow_sets_pipeline_coordination_and_empty_gather() {
        let w = dev();
        assert_eq!(w.coordination, Coordination::Pipeline);
        assert!(
            w.gather_strategy.is_empty(),
            "Pipeline coordination leaves gather_strategy unused (review plugin will populate it)",
        );
    }

    // --- transitions ----------------------------------------------------

    #[test]
    fn can_transition_recognizes_declared_edges() {
        let w = dev();
        assert!(can_transition(&w, "planning", "in_progress"));
        assert!(can_transition(&w, "in_progress", "done"));
    }

    #[test]
    fn can_transition_rejects_undeclared_edges() {
        let w = dev();
        // Skip-ahead: not in the dev plugin's declaration.
        assert!(!can_transition(&w, "planning", "done"));
        // Reverse: not declared.
        assert!(!can_transition(&w, "in_progress", "planning"));
        assert!(!can_transition(&w, "done", "in_progress"));
        // Legacy pre-merge states are no longer valid edges.
        assert!(!can_transition(&w, "planning", "implement"));
        assert!(!can_transition(&w, "check", "done"));
        // Unknown states.
        assert!(!can_transition(&w, "nope", "planning"));
        assert!(!can_transition(&w, "planning", "nope"));
    }

    // --- breadcrumb -----------------------------------------------------

    #[test]
    fn breadcrumb_for_returns_expected_text_per_state() {
        let w = dev();
        // Each state has a non-empty breadcrumb that names
        // the state name (sanity: the text is reachable +
        // identity-checkable by the injection seam).
        let p = breadcrumb_for(&w, "planning");
        assert!(!p.is_empty(), "planning must have a breadcrumb");
        assert!(p.contains("planning"));
        assert!(p.contains("wf-brainstorm"));

        let i = breadcrumb_for(&w, "in_progress");
        assert!(i.contains("in_progress"));
        assert!(i.contains("wf-before-dev"));
        // Post-merge: the in_progress breadcrumb also references
        // the adversarial checker workflow.
        assert!(i.contains("checker"));
        assert!(i.contains("wf-check"));

        let d = breadcrumb_for(&w, "done");
        assert!(d.contains("done"));
        assert!(d.contains("spec"));
    }

    #[test]
    fn breadcrumb_for_unknown_state_returns_empty_string() {
        // Infaillible default — the injection seam must
        // NEVER see a missing key forced back up the call
        // stack as `Option`.
        let w = dev();
        assert_eq!(breadcrumb_for(&w, "nonexistent"), "");
        assert_eq!(breadcrumb_for(&w, ""), "");
    }

    // --- delegation templates ------------------------------------------

    #[test]
    fn delegation_template_for_returns_some_for_each_dev_role() {
        let w = dev();
        let r = delegation_template_for(&w, "researcher").expect("researcher template");
        assert!(r.contains("{title}"));
        assert!(r.contains("{summary}"));
        assert!(r.contains("{state}"));
        assert!(r.contains("{relevant_specs}"));
        assert!(r.contains("researcher"));

        let i = delegation_template_for(&w, "implementer").expect("implementer template");
        assert!(i.contains("implementer"));
        assert!(i.contains("update_checklist"));

        let c = delegation_template_for(&w, "checker").expect("checker template");
        assert!(c.contains("checker"));
        // 08-27-builtin-agent-prompt-generalize:模板已脱栈通用化(原断言
        // `contains("cargo test")` 随 cargo 硬编码一并移除),改为锚定
        // 通用验证 + PASS/FAIL 判定结构。
        assert!(c.contains("项目验证"));
        assert!(c.contains("PASS"));
        assert!(c.contains("FAIL"));
    }

    #[test]
    fn delegation_template_for_unknown_role_returns_none() {
        // `None` carries semantic weight: "this plugin didn't
        // customize the worker for this role" — the
        // dispatcher falls back to the sub-agent's own
        // system prompt.
        let w = dev();
        assert!(delegation_template_for(&w, "general-purpose").is_none());
        assert!(delegation_template_for(&w, "").is_none());
        assert!(delegation_template_for(&w, "nope").is_none());
    }

    // --- allowed_roles fallback ----------------------------------------

    #[test]
    fn allowed_roles_for_unknown_state_returns_empty_slice() {
        let w = dev();
        let roles = allowed_roles(&w, "nonexistent");
        assert!(
            roles.is_empty(),
            "missing state MUST return empty slice, not panic"
        );
        // The slice's lifetime is tied to the def borrow —
        // explicitly test it to catch accidental `&'static`
        // shortcuts.
        let _: &[String] = roles;
    }

    // --- Coordination parser (used by Phase 2 serde) -------------------

    #[test]
    fn coordination_parser_recognizes_known_forms() {
        assert_eq!(
            Coordination::from_str_opt("pipeline"),
            Coordination::Pipeline
        );
        assert_eq!(
            Coordination::from_str_opt("synthesis_round"),
            Coordination::SynthesisRound
        );
        assert_eq!(
            Coordination::from_str_opt("synthesis-round"),
            Coordination::SynthesisRound
        );
        // Lenient: unknown → Pipeline (the dev default).
        assert_eq!(Coordination::from_str_opt("nope"), Coordination::Pipeline);
        assert_eq!(Coordination::from_str_opt(""), Coordination::Pipeline);
        assert_eq!(
            Coordination::from_str_opt("  Pipeline  "),
            Coordination::Pipeline,
            "whitespace + case-insensitive match"
        );
    }

    #[test]
    fn coordination_as_str_round_trips_through_parser() {
        // The Phase 2 loader will parse `workflow.json`
        // strings back into the enum via
        // `from_str_opt(as_str(x))`.
        for c in [Coordination::Pipeline, Coordination::SynthesisRound] {
            assert_eq!(Coordination::from_str_opt(c.as_str()), c);
        }
    }

    // --- Phase 2 Step 2.1: load_workflow + validate -----------------------

    /// Tiny helper: write `body` to a temp project dir at the
    /// canonical plugin path (`<tmp>/.everlasting/workflow/<wf>/workflow.json`)
    /// and return the tmp's project path. Mirrors
    /// `write_plugin_skill` in `skill::loader::tests`.
    fn write_workflow(project: &std::path::Path, wf: &str, body: &str) {
        let path = workflow_json_path(wf, &project.to_string_lossy());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
    }

    #[test]
    fn workflow_json_path_lands_under_workflow_subdir() {
        let p = workflow_json_path("dev", "/tmp/proj");
        assert_eq!(
            p,
            std::path::PathBuf::from(
                "/tmp/proj/.everlasting/workflow/dev/workflow.json",
            ),
            "workflow_json_path must resolve to `<project>/.everlasting/workflow/<wf>/workflow.json`",
        );
    }

    #[test]
    fn load_workflow_missing_file_falls_back_to_default() {
        // No file written — the loader must silently fall
        // back to `default_workflow()` (matches the
        // pre-Step-2.1 behavior: non-workflow callers got
        // the default, workflow callers also got the
        // default, no plugin JSON existed yet).
        let proj_tmp = tempfile::TempDir::new().unwrap();
        let path = proj_tmp.path().to_string_lossy().to_string();

        let loaded = load_workflow("dev", &path);
        assert_eq!(loaded.name, "dev");
        assert_eq!(loaded.initial, "planning");
        assert_eq!(loaded.states.len(), 3);
    }

    #[test]
    fn load_workflow_valid_json_overrides_default() {
        // Write a workflow with a non-default state name
        // (`intake` instead of `planning`) — the loader
        // must pick it up so the per-state breadcrumb /
        // roles track the file.
        let proj_tmp = tempfile::TempDir::new().unwrap();
        write_workflow(
            proj_tmp.path(),
            "review",
            r#"{
  "name": "review",
  "description": "slim review plugin",
  "states": ["intake", "synth", "done"],
  "initial": "intake",
  "transitions": [
    {"from": "intake", "to": "synth", "requires_user_confirm": true},
    {"from": "synth", "to": "done", "requires_user_confirm": true}
  ],
  "roles_by_state": {
    "intake": ["researcher"],
    "synth": ["synthesizer"],
    "done": []
  },
  "breadcrumb": {
    "intake": "REVIEW-INTAKE",
    "synth": "REVIEW-SYNTH",
    "done": "REVIEW-DONE"
  },
  "delegation_templates": {
    "synthesizer": "synth template"
  },
  "coordination": "synthesis_round",
  "gather_strategy": {"synth": ["researcher", "synthesizer"]}
}"#,
        );
        let path = proj_tmp.path().to_string_lossy().to_string();

        let loaded = load_workflow("review", &path);
        assert_eq!(loaded.name, "review");
        assert_eq!(loaded.states, vec!["intake", "synth", "done"]);
        assert_eq!(loaded.initial, "intake");
        assert_eq!(loaded.coordination, Coordination::SynthesisRound);
        assert_eq!(breadcrumb_for(&loaded, "intake"), "REVIEW-INTAKE");
        assert_eq!(breadcrumb_for(&loaded, "synth"), "REVIEW-SYNTH");
        assert_eq!(
            allowed_roles(&loaded, "synth"),
            &["synthesizer".to_string()]
        );
        assert!(can_transition(&loaded, "intake", "synth"));
        assert!(!can_transition(&loaded, "synth", "intake"));
        assert_eq!(
            delegation_template_for(&loaded, "synthesizer"),
            Some("synth template"),
        );
    }

    #[test]
    fn load_workflow_malformed_json_falls_back_with_warn() {
        // Trailing garbage → serde_json::from_str fails →
        // loader returns default_workflow(). We can't
        // easily assert the warn! line here (tracing
        // subscriber isn't initialized in tests), but the
        // observable contract is "fallback returned".
        let proj_tmp = tempfile::TempDir::new().unwrap();
        write_workflow(proj_tmp.path(), "broken", "{ not valid json");
        let path = proj_tmp.path().to_string_lossy().to_string();

        let loaded = load_workflow("broken", &path);
        // Default's name is "dev" — proof we fell back.
        assert_eq!(loaded.name, "dev");
        assert_eq!(loaded.initial, "planning");
    }

    #[test]
    fn load_workflow_validation_failure_falls_back() {
        // `initial` not in `states` → validate() returns Err
        // → loader falls back to default.
        let proj_tmp = tempfile::TempDir::new().unwrap();
        write_workflow(
            proj_tmp.path(),
            "bad",
            r#"{
  "name": "bad",
  "description": "x",
  "states": ["a", "b"],
  "initial": "missing",
  "transitions": [],
  "roles_by_state": {},
  "breadcrumb": {},
  "delegation_templates": {},
  "coordination": "pipeline",
  "gather_strategy": {}
}"#,
        );
        let path = proj_tmp.path().to_string_lossy().to_string();

        let loaded = load_workflow("bad", &path);
        assert_eq!(loaded.name, "dev", "validation failure must fallback");
    }

    // --- validate() direct unit tests ------------------------------------

    #[test]
    fn validate_passes_on_default_workflow() {
        let w = dev();
        assert!(
            validate(&w).is_ok(),
            "default_workflow() must self-validate"
        );
    }

    #[test]
    fn validate_rejects_empty_states() {
        let mut w = dev();
        w.states.clear();
        let errs = validate(&w).expect_err("empty states must fail");
        assert!(errs
            .iter()
            .any(|e| matches!(e, ValidationError::StatesEmpty)));
    }

    #[test]
    fn validate_rejects_unknown_transition_state() {
        let mut w = dev();
        w.transitions.push(Transition {
            from: "planning".to_string(),
            to: "nowhere".to_string(),
            requires_user_confirm: true,
        });
        let errs = validate(&w).expect_err("unknown transition target must fail");
        assert!(
            errs.iter()
                .any(|e| matches!(e, ValidationError::TransitionUnknownState { .. })),
            "expected TransitionUnknownState, got: {errs:?}",
        );
    }

    #[test]
    fn validate_rejects_unknown_role_key() {
        let mut w = dev();
        w.roles_by_state
            .insert("ghost_state".to_string(), vec!["researcher".to_string()]);
        let errs = validate(&w).expect_err("unknown role key must fail");
        assert!(
            errs.iter()
                .any(|e| matches!(e, ValidationError::RoleKeyUnknownState { .. })),
            "expected RoleKeyUnknownState, got: {errs:?}",
        );
    }

    #[test]
    fn validate_allows_missing_breadcrumb_keys() {
        // Missing keys in breadcrumb are NOT validation
        // errors — the accessor returns "" on miss. Only
        // keys claiming a non-existent state would be
        // silently-dead-fire and thus validation-worthy.
        let mut w = dev();
        w.breadcrumb.remove("planning");
        assert!(
            validate(&w).is_ok(),
            "missing breadcrumb keys are non-blocking (accessor returns empty string)",
        );
    }

    // --- Phase 2 Step 2.2: list_plugins discovery -----------------------

    #[test]
    fn list_plugins_returns_empty_when_root_missing() {
        // 07-09-workflow-builtin-plugin:现在即使项目无 workflow 目录,
        // 也返回内置 plugin 名(至少 dev),不再为空。
        // 07-26-workflow-review-plugin C3:内置清单现在含 dev + review。
        let proj_tmp = tempfile::TempDir::new().unwrap();
        let path = proj_tmp.path().to_string_lossy().to_string();
        assert_eq!(
            list_plugins(&path),
            vec!["dev".to_string(), "review".to_string()]
        );
    }

    #[test]
    fn list_plugins_discovers_alphabetical() {
        // Two plugins written out-of-order; `list_plugins`
        // must sort alphabetically (deterministic popover
        // order in `PluginSelect.vue`). The builtin `dev` +
        // `review` are always included (07-09-workflow-builtin-plugin /
        // 07-26-workflow-review-plugin C3).
        let proj_tmp = tempfile::TempDir::new().unwrap();
        write_workflow(proj_tmp.path(), "zulu", "{}");
        write_workflow(proj_tmp.path(), "alpha", "{}");
        let path = proj_tmp.path().to_string_lossy().to_string();
        assert_eq!(list_plugins(&path), vec!["alpha", "dev", "review", "zulu"]);
    }

    #[test]
    fn list_plugins_ignores_dirs_without_workflow_json() {
        // A directory without `workflow.json` is not a
        // plugin — silently ignored (matches the scratch
        // state contract: empty dirs are typical).
        // The builtin `dev` + `review` are always included
        // (07-09-workflow-builtin-plugin /
        // 07-26-workflow-review-plugin C3).
        let proj_tmp = tempfile::TempDir::new().unwrap();
        write_workflow(proj_tmp.path(), "real", "{}");
        // Empty sibling dir — no workflow.json
        std::fs::create_dir_all(proj_tmp.path().join(".everlasting/workflow/scratch")).unwrap();
        let path = proj_tmp.path().to_string_lossy().to_string();
        assert_eq!(list_plugins(&path), vec!["dev", "real", "review"]);
    }

    #[test]
    fn list_plugins_always_includes_builtin_dev() {
        // 空项目目录 → 只有内置 dev + review(项目可覆盖 + 内置 fallback 的核心行为)。
        // 07-09-workflow-builtin-plugin: 这是新增断言,与上面的修改独立。
        // 07-26-workflow-review-plugin C3: review 加入内置清单。
        let proj_tmp = tempfile::TempDir::new().unwrap();
        let path = proj_tmp.path().to_string_lossy().to_string();
        let plugins = list_plugins(&path);
        assert!(
            plugins.contains(&"dev".to_string()),
            "builtin dev always present: {plugins:?}"
        );
        assert!(
            plugins.contains(&"review".to_string()),
            "builtin review always present (C3): {plugins:?}"
        );
    }

    #[test]
    fn load_workflow_falls_back_to_builtin_when_project_missing() {
        // 项目无 dev 目录 → 内置 dev(非 default_workflow 常量路径,但二者等价)。
        // 07-09-workflow-builtin-plugin: 验证空项目能拿到内置 dev WorkflowDef。
        let proj_tmp = tempfile::TempDir::new().unwrap();
        let path = proj_tmp.path().to_string_lossy().to_string();
        let loaded = load_workflow("dev", &path);
        assert_eq!(loaded.name, "dev");
        assert_eq!(loaded.initial, "planning");
        assert_eq!(loaded.states.len(), 3);
    }

    // ---- Step 2.5: delegation template helpers ------------------------

    /// Minimal `WorkflowCtx` carrying the dev plugin's
    /// delegation_templates (researcher / implementer /
    /// checker). Tests can mutate `current_task` to drive
    /// the placeholder substitution.
    fn dev_ctx_with_task(title: &str, summary: &str, status: TaskStatus) -> WorkflowCtx {
        let workflow_def = default_workflow();
        WorkflowCtx {
            task_workflow_def: workflow_def.clone(),
            workflow_def,
            current_task: Some(TaskJson {
                id: "t1".into(),
                title: title.into(),
                slug: "s1".into(),
                status,
                created_at: "2026-07-09T00:00:00Z".into(),
                updated_at: "2026-07-09T00:00:00Z".into(),
                parent: None,
                summary: summary.into(),
                items: vec![],
                // Step 3.3: pre-archive fixture.
                completed_at: None,
                // C5: dev fixture → dev plugin.
                workflow_plugin: "dev".into(),
            }),
            malformed_tasks: Vec::new(),
        }
    }

    #[test]
    fn delegation_template_substitutes_placeholders() {
        let ctx = dev_ctx_with_task(
            "add wf-overview skill",
            "investigate skill loader plugin layer",
            TaskStatus::Planning,
        );
        let tmp = tempfile::TempDir::new().unwrap();
        let project_path = tmp.path().to_string_lossy().to_string();

        let filled = compute_delegation_template(&ctx, &project_path, "researcher")
            .expect("dev plugin defines researcher template");
        assert!(
            filled.contains("add wf-overview skill"),
            "{{title}} must substitute (got: {filled})",
        );
        assert!(
            filled.contains("investigate skill loader plugin layer"),
            "{{summary}} must substitute",
        );
        assert!(filled.contains("planning"), "{{state}} must substitute",);
        assert!(
            !filled.contains("{title}"),
            "no unsubstituted placeholders should remain (got: {filled})",
        );
        assert!(
            !filled.contains("{relevant_specs}"),
            "relevant_specs placeholder should be resolved (got: {filled})",
        );
    }

    #[test]
    fn delegation_template_relevant_specs_falls_back_when_dir_missing() {
        // Project with no `.everlasting/spec/` → fallback
        // hint text. The fallback message names the
        // `wf-before-dev` skill so the worker has an
        // actionable next step.
        let ctx = dev_ctx_with_task("t", "s", TaskStatus::Planning);
        let tmp = tempfile::TempDir::new().unwrap();
        let project_path = tmp.path().to_string_lossy().to_string();

        let filled = compute_delegation_template(&ctx, &project_path, "implementer")
            .expect("dev plugin defines implementer template");
        assert!(
            filled.contains("(auto-detect via wf-before-dev)"),
            "missing spec dir → fallback hint text (got: {filled})",
        );
    }

    #[test]
    fn delegation_template_relevant_specs_lists_md_files() {
        // Project with `.everlasting/spec/agents/backend/index.md`
        // → that path appears (relative to project root) in
        // the filled template. The recursive walk must
        // descend into subdirs (`agents/backend/`) to find
        // nested .md files; a flat top-level listing would
        // miss them.
        let ctx = dev_ctx_with_task("t", "s", TaskStatus::InProgress);
        let tmp = tempfile::TempDir::new().unwrap();
        let spec_dir = tmp.path().join(".everlasting").join("spec");
        std::fs::create_dir_all(spec_dir.join("agents/backend")).unwrap();
        std::fs::write(spec_dir.join("agents/backend/index.md"), "# backend spec").unwrap();
        std::fs::write(spec_dir.join("agents/backend/style.md"), "# style").unwrap();
        // A non-.md file should be ignored.
        std::fs::write(spec_dir.join("README.txt"), "ignored").unwrap();
        // Top-level .md too — should also appear.
        std::fs::write(spec_dir.join("top.md"), "# top").unwrap();

        let project_path = tmp.path().to_string_lossy().to_string();
        let filled = compute_delegation_template(&ctx, &project_path, "checker")
            .expect("dev plugin defines checker template");
        assert!(
            filled.contains(".everlasting/spec/agents/backend/index.md"),
            "nested spec file path must appear in filled template (got: {filled})",
        );
        assert!(
            filled.contains(".everlasting/spec/agents/backend/style.md"),
            "second nested spec file must also appear (got: {filled})",
        );
        assert!(
            filled.contains(".everlasting/spec/top.md"),
            "top-level spec file must also appear (got: {filled})",
        );
        assert!(
            !filled.contains("README.txt"),
            "non-.md files must be ignored (got: {filled})",
        );
    }

    #[test]
    fn delegation_template_unknown_role_returns_none() {
        // Plugin doesn't define a template for this role
        // → None (caller falls back to the sub-agent's
        // own system prompt).
        let ctx = dev_ctx_with_task("t", "s", TaskStatus::Planning);
        let tmp = tempfile::TempDir::new().unwrap();
        let project_path = tmp.path().to_string_lossy().to_string();
        let filled = compute_delegation_template(&ctx, &project_path, "general-purpose");
        assert!(
            filled.is_none(),
            "general-purpose has no dev-plugin template; expected None (got: {:?})",
            filled
        );
    }

    #[test]
    fn append_delegation_template_pushes_to_user_blocks() {
        // D1 (08-31-cache-head-volatility): the template appends to
        // the worker's LAST message (the delegation task), NOT
        // `messages[0]` (the memory head). A per-dispatch template in
        // the memory head forked the worker-side cached prefix on
        // every dispatch.
        use crate::llm::types::{ChatMessage, ContentBlock, MessageContent, Role};
        let mut messages = vec![
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![ContentBlock::Text {
                    text: "memory block".into(),
                    cache_control: None,
                }]),
                speaker: None,
                attachments: None,
            },
            ChatMessage {
                role: Role::Assistant,
                content: MessageContent::Text("ack".into()),
                speaker: None,
                attachments: None,
            },
            ChatMessage {
                role: Role::User,
                content: MessageContent::Text("delegation task".into()),
                speaker: None,
                attachments: None,
            },
        ];
        let ok = append_delegation_template(&mut messages, Some("PLUGIN_TEMPLATE".to_string()));
        assert!(ok, "append must succeed for a user-role tail message");
        assert_eq!(messages.len(), 3, "no synthetic message opened");
        // Head (memory block) untouched.
        match &messages[0].content {
            MessageContent::Blocks(bs) => {
                assert_eq!(bs.len(), 1, "memory head must stay untouched (D1)");
            }
            _ => panic!("messages[0] should still be Blocks"),
        }
        // Tail widened to Blocks([task, template]).
        match &messages[2].content {
            MessageContent::Blocks(bs) => {
                assert_eq!(bs.len(), 2, "should have 2 blocks (task + template)");
                match &bs[0] {
                    ContentBlock::Text { text, .. } => assert_eq!(text, "delegation task"),
                    _ => panic!("expected the task text block first"),
                }
                match &bs[1] {
                    ContentBlock::Text {
                        text,
                        cache_control,
                    } => {
                        assert_eq!(text.trim(), "PLUGIN_TEMPLATE");
                        assert!(cache_control.is_none());
                    }
                    _ => panic!("expected Text block at index 1"),
                }
            }
            _ => panic!("tail message should have been widened to Blocks"),
        }
    }

    #[test]
    fn append_delegation_template_skips_on_none_template() {
        // No plugin template → no-op (returns false,
        // messages untouched).
        use crate::llm::types::{ChatMessage, ContentBlock, MessageContent, Role};
        let mut messages = vec![ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(vec![ContentBlock::Text {
                text: "memory".into(),
                cache_control: None,
            }]),
            speaker: None,
            attachments: None,
        }];
        let ok = append_delegation_template(&mut messages, None);
        assert!(!ok, "None template → returns false");
        if let MessageContent::Blocks(blocks) = &messages[0].content {
            assert_eq!(blocks.len(), 1, "no template → blocks unchanged");
        } else {
            panic!("messages[0] should still be Blocks");
        }
    }

    #[test]
    fn append_delegation_template_skips_when_messages_empty() {
        // S-B guard: no messages → can't append.
        use crate::llm::types::ChatMessage;
        let mut messages: Vec<ChatMessage> = vec![];
        let ok = append_delegation_template(&mut messages, Some("body".to_string()));
        assert!(!ok, "empty messages → no-op");
    }
}
