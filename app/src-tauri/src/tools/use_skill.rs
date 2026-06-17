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
use crate::skill::loader::{find_skill, SkillCache};
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
    match find_skill(skill_cache, name, Some(&project_path)).await {
        Some(res) => {
            tracing::info!(
                skill = %res.name,
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
