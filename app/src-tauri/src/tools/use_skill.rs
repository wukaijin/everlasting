//! B4 `use_skill` virtual tool — L1 activation.
//!
//! The model calls `use_skill(skill_name)` to load a skill's
//! `SKILL.md` body (L1 progressive disclosure). The body is returned
//! as the tool_result and stays in the conversation for the rest of
//! the session — PR2 brainstorm decision: take the tool_result path,
//! NOT a system-prompt injection. This keeps the `cache_control`
//! structure intact (system prompt stays the stable cached segment)
//! and reuses the existing ⑫ tool_result / ⑩ audit / C3 compaction
//! pair-protection channels with zero new code paths.
//!
//! Named `use_skill.rs` (not `skill.rs`) to avoid confusion with the
//! top-level `crate::skill` loader module this tool reads from.

use crate::llm::types::ToolDef;
use crate::skill::loader::{find_skill_with_workflow, SkillCache};
use crate::tools::ToolContext;

/// The `use_skill` tool definition registered in `builtin_tools()`.
///
/// The description tells the model to consult the `<available-skills>`
/// block (L0 listing injected at session start) and call this tool
/// when the task matches — the model's tool-use ability does the
/// dispatch, no rule engine.
pub fn definition() -> ToolDef {
    ToolDef {
        name: "use_skill".to_string(),
        description: Some(
            "Load a skill's full instruction body. Call this when the \
             user's task matches one of the available skills listed in \
             the <available-skills> block. Pass the skill's exact name. \
             The skill body becomes part of the conversation — follow \
             its instructions to complete the task."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "skill_name": {
                    "type": "string",
                    "description": "The exact name of the skill to load, as listed in <available-skills>."
                }
            },
            "required": ["skill_name"]
        }),
    }
}

/// Execute `use_skill`: resolve the skill body from the cache (L1).
///
/// Returns `(body, is_error)`. `project_path` comes from the tool
/// context's worktree root so the project skill layer (which overrides
/// user) is consulted. An unknown name returns `is_error=true` so the
/// LLM can self-correct (standard ⑫ error-feedback path).
///
/// Step 1.5 (2026-07-08, `07-08-workflow-integration`): the
/// loader is the workflow-aware variant — when
/// `ctx.workflow_name` is `Some(name)`, the plugin skill
/// layer (`<project>/.everlasting/workflow/<name>/skills/`)
/// is consulted ahead of the project / user layers, so
/// workflow session agents can load `wf-overview` /
/// `wf-brainstorm` etc. without falling back to "Skill not
/// found". Non-workflow sessions keep the legacy path.
pub async fn execute(
    input: &serde_json::Value,
    skill_cache: &SkillCache,
    ctx: &ToolContext,
) -> (String, bool) {
    let Some(name) = input.get("skill_name").and_then(|v| v.as_str()) else {
        return (
            "use_skill requires a `skill_name` string argument.".to_string(),
            true,
        );
    };
    let project_path = ctx.worktree_path.to_string_lossy().to_string();
    match find_skill_with_workflow(
        skill_cache,
        name,
        Some(&project_path),
        ctx.workflow_name.as_deref(),
    )
    .await
    {
        Some(res) => {
            tracing::info!(
                skill = %res.name,
                workflow = ?ctx.workflow_name,
                path = %res.path.display(),
                "use_skill: loaded skill body (L1 activation)"
            );
            (res.body, false)
        }
        None => (
            format!(
                "Skill `{}` not found. Check the <available-skills> block for the exact name.",
                name
            ),
            true,
        ),
    }
}

#[cfg(test)]
mod tests {
    //! Step 1.5 wiring tests for `use_skill` (07-08-workflow-integration).
    //!
    //! Verifies that the `use_skill` tool now consults the plugin
    //! skill layer when `ToolContext::workflow_name` is `Some`,
    //! closing the gap left after Step 1.1 (which shipped
    //! `find_skill_with_workflow` without a consumer). Before this
    //! commit, a workflow session asking for `wf-overview` returned
    //! "Skill not found" because the loader never saw the workflow
    //! flag.

