//! W1-builtin (2026-07-09): app 内置 dev workflow 的编译期常量源。
//!
//! `include_str!` 在编译期把 `resources/builtin-workflow/dev/` 下的内容
//! 文件读成 `&'static str`,嵌进二进制。运行时 loader 查不到项目层
//! (`.everlasting/workflow/<name>/`)时 fallback 到这里。
//!
//! **为什么不用 Tauri bundle.resources**:本项目零 resource bundle 先例
//! (`tauri.conf.json` 无 `resources` 键),内置内容惯例全是硬编码 Rust 常量
//! (`builtin_subagents()` / `default_workflow()`)。`include_str!` 是该惯例
//! 的自然延伸,dev 构建即用,无运行时 resource_dir 解析依赖。详见
//! `.trellis/tasks/07-09-workflow-builtin-plugin/design.md §1`。

/// 内置 dev 的 workflow.json 文本(与 `default_workflow()` 常量逐字等价,
/// 但走 serde 解析路径,保证"内置层结构 == 项目层结构")。
pub const BUILTIN_DEV_WORKFLOW_JSON: &str =
    include_str!("../../../resources/builtin-workflow/dev/workflow.json");

/// (slug, SKILL.md body) —— 内置 dev 的 5 个 wf-* skill。
/// slug 必须等于 SKILL.md 所在子目录名(skill loader 用目录名做 fallback name)。
pub const BUILTIN_DEV_SKILLS: &[(&str, &str)] = &[
    ("wf-overview", include_str!("../../../resources/builtin-workflow/dev/skills/wf-overview/SKILL.md")),
    ("wf-brainstorm", include_str!("../../../resources/builtin-workflow/dev/skills/wf-brainstorm/SKILL.md")),
    ("wf-before-dev", include_str!("../../../resources/builtin-workflow/dev/skills/wf-before-dev/SKILL.md")),
    ("wf-check", include_str!("../../../resources/builtin-workflow/dev/skills/wf-check/SKILL.md")),
    ("wf-update-spec", include_str!("../../../resources/builtin-workflow/dev/skills/wf-update-spec/SKILL.md")),
];

/// (role_name, agent.md body) —— 内置 dev 的 3 个角色 agent。
/// role_name 必须等于 frontmatter 的 `name:` 字段(agent loader 要求显式 name)。
pub const BUILTIN_DEV_AGENTS: &[(&str, &str)] = &[
    ("researcher", include_str!("../../../resources/builtin-workflow/dev/agents/researcher.md")),
    ("implementer", include_str!("../../../resources/builtin-workflow/dev/agents/implementer.md")),
    ("checker", include_str!("../../../resources/builtin-workflow/dev/agents/checker.md")),
];

/// 内置 plugin 名清单。`list_plugins` 用它做并集发现。
/// 目前只有 dev;未来加 review 时在此追加 + 加对应常量。
pub const BUILTIN_PLUGIN_NAMES: &[&str] = &["dev"];

/// 返回内置 plugin `<name>` 的 workflow.json 文本。
/// `None` = 无此内置 plugin(交由 caller 继续降级)。
pub fn builtin_workflow_json(name: &str) -> Option<&'static str> {
    match name {
        "dev" => Some(BUILTIN_DEV_WORKFLOW_JSON),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::workflow::{validate, WorkflowDef};

    #[test]
    fn builtin_dev_workflow_json_validates() {
        let def: WorkflowDef =
            serde_json::from_str(BUILTIN_DEV_WORKFLOW_JSON).expect("builtin dev JSON parses");
        assert!(validate(&def).is_ok(), "builtin dev JSON must self-validate");
        assert_eq!(def.name, "dev");
        assert_eq!(def.initial, "planning");
    }

    #[test]
    fn builtin_dev_skills_nonempty_with_frontmatter() {
        assert_eq!(BUILTIN_DEV_SKILLS.len(), 5);
        for (slug, body) in BUILTIN_DEV_SKILLS {
            assert!(!body.is_empty(), "skill {slug} body empty");
            assert!(
                body.starts_with("---\n") && body.contains("name:"),
                "skill {slug} must start with frontmatter fence + name field"
            );
        }
    }

    #[test]
    fn builtin_dev_agents_nonempty_with_frontmatter() {
        assert_eq!(BUILTIN_DEV_AGENTS.len(), 3);
        for (role, body) in BUILTIN_DEV_AGENTS {
            assert!(!body.is_empty(), "agent {role} body empty");
            assert!(
                body.starts_with("---\n") && body.contains("name:"),
                "agent {role} must start with frontmatter fence + name field"
            );
        }
    }
}
