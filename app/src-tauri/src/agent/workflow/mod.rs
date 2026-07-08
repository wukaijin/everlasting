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
/// `append_workflow_breadcrumb`). Phase 0 Step 0.5 ships
/// this; Phase 2 Step 2.5 will add
/// `append_delegation_template` as a sibling helper that
/// composes onto the same `messages[0]` block list.
pub mod inject;

// `task` + `state` slots are reserved for Phase 0 Step 0.4
// and Phase 3 Step 3.1 respectively. The first (`task`) is
// live now; the second (`state`) lands later.
//
// pub mod state; // Phase 3 Step 3.1

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
    allowed_roles, breadcrumb_for, can_transition, default_workflow,
    delegation_template_for, Coordination, Transition, WorkflowDef,
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
    create_task_init, read_task, task_dir, task_json_path, task_prd_path, validate_slug,
    write_task, TaskError, TaskItem, TaskJson, TaskResult, TaskStatus,
};

// Re-export the injection surface at the workflow root.
// `agent::chat::chat` (the IPC entry) imports
// `build_workflow_ctx`; `agent::chat_loop::run_chat_loop`
// imports `append_workflow_breadcrumb`. Keeping both
// under the same `agent::workflow` namespace simplifies
// the consumers' use-list.
#[allow(unused_imports)]
pub use inject::{
    append_workflow_breadcrumb, build_workflow_ctx, WorkflowCtx,
};// ---------------------------------------------------------------------------
// Tests — Phase 0 Step 0.3 acceptance: `cargo test --lib workflow`
// ---------------------------------------------------------------------------

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
    fn default_workflow_is_dev_with_four_states_in_order() {
        let w = dev();
        assert_eq!(w.name, "dev");
        assert_eq!(w.initial, "planning");
        assert_eq!(
            w.states,
            vec![
                "planning".to_string(),
                "implement".to_string(),
                "check".to_string(),
                "done".to_string(),
            ],
            "states MUST be ordered so the UI breadcrumb sequence reads top-down",
        );
    }

    #[test]
    fn default_workflow_has_three_declarative_transitions() {
        let w = dev();
        assert_eq!(w.transitions.len(), 3);
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
        // Symmetric: each non-terminal state has exactly one
        // role; `done` has none (it triggers spec distillation,
        // not a dispatch).
        assert_eq!(allowed_roles(&w, "planning"), &["researcher".to_string()]);
        assert_eq!(allowed_roles(&w, "implement"), &["implementer".to_string()]);
        assert_eq!(allowed_roles(&w, "check"), &["checker".to_string()]);
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
        assert!(can_transition(&w, "planning", "implement"));
        assert!(can_transition(&w, "implement", "check"));
        assert!(can_transition(&w, "check", "done"));
    }

    #[test]
    fn can_transition_rejects_undeclared_edges() {
        let w = dev();
        // Skip-ahead: not in the dev plugin's declaration.
        assert!(!can_transition(&w, "planning", "check"));
        assert!(!can_transition(&w, "planning", "done"));
        // Reverse: not declared.
        assert!(!can_transition(&w, "implement", "planning"));
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

        let i = breadcrumb_for(&w, "implement");
        assert!(i.contains("implement"));
        assert!(i.contains("wf-before-dev"));

        let c = breadcrumb_for(&w, "check");
        assert!(c.contains("check"));
        assert!(c.contains("wf-check"));

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
        assert!(c.contains("cargo test"));
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
        assert!(roles.is_empty(), "missing state MUST return empty slice, not panic");
        // The slice's lifetime is tied to the def borrow —
        // explicitly test it to catch accidental `&'static`
        // shortcuts.
        let _: &[String] = roles;
    }

    // --- Coordination parser (used by Phase 2 serde) -------------------

    #[test]
    fn coordination_parser_recognizes_known_forms() {
        assert_eq!(Coordination::from_str_opt("pipeline"), Coordination::Pipeline);
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
}