    use super::*;
    use crate::memory::file::set_user_dir_for_test;
    use crate::skill::loader::{plugin_skills_dir, SkillCache};
    use crate::tools::update_checklist;

    /// Write a skill at `<project>/.everlasting/workflow/<wf>/skills/<name>/SKILL.md`.
    /// Mirrors the private `write_plugin_skill` helper in `loader::tests`.
    fn write_plugin_skill(project: &std::path::Path, wf: &str, name: &str, body: &str) {
        let dir = plugin_skills_dir(wf, &project.to_string_lossy());
        std::fs::create_dir_all(dir.join(name)).unwrap();
        std::fs::write(dir.join(name).join("SKILL.md"), body).unwrap();
    }

    /// Minimal `ToolContext` for use_skill. Mirrors the pattern used
    /// in other tools' `test_ctx` helpers (see `edit_file::tests`).
    fn make_ctx(project_path: &std::path::Path, workflow_name: Option<String>) -> ToolContext {
        ToolContext {
            tool_use_id: None,
            escalation: Default::default(),
            worktree_path: project_path.to_path_buf(),
            cwd: project_path.to_path_buf(),
            checklist: update_checklist::new_handle(),
            background_shells: crate::background_shell::default_registry(),
            db: crate::tools::test_default_pool(),
            project_id: "test-proj".to_string(),
            data_dir: project_path.to_path_buf(),
            workflow_name,
            mode: crate::db::Mode::Edit,
        }
    }

    #[tokio::test]
    async fn use_skill_resolves_plugin_layer_when_workflow_name_set() {
        // The plugin skill lives ONLY in the workflow plugin layer.
        // Without the workflow flag, the legacy `find_skill` call
        // path would miss it — the regression we're guarding.
        let user_tmp = tempfile::TempDir::new().unwrap();
        let proj_tmp = tempfile::TempDir::new().unwrap();
        write_plugin_skill(
            proj_tmp.path(),
            "dev",
            "wf-overview",
            "---\nname: wf-overview\ndescription: p\n---\nPLUGIN_BODY",
        );

        let prev = set_user_dir_for_test(Some(user_tmp.path().to_path_buf()));
        let cache = SkillCache::arc();
        let project_path = proj_tmp.path().to_string_lossy().to_string();

        // Workflow ctx → plugin layer hit.
        let ctx_wf = make_ctx(std::path::Path::new(&project_path), Some("dev".to_string()));
        let (body, is_err) = execute(
            &serde_json::json!({"skill_name": "wf-overview"}),
            &cache,
            &ctx_wf,
        )
        .await;
        assert!(!is_err, "workflow use_skill must NOT error");
        assert_eq!(body, "PLUGIN_BODY");

        // No workflow ctx → same call falls through to "not found".
        let ctx_none = make_ctx(std::path::Path::new(&project_path), None);
        let (body, is_err) = execute(
            &serde_json::json!({"skill_name": "wf-overview"}),
            &cache,
            &ctx_none,
        )
        .await;
        assert!(is_err, "non-workflow use_skill must miss the plugin layer");
        assert!(
            body.contains("not found"),
            "non-workflow miss should surface a not-found message, got: {body}"
        );

        set_user_dir_for_test(prev);
    }

    #[tokio::test]
    async fn use_skill_empty_workflow_name_treated_as_none() {
        // `Some("")` is normalized to `None` inside
        // `find_skill_with_workflow`. Verify the wiring respects that
        // — an empty plugin name must NOT silently swallow a real
        // skill lookup miss.
        let user_tmp = tempfile::TempDir::new().unwrap();
        let proj_tmp = tempfile::TempDir::new().unwrap();
        // No plugin skill present — empty name should hit the same
        // "not found" branch as None.
        let prev = set_user_dir_for_test(Some(user_tmp.path().to_path_buf()));
        let cache = SkillCache::arc();
        let project_path = proj_tmp.path().to_string_lossy().to_string();

        let ctx = make_ctx(std::path::Path::new(&project_path), Some(String::new()));
        let (_body, is_err) =
            execute(&serde_json::json!({"skill_name": "anything"}), &cache, &ctx).await;
        assert!(is_err, "empty workflow_name must behave like None");
        set_user_dir_for_test(prev);
    }
}
